use crate::fs::{cstring, host, names, node_at, open_at, open_dir, open_dir_at, Node};
use crate::mounts::Mount;
use crate::trash::Trash;
use flit_core::i18n::{Lang, S};
use flit_core::limits::{self, BadName};
use flit_core::path::PathKind;
use flit_core::provider::{Conflict, Failure, FileOp, Recover, Resolve, Stage, Template};
use flit_core::FsPath;
use flit_platform::{Fs, Job, OpOutcome, PlatformError, CANCELLED};
use flit_proto::listing::check_name;
use std::cell::RefCell;
use std::ffi::{CStr, CString};
use std::io::{self, Write};
use std::os::fd::{AsRawFd, BorrowedFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::time::Instant;

mod access;
mod writeback;

type Pairs = Vec<(FsPath, FsPath)>;

const CHUNK: usize = 8 << 20;
const BUFFER: usize = 1 << 20;
const RENAME_NOREPLACE: libc::c_uint = 1;
const NOT_LINUX: &str = "The operation names a path that is not a Linux path";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
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

struct Pump {
    base: u64,
    kernel: bool,
    deferred: bool,
    buf: Vec<u8>,
    ranges: Option<writeback::Ranges>,
}

#[derive(Default)]
struct Tally {
    entries: u64,
    bytes: u64,
}

struct Top {
    dir: OwnedFd,
    name: CString,
    node: Node,
    parent_dev: u64,
}

fn check(r: libc::c_int) -> io::Result<()> {
    if r < 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
}

fn errno(code: i32) -> io::Error {
    io::Error::from_raw_os_error(code)
}

fn unlink_at(dir: RawFd, name: &CStr, folder: bool) -> io::Result<()> {
    check(unsafe { libc::unlinkat(dir, name.as_ptr(), if folder { libc::AT_REMOVEDIR } else { 0 }) })
}

pub(crate) fn rename_exact(sdir: RawFd, sname: &CStr, ddir: RawFd, dname: &CStr) -> io::Result<()> {
    rename_fallback(sdir, sname, ddir, dname, rename_noreplace(sdir, sname, ddir, dname))
}

fn rename_noreplace(sdir: RawFd, sname: &CStr, ddir: RawFd, dname: &CStr) -> io::Result<()> {
    let result = unsafe { libc::syscall(libc::SYS_renameat2, sdir, sname.as_ptr(), ddir, dname.as_ptr(), RENAME_NOREPLACE) };
    if result == 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
}

fn rename_fallback(sdir: RawFd, sname: &CStr, ddir: RawFd, dname: &CStr, result: io::Result<()>) -> io::Result<()> {
    match result {
        Err(e) if matches!(e.raw_os_error(), Some(libc::EINVAL | libc::ENOSYS | libc::EOPNOTSUPP)) => {}
        result => return result,
    }
    match node_at(ddir, dname, false) {
        Ok(_) => return Err(errno(libc::EEXIST)),
        Err(e) if e.raw_os_error() != Some(libc::ENOENT) => return Err(e),
        Err(_) => {}
    }
    check(unsafe { libc::renameat(sdir, sname.as_ptr(), ddir, dname.as_ptr()) })
}

fn rename_at(sdir: RawFd, sname: &CStr, ddir: RawFd, dname: &CStr, replace: bool) -> io::Result<()> {
    if !replace {
        return rename_exact(sdir, sname, ddir, dname);
    }
    check(unsafe { libc::renameat(sdir, sname.as_ptr(), ddir, dname.as_ptr()) })
}

fn seek(fd: RawFd, at: u64, whence: libc::c_int) -> io::Result<u64> {
    let r = unsafe { libc::lseek(fd, at as libc::off_t, whence) };
    if r < 0 { Err(io::Error::last_os_error()) } else { Ok(r as u64) }
}

fn sparse(st: &libc::stat) -> bool {
    (st.st_blocks as u64).saturating_mul(512) < st.st_size as u64
}

fn times(node: &Node) -> [libc::timespec; 2] {
    let ts = |s: crate::fs::Stamp| libc::timespec { tv_sec: s.sec as _, tv_nsec: s.nsec as _ };
    [ts(node.atime), ts(node.mtime)]
}

fn copied_xattr(name: &[u8], move_labels: bool) -> bool {
    !name.is_empty() && (move_labels || name != b"security.selinux")
}

fn xattr_names(fd: RawFd) -> Vec<u8> {
    for _ in 0..3 {
        let n = unsafe { libc::flistxattr(fd, std::ptr::null_mut(), 0) };
        if n <= 0 {
            return Vec::new();
        }
        let mut buf = vec![0u8; n as usize];
        let m = unsafe { libc::flistxattr(fd, buf.as_mut_ptr().cast(), buf.len()) };
        if m >= 0 {
            buf.truncate(m as usize);
            return buf;
        }
        if io::Error::last_os_error().raw_os_error() != Some(libc::ERANGE) {
            return Vec::new();
        }
    }
    Vec::new()
}

fn xattr_value(fd: RawFd, name: &CStr) -> Option<Vec<u8>> {
    for _ in 0..3 {
        let n = unsafe { libc::fgetxattr(fd, name.as_ptr(), std::ptr::null_mut(), 0) };
        if n < 0 {
            return None;
        }
        let mut buf = vec![0u8; n as usize];
        let m = unsafe { libc::fgetxattr(fd, name.as_ptr(), buf.as_mut_ptr().cast(), buf.len()) };
        if m >= 0 {
            buf.truncate(m as usize);
            return Some(buf);
        }
        if io::Error::last_os_error().raw_os_error() != Some(libc::ERANGE) {
            return None;
        }
    }
    None
}

fn keep_xattrs(from: RawFd, to: RawFd, move_labels: bool) {
    let names = xattr_names(from);
    for name in names.split(|&b| b == 0).filter(|n| copied_xattr(n, move_labels)) {
        let Ok(name) = CString::new(name) else { continue };
        if let Some(value) = xattr_value(from, &name) {
            unsafe { libc::fsetxattr(to, name.as_ptr(), value.as_ptr().cast(), value.len(), 0) };
        }
    }
}

fn keep_meta(from: RawFd, fd: RawFd, node: &Node, mode: Mode) {
    let owned = unsafe { libc::fchown(fd, node.uid, node.gid) } == 0;
    keep_xattrs(from, fd, mode != Mode::Copy);
    let mode = if owned { node.mode & 0o7777 } else { node.mode & 0o1777 };
    unsafe {
        libc::fchmod(fd, mode as libc::mode_t);
        libc::futimens(fd, times(node).as_ptr());
    }
}

fn keep_link_meta(dir: RawFd, name: &CStr, node: &Node) {
    unsafe {
        libc::fchownat(dir, name.as_ptr(), node.uid, node.gid, libc::AT_SYMLINK_NOFOLLOW);
        libc::utimensat(dir, name.as_ptr(), times(node).as_ptr(), libc::AT_SYMLINK_NOFOLLOW);
    }
}

fn read_link(dir: RawFd, name: &CStr, hint: u64) -> io::Result<CString> {
    let mut len = (hint as usize).max(255) + 1;
    loop {
        let mut buf = vec![0u8; len];
        let n = unsafe { libc::readlinkat(dir, name.as_ptr(), buf.as_mut_ptr().cast(), buf.len()) };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        if (n as usize) < len {
            buf.truncate(n as usize);
            return cstring(&buf);
        }
        len *= 2;
    }
}

fn exists_at(dir: RawFd, name: &[u8]) -> bool {
    match cstring(name) {
        Ok(c) => node_at(dir, &c, false).is_ok(),
        Err(_) => true,
    }
}

fn numbered(dir: RawFd, name: &[u8], folder: bool, suffix: &[u8], plain: bool) -> Vec<u8> {
    let ext = if folder { 0 } else { flit_core::name::ext_len(name) };
    let (stem, ext) = if ext == 0 || ext + 1 >= name.len() { (name, &b""[..]) } else { name.split_at(name.len() - ext - 1) };
    (1u32..)
        .filter(|&k| k > 1 || plain)
        .map(|k| {
            let mut n = [stem, suffix].concat();
            if k > 1 {
                n.extend_from_slice(format!(" ({k})").as_bytes());
            }
            n.extend_from_slice(ext);
            n
        })
        .find(|n| !exists_at(dir, n))
        .unwrap_or_default()
}

fn sibling(path: &FsPath, name: &[u8]) -> FsPath {
    path.parent().unwrap_or_else(|| path.clone()).join(name)
}

fn inside(path: &FsPath, folder: &FsPath) -> bool {
    let (p, f) = (path.as_bytes(), folder.as_bytes());
    p.len() > f.len() && p.starts_with(f) && (folder.is_root() || p[f.len()] == b'/')
}

fn fs_path(p: &Path) -> Option<FsPath> {
    FsPath::parse_exact(p.as_os_str().as_bytes()).filter(|p| p.kind() == PathKind::Unix)
}

pub fn undo_fs() -> flit_platform::replay::FileSystem {
    flit_platform::replay::FileSystem { stamp: undo_stamp, children: undo_children }
}

fn undo_stamp(path: &FsPath) -> io::Result<(flit_core::undo::Stamp, bool)> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::symlink_metadata(host(path).ok_or_else(|| errno(libc::EINVAL))?)?;
    if meta.ino() == 0 {
        return Err(io::Error::from(io::ErrorKind::Unsupported));
    }
    let stamp = flit_core::undo::Stamp {
        size: meta.size(),
        modified: i128::from(meta.mtime()) * 1_000_000_000 + i128::from(meta.mtime_nsec()),
        device: meta.dev(),
        file: meta.ino(),
        access: Some(flit_core::provider::Access { mode: meta.mode(), uid: meta.uid(), gid: meta.gid() }),
    };
    Ok((stamp, meta.is_dir()))
}

