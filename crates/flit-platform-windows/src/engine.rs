use crate::fs::{map_attrs, NtFs};
use flit_core::listing::{attr, Listing, ListingBuilder, Meta};
use flit_core::provider::{Conflict, Failure, FileOp, LinkKind, OpError, Recover, Resolve, Stage};
use flit_core::sort::CaseMode;
use flit_core::{limits, FsPath};
use flit_core::i18n::S;
use flit_core::tr_en;
use flit_platform::{Fs, Job, PlatformError, PlatformErrorKind};
use std::ffi::c_void;
use windows::core::PCWSTR;
use windows::Win32::Foundation::*;
use windows::Win32::Storage::FileSystem::*;
use windows::Win32::UI::Shell::{SHChangeNotify, SHCNE_UPDATEDIR, SHCNF_FLUSHNOWAIT, SHCNF_PATHW};

type Pairs = Vec<(FsPath, FsPath)>;
type MakeLink<'a> = &'a dyn Fn(LinkKind, &FsPath, &FsPath) -> windows::core::Result<()>;
type ElevateLinks<'a> = &'a dyn Fn(&[(FsPath, FsPath)]) -> windows::core::Result<()>;
use crate::fileops::{Recycle, SystemRecycle};

#[cfg(test)]
#[path = "../test/engine.rs"]
mod tests;

#[cfg(test)]
#[path = "../test/replay.rs"]
mod replay_tests;

const PROTECTED: u32 = FILE_ATTRIBUTE_READONLY.0 | FILE_ATTRIBUTE_HIDDEN.0 | FILE_ATTRIBUTE_SYSTEM.0;

pub(crate) struct Outcome {
    pub items: Pairs,
    pub error: Option<PlatformError>,
    pub rest: Option<FileOp>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Copy,
    Move,
    Across,
}

enum Res {
    Whole(FsPath),
    Part(Pairs),
    Nothing,
}

#[derive(Default)]
struct Tally {
    entries: u64,
    bytes: u64,
    links: bool,
}

struct Chunk<'a> {
    job: &'a Job,
    base: u64,
    #[cfg(test)]
    fail_after_chunk: Option<fn(u64) -> Option<WIN32_ERROR>>,
    #[cfg(test)]
    failure: std::cell::Cell<Option<WIN32_ERROR>>,
}

fn wide(p: &FsPath) -> Vec<u16> {
    p.to_wide(true)
}

fn code(e: &windows::core::Error) -> WIN32_ERROR {
    WIN32_ERROR::from_error(e).unwrap_or(ERROR_GEN_FAILURE)
}

fn reason(path: &FsPath, e: &windows::core::Error) -> String {
    let said = crate::error_reason(e).trim_end().to_owned();
    match code(e) {
        ERROR_SHARING_VIOLATION | ERROR_LOCK_VIOLATION => flit_core::fmt::in_use(&said, &crate::busy::holders(path)).unwrap_or(said),
        _ => said,
    }
}

fn same(a: &FsPath, b: &FsPath) -> bool {
    a.as_bytes().eq_ignore_ascii_case(b.as_bytes())
}

fn inside(path: &FsPath, folder: &FsPath) -> bool {
    let (p, f) = (path.as_bytes(), folder.as_bytes());
    p.len() > f.len() && p[..f.len()].eq_ignore_ascii_case(f) && (folder.is_root() || p[f.len()] == b'\\')
}

fn ticks(t: FILETIME) -> i64 {
    ((t.dwHighDateTime as u64) << 32 | t.dwLowDateTime as u64) as i64
}

pub(crate) fn undo_stamp(path: &FsPath) -> std::io::Result<(flit_core::undo::Stamp, bool)> {
    use flit_core::path::PathKind;
    if !matches!(path.kind(), PathKind::Drive | PathKind::Unc | PathKind::Verbatim) {
        return Err(std::io::Error::from(std::io::ErrorKind::Unsupported));
    }
    let share = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;
    let handle = crate::walk_index::Handle(unsafe { CreateFileW(PCWSTR(wide(path).as_ptr()), FILE_READ_ATTRIBUTES.0, share, None, OPEN_EXISTING, FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT, None) }.map_err(io_error)?);
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    unsafe { GetFileInformationByHandle(handle.0, &mut info) }.map_err(io_error)?;
    let file = u64::from(info.nFileIndexHigh) << 32 | u64::from(info.nFileIndexLow);
    if file == 0 {
        return Err(std::io::Error::from(std::io::ErrorKind::Unsupported));
    }
    let stamp = flit_core::undo::Stamp {
        size: u64::from(info.nFileSizeHigh) << 32 | u64::from(info.nFileSizeLow),
        modified: (i128::from(ticks(info.ftLastWriteTime)) - 116_444_736_000_000_000) * 100,
        device: u64::from(info.dwVolumeSerialNumber),
        file,
        access: None,
    };
    let directory = info.dwFileAttributes & (FILE_ATTRIBUTE_DIRECTORY.0 | FILE_ATTRIBUTE_REPARSE_POINT.0) == FILE_ATTRIBUTE_DIRECTORY.0;
    Ok((stamp, directory))
}

fn io_error(error: windows::core::Error) -> std::io::Error {
    std::io::Error::from_raw_os_error(code(&error).0 as i32)
}

pub(crate) fn undo_children(dir: &FsPath) -> std::io::Result<Vec<FsPath>> {
    use std::os::windows::ffi::OsStrExt;
    std::fs::read_dir(crate::fileops::std_path(dir))?.map(|entry| {
        let name = entry?.file_name();
        let mut bytes = Vec::new();
        flit_core::name::push_utf16(&mut bytes, &name.encode_wide().collect::<Vec<_>>());
        Ok(dir.join(&bytes))
    }).collect()
}

pub(crate) fn link_stat(p: &FsPath) -> Option<Meta> {
    if !p.is_root() {
        return stat(p);
    }
    let mut data = WIN32_FILE_ATTRIBUTE_DATA::default();
    unsafe { GetFileAttributesExW(PCWSTR(wide(p).as_ptr()), GetFileExInfoStandard, &mut data as *mut _ as _) }.ok()?;
    Some(Meta { size: (data.nFileSizeHigh as u64) << 32 | data.nFileSizeLow as u64, modified: ticks(data.ftLastWriteTime), created: ticks(data.ftCreationTime), attrs: map_attrs(data.dwFileAttributes, 0) })
}

pub(crate) fn stat(p: &FsPath) -> Option<Meta> {
    let mut d = WIN32_FIND_DATAW::default();
    let h = unsafe { FindFirstFileExW(PCWSTR(wide(p).as_ptr()), FindExInfoBasic, &mut d as *mut _ as *mut c_void, FindExSearchNameMatch, None, FIND_FIRST_EX_FLAGS(0)) }.ok()?;
    unsafe {
        let _ = FindClose(h);
    }
    let tag = if d.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 { d.dwReserved0 } else { 0 };
    let size = (d.nFileSizeHigh as u64) << 32 | d.nFileSizeLow as u64;
    Some(Meta { size, modified: ticks(d.ftLastWriteTime), created: ticks(d.ftCreationTime), attrs: map_attrs(d.dwFileAttributes, tag) })
}

fn is_dir(m: &Meta) -> bool {
    m.attrs & attr::DIR != 0
}

