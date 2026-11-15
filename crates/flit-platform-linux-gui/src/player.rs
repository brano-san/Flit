use flit_core::preview::{HostEvent, MediaCmd};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

pub(crate) const REPORT_EVERY: Duration = Duration::from_millis(250);
const CONNECT_FOR: Duration = Duration::from_secs(3);
const POLL: Duration = Duration::from_millis(200);
const WATCHED: [&str; 3] = ["time-pos", "duration", "pause"];
const REFRESH_HZ: (f64, f64, f64) = (24.0, 60.0, 360.0);

pub(crate) type Report = Arc<dyn Fn(HostEvent) + Send + Sync>;

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
            (Some("property-change"), Some(name)) => self.change(name, data.and_then(Value::as_f64), data.and_then(Value::as_bool)),
            (Some("end-file"), _) => {
                self.finish();
                true
            }
            _ => false,
        }
    }

    pub(crate) fn change(&mut self, name: &str, number: Option<f64>, flag: Option<bool>) -> bool {
        match (name, number, flag) {
            ("time-pos", Some(t), _) => self.position = t.max(0.0),
            ("duration", Some(d), _) => self.duration = d.max(0.0),
            ("pause", _, Some(paused)) => self.playing = !paused,
            _ => return false,
        }
        true
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

    pub(crate) fn due(&self, sent: &(Instant, Progress)) -> bool {
        self.playing != sent.1.playing || self.duration != sent.1.duration || sent.0.elapsed() >= REPORT_EVERY
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Order {
    Pause(bool),
    Seek(f64),
    Volume(f64),
}

pub(crate) fn order(cmd: MediaCmd) -> Order {
    match cmd {
        MediaCmd::Play => Order::Pause(false),
        MediaCmd::Pause => Order::Pause(true),
        MediaCmd::Seek(at) => Order::Seek(at.max(0.0)),
        MediaCmd::Volume(level) => Order::Volume(percent(level)),
    }
}

pub(crate) fn percent(level: f32) -> f64 {
    f64::from((level.clamp(0.0, 1.0) * 100.0).round())
}

pub(crate) fn line(o: Order) -> String {
    match o {
        Order::Pause(paused) => command(json!(["set_property", "pause", paused])),
        Order::Seek(at) => command(json!(["seek", at, "absolute"])),
        Order::Volume(v) => command(json!(["set_property", "volume", v])),
    }
}

fn stop_signal(o: Order) -> Option<libc::c_int> {
    match o {
        Order::Pause(true) => Some(libc::SIGSTOP),
        Order::Pause(false) => Some(libc::SIGCONT),
        _ => None,
    }
}

pub(crate) fn command(args: Value) -> String {
    format!("{}\n", json!({ "command": args }))
}

pub(crate) fn observe() -> String {
    WATCHED.iter().enumerate().map(|(k, name)| command(json!(["observe_property", k + 1, name]))).collect()
}

pub(crate) fn socket_path(dir: &Path, gen: u64) -> PathBuf {
    dir.join(format!("flit-mpv-{}-{gen}.sock", std::process::id()))
}

pub(crate) fn frame_interval(millihertz: Option<u32>) -> Duration {
    let (low, usual, high) = REFRESH_HZ;
    let hz = millihertz.map_or(usual, |m| (f64::from(m) / 1000.0).clamp(low, high));
    Duration::from_secs_f64(1.0 / hz)
}

pub(crate) fn area(rect: flit_core::render::Rect) -> Option<(u32, u32)> {
    let side = |v: f32| (v.is_finite() && v >= 1.0).then(|| v.round() as u32);
    side(rect.w).zip(side(rect.h))
}

pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(Clone, Debug)]
pub(crate) struct Start {
    pub path: PathBuf,
    pub video: bool,
    pub gen: u64,
    pub volume: Option<f32>,
    pub area: Option<(u32, u32)>,
    pub every: Duration,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Backend {
    Embedded,
    External,
}

pub(crate) fn backend(libmpv: Option<bool>) -> Backend {
    if libmpv == Some(false) {
        Backend::External
    } else {
        Backend::Embedded
    }
}

pub(crate) enum Player {
    Embedded(crate::mpv::Embedded),
    External(External),
}

impl Player {
    pub(crate) fn start(s: Start, report: Report) -> Option<Self> {
        match backend(crate::mpv::known()) {
            Backend::Embedded => Some(Self::Embedded(crate::mpv::Embedded::start(s, report))),
            Backend::External => external(&s, report).map(Self::External),
        }
    }

    pub(crate) fn order(&self, o: Order) -> bool {
        match self {
            Self::Embedded(p) => p.order(o),
            Self::External(p) => p.order(o),
        }
    }

    pub(crate) fn place(&self, area: (u32, u32)) {
        if let Self::Embedded(p) = self {
            p.place(area);
        }
    }

    pub(crate) fn stop(self) {
        match self {
            Self::Embedded(p) => {
                if let Some(thread) = p.stop() {
                    crate::mpv::keep(thread);
                }
            }
            Self::External(p) => p.stop(),
        }
    }
}

pub(crate) fn external(s: &Start, report: Report) -> Option<External> {
    let socket = std::env::var_os("XDG_RUNTIME_DIR").map(|d| socket_path(Path::new(&d), s.gen));
    match crate::shell::play_media(s.video, &s.path, socket.as_deref(), s.volume) {
        Ok((child, ipc)) => Some(External::start(child, socket.filter(|_| ipc), s.gen, move |e| report(e))),
        Err(error) => {
            report(HostEvent::Failed { gen: s.gen, message: flit_core::i18n::localize(&error.message).into_owned() });
            None
        }
    }
}

pub(crate) struct External {
    child: Arc<Mutex<Child>>,
    ipc: Arc<Mutex<Option<UnixStream>>>,
    socket: Option<PathBuf>,
    stopped: Arc<AtomicBool>,
}

impl External {
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

    pub(crate) fn order(&self, o: Order) -> bool {
        self.alive() && (self.send(&line(o)) || stop_signal(o).is_some_and(|s| self.signal(s)))
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
        if progress.due(&sent) {
            report(progress.event(gen));
            sent = (Instant::now(), *progress);
        }
    }
}

#[cfg(test)]
#[path = "../test/player.rs"]
mod tests;
