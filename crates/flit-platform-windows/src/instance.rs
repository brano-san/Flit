use crate::err;
use flit_core::Open;
use flit_platform::{Instance, OpenSink, PlatformError, SingleInstance, Wake};
use crossbeam_channel::{bounded, Receiver};
use flit_proto::instance::{Reply, Request, MAX_FRAME, VERSION};
use flit_proto::transport::{read_frame_limited, read_hello, write_frame, Hello, Protocol};
use flit_proto::ProtoError;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::time::{Duration, Instant};
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Security::Authorization::{ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1};
use windows::Win32::Security::{GetTokenInformation, TokenElevation, TokenUser, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_ELEVATION, TOKEN_QUERY, TOKEN_USER};
use windows::Win32::Storage::FileSystem::*;
use windows::Win32::System::Pipes::*;
use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows::Win32::System::Threading::{CreateMutexW, GetCurrentProcess, GetCurrentProcessId, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION};
use windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const READ_TIMEOUT: Duration = Duration::from_secs(2);
const READ_POLL: Duration = Duration::from_millis(2);
const MAX_CLIENTS: usize = 4;

pub struct PipeInstance {
    pipe: Vec<u16>,
}

pub(crate) fn user_sid() -> Option<String> {
    process_user_sid(unsafe { GetCurrentProcess() })
}

pub(crate) fn elevated() -> bool {
    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION::default();
        let mut len = 0u32;
        let got = GetTokenInformation(token, TokenElevation, Some(&mut elevation as *mut _ as _), size_of::<TOKEN_ELEVATION>() as u32, &mut len);
        let _ = CloseHandle(token);
        got.is_ok() && elevation.TokenIsElevated != 0
    }
}

pub(crate) fn process_user_sid(process: HANDLE) -> Option<String> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(process, TOKEN_QUERY, &mut token).ok()?;
        let mut len = 0u32;
        let _ = GetTokenInformation(token, TokenUser, None, 0, &mut len);
        let mut buf = vec![0u64; (len as usize).div_ceil(8)];
        let got = GetTokenInformation(token, TokenUser, Some(buf.as_mut_ptr() as _), len, &mut len);
        let _ = CloseHandle(token);
        got.ok()?;
        let user = &*(buf.as_ptr() as *const TOKEN_USER);
        let mut s = PWSTR::null();
        ConvertSidToStringSidW(user.User.Sid, &mut s).ok()?;
        let out = s.to_string().ok();
        LocalFree(Some(HLOCAL(s.0 as _)));
        out
    }
}

fn names() -> (Vec<u16>, Vec<u16>) {
    let sid = user_sid().unwrap_or_else(|| "unknown".into());
    let mut session = 0u32;
    unsafe {
        let _ = ProcessIdToSessionId(GetCurrentProcessId(), &mut session);
    }
    let wide = |s: String| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let suffix = if elevated() { "-admin" } else { "" };
    (wide(format!(r"Local\flit-{sid}{suffix}")), wide(format!(r"\\.\pipe\flit-{sid}-{session}{suffix}")))
}

fn same_user_server(f: &File) -> Option<u32> {
    let mut pid = 0u32;
    unsafe {
        GetNamedPipeServerProcessId(HANDLE(f.as_raw_handle()), &mut pid).ok()?;
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let sid = process_user_sid(process);
        let _ = CloseHandle(process);
        (sid.is_some() && sid == user_sid()).then_some(pid)
    }
}

fn proto_err(e: ProtoError) -> PlatformError {
    PlatformError::new(-1, format!("running Flit instance: {e}"))
}

fn connect(pipe: &[u16]) -> Result<File, PlatformError> {
    let deadline = Instant::now() + CONNECT_TIMEOUT;
    let flags = SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION;
    loop {
        let h = unsafe { CreateFileW(PCWSTR(pipe.as_ptr()), (GENERIC_READ | GENERIC_WRITE).0, FILE_SHARE_NONE, None, OPEN_EXISTING, flags, None) };
        match h {
            Ok(h) => return Ok(unsafe { File::from_raw_handle(h.0) }),
            Err(e) if Instant::now() < deadline => {
                if e.code() == ERROR_PIPE_BUSY.to_hresult() {
                    unsafe {
                        let _ = WaitNamedPipeW(PCWSTR(pipe.as_ptr()), 200);
                    }
                } else {
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
            Err(e) => return Err(err(e)),
        }
    }
}

struct Deadline<'a> {
    pipe: &'a File,
    until: Instant,
}

impl<'a> Deadline<'a> {
    fn new(pipe: &'a File) -> Self {
        Self { pipe, until: Instant::now() + READ_TIMEOUT }
    }
}

impl Read for Deadline<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            let mut avail = 0u32;
            unsafe { PeekNamedPipe(HANDLE(self.pipe.as_raw_handle()), None, 0, None, Some(&mut avail), None) }?;
            if avail > 0 || buf.is_empty() {
                let (mut pipe, n) = (self.pipe, buf.len().min(avail as usize));
                return pipe.read(&mut buf[..n]);
            }
            if Instant::now() >= self.until {
                return Err(io::ErrorKind::TimedOut.into());
            }
            std::thread::sleep(READ_POLL);
        }
    }
}

