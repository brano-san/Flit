use crate::fs::{map_attrs, NtFs};
use flit_core::listing::{attr, Listing, ListingBuilder, Meta};
use flit_core::provider::{Conflict, FileOp, Resolve, Stage};
use flit_core::sort::CaseMode;
use flit_core::{limits, FsPath};
use flit_core::i18n::S;
use flit_core::tr_en;
use flit_platform::{Fs, Job, PlatformError};
use std::ffi::c_void;
use windows::core::PCWSTR;
use windows::Win32::Foundation::*;
use windows::Win32::Storage::FileSystem::*;
use windows::Win32::UI::Shell::{SHChangeNotify, SHCNE_UPDATEDIR, SHCNF_FLUSHNOWAIT, SHCNF_PATHW};

type Pairs = Vec<(FsPath, FsPath)>;

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
}

fn wide(p: &FsPath) -> Vec<u16> {
    p.to_wide(true)
}

fn code(e: &windows::core::Error) -> WIN32_ERROR {
    WIN32_ERROR::from_error(e).unwrap_or(ERROR_GEN_FAILURE)
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
    (1u32..)
        .filter(|&k| k > 1 || plain)
        .map(|k| {
            let mut n = [stem, suffix].concat();
            if k > 1 {
                n.extend_from_slice(format!(" ({k})").as_bytes());
            }
            n.extend_from_slice(ext);
            dest.join(&n)
        })
        .find(|p| !exists(p))
        .unwrap()
}

pub(crate) fn free_name(taken: &FsPath, dir: bool) -> FsPath {
    let parent = taken.parent().unwrap_or_else(|| taken.clone());
    numbered(&parent, taken.file_name(), dir, b"", false)
}

pub(crate) fn copy_name(item: &FsPath, dest: &FsPath, dir: bool) -> FsPath {
    numbered(dest, item.file_name(), dir, b" - Copy", true)
}

fn partial(to: &FsPath) -> FsPath {
    let dir = to.parent().unwrap_or_else(|| to.clone());
    numbered(&dir, &[to.file_name(), b".flit-partial"].concat(), true, b"", true)
}

fn move_file(from: &FsPath, to: &FsPath, flags: MOVE_FILE_FLAGS) -> windows::core::Result<()> {
    unsafe { MoveFileExW(PCWSTR(wide(from).as_ptr()), PCWSTR(wide(to).as_ptr()), flags) }
}

