use flit_core::listing::attr;
use flit_core::path::PathKind;
use flit_core::{FsError, FsErrorKind, FsPath, ListingBuilder, Meta};
use flit_platform::Fs;
use std::ffi::{CStr, CString, OsStr};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

const DENTS_BUFFER: usize = 64 << 10;
const BATCH: usize = 1024;
const EPOCH_GAP_SECS: i64 = 11_644_473_600;
const STATX_BASIC_STATS: u32 = 0x7ff;
const STATX_BTIME: u32 = 0x800;
const STATX_ATTR_MOUNT_ROOT: u64 = 0x2000;
const DT_DIR: u8 = 4;
const DT_LNK: u8 = 10;

#[repr(C)]
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stamp {
    pub sec: i64,
    pub nsec: u32,
    _reserved: i32,
}

impl Stamp {
    pub fn new(sec: i64, nsec: u32) -> Self {
        Self { sec, nsec, _reserved: 0 }
    }
}

#[repr(C)]
#[derive(Default)]
struct Statx {
    mask: u32,
    _blksize: u32,
    attributes: u64,
    _nlink: u32,
    uid: u32,
    gid: u32,
    mode: u16,
    _spare0: u16,
    ino: u64,
    size: u64,
    _blocks: u64,
    attributes_mask: u64,
    atime: Stamp,
    btime: Stamp,
    _ctime: Stamp,
    mtime: Stamp,
    _rdev: [u32; 2],
    dev: [u32; 2],
    _spare2: [u64; 14],
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Node {
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub size: u64,
    pub dev: u64,
    pub ino: u64,
    pub atime: Stamp,
    pub mtime: Stamp,
    pub btime: i64,
    pub mount_root: Option<bool>,
}

impl Node {
    pub fn kind(&self) -> u32 {
        self.mode & libc::S_IFMT
    }

    pub fn is_dir(&self) -> bool {
        self.kind() == libc::S_IFDIR
    }

    pub fn is_link(&self) -> bool {
        self.kind() == libc::S_IFLNK
    }

    pub fn mounted_on(&self, parent_dev: u64) -> bool {
        self.mount_root.unwrap_or(self.dev != parent_dev)
    }

