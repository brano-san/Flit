use flit_platform::{AgentImage, AgentImages, AgentLauncher, AgentProcess};
use std::io::{self, Read, Write};
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, ChildStderr, Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use windows::core::PCWSTR;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::JobObjects::{AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE};
use windows::Win32::System::SystemInformation::{IMAGE_FILE_MACHINE, IMAGE_FILE_MACHINE_ARM64};
use windows::Win32::System::Threading::{GetCurrentProcess, IsWow64Process2, CREATE_NO_WINDOW};

const AGENT_DIR: &str = r#""${XDG_CACHE_HOME:-$HOME/.cache}/flit""#;

pub fn native_arm64() -> bool {
    let (mut process, mut native) = (IMAGE_FILE_MACHINE::default(), IMAGE_FILE_MACHINE::default());
    unsafe { IsWow64Process2(GetCurrentProcess(), &mut process, Some(&mut native)) }.is_ok() && native == IMAGE_FILE_MACHINE_ARM64
}

pub fn launcher(distro: &str, images: &AgentImages) -> Option<Box<dyn AgentLauncher>> {
    let image = if native_arm64() { images.arm64 } else { images.x64 }?;
    Some(Box::new(WslLauncher { distro: distro.to_owned(), image }))
}

fn job() -> Option<HANDLE> {
    static JOB: OnceLock<usize> = OnceLock::new();
    let raw = *JOB.get_or_init(|| unsafe {
        let Ok(job) = CreateJobObjectW(None, PCWSTR::null()) else { return 0 };
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let set = SetInformationJobObject(job, JobObjectExtendedLimitInformation, &info as *const _ as *const _, size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32);
        if set.is_err() {
            return 0;
        }
        job.0 as usize
    });
    (raw != 0).then_some(HANDLE(raw as *mut _))
}

pub fn wsl_exe() -> PathBuf {
    PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into())).join(r"System32\wsl.exe")
}

pub fn shell(distro: &str, script: &str) -> io::Result<Child> {
    let child = Command::new(wsl_exe())
        .args(["-d", distro, "--exec", "/bin/sh", "-c", script])
        .creation_flags(CREATE_NO_WINDOW.0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let assigned = job().map(|job| unsafe { AssignProcessToJobObject(job, HANDLE(child.as_raw_handle())) });
    if !matches!(assigned, Some(Ok(()))) {
        tracing::warn!("wsl.exe for {distro} runs outside the Flit job object: {assigned:?}");
    }
    Ok(child)
}

pub fn decode(bytes: &[u8]) -> String {
    let text = if bytes.contains(&0) { String::from_utf16_lossy(&bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect::<Vec<_>>()) } else { String::from_utf8_lossy(bytes).into_owned() };
    text.trim().to_owned()
}

fn log_stderr(distro: String, mut err: ChildStderr) {
    let _ = std::thread::Builder::new().name(format!("flit-wsl-err-{distro}")).spawn(move || {
        let mut buf = [0u8; 4096];
        while let Ok(n @ 1..) = err.read(&mut buf) {
            tracing::info!("wsl {distro}: {}", decode(&buf[..n]));
        }
    });
}

fn stop(mut child: Child, grace: Duration) {
    let end = Instant::now() + grace;
    while Instant::now() < end {
        if let Ok(Some(_)) = child.try_wait() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let _ = child.kill();
    let _ = child.wait();
}

pub struct WslLauncher {
    distro: String,
    image: AgentImage,
}

impl WslLauncher {
    pub fn new(distro: &str, image: AgentImage) -> Self {
        Self { distro: distro.to_owned(), image }
    }
}

impl AgentLauncher for WslLauncher {
    fn build(&self) -> &str {
        self.image.build
    }

    fn launch(&self) -> io::Result<AgentProcess> {
        let mut child = shell(&self.distro, &format!("exec {AGENT_DIR}/agent-{}", self.image.build))?;
        let (input, output, err) = (child.stdin.take(), child.stdout.take(), child.stderr.take());
        let (Some(input), Some(output), Some(err)) = (input, output, err) else { return Err(io::Error::other("wsl.exe has no pipes")) };
        log_stderr(self.distro.clone(), err);
        Ok(AgentProcess { input: Box::new(input), output: Box::new(output), stop: Box::new(move |grace| stop(child, grace)) })
    }

    fn deliver(&self) -> io::Result<()> {
        let id = self.image.build;
        let script = format!(r#"set -e; d={AGENT_DIR}; mkdir -p "$d"; chmod 700 "$d"; t="$d/agent-{id}.$$"; cat > "$t"; chmod 700 "$t"; mv -f "$t" "$d/agent-{id}""#);
        let mut child = shell(&self.distro, &script)?;
        let mut input = child.stdin.take().ok_or_else(|| io::Error::other("wsl.exe has no stdin"))?;
        let bytes = self.image.bytes;
        let writer = std::thread::spawn(move || input.write_all(bytes));
        let out = child.wait_with_output()?;
        let written = writer.join().unwrap_or_else(|_| Err(io::Error::other("agent writer panicked")));
        if !out.status.success() {
            return Err(io::Error::other(format!("{} ({})", decode(&out.stderr), out.status)));
        }
        written
    }
}

#[cfg(test)]
#[path = "../test/wsl.rs"]
mod tests;