fn is_dir_link(m: &Meta) -> bool {
    is_dir(m) && m.attrs & attr::LINK != 0
}

fn exists(p: &FsPath) -> bool {
    unsafe { GetFileAttributesW(PCWSTR(wide(p).as_ptr())) != INVALID_FILE_ATTRIBUTES }
}

fn children(dir: &FsPath) -> Result<Listing, flit_core::FsError> {
    let mut go = |_: &ListingBuilder<'_>| true;
    let mut b = ListingBuilder::new(CaseMode::Sensitive, &mut go);
    NtFs.read_dir(dir, &mut b)?;
    Ok(std::mem::take(&mut b.listing))
}

fn numbered(dest: &FsPath, name: &[u8], dir: bool, suffix: &[u8], plain: bool) -> FsPath {
    let ext = if dir { 0 } else { flit_core::name::ext_len(name) };
    let (stem, ext) = if ext == 0 || ext + 1 >= name.len() { (name, &b""[..]) } else { name.split_at(name.len() - ext - 1) };
    let mut k = if plain { 1u32 } else { 2 };
    loop {
        let mut n = [stem, suffix].concat();
        if k > 1 {
            n.extend_from_slice(format!(" ({k})").as_bytes());
        }
        n.extend_from_slice(ext);
        let path = dest.join(&n);
        if !exists(&path) {
            return path;
        }
        k += 1;
    }
}

pub(crate) fn free_name(taken: &FsPath, dir: bool) -> FsPath {
    let parent = taken.parent().unwrap_or_else(|| taken.clone());
    numbered(&parent, taken.file_name(), dir, b"", false)
}

pub(crate) fn copy_name(item: &FsPath, dest: &FsPath, dir: bool) -> FsPath {
    numbered(dest, item.file_name(), dir, b" - Copy", true)
}

fn partial(to: &FsPath) -> FsPath {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = to.parent().unwrap_or_else(|| to.clone());
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    dir.join(format!(".flit-stage-{}-{id}.flit-partial", std::process::id()).as_bytes())
}

struct LinkStage {
    dir: FsPath,
    path: FsPath,
    made: bool,
    directory: bool,
    pending: Option<flit_platform::pending::Record>,
}

impl LinkStage {
    fn reserve(dir: FsPath, kind: LinkKind, directory: bool, state: Option<&std::path::Path>) -> windows::core::Result<Self> {
        unsafe { CreateDirectoryW(PCWSTR(wide(&dir).as_ptr()), None) }?;
        let _ = set_hidden(&dir, true);
        let path = dir.join(if kind == LinkKind::Shortcut { b"link.lnk" } else { b"link" });
        let pending = state.and_then(|state| {
            let (stamp, _) = undo_stamp(&dir).ok()?;
            flit_platform::pending::partial(state, &dir, stamp).ok()
        });
        Ok(Self { dir, path, made: false, directory, pending })
    }

    fn new(dst: &FsPath, kind: LinkKind, directory: bool, state: Option<&std::path::Path>) -> windows::core::Result<Self> {
        loop {
            match Self::reserve(partial(dst), kind, directory, state) {
                Err(error) if matches!(code(&error), ERROR_ALREADY_EXISTS | ERROR_FILE_EXISTS) => {}
                result => return result,
            }
        }
    }

    fn place(&mut self, destination: &FsPath, replace: bool) -> windows::core::Result<()> {
        move_file(&self.path, destination, if replace { MOVEFILE_REPLACE_EXISTING } else { MOVE_FILE_FLAGS(0) })?;
        self.made = false;
        Ok(())
    }
}

struct Waiting {
    item: FsPath,
    dst: FsPath,
    replace: bool,
    stage: LinkStage,
    meta: Meta,
    dir: bool,
}

impl Drop for LinkStage {
    fn drop(&mut self) {
        if self.made {
            if self.directory { let _ = remove_dir(&self.path); } else { let _ = delete_file(&self.path); }
        }
        if let (Ok(()), Some(pending)) = (remove_dir(&self.dir), self.pending.take()) {
            pending.finish();
        }
    }
}

fn link_name(item: &FsPath, kind: LinkKind) -> Vec<u8> {
    let stem: Vec<u8> = item.file_name().iter().copied().filter(|&c| c != b':' && c != b'\\').collect();
    let mut name = flit_core::name::link_name(&stem);
    if kind == LinkKind::Shortcut {
        name.extend_from_slice(b".lnk");
    }
    name
}

fn move_file(from: &FsPath, to: &FsPath, flags: MOVE_FILE_FLAGS) -> windows::core::Result<()> {
    unsafe { MoveFileExW(PCWSTR(wide(from).as_ptr()), PCWSTR(wide(to).as_ptr()), flags) }
}

fn create_dir(dst: &FsPath, template: &FsPath) -> windows::core::Result<()> {
    unsafe { CreateDirectoryExW(PCWSTR(wide(template).as_ptr()), PCWSTR(wide(dst).as_ptr()), None) }
}

fn set_hidden(path: &FsPath, hidden: bool) -> windows::core::Result<()> {
    let name = wide(path);
    let attributes = unsafe { GetFileAttributesW(PCWSTR(name.as_ptr())) };
    if attributes == INVALID_FILE_ATTRIBUTES {
        return Err(windows::core::Error::from_thread());
    }
    let set = if hidden { attr::HIDDEN } else { 0 };
    let clear = if hidden { 0 } else { attr::HIDDEN };
    let next = crate::fileops::win_attributes(attributes, set, clear);
    unsafe { SetFileAttributesW(PCWSTR(name.as_ptr()), FILE_FLAGS_AND_ATTRIBUTES(next)) }
}

fn unprotect(p: &FsPath) -> bool {
    let w = wide(p);
    let a = unsafe { GetFileAttributesW(PCWSTR(w.as_ptr())) };
    a != INVALID_FILE_ATTRIBUTES && a & PROTECTED != 0 && unsafe { SetFileAttributesW(PCWSTR(w.as_ptr()), FILE_FLAGS_AND_ATTRIBUTES(a & !PROTECTED)) }.is_ok()
}

fn delete_file(p: &FsPath) -> windows::core::Result<()> {
    let w = wide(p);
    match unsafe { DeleteFileW(PCWSTR(w.as_ptr())) } {
        Err(e) if code(&e) == ERROR_ACCESS_DENIED && unprotect(p) => unsafe { DeleteFileW(PCWSTR(w.as_ptr())) },
        r => r,
    }
}

fn remove_dir(p: &FsPath) -> windows::core::Result<()> {
    let w = wide(p);
    match unsafe { RemoveDirectoryW(PCWSTR(w.as_ptr())) } {
        Err(e) if code(&e) == ERROR_ACCESS_DENIED && unprotect(p) => unsafe { RemoveDirectoryW(PCWSTR(w.as_ptr())) },
        r => r,
    }
}

fn free_space(dir: &FsPath) -> Option<u64> {
    let mut free = 0u64;
    unsafe { GetDiskFreeSpaceExW(PCWSTR(wide(dir).as_ptr()), Some(&mut free as *mut u64), None, None) }.ok()?;
    Some(free)
}

pub(crate) fn volume_file_system(p: &FsPath) -> Option<String> {
    let mut volume = [0u16; 300];
    unsafe { GetVolumePathNameW(PCWSTR(wide(p).as_ptr()), &mut volume) }.ok()?;
    let mut fs = [0u16; 64];
    unsafe { GetVolumeInformationW(PCWSTR(volume.as_ptr()), None, None, None, None, Some(&mut fs)) }.ok()?;
    Some(String::from_utf16_lossy(&fs[..fs.iter().position(|&c| c == 0)?]))
}

