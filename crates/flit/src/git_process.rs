use flit_core::git::{self, GitProgress};
use flit_core::i18n::S;
use flit_core::provider::OpCtl;
use std::io::{self, Read};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const POLL: Duration = Duration::from_millis(30);
const STOP_GRACE: Duration = Duration::from_secs(2);
const MAX_LINE: usize = 16 * 1024;

#[derive(Debug, PartialEq, Eq)]
pub(super) enum RunError {
    Cancelled { forced: bool },
    Failed(String),
}

#[derive(Default)]
struct Stderr {
    line: Vec<u8>,
    auth: Option<String>,
    first_error: Option<String>,
    last_line: Option<String>,
    last_progress: Option<GitProgress>,
}

impl Stderr {
    fn feed(&mut self, bytes: &[u8], progress: &mut impl FnMut(GitProgress)) {
        for bytes in bytes.split_inclusive(|&b| b == b'\r' || b == b'\n') {
            let delimited = bytes.last().is_some_and(|b| matches!(*b, b'\r' | b'\n'));
            let content = if delimited { &bytes[..bytes.len() - 1] } else { bytes };
            let count = content.len().min(MAX_LINE - self.line.len());
            self.line.extend_from_slice(&content[..count]);
            if delimited {
                self.flush(progress);
            }
        }
    }

    fn flush(&mut self, progress: &mut impl FnMut(GitProgress)) {
        let line = String::from_utf8_lossy(&self.line);
        let line = line.trim();
        if let Some(update) = git::progress_line(line) {
            if self.last_progress != Some(update) {
                self.last_progress = Some(update);
                progress(update);
            }
        } else if !line.is_empty() {
            if git::failure(line) == git::Failure::Auth && self.auth.as_ref().is_none_or(|auth| super::auth_error_host(auth).is_none() && super::auth_error_host(line).is_some()) {
                self.auth = Some(line.to_owned());
            }
            if self.first_error.is_none() && (line.starts_with("error:") || line.starts_with("fatal:")) {
                self.first_error = Some(line.to_owned());
            }
            self.last_line = Some(line.to_owned());
        }
        self.line.clear();
    }

    fn error(self) -> String {
        self.auth.or(self.first_error).or(self.last_line).unwrap_or_else(|| S::GitErrorUnknown.t().into())
    }
}

pub(super) fn run(mut cmd: Command, ctl: &OpCtl, mut progress: impl FnMut(GitProgress)) -> Result<(), RunError> {
    if ctl.is_cancelled() {
        return Err(RunError::Cancelled { forced: false });
    }
    cmd.stdout(Stdio::null()).stderr(Stdio::piped());
    let mut task = Tree::spawn(cmd).map_err(|e| RunError::Failed(super::spawn_error(&e)))?;
    let Some(mut stderr) = task.child.stderr.take() else {
        task.stop();
        return Err(RunError::Failed(S::GitErrorUnknown.t().into()));
    };
    let (send, receive) = crossbeam_channel::bounded(8);
    if let Err(e) = std::thread::Builder::new().name("flit-git-stderr".into()).spawn(move || {
        let mut buffer = [0; 8192];
        loop {
            let chunk = match stderr.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => Ok(buffer[..n].to_vec()),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => {
                    let _ = send.send(Err(e));
                    break;
                }
            };
            if send.send(chunk).is_err() { break; }
        }
    }) {
        task.stop();
        return Err(RunError::Failed(e.to_string()));
    }
    use crossbeam_channel::RecvTimeoutError;
    let mut text = Stderr::default();
    let mut open = true;
    let mut exit = None;
    loop {
        if ctl.is_cancelled() {
            if task.child.try_wait().ok().flatten().is_some_and(|status| status.success()) {
                return Ok(());
            }
            let forced = task.stop();
            return if task.child.try_wait().ok().flatten().is_some_and(|status| status.success()) {
                Ok(())
            } else {
                Err(RunError::Cancelled { forced })
            };
        }
        if open {
            match receive.recv_timeout(POLL) {
                Ok(Ok(chunk)) => text.feed(&chunk, &mut progress),
                Ok(Err(e)) => {
                    task.stop();
                    return Err(RunError::Failed(e.to_string()));
                }
                Err(RecvTimeoutError::Disconnected) => open = false,
                Err(RecvTimeoutError::Timeout) => {}
            }
        } else if exit.is_none() {
            std::thread::sleep(POLL);
        }
        if exit.is_none() {
            exit = match task.child.try_wait() {
                Ok(exit) => exit,
                Err(e) => {
                    task.stop();
                    return Err(RunError::Failed(e.to_string()));
                }
            };
        }
        if let Some(exit) = exit.filter(|_| !open) {
            text.flush(&mut progress);
            return if exit.success() { Ok(()) } else { Err(RunError::Failed(text.error())) };
        }
    }
}

