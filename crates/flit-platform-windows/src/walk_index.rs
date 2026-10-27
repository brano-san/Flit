use crate::fs::{map_attrs, open_dir};
use crate::mft::{query_journal, usn_records, UsnRecord};
use crossbeam_channel::{bounded, unbounded};
use flit_core::listing::attr;
use flit_core::path::PathKind;
use flit_core::{name, FsPath};
use flit_platform::{IndexSource, PlatformError};
use flit_search::index::{Checkpoint, IndexRecord, IndexSink};
use flit_search::proto::{VolumeInfo, VolumeState};
use std::collections::HashSet;
use std::mem::{offset_of, size_of};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use windows::core::PCWSTR;
use windows::Wdk::Storage::FileSystem::{FileIdBothDirectoryInformation, NtQueryDirectoryFile, FILE_ID_BOTH_DIR_INFORMATION};
use windows::Win32::Foundation::*;
use windows::Win32::Storage::FileSystem::*;
use windows::Win32::System::Threading::{CreateEventW, ResetEvent, SetEvent, WaitForMultipleObjects, INFINITE};
use windows::Win32::System::Ioctl::{FSCTL_READ_UNPRIVILEGED_USN_JOURNAL, READ_USN_JOURNAL_DATA_V0, USN_JOURNAL_DATA_V0, USN_REASON_FILE_DELETE, USN_REASON_RENAME_NEW_NAME};
use windows::Win32::System::IO::{CancelIoEx, DeviceIoControl, GetOverlappedResult, IO_STATUS_BLOCK, OVERLAPPED};

const DIR_BUFFER: usize = 64 * 1024;
const WATCH_BUFFER: usize = 512 * 1024;
const JOURNAL_BUFFER: usize = 1 << 20;
const DRIVE_FIXED: u32 = 3;
const WALK_THREADS: usize = 8;
const SYNTHETIC: u64 = 1 << 63;

type Record = FILE_NOTIFY_EXTENDED_INFORMATION;

pub(crate) struct Handle(pub HANDLE);
unsafe impl Send for Handle {}
unsafe impl Sync for Handle {}

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

#[derive(Default)]
struct Queue {
    buffers: Vec<Vec<u8>>,
    overflow: bool,
}