fn make_parents(p: &FsPath) -> bool {
    p.parent().is_some_and(|dir| std::fs::create_dir_all(crate::fileops::std_path(&dir)).is_ok())
}

unsafe extern "system" fn on_chunk(msg: *const COPYFILE2_MESSAGE, ctx: *const c_void) -> COPYFILE2_MESSAGE_ACTION {
    let c = &*(ctx as *const Chunk);
    if c.job.cancelled() {
        return COPYFILE2_PROGRESS_CANCEL;
    }
    if (*msg).Type == COPYFILE2_CALLBACK_CHUNK_STARTED && !c.job.pace_copy((*msg).Info.ChunkStarted.uliChunkSize) {
        return COPYFILE2_PROGRESS_CANCEL;
    }
    if (*msg).Type == COPYFILE2_CALLBACK_CHUNK_FINISHED {
        let done = (*msg).Info.ChunkFinished.uliTotalBytesTransferred;
        c.job.update(|p| p.bytes = c.base + done);
        #[cfg(test)]
        if let Some(error) = c.fail_after_chunk.and_then(|fail| fail(done)) {
            c.failure.set(Some(error));
            return COPYFILE2_PROGRESS_CANCEL;
        }
    }
    COPYFILE2_PROGRESS_CONTINUE
}

fn copy_file(from: &FsPath, to: &FsPath, size: u64, chunk: &Chunk<'_>) -> windows::core::Result<()> {
    let params = COPYFILE2_EXTENDED_PARAMETERS {
        dwSize: size_of::<COPYFILE2_EXTENDED_PARAMETERS>() as u32,
        dwCopyFlags: COPY_FILE_FAIL_IF_EXISTS,
        pProgressRoutine: Some(on_chunk),
        pvCallbackContext: chunk as *const Chunk as *mut c_void,
        ..Default::default()
    };
    let result = unsafe { CopyFile2(PCWSTR(wide(from).as_ptr()), PCWSTR(wide(to).as_ptr()), Some(&params)) };
    #[cfg(test)]
    if let Some(error) = chunk.failure.get() {
        return Err(error.to_hresult().into());
    }
    result?;
    chunk.job.update(|p| {
        p.bytes = chunk.base + size;
        p.items += 1;
    });
    Ok(())
}

const QUICK_MAX: u64 = 1 << 20;
const QUICK_MIN: usize = 8;
const QUICK_THREADS: usize = 4;

fn quick_copy(from: &FsPath, to: &FsPath) -> windows::core::Result<()> {
    let params = COPYFILE2_EXTENDED_PARAMETERS { dwSize: size_of::<COPYFILE2_EXTENDED_PARAMETERS>() as u32, dwCopyFlags: COPY_FILE_FAIL_IF_EXISTS, ..Default::default() };
    unsafe { CopyFile2(PCWSTR(wide(from).as_ptr()), PCWSTR(wide(to).as_ptr()), Some(&params)) }
}

fn quick(job: &Job, files: &[(FsPath, FsPath, u64)], across: bool) -> Vec<bool> {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};
    let done: Vec<AtomicBool> = files.iter().map(|_| AtomicBool::new(false)).collect();
    if files.len() < QUICK_MIN {
        return vec![false; files.len()];
    }
    let next = AtomicUsize::new(0);
    std::thread::scope(|s| {
        for _ in 0..QUICK_THREADS {
            s.spawn(|| loop {
                let k = next.fetch_add(1, Relaxed);
                let Some((from, to, size)) = files.get(k) else { break };
                if !job.pace_copy(*size) {
                    break;
                }
                if quick_copy(from, to).is_err() {
                    continue;
                }
                if across && delete_file(from).is_err() {
                    let _ = delete_file(to);
                    continue;
                }
                job.update(|p| {
                    p.items += 1;
                    p.bytes += size;
                });
                done[k].store(true, Relaxed);
            });
        }
    });
    done.into_iter().map(AtomicBool::into_inner).collect()
}

pub(crate) struct Engine<'a> {
    job: &'a Job,
    done: Pairs,
    failed: usize,
    first: Option<String>,
    first_code: Option<i64>,
    fatal: bool,
    elevate: bool,
    dirs: Vec<FsPath>,
    limit: Option<(Vec<u8>, Option<u64>)>,
    make_link: MakeLink<'a>,
    elevate_links: ElevateLinks<'a>,
    recycle: &'a dyn Recycle,
    pending_state: Option<std::path::PathBuf>,
    replacement_stage: Option<LinkStage>,
    #[cfg(test)]
    fail_after_chunk: Option<fn(u64) -> Option<WIN32_ERROR>>,
}

