use crossbeam_channel::{bounded, Receiver};
use flit_platform::{Duplex, OnClient, PlatformError};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub const SOCKET: &str = "flit/indexd.sock";
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);
const BUSY_RETRY: Duration = Duration::from_millis(50);
const ACCEPT_RETRY: Duration = Duration::from_millis(100);
const FIRST_LISTEN_FD: RawFd = 3;

pub fn endpoint() -> Option<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).filter(|d| d.is_absolute()).map(|d| d.join(SOCKET))
}

pub fn uid() -> u32 {
    unsafe { libc::geteuid() }
}

fn fail(what: impl std::fmt::Display) -> PlatformError {
    PlatformError::new(-1, what.to_string())
}

pub struct UnixConn {
    stream: UnixStream,
    timeout: Option<Duration>,
}

impl UnixConn {
    fn new(stream: UnixStream) -> io::Result<Self> {
        stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
        Ok(Self { stream, timeout: None })
    }

    pub fn peer_uid(&self) -> io::Result<u32> {
        peer_uid(self.stream.as_raw_fd())
    }
}

fn peer_uid(fd: RawFd) -> io::Result<u32> {
    let mut cred = libc::ucred { pid: 0, uid: 0, gid: 0 };
    let mut len = size_of::<libc::ucred>() as libc::socklen_t;
    if unsafe { libc::getsockopt(fd, libc::SOL_SOCKET, libc::SO_PEERCRED, (&mut cred as *mut libc::ucred).cast(), &mut len) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(cred.uid)
}

fn wait_readable(fd: RawFd, limit: Duration) -> io::Result<()> {
    let deadline = Instant::now() + limit;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let ms = left.as_micros().div_ceil(1000).min(i32::MAX as u128) as i32;
        let mut p = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
        match unsafe { libc::poll(&mut p, 1, ms) } {
            0 => return Err(io::ErrorKind::TimedOut.into()),
            n if n > 0 => return Ok(()),
            _ => {
                let e = io::Error::last_os_error();
                if e.kind() != io::ErrorKind::Interrupted {
                    return Err(e);
                }
            }
        }
    }
}

impl Read for UnixConn {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if let Some(limit) = self.timeout {
            wait_readable(self.stream.as_raw_fd(), limit)?;
        }
        match self.stream.read(buf) {
            Err(e) if e.kind() == io::ErrorKind::ConnectionReset => Ok(0),
            r => r,
        }
    }
}

impl Write for UnixConn {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        loop {
            let n = unsafe { libc::send(self.stream.as_raw_fd(), buf.as_ptr().cast(), buf.len(), libc::MSG_NOSIGNAL) };
            if n >= 0 {
                return Ok(n as usize);
            }
            let e = io::Error::last_os_error();
            match e.kind() {
                io::ErrorKind::Interrupted => {}
                io::ErrorKind::WouldBlock => return Err(io::ErrorKind::TimedOut.into()),
                _ => return Err(e),
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Duplex for UnixConn {
    fn try_clone(&self) -> io::Result<Self> {
        Ok(Self { stream: self.stream.try_clone()?, timeout: self.timeout })
    }

    fn set_read_timeout(&mut self, timeout: Option<Duration>) {
        self.timeout = timeout;
    }
}

fn private_dir(dir: &Path, owner: u32) -> Result<(), PlatformError> {
    let m = std::fs::symlink_metadata(dir).map_err(|e| fail(format_args!("{}: {e}", dir.display())))?;
    if !m.is_dir() || m.uid() != owner || m.mode() & 0o077 != 0 {
        return Err(fail(format_args!("{} is not a folder private to this user", dir.display())));
    }
    Ok(())
}

pub fn trusted(path: &Path, owner: u32) -> Result<(), PlatformError> {
    private_dir(path.parent().ok_or_else(|| fail("the index socket has no folder"))?, owner)?;
    let m = std::fs::symlink_metadata(path).map_err(|e| fail(format_args!("{}: {e}", path.display())))?;
    if !m.file_type().is_socket() || m.uid() != owner || m.mode() & 0o077 != 0 {
        return Err(fail(format_args!("{} is not a socket private to this user", path.display())));
    }
    Ok(())
}

fn address(path: &Path) -> io::Result<(libc::sockaddr_un, libc::socklen_t)> {
    let bytes = path.as_os_str().as_bytes();
    let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    addr.sun_family = libc::AF_UNIX as libc::sa_family_t;
    if bytes.len() >= addr.sun_path.len() || bytes.contains(&0) {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    for (d, s) in addr.sun_path.iter_mut().zip(bytes) {
        *d = *s as libc::c_char;
    }
    Ok((addr, (std::mem::offset_of!(libc::sockaddr_un, sun_path) + bytes.len() + 1) as libc::socklen_t))
}

fn connect_within(path: &Path, limit: Duration) -> io::Result<UnixStream> {
    let (addr, len) = address(path)?;
    let deadline = Instant::now() + limit;
    loop {
        let raw = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK, 0) };
        if raw < 0 {
            return Err(io::Error::last_os_error());
        }
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };
        if unsafe { libc::connect(fd.as_raw_fd(), (&addr as *const libc::sockaddr_un).cast(), len) } == 0 {
            let stream = UnixStream::from(fd);
            stream.set_nonblocking(false)?;
            return Ok(stream);
        }
        let e = io::Error::last_os_error();
        match e.raw_os_error() {
            Some(libc::EINTR) => {}
            Some(libc::EAGAIN) if Instant::now() < deadline => std::thread::sleep(BUSY_RETRY),
            Some(libc::EAGAIN) => return Err(io::ErrorKind::TimedOut.into()),
            _ => return Err(e),
        }
    }
}

pub fn connect_at(path: &Path, owner: u32, limit: Duration) -> Result<UnixConn, PlatformError> {
    trusted(path, owner)?;
    let stream = connect_within(path, limit).map_err(|e| fail(format_args!("{}: {e}", path.display())))?;
    match peer_uid(stream.as_raw_fd()) {
        Ok(peer) if peer == owner => UnixConn::new(stream).map_err(fail),
        Ok(_) => Err(fail("the index service socket is served by another user")),
        Err(e) => Err(fail(e)),
    }
}

pub struct Bound {
    listener: UnixListener,
    _lock: Option<std::fs::File>,
}

impl Bound {
    pub fn listener(&self) -> &UnixListener {
        &self.listener
    }
}

pub fn lock_path(socket: &Path) -> PathBuf {
    socket.with_extension("lock")
}

fn lock(socket: &Path) -> Result<std::fs::File, PlatformError> {
    use std::os::unix::fs::OpenOptionsExt;
    let path = lock_path(socket);
    let file = std::fs::OpenOptions::new().read(true).write(true).create(true).mode(0o600).custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC).open(&path).map_err(|e| fail(format_args!("{}: {e}", path.display())))?;
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        let e = io::Error::last_os_error();
        if e.kind() == io::ErrorKind::WouldBlock {
            return Err(fail(format_args!("another flit-indexd is starting or already serves {}", socket.display())));
        }
        return Err(fail(format_args!("{}: {e}", path.display())));
    }
    Ok(file)
}