struct RootWatch {
    queue: Arc<Mutex<Queue>>,
    stop: Arc<Handle>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl RootWatch {
    fn start(root: &FsPath) -> Option<Self> {
        let dir = Handle(open_dir(root, FILE_LIST_DIRECTORY.0, true).ok()?);
        let stop = Arc::new(Handle(unsafe { CreateEventW(None, true, false, PCWSTR::null()) }.ok()?));
        let event = Handle(unsafe { CreateEventW(None, true, false, PCWSTR::null()) }.ok()?);
        let queue = Arc::new(Mutex::new(Queue::default()));
        let (armed_tx, armed) = bounded(1);
        let (q, stop_h) = (queue.clone(), stop.clone());
        let thread = std::thread::Builder::new()
            .name("flit-walk-watch".into())
            .spawn(move || watch_loop(dir, event, stop_h, q, armed_tx))
            .ok()?;
        armed.recv().ok().filter(|&ok| ok)?;
        Some(Self { queue, stop, thread: Some(thread) })
    }

    fn take(&self) -> Queue {
        std::mem::take(&mut *self.queue.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

impl Drop for RootWatch {
    fn drop(&mut self) {
        unsafe {
            let _ = SetEvent(self.stop.0);
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn watch_loop(dir: Handle, event: Handle, stop: Arc<Handle>, queue: Arc<Mutex<Queue>>, armed: crossbeam_channel::Sender<bool>) {
    let filter = FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_DIR_NAME | FILE_NOTIFY_CHANGE_ATTRIBUTES | FILE_NOTIFY_CHANGE_SIZE | FILE_NOTIFY_CHANGE_LAST_WRITE | FILE_NOTIFY_CHANGE_CREATION;
    let mut buf = vec![0u64; WATCH_BUFFER / 8];
    let mut first = true;
    loop {
        let mut ov = OVERLAPPED { hEvent: event.0, ..Default::default() };
        unsafe {
            let _ = ResetEvent(event.0);
        }
        let issued = unsafe { ReadDirectoryChangesExW(dir.0, buf.as_mut_ptr() as _, WATCH_BUFFER as u32, true, filter, None, Some(&mut ov), None, ReadDirectoryNotifyExtendedInformation) };
        if first {
            let _ = armed.send(issued.is_ok());
            first = false;
        }
        if issued.is_err() {
            queue.lock().unwrap_or_else(|e| e.into_inner()).overflow = true;
            return;
        }
        let woke = unsafe { WaitForMultipleObjects(&[event.0, stop.0], false, INFINITE) };
        let mut n = 0u32;
        if woke != WAIT_OBJECT_0 {
            unsafe {
                let _ = CancelIoEx(dir.0, Some(&ov));
                let _ = GetOverlappedResult(dir.0, &ov, &mut n, true);
            }
            return;
        }
        let ok = unsafe { GetOverlappedResult(dir.0, &ov, &mut n, false) }.is_ok();
        let mut q = queue.lock().unwrap_or_else(|e| e.into_inner());
        if ok && n > 0 {
            let bytes = unsafe { std::slice::from_raw_parts(buf.as_ptr() as *const u8, (n as usize).min(WATCH_BUFFER)) };
            q.buffers.push(bytes.to_vec());
        } else {
            q.overflow = true;
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Notice {
    pub action: FILE_ACTION,
    pub id: u64,
    pub parent: u64,
    pub path: Vec<u16>,
    pub size: u64,
    pub modified: i64,
    pub attrs: u32,
}

fn u32_at(buf: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(buf.get(at..at + 4)?.try_into().ok()?))
}

fn i64_at(buf: &[u8], at: usize) -> Option<i64> {
    Some(i64::from_le_bytes(buf.get(at..at + 8)?.try_into().ok()?))
}

fn notice_at(buf: &[u8], off: usize) -> Option<(Notice, u32)> {
    let len = u32_at(buf, off + offset_of!(Record, FileNameLength))? as usize;
    let at = off + offset_of!(Record, FileName);
    let path = buf.get(at..at + len)?.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    let notice = Notice {
        action: FILE_ACTION(u32_at(buf, off + offset_of!(Record, Action))?),
        id: i64_at(buf, off + offset_of!(Record, FileId))? as u64,
        parent: i64_at(buf, off + offset_of!(Record, ParentFileId))? as u64,
        path,
        size: i64_at(buf, off + offset_of!(Record, FileSize))? as u64,
        modified: i64_at(buf, off + offset_of!(Record, LastModificationTime))?,
        attrs: map_attrs(u32_at(buf, off + offset_of!(Record, FileAttributes))?, u32_at(buf, off + offset_of!(Record, Anonymous))?),
    };
    Some((notice, u32_at(buf, off + offset_of!(Record, NextEntryOffset))?))
}

pub(crate) fn notices(buf: &[u8]) -> Option<Vec<Notice>> {
    crate::watcher::records(buf, |b, off| notice_at(b, off).map(|(n, next)| (Some(n), next)))
}

struct Job {
    path: FsPath,
    id: u64,
}

#[derive(Default)]
struct Batch {
    parent: u64,
    names: Vec<u8>,
    items: Vec<(u64, usize, u64, i64, u32)>,
}

impl Batch {
    fn subdirs(&self) -> impl Iterator<Item = (&[u8], u64)> {
        let starts = std::iter::once(0).chain(self.items.iter().map(|it| it.1));
        self.items.iter().zip(starts).filter(|(it, _)| it.4 & attr::DIR != 0 && it.4 & attr::LINK == 0).map(|(it, start)| (&self.names[start..it.1], it.0))
    }

    fn feed<S: IndexSink>(&self, sink: &mut S) {
        let mut start = 0;
        for &(id, end, size, modified, attrs) in &self.items {
            sink.upsert(&IndexRecord { id, parent: self.parent, name: &self.names[start..end], size, modified, attrs });
            start = end;
        }
    }
}

fn read_dir_ids(job: &Job, synthetic: &AtomicU64, subdirs: &mut Vec<Job>) -> Batch {
    let Ok(h) = open_dir(&job.path, FILE_LIST_DIRECTORY.0, false) else { return Batch { parent: job.id, ..Default::default() } };
    let batch = list(&Handle(h), job.id, synthetic);
    subdirs.extend(batch.subdirs().map(|(name, id)| Job { path: job.path.join(name), id }));
    batch
}

fn list(h: &Handle, parent: u64, synthetic: &AtomicU64) -> Batch {
    let mut batch = Batch { parent, ..Default::default() };
    let mut buf = vec![0u64; DIR_BUFFER / 8];
    let mut first = true;
    loop {
        let mut iosb = IO_STATUS_BLOCK::default();
        let st = unsafe { NtQueryDirectoryFile(h.0, None, None, None, &mut iosb, buf.as_mut_ptr() as _, DIR_BUFFER as u32, FileIdBothDirectoryInformation, false, None, first) };
        if st.is_err() || st == STATUS_NO_MORE_FILES {
            break;
        }
        first = false;
        let base = buf.as_ptr() as *const u8;
        let mut off = 0usize;
        loop {
            let e = unsafe { base.add(off) } as *const FILE_ID_BOTH_DIR_INFORMATION;
            let (next, wide, id, size, modified, attrs) = unsafe {
                let at = base.add(off + offset_of!(FILE_ID_BOTH_DIR_INFORMATION, FileName)) as *const u16;
                let wide = std::slice::from_raw_parts(at, (*e).FileNameLength as usize / 2);
                ((*e).NextEntryOffset, wide, (*e).FileId as u64, (*e).EndOfFile as u64, (*e).LastWriteTime, map_attrs((*e).FileAttributes, (*e).EaSize))
            };
            let dots = wide == [b'.' as u16] || wide == [b'.' as u16, b'.' as u16];
            if !dots {
                let id = if id == 0 { SYNTHETIC | synthetic.fetch_add(1, Ordering::Relaxed) } else { id };
                name::push_utf16(&mut batch.names, wide);
                batch.items.push((id, batch.names.len(), size, modified, attrs));
            }
            if next == 0 {
                break;
            }
            off += next as usize;
        }
    }
    batch
}

fn walk<S: IndexSink>(root: &FsPath, root_id: u64, sink: &mut S, synthetic: &AtomicU64) {
    let (job_tx, job_rx) = unbounded::<Job>();
    let (out_tx, out_rx) = unbounded::<Batch>();
    let pending = AtomicUsize::new(1);
    let stop = AtomicBool::new(false);
    let _ = job_tx.send(Job { path: root.clone(), id: root_id });
    std::thread::scope(|s| {
        for _ in 0..WALK_THREADS {
            let (job_tx, job_rx, out_tx, pending, stop) = (job_tx.clone(), job_rx.clone(), out_tx.clone(), &pending, &stop);
            s.spawn(move || {
                let mut subdirs = Vec::new();
                while pending.load(Ordering::Acquire) > 0 && !stop.load(Ordering::Relaxed) {
                    let Ok(job) = job_rx.recv_timeout(Duration::from_millis(10)) else { continue };
                    let batch = read_dir_ids(&job, synthetic, &mut subdirs);
                    pending.fetch_add(subdirs.len(), Ordering::AcqRel);
                    for sub in subdirs.drain(..) {
                        let _ = job_tx.send(sub);
                    }
                    let _ = out_tx.send(batch);
                    pending.fetch_sub(1, Ordering::AcqRel);
                }
            });
        }
        drop(out_tx);
        let mut done = 0u64;
        for batch in out_rx {
            batch.feed(sink);
            done += 1;
            sink.progress(done, done + pending.load(Ordering::Relaxed) as u64);
            if sink.cancelled() {
                stop.store(true, Ordering::Relaxed);
            }
        }
    });
}

pub(crate) fn file_id(path: &FsPath) -> Option<u64> {
    let h = Handle(open_dir(path, FILE_READ_ATTRIBUTES.0, false).ok()?);
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    unsafe { GetFileInformationByHandle(h.0, &mut info) }.ok()?;
    Some(((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64)
}

pub(crate) fn volume_id(root: &FsPath) -> String {
    let d = root.display();
    if root.kind() == PathKind::Drive && root.is_root() {
        d.trim_end_matches('\\').to_owned()
    } else {
        d.into_owned()
    }
}

fn relative(root: &FsPath, rel: &[u16]) -> FsPath {
    let mut bytes = Vec::with_capacity(rel.len());
    name::push_utf16(&mut bytes, rel);
    bytes.split(|&b| b == b'\\').filter(|c| !c.is_empty()).fold(root.clone(), |p, c| p.join(c))
}

enum Live {
    Journal(Handle),
    Watch(Option<RootWatch>),
}

fn open_journal(root: &FsPath) -> Option<(Handle, USN_JOURNAL_DATA_V0)> {
    let h = Handle(open_dir(root, FILE_READ_ATTRIBUTES.0, false).ok()?);
    let j = query_journal(h.0).ok()?;
    Some((h, j))
}

fn open_id(hint: &Handle, id: u64) -> Option<Handle> {
    let desc = FILE_ID_DESCRIPTOR { dwSize: size_of::<FILE_ID_DESCRIPTOR>() as u32, Type: FileIdType, Anonymous: FILE_ID_DESCRIPTOR_0 { FileId: id as i64 } };
    let share = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;
    Some(Handle(unsafe { OpenFileById(hint.0, &desc, FILE_LIST_DIRECTORY.0, share, None, FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT) }.ok()?))
}

#[derive(Default)]
struct Touched {
    dirs: Vec<u64>,
    removed: Vec<u64>,
    moved: Vec<(u64, u64)>,
    seen: HashSet<u64>,
}

impl Touched {
    fn push(&mut self, r: &UsnRecord<'_>) {
        if r.reason & USN_REASON_FILE_DELETE != 0 {
            self.removed.push(r.frn);
        } else if r.reason & USN_REASON_RENAME_NEW_NAME != 0 {
            self.moved.push((r.frn, r.parent));
        }
        if self.seen.insert(r.parent) {
            self.dirs.push(r.parent);
        }
    }
}

fn read_journal(dir: &Handle, from: Checkpoint, touched: &mut Touched) -> Option<i64> {
    let mut next = from.next;
    let mut buf = vec![0u64; JOURNAL_BUFFER / 8];
    loop {
        let input = READ_USN_JOURNAL_DATA_V0 { StartUsn: next, ReasonMask: u32::MAX, ReturnOnlyOnClose: 0, Timeout: 0, BytesToWaitFor: 0, UsnJournalID: from.journal };
        let mut got = 0u32;
        unsafe { DeviceIoControl(dir.0, FSCTL_READ_UNPRIVILEGED_USN_JOURNAL, Some(&input as *const _ as _), size_of::<READ_USN_JOURNAL_DATA_V0>() as u32, Some(buf.as_mut_ptr() as _), JOURNAL_BUFFER as u32, Some(&mut got), None) }.ok()?;
        let bytes = unsafe { std::slice::from_raw_parts(buf.as_ptr() as *const u8, (got as usize).min(JOURNAL_BUFFER)) };
        let after = i64::from_le_bytes(bytes.get(..8)?.try_into().ok()?);
        usn_records(&bytes[8..]).for_each(|r| touched.push(&r));
        if bytes.len() <= 8 || after <= next {
            return Some(next.max(after));
        }
        next = after;
    }
}

fn relist<S: IndexSink>(dir: &Handle, touched: Touched, sink: &mut S, synthetic: &AtomicU64) {
    for &id in &touched.removed {
        sink.remove(id);
    }
    for &(id, parent) in &touched.moved {
        if !sink.knows(parent) {
            sink.remove(id);
        }
    }
    let mut queue: Vec<u64> = touched.dirs.into_iter().filter(|&d| sink.knows(d)).collect();
    let mut listed = HashSet::new();
    while let Some(d) = queue.pop() {
        if !listed.insert(d) {
            continue;
        }
        let Some(h) = open_id(dir, d) else { continue };
        let batch = list(&h, d, synthetic);
        queue.extend(batch.subdirs().map(|(_, id)| id).filter(|&id| !sink.knows(id)));
        batch.feed(sink);
    }
}

pub struct WalkIndex {
    roots: Vec<FsPath>,
    live: Vec<(String, Live)>,
    synthetic: AtomicU64,
}

impl WalkIndex {
    pub fn with_roots(roots: Vec<FsPath>) -> Self {
        Self { roots, live: Vec::new(), synthetic: AtomicU64::new(1) }
    }

    fn follow<S: IndexSink>(&self, dir: &Handle, from: Checkpoint, sink: &mut S) -> Option<Checkpoint> {
        let j = query_journal(dir.0).ok()?;
        if j.UsnJournalID != from.journal || from.next < j.FirstUsn || from.next < j.LowestValidUsn {
            return None;
        }
        if from.next >= j.NextUsn {
            return Some(from);
        }
        let mut touched = Touched::default();
        let next = read_journal(dir, from, &mut touched)?;
        relist(dir, touched, sink, &self.synthetic);
        Some(Checkpoint { journal: from.journal, next })
    }

    fn root(&self, volume: &str) -> Result<FsPath, PlatformError> {
        self.roots.iter().find(|r| volume_id(r) == volume).cloned().ok_or_else(|| PlatformError::new(-1, format!("unknown volume {volume}")))
    }

    fn apply<S: IndexSink>(&self, root: &FsPath, list: Vec<Notice>, sink: &mut S) -> bool {
        let old: HashSet<u64> = list.iter().filter(|n| n.action == FILE_ACTION_RENAMED_OLD_NAME).map(|n| n.id).collect();
        let new: HashSet<u64> = list.iter().filter(|n| n.action == FILE_ACTION_RENAMED_NEW_NAME).map(|n| n.id).collect();
        for n in list {
            if n.id == 0 || n.parent == 0 {
                return false;
            }
            match n.action {
                FILE_ACTION_ADDED | FILE_ACTION_MODIFIED | FILE_ACTION_RENAMED_NEW_NAME => {
                    let start = n.path.iter().rposition(|&c| c == b'\\' as u16).map_or(0, |i| i + 1);
                    let mut leaf = Vec::new();
                    name::push_utf16(&mut leaf, &n.path[start..]);
                    if leaf.is_empty() {
                        continue;
                    }
                    sink.upsert(&IndexRecord { id: n.id, parent: n.parent, name: &leaf, size: n.size, modified: n.modified, attrs: n.attrs });
                    let arrived = n.action == FILE_ACTION_ADDED || (n.action == FILE_ACTION_RENAMED_NEW_NAME && !old.contains(&n.id));
                    if arrived && n.attrs & attr::DIR != 0 && n.attrs & attr::LINK == 0 {
                        walk(&relative(root, &n.path), n.id, sink, &self.synthetic);
                    }
                }
                FILE_ACTION_REMOVED => sink.remove(n.id),
                FILE_ACTION_RENAMED_OLD_NAME if !new.contains(&n.id) => sink.remove(n.id),
                _ => {}
            }
        }
        true
    }
}

pub(crate) fn fixed_drives() -> Vec<FsPath> {
    let mask = unsafe { GetLogicalDrives() };
    (0..26u8)
        .filter(|i| mask & (1 << i) != 0)
        .filter_map(|i| {
            let root: Vec<u16> = [b'A' + i, b':', b'\\'].iter().map(|&c| c as u16).chain(Some(0)).collect();
            (unsafe { GetDriveTypeW(PCWSTR(root.as_ptr())) } == DRIVE_FIXED).then(|| FsPath::parse(&format!("{}:\\", (b'A' + i) as char))).flatten()
        })
        .collect()
}

impl IndexSource for WalkIndex {
    fn new() -> Result<Self, PlatformError> {
        Ok(Self::with_roots(fixed_drives()))
    }

    fn volumes(&self) -> Result<Vec<VolumeInfo>, PlatformError> {
        Ok(self.roots.iter().map(|r| VolumeInfo { id: volume_id(r), label: String::new(), file_system: String::new(), files: 0, state: VolumeState::Scanning { progress: 0.0 } }).collect())
    }

    fn scan<S: IndexSink>(&mut self, volume: &str, sink: &mut S) -> Result<Checkpoint, PlatformError> {
        let root = self.root(volume)?;
        self.live.retain(|(v, _)| v != volume);
        let root_id = file_id(&root).ok_or_else(|| PlatformError::new(-1, format!("cannot open {}", root.display())))?;
        let (live, cp) = match open_journal(&root) {
            Some((h, j)) => (Live::Journal(h), Checkpoint { journal: j.UsnJournalID, next: j.NextUsn }),
            None => (Live::Watch(RootWatch::start(&root)), Checkpoint { journal: root_id, next: 0 }),
        };
        self.live.push((volume.to_owned(), live));
        sink.upsert(&IndexRecord { id: root_id, parent: root_id, name: b".", size: 0, modified: 0, attrs: attr::DIR });
        walk(&root, root_id, sink, &self.synthetic);
        Ok(cp)
    }

    fn changes<S: IndexSink>(&mut self, volume: &str, from: Checkpoint, sink: &mut S) -> Result<Option<Checkpoint>, PlatformError> {
        let root = self.root(volume)?;
        if !self.live.iter().any(|(v, _)| v == volume) {
            match open_journal(&root) {
                Some((h, j)) if j.UsnJournalID == from.journal => self.live.push((volume.to_owned(), Live::Journal(h))),
                _ => return Ok(None),
            }
        }
        let Some((_, live)) = self.live.iter().find(|(v, _)| v == volume) else { return Ok(None) };
        let watch = match live {
            Live::Journal(dir) => return Ok(self.follow(dir, from, sink)),
            Live::Watch(w) => w,
        };
        let Some(w) = watch else { return Ok(Some(from)) };
        let q = w.take();
        if q.overflow {
            return Ok(None);
        }
        for buf in q.buffers {
            let Some(list) = notices(&buf) else { return Ok(None) };
            if !self.apply(&root, list, sink) {
                return Ok(None);
            }
        }
        Ok(Some(from))
    }
}

#[cfg(test)]
#[path = "../test/walk_index.rs"]
mod tests;