impl<'a> Engine<'a> {
    pub(crate) fn new(job: &'a Job) -> Self {
        Self { job, done: Vec::new(), failed: 0, first: None, first_code: None, fatal: false, elevate: false, dirs: Vec::new(), limit: None, make_link: &crate::links::create, elevate_links: &crate::links::elevated, recycle: &SystemRecycle, pending_state: pending_state(), replacement_stage: None, #[cfg(test)] fail_after_chunk: None }
    }

    fn stopped(&self) -> bool {
        self.fatal || self.elevate || self.job.cancelled()
    }

    fn touch(&mut self, dir: Option<FsPath>) {
        if let Some(d) = dir.filter(|d| !self.dirs.iter().any(|k| same(k, d))) {
            self.dirs.push(d);
        }
    }

    fn fail(&mut self, path: &FsPath, e: &windows::core::Error, top: bool) {
        let c = code(e);
        if top && c == ERROR_ACCESS_DENIED {
            self.elevate = true;
            return;
        }
        if matches!(c, ERROR_DISK_FULL | ERROR_HANDLE_DISK_FULL) {
            self.fatal = true;
        }
        self.noted(Some(i64::from(e.code().0)), format!("{}: {}", path.display(), reason(path, e)));
    }

    fn note(&mut self, msg: String) {
        self.noted(None, msg);
    }

    fn noted(&mut self, code: Option<i64>, msg: String) {
        self.failed += 1;
        if self.first.is_none() {
            self.first = Some(msg);
            self.first_code = code;
        }
    }

    fn left_staged(&mut self, original: &FsPath, saved: &FsPath, error: &windows::core::Error) {
        self.first = Some(tr_en!(S::ErrorsOpsSavedFile, original = original.display(), saved = saved.display(), error = reason(saved, error)));
        self.failed += 1;
    }

    fn trouble(&mut self, path: &FsPath, e: &windows::core::Error, top: bool) -> bool {
        if self.job.cancelled() {
            return false;
        }
        let c = code(e);
        if !self.job.asks_failures() || (top && c == ERROR_ACCESS_DENIED) {
            self.fail(path, e, top);
            return false;
        }
        let fatal = matches!(c, ERROR_DISK_FULL | ERROR_HANDLE_DISK_FULL);
        let error = reason(path, e);
        match self.job.fail(Failure { path: path.clone(), error: error.clone().into(), fatal }) {
            Some(Recover::Retry) => true,
            Some(Recover::Skip) => {
                self.fatal |= fatal;
                self.noted(Some(i64::from(e.code().0)), format!("{}: {}", path.display(), error));
                false
            }
            None => false,
        }
    }

    fn error(&self) -> Option<PlatformError> {
        if self.job.cancelled() && self.first.is_none() {
            return Some(PlatformError::cancelled(ERROR_CANCELLED.0 as i64, S::ErrorsEngineCancelled.en()));
        }
        let first = self.first.clone()?;
        Some(crate::win_error(self.first_code.unwrap_or(-1), if self.failed > 1 { tr_en!(S::ErrorsEngineFailed, count = self.failed, error = first) } else { first }))
    }

    fn running(&self, total: Tally) {
        self.job.update(|p| {
            p.stage = Stage::Running;
            p.total_items = total.entries;
            p.total_bytes = total.bytes;
        });
    }

    fn tally(&self, dir: &FsPath, t: &mut Tally) {
        let Ok(l) = children(dir) else { return };
        for i in 0..l.len() as u32 {
            if self.job.cancelled() {
                return;
            }
            let m = l.entries[i as usize].meta();
            t.entries += 1;
            if is_dir_link(&m) {
                t.links = true;
            } else if is_dir(&m) {
                self.tally(&dir.join(l.name(i)), t);
            } else {
                t.bytes += m.size;
            }
        }
    }

    fn measure(&self, item: &FsPath, m: &Meta, deep: bool, total: &mut Tally) -> bool {
        total.entries += 1;
        total.bytes += if is_dir(m) { 0 } else { m.size };
        if !deep || !is_dir(m) {
            return true;
        }
        let mut t = Tally { links: is_dir_link(m), ..Default::default() };
        if !t.links {
            self.tally(item, &mut t);
        }
        total.entries += t.entries;
        total.bytes += t.bytes;
        !t.links
    }

    pub(crate) fn room(&mut self, dest: &FsPath, need: u64) -> bool {
        let free = match self.job.room(dest, need, || free_space(dest)) {
            Ok(()) => return true,
            Err(None) => return false,
            Err(Some(free)) => free,
        };
        let (mut needed, mut available) = (String::new(), String::new());
        flit_core::fmt::size(&mut needed, need);
        flit_core::fmt::size(&mut available, free);
        self.note(tr_en!(S::ErrorsEngineNoRoom, folder = dest.display(), need = needed, free = available));
        false
    }

    fn size_limit(&mut self, dst: &FsPath) -> Option<u64> {
        let root = dst.root_bytes();
        match &self.limit {
            Some((r, l)) if r.as_slice() == root => *l,
            _ => {
                let l = volume_file_system(dst).and_then(|fs| limits::max_file_size(&fs));
                self.limit = Some((root.to_vec(), l));
                l
            }
        }
    }

    fn ask(&self, src: &FsPath, from: Meta, dst: &FsPath) -> Option<Resolve> {
        let to = stat(dst).unwrap_or_default();
        let choice = self.job.resolve(Conflict { source: src.clone(), target: dst.clone(), from, to })?;
        Some(if choice == Resolve::Replace && is_dir(&from) != is_dir(&to) { Resolve::KeepBoth } else { choice })
    }

    fn chunk(&self) -> Chunk<'a> {
        Chunk {
            job: self.job,
            base: self.job.progress().bytes,
            #[cfg(test)]
            fail_after_chunk: self.fail_after_chunk,
            #[cfg(test)]
            failure: Default::default(),
        }
    }

    pub(crate) fn copy_file(&self, from: &FsPath, to: &FsPath, size: u64) -> windows::core::Result<()> {
        copy_file(from, to, size, &self.chunk())
    }

    fn replace_file(&mut self, src: &FsPath, dst: &FsPath, size: u64, mode: Mode) -> windows::core::Result<()> {
        let chunk = self.chunk();
        self.replace_staged(src, dst, mode, |stage| copy_file(src, stage, size, &chunk))
    }

    fn replace_staged(&mut self, src: &FsPath, dst: &FsPath, mode: Mode, copy: impl FnOnce(&FsPath) -> windows::core::Result<()>) -> windows::core::Result<()> {
        let mut stage = match self.replacement_stage.take() {
            Some(stage) if stage.dir.parent() == dst.parent() => stage,
            _ => LinkStage::new(dst, LinkKind::Hard, false, self.pending_state.as_deref())?,
        };
        if mode != Mode::Move {
            stage.made = true;
            if let Err(error) = copy(&stage.path) {
                if matches!(code(&error), ERROR_ALREADY_EXISTS | ERROR_FILE_EXISTS) {
                    stage.made = false;
                }
                return Err(error);
            }
        }
        if self.job.cancelled() {
            return Err(ERROR_CANCELLED.to_hresult().into());
        }
        let placed_stamp = (mode == Mode::Across).then(|| undo_stamp(&stage.path).ok()).flatten();

        let old = stage.dir.join(b"replaced");
        let recycled = self.recycle.direct(dst);
        if recycled.is_none() {
            if self.job.cancelled() {
                return Err(ERROR_CANCELLED.to_hresult().into());
            }
            move_file(dst, &old, MOVE_FILE_FLAGS(0))?;
        }
        let saved = recycled.as_ref().unwrap_or(&old);
        let restore_old = || {
            if let Some(backup) = &recycled {
                self.recycle.restore(backup, dst)
            } else {
                move_file(&old, dst, MOVE_FILE_FLAGS(0)).is_ok()
            }
        };
        let placed = if mode == Mode::Move {
            move_file(src, dst, MOVE_FILE_FLAGS(0))
        } else {
            stage.place(dst, false)
        };
        if let Err(error) = placed {
            if !restore_old() {
                self.left_staged(dst, saved, &error);
                self.job.replaced(dst.clone(), recycled);
                self.fatal = true;
            }
            return Err(error);
        }
        if mode == Mode::Across {
            if let Err(error) = delete_file(src) {
                let displaced = placed_stamp.is_some_and(|stamp| undo_stamp(dst).is_ok_and(|current| current == stamp))
                    && move_file(dst, &stage.path, MOVE_FILE_FLAGS(0)).is_ok();
                stage.made = displaced;
                if !displaced || !restore_old() {
                    self.left_staged(dst, saved, &error);
                    self.job.replaced(dst.clone(), recycled);
                    self.fatal = true;
                }
                return Err(error);
            }
        }
        if mode == Mode::Move {
            self.job.update(|p| p.items += 1);
        }
        let clean = if recycled.is_none() {
            unprotect(&old);
            match delete_file(&old) {
                Ok(()) => true,
                Err(error) => {
                    self.left_staged(dst, &old, &error);
                    false
                }
            }
        } else {
            true
        };
        if clean {
            self.replacement_stage = Some(stage);
        }
        self.job.replaced(dst.clone(), recycled);
        Ok(())
    }

    fn skip(&self, m: &Meta) {
        self.job.update(|p| {
            p.items += 1;
            p.bytes += if is_dir(m) { 0 } else { m.size };
        });
    }

