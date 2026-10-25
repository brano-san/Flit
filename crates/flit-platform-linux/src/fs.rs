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
const DT_DIR: u8 = 4;
const DT_LNK: u8 = 10;

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct Stamp {
    sec: i64,
    nsec: u32,
    _reserved: i32,
}

#[repr(C)]
#[derive(Default)]
struct Statx {
    mask: u32,
    _blksize: u32,
    _attributes: u64,
    _nlink: u32,
    _uid: u32,
    _gid: u32,
    mode: u16,
    _spare0: u16,
    _ino: u64,
    size: u64,
    _blocks: u64,
    _attributes_mask: u64,
    _atime: Stamp,
    btime: Stamp,
    _ctime: Stamp,
    mtime: Stamp,
    _rdev: [u32; 2],
    _dev: [u32; 2],
    _spare2: [u64; 14],
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

fn cstring(p: &Path) -> io::Result<CString> {
    CString::new(p.as_os_str().as_bytes()).map_err(|_| io::Error::from_raw_os_error(libc::EINVAL))
}

struct Raw {
    mode: u32,
    size: u64,
    mtime: i64,
    btime: i64,
}

fn statx_at(dir: libc::c_int, name: &CStr, follow: bool) -> io::Result<Raw> {
    let nofollow = if follow { 0 } else { libc::AT_SYMLINK_NOFOLLOW };
    if !NO_STATX.load(Ordering::Relaxed) {
        let mut sx = Statx::default();
        let r = unsafe { libc::syscall(libc::SYS_statx, dir, name.as_ptr(), nofollow, STATX_BASIC_STATS | STATX_BTIME, &mut sx as *mut Statx) };
        if r == 0 {
            let btime = if sx.mask & STATX_BTIME != 0 { filetime(sx.btime.sec, sx.btime.nsec as i64) } else { 0 };
            return Ok(Raw { mode: sx.mode as u32, size: sx.size, mtime: filetime(sx.mtime.sec, sx.mtime.nsec as i64), btime });
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
    Ok(Raw { mode: st.st_mode, size: st.st_size as u64, mtime: filetime(st.st_mtime, st.st_mtime_nsec), btime: 0 })
}

fn is_dir(mode: u32) -> bool {
    mode & libc::S_IFMT == libc::S_IFDIR
}

fn to_meta(name: &[u8], raw: &Raw, target_dir: impl FnOnce() -> bool) -> Meta {
    let mut attrs = 0;
    if raw.mode & libc::S_IFMT == libc::S_IFLNK {
        attrs |= attr::LINK;
        if target_dir() {
            attrs |= attr::DIR;
        }
    } else if is_dir(raw.mode) {
        attrs |= attr::DIR;
    }
    if name.first() == Some(&b'.') {
        attrs |= attr::HIDDEN;
    }
    if raw.mode & 0o222 == 0 {
        attrs |= attr::READONLY;
    }
    Meta { size: raw.size, modified: raw.mtime, created: raw.btime, attrs }
}

fn entry_meta(dir: libc::c_int, name: &CStr, d_type: u8) -> Meta {
    let bytes = name.to_bytes();
    match statx_at(dir, name, false) {
        Ok(raw) => to_meta(bytes, &raw, || statx_at(dir, name, true).is_ok_and(|t| is_dir(t.mode))),
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
    let c = cstring(path)?;
    let raw = statx_at(libc::AT_FDCWD, &c, false)?;
    let name = path.file_name().map_or(&b""[..], |n| n.as_bytes());
    Ok(to_meta(name, &raw, || statx_at(libc::AT_FDCWD, &c, true).is_ok_and(|t| is_dir(t.mode))))
}

fn open_dir(path: &Path) -> io::Result<OwnedFd> {
    let c = cstring(path)?;
    let fd = unsafe { libc::open(c.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

pub fn scan(dir: &Path, mut each: impl FnMut(&[u8], Meta) -> bool) -> io::Result<()> {
    let fd = open_dir(dir)?;
    let mut buf = vec![0u8; DENTS_BUFFER];
    loop {
        let n = unsafe { libc::syscall(libc::SYS_getdents64, fd.as_raw_fd(), buf.as_mut_ptr(), buf.len()) };
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
            if !each(name.to_bytes(), entry_meta(fd.as_raw_fd(), name, d_type)) {
                return Ok(());
            }
        }
    }
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