fn create_dir(dst: &FsPath, template: &FsPath) -> windows::core::Result<()> {
    unsafe { CreateDirectoryExW(PCWSTR(wide(template).as_ptr()), PCWSTR(wide(dst).as_ptr()), None) }
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
    if (*msg).Type == COPYFILE2_CALLBACK_CHUNK_FINISHED {
        let done = (*msg).Info.ChunkFinished.uliTotalBytesTransferred;
        c.job.update(|p| p.bytes = c.base + done);
    }
    COPYFILE2_PROGRESS_CONTINUE
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
                if job.cancelled() {
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
    fatal: bool,
    elevate: bool,
    dirs: Vec<FsPath>,
    limit: Option<(Vec<u8>, Option<u64>)>,
}

impl<'a> Engine<'a> {
    pub(crate) fn new(job: &'a Job) -> Self {
        Self { job, done: Vec::new(), failed: 0, first: None, fatal: false, elevate: false, dirs: Vec::new(), limit: None }
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
        self.note(format!("{}: {}", path.display(), e.message()));
    }

    fn note(&mut self, msg: String) {
        self.failed += 1;
        self.first.get_or_insert(msg);
    }

    fn error(&self) -> Option<PlatformError> {
        if self.job.cancelled() {
            return Some(PlatformError::new(ERROR_CANCELLED.0 as i64, S::ErrorsEngineCancelled.en()));
        }
        let first = self.first.clone()?;
        Some(PlatformError::new(-1, if self.failed > 1 { tr_en!(S::ErrorsEngineFailed, count = self.failed, first = first) } else { first }))
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
        let Some(free) = free_space(dest).filter(|&free| free < need) else { return true };
        if self.job.ctl().policy().is_none() && self.job.resolve(limits::short_of_space(dest, need, free)).is_some() {
            return true;
        }
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

    pub(crate) fn copy_file(&self, from: &FsPath, to: &FsPath, size: u64, replace: bool) -> windows::core::Result<()> {
        let chunk = Chunk { job: self.job, base: self.job.progress().bytes };
        let params = COPYFILE2_EXTENDED_PARAMETERS {
            dwSize: size_of::<COPYFILE2_EXTENDED_PARAMETERS>() as u32,
            dwCopyFlags: COPY_FILE_FAIL_IF_EXISTS,
            pProgressRoutine: Some(on_chunk),
            pvCallbackContext: &chunk as *const Chunk as *mut c_void,
            ..Default::default()
        };
        let target = if replace { partial(to) } else { to.clone() };
        unsafe { CopyFile2(PCWSTR(wide(from).as_ptr()), PCWSTR(wide(&target).as_ptr()), Some(&params)) }?;
        let placed = if replace { move_file(&target, to, MOVEFILE_REPLACE_EXISTING) } else { Ok(()) };
        if let Err(e) = placed {
            let _ = delete_file(&target);
            return Err(e);
        }
        self.job.update(|p| {
            p.bytes = chunk.base + size;
            p.items += 1;
        });
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
        let (mut replace, mut retried) = (false, false);
        loop {
            let tried = match mode {
                Mode::Move => move_file(src, &dst, if replace { MOVEFILE_REPLACE_EXISTING } else { MOVE_FILE_FLAGS(0) }),
                _ if dir => create_dir(&dst, src),
                _ => self.copy_file(src, &dst, m.size, replace),
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
            if replace && c == ERROR_ACCESS_DENIED && !retried && unprotect(&dst) {
                retried = true;
                continue;
            }
            if !replace && exists(&dst) {
                match self.ask(src, m, &dst) {
                    Some(Resolve::KeepBoth) => dst = free_name(&dst, dir),
                    Some(Resolve::Replace) if dir => return self.merge(src, &dst, mode),
                    Some(Resolve::Replace) => replace = true,
                    Some(Resolve::Skip) => {
                        self.skip(&m);
                        return Res::Nothing;
                    }
                    None => return Res::Nothing,
                }
                continue;
            }
            self.fail(src, &e, top);
            return Res::Nothing;
        }
        match (mode, dir) {
            (Mode::Move, _) => {
                self.job.update(|p| p.items += 1);
                Res::Whole(dst)
            }
            (Mode::Across, false) => match delete_file(src) {
                Ok(()) => Res::Whole(dst),
                Err(e) => {
                    let _ = delete_file(&dst);
                    self.fail(src, &e, top);
                    Res::Nothing
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

    fn copy(&mut self, items: &[FsPath], dest: &FsPath) -> Option<FileOp> {
        self.job.update(|p| p.stage = Stage::Preparing);
        let metas = self.stat_all(items);
        let mut total = Tally::default();
        let native: Vec<bool> = items.iter().zip(&metas).map(|(p, m)| m.is_none_or(|m| self.measure(p, &m, true, &mut total))).collect();
        if !self.room(dest, total.bytes) {
            return None;
        }
        self.running(total);
        self.touch(Some(dest.clone()));
        let mut shell = Vec::new();
        let small: Vec<usize> = (0..items.len()).filter(|&k| native[k] && metas[k].is_some_and(|m| !is_dir(&m) && m.size <= QUICK_MAX) && !items[k].parent().is_some_and(|p| same(&p, dest))).collect();
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
            self.keep(item, r);
            if self.elevate {
                shell.extend(items[k..].iter().cloned());
                self.elevate = false;
                break;
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
        let far_dest = pairs.iter().find(|p| across(p)).and_then(|(_, to)| to.parent());
        if far_dest.is_some_and(|d| !self.room(&d, far.bytes)) {
            return None;
        }
        total.entries += far.entries;
        total.bytes += far.bytes;
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
            self.keep(from, r);
            if self.elevate {
                shell.extend(pairs[k..].iter().cloned());
                self.elevate = false;
                break;
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
                if !self.job.cancelled() {
                    self.fail(p, &e, top);
                }
                false
            }
        }
    }

    fn delete(&mut self, items: &[FsPath]) -> Option<FileOp> {
        self.job.update(|p| p.stage = Stage::Preparing);
        let metas = self.stat_all(items);
        let mut total = Tally::default();
        for (p, m) in items.iter().zip(&metas) {
            if let Some(m) = m {
                self.measure(p, m, true, &mut total);
            }
        }
        self.running(total);
        for (k, item) in items.iter().enumerate() {
            let Some(m) = metas[k] else { continue };
            self.touch(item.parent());
            if self.delete_tree(item, &m, true) {
                self.done.push((item.clone(), item.clone()));
            }
            if self.elevate {
                self.elevate = false;
                return Some(FileOp::Delete(items[k..].to_vec()));
            }
            if self.stopped() {
                break;
            }
        }
        None
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

pub(crate) fn native(op: &FileOp) -> bool {
    matches!(op, FileOp::Copy { .. } | FileOp::Move { .. } | FileOp::MoveTo(_) | FileOp::Rename { .. } | FileOp::Delete(_) | FileOp::Flatten(_))
}

pub(crate) fn run(op: &FileOp, job: &Job) -> Outcome {
    let mut e = Engine::new(job);
    let rest = match op {
        FileOp::Copy { items, dest } => e.copy(items, dest),
        FileOp::Move { items, dest } => e.moves(&items.iter().map(|i| (i.clone(), dest.join(i.file_name()))).collect::<Vec<_>>()),
        FileOp::MoveTo(pairs) => e.moves(pairs),
        FileOp::Rename { item, name } => e.rename(item, name),
        FileOp::Delete(items) => e.delete(items),
        FileOp::Flatten(dirs) => e.flatten(dirs),
        _ => Some(op.clone()),
    };
    e.notify();
    let rest = rest.filter(|_| !job.cancelled());
    Outcome { error: e.error(), items: e.done, rest }
}