    fn transfer(&mut self, src: &FsPath, mut dst: FsPath, m: Meta, mode: Mode, top: bool) -> Res {
        if self.stopped() || src.as_bytes() == dst.as_bytes() || (mode != Mode::Move && same(src, &dst)) {
            return Res::Nothing;
        }
        let dir = is_dir(&m);
        if dir && inside(&dst, src) {
            self.note(tr_en!(S::ErrorsEngineInside, path = src.display()));
            return Res::Nothing;
        }
        if mode != Mode::Move && is_dir_link(&m) {
            self.note(tr_en!(S::ErrorsEngineLinkSkipped, path = src.display()));
            return Res::Nothing;
        }
        if !dir && mode != Mode::Move {
            if let Some(limit) = self.size_limit(&dst).filter(|&l| limits::exceeds(m.size, Some(l))) {
                let unasked = self.job.ctl().policy().is_some();
                if self.job.resolve(limits::over_limit(src, &dst, m.size, limit)).is_some() {
                    self.skip(&m);
                }
                if unasked {
                    let mut most = String::new();
                    flit_core::fmt::size(&mut most, limit);
                    self.note(tr_en!(S::ErrorsOpsExtraOverLimit, path = src.display(), limit = most));
                }
                return Res::Nothing;
            }
        }
        let group = top && self.job.has_companions(src);
        let (mut replace, mut retried) = (false, false);
        loop {
            let occupied = group && !replace && !same(src, &dst) && exists(&dst);
            if !occupied && group {
                if let Some(error) = self.job.companion_collision(src, &dst) {
                    self.note(error);
                    return Res::Nothing;
                }
            }
            if replace {
                match self.replace_file(src, &dst, m.size, mode) {
                    Ok(()) => return Res::Whole(dst),
                    Err(e) if !self.stopped() && mode == Mode::Move && code(&e) == ERROR_NOT_SAME_DEVICE => return self.transfer(src, dst, m, Mode::Across, top),
                    Err(e) if !self.stopped() && self.trouble(src, &e, false) => continue,
                    Err(_) => return Res::Nothing,
                }
            }
            let tried = if occupied {
                Err(ERROR_ALREADY_EXISTS.into())
            } else {
                let _group = self.job.defer_companion_cancellation(src);
                match mode {
                    Mode::Move => move_file(src, &dst, MOVE_FILE_FLAGS(0)),
                    _ if dir => create_dir(&dst, src),
                    _ => self.copy_file(src, &dst, m.size),
                }
            };
            let Err(e) = tried else { break };
            if self.job.cancelled() {
                return Res::Nothing;
            }
            let c = code(&e);
            if mode == Mode::Move && c == ERROR_NOT_SAME_DEVICE {
                return self.transfer(src, dst, m, Mode::Across, top);
            }
            if c == ERROR_PATH_NOT_FOUND && mode != Mode::Copy && !retried && exists(src) && make_parents(&dst) {
                retried = true;
                continue;
            }
            if exists(&dst) {
                match self.ask(src, m, &dst) {
                    Some(Resolve::KeepBoth) => dst = free_name(&dst, dir),
                    Some(Resolve::Replace) if dir => return self.merge(src, &dst, mode),
                    Some(Resolve::Replace) => replace = true,
                    Some(Resolve::Skip | Resolve::SkipSame | Resolve::ReplaceOlder) => {
                        self.skip(&m);
                        return Res::Nothing;
                    }
                    None => return Res::Nothing,
                }
                continue;
            }
            if self.trouble(src, &e, top && !group) {
                continue;
            }
            return Res::Nothing;
        }
        let _group = self.job.defer_companion_cancellation(src);
        match (mode, dir) {
            (Mode::Move, _) => {
                self.job.update(|p| p.items += 1);
                Res::Whole(dst)
            }
            (Mode::Across, false) => loop {
                match delete_file(src) {
                    Ok(()) => break Res::Whole(dst),
                    Err(e) if self.trouble(src, &e, top && !group) => {}
                    Err(_) => {
                        let _ = delete_file(&dst);
                        break Res::Nothing;
                    }
                }
            },
            (_, false) => Res::Whole(dst),
            (_, true) => {
                self.job.update(|p| p.items += 1);
                let (parts, complete) = self.children(src, &dst, mode);
                match mode {
                    Mode::Copy => Res::Whole(dst),
                    _ if complete && remove_dir(src).is_ok() => Res::Whole(dst),
                    _ => Res::Part(parts),
                }
            }
        }
    }

    fn merge(&mut self, src: &FsPath, dst: &FsPath, mode: Mode) -> Res {
        let (parts, complete) = self.children(src, dst, mode);
        if mode != Mode::Copy && complete {
            let _ = remove_dir(src);
        }
        Res::Part(parts)
    }

    fn children(&mut self, src: &FsPath, dst: &FsPath, mode: Mode) -> (Pairs, bool) {
        let l = match children(src) {
            Ok(l) => l,
            Err(e) => {
                self.note(tr_en!(S::ErrorsEngineUnreadable, path = src.display(), code = e.code));
                return (Vec::new(), false);
            }
        };
        let (mut out, mut complete) = (Vec::new(), true);
        let small: Vec<u32> = match mode {
            Mode::Move => Vec::new(),
            _ => (0..l.len() as u32).filter(|&i| !is_dir(&l.entries[i as usize].meta()) && l.entries[i as usize].size <= QUICK_MAX).collect(),
        };
        let files: Vec<(FsPath, FsPath, u64)> = small.iter().map(|&i| (src.join(l.name(i)), dst.join(l.name(i)), l.entries[i as usize].size)).collect();
        let mut copied = vec![false; l.len()];
        for (&i, ok) in small.iter().zip(quick(self.job, &files, mode == Mode::Across)) {
            copied[i as usize] = ok;
        }
        for i in 0..l.len() as u32 {
            if self.stopped() {
                return (out, false);
            }
            let name = l.name(i);
            let s = src.join(name);
            if copied[i as usize] {
                out.push((s, dst.join(name)));
                continue;
            }
            match self.transfer(&s, dst.join(name), l.entries[i as usize].meta(), mode, false) {
                Res::Whole(d) => out.push((s, d)),
                Res::Part(v) => {
                    out.extend(v);
                    complete = false;
                }
                Res::Nothing => complete = false,
            }
        }
        (out, complete)
    }

    fn keep(&mut self, from: &FsPath, r: Res) {
        match r {
            Res::Whole(to) => self.done.push((from.clone(), to)),
            Res::Part(v) => self.done.extend(v),
            Res::Nothing => {}
        }
    }

    fn stat_all(&mut self, items: &[FsPath]) -> Vec<Option<Meta>> {
        items
            .iter()
            .map(|p| {
                let m = stat(p);
                if m.is_none() {
                    self.note(tr_en!(S::ErrorsEngineGone, path = p.display()));
                }
                m
            })
            .collect()
    }

    fn offer_elevation(&mut self, item: &FsPath) -> bool {
        if !self.job.asks_failures() || self.job.cancelled() {
            return false;
        }
        match self.job.fail(Failure { path: item.clone(), error: OpError::new(PlatformErrorKind::SymlinkPrivilege, S::ErrorsEngineSymlinkPrivilege.en()), fatal: false }) {
            Some(Recover::Retry) => !self.job.cancelled(),
            Some(_) => false,
            None => {
                self.job.ctl().cancel();
                false
            }
        }
    }

