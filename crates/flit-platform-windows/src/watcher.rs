use crate::fs::{map_attrs, open_dir};
use crossbeam_channel::Sender;
use flit_core::listing::{ChangeKind, DirChange, Meta};
use flit_core::{name, FsPath, ProviderEvent, TabId};
use flit_platform::{PlatformError, Wake, Watcher};
use std::collections::HashMap;
use std::mem::offset_of;
use std::sync::{Arc, Mutex, MutexGuard};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows::Win32::Storage::FileSystem::*;
use windows::Win32::System::IO::{CancelIoEx, CreateIoCompletionPort, GetQueuedCompletionStatus, PostQueuedCompletionStatus, OVERLAPPED};
use windows::Win32::System::Threading::INFINITE;

const BUFFER: usize = 64 * 1024;
const QUIT: usize = 0;

type Record = FILE_NOTIFY_EXTENDED_INFORMATION;

struct Port(HANDLE);
unsafe impl Send for Port {}

pub struct DirWatcher {
    port: Port,
    state: Arc<Mutex<State>>,
}

#[derive(Default)]
struct State {
    slots: HashMap<usize, Watch>,
    by_tab: HashMap<TabId, usize>,
    latest: HashMap<TabId, (u64, bool)>,
    last_key: usize,
}

impl State {
    fn close(&mut self, key: usize) {
        if let Some(w) = self.slots.remove(&key) {
            unsafe {
                let _ = CloseHandle(w.dir);
            }
        }
    }

    fn stop(&mut self, key: usize) {
        if let Some(w) = self.slots.get_mut(&key) {
            w.cancel();
        }
    }

    fn stop_tab(&mut self, tab: TabId) {
        if let Some(k) = self.by_tab.remove(&tab) {
            self.stop(k);
        }
    }
}

fn lock(state: &Mutex<State>) -> MutexGuard<'_, State> {
    state.lock().unwrap_or_else(|e| e.into_inner())
}

impl Watcher for DirWatcher {
    fn new(events: Sender<ProviderEvent>, wake: Wake) -> Result<Self, PlatformError> {
        let port = unsafe { CreateIoCompletionPort(INVALID_HANDLE_VALUE, None, 0, 1) }.map_err(crate::err)?;
        let state = Arc::new(Mutex::new(State::default()));
        let (thread_port, thread_state) = (Port(port), state.clone());
        std::thread::Builder::new()
            .name("flit-watcher".into())
            .spawn(move || {
                let port = thread_port;
                run(port.0, &thread_state, events, wake)
            })
            .map_err(|e| PlatformError::new(-1, format!("cannot start watcher: {e}")))?;
        Ok(Self { port: Port(port), state })
    }

    fn watch(&mut self, tab: TabId, gen: u64, dir: Option<&FsPath>) {
        let rank = (gen, dir.is_none());
        {
            let mut st = lock(&self.state);
            if st.latest.get(&tab).is_some_and(|&newest| newest > rank) {
                return;
            }
            st.latest.insert(tab, rank);
            st.stop_tab(tab);
        }
        let Some(dir) = dir else { return };
        let Ok(h) = open_dir(dir, FILE_LIST_DIRECTORY.0, true) else { return };
        let mut st = lock(&self.state);
        st.last_key += 1;
        let key = st.last_key;
        let current = st.latest.get(&tab) == Some(&rank);
        if !current || unsafe { CreateIoCompletionPort(h, Some(self.port.0), key, 0) }.is_err() {
            unsafe {
                let _ = CloseHandle(h);
            }
            return;
        }
        st.stop_tab(tab);
        st.slots.insert(key, Watch { tab, gen, path: dir.clone(), dir: h, ov: Box::default(), buf: vec![0u64; BUFFER / 8], closing: false });
        if st.slots.get_mut(&key).is_some_and(Watch::issue) {
            st.by_tab.insert(tab, key);
        } else {
            st.close(key);
        }
    }
}

impl Drop for DirWatcher {
    fn drop(&mut self) {
        unsafe {
            let _ = PostQueuedCompletionStatus(self.port.0, 0, QUIT, None);
        }
    }
}

struct Watch {
    tab: TabId,
    gen: u64,
    path: FsPath,
    dir: HANDLE,
    ov: Box<OVERLAPPED>,
    buf: Vec<u64>,
    closing: bool,
}

unsafe impl Send for Watch {}

impl Watch {
    fn issue(&mut self) -> bool {
        let filter = FILE_NOTIFY_CHANGE_FILE_NAME
            | FILE_NOTIFY_CHANGE_DIR_NAME
            | FILE_NOTIFY_CHANGE_ATTRIBUTES
            | FILE_NOTIFY_CHANGE_SIZE
            | FILE_NOTIFY_CHANGE_LAST_WRITE
            | FILE_NOTIFY_CHANGE_CREATION;
        *self.ov = OVERLAPPED::default();
        unsafe { ReadDirectoryChangesExW(self.dir, self.buf.as_mut_ptr() as _, BUFFER as u32, false, filter, None, Some(&mut *self.ov), None, ReadDirectoryNotifyExtendedInformation).is_ok() }
    }

    fn cancel(&mut self) {
        self.closing = true;
        unsafe {
            let _ = CancelIoEx(self.dir, Some(&*self.ov));
        }
    }

    fn bytes(&self, len: usize) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.buf.as_ptr() as *const u8, len.min(BUFFER)) }
    }
}

pub(crate) struct Notice {
    pub kind: ChangeKind,
    pub name: Vec<u16>,
    pub meta: Meta,
}

fn u32_at(buf: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(buf.get(at..at + 4)?.try_into().ok()?))
}

