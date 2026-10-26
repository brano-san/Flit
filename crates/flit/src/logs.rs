use crossbeam_channel::{bounded, unbounded, Receiver, Sender};
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::time::{FormatTime, SystemTime};
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::prelude::*;
use tracing_subscriber::{reload, Registry};

pub const FILE: &str = flit_core::ui::LOG_FILE;
pub const OLD: &str = "flit.1.log";
pub const MAX_BYTES: u64 = 1 << 20;
pub const LEVELS: [LevelFilter; 5] = [LevelFilter::OFF, LevelFilter::ERROR, LevelFilter::WARN, LevelFilter::INFO, LevelFilter::DEBUG];
const PANIC_FLUSH: Duration = Duration::from_secs(2);

enum Msg {
    Line(Vec<u8>),
    Flush(Sender<()>),
}

#[derive(Clone)]
pub struct Sink(Sender<Msg>);

impl Sink {
    pub fn start(dir: Option<PathBuf>, echo: bool) -> Option<Sink> {
        let (tx, rx) = unbounded();
        std::thread::Builder::new().name("flit-log".into()).spawn(move || run(dir, rx, echo)).ok()?;
        Some(Sink(tx))
    }

    pub fn line(&self, bytes: Vec<u8>) {
        let _ = self.0.send(Msg::Line(bytes));
    }

    pub fn flush(&self, wait: Duration) -> bool {
        let (tx, rx) = bounded(1);
        self.0.send(Msg::Flush(tx)).is_ok() && rx.recv_timeout(wait).is_ok()
    }
}

impl Write for &Sink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.line(buf.to_vec());
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for Sink {
    type Writer = &'a Sink;

    fn make_writer(&'a self) -> &'a Sink {
        self
    }
}

struct Out {
    dir: Option<PathBuf>,
    file: Option<BufWriter<File>>,
    size: u64,
    failed: bool,
}

impl Out {
    fn open(&mut self) -> Option<&mut BufWriter<File>> {
        if self.file.is_none() && !self.failed {
            let dir = self.dir.as_deref()?;
            let path = dir.join(FILE);
            if std::fs::metadata(&path).is_ok_and(|m| m.len() >= MAX_BYTES) {
                rotate(dir);
            }
            match std::fs::create_dir_all(dir).and_then(|_| OpenOptions::new().create(true).append(true).open(&path)) {
                Ok(f) => {
                    self.size = f.metadata().map_or(0, |m| m.len());
                    self.file = Some(BufWriter::new(f));
                }
                Err(_) => self.failed = true,
            }
        }
        self.file.as_mut()
    }

    fn put(&mut self, bytes: &[u8]) {
        let Some(f) = self.open() else { return };
        if f.write_all(bytes).is_err() {
            self.file = None;
            return;
        }
        self.size += bytes.len() as u64;
        if self.size >= MAX_BYTES {
            if let Some(mut f) = self.file.take() {
                let _ = f.flush();
            }
            if let Some(dir) = &self.dir {
                rotate(dir);
            }
        }
    }

    fn flush(&mut self) {
        if let Some(f) = &mut self.file {
            let _ = f.flush();
        }
    }
}

fn rotate(dir: &Path) {
    let _ = std::fs::rename(dir.join(FILE), dir.join(OLD));
}

fn run(dir: Option<PathBuf>, rx: Receiver<Msg>, echo: bool) {
    let mut out = Out { dir, file: None, size: 0, failed: false };
    while let Ok(first) = rx.recv() {
        let mut flushed = Vec::new();
        for msg in std::iter::once(first).chain(rx.try_iter()) {
            match msg {
                Msg::Line(bytes) => {
                    if echo {
                        let _ = std::io::stderr().write_all(&bytes);
                    }
                    out.put(&bytes);
                }
                Msg::Flush(done) => flushed.push(done),
            }
        }
        out.flush();
        for done in flushed {
            let _ = done.send(());
        }
    }
}

pub fn crash_report(dir: &Path, since: std::time::SystemTime) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "dmp"))
        .filter_map(|e| Some((e.metadata().and_then(|m| m.modified()).ok()?, e.path())))
        .filter(|(t, _)| *t >= since)
        .max()
        .map(|(_, p)| p)
}

static SINK: OnceLock<Sink> = OnceLock::new();
static LEVEL: OnceLock<(reload::Handle<LevelFilter, Registry>, LevelFilter)> = OnceLock::new();

pub fn stamp() -> String {
    let mut s = String::new();
    let _ = SystemTime.format_time(&mut Writer::new(&mut s));
    s
}

pub fn panic_record(info: &std::panic::PanicHookInfo<'_>) -> String {
    let thread = std::thread::current().name().unwrap_or("unnamed").to_owned();
    format!("{} PANIC thread '{thread}' {info}\n{}\n", stamp(), std::backtrace::Backtrace::force_capture())
}

fn on_panic(info: &std::panic::PanicHookInfo<'_>) {
    if let Some(sink) = SINK.get() {
        sink.line(panic_record(info).into_bytes());
        sink.flush(PANIC_FLUSH);
    }
}

pub fn init(dir: Option<PathBuf>, echo: bool) {
    let Some(sink) = Sink::start(dir, echo) else { return };
    let Ok(()) = SINK.set(sink) else { return };
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        on_panic(info);
        previous(info);
    }));
    let floor = if echo { LevelFilter::DEBUG } else { LevelFilter::OFF };
    let (filter, handle) = reload::Layer::new(LevelFilter::INFO.max(floor));
    let _ = LEVEL.set((handle, floor));
    let registry = tracing_subscriber::registry().with(filter);
    #[cfg(feature = "profile")]
    let _ = tracing::subscriber::set_global_default(registry.with(tracing_tracy::TracyLayer::default()));
    #[cfg(not(feature = "profile"))]
    if let Some(sink) = SINK.get().cloned() {
        let _ = tracing::subscriber::set_global_default(registry.with(tracing_subscriber::fmt::layer().with_thread_names(true).with_writer(sink)));
    }
}

pub fn set_level(choice: usize) {
    let Some((handle, floor)) = LEVEL.get() else { return };
    let wanted = LEVELS.get(choice).copied().unwrap_or(LevelFilter::INFO).max(*floor);
    if handle.clone_current() != Some(wanted) {
        let _ = handle.modify(|f| *f = wanted);
    }
}

pub fn flush() {
    if let Some(sink) = SINK.get() {
        sink.flush(PANIC_FLUSH);
    }
}

#[cfg(test)]
#[path = "../test/logs.rs"]
mod tests;
