use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

pub const SUBCOMMAND: &str = "--parse-preview";
pub const PROBE_TASK: &str = "probe";
pub const ARCHIVE_HEADER: &[u8] = b"FLIT-ARCHIVE\0";
pub const EXIT_TOO_LARGE: i32 = 3;
pub const EXIT_PASSWORD_REQUIRED: i32 = 4;
pub const EXIT_UNSUPPORTED: i32 = 5;
const POLL: Duration = Duration::from_millis(20);
const TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Failed,
    TimedOut,
    Cancelled,
    TooLarge,
    PasswordRequired,
    Unsupported,
}

#[cfg(target_os = "linux")]
use std::{path::PathBuf, sync::OnceLock};

#[cfg(target_os = "linux")]
fn bwrap_on_path() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|p| p.join("bwrap")).find(|p| p.is_file())
}

#[cfg(target_os = "linux")]
fn sandbox(bwrap: &Path, exe: &Path, input: &Path, mapped: &Path) -> Command {
    let mut cmd = Command::new(bwrap);
    cmd.args(["--unshare-all", "--die-with-parent", "--new-session", "--clearenv", "--chdir", "/"]);
    for dir in ["/lib", "/lib64", "/usr/lib", "/usr/lib64", "/etc/ld.so.cache", "/nix/store"] {
        cmd.args(["--ro-bind-try", dir, dir]);
    }
    cmd.arg("--ro-bind").arg(exe).arg("/flit");
    cmd.arg("--ro-bind").arg(input).arg(mapped);
    cmd.args(["--remount-ro", "/", "--", "/flit"]);
    cmd
}

#[cfg(target_os = "linux")]
fn sandbox_command(cache: &OnceLock<Option<PathBuf>>, exe: &Path, input: &Path, find: impl FnOnce() -> Option<PathBuf>) -> Option<(Command, PathBuf)> {
    let bwrap = cache
        .get_or_init(|| {
            let bwrap = find()?;
            let mapped = Path::new("/source/flit");
            let mut probe = sandbox(&bwrap, exe, exe, mapped);
            probe.args([SUBCOMMAND, PROBE_TASK]).arg(mapped);
            collect(&mut probe, 0, &|| true, TIMEOUT).ok()?;
            Some(bwrap)
        })
        .as_deref()?;
    let mapped = Path::new("/source").join(input.file_name()?);
    let cmd = sandbox(bwrap, exe, &input.canonicalize().ok()?, &mapped);
    Some((cmd, mapped))
}

fn command(exe: &Path, input: &Path, task: &str, args: &[&str]) -> Command {
    #[cfg(target_os = "linux")]
    let exe = if exe.is_file() {
        static BWRAP: OnceLock<Option<PathBuf>> = OnceLock::new();
        if let Some((mut cmd, mapped)) = sandbox_command(&BWRAP, exe, input, bwrap_on_path) {
            cmd.args([SUBCOMMAND, task]).arg(mapped).args(args);
            return cmd;
        }
        exe
    } else {
        Path::new("/proc/self/exe")
    };
    let mut cmd = Command::new(exe);
    cmd.args([SUBCOMMAND, task]).arg(input).args(args);
    cmd
}

pub fn run(input: &Path, task: &str, args: &[&str], max_output: usize, current: &dyn Fn() -> bool) -> Result<Vec<u8>, Error> {
    if !current() {
        return Err(Error::Cancelled);
    }
    if !input.is_file() {
        return Err(Error::Failed);
    }
    let exe = std::env::current_exe().map_err(|_| Error::Failed)?;
    let mut cmd = command(&exe, input, task, args);
    collect(&mut cmd, max_output, current, TIMEOUT)
}

fn collect(cmd: &mut Command, max_output: usize, current: &dyn Fn() -> bool, timeout: Duration) -> Result<Vec<u8>, Error> {
    let (status, bytes) = bounded(cmd.stdin(Stdio::null()), max_output, timeout, current, |_| true)?;
    let outcome = match status.code() {
        Some(0) => return Ok(bytes),
        Some(EXIT_TOO_LARGE) => Error::TooLarge,
        Some(EXIT_PASSWORD_REQUIRED) => Error::PasswordRequired,
        Some(EXIT_UNSUPPORTED) => Error::Unsupported,
        _ => return Err(Error::Failed),
    };
    Err(if bytes.is_empty() { outcome } else { Error::Failed })
}

pub fn bounded(cmd: &mut Command, max_output: usize, timeout: Duration, current: &dyn Fn() -> bool, started: impl FnOnce(&mut Child) -> bool) -> Result<(ExitStatus, Vec<u8>), Error> {
    if !current() {
        return Err(Error::Cancelled);
    }
    let cap = (max_output as u64).checked_add(1).ok_or(Error::Failed)?;
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd.stdout(Stdio::piped()).stderr(Stdio::null()).spawn().map_err(|_| Error::Failed)?;
    let until = Instant::now() + timeout;
    let result = if started(&mut child) { wait(&mut child, cap, max_output, current, until) } else { Err(Error::Failed) };
    let _ = child.kill();
    let _ = child.wait();
    result
}

fn wait(child: &mut Child, cap: u64, max_output: usize, current: &dyn Fn() -> bool, until: Instant) -> Result<(ExitStatus, Vec<u8>), Error> {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let stdout = child.stdout.take().ok_or(Error::Failed)?;
    std::thread::Builder::new()
        .name("flit-parser-output".into())
        .spawn(move || {
            let mut bytes = Vec::new();
            let result = stdout.take(cap).read_to_end(&mut bytes).is_ok() && bytes.len() <= max_output;
            let _ = tx.send(result.then_some(bytes));
        })
        .map_err(|_| Error::Failed)?;
    let mut data = None;
    loop {
        if !current() {
            return Err(Error::Cancelled);
        }
        if Instant::now() >= until {
            return Err(Error::TimedOut);
        }
        if data.is_none() {
            match rx.try_recv() {
                Ok(Some(bytes)) => data = Some(bytes),
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                _ => return Err(Error::Failed),
            }
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                if let Some(bytes) = data.take() {
                    return Ok((status, bytes));
                }
            }
            Ok(None) => {}
            Err(_) => return Err(Error::Failed),
        }
        std::thread::sleep(POLL);
    }
}

#[cfg(test)]
#[path = "../test/parse_process.rs"]
mod tests;
