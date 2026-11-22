use crate::thumbs::ThumbCache;
use crossbeam_channel::{unbounded, Receiver, Sender};
use flit_core::ui::settings::{self, Doc};
use flit_core::ui::{Value, WatchLimit, SETTINGS};
use flit_platform::{PlatformError, ProfilePlatform, SystemPlatform, Wake, WindowId};
use serde_json::{Map, Value as Json};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

pub enum Job {
    Write(String),
    SaveState(PathBuf, Box<crate::state::Saved>),
    Backup,
    Check,
    #[cfg(target_os = "linux")]
    CheckTray,
    Export(PathBuf, String, bool),
    Import(WindowId, PathBuf),
    Portable(WindowId, u64, flit_core::portable_meta::Request),
    DiskChanges(WindowId, u64, flit_core::disk_changes::Request),
    BackupImport(WindowId, Box<settings::ImportPlan>, String),
    CacheSize,
    CacheLimit(u32),
    Integrate(bool),
    RaiseWatchLimit(fn() -> WatchLimit),
    SystemChanges(crate::system_changes::Action),
    ResolveLink(flit_platform::WindowId, flit_core::open::Open),
    #[cfg(target_os = "linux")]
    Portal { on: bool, startup: bool },
}

pub enum Reply {
    System { os: String, dates: (String, String), missing: Vec<flit_core::ui::MissingTool> },
    Changed(String),
    #[cfg(target_os = "linux")]
    TrayAvailable(bool),
    SaveFailed(String),
    Imported(WindowId, Result<Doc, String>),
    Portable(WindowId, u64, Result<flit_core::portable_meta::Metadata, String>),
    DiskChanges(WindowId, u64, flit_core::disk_changes::Reply),
    ImportBackedUp(WindowId, Box<settings::ImportPlan>, Result<PathBuf, String>),
    Exported(Result<PathBuf, String>),
    ExportExists { path: PathBuf, size: u64, modified: i64 },
    CacheUsed(u64),
    IntegrationFailed(bool, String),
    WatchLimit(WatchLimit),
    SystemChanges(crate::system_changes::Reply),
    SystemChangesUnavailable { notify: bool },
    ResolvedLink(flit_platform::WindowId, Option<crate::system_link::Link>),
    #[cfg(target_os = "linux")]
    Portal(bool, Result<Option<bool>, String>),
}

pub type Integrate = fn(bool) -> Result<(), PlatformError>;

pub fn file<P: ProfilePlatform>() -> Option<PathBuf> {
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

fn read_import(from: &Path) -> Result<Doc, String> {
    use std::io::Read;
    const MAX_IMPORT: u64 = 4 * 1024 * 1024;
    let mut text = String::new();
    let file = std::fs::File::open(from).map_err(|e| e.to_string())?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err(flit_core::i18n::S::SettingsPageImportUnavailable.en().into());
    }
    let count = file.take(MAX_IMPORT + 1).read_to_string(&mut text).map_err(|e| e.to_string())?;
    if count as u64 > MAX_IMPORT {
        return Err(flit_core::i18n::S::SettingsPageImportTooLarge.en().into());
    }
    Ok(settings::parse(&text))
}

fn backup_import(path: Option<&Path>, last: &str, text: &str) -> Result<PathBuf, String> {
    use flit_core::i18n::S;
    let path = path.ok_or_else(|| S::SettingsPageImportUnavailable.en().to_owned())?;
    match std::fs::read_to_string(path) {
        Ok(current) if current == last => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && last.is_empty() => {}
        Ok(_) => return Err(S::SettingsPageImportStale.en().into()),
        Err(e) => return Err(e.to_string()),
    }
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|e| e.to_string())?;
    let backup = path.with_file_name(format!("settings.before-import-{}-{}.json", std::process::id(), stamp.as_nanos()));
    write_atomic(&backup, text.as_bytes()).map_err(|e| e.to_string())?;
    Ok(backup)
}

fn backup_path(path: &Path) -> PathBuf {
    path.with_file_name("settings.broken.json")
}

fn locked(c: &Mutex<ThumbCache>) -> std::sync::MutexGuard<'_, ThumbCache> {
    c.lock().unwrap_or_else(|e| e.into_inner())
}

