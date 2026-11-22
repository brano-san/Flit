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
use std::ffi::{CStr, CString};
use std::io::{self, Write};
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

mod access;

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
    buf: Vec<u8>,
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

fn rename_at(sdir: RawFd, sname: &CStr, ddir: RawFd, dname: &CStr, replace: bool) -> io::Result<()> {
    if !replace {
        let r = unsafe { libc::syscall(libc::SYS_renameat2, sdir, sname.as_ptr(), ddir, dname.as_ptr(), RENAME_NOREPLACE) };
        if r == 0 {
            return Ok(());
        }
        let e = io::Error::last_os_error();
        if !matches!(e.raw_os_error(), Some(libc::EINVAL | libc::ENOSYS)) {
            return Err(e);
        }
        if node_at(ddir, dname, false).is_ok() {
            return Err(errno(libc::EEXIST));
        }
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
    lang: Lang,
    free: fn(RawFd) -> Option<u64>,
}

impl<'a> Engine<'a> {
    fn new(job: &'a Job) -> Self {
        Self { job, done: Vec::new(), failed: 0, first: None, first_kind: Default::default(), fatal: false, denied: false, untrashable: Vec::new(), dest_fs: None, lang: flit_core::i18n::lang(), free: free_space }
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
        if self.job.cancelled() {
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
        let dev = node_at(dir, c".", false).ok()?.dev;
        match &self.dest_fs {
            Some((known, fs)) if *known == dev => fs.clone(),
            _ => {
                let fs = crate::mounts::read().ok().and_then(|m| file_system(&m, dev));
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

    fn copy_data(&self, from: RawFd, to: RawFd, sparse: bool) -> io::Result<()> {
        let mut pump = Pump { base: self.job.progress().bytes, kernel: true, buf: Vec::new() };
        if !sparse {
            return self.pump(from, to, 0, u64::MAX, &mut pump).map(drop);
        }
        let mut at = 0;
        loop {
            let start = match seek(from, at, libc::SEEK_DATA) {
                Ok(s) => s,
                Err(e) if e.raw_os_error() == Some(libc::ENXIO) => break,
                Err(e) if at == 0 && matches!(e.raw_os_error(), Some(libc::EINVAL | libc::EOPNOTSUPP)) => return self.pump(from, to, 0, u64::MAX, &mut pump).map(drop),
                Err(e) => return Err(e),
            };
            let end = seek(from, start, libc::SEEK_HOLE)?;
            seek(from, start, libc::SEEK_SET)?;
            seek(to, start, libc::SEEK_SET)?;
            let copied = self.pump(from, to, start, end - start, &mut pump)?;
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
            if self.job.cancelled() {
                return Err(errno(libc::ECANCELED));
            }
            let want = (limit - done).min(CHUNK as u64) as usize;
            if want == 0 {
                return Ok(done);
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
                return Ok(done);
            }
            done += n as u64;
            let at = p.base + offset + done;
            self.job.update(|p| p.bytes = at);
        }
    }

    fn placed(&self, dir: RawFd, name: &CStr, replace: bool, make: impl FnOnce(&CStr) -> io::Result<()>) -> io::Result<()> {
        if !replace {
            return make(name);
        }
        let tmp = cstring(&numbered(dir, &[name.to_bytes(), b".flit-partial"].concat(), true, b"", true))?;
        let r = make(&tmp).and_then(|()| rename_at(dir, &tmp, dir, name, true));
        if r.is_err() {
            let _ = unlink_at(dir, &tmp, false);
        }
        r
    }

    #[allow(clippy::too_many_arguments)]
    fn copy_file(&self, sdir: RawFd, sname: &CStr, node: &Node, ddir: RawFd, dname: &CStr, replace: bool, mode: Mode) -> io::Result<()> {
        let src = open_at(sdir, sname, libc::O_RDONLY | libc::O_NONBLOCK, 0)?;
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        check(unsafe { libc::fstat(src.as_raw_fd(), &mut st) })?;
        if st.st_mode & libc::S_IFMT != libc::S_IFREG || st.st_ino != node.ino {
            return Err(io::Error::other("the file changed while it was being copied"));
        }
        let base = self.job.progress().bytes;
        self.placed(ddir, dname, replace, |name| {
            let dst = open_at(ddir, name, libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL, 0o600)?;
            let r = self.copy_data(src.as_raw_fd(), dst.as_raw_fd(), sparse(&st));
            if r.is_ok() {
                keep_meta(src.as_raw_fd(), dst.as_raw_fd(), node, mode);
            }
            drop(dst);
            if r.is_err() {
                let _ = unlink_at(ddir, name, false);
            }
            r
        })?;
        self.job.update(|p| {
            p.bytes = base + node.size;
            p.items += 1;
        });
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn create(&self, sdir: RawFd, sname: &CStr, node: &Node, ddir: RawFd, dname: &CStr, replace: bool, mode: Mode) -> io::Result<()> {
        match node.kind() {
            libc::S_IFDIR => check(unsafe { libc::mkdirat(ddir, dname.as_ptr(), 0o700) }),
            libc::S_IFREG => self.copy_file(sdir, sname, node, ddir, dname, replace, mode),
            libc::S_IFLNK => {
                let target = read_link(sdir, sname, node.size)?;
                self.placed(ddir, dname, replace, |name| {
                    check(unsafe { libc::symlinkat(target.as_ptr(), ddir, name.as_ptr()) })?;
                    keep_link_meta(ddir, name, node);
                    Ok(())
                })?;
                self.job.update(|p| p.items += 1);
                Ok(())
            }
            libc::S_IFIFO => {
                self.placed(ddir, dname, replace, |name| {
                    check(unsafe { libc::mknodat(ddir, name.as_ptr(), libc::S_IFIFO | (node.mode & 0o777) as libc::mode_t, 0) })?;
                    keep_link_meta(ddir, name, node);
                    Ok(())
                })?;
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
        let mut replace = false;
        let dname = loop {
            let dname = match cstring(dst.file_name()) {
                Ok(n) => n,
                Err(e) => {
                    self.fail(&dst, &e);
                    return Res::Nothing;
                }
            };
            let tried = match mode {
                Mode::Move => rename_at(sdir, sname, ddir, &dname, replace),
                _ => self.create(sdir, sname, node, ddir, &dname, replace, mode),
            };
            let Err(e) = tried else { break dname };
            if self.job.cancelled() {
                return Res::Nothing;
            }
            let blamed = if mode != Mode::Move && e.raw_os_error() == Some(libc::EROFS) { &dst } else { src };
            match e.raw_os_error() {
                Some(libc::EXDEV) if mode == Mode::Move => return self.transfer(sdir, sname, src, ddir, dst, node, Mode::Across),
                Some(libc::EEXIST) if !replace => {}
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
            self.keep(item, r);
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
            self.keep(from, r);
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

    fn delete(&mut self, items: &[FsPath]) {
        self.preparing();
        let tops = self.tops(items);
        let mut total = Tally::default();
        let mut allowed = Vec::with_capacity(items.len());
        for (item, t) in items.iter().zip(&tops) {
            let ok = match t {
                Some(t) if t.node.mounted_on(t.parent_dev) => {
                    self.note(format!("{}: a file system is mounted here, and Flit does not delete mount points", item.display()));
                    false
                }
                Some(t) => {
                    self.tally(t.dir.as_raw_fd(), &t.name, &t.node, false, &mut total);
                    true
                }
                None => false,
            };
            allowed.push(ok);
        }
        self.running(total);
        for ((item, t), ok) in items.iter().zip(&tops).zip(allowed) {
            if self.stopped() {
                break;
            }
            let Some(t) = t.as_ref().filter(|_| ok) else { continue };
            if self.delete_at(t.dir.as_raw_fd(), &t.name, item, &t.node) {
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
            let moved = host(p).ok_or_else(|| errno(libc::EINVAL)).and_then(|h| trash.trash(h));
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

pub fn run(op: &FileOp, job: &Job, trash: Option<&Trash>) -> OpOutcome {
    run_in(op, job, trash, flit_core::i18n::lang())
}

pub fn run_in(op: &FileOp, job: &Job, trash: Option<&Trash>, lang: Lang) -> OpOutcome {
    let mut e = Engine { lang, ..Engine::new(job) };
    let linux = op.all_paths(|p| p.kind() == PathKind::Unix);
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
        FileOp::Delete(items) => e.delete(items),
        FileOp::Trash(items) => e.trash(items, trash),
        FileOp::Restore(pairs) => e.restore(pairs, trash),
        FileOp::Link { items, dest } => e.link(items, dest),
        FileOp::Flatten(dirs) => e.flatten(dirs),
        _ => e.note("The Linux agent does not run this operation".into()),
    }
    OpOutcome { error: e.error().map(|err| err.untrashable(std::mem::take(&mut e.untrashable))), items: e.done }
}

#[cfg(test)]
#[path = "../test/ops.rs"]
mod tests;