    pub fn meta(&self, name: &[u8]) -> Meta {
        to_meta(name, self, || false)
    }
}

static NO_STATX: AtomicBool = AtomicBool::new(false);

pub fn filetime(sec: i64, nsec: i64) -> i64 {
    (sec + EPOCH_GAP_SECS) * 10_000_000 + nsec / 100
}

pub fn host(p: &FsPath) -> Option<&Path> {
    (p.kind() == PathKind::Unix).then(|| Path::new(OsStr::from_bytes(p.as_bytes())))
}

pub fn fs_error(e: &io::Error) -> FsError {
    let code = e.raw_os_error().unwrap_or(-1);
    let kind = match code {
        libc::ENOENT => FsErrorKind::NotFound,
        libc::EACCES | libc::EPERM => FsErrorKind::AccessDenied,
        libc::ENOTDIR => FsErrorKind::NotADirectory,
        _ => FsErrorKind::Other,
    };
    FsError { kind, code: code as i64 }
}

pub fn cstring(p: &[u8]) -> io::Result<CString> {
    CString::new(p).map_err(|_| io::Error::from_raw_os_error(libc::EINVAL))
}

fn dev_of(major: u32, minor: u32) -> u64 {
    (major as u64) << 32 | minor as u64
}

pub fn node_at(dir: libc::c_int, name: &CStr, follow: bool) -> io::Result<Node> {
    let nofollow = if follow { 0 } else { libc::AT_SYMLINK_NOFOLLOW };
    if !NO_STATX.load(Ordering::Relaxed) {
        let mut sx = Statx::default();
        let r = unsafe { libc::syscall(libc::SYS_statx, dir, name.as_ptr(), nofollow, STATX_BASIC_STATS | STATX_BTIME, &mut sx as *mut Statx) };
        if r == 0 {
            let btime = if sx.mask & STATX_BTIME != 0 { filetime(sx.btime.sec, sx.btime.nsec as i64) } else { 0 };
            let mount_root = (sx.attributes_mask & STATX_ATTR_MOUNT_ROOT != 0).then_some(sx.attributes & STATX_ATTR_MOUNT_ROOT != 0);
            return Ok(Node { mode: sx.mode as u32, uid: sx.uid, gid: sx.gid, size: sx.size, dev: dev_of(sx.dev[0], sx.dev[1]), ino: sx.ino, atime: sx.atime, mtime: sx.mtime, btime, mount_root });
        }
        let e = io::Error::last_os_error();
        if e.raw_os_error() != Some(libc::ENOSYS) {
            return Err(e);
        }
        NO_STATX.store(true, Ordering::Relaxed);
    }
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstatat(dir, name.as_ptr(), &mut st, nofollow) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(Node {
        mode: st.st_mode,
        uid: st.st_uid,
        gid: st.st_gid,
        size: st.st_size as u64,
        dev: dev_of(libc::major(st.st_dev), libc::minor(st.st_dev)),
        ino: st.st_ino,
        atime: Stamp::new(st.st_atime, st.st_atime_nsec as u32),
        mtime: Stamp::new(st.st_mtime, st.st_mtime_nsec as u32),
        btime: 0,
        mount_root: None,
    })
}

fn to_meta(name: &[u8], node: &Node, target_dir: impl FnOnce() -> bool) -> Meta {
    let mut attrs = 0;
    if node.is_link() {
        attrs |= attr::LINK;
        if target_dir() {
            attrs |= attr::DIR;
        }
    } else if node.is_dir() {
        attrs |= attr::DIR;
    }
    if name.first() == Some(&b'.') {
        attrs |= attr::HIDDEN;
    }
    if node.mode & 0o222 == 0 {
        attrs |= attr::READONLY;
    }
    Meta { size: node.size, modified: filetime(node.mtime.sec, node.mtime.nsec as i64), created: node.btime, attrs }
}

fn entry_meta(dir: libc::c_int, name: &CStr, d_type: u8) -> Meta {
    let bytes = name.to_bytes();
    match node_at(dir, name, false) {
        Ok(node) => to_meta(bytes, &node, || node_at(dir, name, true).is_ok_and(|t| t.is_dir())),
        Err(_) => {
            let attrs = match d_type {
                DT_DIR => attr::DIR,
                DT_LNK => attr::LINK,
                _ => 0,
            };
            Meta { attrs: attrs | if bytes.first() == Some(&b'.') { attr::HIDDEN } else { 0 }, ..Default::default() }
        }
    }
}

pub fn stat_path(path: &Path) -> io::Result<Meta> {
    let c = cstring(path.as_os_str().as_bytes())?;
    let node = node_at(libc::AT_FDCWD, &c, false)?;
    let name = path.file_name().map_or(&b""[..], |n| n.as_bytes());
    Ok(to_meta(name, &node, || node_at(libc::AT_FDCWD, &c, true).is_ok_and(|t| t.is_dir())))
}

fn owned(fd: libc::c_int) -> io::Result<OwnedFd> {
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

pub fn open_dir(path: &Path) -> io::Result<OwnedFd> {
    let c = cstring(path.as_os_str().as_bytes())?;
    owned(unsafe { libc::open(c.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC) })
}

pub fn open_dir_at(dir: libc::c_int, name: &CStr) -> io::Result<OwnedFd> {
    owned(unsafe { libc::openat(dir, name.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC) })
}

pub fn open_at(dir: libc::c_int, name: &CStr, flags: libc::c_int, mode: libc::mode_t) -> io::Result<OwnedFd> {
    owned(unsafe { libc::openat(dir, name.as_ptr(), flags | libc::O_NOFOLLOW | libc::O_CLOEXEC, mode as libc::c_uint) })
}

pub fn dents(fd: libc::c_int, mut each: impl FnMut(&CStr, u8) -> bool) -> io::Result<()> {
    let mut buf = vec![0u8; DENTS_BUFFER];
    loop {
        let n = unsafe { libc::syscall(libc::SYS_getdents64, fd, buf.as_mut_ptr(), buf.len()) };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        if n == 0 {
            return Ok(());
        }
        let mut off = 0;
        while off + 19 < n as usize {
            let reclen = u16::from_ne_bytes([buf[off + 16], buf[off + 17]]) as usize;
            if reclen < 20 || off + reclen > n as usize {
                return Err(io::Error::from_raw_os_error(libc::EIO));
            }
            let d_type = buf[off + 18];
            let Ok(name) = CStr::from_bytes_until_nul(&buf[off + 19..off + reclen]) else { break };
            off += reclen;
            if matches!(name.to_bytes(), b"." | b"..") {
                continue;
            }
            if !each(name, d_type) {
                return Ok(());
            }
        }
    }
}

pub fn names(fd: libc::c_int) -> io::Result<Vec<CString>> {
    let mut out = Vec::new();
    dents(fd, |name, _| {
        out.push(name.to_owned());
        true
    })?;
    Ok(out)
}

pub fn scan(dir: &Path, mut each: impl FnMut(&[u8], Meta) -> bool) -> io::Result<()> {
    let fd = open_dir(dir)?;
    dents(fd.as_raw_fd(), |name, d_type| each(name.to_bytes(), entry_meta(fd.as_raw_fd(), name, d_type)))
}

pub fn tree_size(root: &Path, keep_going: &dyn Fn() -> bool) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if !keep_going() {
            break;
        }
        let _ = scan(&dir, |name, m| {
            if m.attrs & attr::DIR != 0 && m.attrs & attr::LINK == 0 {
                stack.push(dir.join(OsStr::from_bytes(name)));
            } else if m.attrs & attr::DIR == 0 {
                total += m.size;
            }
            true
        });
    }
    total
}

