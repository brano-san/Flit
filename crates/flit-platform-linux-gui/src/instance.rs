use crate::xdg;
use crossbeam_channel::{bounded, Receiver};
use flit_core::open::OpenTarget;
use flit_core::path::RECYCLE_BIN_SHELL;
use flit_core::Open;
use flit_platform::{Instance, OpenSink, PlatformError, SingleInstance, Wake};
use flit_platform_linux::percent_decode;
use flit_proto::instance::{Reply, Request, MAX_FRAME, VERSION};
use flit_proto::transport::{encode, read_frame_limited, read_hello, write_frame, Hello, Protocol};
use flit_proto::ProtoError;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{Duration, Instant};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const IO_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_CLIENTS: usize = 4;
pub const SHUTDOWN_SIGNALS: [libc::c_int; 3] = [libc::SIGTERM, libc::SIGHUP, libc::SIGINT];

pub struct SocketInstance {
    listener: UnixListener,
    _lock: File,
}

pub fn endpoints(runtime: &Path) -> (PathBuf, PathBuf) {
    let tag = std::env::var("FLIT_INSTANCE").map(|t| format!("-{t}")).unwrap_or_default();
    (runtime.join(format!("flit{tag}.lock")), runtime.join(format!("flit{tag}.sock")))
}

fn os_error(what: &str, e: &io::Error) -> PlatformError {
    PlatformError::new(e.raw_os_error().unwrap_or(-1) as i64, format!("{what}: {e}"))
}

fn proto_err(e: ProtoError) -> PlatformError {
    PlatformError::new(-1, format!("running Flit instance: {e}"))
}

pub fn peer_uid(s: &UnixStream) -> io::Result<u32> {
    let mut cred = libc::ucred { pid: 0, uid: u32::MAX, gid: 0 };
    let mut len = size_of::<libc::ucred>() as libc::socklen_t;
    let r = unsafe { libc::getsockopt(s.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PEERCRED, (&mut cred as *mut libc::ucred).cast(), &mut len) };
    if r != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(cred.uid)
}

fn same_user(s: &UnixStream) -> bool {
    peer_uid(s).is_ok_and(|uid| uid == unsafe { libc::getuid() })
}

fn connect(socket: &Path) -> io::Result<UnixStream> {
    let deadline = Instant::now() + CONNECT_TIMEOUT;
    loop {
        match UnixStream::connect(socket) {
            Ok(s) => return Ok(s),
            Err(e) if Instant::now() < deadline && matches!(e.kind(), io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused) => std::thread::sleep(Duration::from_millis(5)),
            Err(e) => return Err(e),
        }
    }
}

pub fn forward(socket: &Path, open: &Open) -> Result<(), PlatformError> {
    let mut s = connect(socket).map_err(|e| os_error("cannot reach the running Flit", &e))?;
    if !same_user(&s) {
        return Err(PlatformError::new(-1, "the single instance socket belongs to another user"));
    }
    let _ = s.set_read_timeout(Some(IO_TIMEOUT));
    let _ = s.set_write_timeout(Some(IO_TIMEOUT));
    let hello = Hello::new(Protocol::Instance, VERSION);
    let mut out = Vec::new();
    encode(&hello, &mut out).map_err(proto_err)?;
    encode(&Request::Open(open.clone()), &mut out).map_err(proto_err)?;
    s.write_all(&out).map_err(|e| proto_err(e.into()))?;
    read_hello(&mut s, &hello).map_err(proto_err)?;
    match read_frame_limited::<_, Reply>(&mut s, &mut Vec::new(), MAX_FRAME).map_err(proto_err)? {
        Reply::Accepted => Ok(()),
    }
}

