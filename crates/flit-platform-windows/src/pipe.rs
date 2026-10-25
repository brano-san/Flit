use crate::err;
use crate::instance::{process_user_sid, user_sid};
use crossbeam_channel::{bounded, Receiver};
use flit_platform::{Duplex, OnClient, PlatformError};
use std::io::{self, Read, Write};
use std::sync::Arc;
use std::time::{Duration, Instant};
use windows::core::PCWSTR;
use windows::Win32::Foundation::*;
use windows::Win32::Security::Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1};
use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows::Win32::Storage::FileSystem::*;
use windows::Win32::System::Pipes::*;
use windows::Win32::System::Threading::{CreateEventW, OpenProcess, ResetEvent, WaitForSingleObject, INFINITE, PROCESS_QUERY_LIMITED_INFORMATION};
use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};

pub const SYSTEM_SID: &str = "S-1-5-18";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);
const PIPE_BUFFER: u32 = 64 * 1024;
const CLIENT_RIGHTS: &str = "0x12008b";

struct Owned(HANDLE);
unsafe impl Send for Owned {}
unsafe impl Sync for Owned {}

impl Drop for Owned {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

pub struct PipeConn {
    pipe: Arc<Owned>,
    event: Owned,
    timeout: Option<Duration>,
    peer_sid: Option<String>,
}

fn event() -> io::Result<Owned> {
    Ok(Owned(unsafe { CreateEventW(None, true, false, PCWSTR::null()) }.map_err(io::Error::other)?))
}

fn wait_ms(t: Option<Duration>) -> u32 {
    t.map_or(INFINITE, |d| d.as_millis().min(INFINITE as u128 - 1) as u32)
}

impl PipeConn {
    fn new(h: HANDLE, peer_sid: Option<String>) -> io::Result<Self> {
        let pipe = Arc::new(Owned(h));
        Ok(Self { pipe, event: event()?, timeout: None, peer_sid })
    }

    pub fn peer_sid(&self) -> Option<&str> {
        self.peer_sid.as_deref()
    }

    fn io(&mut self, timeout: Option<Duration>, start: impl FnOnce(HANDLE, &mut OVERLAPPED) -> windows::core::Result<()>) -> io::Result<usize> {
        let h = self.pipe.0;
        let mut ov = OVERLAPPED { hEvent: self.event.0, ..Default::default() };
        unsafe {
            let _ = ResetEvent(self.event.0);
        }
        if let Err(e) = start(h, &mut ov) {
            if e.code() != ERROR_IO_PENDING.to_hresult() {
                return Err(io_err(e));
            }
        }
        let mut n = 0u32;
        if unsafe { WaitForSingleObject(self.event.0, wait_ms(timeout)) } != WAIT_OBJECT_0 {
            unsafe {
                let _ = CancelIoEx(h, Some(&ov));
                let _ = GetOverlappedResult(h, &ov, &mut n, true);
            }
            return Err(io::ErrorKind::TimedOut.into());
        }
        unsafe { GetOverlappedResult(h, &ov, &mut n, false) }.map_err(io_err)?;
        Ok(n as usize)
    }
}

fn io_err(e: windows::core::Error) -> io::Error {
    if e.code() == ERROR_BROKEN_PIPE.to_hresult() || e.code() == ERROR_PIPE_NOT_CONNECTED.to_hresult() {
        return io::ErrorKind::BrokenPipe.into();
    }
    io::Error::other(e)
}

impl Read for PipeConn {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let timeout = self.timeout;
        match self.io(timeout, |h, ov| unsafe { ReadFile(h, Some(buf), None, Some(ov)) }) {
            Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(0),
            r => r,
        }
    }
}

