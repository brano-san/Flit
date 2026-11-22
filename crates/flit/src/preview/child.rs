use std::ffi::OsStr;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::Duration;

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
    cmd.arg(mode).args(args).stdin(Stdio::null());
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
    #[cfg(windows)]
    let started = |child: &mut Child| job.as_ref().is_none_or(|job| start_limited(child, job));
    #[cfg(not(windows))]
    let started = |_: &mut Child| true;
    #[cfg(not(any(windows, target_os = "linux")))]
    let _ = max_memory;
    flit_platform::parse_process::bounded(&mut cmd, usize::try_from(max_output).ok()?, timeout, &current, started).ok()
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