pub fn stale(probe: &io::Result<UnixStream>) -> bool {
    matches!(probe, Err(e) if e.raw_os_error() == Some(libc::ECONNREFUSED))
}

pub fn bind_at(path: &Path, owner: u32) -> Result<Bound, PlatformError> {
    let dir = path.parent().ok_or_else(|| fail("the index socket has no folder"))?;
    match std::fs::DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(fail(format_args!("{}: {e}", dir.display()))),
    }
    private_dir(dir, owner)?;
    let held = lock(path)?;
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_socket() && m.uid() == owner => {
            let probe = connect_within(path, CONNECT_TIMEOUT);
            if !stale(&probe) {
                return Err(fail(format_args!("another flit-indexd already serves {}", path.display())));
            }
            std::fs::remove_file(path).map_err(|e| fail(format_args!("{}: {e}", path.display())))?;
        }
        Ok(_) => return Err(fail(format_args!("{} exists and is not this user's socket", path.display()))),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(fail(format_args!("{}: {e}", path.display()))),
    }
    let listener = UnixListener::bind(path).map_err(|e| fail(format_args!("{}: {e}", path.display())))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(|e| fail(format_args!("{}: {e}", path.display())))?;
    Ok(Bound { listener, _lock: Some(held) })
}

pub fn listen_fd(pid: Option<&str>, fds: Option<&str>, me: u32) -> Option<RawFd> {
    let pid: u32 = pid?.trim().parse().ok()?;
    let fds: u32 = fds?.trim().parse().ok()?;
    (pid == me && fds >= 1).then_some(FIRST_LISTEN_FD)
}

pub fn activated() -> Option<Bound> {
    let (pid, fds) = (std::env::var("LISTEN_PID").ok(), std::env::var("LISTEN_FDS").ok());
    let fd = listen_fd(pid.as_deref(), fds.as_deref(), std::process::id())?;
    let mut kind: libc::c_int = 0;
    let mut len = size_of::<libc::c_int>() as libc::socklen_t;
    let ok = unsafe { libc::getsockopt(fd, libc::SOL_SOCKET, libc::SO_TYPE, (&mut kind as *mut libc::c_int).cast(), &mut len) } == 0;
    if !ok || kind != libc::SOCK_STREAM {
        return None;
    }
    unsafe {
        libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
    }
    let listener = unsafe { UnixListener::from_raw_fd(fd) };
    listener.local_addr().ok()?.as_pathname()?;
    Some(Bound { listener, _lock: None })
}

struct Slot(Receiver<()>);

impl Drop for Slot {
    fn drop(&mut self) {
        let _ = self.0.try_recv();
    }
}

pub fn serve_on(bound: Bound, owner: u32, max_clients: usize, on_client: OnClient<UnixConn>) -> Result<(), PlatformError> {
    let (taken, freed) = bounded::<()>(max_clients.max(1));
    loop {
        if taken.send(()).is_err() {
            return Ok(());
        }
        let slot = Slot(freed.clone());
        let stream = match bound.listener.accept() {
            Ok((s, _)) => s,
            Err(e) if matches!(e.kind(), io::ErrorKind::Interrupted | io::ErrorKind::ConnectionAborted) => continue,
            Err(e) if matches!(e.raw_os_error(), Some(libc::EMFILE | libc::ENFILE | libc::ENOBUFS | libc::ENOMEM)) => {
                std::thread::sleep(ACCEPT_RETRY);
                continue;
            }
            Err(e) => return Err(fail(format_args!("the index socket stopped accepting clients: {e}"))),
        };
        if peer_uid(stream.as_raw_fd()).ok() != Some(owner) {
            continue;
        }
        let Ok(conn) = UnixConn::new(stream) else { continue };
        let on_client = on_client.clone();
        let _ = std::thread::Builder::new().name("flit-indexd-client".into()).spawn(move || {
            let _slot = slot;
            on_client(conn);
        });
    }
}

#[cfg(test)]
#[path = "../test/sock.rs"]
mod tests;
