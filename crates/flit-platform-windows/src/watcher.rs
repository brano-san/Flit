use crate::fs::{map_attrs, open_dir};
use crossbeam_channel::{unbounded, Receiver, Sender};
use flit_core::listing::{ChangeKind, DirChange, Meta};
use flit_core::{name, FsPath, ProviderEvent, TabId};
use flit_platform::{PlatformError, Wake, Watcher};
use std::collections::HashMap;
use windows::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows::Win32::Storage::FileSystem::*;
use windows::Win32::System::IO::{CancelIoEx, CreateIoCompletionPort, GetQueuedCompletionStatus, PostQueuedCompletionStatus, OVERLAPPED};
use windows::Win32::System::Threading::INFINITE;

const BUFFER: usize = 64 * 1024;
const COMMAND: usize = 0;

struct Port(HANDLE);
unsafe impl Send for Port {}

enum Cmd {
    Set { tab: TabId, gen: u64, dir: Option<FsPath> },
    Quit,
}

pub struct DirWatcher {
    port: Port,
    cmds: Sender<Cmd>,
}

impl DirWatcher {
    fn post(&self, cmd: Cmd) {
        let _ = self.cmds.send(cmd);
        unsafe {
            let _ = PostQueuedCompletionStatus(self.port.0, 0, COMMAND, None);
        }
    }
}

impl Watcher for DirWatcher {
    fn new(events: Sender<ProviderEvent>, wake: Wake) -> Result<Self, PlatformError> {
        let port = unsafe { CreateIoCompletionPort(INVALID_HANDLE_VALUE, None, 0, 1) }.map_err(crate::err)?;
        let (cmds, rx) = unbounded();
        let thread_port = Port(port);
        std::thread::Builder::new()
            .name("flit-watcher".into())
            .spawn(move || {
                let port = thread_port;
                run(port.0, rx, events, wake)
            })
            .map_err(|e| PlatformError::new(-1, format!("cannot start watcher: {e}")))?;
        Ok(Self { port: Port(port), cmds })
    }

    fn watch(&mut self, tab: TabId, gen: u64, dir: Option<&FsPath>) {
        self.post(Cmd::Set { tab, gen, dir: dir.cloned() });
    }
}

impl Drop for DirWatcher {
    fn drop(&mut self) {
        self.post(Cmd::Quit);
    }
}

struct Watch {
    tab: TabId,
    gen: u64,
    dir: HANDLE,
    ov: Box<OVERLAPPED>,
    buf: Vec<u64>,
    closing: bool,
}

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

    fn cancel(&mut self) -> bool {
        self.closing = true;
        unsafe { CancelIoEx(self.dir, Some(&*self.ov)).is_ok() }
    }

    fn changes(&self, len: usize) -> Vec<DirChange> {
        let mut out = Vec::new();
        let base = self.buf.as_ptr() as *const u8;
        let mut off = 0usize;
        while off + size_of::<FILE_NOTIFY_EXTENDED_INFORMATION>() <= len {
            let e = unsafe { &*(base.add(off) as *const FILE_NOTIFY_EXTENDED_INFORMATION) };
            let wide = unsafe { std::slice::from_raw_parts(e.FileName.as_ptr(), e.FileNameLength as usize / 2) };
            let mut n = Vec::with_capacity(wide.len());
            name::push_utf16(&mut n, wide);
            let meta = Meta { size: e.FileSize as u64, modified: e.LastModificationTime, created: e.CreationTime, attrs: map_attrs(e.FileAttributes) };
            let change = match e.Action {
                FILE_ACTION_ADDED | FILE_ACTION_RENAMED_NEW_NAME => Some(DirChange { kind: ChangeKind::Added, name: n, meta: Some(meta) }),
                FILE_ACTION_REMOVED | FILE_ACTION_RENAMED_OLD_NAME => Some(DirChange { kind: ChangeKind::Removed, name: n, meta: None }),
                FILE_ACTION_MODIFIED => Some(DirChange { kind: ChangeKind::Modified, name: n, meta: Some(meta) }),
                _ => None,
            };
            out.extend(change);
            if e.NextEntryOffset == 0 {
                break;
            }
            off += e.NextEntryOffset as usize;
        }
        out
    }
}

fn run(port: HANDLE, rx: Receiver<Cmd>, events: Sender<ProviderEvent>, wake: Wake) {
    let mut slots: HashMap<usize, Watch> = HashMap::new();
    let mut by_tab: HashMap<TabId, usize> = HashMap::new();
    let mut next_key = 1usize;
    let close = |slots: &mut HashMap<usize, Watch>, key: usize| {
        if let Some(w) = slots.remove(&key) {
            unsafe {
                let _ = CloseHandle(w.dir);
            }
        }
    };
    loop {
        let (mut n, mut key, mut ov) = (0u32, 0usize, std::ptr::null_mut::<OVERLAPPED>());
        let ok = unsafe { GetQueuedCompletionStatus(port, &mut n, &mut key, &mut ov, INFINITE) }.is_ok();
        if key == COMMAND {
            while let Ok(cmd) = rx.try_recv() {
                let (tab, gen, dir) = match cmd {
                    Cmd::Quit => return,
                    Cmd::Set { tab, gen, dir } => (tab, gen, dir),
                };
                if let Some(k) = by_tab.remove(&tab) {
                    if !slots.get_mut(&k).is_some_and(Watch::cancel) {
                        close(&mut slots, k);
                    }
                }
                let Some(dir) = dir else { continue };
                let Ok(h) = open_dir(&dir, FILE_LIST_DIRECTORY.0, true) else { continue };
                let k = next_key;
                next_key += 1;
                if unsafe { CreateIoCompletionPort(h, Some(port), k, 0) }.is_err() {
                    unsafe {
                        let _ = CloseHandle(h);
                    }
                    continue;
                }
                let mut w = Watch { tab, gen, dir: h, ov: Box::default(), buf: vec![0u64; BUFFER / 8], closing: false };
                if w.issue() {
                    slots.insert(k, w);
                    by_tab.insert(tab, k);
                } else {
                    unsafe {
                        let _ = CloseHandle(h);
                    }
                }
            }
            continue;
        }
        let Some(w) = slots.get_mut(&key) else { continue };
        if w.closing {
            close(&mut slots, key);
            continue;
        }
        let ev = if !ok || n == 0 {
            ProviderEvent::Rescan { tab: w.tab, gen: w.gen }
        } else {
            ProviderEvent::Changes { tab: w.tab, gen: w.gen, changes: w.changes(n as usize) }
        };
        let _ = events.send(ev);
        wake();
        if !ok || !w.issue() {
            by_tab.retain(|_, v| *v != key);
            close(&mut slots, key);
        }
    }
}