fn serve(mut s: UnixStream, opens: &OpenSink, wake: &Wake) -> Result<(), ProtoError> {
    if !same_user(&s) {
        return Err(ProtoError::Invalid("the client runs as another user"));
    }
    let _ = s.set_read_timeout(Some(IO_TIMEOUT));
    let _ = s.set_write_timeout(Some(IO_TIMEOUT));
    let ours = Hello::new(Protocol::Instance, VERSION);
    let theirs = read_hello(&mut s, &ours);
    write_frame(&mut s, &ours)?;
    theirs?;
    let Request::Open(open) = read_frame_limited::<_, Request>(&mut s, &mut Vec::new(), MAX_FRAME)?;
    let sink = opens.lock().unwrap_or_else(|e| e.into_inner());
    if sink.as_ref().is_some_and(|tx| tx.send(open).is_ok()) {
        wake();
        write_frame(&mut s, &Reply::Accepted)?;
    }
    Ok(())
}

struct ClientSlot(Receiver<()>);

impl Drop for ClientSlot {
    fn drop(&mut self) {
        let _ = self.0.try_recv();
    }
}

fn lock(path: &Path) -> io::Result<Option<File>> {
    let file = OpenOptions::new().read(true).write(true).create(true).truncate(false).mode(0o600).open(path)?;
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        return Ok(Some(file));
    }
    let e = io::Error::last_os_error();
    if e.raw_os_error() == Some(libc::EWOULDBLOCK) { Ok(None) } else { Err(e) }
}

impl SocketInstance {
    pub fn acquire_in(runtime: &Path, open: &Open) -> Instance<Self> {
        let (lock_path, socket) = endpoints(runtime);
        match lock(&lock_path) {
            Ok(Some(file)) => {
                let _ = std::fs::remove_file(&socket);
                match UnixListener::bind(&socket) {
                    Ok(listener) => {
                        let _ = std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600));
                        Instance::Primary(SocketInstance { listener, _lock: file })
                    }
                    Err(e) => Instance::Standalone(os_error("cannot create the single instance socket", &e)),
                }
            }
            Ok(None) => match forward(&socket, open) {
                Ok(()) => Instance::Forwarded,
                Err(e) => Instance::Standalone(e),
            },
            Err(e) => Instance::Standalone(os_error("cannot lock the single instance file", &e)),
        }
    }
}

impl SingleInstance for SocketInstance {
    fn acquire(open: &Open) -> Instance<Self> {
        match xdg::runtime_dir() {
            Some(dir) => Self::acquire_in(&dir, open),
            None => Instance::Standalone(PlatformError::new(-1, "XDG_RUNTIME_DIR is not set, so Flit cannot find a running instance")),
        }
    }

    fn listen(self, opens: OpenSink, wake: Wake) {
        crate::dbus::start(opens.clone(), wake.clone());
        let _ = std::thread::Builder::new().name("flit-instance".into()).spawn(move || {
            let _lock = self._lock;
            let (taken, freed) = bounded::<()>(MAX_CLIENTS);
            for conn in self.listener.incoming() {
                let Ok(s) = conn else { continue };
                if taken.send(()).is_err() {
                    return;
                }
                let slot = ClientSlot(freed.clone());
                let (opens, wake) = (opens.clone(), wake.clone());
                let _ = std::thread::Builder::new().name("flit-instance-client".into()).spawn(move || {
                    let _slot = slot;
                    if let Err(e) = serve(s, &opens, &wake) {
                        eprintln!("flit: rejected open request: {e}");
                    }
                });
            }
        });
    }
}

pub(crate) fn decode_uri(arg: &[u8]) -> Option<Vec<u8>> {
    if arg.starts_with(b"trash:") {
        return Some(RECYCLE_BIN_SHELL.as_bytes().to_vec());
    }
    if arg.starts_with(b"computer:") {
        return Some(b"shell:MyComputerFolder".to_vec());
    }
    let rest = arg.strip_prefix(b"file://")?;
    let slash = rest.iter().position(|&b| b == b'/')?;
    let host = &rest[..slash];
    if !host.is_empty() && host != b"localhost" && host != hostname().as_slice() {
        return None;
    }
    let path = &rest[slash..];
    let path = &path[..path.iter().position(|&b| b == b'?' || b == b'#').unwrap_or(path.len())];
    Some(percent_decode(path))
}