    fn place_link(&mut self, mut waiting: Waiting) {
        loop {
            if self.job.cancelled() {
                return;
            }
            match waiting.stage.place(&waiting.dst, waiting.replace) {
                Ok(()) => {
                    self.done.push((waiting.item, waiting.dst));
                    self.job.update(|p| p.items += 1);
                    return;
                }
                Err(error) => {
                    let code = code(&error);
                    if matches!(code, ERROR_ALREADY_EXISTS | ERROR_FILE_EXISTS) && !waiting.replace {
                        match self.ask(&waiting.item, waiting.meta, &waiting.dst) {
                            Some(Resolve::KeepBoth) => waiting.dst = free_name(&waiting.dst, waiting.dir),
                            Some(Resolve::Replace) if !waiting.dir && stat(&waiting.dst).is_some_and(|m| !is_dir(&m)) => waiting.replace = true,
                            _ => return,
                        }
                    } else if !self.trouble(&waiting.dst, &error, false) {
                        return;
                    }
                }
            }
        }
    }

    fn elevated_links(&mut self, waiting: Vec<Waiting>) {
        if waiting.is_empty() || self.job.cancelled() {
            return;
        }
        let pairs: Pairs = waiting.iter().map(|w| (w.item.clone(), w.stage.path.clone())).collect();
        let result = (self.elevate_links)(&pairs);
        for mut waiting in waiting {
            if stat(&waiting.stage.path).is_some() {
                waiting.stage.made = true;
                self.place_link(waiting);
                continue;
            }
            match &result {
                Err(error) if !matches!(code(error), ERROR_CANCELLED | ERROR_PRIVILEGE_NOT_HELD) => self.noted(Some(code(error).0 as i64), format!("{}: {}", waiting.dst.display(), reason(&waiting.dst, error))),
                _ => self.noted(Some(ERROR_PRIVILEGE_NOT_HELD.0 as i64), S::ErrorsEngineSymlinkPrivilege.en().to_owned()),
            }
        }
    }

    fn links(&mut self, items: &[FsPath], dest: &FsPath, kind: LinkKind) -> Option<FileOp> {
        self.running(Tally { entries: items.len() as u64, ..Default::default() });
        self.touch(Some(dest.clone()));
        let mut elevation = None;
        let mut waiting = Vec::new();
        for item in items {
            if self.stopped() {
                break;
            }
            let Some(meta) = link_stat(item) else {
                self.note(tr_en!(S::ErrorsEngineMissing, path = item.display()));
                continue;
            };
            let name = link_name(item, kind);
            let mut dst = dest.join(&name);
            let dir = is_dir(&meta) && kind != LinkKind::Shortcut;
            let output_meta = Meta { attrs: if kind == LinkKind::Shortcut { meta.attrs & !attr::DIR } else { meta.attrs }, ..meta };
            let mut replace = false;
            if exists(&dst) {
                match self.ask(item, output_meta, &dst) {
                    Some(Resolve::KeepBoth) => dst = free_name(&dst, dir),
                    Some(Resolve::Replace) if !dir && stat(&dst).is_some_and(|m| !is_dir(&m)) => replace = true,
                    Some(Resolve::Replace) => dst = free_name(&dst, dir),
                    Some(_) => { self.job.update(|p| p.items += 1); continue; }
                    None => break,
                }
            }
            loop {
                if self.job.cancelled() {
                    break;
                }
                let mut stage = match LinkStage::new(&dst, kind, dir, self.pending_state.as_deref()) {
                    Ok(stage) => stage,
                    Err(error) if self.trouble(&dst, &error, false) => continue,
                    Err(_) => break,
                };
                let created = if kind == LinkKind::Symbolic && elevation == Some(true) { Err(ERROR_PRIVILEGE_NOT_HELD.to_hresult().into()) } else { (self.make_link)(kind, item, &stage.path) };
                if let Err(error) = &created {
                    if kind == LinkKind::Symbolic && code(error) == ERROR_PRIVILEGE_NOT_HELD {
                        if elevation.is_none() {
                            elevation = Some(self.offer_elevation(item));
                        }
                        if elevation == Some(true) {
                            waiting.push(Waiting { item: item.clone(), dst, replace, stage, meta: output_meta, dir });
                        } else if !self.job.cancelled() {
                            self.noted(Some(ERROR_PRIVILEGE_NOT_HELD.0 as i64), S::ErrorsEngineSymlinkPrivilege.en().to_owned());
                        }
                        break;
                    }
                }
                let made = created.and_then(|()| {
                    stage.made = true;
                    if self.job.cancelled() {
                        return Err(ERROR_CANCELLED.to_hresult().into());
                    }
                    stage.place(&dst, replace)
                });
                if let Err(error) = made {
                    let code = code(&error);
                    if matches!(code, ERROR_ALREADY_EXISTS | ERROR_FILE_EXISTS) && !replace {
                        match self.ask(item, output_meta, &dst) {
                            Some(Resolve::KeepBoth) => { dst = free_name(&dst, dir); continue; }
                            Some(Resolve::Replace) if !dir && stat(&dst).is_some_and(|m| !is_dir(&m)) => { replace = true; continue; }
                            _ => break,
                        }
                    }
                    if code == ERROR_CANCELLED {
                        self.job.ctl().cancel();
                    } else if self.trouble(&dst, &error, false) {
                        continue;
                    }
                    break;
                }
                self.done.push((item.clone(), dst));
                self.job.update(|p| p.items += 1);
                break;
            }
        }
        self.elevated_links(waiting);
        None
    }

    fn keep_group(&mut self, source: &FsPath, result: Res, mode: Mode) {
        let companions = match &result {
            Res::Whole(target) => self.job.companion_targets(source, target),
            _ => Vec::new(),
        };
        self.keep(source, result);
        if companions.is_empty() { return; }
        let job = Job::detached(Resolve::Skip);
        let mut engine = Engine::new(&job);
        for (from, to) in companions {
            engine.touch(from.parent());
            engine.touch(to.parent());
            let Some(meta) = stat(&from) else {
                engine.fail(&from, &ERROR_FILE_NOT_FOUND.into(), false);
                continue;
            };
            let result = engine.transfer(&from, to.clone(), meta, mode, false);
            if matches!(result, Res::Nothing) && exists(&to) {
                engine.note(flit_platform::companion_collision(&to));
            }
            engine.keep(&from, result);
        }
        engine.notify();
        if let Some(error) = engine.error() {
            self.note(error.to_string());
        }
        self.done.extend(engine.done);
    }

