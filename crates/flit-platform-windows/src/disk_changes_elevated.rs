use super::read;
use flit_core::FsPath;
use flit_core::cmdline::quote_windows;
use flit_core::disk_changes::{Error, Filter, Report};
use flit_core::i18n::S;
use flit_proto::transport::{read_frame_limited, write_frame};
use std::ffi::OsString;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::windows::io::AsRawHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use windows::Win32::Foundation::*;
use windows::Win32::System::Pipes::*;
use windows::Win32::System::Threading::{GetCurrentProcessId, GetProcessId, WaitForSingleObject};
use windows::Win32::UI::Shell::{SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW};
use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;
use windows::core::{PCWSTR, w};

use flit_core::disk_changes::ELEVATED_FLAG;
const MAX_REPLY: usize = 16 * 1024 * 1024;
const IDLE_TIMEOUT: Duration = Duration::from_secs(180);
const HEARTBEAT: Duration = Duration::from_secs(1);
const PROGRESS: u8 = 0;
const RESULT: u8 = 1;

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

fn ipc_error() -> Error {
    Error::Failed(S::DiskChangesElevatedFailed.en().into())
}

pub fn read_elevated(root: &FsPath, filter: &Filter, cancelled: &dyn Fn() -> bool) -> Result<Report, Error> {
    if cancelled() {
        return Err(Error::Cancelled);
    }
    let pid = unsafe { GetCurrentProcessId() };
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map_err(|_| ipc_error())?.as_nanos();
    let pipe = format!(r"\\.\pipe\flit-usn-{pid}-{stamp}");
    let pipe = wide(&pipe);
    let server = crate::instance::private_polling_pipe(&pipe).map_err(|_| ipc_error())?;
    let mask = u8::from(filter.created) | (u8::from(filter.deleted) << 1) | (u8::from(filter.renamed) << 2);
    let args = [ELEVATED_FLAG.to_owned(), pid.to_string(), stamp.to_string(), root.display().into_owned(), filter.since.to_string(), filter.until.to_string(), mask.to_string(), filter.name.clone()].iter().map(|s| quote_windows(s)).collect::<Vec<_>>().join(" ");
    let exe = std::env::current_exe().map_err(|_| ipc_error())?;
    let file = wide(&exe.to_string_lossy());
    let args = wide(&args);
    let mut request =
        SHELLEXECUTEINFOW { cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32, fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC, lpVerb: w!("runas"), lpFile: PCWSTR(file.as_ptr()), lpParameters: PCWSTR(args.as_ptr()), nShow: SW_HIDE.0, ..Default::default() };
    if let Err(e) = unsafe { ShellExecuteExW(&mut request) } {
        return Err(if WIN32_ERROR::from_error(&e) == Some(ERROR_CANCELLED) { Error::Cancelled } else { ipc_error() });
    }
    let process = crate::walk_index::Handle(request.hProcess);
    let expected = unsafe { GetProcessId(process.0) };
    let pipe = HANDLE(server.as_raw_handle());
    let mut input = PipeInput { server, process: process.0, cancelled, idle: Idle::default() };
    loop {
        match unsafe { ConnectNamedPipe(pipe, None) } {
            Ok(()) => break,
            Err(e) if WIN32_ERROR::from_error(&e) == Some(ERROR_PIPE_CONNECTED) => break,
            Err(e) if WIN32_ERROR::from_error(&e) == Some(ERROR_PIPE_LISTENING) => {
                input.active()?;
                std::thread::sleep(Duration::from_millis(15));
            }
            Err(_) => return Err(ipc_error()),
        }
    }
    let mut client = 0;
    unsafe { GetNamedPipeClientProcessId(pipe, &mut client) }.map_err(|_| ipc_error())?;
    if expected == 0 || client != expected {
        return Err(ipc_error());
    }
    input.idle.progress(Instant::now());
    read_reply(&mut input).map_err(|_| if cancelled() { Error::Cancelled } else { ipc_error() })?
}

#[derive(Default)]
struct Idle(Option<Instant>);

