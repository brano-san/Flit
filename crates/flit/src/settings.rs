use crate::thumbs::ThumbCache;
use crossbeam_channel::{unbounded, Receiver, Sender};
use flit_core::ui::settings::{self, Doc};
use flit_core::ui::{Value, SETTINGS};
use flit_platform::{Platform, Wake};
use serde_json::{Map, Value as Json};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

pub enum Job {
    Write(String),
    Backup,
    Check,
    Export(PathBuf, String, bool),
    Import(PathBuf),
    CacheSize,
    CacheLimit(u32),
}

pub enum Reply {
    System { os: String, dates: (String, String) },
    Changed(String),
    Imported(Result<String, String>),
    Exported(Result<PathBuf, String>),
    ExportExists { path: PathBuf, size: u64, modified: i64 },
    CacheUsed(u64),
}

pub fn file<P: Platform>() -> Option<PathBuf> {
    P::config_dir().map(|d| d.join("settings.json"))
}

pub fn read(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

fn export(to: PathBuf, text: &str, replace: bool) -> Reply {
    match std::fs::metadata(&to) {
        Ok(m) if m.is_dir() => Reply::Exported(Err(flit_core::tr_en!(flit_core::i18n::S::ErrorsSettingsIsFolder, path = to.display()))),
        Ok(m) if !replace => {
            let modified = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| (d.as_secs() as i64 + 11_644_473_600) * 10_000_000 + i64::from(d.subsec_nanos() / 100));
            Reply::ExportExists { path: to, size: m.len(), modified }
        }
        _ => Reply::Exported(write_atomic(&to, text.as_bytes()).map(|_| to).map_err(|e| e.to_string())),
    }
}

fn backup_path(path: &Path) -> PathBuf {
    path.with_file_name("settings.broken.json")
}

fn locked(c: &Mutex<ThumbCache>) -> std::sync::MutexGuard<'_, ThumbCache> {
    c.lock().unwrap_or_else(|e| e.into_inner())
}

fn work(path: Option<PathBuf>, cache: Option<Arc<Mutex<ThumbCache>>>, first: String, jobs: Receiver<Job>, replies: Sender<Reply>, wake: Wake) {
    let mut last = first;
    let reply = |r: Reply| {
        let _ = replies.send(r);
        wake();
    };
    while let Ok(job) = jobs.recv() {
        let mut batch = vec![job];
        batch.extend(jobs.try_iter());
        let newest_write = batch.iter().rposition(|j| matches!(j, Job::Write(_)));
        for (k, job) in batch.into_iter().enumerate() {
            match job {
                Job::Write(_) if Some(k) != newest_write => {}
                Job::Write(text) => {
                    if let Some(p) = path.as_deref() {
                        match write_atomic(p, text.as_bytes()) {
                            Ok(()) => last = text,
                            Err(e) => tracing::warn!("cannot write the settings file {}: {e}", p.display()),
                        }
                    }
                }
                Job::Backup => {
                    if let Some(p) = &path {
                        let _ = std::fs::copy(p, backup_path(p));
                    }
                }
                Job::Check => {
                    if let Some(text) = path.as_deref().and_then(read).filter(|t| *t != last) {
                        last.clone_from(&text);
                        reply(Reply::Changed(text));
                    }
                }
                Job::Export(to, text, replace) => reply(export(to, &text, replace)),
                Job::Import(from) => reply(Reply::Imported(std::fs::read_to_string(&from).map_err(|e| e.to_string()))),
                Job::CacheSize => {
                    if let Some(c) = &cache {
                        let used = locked(c).used();
                        reply(Reply::CacheUsed(used));
                    }
                }
                Job::CacheLimit(mb) => {
                    if let Some(c) = &cache {
                        locked(c).set_limit(mb);
                    }
                }
            }
        }
    }
}

pub struct Settings {
    pub values: Vec<Value>,
    pub unknown: Map<String, Json>,
    pub replies: Receiver<Reply>,
    pub read_only: bool,
    jobs: Option<Sender<Job>>,
    thread: Option<JoinHandle<()>>,
}

impl Settings {
    pub fn start<P: Platform>(file: Option<PathBuf>, text: Option<String>, cache: Option<Arc<Mutex<ThumbCache>>>, wake: Wake) -> Self {
        let (jobs, rx) = unbounded::<Job>();
        let (tx, replies) = unbounded::<Reply>();
        let (path, first) = (file, text.unwrap_or_default());
        let thread = std::thread::Builder::new()
            .name("flit-settings".into())
            .spawn(move || {
                let _ = tx.send(Reply::System { os: P::os_version(), dates: P::date_patterns() });
                wake();
                work(path, cache, first, rx, tx, wake)
            })
            .ok();
        Self { values: Vec::new(), unknown: Map::new(), replies, read_only: false, jobs: thread.as_ref().map(|_| jobs), thread }
    }

    pub fn send(&self, job: Job) {
        if let Some(tx) = &self.jobs {
            let _ = tx.send(job);
        }
    }

    pub fn text(&self) -> String {
        settings::render(&self.values, &self.unknown, None)
    }

    pub fn save(&self) {
        if !self.read_only {
            self.send(Job::Write(self.text()));
        }
    }

    pub fn set_values(&mut self, values: Vec<Value>) {
        let limit = cache_limit(&values);
        if !self.values.is_empty() && limit != cache_limit(&self.values) {
            if let Some(mb) = limit {
                self.send(Job::CacheLimit(mb));
            }
        }
        if let Some(level) = log_level(&values) {
            crate::logs::set_level(level);
        }
        self.values = values;
    }

    pub fn adopt(&mut self, doc: &Doc, values: Vec<Value>) {
        for problem in &doc.problems {
            tracing::warn!("settings file: {problem}");
        }
        if !doc.broken {
            self.unknown = doc.unknown.clone();
        }
        self.set_values(values);
        self.read_only = doc.newer();
        if doc.broken {
            self.send(Job::Backup);
        }
        if doc.broken || !doc.bad.is_empty() || doc.values.len() < SETTINGS.len() {
            self.save();
        }
    }

    pub fn finish(&mut self) {
        self.jobs = None;
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

pub fn cache_limit(values: &[Value]) -> Option<u32> {
    match values.get(settings::index("advanced.thumb_cache_mb")?) {
        Some(Value::Number(n)) => u32::try_from(*n).ok(),
        _ => None,
    }
}

pub fn log_level(values: &[Value]) -> Option<usize> {
    match values.get(settings::index(settings::LOG_LEVEL)?) {
        Some(Value::Choice(k)) => Some(*k),
        _ => None,
    }
}

pub fn thumb_cache_mb(doc: &Doc) -> Option<u32> {
    match doc.value("advanced.thumb_cache_mb") {
        Some(Value::Number(n)) => u32::try_from(*n).ok(),
        _ => None,
    }
}

#[cfg(test)]
#[path = "../test/settings.rs"]
mod tests;