    fn copy(&mut self, items: &[FsPath], dest: &FsPath) -> Option<FileOp> {
        self.job.update(|p| p.stage = Stage::Preparing);
        let metas = self.stat_all(items);
        let mut total = Tally::default();
        let native: Vec<bool> = items.iter().zip(&metas).map(|(p, m)| m.is_none_or(|m| self.measure(p, &m, true, &mut total))).collect();
        self.job.set_copy_limit_available(native.iter().zip(&metas).any(|(&supported, meta)| supported && meta.is_some()));
        if !self.room(dest, total.bytes) {
            return None;
        }
        if !self.job.preflight_conflicts(|| {
            let items = items.iter().zip(&native).filter(|(_, supported)| **supported).map(|(item, _)| item.clone()).collect();
            flit_platform::conflicts::collect(&crate::NtFs, &FileOp::Copy { items, dest: dest.clone() }, self.job.ctl())
        }) {
            return None;
        }
        self.running(total);
        self.touch(Some(dest.clone()));
        let mut shell = Vec::new();
        let small: Vec<usize> = if self.job.has_companion_groups() {
            Vec::new()
        } else {
            (0..items.len()).filter(|&k| native[k] && metas[k].is_some_and(|m| !is_dir(&m) && m.size <= QUICK_MAX)
                && !items[k].parent().is_some_and(|p| same(&p, dest))).collect()
        };
        let files: Vec<(FsPath, FsPath, u64)> = small.iter().map(|&k| (items[k].clone(), dest.join(items[k].file_name()), metas[k].map_or(0, |m| m.size))).collect();
        let mut copied = vec![false; items.len()];
        for (&k, ok) in small.iter().zip(quick(self.job, &files, false)) {
            copied[k] = ok;
        }
        for (k, item) in items.iter().enumerate() {
            if self.stopped() {
                break;
            }
            let Some(m) = metas[k] else { continue };
            if copied[k] {
                self.done.push((item.clone(), dest.join(item.file_name())));
                continue;
            }
            if !native[k] {
                shell.push(item.clone());
                continue;
            }
            let dup = item.parent().is_some_and(|p| same(&p, dest));
            let dst = if dup { copy_name(item, dest, is_dir(&m)) } else { dest.join(item.file_name()) };
            let r = self.transfer(item, dst, m, Mode::Copy, true);
            self.keep_group(item, r, Mode::Copy);
            if self.elevate {
                self.elevate = false;
                if self.job.has_companion_groups() {
                    shell.push(item.clone());
                } else {
                    shell.extend(items[k..].iter().cloned());
                    break;
                }
            }
        }
        (!shell.is_empty()).then(|| FileOp::Copy { items: shell, dest: dest.clone() })
    }

    fn moves(&mut self, pairs: &[(FsPath, FsPath)]) -> Option<FileOp> {
        self.job.update(|p| p.stage = Stage::Preparing);
        let froms: Vec<FsPath> = pairs.iter().map(|(f, _)| f.clone()).collect();
        let metas = self.stat_all(&froms);
        let mut total = Tally::default();
        let across = |(f, t): &(FsPath, FsPath)| !f.root_bytes().eq_ignore_ascii_case(t.root_bytes());
        let mut far = Tally::default();
        let native: Vec<bool> = pairs.iter().zip(&metas).map(|(pair, m)| m.is_none_or(|m| if across(pair) { self.measure(&pair.0, &m, true, &mut far) } else { self.measure(&pair.0, &m, false, &mut total) })).collect();
        self.job.set_copy_limit_available(native.iter().zip(&metas).any(|(&supported, meta)| supported && meta.is_some()));
        let far_dest = pairs.iter().find(|p| across(p)).and_then(|(_, to)| to.parent());
        if far_dest.is_some_and(|d| !self.room(&d, far.bytes)) {
            return None;
        }
        total.entries += far.entries;
        total.bytes += far.bytes;
        if !self.job.preflight_conflicts(|| {
            let pairs = pairs.iter().zip(&native).filter(|(_, supported)| **supported).map(|(pair, _)| pair.clone()).collect();
            flit_platform::conflicts::collect(&crate::NtFs, &FileOp::MoveTo(pairs), self.job.ctl())
        }) {
            return None;
        }
        self.running(total);
        let mut shell = Vec::new();
        for (k, (from, to)) in pairs.iter().enumerate() {
            if self.stopped() {
                break;
            }
            let Some(m) = metas[k] else { continue };
            self.touch(from.parent());
            self.touch(to.parent());
            if !native[k] {
                shell.push((from.clone(), to.clone()));
                continue;
            }
            let r = self.transfer(from, to.clone(), m, Mode::Move, true);
            self.keep_group(from, r, Mode::Move);
            if self.elevate {
                self.elevate = false;
                if self.job.has_companion_groups() {
                    shell.push((from.clone(), to.clone()));
                } else {
                    shell.extend(pairs[k..].iter().cloned());
                    break;
                }
            }
        }
        (!shell.is_empty()).then_some(FileOp::MoveTo(shell))
    }

    fn rename(&mut self, item: &FsPath, name: &[u8]) -> Option<FileOp> {
        let Some(dir) = item.parent() else { return Some(FileOp::Rename { item: item.clone(), name: name.to_vec() }) };
        let to = dir.join(name);
        self.touch(Some(dir));
        match move_file(item, &to, MOVE_FILE_FLAGS(0)) {
            Ok(()) => self.done.push((item.clone(), to)),
            Err(_) if exists(&to) && !same(item, &to) => self.note(tr_en!(S::ErrorsEngineExists, name = flit_core::name::display(name))),
            Err(e) if code(&e) == ERROR_ACCESS_DENIED => return Some(FileOp::Rename { item: item.clone(), name: name.to_vec() }),
            Err(e) => self.fail(item, &e, false),
        }
        None
    }

    fn delete_tree(&mut self, p: &FsPath, m: &Meta, top: bool) -> bool {
        if self.stopped() {
            return false;
        }
        let before = self.failed;
        if is_dir(m) && !is_dir_link(m) {
            match children(p) {
                Ok(l) => {
                    for i in 0..l.len() as u32 {
                        self.delete_tree(&p.join(l.name(i)), &l.entries[i as usize].meta(), false);
                    }
                }
                Err(e) => self.note(tr_en!(S::ErrorsEngineUnreadable, path = p.display(), code = e.code)),
            }
        }
        if self.stopped() {
            return false;
        }
        let gone = if is_dir(m) { remove_dir(p) } else { delete_file(p) };
        match gone {
            Ok(()) => {
                self.job.update(|q| {
                    q.items += 1;
                    q.bytes += if is_dir(m) { 0 } else { m.size };
                });
                true
            }
            Err(e) => {
                let explained = code(&e) == ERROR_DIR_NOT_EMPTY && self.failed > before;
                !explained && self.trouble(p, &e, top) && self.delete_tree(p, m, top)
            }
        }
    }