fn undo_children(dir: &FsPath) -> io::Result<Vec<FsPath>> {
    std::fs::read_dir(host(dir).ok_or_else(|| errno(libc::EINVAL))?)?.map(|e| {
        Ok(dir.join(e?.file_name().as_bytes()))
    }).collect()
}

fn reserve_stage(dir: RawFd) -> io::Result<(CString, OwnedFd)> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    loop {
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let name = CString::new(format!(".flit-stage-{}-{id}.flit-partial", std::process::id())).map_err(|_| errno(libc::EINVAL))?;
        match check(unsafe { libc::mkdirat(dir, name.as_ptr(), 0o700) }) {
            Ok(()) => return open_dir_at(dir, &name).map(|fd| (name, fd)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
}

fn on_windows_volume(path: &FsPath) -> bool {
    crate::fs::LinuxFs.file_system(path).is_some_and(|fs| matches!(fs.as_str(), "ntfs" | "ntfs3" | "fuseblk"))
}

fn in_use(path: &FsPath, e: &io::Error) -> Option<String> {
    let busy = matches!(e.raw_os_error(), Some(libc::EBUSY | libc::ETXTBSY));
    busy.then(|| host(path)).flatten().and_then(|h| flit_core::fmt::in_use(&flit_core::fmt::io_reason(e), &crate::busy::holders(h)))
}

fn reason(path: &FsPath, e: &io::Error) -> String {
    match e.raw_os_error() {
        Some(libc::EROFS) if on_windows_volume(path) => S::ErrorsOpsExtraReadOnlyWindows.en().to_owned(),
        Some(libc::EROFS) => S::ErrorsOpsExtraReadOnly.en().to_owned(),
        _ => in_use(path, e).unwrap_or_else(|| flit_core::fmt::io_reason(e)),
    }
}

fn failed_at(path: &FsPath, e: &io::Error) -> String {
    match e.raw_os_error() {
        Some(libc::EROFS) if on_windows_volume(path) => flit_core::tr_en!(S::ErrorsOpsExtraReadOnlyWindowsAt, path = path.display()),
        Some(libc::EROFS) => flit_core::tr_en!(S::ErrorsOpsExtraReadOnlyAt, path = path.display()),
        _ => format!("{}: {}", path.display(), in_use(path, e).unwrap_or_else(|| flit_core::fmt::io_reason(e))),
    }
}

fn rename_through_temporary(dir: RawFd, from: &CStr, to: &CStr) -> io::Result<()> {
    let tmp = cstring(&numbered(dir, &[from.to_bytes(), b".flit-rename"].concat(), true, b"", true))?;
    rename_at(dir, from, dir, &tmp, false)?;
    rename_at(dir, &tmp, dir, to, false).inspect_err(|_| {
        let _ = rename_at(dir, &tmp, dir, from, false);
    })
}

fn free_space(fd: RawFd) -> Option<u64> {
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    (unsafe { libc::fstatvfs(fd, &mut st) } == 0).then(|| st.f_bavail as u64 * st.f_frsize as u64)
}

fn top(p: &FsPath) -> io::Result<Top> {
    let parent = p.parent().filter(|_| !p.is_root() && p.kind() == PathKind::Unix).ok_or_else(|| errno(libc::EINVAL))?;
    let dir = open_dir(host(&parent).ok_or_else(|| errno(libc::EINVAL))?)?;
    let name = cstring(p.file_name())?;
    let node = node_at(dir.as_raw_fd(), &name, false)?;
    let parent_dev = node_at(dir.as_raw_fd(), c".", false)?.dev;
    Ok(Top { dir, name, node, parent_dev })
}

fn file_system(mounts: &[Mount], dev: u64) -> Option<String> {
    mounts.iter().rev().find(|m| m.dev == ((dev >> 32) as u32, dev as u32)).map(|m| m.fstype.clone())
}

fn deletable_instead(item: &FsPath, e: &io::Error) -> bool {
    !matches!(e.raw_os_error(), Some(libc::ENOENT | libc::EINVAL | libc::EROFS)) && top(item).is_ok_and(|t| !t.node.mounted_on(t.parent_dev))
}

fn open_parent(to: &FsPath, create: bool) -> io::Result<OwnedFd> {
    let parent = to.parent().filter(|_| !to.is_root()).ok_or_else(|| errno(libc::EINVAL))?;
    let dir = host(&parent).ok_or_else(|| errno(libc::EINVAL))?;
    match open_dir(dir) {
        Err(e) if create && e.raw_os_error() == Some(libc::ENOENT) => {
            std::fs::create_dir_all(dir)?;
            open_dir(dir)
        }
        r => r,
    }
}

struct Engine<'a> {
    job: &'a Job,
    done: Pairs,
    failed: usize,
    first: Option<String>,
    first_kind: flit_platform::PlatformErrorKind,
    fatal: bool,
    denied: bool,
    untrashable: Vec<FsPath>,
    dest_fs: Option<(u64, Option<String>)>,
    volumes: RefCell<writeback::Volumes>,
    lang: Lang,
    free: fn(RawFd) -> Option<u64>,
    trash: Option<&'a Trash>,
}

impl<'a> Engine<'a> {
    fn new(job: &'a Job) -> Self {
        Self { job, done: Vec::new(), failed: 0, first: None, first_kind: Default::default(), fatal: false, denied: false, untrashable: Vec::new(), dest_fs: None, volumes: RefCell::default(), lang: flit_core::i18n::lang(), free: free_space, trash: None }
    }

    fn stopped(&self) -> bool {
        self.fatal || self.job.cancelled()
    }

    fn note(&mut self, msg: String) {
        self.failed += 1;
        self.first.get_or_insert(msg);
    }

    fn note_io(&mut self, path: &FsPath, e: &io::Error) {
        if self.first.is_none() {
            self.first_kind = if e.raw_os_error() == Some(libc::ECANCELED) {
                flit_platform::PlatformErrorKind::Cancelled
            } else {
                PlatformError::from_io(e, "").kind
            };
        }
        self.note(failed_at(path, e));
    }

    fn fail(&mut self, path: &FsPath, e: &io::Error) {
        match e.raw_os_error() {
            Some(libc::ENOSPC | libc::EDQUOT) => self.fatal = true,
            Some(libc::EACCES | libc::EPERM) => self.denied = true,
            _ => {}
        }
        self.note_io(path, e);
    }

    fn trouble(&mut self, path: &FsPath, e: &io::Error) -> bool {
        if self.job.cancelled() {
            return false;
        }
        if !self.job.asks_failures() {
            self.fail(path, e);
            return false;
        }
        let fatal = matches!(e.raw_os_error(), Some(libc::ENOSPC | libc::EDQUOT));
        match self.job.fail(Failure { path: path.clone(), error: reason(path, e).into(), fatal }) {
            Some(Recover::Retry) => true,
            Some(Recover::Skip) => {
                self.fatal |= fatal;
                self.note_io(path, e);
                false
            }
            None => false,
        }
    }

    fn error(&self) -> Option<PlatformError> {
        if self.job.cancelled() && self.first.is_none() {
            return Some(PlatformError::cancelled(libc::ECANCELED as i64, CANCELLED));
        }
        let first = self.first.clone()?;
        let code = if self.denied { libc::EACCES as i64 } else { -1 };
        Some(PlatformError::new(code, if self.failed > 1 { format!("{} items failed. {first}", self.failed) } else { first }).with_kind(self.first_kind))
    }

    fn preparing(&self) {
        self.job.update(|p| p.stage = Stage::Preparing);
    }

    fn running(&self, total: Tally) {
        self.job.update(|p| {
            p.stage = Stage::Running;
            p.total_items = total.entries;
            p.total_bytes = total.bytes;
        });
    }

    fn counted(&self, node: &Node) {
        self.job.update(|p| {
            p.items += 1;
            p.bytes += if node.kind() == libc::S_IFREG { node.size } else { 0 };
        });
    }

    fn tops(&mut self, items: &[FsPath]) -> Vec<Option<Top>> {
        items
            .iter()
            .map(|p| loop {
                match top(p) {
                    Ok(t) => break Some(t),
                    Err(e) if e.raw_os_error() == Some(libc::ENOENT) => {
                        self.note(format!("{}: the item no longer exists", p.display()));
                        break None;
                    }
                    Err(e) if self.trouble(p, &e) => {}
                    Err(_) => break None,
                }
            })
            .collect()
    }

    fn tally(&self, dir: RawFd, name: &CStr, node: &Node, mounts: bool, t: &mut Tally) {
        t.entries += 1;
        if node.kind() == libc::S_IFREG {
            t.bytes += node.size;
        }
        if !node.is_dir() {
            return;
        }
        let Ok(fd) = open_dir_at(dir, name) else { return };
        for n in names(fd.as_raw_fd()).unwrap_or_default() {
            if self.job.cancelled() {
                return;
            }
            match node_at(fd.as_raw_fd(), &n, false) {
                Ok(child) if child.is_dir() && !mounts && child.mounted_on(node.dev) => t.entries += 1,
                Ok(child) => self.tally(fd.as_raw_fd(), &n, &child, mounts, t),
                Err(_) => {}
            }
        }
    }

    fn room(&mut self, fd: RawFd, dest: &FsPath, need: u64) -> bool {
        let free = match self.job.room(dest, need, || (self.free)(fd)) {
            Ok(()) => return true,
            Err(None) => return false,
            Err(Some(free)) => free,
        };
        let (mut needed, mut available) = (String::new(), String::new());
        flit_core::fmt::size(&mut needed, need);
        flit_core::fmt::size(&mut available, free);
        self.note(flit_core::tr_en!(S::ErrorsEngineNoRoom, folder = dest.display(), need = needed, free = available));
        false
    }

    fn dest_fs(&mut self, dir: RawFd) -> Option<String> {
        let Ok(node) = node_at(dir, c".", false) else {
            self.dest_fs = None;
            return None;
        };
        let dev = node.dev;
        match &self.dest_fs {
            Some((known, fs)) if *known == dev => fs.clone(),
            _ => {
                let fs = self.volumes.get_mut().file_system(dev);
                self.dest_fs = Some((dev, fs.clone()));
                fs
            }
        }
    }

    fn fat_safe_dst(&mut self, dir: RawFd, dst: &FsPath) -> Option<FsPath> {
        let fs = self.dest_fs(dir)?;
        (limits::windows_names(&fs) && limits::bad_name(dst.file_name()).is_some()).then(|| sibling(dst, &limits::fat_safe(dst.file_name())))
    }

    fn bad_name_at(&mut self, dir: RawFd, name: &[u8]) -> Option<BadName> {
        let fs = self.dest_fs(dir)?;
        limits::windows_names(&fs).then(|| limits::bad_name(name)).flatten()
    }

    fn fits(&mut self, ddir: RawFd, src: &FsPath, dst: &FsPath, node: &Node) -> bool {
        let Some(fs) = self.dest_fs(ddir) else { return true };
        if let Some(bad) = limits::windows_names(&fs).then(|| limits::bad_name(dst.file_name())).flatten() {
            self.note(limits::bad_name_error(dst, bad));
            self.skip(node);
            return false;
        }
        let Some(limit) = limits::max_file_size(&fs).filter(|&l| node.kind() == libc::S_IFREG && limits::exceeds(node.size, Some(l))) else { return true };
        let unasked = self.job.ctl().policy().is_some();
        if self.job.resolve(limits::over_limit(src, dst, node.size, limit)).is_some() {
            self.skip(node);
        }
        if unasked {
            let mut most = String::new();
            flit_core::fmt::size(&mut most, limit);
            self.note(flit_core::tr_en!(S::ErrorsOpsExtraOverLimit, path = src.display(), limit = most));
        }
        false
    }

    fn ask(&self, src: &FsPath, node: &Node, dst: &FsPath, existing: Option<Node>) -> Option<Resolve> {
        let to = existing.map(|n| n.meta(dst.file_name())).unwrap_or_default();
        let choice = self.job.resolve(Conflict { source: src.clone(), target: dst.clone(), from: node.meta(src.file_name()), to })?;
        let same_kind = existing.is_some_and(|e| e.is_dir() == node.is_dir());
        Some(if choice == Resolve::Replace && !same_kind { Resolve::KeepBoth } else { choice })
    }

    fn skip(&self, node: &Node) {
        self.counted(node);
    }

    fn copy_data(&self, from: RawFd, to: RawFd, sparse: bool, mode: writeback::CopyMode) -> io::Result<Option<usize>> {
        let ranges = match mode {
            writeback::CopyMode::Windowed(window) => Some(writeback::Ranges::new(window)),
            _ => None,
        };
        let mut pump = Pump { base: self.job.progress().bytes, kernel: true, deferred: mode == writeback::CopyMode::Deferred, buf: Vec::new(), ranges };
        self.copy_extents(from, to, sparse, &mut pump)?;
        if let Some(ref mut ranges) = pump.ranges {
            if let Some(end) = ranges.wait(|flush, offset, bytes| writeback::range(to, flush, offset, bytes), Instant::now)? {
                self.job.update(|progress| progress.bytes = pump.base + end);
            }
        }
        Ok(pump.ranges.and_then(|ranges| ranges.size()))
    }

    fn copy_extents(&self, from: RawFd, to: RawFd, sparse: bool, pump: &mut Pump) -> io::Result<()> {
        if !sparse {
            return self.pump(from, to, 0, u64::MAX, pump).map(drop);
        }
        let mut at = 0;
        loop {
            let start = match seek(from, at, libc::SEEK_DATA) {
                Ok(s) => s,
                Err(e) if e.raw_os_error() == Some(libc::ENXIO) => break,
                Err(e) if at == 0 && matches!(e.raw_os_error(), Some(libc::EINVAL | libc::EOPNOTSUPP)) => return self.pump(from, to, 0, u64::MAX, pump).map(drop),
                Err(e) => return Err(e),
            };
            let end = seek(from, start, libc::SEEK_HOLE)?;
            seek(from, start, libc::SEEK_SET)?;
            seek(to, start, libc::SEEK_SET)?;
            let copied = self.pump(from, to, start, end - start, pump)?;
            if copied < end - start {
                break;
            }
            at = end;
        }
        let size = seek(from, 0, libc::SEEK_END)?;
        check(unsafe { libc::ftruncate(to, size as libc::off_t) })
    }

    fn pump(&self, from: RawFd, to: RawFd, offset: u64, limit: u64, p: &mut Pump) -> io::Result<u64> {
        let mut done = 0u64;
        loop {
            self.job.ctl().wait_while_paused();
            if self.job.cancelled() {
                return Err(errno(libc::ECANCELED));
            }
            let room = p.ranges.as_ref().map_or(CHUNK, |ranges| ranges.room().min(CHUNK));
            let want = self.job.copy_chunk_size((limit - done).min(room as u64) as usize);
            if want == 0 {
                break;
            }
            let n = if p.kernel {
                let r = unsafe { libc::syscall(libc::SYS_copy_file_range, from, std::ptr::null_mut::<i64>(), to, std::ptr::null_mut::<i64>(), want, 0) };
                if r < 0 {
                    let e = io::Error::last_os_error();
                    if done == 0 && matches!(e.raw_os_error(), Some(libc::EXDEV | libc::ENOSYS | libc::EINVAL | libc::EOPNOTSUPP | libc::EPERM | libc::EBADF)) {
                        p.kernel = false;
                        continue;
                    }
                    return Err(e);
                }
                if r == 0 && done == 0 {
                    p.kernel = false;
                    continue;
                }
                r as usize
            } else {
                p.buf.resize(BUFFER.min(want), 0);
                let r = unsafe { libc::read(from, p.buf.as_mut_ptr().cast(), p.buf.len()) };
                if r < 0 {
                    let e = io::Error::last_os_error();
                    if e.kind() == io::ErrorKind::Interrupted {
                        continue;
                    }
                    return Err(e);
                }
                let mut rest = &p.buf[..r as usize];
                while !rest.is_empty() {
                    let w = unsafe { libc::write(to, rest.as_ptr().cast(), rest.len()) };
                    if w < 0 {
                        let e = io::Error::last_os_error();
                        if e.kind() == io::ErrorKind::Interrupted {
                            continue;
                        }
                        return Err(e);
                    }
                    rest = &rest[w as usize..];
                }
                r as usize
            };
            if n == 0 {
                break;
            }
            if !self.job.pace_copy(n as u64) {
                return Err(errno(libc::ECANCELED));
            }
            let start = offset + done;
            done += n as u64;
            if let Some(ranges) = p.ranges.as_mut().filter(|ranges| ranges.size().is_some()) {
                ranges.add(start, n);
                if ranges.room() == 0 {
                    self.submit_window(to, p)?;
                }
            } else if !p.deferred {
                self.job.update(|progress| progress.bytes = p.base + offset + done);
            }
        }
        self.submit_window(to, p)?;
        Ok(done)
    }

    fn submit_window(&self, fd: RawFd, pump: &mut Pump) -> io::Result<()> {
        if let Some(ranges) = &mut pump.ranges {
            if let Some(end) = ranges.submit(|flush, offset, bytes| writeback::range(fd, flush, offset, bytes), Instant::now)? {
                self.job.update(|progress| progress.bytes = pump.base + end);
            }
        }
        Ok(())
    }

    fn replace_file(&self, source: (RawFd, &CStr), target: (RawFd, &CStr, &FsPath), node: &Node, mode: Mode) -> io::Result<()> {
        let (sdir, sname) = source;
        let (ddir, dname, dst) = target;
        let (name, stage) = reserve_stage(ddir)?;
        let pending = pending_state().and_then(|state| {
            let path = sibling(dst, name.to_bytes());
            let (stamp, _) = undo_stamp(&path).ok()?;
            flit_platform::pending::partial(&state, &path, stamp).ok()
        });
        let dir = stage.as_raw_fd();
        let new = c"new";
        let old = c"replaced";
        let result = (|| {
            if mode != Mode::Move {
                self.create(sdir, sname, node, dir, new, mode, dst)?;
            }
            if self.job.cancelled() {
                return Err(errno(libc::ECANCELED));
            }
            let recycled = self.trash.and_then(|trash| {
                let path = host(dst)?;
                let file = fs_path(&trash.trash_checked(path).ok()?)?;
                Some((trash, file))
            });
            if recycled.is_none() {
                rename_exact(ddir, dname, dir, old)?;
            }
            let restore = || {
                if let Some((trash, backup)) = &recycled {
                    let info = host(backup).and_then(|p| trash.entry(p).ok()).map(|(info, _)| info);
                    if let (Some(info), Ok(from)) = (info, top(backup)) {
                        if rename_exact(from.dir.as_raw_fd(), &from.name, ddir, dname).is_ok() {
                            let _ = std::fs::remove_file(info);
                        }
                    }
                } else {
                    let _ = rename_exact(dir, old, ddir, dname);
                }
            };
            let (from, file) = if mode == Mode::Move { (sdir, sname) } else { (dir, new) };
            let placed_node = (mode == Mode::Across).then(|| node_at(from, file, false).ok()).flatten();
            if let Err(error) = rename_exact(from, file, ddir, dname) {
                restore();
                return Err(error);
            }
            if mode == Mode::Across {
                if let Err(error) = unlink_at(sdir, sname, false) {
                    if placed_node.is_some_and(|n| node_at(ddir, dname, false).is_ok_and(|now| (now.dev, now.ino) == (n.dev, n.ino)))
                        && rename_exact(ddir, dname, dir, new).is_ok() {
                        restore();
                    }
                    return Err(error);
                }
            }
            if mode == Mode::Move {
                self.job.update(|p| p.items += 1);
            }
            if recycled.is_none() {
                let _ = unlink_at(dir, old, false);
            }
            self.job.replaced(dst.clone(), recycled.map(|(_, path)| path));
            Ok(())
        })();
        if result.is_err() {
            let _ = unlink_at(dir, new, false);
        }
        if let (Ok(()), Some(pending)) = (unlink_at(ddir, &name, true), pending) {
            pending.finish();
        }
        result
    }

    #[allow(clippy::too_many_arguments)]
    fn copy_file(&self, sdir: RawFd, sname: &CStr, node: &Node, ddir: RawFd, dname: &CStr, mode: Mode, dest: &FsPath) -> io::Result<()> {
        let src = open_at(sdir, sname, libc::O_RDONLY | libc::O_NONBLOCK, 0)?;
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        check(unsafe { libc::fstat(src.as_raw_fd(), &mut st) })?;
        if st.st_mode & libc::S_IFMT != libc::S_IFREG || st.st_ino != node.ino {
            return Err(io::Error::other("the file changed while it was being copied"));
        }
        let base = self.job.progress().bytes;
        let deferred = {
            let dst = open_at(ddir, dname, libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL, 0o600)?;
            let r = (|| -> io::Result<Option<u64>> {
                let dev = self.dest_fs.as_ref().map(|(dev, _)| *dev);
                let writeback = if let Some(dev) = dev {
                    let dir = unsafe { BorrowedFd::borrow_raw(ddir) };
                    self.volumes.borrow_mut().open(dev, dir, dest, st.st_size as u64)?
                } else {
                    writeback::CopyMode::Buffered
                };
                let next = self.copy_data(src.as_raw_fd(), dst.as_raw_fd(), sparse(&st), writeback)?;
                if let Some(dev) = dev.filter(|_| matches!(writeback, writeback::CopyMode::Windowed(_))) {
                    self.volumes.borrow_mut().adjust(dev, next);
                }
                Ok(dev.filter(|_| writeback == writeback::CopyMode::Deferred))
            })();
            if r.is_ok() {
                keep_meta(src.as_raw_fd(), dst.as_raw_fd(), node, mode);
            }
            drop(dst);
            if r.is_err() {
                let _ = unlink_at(ddir, dname, false);
            }
            r?
        };
        if let Some(dev) = deferred {
            self.volumes.borrow_mut().defer(dev, node.size);
        }
        self.job.update(|p| {
            if deferred.is_none() {
                p.bytes = base + node.size;
            }
            p.items += 1;
        });
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn create(&self, sdir: RawFd, sname: &CStr, node: &Node, ddir: RawFd, dname: &CStr, mode: Mode, dest: &FsPath) -> io::Result<()> {
        if let Some((dev, _)) = self.dest_fs.as_ref().filter(|_| node.kind() != libc::S_IFREG) {
            let dir = unsafe { BorrowedFd::borrow_raw(ddir) };
            self.volumes.borrow_mut().open(*dev, dir, dest, 0)?;
        }
        match node.kind() {
            libc::S_IFDIR => check(unsafe { libc::mkdirat(ddir, dname.as_ptr(), 0o700) }),
            libc::S_IFREG => self.copy_file(sdir, sname, node, ddir, dname, mode, dest),
            libc::S_IFLNK => {
                let target = read_link(sdir, sname, node.size)?;
                check(unsafe { libc::symlinkat(target.as_ptr(), ddir, dname.as_ptr()) })?;
                keep_link_meta(ddir, dname, node);
                self.job.update(|p| p.items += 1);
                Ok(())
            }
            libc::S_IFIFO => {
                check(unsafe { libc::mknodat(ddir, dname.as_ptr(), libc::S_IFIFO | (node.mode & 0o777) as libc::mode_t, 0) })?;
                keep_link_meta(ddir, dname, node);
                self.job.update(|p| p.items += 1);
                Ok(())
            }
            _ => Err(io::Error::other("devices and sockets are not copied")),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn transfer(&mut self, sdir: RawFd, sname: &CStr, src: &FsPath, ddir: RawFd, mut dst: FsPath, node: &Node, mode: Mode) -> Res {
        if self.stopped() || src == &dst {
            return Res::Nothing;
        }
        if node.is_dir() && inside(&dst, src) {
            self.note(format!("{}: the destination folder is inside the source folder", src.display()));
            return Res::Nothing;
        }
        if let Some(safe) = (mode != Mode::Move).then(|| self.fat_safe_dst(ddir, &dst)).flatten() {
            dst = safe;
        }
        if mode != Mode::Move && !self.fits(ddir, src, &dst, node) {
            return Res::Nothing;
        }
        let group = self.job.has_companions(src);
        let mut replace = false;
        let dname = loop {
            let dname = match cstring(dst.file_name()) {
                Ok(n) => n,
                Err(e) => {
                    self.fail(&dst, &e);
                    return Res::Nothing;
                }
            };
            let occupied = group && !replace && node_at(ddir, &dname, false).is_ok();
            if !occupied && group {
                if let Some(error) = self.job.companion_collision(src, &dst) {
                    self.note(error);
                    return Res::Nothing;
                }
            }
            if replace {
                match self.replace_file((sdir, sname), (ddir, &dname, &dst), node, mode) {
                    Ok(()) => return Res::Whole(dst),
                    Err(e) if mode == Mode::Move && e.raw_os_error() == Some(libc::EXDEV) => return self.transfer(sdir, sname, src, ddir, dst, node, Mode::Across),
                    Err(e) if self.trouble(src, &e) => continue,
                    Err(_) => return Res::Nothing,
                }
            }
            let tried = if occupied {
                Err(errno(libc::EEXIST))
            } else {
                let _group = self.job.defer_companion_cancellation(src);
                match mode {
                    Mode::Move if self.job.ctl().replay().is_some() => rename_exact(sdir, sname, ddir, &dname),
                    Mode::Move => rename_at(sdir, sname, ddir, &dname, false),
                    _ => self.create(sdir, sname, node, ddir, &dname, mode, &dst),
                }
            };
            let Err(e) = tried else { break dname };
            if self.job.cancelled() {
                return Res::Nothing;
            }
            let blamed = if mode != Mode::Move && e.raw_os_error() == Some(libc::EROFS) { &dst } else { src };
            match e.raw_os_error() {
                Some(libc::EXDEV) if mode == Mode::Move => return self.transfer(sdir, sname, src, ddir, dst, node, Mode::Across),
                Some(libc::EEXIST) => {}
                _ if self.trouble(blamed, &e) => continue,
                _ => return Res::Nothing,
            }
            let existing = node_at(ddir, &dname, false).ok();
            match self.ask(src, node, &dst, existing) {
                Some(Resolve::KeepBoth) => dst = sibling(&dst, &numbered(ddir, dst.file_name(), node.is_dir(), b"", false)),
                Some(Resolve::Replace) if node.is_dir() => return self.merge(sdir, sname, src, ddir, &dname, &dst, node, mode),
                Some(Resolve::Replace) => replace = true,
                Some(Resolve::Skip | Resolve::SkipSame | Resolve::ReplaceOlder) => {
                    self.skip(node);
                    return Res::Nothing;
                }
                None => return Res::Nothing,
            }
        };
        let _group = self.job.defer_companion_cancellation(src);
        match (mode, node.is_dir()) {
            (Mode::Move, _) => {
                self.job.update(|p| p.items += 1);
                Res::Whole(dst)
            }
            (Mode::Across, false) => loop {
                match unlink_at(sdir, sname, false) {
                    Ok(()) => break Res::Whole(dst),
                    Err(e) if self.trouble(src, &e) => {}
                    Err(_) => {
                        let _ = unlink_at(ddir, &dname, false);
                        break Res::Nothing;
                    }
                }
            },
            (_, false) => Res::Whole(dst),
            (_, true) => {
                self.job.update(|p| p.items += 1);
                let (parts, complete) = self.children(sdir, sname, src, ddir, &dname, &dst, node, mode, true);
                match mode {
                    Mode::Copy => Res::Whole(dst),
                    _ if complete && unlink_at(sdir, sname, true).is_ok() => Res::Whole(dst),
                    _ => Res::Part(parts),
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn merge(&mut self, sdir: RawFd, sname: &CStr, src: &FsPath, ddir: RawFd, dname: &CStr, dst: &FsPath, node: &Node, mode: Mode) -> Res {
        let (parts, complete) = self.children(sdir, sname, src, ddir, dname, dst, node, mode, false);
        if mode != Mode::Copy && complete {
            let _ = unlink_at(sdir, sname, true);
        }
        Res::Part(parts)
    }

    #[allow(clippy::too_many_arguments)]
    fn children(&mut self, sdir: RawFd, sname: &CStr, src: &FsPath, ddir: RawFd, dname: &CStr, dst: &FsPath, node: &Node, mode: Mode, fresh: bool) -> (Pairs, bool) {
        let (sfd, dfd) = loop {
            match open_dir_at(sdir, sname).map_err(|e| (src, e)).and_then(|s| open_dir_at(ddir, dname).map(|d| (s, d)).map_err(|e| (dst, e))) {
                Ok(fds) => break fds,
                Err((p, e)) if self.trouble(p, &e) => {}
                Err(_) => return (Vec::new(), false),
            }
        };
        let list = loop {
            match names(sfd.as_raw_fd()) {
                Ok(l) => break l,
                Err(e) if self.trouble(src, &e) => {}
                Err(_) => return (Vec::new(), false),
            }
        };
        let (mut out, mut complete) = (Vec::new(), true);
        for n in &list {
            if self.stopped() {
                complete = false;
                break;
            }
            let s = src.join(n.to_bytes());
            let child = match node_at(sfd.as_raw_fd(), n, false) {
                Ok(c) => c,
                Err(e) if e.raw_os_error() == Some(libc::ENOENT) => continue,
                Err(e) => {
                    self.fail(&s, &e);
                    complete = false;
                    continue;
                }
            };
            if mode == Mode::Across && child.is_dir() && child.mounted_on(node.dev) {
                self.note(format!("{}: a file system is mounted here, so it was left in place", s.display()));
                complete = false;
                continue;
            }
            match self.transfer(sfd.as_raw_fd(), n, &s, dfd.as_raw_fd(), dst.join(n.to_bytes()), &child, mode) {
                Res::Whole(d) => out.push((s, d)),
                Res::Part(v) => {
                    out.extend(v);
                    complete = false;
                }
                Res::Nothing => complete = false,
            }
        }
        if fresh && mode != Mode::Move {
            keep_meta(sfd.as_raw_fd(), dfd.as_raw_fd(), node, mode);
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
            let item = top(&from).and_then(|item| open_parent(&to, false).map(|dest| (item, dest)));
            let (item, dest) = match item {
                Ok(opened) => opened,
                Err(error) => {
                    engine.fail(&from, &error);
                    continue;
                }
            };
            let result = engine.transfer(item.dir.as_raw_fd(), &item.name, &from, dest.as_raw_fd(), to.clone(), &item.node, mode);
            if matches!(result, Res::Nothing) && host(&to).is_some_and(|p| std::fs::symlink_metadata(p).is_ok()) {
                engine.note(flit_platform::companion_collision(&to));
            }
            engine.keep(&from, result);
        }
        if let Some(error) = engine.error() {
            self.note(error.to_string());
        }
        self.done.extend(engine.done);
    }

    fn copy(&mut self, items: &[FsPath], dest: &FsPath) {
        self.preparing();
        let dest_fd = loop {
            match host(dest).ok_or_else(|| errno(libc::EINVAL)).and_then(open_dir) {
                Ok(fd) => break fd,
                Err(e) if self.trouble(dest, &e) => {}
                Err(_) => return,
            }
        };
        let tops = self.tops(items);
        let mut total = Tally::default();
        for t in tops.iter().flatten() {
            self.tally(t.dir.as_raw_fd(), &t.name, &t.node, true, &mut total);
        }
        if !self.room(dest_fd.as_raw_fd(), dest, total.bytes) {
            return;
        }
        self.running(total);
        for (item, t) in items.iter().zip(&tops) {
            if self.stopped() {
                break;
            }
            let Some(t) = t else { continue };
            let dup = item.parent().as_ref() == Some(dest);
            let dst = if dup { dest.join(&numbered(dest_fd.as_raw_fd(), item.file_name(), t.node.is_dir(), flit_core::name::copy_suffix_in(self.lang).as_bytes(), true)) } else { dest.join(item.file_name()) };
            let r = self.transfer(t.dir.as_raw_fd(), &t.name, item, dest_fd.as_raw_fd(), dst, &t.node, Mode::Copy);
            self.keep_group(item, r, Mode::Copy);
        }
    }

    fn moves(&mut self, pairs: &[(FsPath, FsPath)], make_parents: bool) -> Vec<Option<FsPath>> {
        self.preparing();
        let froms: Vec<FsPath> = pairs.iter().map(|(f, _)| f.clone()).collect();
        let tops = self.tops(&froms);
        let mut dests = Vec::with_capacity(pairs.len());
        let mut total = Tally::default();
        let mut far: Vec<(u64, usize, u64)> = Vec::new();
        for ((_, to), t) in pairs.iter().zip(&tops) {
            let fd = match t {
                Some(_) => loop {
                    match open_parent(to, make_parents) {
                        Ok(fd) => break Some(fd),
                        Err(e) if self.trouble(to, &e) => {}
                        Err(_) => break None,
                    }
                },
                None => None,
            };
            if let (Some(t), Some(fd)) = (t, &fd) {
                match node_at(fd.as_raw_fd(), c".", false).map(|d| d.dev).ok().filter(|&dev| dev != t.node.dev) {
                    Some(dev) => {
                        let mut moved = Tally::default();
                        self.tally(t.dir.as_raw_fd(), &t.name, &t.node, false, &mut moved);
                        total.entries += moved.entries;
                        total.bytes += moved.bytes;
                        match far.iter_mut().find(|(d, ..)| *d == dev) {
                            Some((_, _, need)) => *need += moved.bytes,
                            None => far.push((dev, dests.len(), moved.bytes)),
                        }
                    }
                    None => total.entries += 1,
                }
            }
            dests.push(fd);
        }
        for (_, k, need) in far {
            let (Some(fd), Some(dir)) = (&dests[k], pairs[k].1.parent()) else { continue };
            if !self.room(fd.as_raw_fd(), &dir, need) {
                return vec![None; pairs.len()];
            }
        }
        self.running(total);
        let mut placed = vec![None; pairs.len()];
        for (k, ((from, to), t)) in pairs.iter().zip(&tops).enumerate() {
            if self.stopped() {
                break;
            }
            let (Some(t), Some(fd)) = (t, &dests[k]) else { continue };
            let r = self.transfer(t.dir.as_raw_fd(), &t.name, from, fd.as_raw_fd(), to.clone(), &t.node, Mode::Move);
            if let Res::Whole(to) = &r {
                placed[k] = Some(to.clone());
            }
            self.keep_group(from, r, Mode::Move);
        }
        placed
    }

    fn rename(&mut self, item: &FsPath, name: &[u8]) {
        if check_name(name, false).is_err() {
            return self.note(format!("\"{}\" is not a valid name", flit_core::name::display(name)));
        }
        let to = sibling(item, name);
        if let Some(bad) = open_parent(&to, false).ok().and_then(|d| self.bad_name_at(d.as_raw_fd(), name)) {
            return self.note(limits::bad_name_error(&to, bad));
        }
        let renamed = top(item).and_then(|t| {
            let (dir, target) = (t.dir.as_raw_fd(), cstring(name)?);
            if self.job.ctl().replay().is_some() {
                return rename_exact(dir, &t.name, dir, &target);
            }
            match rename_at(dir, &t.name, dir, &target, false) {
                Err(e) if e.raw_os_error() == Some(libc::EEXIST) && t.name != target && node_at(dir, &target, false).is_ok_and(|n| (n.dev, n.ino) == (t.node.dev, t.node.ino)) => rename_through_temporary(dir, &t.name, &target),
                r => r,
            }
        });
        match renamed {
            Ok(()) => self.done.push((item.clone(), to)),
            Err(e) if e.raw_os_error() == Some(libc::EEXIST) => self.note(format!("An item named \"{}\" already exists", flit_core::name::display(name))),
            Err(e) => self.fail(item, &e),
        }
    }

    fn new_folder(&mut self, dir: &FsPath, name: &[u8]) {
        if check_name(name, false).is_err() {
            return self.note(format!("\"{}\" is not a valid name", flit_core::name::display(name)));
        }
        let fd = match host(dir).ok_or_else(|| errno(libc::EINVAL)).and_then(open_dir) {
            Ok(fd) => fd,
            Err(e) => return self.fail(dir, &e),
        };
        let mut name = name.to_vec();
        loop {
            let to = dir.join(&name);
            let made = cstring(&name).and_then(|c| check(unsafe { libc::mkdirat(fd.as_raw_fd(), c.as_ptr(), 0o777) }));
            match made {
                Ok(()) => return self.done.push((to.clone(), to)),
                Err(e) if e.raw_os_error() == Some(libc::EEXIST) => name = numbered(fd.as_raw_fd(), &name, true, b"", false),
                Err(e) => return self.fail(&to, &e),
            }
        }
    }

    fn new_file(&mut self, dir: &FsPath, name: &[u8], template: &Template) {
        if check_name(name, false).is_err() {
            return self.note(format!("\"{}\" is not a valid name", flit_core::name::display(name)));
        }
        let fd = match host(dir).ok_or_else(|| errno(libc::EINVAL)).and_then(open_dir) {
            Ok(fd) => fd,
            Err(e) => return self.fail(dir, &e),
        };
        let source = match template {
            Template::Copy(src) => match host(src).ok_or_else(|| errno(libc::EINVAL)).and_then(std::fs::File::open) {
                Ok(f) => Some(f),
                Err(e) => return self.fail(src, &e),
            },
            _ => None,
        };
        let mode = if matches!(template, Template::Data(d) if d.starts_with(b"#!")) { 0o777 } else { 0o666 };
        let mut name = name.to_vec();
        loop {
            let to = dir.join(&name);
            let made = cstring(&name).and_then(|c| open_at(fd.as_raw_fd(), &c, libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL, mode));
            let file = match made {
                Ok(f) => std::fs::File::from(f),
                Err(e) if e.raw_os_error() == Some(libc::EEXIST) => {
                    name = numbered(fd.as_raw_fd(), &name, false, b"", false);
                    continue;
                }
                Err(e) => return self.fail(&to, &e),
            };
            let written = match (template, source) {
                (Template::Data(data), _) => (&file).write_all(data),
                (_, Some(mut src)) => io::copy(&mut src, &mut &file).map(drop),
                _ => Ok(()),
            };
            match written {
                Ok(()) => self.done.push((to.clone(), to)),
                Err(e) => self.fail(&to, &e),
            }
            return;
        }
    }

    fn link(&mut self, items: &[FsPath], dest: &FsPath) {
        let fd = match host(dest).ok_or_else(|| errno(libc::EINVAL)).and_then(open_dir) {
            Ok(fd) => fd,
            Err(e) => return self.fail(dest, &e),
        };
        for target in items {
            if self.stopped() {
                break;
            }
            let Some(source) = host(target).filter(|s| std::fs::symlink_metadata(s).is_ok()) else {
                self.note(flit_core::tr_en!(S::ErrorsEngineMissing, path = target.display()));
                continue;
            };
            let folder = std::fs::metadata(source).is_ok_and(|m| m.is_dir());
            let pointer = match cstring(target.as_bytes()) {
                Ok(c) => c,
                Err(e) => {
                    self.fail(target, &e);
                    continue;
                }
            };
            let mut name = flit_core::name::link_name_in(target.file_name(), self.lang);
            loop {
                let to = dest.join(&name);
                let made = cstring(&name).and_then(|c| check(unsafe { libc::symlinkat(pointer.as_ptr(), fd.as_raw_fd(), c.as_ptr()) }));
                match made {
                    Ok(()) => self.done.push((target.clone(), to)),
                    Err(e) if e.raw_os_error() == Some(libc::EEXIST) => {
                        name = numbered(fd.as_raw_fd(), &name, folder, b"", false);
                        continue;
                    }
                    Err(e) => self.fail(&to, &e),
                }
                break;
            }
        }
    }

    fn flatten_dir(&mut self, root_fd: RawFd, root: &FsPath, dfd: RawFd, dir: &FsPath, dev: u64) {
        let list = match names(dfd) {
            Ok(l) => l,
            Err(e) => return self.fail(dir, &e),
        };
        for n in &list {
            if self.stopped() {
                return;
            }
            let s = dir.join(n.to_bytes());
            let node = match node_at(dfd, n, false) {
                Ok(node) => node,
                Err(e) if e.raw_os_error() == Some(libc::ENOENT) => continue,
                Err(e) => {
                    self.fail(&s, &e);
                    continue;
                }
            };
            if node.is_dir() && node.mounted_on(dev) {
                self.note(format!("{}: a file system is mounted here, so it was left in place", s.display()));
            } else if node.is_dir() {
                match open_dir_at(dfd, n) {
                    Ok(sub) => self.flatten_dir(root_fd, root, sub.as_raw_fd(), &s, dev),
                    Err(e) => {
                        self.fail(&s, &e);
                        continue;
                    }
                }
                if unlink_at(dfd, n, true).is_ok() {
                    self.job.update(|p| p.items += 1);
                }
            } else if dir == root {
                self.skip(&node);
            } else {
                let r = self.transfer(dfd, n, &s, root_fd, root.join(n.to_bytes()), &node, Mode::Move);
                self.keep(&s, r);
            }
        }
    }

    fn flatten(&mut self, dirs: &[FsPath]) {
        self.preparing();
        let tops = self.tops(dirs);
        let mut total = Tally::default();
        for t in tops.iter().flatten() {
            self.tally(t.dir.as_raw_fd(), &t.name, &t.node, false, &mut total);
        }
        self.running(total);
        for (d, t) in dirs.iter().zip(&tops) {
            if self.stopped() {
                break;
            }
            let Some(t) = t else { continue };
            match open_dir_at(t.dir.as_raw_fd(), &t.name) {
                Ok(fd) => self.flatten_dir(fd.as_raw_fd(), d, fd.as_raw_fd(), d, t.node.dev),
                Err(e) => self.fail(d, &e),
            }
        }
    }

    fn delete_at(&mut self, dir: RawFd, name: &CStr, path: &FsPath, node: &Node) -> bool {
        if self.stopped() {
            return false;
        }
        let before = self.failed;
        if node.is_dir() {
            match open_dir_at(dir, name).and_then(|fd| names(fd.as_raw_fd()).map(|l| (fd, l))) {
                Ok((fd, list)) => {
                    for n in list {
                        if self.stopped() {
                            return false;
                        }
                        let p = path.join(n.to_bytes());
                        match node_at(fd.as_raw_fd(), &n, false) {
                            Ok(child) if child.is_dir() && child.mounted_on(node.dev) => self.note(format!("{}: a file system is mounted here, so it was left in place", p.display())),
                            Ok(child) => {
                                self.delete_at(fd.as_raw_fd(), &n, &p, &child);
                            }
                            Err(e) if e.raw_os_error() == Some(libc::ENOENT) => {}
                            Err(e) => self.fail(&p, &e),
                        }
                    }
                }
                Err(e) if self.trouble(path, &e) => return self.delete_at(dir, name, path, node),
                Err(_) => {}
            }
        }
        if self.stopped() {
            return false;
        }
        match unlink_at(dir, name, node.is_dir()) {
            Ok(()) => {
                self.counted(node);
                true
            }
            Err(e) => {
                let explained = e.raw_os_error() == Some(libc::ENOTEMPTY) && self.failed > before;
                !explained && self.trouble(path, &e) && self.delete_at(dir, name, path, node)
            }
        }
    }

    fn delete(&mut self, items: &[FsPath], state: Option<&Path>) {
        self.preparing();
        let tops = self.tops(items);
        let stages: Vec<_> = items.iter().zip(&tops).map(|(path, top)| {
            let state = state?;
            let top = top.as_ref().filter(|t| !t.node.mounted_on(t.parent_dev) && !self.stopped())?;
            let (stamp, _) = undo_stamp(path).ok()?;
            flit_platform::pending::stage_delete(state, path, stamp, false, |_, to| {
                let dir = top.dir.as_raw_fd();
                rename_exact(dir, &top.name, dir, &cstring(to.file_name())?)
            }).ok()
        }).collect();
        let mut total = Tally::default();
        let mut allowed = Vec::with_capacity(items.len());
        for ((item, t), stage) in items.iter().zip(&tops).zip(&stages) {
            let ok = match t {
                Some(t) if t.node.mounted_on(t.parent_dev) => {
                    self.note(format!("{}: a file system is mounted here, and Flit does not delete mount points", item.display()));
                    false
                }
                Some(t) => {
                    let name = stage.as_ref().and_then(|(p, _)| cstring(p.file_name()).ok());
                    self.tally(t.dir.as_raw_fd(), name.as_deref().unwrap_or(&t.name), &t.node, false, &mut total);
                    true
                }
                None => false,
            };
            allowed.push(ok);
        }
        self.running(total);
        for (((item, t), ok), stage) in items.iter().zip(&tops).zip(allowed).zip(stages) {
            let Some(t) = t.as_ref().filter(|_| ok) else { continue };
            let dir = t.dir.as_raw_fd();
            let deleted = if let Some((path, record)) = stage {
                let name = match cstring(path.file_name()) {
                    Ok(name) => name,
                    Err(error) => {
                        record.keep();
                        self.fail(&path, &error);
                        continue;
                    }
                };
                let matches = || undo_stamp(&path).is_ok_and(|(stamp, _)| record.matches(stamp));
                let deleted = if matches() && !self.stopped() {
                    let prepared = if t.node.is_dir() {
                        record.inventory(&path, undo_fs(), || self.stopped())
                    } else {
                        Ok(())
                    };
                    match prepared.and_then(|()| record.begin_delete()) {
                        Ok(()) => self.delete_at(dir, &name, &path, &t.node),
                        Err(error) => {
                            if !self.stopped() {
                                self.fail(item, &error);
                            }
                            false
                        }
                    }
                } else {
                    false
                };
                if deleted {
                    record.finish();
                } else {
                    record.keep();
                    let restored = if matches() { rename_noreplace(dir, &name, dir, &t.name) } else { Err(errno(libc::EBUSY)) };
                    match restored {
                        Ok(()) => record.finish(),
                        Err(error) => {
                            self.first = Some(flit_core::i18n::fill(S::ErrorsOpsSavedFile.in_lang(self.lang), &[
                                ("original", &item.display()), ("saved", &path.display()), ("error", &reason(&path, &error)),
                            ]));
                            self.failed += 1;
                        }
                    }
                }
                deleted
            } else {
                self.delete_at(dir, &t.name, item, &t.node)
            };
            if deleted {
                self.done.push((item.clone(), item.clone()));
            }
        }
    }

    fn trash(&mut self, items: &[FsPath], trash: Option<&Trash>) {
        let Some(trash) = trash else {
            self.untrashable.extend(items.iter().filter(|p| top(p).is_ok()).cloned());
            return self.note("The Trash folder is unknown: neither XDG_DATA_HOME nor HOME is set".into());
        };
        self.running(Tally { entries: items.len() as u64, bytes: 0 });
        for p in items {
            if self.stopped() {
                break;
            }
            let moved = host(p).ok_or_else(|| errno(libc::EINVAL)).and_then(|h| {
                if self.job.ctl().replay().is_some() { trash.trash_checked(h) } else { trash.trash(h) }
            });
            match moved.map(|to| fs_path(&to)) {
                Ok(Some(to)) => self.done.push((p.clone(), to)),
                Ok(None) => self.note(format!("{} was moved to the Trash under a name Flit cannot record", p.display())),
                Err(e) => {
                    if deletable_instead(p, &e) {
                        self.untrashable.push(p.clone());
                    }
                    match e.raw_os_error() {
                        Some(libc::EROFS) => self.note(failed_at(p, &e)),
                        _ => self.note(format!("Cannot move {} to the Trash: {}", p.display(), flit_core::fmt::io_reason(&e))),
                    }
                }
            }
            self.job.update(|q| q.items += 1);
        }
    }

    fn restore(&mut self, pairs: &[(FsPath, FsPath)], trash: Option<&Trash>) {
        let Some(trash) = trash else { return self.note("The Trash folder is unknown: neither XDG_DATA_HOME nor HOME is set".into()) };
        let mut infos = Vec::new();
        let mut valid = Vec::new();
        for (trashed, original) in pairs {
            match host(trashed).ok_or_else(|| errno(libc::EINVAL)).and_then(|h| trash.entry(h)) {
                Ok((info, from)) if fs_path(&from).as_ref() == Some(original) => {
                    infos.push(info);
                    valid.push((trashed.clone(), original.clone()));
                }
                Ok((_, from)) => self.note(format!("{} in the Trash came from {}, not from {}", trashed.display(), from.display(), original.display())),
                Err(e) => self.note(format!("Cannot restore {}: {}", trashed.display(), flit_core::fmt::io_reason(&e))),
            }
        }
        let placed = self.moves(&valid, true);
        for ((trashed, _), (info, to)) in valid.iter().zip(infos.iter().zip(placed)) {
            let gone = host(trashed).is_some_and(|h| std::fs::symlink_metadata(h).is_err());
            if to.is_some() || gone {
                let _ = std::fs::remove_file(info);
            }
        }
    }
}

fn pending_state() -> Option<std::path::PathBuf> {
    use flit_platform::IndexPlatform;
    if cfg!(test) {
        Some(std::env::temp_dir().join(format!("flit-pending-state-{}", std::process::id())))
    } else {
        crate::LinuxIndex::state_dir()
    }
}

pub fn start_pending_recovery() {
    let Some(state) = pending_state() else { return };
    static START: std::sync::Once = std::sync::Once::new();
    START.call_once(|| {
        if let Err(error) = std::thread::Builder::new().name("flit-pending-deletes".into()).spawn(move || {
            recover_pending(&state, process_is_live);
        }) {
            eprintln!("Flit could not start pending-delete recovery: {error}");
        }
    });
}

fn process_is_live(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else { return false };
    (unsafe { libc::kill(pid, 0) == 0 }) || io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

fn recover_pending(state: &Path, live: impl Fn(u32) -> bool) {
    flit_platform::pending::recover(state, live, undo_fs(), delete_pending, |staged, original, _| {
        let item = top(staged)?;
        let dir = item.dir.as_raw_fd();
        rename_noreplace(dir, &item.name, dir, &cstring(original.file_name())?)
    }, |_| Ok(()));
}

fn delete_pending(path: &FsPath) -> io::Result<()> {
    if path.file_name().ends_with(b".flit-partial") {
        let children = undo_children(path)?;
        for child in &children {
            if child.file_name() != b"new" || undo_stamp(child)?.1 {
                return Err(errno(libc::EEXIST));
            }
        }
        for child in children {
            delete_pending(&child)?;
        }
    }
    let item = top(path)?;
    if item.node.mounted_on(item.parent_dev) {
        return Err(errno(libc::EBUSY));
    }
    unlink_at(item.dir.as_raw_fd(), &item.name, item.node.is_dir())
}

pub fn run(op: &FileOp, job: &Job, trash: Option<&Trash>) -> OpOutcome {
    run_in(op, job, trash, flit_core::i18n::lang())
}

pub fn run_in(op: &FileOp, job: &Job, trash: Option<&Trash>, lang: Lang) -> OpOutcome {
    let mut e = Engine { lang, trash, ..Engine::new(job) };
    let linux = op.all_paths(|p| p.kind() == PathKind::Unix);
    job.set_copy_limit_available(linux && op.is_copy_or_move());
    match op {
        _ if !linux => e.note(NOT_LINUX.into()),
        FileOp::Copy { items, dest } => e.copy(items, dest),
        FileOp::Move { items, dest } => {
            let pairs: Vec<_> = items.iter().map(|i| (i.clone(), dest.join(i.file_name()))).collect();
            e.moves(&pairs, false);
        }
        FileOp::MoveTo(pairs) => {
            e.moves(pairs, true);
        }
        FileOp::Rename { item, name } => e.rename(item, name),
        FileOp::SetAccess { items, edit } => e.set_access(items, edit),
        FileOp::Access(changes) => e.access(changes),
        FileOp::NewFolder { dir, name } => e.new_folder(dir, name),
        FileOp::NewFile { dir, name, template } => e.new_file(dir, name, template),
        FileOp::Delete(items) => e.delete(items, pending_state().as_deref()),
        FileOp::Trash(items) => e.trash(items, trash),
        FileOp::Restore(pairs) => e.restore(pairs, trash),
        FileOp::Link { items, dest } => e.link(items, dest),
        FileOp::Flatten(dirs) => e.flatten(dirs),
        _ => e.note("The Linux agent does not run this operation".into()),
    }
    let (confirmed, failures) = e.volumes.get_mut().finish();
    if confirmed != 0 {
        job.update(|p| p.bytes += confirmed);
    }
    for (path, error) in failures {
        e.fail(&path, &error);
    }
    OpOutcome { error: e.error().map(|err| err.untrashable(std::mem::take(&mut e.untrashable))), items: e.done }
}

#[cfg(test)]
#[path = "../test/ops.rs"]
mod tests;