fn hostname() -> Vec<u8> {
    let mut buf = [0u8; 256];
    if unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) } != 0 {
        return Vec::new();
    }
    buf[..buf.iter().position(|&b| b == 0).unwrap_or(buf.len())].to_vec()
}

fn location_arg(arg: &[u8]) -> Vec<u8> {
    decode_uri(arg).unwrap_or_else(|| arg.to_vec())
}

pub fn parse_args(args: &[Vec<u8>], cwd: Option<Vec<u8>>) -> Open {
    if args.iter().any(|a| a.eq_ignore_ascii_case(b"-tag") || a.eq_ignore_ascii_case(b"--tag")) {
        let mapped: Vec<Vec<u8>> = args.iter().map(|a| if a.eq_ignore_ascii_case(b"--tag") { b"-tag".to_vec() } else { location_arg(a) }).collect();
        return flit_core::open::parse_args(&mapped, cwd);
    }
    let mut open = Open { cwd, ..Default::default() };
    let mut it = args.iter();
    let mut positional_only = false;
    while let Some(a) = it.next() {
        if positional_only || !a.starts_with(b"-") || a.as_slice() == b"-" {
            open.paths.push(location_arg(a));
            continue;
        }
        match a.as_slice() {
            b"--" => positional_only = true,
            b"--new-tab" => open.target = OpenTarget::NewTab,
            b"--new-window" => open.target = OpenTarget::NewWindow,
            b"--current-tab" => open.target = OpenTarget::CurrentTab,
            b"--select" => open.select.extend(it.next().map(|v| location_arg(v))),
            other => {
                if let Some(v) = other.strip_prefix(b"--select=") {
                    open.select.push(location_arg(v));
                }
            }
        }
    }
    open
}

static SIGNAL_PIPE: AtomicI32 = AtomicI32::new(-1);

extern "C" fn on_signal(sig: libc::c_int) {
    let fd = SIGNAL_PIPE.load(Ordering::Relaxed);
    if fd >= 0 {
        let b = sig as u8;
        unsafe { libc::write(fd, (&b as *const u8).cast(), 1) };
    }
}

pub fn on_shutdown_signal(handler: impl Fn(i32) + Send + 'static) -> io::Result<()> {
    let mut fds = [0 as libc::c_int; 2];
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let (read_end, write_end) = (fds[0], fds[1]);
    unsafe { libc::fcntl(write_end, libc::F_SETFL, libc::O_NONBLOCK) };
    let mut reader = unsafe { <File as std::os::fd::FromRawFd>::from_raw_fd(read_end) };
    if SIGNAL_PIPE.compare_exchange(-1, write_end, Ordering::AcqRel, Ordering::Acquire).is_err() {
        unsafe { libc::close(write_end) };
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, "shutdown signals are already handled"));
    }
    std::thread::Builder::new().name("flit-signals".into()).spawn(move || {
        let mut seen = false;
        let mut b = [0u8; 1];
        while let Ok(1) = reader.read(&mut b) {
            let sig = b[0] as libc::c_int;
            if seen {
                unsafe {
                    libc::signal(sig, libc::SIG_DFL);
                    libc::raise(sig);
                }
                continue;
            }
            seen = true;
            handler(sig);
        }
    })?;
    for sig in SHUTDOWN_SIGNALS {
        let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
        action.sa_sigaction = on_signal as extern "C" fn(libc::c_int) as usize;
        action.sa_flags = libc::SA_RESTART;
        unsafe {
            libc::sigemptyset(&mut action.sa_mask);
            if libc::sigaction(sig, &action, std::ptr::null_mut()) != 0 {
                return Err(io::Error::last_os_error());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "../test/instance.rs"]
mod tests;