    fn delete(&mut self, items: &[FsPath], state: Option<&std::path::Path>) -> Option<FileOp> {
        self.job.update(|p| p.stage = Stage::Preparing);
        let metas = self.stat_all(items);
        let stages: Vec<_> = items.iter().zip(&metas).map(|(item, meta)| {
            let state = state?;
            let _ = meta.as_ref().filter(|_| !self.stopped())?;
            let (stamp, _) = undo_stamp(item).ok()?;
            let attributes = unsafe { GetFileAttributesW(PCWSTR(wide(item).as_ptr())) };
            if attributes == INVALID_FILE_ATTRIBUTES {
                return None;
            }
            let hidden = attributes & FILE_ATTRIBUTE_HIDDEN.0 != 0;
            let (path, record) = flit_platform::pending::stage_delete(state, item, stamp, hidden, |from, to| {
                move_file(from, to, MOVE_FILE_FLAGS(0)).map_err(io_error)
            }).ok()?;
            let _ = set_hidden(&path, true);
            self.touch(item.parent());
            Some((path, record, hidden))
        }).collect();
        let mut total = Tally::default();
        for ((p, m), stage) in items.iter().zip(&metas).zip(&stages) {
            if let Some(m) = m {
                self.measure(stage.as_ref().map_or(p, |(staged, _, _)| staged), m, true, &mut total);
            }
        }
        self.running(total);
        let mut rest = Vec::new();
        for ((item, meta), stage) in items.iter().zip(metas).zip(stages) {
            let Some(m) = meta else { continue };
            let mut deleted = false;
            let mut original_available = true;
            if let Some((path, record, hidden)) = stage {
                let matches = || undo_stamp(&path).is_ok_and(|(stamp, _)| record.matches(stamp));
                if matches() && !self.stopped() {
                    let prepared = if is_dir(&m) && !is_dir_link(&m) {
                        record.inventory(&path, undo_fs(), || self.stopped())
                    } else {
                        Ok(())
                    };
                    match prepared.and_then(|()| record.begin_delete()) {
                        Ok(()) => deleted = self.delete_tree(&path, &m, true),
                        Err(error) if !self.stopped() => {
                            original_available = false;
                            self.note(format!("{}: {}", item.display(), flit_core::fmt::io_reason(&error)));
                        }
                        Err(_) => {}
                    }
                }
                if deleted {
                    record.finish();
                } else {
                    record.keep();
                    let restored = if matches() { move_file(&path, item, MOVE_FILE_FLAGS(0)) }
                        else { Err(ERROR_INVALID_DATA.to_hresult().into()) };
                    match restored {
                        Ok(()) => {
                            record.finish();
                            let _ = set_hidden(item, hidden);
                        }
                        Err(error) => {
                            original_available = false;
                            drop(record);
                            let _ = set_hidden(&path, false);
                            self.left_staged(item, &path, &error);
                        }
                    }
                }
            } else if !self.stopped() {
                self.touch(item.parent());
                deleted = self.delete_tree(item, &m, true);
            }
            if deleted {
                self.done.push((item.clone(), item.clone()));
            } else if self.elevate && !self.fatal && !self.job.cancelled() && original_available {
                rest.push(item.clone());
            }
        }
        (!rest.is_empty()).then_some(FileOp::Delete(rest))
    }

    fn flatten_dir(&mut self, root: &FsPath, dir: &FsPath) {
        let l = match children(dir) {
            Ok(l) => l,
            Err(e) => return self.note(tr_en!(S::ErrorsEngineUnreadable, path = dir.display(), code = e.code)),
        };
        for i in 0..l.len() as u32 {
            if self.stopped() {
                return;
            }
            let (name, m) = (l.name(i), l.entries[i as usize].meta());
            let s = dir.join(name);
            if is_dir(&m) && !is_dir_link(&m) {
                self.flatten_dir(root, &s);
                if remove_dir(&s).is_ok() {
                    self.job.update(|p| p.items += 1);
                }
            } else if same(dir, root) {
                self.skip(&m);
            } else {
                let r = self.transfer(&s, root.join(name), m, Mode::Move, false);
                self.keep(&s, r);
            }
        }
    }

    fn flatten(&mut self, dirs: &[FsPath]) -> Option<FileOp> {
        self.job.update(|p| p.stage = Stage::Preparing);
        let mut total = Tally::default();
        for d in dirs {
            self.tally(d, &mut total);
        }
        self.running(total);
        for d in dirs {
            self.touch(Some(d.clone()));
            self.flatten_dir(d, d);
        }
        None
    }

    fn notify(&self) {
        for d in &self.dirs {
            let w = d.to_wide(false);
            unsafe { SHChangeNotify(SHCNE_UPDATEDIR, SHCNF_PATHW | SHCNF_FLUSHNOWAIT, Some(w.as_ptr().cast()), None) };
        }
    }
}

fn pending_state() -> Option<std::path::PathBuf> {
    #[cfg(test)]
    {
        Some(crate::test_temp().join(format!("flit-pending-state-{}", std::process::id())))
    }
    #[cfg(not(test))]
    {
        use flit_platform::Platform;
        crate::Windows::state_dir()
    }
}

pub(crate) fn start_pending_recovery() {
    let Some(state) = pending_state() else { return };
    static START: std::sync::Once = std::sync::Once::new();
    START.call_once(|| {
        if let Err(error) = std::thread::Builder::new().name("flit-pending-deletes".into()).spawn(move || {
            recover_pending(&state, process_is_live);
        }) {
            tracing::warn!(?error, "Flit could not start pending-delete recovery");
        }
    });
}

fn process_is_live(pid: u32) -> bool {
    use windows::Win32::System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE};
    match unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, pid) } {
        Ok(handle) => {
            let handle = crate::walk_index::Handle(handle);
            unsafe { WaitForSingleObject(handle.0, 0) != WAIT_OBJECT_0 }
        }
        Err(error) => code(&error) != ERROR_INVALID_PARAMETER,
    }
}

fn recover_pending(state: &std::path::Path, live: impl Fn(u32) -> bool) {
    flit_platform::pending::recover(state, live, undo_fs(), delete_pending, |staged, original, hidden| {
        move_file(staged, original, MOVE_FILE_FLAGS(0)).map_err(io_error)?;
        if let Err(error) = set_hidden(original, hidden) {
            tracing::warn!(?error, path = %original.display(), "Flit could not restore the hidden attribute");
        }
        Ok(())
    }, |staged| set_hidden(staged, false).map_err(io_error));
}

fn delete_pending(path: &FsPath) -> std::io::Result<()> {
    let fail = || std::io::Error::other("pending deletion was incomplete");
    if path.file_name().ends_with(b".flit-partial") {
        let children = undo_children(path)?;
        for child in &children {
            let m = stat(child).ok_or_else(fail)?;
            if !matches!(child.file_name(), b"link" | b"link.lnk" | b"new") || (is_dir(&m) && !is_dir_link(&m)) {
                return Err(fail());
            }
        }
        for child in children {
            delete_pending(&child)?;
        }
    }
    let m = stat(path).ok_or_else(fail)?;
    if is_dir(&m) { remove_dir(path) } else { delete_file(path) }.map_err(io_error)
}

pub(crate) fn undo_fs() -> flit_platform::replay::FileSystem {
    flit_platform::replay::FileSystem { stamp: undo_stamp, children: undo_children }
}

pub(crate) fn native(op: &FileOp) -> bool {
    matches!(op, FileOp::Copy { .. } | FileOp::Move { .. } | FileOp::MoveTo(_) | FileOp::Rename { .. } | FileOp::Delete(_) | FileOp::Flatten(_) | FileOp::Link { .. } | FileOp::LinkAs { .. })
}

pub(crate) fn run(op: &FileOp, job: &Job) -> Outcome {
    run_with_recycler(op, job, &SystemRecycle)
}

pub(crate) fn run_with_recycler(op: &FileOp, job: &Job, recycle: &dyn Recycle) -> Outcome {
    let mut e = Engine { recycle, ..Engine::new(job) };
    let rest = match op {
        FileOp::Copy { items, dest } => e.copy(items, dest),
        FileOp::Move { items, dest } => e.moves(&items.iter().map(|i| (i.clone(), dest.join(i.file_name()))).collect::<Vec<_>>()),
        FileOp::MoveTo(pairs) => e.moves(pairs),
        FileOp::Rename { item, name } => e.rename(item, name),
        FileOp::Delete(items) => {
            let state = e.pending_state.clone();
            e.delete(items, state.as_deref())
        }
        FileOp::Flatten(dirs) => e.flatten(dirs),
        FileOp::Link { items, dest } => e.links(items, dest, LinkKind::Shortcut),
        FileOp::LinkAs { items, dest, kind } => e.links(items, dest, *kind),
        _ => Some(op.clone()),
    };
    e.notify();
    let rest = rest.filter(|_| !job.cancelled());
    Outcome { error: e.error(), items: e.done, rest }
}