fn forward(pipe: &[u16], open: &Open) -> Result<(), PlatformError> {
    let mut f = connect(pipe)?;
    let server = same_user_server(&f).ok_or_else(|| PlatformError::new(-1, "the single instance pipe belongs to another user"))?;
    unsafe {
        let _ = AllowSetForegroundWindow(server);
    }
    let hello = Hello::new(Protocol::Instance, VERSION);
    let mut out = Vec::new();
    flit_proto::transport::encode(&hello, &mut out).map_err(proto_err)?;
    flit_proto::transport::encode(&Request::Open(open.clone()), &mut out).map_err(proto_err)?;
    f.write_all(&out).map_err(|e| proto_err(e.into()))?;
    let mut input = Deadline::new(&f);
    read_hello(&mut input, &hello).map_err(proto_err)?;
    match read_frame_limited::<_, Reply>(&mut input, &mut Vec::new(), MAX_FRAME).map_err(proto_err)? {
        Reply::Accepted => Ok(()),
    }
}

fn serve(f: File, opens: &OpenSink, wake: &Wake) -> Result<(), ProtoError> {
    let mut input = Deadline::new(&f);
    let ours = Hello::new(Protocol::Instance, VERSION);
    let theirs = read_hello(&mut input, &ours);
    write_frame(&mut &f, &ours)?;
    theirs?;
    let Request::Open(open) = read_frame_limited::<_, Request>(&mut input, &mut Vec::new(), MAX_FRAME)?;
    let sink = opens.lock().unwrap_or_else(|e| e.into_inner());
    if sink.as_ref().is_some_and(|tx| tx.send(open).is_ok()) {
        wake();
        write_frame(&mut &f, &Reply::Accepted)?;
    }
    Ok(())
}

struct OwnerOnly(PSECURITY_DESCRIPTOR);

impl Drop for OwnerOnly {
    fn drop(&mut self) {
        unsafe {
            LocalFree(Some(HLOCAL(self.0 .0)));
        }
    }
}

fn owner_only() -> Result<OwnerOnly, String> {
    let sid = user_sid().ok_or("the current user SID is unavailable")?;
    let sddl: Vec<u16> = format!("D:P(A;;GA;;;{sid})").encode_utf16().chain(Some(0)).collect();
    let mut sd = PSECURITY_DESCRIPTOR::default();
    unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(PCWSTR(sddl.as_ptr()), SDDL_REVISION_1, &mut sd, None) }.map_err(|e| e.to_string())?;
    Ok(OwnerOnly(sd))
}

struct ClientSlot(Receiver<()>);

impl Drop for ClientSlot {
    fn drop(&mut self) {
        let _ = self.0.try_recv();
    }
}

fn listen_loop(pipe: Vec<u16>, opens: OpenSink, wake: Wake) {
    let security = match owner_only() {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("single instance disabled: cannot build the pipe security descriptor: {e}");
            return;
        }
    };
    let attrs = SECURITY_ATTRIBUTES { nLength: size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: security.0 .0, bInheritHandle: false.into() };
    let (taken, freed) = bounded::<()>(MAX_CLIENTS);
    let mut first = true;
    loop {
        if taken.send(()).is_err() {
            return;
        }
        let slot = ClientSlot(freed.clone());
        let mode = if first { PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE } else { PIPE_ACCESS_DUPLEX };
        let h = unsafe { CreateNamedPipeW(PCWSTR(pipe.as_ptr()), mode, PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS, PIPE_UNLIMITED_INSTANCES, 64 * 1024, 64 * 1024, 0, Some(&attrs)) };
        if h.is_invalid() {
            let e = windows::core::Error::from_thread();
            if first {
                tracing::error!("single instance disabled: cannot create the pipe, another process may own its name: {e}");
            } else {
                tracing::error!("single instance pipe unavailable: {e}");
            }
            return;
        }
        first = false;
        let connected = unsafe { ConnectNamedPipe(h, None) };
        if connected.is_err() && unsafe { GetLastError() } != ERROR_PIPE_CONNECTED {
            unsafe {
                let _ = CloseHandle(h);
            }
            continue;
        }
        let f = unsafe { File::from_raw_handle(h.0) };
        let (opens, wake) = (opens.clone(), wake.clone());
        let _ = std::thread::Builder::new().name("flit-instance-client".into()).spawn(move || {
            let _slot = slot;
            if let Err(e) = serve(f, &opens, &wake) {
                tracing::warn!("rejected open request: {e}");
            }
        });
    }
}

impl SingleInstance for PipeInstance {
    fn acquire(open: &Open) -> Instance<Self> {
        let (mutex_name, pipe) = names();
        let mutex = unsafe { CreateMutexW(None, false, PCWSTR(mutex_name.as_ptr())) };
        let existed = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        let mutex = match mutex {
            Ok(m) => m,
            Err(e) => return Instance::Standalone(err(e)),
        };
        if !existed {
            return Instance::Primary(PipeInstance { pipe });
        }
        unsafe {
            let _ = CloseHandle(mutex);
        }
        match forward(&pipe, open) {
            Ok(()) => Instance::Forwarded,
            Err(e) => Instance::Standalone(e),
        }
    }

    fn listen(self, opens: OpenSink, wake: Wake) {
        let _ = std::thread::Builder::new().name("flit-instance".into()).spawn(move || listen_loop(self.pipe, opens, wake));
    }
}

#[cfg(test)]
#[path = "../test/instance.rs"]
mod tests;
