use crate::fs::{cstring, host, names, node_at, open_at, open_dir, open_dir_at, Node};
use crate::trash::Trash;
use flit_core::path::PathKind;
use flit_core::provider::{Conflict, FileOp, Resolve, Stage};
use flit_core::FsPath;
use flit_platform::{Job, OpOutcome, PlatformError, CANCELLED};
use flit_proto::listing::check_name;
use std::ffi::{CStr, CString};
use std::io;
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

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

fn times(node: &Node) -> [libc::timespec; 2] {
    let ts = |s: crate::fs::Stamp| libc::timespec { tv_sec: s.sec as _, tv_nsec: s.nsec as _ };
    [ts(node.atime), ts(node.mtime)]
}

fn keep_meta(fd: RawFd, node: &Node) {
    let owned = unsafe { libc::fchown(fd, node.uid, node.gid) } == 0;
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
    fatal: bool,
}

impl<'a> Engine<'a> {
    fn new(job: &'a Job) -> Self {
        Self { job, done: Vec::new(), failed: 0, first: None, fatal: false }
    }

    fn stopped(&self) -> bool {
        self.fatal || self.job.cancelled()
    }

    fn note(&mut self, msg: String) {
        self.failed += 1;
        self.first.get_or_insert(msg);
    }

    fn fail(&mut self, path: &FsPath, e: &io::Error) {
        if matches!(e.raw_os_error(), Some(libc::ENOSPC | libc::EDQUOT)) {
            self.fatal = true;
        }
        self.note(format!("{}: {e}", path.display()));
    }