pub fn dir_sizes(dir: &Path, items: &[(u32, PathBuf)], keep_going: &(dyn Fn() -> bool + Sync), report: &(dyn Fn(u32, u64) + Sync)) {
    let next = AtomicUsize::new(0);
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get()).min(items.len()).max(1);
    std::thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| {
                while let Some((idx, name)) = items.get(next.fetch_add(1, Ordering::Relaxed)) {
                    if !keep_going() {
                        return;
                    }
                    let size = tree_size(&dir.join(name), keep_going);
                    if keep_going() {
                        report(*idx, size);
                    }
                }
            });
        }
    });
}

pub struct LinuxFs;

impl Fs for LinuxFs {
    fn read_dir(&self, dir: &FsPath, out: &mut ListingBuilder<'_>) -> Result<(), FsError> {
        let path = host(dir).ok_or(FsError { kind: FsErrorKind::Unavailable, code: 0 })?;
        let mut n = 0usize;
        scan(path, |name, meta| {
            out.push(name, meta);
            n += 1;
            !n.is_multiple_of(BATCH) || out.batch_done()
        })
        .map_err(|e| fs_error(&e))?;
        out.batch_done();
        Ok(())
    }

    fn seek_penalty(&self, _: &FsPath) -> Option<bool> {
        None
    }

    fn dir_sizes(&self, dir: &FsPath, items: &[(u32, Vec<u8>)], keep_going: &(dyn Fn() -> bool + Sync), report: &(dyn Fn(u32, u64) + Sync)) {
        let Some(path) = host(dir) else { return };
        let items: Vec<(u32, PathBuf)> = items.iter().map(|(i, n)| (*i, PathBuf::from(OsStr::from_bytes(n)))).collect();
        dir_sizes(path, &items, keep_going, report);
    }
}

#[cfg(test)]
#[path = "../test/fs.rs"]
mod tests;