fn work<P: ProfilePlatform>(path: Option<PathBuf>, cache: Option<Arc<Mutex<ThumbCache>>>, integrate: Option<Integrate>, first: String, jobs: Receiver<Job>, replies: Sender<Reply>, wake: Wake) {
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
                            Err(e) => {
                                tracing::warn!("cannot write the settings file {}: {e}", p.display());
                                reply(Reply::SaveFailed(e.to_string()));
                            }
                        }
                    }
                }
                Job::SaveState(path, saved) => crate::state::save_to(&path, &saved),
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
                #[cfg(target_os = "linux")]
                Job::CheckTray => reply(Reply::TrayAvailable(flit_platform_linux_gui::tray_host_available())),
                Job::Export(to, text, replace) => reply(export(to, &text, replace)),
                Job::Import(id, from) => reply(Reply::Imported(id, read_import(&from))),
                Job::Portable(id, request, req) => reply(Reply::Portable(id, request, crate::portable_meta::request(req).map_err(|e| e.to_string()))),
                Job::DiskChanges(id, request, req) => {
                    let exporting = matches!(req, flit_core::disk_changes::Request::Export { .. });
                    let (tx, wake) = (replies.clone(), wake.clone());
                    let started = std::thread::Builder::new().name("flit-disk-changes".into()).spawn(move || {
                        let _ = tx.send(Reply::DiskChanges(id, request, crate::disk_changes::run(req)));
                        wake();
                    });
                    if let Err(e) = started {
                        use flit_core::disk_changes::{Error, Reply as DiskReply};
                        let failed = Error::Failed(e.to_string());
                        let outcome = if exporting { DiskReply::Exported(Err(failed)) } else { DiskReply::Read(Err(failed)) };
                        reply(Reply::DiskChanges(id, request, outcome));
                    }
                }
                Job::BackupImport(id, plan, text) => reply(Reply::ImportBackedUp(id, plan, backup_import(path.as_deref(), &last, &text))),
                Job::CacheSize => {
                    if let Some(c) = &cache {
                        let used = locked(c).used();
                        reply(Reply::CacheUsed(used));
                    }
                }
                Job::Integrate(on) => {
                    if let Some(Err(e)) = integrate.map(|f| f(on)) {
                        tracing::warn!("cannot turn the default file manager {}: {e:?}", if on { "on" } else { "off" });
                        reply(Reply::IntegrationFailed(on, e.message));
                    }
                }
                Job::RaiseWatchLimit(raise) => {
                    let (replies, wake) = (replies.clone(), wake.clone());
                    let spawned = std::thread::Builder::new().name("flit-watch-limit".into()).spawn(move || {
                        let _ = replies.send(Reply::WatchLimit(raise()));
                        wake();
                    });
                    if let Err(e) = spawned {
                        reply(Reply::WatchLimit(WatchLimit::Failed(e.to_string())));
                    }
                }
                Job::ResolveLink(id, open) => reply(Reply::ResolvedLink(id, crate::system_link::resolve(&open))),
                Job::SystemChanges(action) => {
                    if integrate.is_some() {
                        reply(Reply::SystemChanges(crate::system_changes::run::<P>(action)));
                    } else {
                        use crate::system_changes::Action;
                        let notify = matches!(action, Action::Undo | Action::Autostart { startup: false, .. }
                            | Action::Protocol { startup: false, .. } | Action::RegisterArchive { startup: false });
                        reply(Reply::SystemChangesUnavailable { notify });
                    }
                }
                #[cfg(target_os = "linux")]
                Job::Portal { on, startup } => {
                    if path.is_some() {
                        reply(Reply::Portal(on, crate::portal_config::configure::<P>(on, startup).map_err(|e| e.to_string())));
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
    pub fn start<P: ProfilePlatform + SystemPlatform>(file: Option<PathBuf>, text: Option<String>, cache: Option<Arc<Mutex<ThumbCache>>>, integrate: bool, wake: Wake) -> Self {
        let (jobs, rx) = unbounded::<Job>();
        let (tx, replies) = unbounded::<Reply>();
        let (path, first) = (file, text.unwrap_or_default());
        let integrate: Option<Integrate> = integrate.then_some(P::integrate);
        let thread = std::thread::Builder::new()
            .name("flit-settings".into())
            .spawn(move || {
                let _ = tx.send(Reply::System { os: P::os_version(), dates: P::date_patterns(), missing: P::missing_tools() });
                wake();
                if integrate.is_some() {
                    P::tidy_integration();
                    #[cfg(windows)]
                    let _ = tx.send(Reply::SystemChanges(crate::system_changes::run::<P>(crate::system_changes::Action::RegisterArchive { startup: true })));
                    wake();
                }
                work::<P>(path, cache, integrate, first, rx, tx, wake)
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
        if let Some(on) = integration_change(&self.values, &values) {
            self.send(Job::Integrate(on));
        }
        if let Some(action) = autostart_change(&self.values, &values) {
            self.send(Job::SystemChanges(action));
        }
        if let Some(on) = flag(&values, settings::PROTOCOL_LINKS) {
            let previous = flag(&self.values, settings::PROTOCOL_LINKS);
            if previous.map_or(on, |was| was != on) {
                self.send(Job::SystemChanges(crate::system_changes::Action::Protocol { on, startup: previous.is_none() }));
            }
        }
        #[cfg(target_os = "linux")]
        if let Some(on) = flag(&values, settings::PORTAL_PICKER).filter(|on| *on != flag(&self.values, settings::PORTAL_PICKER).unwrap_or(false)) {
            self.send(Job::Portal { on, startup: flag(&self.values, settings::PORTAL_PICKER).is_none() });
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

pub fn integration_change(before: &[Value], now: &[Value]) -> Option<bool> {
    match (flag(before, settings::DEFAULT_MANAGER), flag(now, settings::DEFAULT_MANAGER)) {
        (Some(was), Some(on)) if was != on => Some(on),
        (None, Some(true)) => Some(true),
        _ => None,
    }
}

pub fn autostart_change(before: &[Value], now: &[Value]) -> Option<crate::system_changes::Action> {
    let on = flag(now, settings::AUTOSTART)?;
    let previous = flag(before, settings::AUTOSTART);
    let background = flag(now, settings::BACKGROUND).unwrap_or(false);
    let background_changed = flag(before, settings::BACKGROUND).is_some_and(|was| was != background);
    (previous.map_or(on, |was| was != on) || (on && background_changed))
        .then_some(crate::system_changes::Action::Autostart { on, startup: previous.is_none(), background })
}

pub fn flag(values: &[Value], id: &str) -> Option<bool> {
    match values.get(settings::index(id)?) {
        Some(Value::Bool(b)) => Some(*b),
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
