use crate::err;
use flit_platform::PlatformError;
use std::ffi::c_void;
use std::io::Write;
use std::time::Duration;
use windows::core::{HSTRING, PCWSTR};
use windows::core::w;
use windows::Win32::Foundation::{CloseHandle, ERROR_CANCELLED, WIN32_ERROR};
use windows::Win32::System::Threading::{GetExitCodeProcess, INFINITE};
use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;
use windows::Win32::Networking::WinHttp::*;
use windows::Win32::System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE};

const TIMEOUT_MS: i32 = 30_000;
const CHUNK: usize = 64 * 1024;

struct Handle(*mut c_void);

impl Handle {
    fn new(h: *mut c_void, what: &str) -> Result<Self, PlatformError> {
        if h.is_null() {
            return Err(last(what));
        }
        Ok(Self(h))
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        let _ = unsafe { WinHttpCloseHandle(self.0) };
    }
}

fn last(what: &str) -> PlatformError {
    let e = windows::core::Error::from_thread();
    PlatformError::new(e.code().0 as i64, format!("{what}: {}", e.message()))
}

fn context(what: &str) -> impl Fn(windows::core::Error) -> PlatformError + '_ {
    move |e| {
        let mut p = err(e);
        p.message = format!("{what}: {}", p.message);
        p
    }
}

fn query_number(request: &Handle, info: u32) -> Option<u64> {
    let mut text = [0u16; 32];
    let mut len = std::mem::size_of_val(&text) as u32;
    unsafe { WinHttpQueryHeaders(request.0, info, PCWSTR::null(), Some(text.as_mut_ptr() as *mut c_void), &mut len, std::ptr::null_mut()) }.ok()?;
    String::from_utf16_lossy(&text[..len as usize / 2]).trim().parse().ok()
}

pub(crate) fn download(url: &str, out: &mut dyn Write, progress: &mut dyn FnMut(u64, Option<u64>) -> bool) -> Result<(), PlatformError> {
    let rest = url.strip_prefix("https://").ok_or_else(|| PlatformError::new(-1, format!("{url}: only https addresses are downloaded")))?;
    let (host, path) = match rest.find('/') {
        Some(k) => (&rest[..k], &rest[k..]),
        None => (rest, "/"),
    };
    let agent = HSTRING::from(format!("Flit/{}", env!("CARGO_PKG_VERSION")));
    let session = Handle::new(unsafe { WinHttpOpen(&agent, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, PCWSTR::null(), PCWSTR::null(), 0) }, "WinHttpOpen")?;
    unsafe { WinHttpSetTimeouts(session.0, TIMEOUT_MS, TIMEOUT_MS, TIMEOUT_MS, TIMEOUT_MS) }.map_err(context("WinHttpSetTimeouts"))?;
    let connection = Handle::new(unsafe { WinHttpConnect(session.0, &HSTRING::from(host), INTERNET_DEFAULT_HTTPS_PORT, 0) }, host)?;
    let request = Handle::new(unsafe { WinHttpOpenRequest(connection.0, &HSTRING::from("GET"), &HSTRING::from(path), PCWSTR::null(), PCWSTR::null(), std::ptr::null(), WINHTTP_FLAG_SECURE) }, url)?;
    unsafe { WinHttpSendRequest(request.0, None, None, 0, 0, 0) }.map_err(context(host))?;
    unsafe { WinHttpReceiveResponse(request.0, std::ptr::null_mut()) }.map_err(context(host))?;
    let status = query_number(&request, WINHTTP_QUERY_STATUS_CODE).unwrap_or(0);
    if status != 200 {
        return Err(PlatformError::new(status as i64, format!("{url}: HTTP {status}")));
    }
    let total = query_number(&request, WINHTTP_QUERY_CONTENT_LENGTH);
    let mut buf = vec![0u8; CHUNK];
    let mut received = 0u64;
    if !progress(0, total) {
        return Err(PlatformError::new(-1, "cancelled"));
    }
    loop {
        let mut read = 0u32;
        unsafe { WinHttpReadData(request.0, buf.as_mut_ptr() as *mut c_void, buf.len() as u32, &mut read) }.map_err(context(host))?;
        if read == 0 {
            break;
        }
        out.write_all(&buf[..read as usize]).map_err(|e| PlatformError::new(-1, e.to_string()))?;
        received += u64::from(read);
        if !progress(received, total) {
            return Err(PlatformError::new(-1, "cancelled"));
        }
    }
    if total.is_some_and(|t| t != received) {
        return Err(PlatformError::new(-1, format!("{url}: the connection closed after {received} bytes")));
    }
    Ok(())
}

pub(crate) fn wait_exit(pid: u32, limit: Duration) {
    let Ok(process) = (unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, pid) }) else { return };
    unsafe {
        WaitForSingleObject(process, limit.as_millis().min(u32::MAX as u128) as u32);
        let _ = CloseHandle(process);
    }
}

pub(crate) fn command_line(args: &[String]) -> String {
    let mut out = String::new();
    for a in args {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push('"');
        let mut slashes = 0;
        for c in a.chars() {
            match c {
                '\\' => slashes += 1,
                '"' => {
                    out.extend(std::iter::repeat_n('\\', slashes * 2 + 1));
                    slashes = 0;
                }
                _ => {
                    out.extend(std::iter::repeat_n('\\', slashes));
                    slashes = 0;
                }
            }
            if c != '\\' {
                out.push(c);
            }
        }
        out.extend(std::iter::repeat_n('\\', slashes * 2));
        out.push('"');
    }
    out
}

pub fn run_elevated(args: &[String]) -> Result<u32, PlatformError> {
    let exe = std::env::current_exe().map_err(|e| PlatformError::new(-1, e.to_string()))?;
    let file: Vec<u16> = exe.to_string_lossy().encode_utf16().chain([0]).collect();
    let params: Vec<u16> = command_line(args).encode_utf16().chain([0]).collect();
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
        lpVerb: w!("runas"),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    if let Err(e) = unsafe { ShellExecuteExW(&mut info) } {
        return Err(match WIN32_ERROR::from_error(&e) {
            Some(ERROR_CANCELLED) => PlatformError::new(i64::from(ERROR_CANCELLED.0), "cancelled"),
            _ => err(e),
        });
    }
    let mut code = 1u32;
    unsafe {
        WaitForSingleObject(info.hProcess, INFINITE);
        let _ = GetExitCodeProcess(info.hProcess, &mut code);
        let _ = CloseHandle(info.hProcess);
    }
    Ok(code)
}

#[cfg(test)]
#[path = "../test/update.rs"]
mod tests;