fn i64_at(buf: &[u8], at: usize) -> Option<i64> {
    Some(i64::from_le_bytes(buf.get(at..at + 8)?.try_into().ok()?))
}

fn notice_at(buf: &[u8], off: usize) -> Option<(Option<Notice>, u32)> {
    let name_len = u32_at(buf, off + offset_of!(Record, FileNameLength))? as usize;
    let name_at = off + offset_of!(Record, FileName);
    let name = buf.get(name_at..name_at + name_len)?;
    let attrs = map_attrs(u32_at(buf, off + offset_of!(Record, FileAttributes))?, u32_at(buf, off + offset_of!(Record, Anonymous))?);
    let meta = Meta {
        size: i64_at(buf, off + offset_of!(Record, FileSize))? as u64,
        modified: i64_at(buf, off + offset_of!(Record, LastModificationTime))?,
        created: i64_at(buf, off + offset_of!(Record, CreationTime))?,
        attrs,
    };
    let kind = match FILE_ACTION(u32_at(buf, off + offset_of!(Record, Action))?) {
        FILE_ACTION_ADDED | FILE_ACTION_RENAMED_NEW_NAME => Some(ChangeKind::Added),
        FILE_ACTION_REMOVED | FILE_ACTION_RENAMED_OLD_NAME => Some(ChangeKind::Removed),
        FILE_ACTION_MODIFIED => Some(ChangeKind::Modified),
        _ => None,
    };
    let name = name.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    Some((kind.map(|kind| Notice { kind, name, meta }), u32_at(buf, off + offset_of!(Record, NextEntryOffset))?))
}

pub(crate) fn notices(buf: &[u8]) -> Vec<Notice> {
    let mut out = Vec::new();
    let mut off = 0usize;
    while let Some((notice, next)) = notice_at(buf, off) {
        out.extend(notice);
        if next == 0 {
            break;
        }
        off += next as usize;
    }
    out
}

pub(crate) fn short_alias(name: &[u16]) -> bool {
    let is = |c: u16, ch: u8| c == ch as u16;
    let (base, ext) = match name.iter().rposition(|&c| is(c, b'.')) {
        Some(dot) => (&name[..dot], &name[dot + 1..]),
        None => (name, &[][..]),
    };
    let Some(tilde) = base.iter().rposition(|&c| is(c, b'~')) else { return false };
    let digits = &base[tilde + 1..];
    base.len() <= 8 && ext.len() <= 3 && !base.iter().any(|&c| is(c, b'.')) && !digits.is_empty() && digits.iter().all(|&c| (b'0' as u16..=b'9' as u16).contains(&c))
}

fn long_name(dir: &FsPath, alias: &[u16]) -> Option<Vec<u16>> {
    let mut short = Vec::new();
    name::push_utf16(&mut short, alias);
    let path = dir.join(&short).to_wide(true);
    let need = unsafe { GetLongPathNameW(PCWSTR(path.as_ptr()), None) } as usize;
    if need == 0 {
        return None;
    }
    let mut full = vec![0u16; need];
    let n = unsafe { GetLongPathNameW(PCWSTR(path.as_ptr()), Some(&mut full)) } as usize;
    if n == 0 || n >= need {
        return None;
    }
    let start = full[..n].iter().rposition(|&c| c == b'\\' as u16).map_or(0, |i| i + 1);
    Some(full[start..n].to_vec()).filter(|n| !n.is_empty())
}

fn changes(dir: &FsPath, notices: Vec<Notice>) -> Option<Vec<DirChange>> {
    notices
        .into_iter()
        .map(|n| {
            let wide = match short_alias(&n.name) {
                false => n.name,
                true if n.kind == ChangeKind::Removed => return None,
                true => long_name(dir, &n.name)?,
            };
            let mut name = Vec::with_capacity(wide.len());
            name::push_utf16(&mut name, &wide);
            let meta = (n.kind != ChangeKind::Removed).then_some(n.meta);
            Some(DirChange { kind: n.kind, name, meta })
        })
        .collect()
}

fn run(port: HANDLE, state: &Mutex<State>, events: Sender<ProviderEvent>, wake: Wake) {
    let mut quitting = false;
    loop {
        let (mut n, mut key, mut ov) = (0u32, 0usize, std::ptr::null_mut::<OVERLAPPED>());
        let ok = unsafe { GetQueuedCompletionStatus(port, &mut n, &mut key, &mut ov, INFINITE) }.is_ok();
        if !ok && ov.is_null() {
            break;
        }
        let mut st = lock(state);
        if key == QUIT {
            quitting = true;
            let keys: Vec<usize> = st.slots.keys().copied().collect();
            keys.into_iter().for_each(|k| st.stop(k));
        } else if quitting {
            st.close(key);
        }
        if quitting {
            if st.slots.is_empty() {
                break;
            }
            continue;
        }
        let Some(w) = st.slots.get_mut(&key) else { continue };
        if w.closing {
            st.close(key);
            continue;
        }
        let (tab, gen) = (w.tab, w.gen);
        let found = (ok && n > 0).then(|| (notices(w.bytes(n as usize)), w.path.clone()));
        if !ok || !w.issue() {
            st.by_tab.retain(|_, v| *v != key);
            st.close(key);
        }
        drop(st);
        let ev = match found.and_then(|(list, dir)| changes(&dir, list)) {
            Some(changes) => ProviderEvent::Changes { tab, gen, changes },
            None => ProviderEvent::Rescan { tab, gen },
        };
        let _ = events.send(ev);
        wake();
    }
    unsafe {
        let _ = CloseHandle(port);
    }
}

#[cfg(test)]
#[path = "../test/watcher.rs"]
mod tests;
