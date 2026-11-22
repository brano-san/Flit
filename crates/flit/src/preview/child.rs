use std::ffi::OsStr;
use std::io::Read;
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

pub(super) fn run(
    mode: &str,
    args: &[&OsStr],
    max_output: u64,
    timeout: Duration,
    max_memory: Option<usize>,
    current: impl Fn() -> bool,
) -> Option<(ExitStatus, Vec<u8>)> {
    if !current() {
        return None;
    }
    let mut cmd = Command::new(std::env::current_exe().ok()?);
    cmd.arg(mode).args(args).stdin(Stdio::null()).stderr(Stdio::null()).stdout(Stdio::piped());
    #[cfg(target_os = "linux")]
    if let Some(limit) = max_memory {
        limit_address_space(&mut cmd, limit);
    }
    #[cfg(windows)]
    let job = match max_memory {
        Some(limit) => {
            cmd.stdin(Stdio::piped());
            Some(memory_job(limit)?)
        }
        None => None,
    };
    #[cfg(not(any(windows, target_os = "linux")))]
    let _ = max_memory;
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        use windows::Win32::System::Threading::CREATE_NO_WINDOW;
        cmd.creation_flags(CREATE_NO_WINDOW.0);
    }
    let mut child = cmd.spawn().ok()?;
    #[cfg(windows)]
    if let Some(job) = &job {
        if !start_limited(&mut child, job) {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
    }
    let reader = child.stdout.take().and_then(|out| {
        std::thread::Builder::new().name("flit-preview-output".into()).spawn(move || {
            let mut bytes = Vec::new();
            out.take(max_output + 1).read_to_end(&mut bytes).ok()?;
            (bytes.len() as u64 <= max_output).then_some(bytes)
        }).ok()
    });
    let end = Instant::now() + timeout;
    if let Some(reader) = reader {
        while current() && Instant::now() < end {
            match child.try_wait() {
                Ok(Some(status)) => return Some((status, reader.join().ok()??)),
                Ok(None) => std::thread::sleep(Duration::from_millis(4)),
                Err(_) => break,
            }
        }
        let _ = child.kill();
        let _ = child.wait();
        let _ = reader.join();
    } else {
        let _ = child.kill();
        let _ = child.wait();
    }
    None
}

#[cfg(target_os = "linux")]
pub(super) fn limit_address_space(cmd: &mut Command, bytes: usize) {
    use std::os::unix::process::CommandExt;
    unsafe {
        cmd.pre_exec(move || {
            let mut limit = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
            if libc::getrlimit(libc::RLIMIT_AS, &mut limit) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            limit.rlim_cur = limit.rlim_cur.min(bytes as libc::rlim_t);
            limit.rlim_max = limit.rlim_cur;
            if libc::setrlimit(libc::RLIMIT_AS, &limit) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

#[cfg(windows)]
pub(super) fn memory_job(bytes: usize) -> Option<std::os::windows::io::OwnedHandle> {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::JobObjects::*;

    let job = unsafe { OwnedHandle::from_raw_handle(CreateJobObjectW(None, PCWSTR::null()).ok()?.0) };
    let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION { ProcessMemoryLimit: bytes, ..Default::default() };
    info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_PROCESS_MEMORY | JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    unsafe {
        SetInformationJobObject(HANDLE(job.as_raw_handle()), JobObjectExtendedLimitInformation, (&raw const info).cast(), size_of_val(&info) as u32).ok()?;
    }
    Some(job)
}

#[cfg(windows)]
fn start_limited(child: &mut std::process::Child, job: &std::os::windows::io::OwnedHandle) -> bool {
    use std::io::Write;
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::JobObjects::AssignProcessToJobObject;

    unsafe { AssignProcessToJobObject(HANDLE(job.as_raw_handle()), HANDLE(child.as_raw_handle())) }.is_ok()
        && child.stdin.take().is_some_and(|mut stdin| stdin.write_all(&[1]).is_ok())
}
