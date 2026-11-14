use flit_core::preview::HostEvent;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

const REPORT_EVERY: Duration = Duration::from_millis(250);
const CONNECT_FOR: Duration = Duration::from_secs(3);
const POLL: Duration = Duration::from_millis(200);
const WATCHED: [&str; 3] = ["time-pos", "duration", "pause"];

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Progress {
    pub position: f64,
    pub duration: f64,
    pub playing: bool,
    pub ended: bool,
}

impl Progress {
    pub(crate) fn apply(&mut self, line: &str) -> bool {
        let Ok(v) = serde_json::from_str::<Value>(line) else { return false };
        let data = v.get("data");
        match (v.get("event").and_then(Value::as_str), v.get("name").and_then(Value::as_str)) {
            (Some("property-change"), Some("time-pos")) => data.and_then(Value::as_f64).is_some_and(|t| {
                self.position = t.max(0.0);
                true
            }),
            (Some("property-change"), Some("duration")) => data.and_then(Value::as_f64).is_some_and(|d| {
                self.duration = d.max(0.0);
                true
            }),
            (Some("property-change"), Some("pause")) => data.and_then(Value::as_bool).is_some_and(|paused| {
                self.playing = !paused;
                true
            }),
            (Some("end-file"), _) => {
                self.finish();
                true
            }
            _ => false,
        }
    }

    pub(crate) fn finish(&mut self) {
        self.ended = true;
        self.playing = false;
        if self.duration > 0.0 {
            self.position = self.duration;
        }
    }

    pub(crate) fn event(&self, gen: u64) -> HostEvent {
        HostEvent::Media { gen, position: self.position, duration: self.duration, playing: self.playing }
    }
}

pub(crate) fn command(args: Value) -> String {
    format!("{}\n", json!({ "command": args }))
}

pub(crate) fn observe() -> String {
    WATCHED.iter().enumerate().map(|(k, name)| command(json!(["observe_property", k + 1, name]))).collect()
}

pub(crate) fn seek(seconds: f64) -> String {
    command(json!(["seek", seconds.max(0.0), "absolute"]))
}

pub(crate) fn volume(level: f32) -> String {
    command(json!(["set_property", "volume", (level.clamp(0.0, 1.0) * 100.0).round()]))
}

pub(crate) fn pause(paused: bool) -> String {
    command(json!(["set_property", "pause", paused]))
}

pub(crate) fn socket_path(dir: &Path, gen: u64) -> PathBuf {
    dir.join(format!("flit-mpv-{}-{gen}.sock", std::process::id()))
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

pub(crate) struct Player {
    child: Arc<Mutex<Child>>,
    ipc: Arc<Mutex<Option<UnixStream>>>,
    socket: Option<PathBuf>,
    stopped: Arc<AtomicBool>,
}

impl Player {
    pub(crate) fn start(child: Child, socket: Option<PathBuf>, gen: u64, report: impl Fn(HostEvent) + Send + 'static) -> Self {
        let player = Self { child: Arc::new(Mutex::new(child)), ipc: Arc::default(), socket, stopped: Arc::default() };
        let (child, ipc, socket, stopped) = (player.child.clone(), player.ipc.clone(), player.socket.clone(), player.stopped.clone());
        let watch = move || {
            let mut progress = Progress { playing: true, ..Progress::default() };
            if let Some(stream) = socket.as_deref().and_then(|s| connect(s, &child)) {
                follow(stream, &ipc, &stopped, &mut progress, gen, &report);
            }
            while !stopped.load(Ordering::Acquire) && matches!(lock(&child).try_wait(), Ok(None)) {
                std::thread::sleep(POLL);
            }
            if !stopped.load(Ordering::Acquire) {
                progress.finish();
                report(progress.event(gen));
            }
        };
        if let Err(e) = std::thread::Builder::new().name("flit-player".into()).spawn(watch) {
            tracing::warn!("cannot watch the media player: {e}");
        }
        player
    }

    pub(crate) fn alive(&self) -> bool {
        matches!(lock(&self.child).try_wait(), Ok(None))
    }

    pub(crate) fn signal(&self, signal: libc::c_int) -> bool {
        let child = lock(&self.child);
        let Ok(pid) = libc::pid_t::try_from(child.id()) else { return false };
        unsafe { libc::kill(-pid, signal) == 0 }
    }

    pub(crate) fn send(&self, line: &str) -> bool {
        lock(&self.ipc).as_mut().is_some_and(|s| s.write_all(line.as_bytes()).is_ok())
    }

    pub(crate) fn stop(self) {
        self.stopped.store(true, Ordering::Release);
        self.signal(libc::SIGKILL);
        {
            let mut child = lock(&self.child);
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(s) = lock(&self.ipc).take() {
            let _ = s.shutdown(std::net::Shutdown::Both);
        }
        if let Some(path) = &self.socket {
            let _ = std::fs::remove_file(path);
        }
    }
}

fn connect(socket: &Path, child: &Mutex<Child>) -> Option<UnixStream> {
    let until = Instant::now() + CONNECT_FOR;
    while Instant::now() < until && matches!(lock(child).try_wait(), Ok(None)) {
        if let Ok(s) = UnixStream::connect(socket) {
            return Some(s);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    None
}

fn follow(stream: UnixStream, ipc: &Mutex<Option<UnixStream>>, stopped: &AtomicBool, progress: &mut Progress, gen: u64, report: &impl Fn(HostEvent)) {
    let Ok(mut writer) = stream.try_clone() else { return };
    if writer.write_all(observe().as_bytes()).is_err() {
        return;
    }
    *lock(ipc) = Some(writer);
    let mut sent = (Instant::now(), *progress);
    for line in BufReader::new(stream).lines() {
        let Ok(line) = line else { break };
        if stopped.load(Ordering::Acquire) {
            return;
        }
        if !progress.apply(&line) {
            continue;
        }
        if progress.ended {
            break;
        }
        let urgent = progress.playing != sent.1.playing || progress.duration != sent.1.duration;
        if urgent || sent.0.elapsed() >= REPORT_EVERY {
            report(progress.event(gen));
            sent = (Instant::now(), *progress);
        }
    }
}

#[cfg(test)]
#[path = "../test/player.rs"]
mod tests;