    fn error(&self) -> Option<PlatformError> {
        if self.job.cancelled() {
            return Some(PlatformError::new(libc::ECANCELED as i64, CANCELLED));
        }
        let first = self.first.clone()?;
        Some(PlatformError::new(-1, if self.failed > 1 { format!("{} items failed. {first}", self.failed) } else { first }))
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
            .map(|p| match top(p) {
                Ok(t) => Some(t),
                Err(e) if e.raw_os_error() == Some(libc::ENOENT) => {
                    self.note(format!("{}: the item no longer exists", p.display()));
                    None
                }
                Err(e) => {
                    self.fail(p, &e);
                    None
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
        let Some(free) = free_space(fd).filter(|&free| free < need) else { return true };
        let mut msg = format!("There is not enough free space in {}: the items need ", dest.display());
        flit_core::fmt::size(&mut msg, need);
        msg.push_str(", but only ");
        flit_core::fmt::size(&mut msg, free);
        msg.push_str(" is free");
        self.note(msg);
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

    fn copy_data(&self, from: RawFd, to: RawFd) -> io::Result<()> {
        let base = self.job.progress().bytes;
        let (mut done, mut kernel, mut buf) = (0u64, true, Vec::new());
        loop {
            if self.job.cancelled() {
                return Err(errno(libc::ECANCELED));
            }
            let n = if kernel {
                let r = unsafe { libc::syscall(libc::SYS_copy_file_range, from, std::ptr::null_mut::<i64>(), to, std::ptr::null_mut::<i64>(), CHUNK, 0) };
                if r < 0 {
                    let e = io::Error::last_os_error();
                    if done == 0 && matches!(e.raw_os_error(), Some(libc::EXDEV | libc::ENOSYS | libc::EINVAL | libc::EOPNOTSUPP | libc::EPERM | libc::EBADF)) {
                        kernel = false;
                        continue;
                    }
                    return Err(e);
                }
                if r == 0 && done == 0 {
                    kernel = false;
                    continue;
                }
                r as usize
            } else {
                buf.resize(BUFFER, 0);
                let r = unsafe { libc::read(from, buf.as_mut_ptr().cast(), buf.len()) };
                if r < 0 {
                    let e = io::Error::last_os_error();
                    if e.kind() == io::ErrorKind::Interrupted {
                        continue;
                    }
                    return Err(e);
                }
                let mut rest = &buf[..r as usize];
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
                return Ok(());
            }
            done += n as u64;
            self.job.update(|p| p.bytes = base + done);
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

    fn copy_file(&self, sdir: RawFd, sname: &CStr, node: &Node, ddir: RawFd, dname: &CStr, replace: bool) -> io::Result<()> {
        let src = open_at(sdir, sname, libc::O_RDONLY | libc::O_NONBLOCK, 0)?;
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        check(unsafe { libc::fstat(src.as_raw_fd(), &mut st) })?;
        if st.st_mode & libc::S_IFMT != libc::S_IFREG || st.st_ino != node.ino {
            return Err(io::Error::other("the file changed while it was being copied"));
        }
        let base = self.job.progress().bytes;
        self.placed(ddir, dname, replace, |name| {
            let dst = open_at(ddir, name, libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL, 0o600)?;
            let r = self.copy_data(src.as_raw_fd(), dst.as_raw_fd());
            if r.is_ok() {
                keep_meta(dst.as_raw_fd(), node);
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

    fn create(&self, sdir: RawFd, sname: &CStr, node: &Node, ddir: RawFd, dname: &CStr, replace: bool) -> io::Result<()> {
        match node.kind() {
            libc::S_IFDIR => check(unsafe { libc::mkdirat(ddir, dname.as_ptr(), 0o700) }),
            libc::S_IFREG => self.copy_file(sdir, sname, node, ddir, dname, replace),
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
                _ => self.create(sdir, sname, node, ddir, &dname, replace),
            };
            let Err(e) = tried else { break dname };
            if self.job.cancelled() {
                return Res::Nothing;
            }
            match e.raw_os_error() {
                Some(libc::EXDEV) if mode == Mode::Move => return self.transfer(sdir, sname, src, ddir, dst, node, Mode::Across),
                Some(libc::EEXIST) if !replace => {}
                _ => {
                    self.fail(src, &e);
                    return Res::Nothing;
                }
            }
            let existing = node_at(ddir, &dname, false).ok();
            match self.ask(src, node, &dst, existing) {
                Some(Resolve::KeepBoth) => dst = sibling(&dst, &numbered(ddir, dst.file_name(), node.is_dir(), b"", false)),
                Some(Resolve::Replace) if node.is_dir() => return self.merge(sdir, sname, src, ddir, &dname, &dst, node, mode),
                Some(Resolve::Replace) => replace = true,
                Some(Resolve::Skip) => {
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
            (Mode::Across, false) => match unlink_at(sdir, sname, false) {
                Ok(()) => Res::Whole(dst),
                Err(e) => {
                    let _ = unlink_at(ddir, &dname, false);
                    self.fail(src, &e);
                    Res::Nothing
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
        let opened = open_dir_at(sdir, sname).map_err(|e| (src, e)).and_then(|s| open_dir_at(ddir, dname).map(|d| (s, d)).map_err(|e| (dst, e)));
        let (sfd, dfd) = match opened {
            Ok(fds) => fds,
            Err((p, e)) => {
                self.fail(p, &e);
                return (Vec::new(), false);
            }
        };
        let list = match names(sfd.as_raw_fd()) {
            Ok(l) => l,
            Err(e) => {
                self.fail(src, &e);
                return (Vec::new(), false);
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
            keep_meta(dfd.as_raw_fd(), node);
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
        let dest_fd = match host(dest).ok_or_else(|| errno(libc::EINVAL)).and_then(open_dir) {
            Ok(fd) => fd,
            Err(e) => return self.fail(dest, &e),
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
            let dst = if dup { dest.join(&numbered(dest_fd.as_raw_fd(), item.file_name(), t.node.is_dir(), b" - Copy", true)) } else { dest.join(item.file_name()) };
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
        for ((_, to), t) in pairs.iter().zip(&tops) {
            let fd = match t.as_ref().map(|_| open_parent(to, make_parents)) {
                Some(Ok(fd)) => Some(fd),
                Some(Err(e)) => {
                    self.fail(to, &e);
                    None
                }
                None => None,
            };
            if let (Some(t), Some(fd)) = (t, &fd) {
                let across = node_at(fd.as_raw_fd(), c".", false).is_ok_and(|d| d.dev != t.node.dev);
                if across {
                    self.tally(t.dir.as_raw_fd(), &t.name, &t.node, false, &mut total);
                } else {
                    total.entries += 1;
                }
            }
            dests.push(fd);
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
        let renamed = top(item).and_then(|t| {
            let target = cstring(name)?;
            rename_at(t.dir.as_raw_fd(), &t.name, t.dir.as_raw_fd(), &target, false)
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
                Err(e) => self.fail(path, &e),
            }
        }
        match unlink_at(dir, name, node.is_dir()) {
            Ok(()) => {
                self.counted(node);
                true
            }
            Err(e) => {
                let explained = e.raw_os_error() == Some(libc::ENOTEMPTY) && self.failed > before;
                if !self.job.cancelled() && !explained {
                    self.fail(path, &e);
                }
                false
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
        let Some(trash) = trash else { return self.note("The Trash folder is unknown: neither XDG_DATA_HOME nor HOME is set".into()) };
        self.running(Tally { entries: items.len() as u64, bytes: 0 });
        for p in items {
            if self.stopped() {
                break;
            }
            let moved = host(p).ok_or_else(|| errno(libc::EINVAL)).and_then(|h| trash.trash(h));
            match moved.map(|to| fs_path(&to)) {
                Ok(Some(to)) => self.done.push((p.clone(), to)),
                Ok(None) => self.note(format!("{} was moved to the Trash under a name Flit cannot record", p.display())),
                Err(e) => self.note(format!("Cannot move {} to the Trash: {e}", p.display())),
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
                Err(e) => self.note(format!("Cannot restore {}: {e}", trashed.display())),
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
    let mut e = Engine::new(job);
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
        FileOp::NewFolder { dir, name } => e.new_folder(dir, name),
        FileOp::Delete(items) => e.delete(items),
        FileOp::Trash(items) => e.trash(items, trash),
        FileOp::Restore(pairs) => e.restore(pairs, trash),
        _ => e.note("The Linux agent does not run this operation".into()),
    }
    OpOutcome { error: e.error(), items: e.done }
}

#[cfg(test)]
#[path = "../test/ops.rs"]
mod tests;
