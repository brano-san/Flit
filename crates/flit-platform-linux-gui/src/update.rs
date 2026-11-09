use flit_platform::PlatformError;
use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const CHUNK: usize = 64 * 1024;

fn failed(e: impl std::fmt::Display) -> PlatformError {
    PlatformError::new(-1, e.to_string())
}

pub(crate) fn download(url: &str, out: &mut dyn Write, progress: &mut dyn FnMut(u64, Option<u64>) -> bool) -> Result<(), PlatformError> {
    let agent = format!("Flit/{}", env!("CARGO_PKG_VERSION"));
    let mut child = Command::new("curl")
        .args(["-fsSL", "--proto", "=https", "--connect-timeout", "30", "--max-time", "900", "-A", &agent, "-D", "/dev/stderr", "--", url])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| match e.kind() {
            ErrorKind::NotFound => failed("curl is not installed: install curl to download Flit updates"),
            _ => failed(format!("curl: {e}")),
        })?;
    let (Some(mut body), Some(headers)) = (child.stdout.take(), child.stderr.take()) else { return Err(failed("curl: no output")) };
    let length = Arc::new(AtomicU64::new(0));
    let seen = length.clone();
    let reader = std::thread::spawn(move || {
        let mut message = String::new();
        for line in BufReader::new(headers).lines().map_while(Result::ok) {
            let line = line.trim();
            match line.split_once(':') {
                Some((k, v)) if k.eq_ignore_ascii_case("content-length") => seen.store(v.trim().parse().unwrap_or(0), Ordering::Release),
                _ if line.starts_with("curl:") => message = line.to_owned(),
                _ => {}
            }
        }
        message
    });
    let mut buf = vec![0u8; CHUNK];
    let mut received = 0u64;
    let outcome = loop {
        let n = match body.read(&mut buf) {
            Ok(0) => break Ok(()),
            Ok(n) => n,
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) => break Err(failed(e)),
        };
        if let Err(e) = out.write_all(&buf[..n]) {
            break Err(failed(e));
        }
        received += n as u64;
        let total = Some(length.load(Ordering::Acquire)).filter(|&t| t > 0);
        if !progress(received, total) {
            break Err(failed("cancelled"));
        }
    };
    if outcome.is_err() {
        let _ = child.kill();
    }
    let status = child.wait().map_err(failed)?;
    let message = reader.join().unwrap_or_default();
    outcome?;
    if !status.success() {
        return Err(PlatformError::new(status.code().unwrap_or(-1).into(), if message.is_empty() { format!("curl failed on {url}") } else { message }));
    }
    Ok(())
}

fn alive(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|s| s.rsplit_once(')').is_some_and(|(_, rest)| !rest.trim_start().starts_with('Z')))
}

pub(crate) fn wait_exit(pid: u32, limit: Duration) {
    let end = Instant::now() + limit;
    while alive(pid) && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(20));
    }
}