struct Tree {
    child: Child,
    #[cfg(windows)]
    job: std::os::windows::io::OwnedHandle,
}

#[cfg(unix)]
impl Tree {
    fn spawn(mut cmd: Command) -> io::Result<Self> {
        use std::os::unix::process::CommandExt;
        let child = cmd.process_group(0).spawn()?;
        Ok(Self { child })
    }

    fn stop(&mut self) -> bool {
        let group = -(self.child.id() as libc::pid_t);
        let _ = unsafe { libc::kill(group, libc::SIGTERM) };
        let deadline = Instant::now() + STOP_GRACE;
        loop {
            let _ = self.child.try_wait();
            if unsafe { libc::kill(group, 0) } == -1 && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
                let _ = self.child.wait();
                return false;
            }
            if Instant::now() >= deadline { break; }
            std::thread::sleep(POLL);
        }
        if unsafe { libc::kill(group, libc::SIGKILL) } != 0 {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
        true
    }
}

#[cfg(windows)]
mod windows_tree {
    use super::*;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::os::windows::process::CommandExt;
    use std::sync::Mutex;
    use windows::core::{BOOL, PCWSTR};
    use windows::Win32::Foundation::{HANDLE, STATUS_CONTROL_C_EXIT};
    use windows::Win32::System::Console::{AttachConsole, FreeConsole, GenerateConsoleCtrlEvent, GetConsoleProcessList, GetStdHandle, SetConsoleCtrlHandler, SetStdHandle, CTRL_C_EVENT, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE};
    use windows::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Thread32First, Thread32Next, THREADENTRY32, TH32CS_SNAPTHREAD};
    use windows::Win32::System::JobObjects::{AssignProcessToJobObject, CreateJobObjectW, IsProcessInJob, JobObjectBasicAccountingInformation, JobObjectExtendedLimitInformation, QueryInformationJobObject, SetInformationJobObject, TerminateJobObject, JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE};
    use windows::Win32::System::Threading::{OpenProcess, OpenThread, ResumeThread, CREATE_NO_WINDOW, CREATE_SUSPENDED, PROCESS_QUERY_LIMITED_INFORMATION, THREAD_SUSPEND_RESUME};

    static CONSOLE: Mutex<()> = Mutex::new(());

    impl Tree {
        pub(super) fn spawn(mut cmd: Command) -> io::Result<Self> {
            let handle = unsafe { CreateJobObjectW(None, PCWSTR::null()) }?;
            let job = unsafe { OwnedHandle::from_raw_handle(handle.0) };
            let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            unsafe { SetInformationJobObject(handle, JobObjectExtendedLimitInformation, (&raw const info).cast(), size_of_val(&info) as u32) }?;
            let mut child = {
                let _guard = CONSOLE.lock().unwrap_or_else(|e| e.into_inner());
                if unsafe { GetConsoleProcessList(&mut [0]) } == 0 {
                    let _ = unsafe { SetConsoleCtrlHandler(None, false) };
                }
                cmd.creation_flags((CREATE_NO_WINDOW | CREATE_SUSPENDED).0).spawn()?
            };
            let started = unsafe { AssignProcessToJobObject(handle, HANDLE(child.as_raw_handle())) }.map_err(io::Error::from).and_then(|()| resume(&child));
            if let Err(e) = started {
                let _ = child.kill();
                let _ = child.wait();
                return Err(e);
            }
            Ok(Self { child, job })
        }

        pub(super) fn stop(&mut self) -> bool {
            let job = HANDLE(self.job.as_raw_handle());
            if !job_running(job) { return false; }
            if interrupt_console(self.child.id(), job) && wait_job(job) {
                return self.child.wait().ok().and_then(|exit| exit.code()) == Some(STATUS_CONTROL_C_EXIT.0);
            }
            if unsafe { TerminateJobObject(job, 1) }.is_err() {
                let _ = self.child.kill();
            }
            let _ = self.child.wait();
            wait_job(job);
            true
        }
    }

    fn job_running(job: HANDLE) -> bool {
        let mut info = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
        let result = unsafe { QueryInformationJobObject(Some(job), JobObjectBasicAccountingInformation, (&raw mut info).cast(), size_of_val(&info) as u32, None) };
        result.is_err() || info.ActiveProcesses > 0
    }

    fn wait_job(job: HANDLE) -> bool {
        let deadline = Instant::now() + STOP_GRACE;
        while job_running(job) {
            if Instant::now() >= deadline { return false; }
            std::thread::sleep(POLL);
        }
        true
    }

    fn interrupt_console(pid: u32, job: HANDLE) -> bool {
        let _guard = CONSOLE.lock().unwrap_or_else(|e| e.into_inner());
        let std_handles = [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE].map(|id| (id, unsafe { GetStdHandle(id) }.unwrap_or_default()));
        if unsafe { AttachConsole(pid) }.is_err() { return false; }
        for (id, handle) in std_handles {
            let _ = unsafe { SetStdHandle(id, handle) };
        }
        let mut pids = [0; 64];
        let count = unsafe { GetConsoleProcessList(&mut pids) } as usize;
        let sent = count >= 2 && pids.get(..count).is_some_and(|pids| {
            pids.contains(&pid) && pids.iter().all(|&id| id == std::process::id() || belongs_to_job(id, job))
        }) && unsafe { SetConsoleCtrlHandler(None, true) }.is_ok()
            && unsafe { GenerateConsoleCtrlEvent(CTRL_C_EVENT, 0) }.is_ok();
        let _ = unsafe { FreeConsole() };
        for (id, handle) in std_handles {
            let _ = unsafe { SetStdHandle(id, handle) };
        }
        sent
    }

    fn belongs_to_job(pid: u32, job: HANDLE) -> bool {
        let Ok(process) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }) else { return false };
        let _process = unsafe { OwnedHandle::from_raw_handle(process.0) };
        let mut member = BOOL(0);
        unsafe { IsProcessInJob(process, Some(job), &mut member) }.is_ok() && member.as_bool()
    }

    fn resume(child: &Child) -> io::Result<()> {
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) }?;
        let _snapshot = unsafe { OwnedHandle::from_raw_handle(snapshot.0) };
        let size = size_of::<THREADENTRY32>() as u32;
        let mut entry = THREADENTRY32 { dwSize: size, ..Default::default() };
        unsafe { Thread32First(snapshot, &mut entry) }?;
        loop {
            if entry.th32OwnerProcessID == child.id() {
                let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, false, entry.th32ThreadID) }?;
                let _thread = unsafe { OwnedHandle::from_raw_handle(thread.0) };
                if unsafe { ResumeThread(thread) } == u32::MAX {
                    return Err(io::Error::last_os_error());
                }
                return Ok(());
            }
            entry.dwSize = size;
            unsafe { Thread32Next(snapshot, &mut entry) }?;
        }
    }
}

#[cfg(test)]
#[path = "../test/git_process.rs"]
mod tests;