impl Write for PipeConn {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.io(Some(WRITE_TIMEOUT), |h, ov| unsafe { WriteFile(h, Some(buf), None, Some(ov)) })
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Duplex for PipeConn {
    fn try_clone(&self) -> io::Result<Self> {
        Ok(Self { pipe: self.pipe.clone(), event: event()?, timeout: self.timeout, peer_sid: self.peer_sid.clone() })
    }

    fn set_read_timeout(&mut self, timeout: Option<Duration>) {
        self.timeout = timeout;
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn peer_sid(pid: windows::core::Result<u32>) -> Option<String> {
    let pid = pid.ok()?;
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let sid = process_user_sid(process);
        let _ = CloseHandle(process);
        sid
    }
}

pub fn server_sid(conn: &PipeConn) -> Option<String> {
    let mut pid = 0u32;
    peer_sid(unsafe { GetNamedPipeServerProcessId(conn.pipe.0, &mut pid) }.map(|()| pid))
}

pub fn connect_to(name: &str) -> Result<PipeConn, PlatformError> {
    let name = wide(name);
    let flags = FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION;
    let deadline = Instant::now() + CONNECT_TIMEOUT;
    let h = loop {
        match unsafe { CreateFileW(PCWSTR(name.as_ptr()), (GENERIC_READ | GENERIC_WRITE).0, FILE_SHARE_NONE, None, OPEN_EXISTING, flags, None) } {
            Ok(h) => break h,
            Err(e) if e.code() == ERROR_PIPE_BUSY.to_hresult() && Instant::now() < deadline => unsafe {
                let _ = WaitNamedPipeW(PCWSTR(name.as_ptr()), 250);
            },
            Err(e) => return Err(err(e)),
        }
    };
    let mut conn = PipeConn::new(h, None).map_err(|e| PlatformError::new(-1, e.to_string()))?;
    let server = server_sid(&conn);
    let trusted = server.as_deref() == Some(SYSTEM_SID) || (server.is_some() && server == user_sid());
    if !trusted {
        return Err(PlatformError::new(-1, "the index service pipe is owned by an untrusted process"));
    }
    conn.peer_sid = server;
    Ok(conn)
}

struct Descriptor(PSECURITY_DESCRIPTOR);

impl Drop for Descriptor {
    fn drop(&mut self) {
        unsafe {
            LocalFree(Some(HLOCAL(self.0 .0)));
        }
    }
}

pub fn dacl_for(me: &str) -> String {
    if me == SYSTEM_SID {
        format!("D:P(A;;GA;;;SY)(A;;{CLIENT_RIGHTS};;;IU)")
    } else {
        format!("D:P(A;;GA;;;{me})")
    }
}

fn descriptor(sddl: &str) -> Result<Descriptor, PlatformError> {
    let sddl = wide(sddl);
    let mut sd = PSECURITY_DESCRIPTOR::default();
    unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(PCWSTR(sddl.as_ptr()), SDDL_REVISION_1, &mut sd, None) }.map_err(err)?;
    Ok(Descriptor(sd))
}

struct Slot(Receiver<()>);

impl Drop for Slot {
    fn drop(&mut self) {
        let _ = self.0.try_recv();
    }
}

fn accept(h: HANDLE) -> bool {
    let Ok(ev) = event() else { return false };
    let mut ov = OVERLAPPED { hEvent: ev.0, ..Default::default() };
    match unsafe { ConnectNamedPipe(h, Some(&mut ov)) } {
        Ok(()) => true,
        Err(e) if e.code() == ERROR_PIPE_CONNECTED.to_hresult() => true,
        Err(e) if e.code() == ERROR_IO_PENDING.to_hresult() => {
            let mut n = 0u32;
            unsafe { GetOverlappedResult(h, &ov, &mut n, true) }.is_ok()
        }
        Err(_) => false,
    }
}

pub fn listen_at(name: &str, max_clients: usize, on_client: OnClient<PipeConn>) -> Result<(), PlatformError> {
    let me = user_sid().ok_or_else(|| PlatformError::new(-1, "the current user SID is unavailable"))?;
    let sd = descriptor(&dacl_for(&me))?;
    let attrs = SECURITY_ATTRIBUTES { nLength: size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: sd.0 .0, bInheritHandle: false.into() };
    let name = wide(name);
    let (taken, freed) = bounded::<()>(max_clients.max(1));
    let mut first = true;
    loop {
        if taken.send(()).is_err() {
            return Ok(());
        }
        let slot = Slot(freed.clone());
        let mut mode = PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED;
        if first {
            mode |= FILE_FLAG_FIRST_PIPE_INSTANCE;
        }
        let h = unsafe { CreateNamedPipeW(PCWSTR(name.as_ptr()), mode, PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS, PIPE_UNLIMITED_INSTANCES, PIPE_BUFFER, PIPE_BUFFER, 0, Some(&attrs)) };
        if h.is_invalid() {
            let e = windows::core::Error::from_thread();
            return Err(PlatformError::new(e.code().0 as i64, format!("cannot create the index pipe (another process may own its name): {}", e.message())));
        }
        first = false;
        let owned = Owned(h);
        if !accept(h) {
            continue;
        }
        let mut pid = 0u32;
        let client = peer_sid(unsafe { GetNamedPipeClientProcessId(h, &mut pid) }.map(|()| pid));
        let allowed = match client.as_deref() {
            Some(sid) => me == SYSTEM_SID || sid == me,
            None => false,
        };
        if !allowed {
            tracing::warn!("rejected index client {pid} ({client:?})");
            continue;
        }
        std::mem::forget(owned);
        let Ok(conn) = PipeConn::new(h, client) else { continue };
        let on_client = on_client.clone();
        let _ = std::thread::Builder::new().name("flit-indexd-client".into()).spawn(move || {
            let _slot = slot;
            on_client(conn);
        });
    }
}

#[cfg(test)]
#[path = "../test/pipe.rs"]
mod tests;