impl Idle {
    fn progress(&mut self, now: Instant) {
        self.0 = Some(now + IDLE_TIMEOUT);
    }

    fn expired(&self, now: Instant) -> bool {
        self.0.is_some_and(|deadline| now >= deadline)
    }
}

fn read_reply(input: &mut impl Read) -> io::Result<Result<Report, Error>> {
    let mut tag = [0];
    loop {
        input.read_exact(&mut tag)?;
        match tag[0] {
            PROGRESS => {}
            RESULT => return read_frame_limited(input, &mut Vec::new(), MAX_REPLY).map_err(io::Error::other),
            _ => return Err(io::ErrorKind::InvalidData.into()),
        }
    }
}

struct PipeInput<'a> {
    server: File,
    process: HANDLE,
    cancelled: &'a dyn Fn() -> bool,
    idle: Idle,
}

impl PipeInput<'_> {
    fn active(&self) -> Result<(), Error> {
        if (self.cancelled)() {
            return Err(Error::Cancelled);
        }
        if self.idle.expired(Instant::now()) || unsafe { WaitForSingleObject(self.process, 0) } == WAIT_OBJECT_0 {
            return Err(ipc_error());
        }
        Ok(())
    }
}

impl Read for PipeInput<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        loop {
            if (self.cancelled)() {
                return Err(io::ErrorKind::TimedOut.into());
            }
            let mut available = 0u32;
            let h = HANDLE(self.server.as_raw_handle());
            unsafe { PeekNamedPipe(h, None, 0, None, Some(&mut available), None) }?;
            if available > 0 {
                let count = buf.len().min(available as usize);
                let read = self.server.read(&mut buf[..count])?;
                if read != 0 {
                    self.idle.progress(Instant::now());
                }
                return Ok(read);
            }
            self.active().map_err(|_| io::Error::from(io::ErrorKind::TimedOut))?;
            std::thread::sleep(Duration::from_millis(15));
        }
    }
}

pub fn from_command_line(args: impl Iterator<Item = OsString>) -> i32 {
    if !crate::instance::elevated() {
        return 1;
    }
    let Some(args) = args.map(|s| s.into_string().ok()).collect::<Option<Vec<_>>>() else {
        return 1;
    };
    let [pid, stamp, volume, since, until, mask, name] = args.as_slice() else {
        return 1;
    };
    let Ok(parent) = pid.parse::<u32>() else {
        return 1;
    };
    let Ok(stamp) = stamp.parse::<u128>() else {
        return 1;
    };
    let Some(root) = FsPath::parse(volume) else {
        return 1;
    };
    let (Ok(since), Ok(until), Ok(mask)) = (since.parse::<i64>(), until.parse::<i64>(), mask.parse::<u8>()) else {
        return 1;
    };
    if name.len() > 512 || mask == 0 || mask > 7 || since < 0 || since >= until {
        return 1;
    }
    let name = name.clone();
    let filter = Filter { since, until, name, created: mask & 1 != 0, deleted: mask & 2 != 0, renamed: mask & 4 != 0 };
    let pipe_name = wide(&format!(r"\\.\pipe\flit-usn-{parent}-{stamp}"));
    let Ok(mut peer) = crate::instance::connect(&pipe_name) else {
        return 1;
    };
    if crate::instance::server_pid(&peer) != Some(parent) {
        return 1;
    }
    let beat = std::cell::Cell::new(Instant::now());
    let reply = read(&root, &filter, &|| {
        let now = Instant::now();
        if now.duration_since(beat.get()) >= HEARTBEAT {
            beat.set(now);
            if (&peer).write_all(&[PROGRESS]).is_err() {
                return true;
            }
        }
        unsafe { PeekNamedPipe(HANDLE(peer.as_raw_handle()), None, 0, None, None, None) }.is_err()
    });
    if peer.write_all(&[RESULT]).is_err() || write_frame(&mut peer, &reply).is_err() {
        return 1;
    }
    let mut end = [0u8; 1];
    let _ = peer.read(&mut end);
    0
}

#[cfg(test)]
#[path = "../test/disk_changes_elevated.rs"]
mod tests;
