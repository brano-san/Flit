use crate::err;
use crossbeam_channel::Sender;
use flit_core::Open;
use flit_platform::{Instance, PlatformError, SingleInstance, Wake};
use flit_proto::instance::{Reply, Request, VERSION};
use flit_proto::transport::{read_frame, read_hello, write_frame, Hello, Protocol};
use flit_proto::ProtoError;
use std::fs::File;
use std::io::Write;
use std::os::windows::io::FromRawHandle;
use std::time::{Duration, Instant};
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows::Win32::Security::{GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER};
use windows::Win32::Storage::FileSystem::*;
use windows::Win32::System::Pipes::*;
use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows::Win32::System::Threading::{CreateMutexW, GetCurrentProcess, GetCurrentProcessId, OpenProcessToken};
use windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

pub struct PipeInstance {
    pipe: Vec<u16>,
}

fn user_sid() -> Option<String> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).ok()?;
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
    (wide(format!(r"Local\flit-{sid}")), wide(format!(r"\\.\pipe\flit-{sid}-{session}")))
}

fn proto_err(e: ProtoError) -> PlatformError {
    PlatformError::new(-1, format!("running Flit instance: {e}"))
}

fn connect(pipe: &[u16]) -> Result<File, PlatformError> {
    let deadline = Instant::now() + CONNECT_TIMEOUT;
    loop {
        let h = unsafe { CreateFileW(PCWSTR(pipe.as_ptr()), (GENERIC_READ | GENERIC_WRITE).0, FILE_SHARE_NONE, None, OPEN_EXISTING, FILE_FLAGS_AND_ATTRIBUTES(0), None) };
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

fn forward(pipe: &[u16], open: &Open) -> Result<(), PlatformError> {
    let mut f = connect(pipe)?;
    let mut server = 0u32;
    if unsafe { GetNamedPipeServerProcessId(HANDLE(std::os::windows::io::AsRawHandle::as_raw_handle(&f)), &mut server) }.is_ok() {
        unsafe {
            let _ = AllowSetForegroundWindow(server);
        }
    }
    let hello = Hello::new(Protocol::Instance, VERSION);
    let mut out = Vec::new();
    flit_proto::transport::encode(&hello, &mut out).map_err(proto_err)?;
    flit_proto::transport::encode(&Request::Open(open.clone()), &mut out).map_err(proto_err)?;
    f.write_all(&out).map_err(|e| proto_err(e.into()))?;
    read_hello(&mut f, &hello).map_err(proto_err)?;
    match read_frame::<_, Reply>(&mut f, &mut Vec::new()).map_err(proto_err)? {
        Reply::Accepted => Ok(()),
    }
}

fn serve(mut f: File, opens: &Sender<Open>, wake: &Wake) -> Result<(), ProtoError> {
    let ours = Hello::new(Protocol::Instance, VERSION);
    let theirs = read_hello(&mut f, &ours);
    write_frame(&mut f, &ours)?;
    theirs?;
    let Request::Open(open) = read_frame::<_, Request>(&mut f, &mut Vec::new())?;
    if opens.send(open).is_ok() {
        wake();
        write_frame(&mut f, &Reply::Accepted)?;
    }
    Ok(())
}

fn listen_loop(pipe: Vec<u16>, opens: Sender<Open>, wake: Wake) {
    let mut first = true;
    loop {
        let mode = if first { PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE } else { PIPE_ACCESS_DUPLEX };
        let h = unsafe { CreateNamedPipeW(PCWSTR(pipe.as_ptr()), mode, PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS, PIPE_UNLIMITED_INSTANCES, 64 * 1024, 64 * 1024, 0, None) };
        if h.is_invalid() {
            tracing::error!("single instance pipe unavailable: {}", windows::core::Error::from_thread());
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

    fn listen(self, opens: Sender<Open>, wake: Wake) {
        let _ = std::thread::Builder::new().name("flit-instance".into()).spawn(move || listen_loop(self.pipe, opens, wake));
    }
}
