use crossbeam_channel::{unbounded, Receiver, Sender};
use flit_core::library::LibraryOp;
use flit_core::ui::{Bitmap, IconKind, IconRequest, KnownFolder, Places};
use flit_core::{FsPath, Location};
use flit_platform::{Platform, Wake};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

const RECENT_LIMIT: usize = 24;
const SNAPSHOT_VERSION: u32 = 1;
const KEPT_ICONS: usize = 256;
const ICONS_SETTLE: std::time::Duration = std::time::Duration::from_millis(500);
pub const PLACES_FILE: &str = "places.bin";
pub const ICONS_FILE: &str = "chrome-icons.bin";

enum Job {
    Refresh,
    Library(LibraryOp),
    Pin(Vec<FsPath>),
}

pub enum Reply {
    Places(Arc<Places>),
    Failed(String),
    Pin(Vec<Location>),
}

pub struct PlacesWorker {
    requests: Sender<Job>,
    results: Receiver<Reply>,
    current: Arc<Places>,
    pending: Option<Box<dyn FnOnce() + Send>>,
}

fn is_dir(p: &FsPath) -> bool {
    std::fs::metadata(flit_platform::std_path(p)).is_ok_and(|m| m.is_dir())
}

impl PlacesWorker {
    pub fn new<P: Platform>(wake: Wake, snapshot: Option<PathBuf>) -> Self {
        let current: Arc<Places> = Arc::new(snapshot.as_deref().and_then(load).unwrap_or_default());
        let start = current.clone();
        let (requests, jobs) = unbounded::<Job>();
        let (done, results) = unbounded();
        let again = requests.clone();
        let run = move || {
            P::watch_wsl(Box::new(move || {
                let _ = again.send(Job::Refresh);
            }));
            let mut shown = start.clone();
            let mut send = |reply: Reply| {
                if let Reply::Places(p) = &reply {
                    if *p == shown {
                        return true;
                    }
                    shown = p.clone();
                }
                let sent = done.send(reply).is_ok();
                wake();
                sent
            };
            let mut cached = (*start).clone();
            let mut stored = cached.clone();
            let mut templates = false;
            while let Ok(first) = jobs.recv() {
                let mut refresh = false;
                for job in std::iter::once(first).chain(jobs.try_iter()) {
                    match job {
                        Job::Refresh => refresh = true,
                        Job::Library(op) => match P::library(&op) {
                            Ok(()) => refresh = true,
                            Err(e) => {
                                if !send(Reply::Failed(e.to_string())) {
                                    return;
                                }
                            }
                        },
                        Job::Pin(paths) => {
                            if !send(Reply::Pin(paths.iter().filter(|p| is_dir(p)).cloned().map(Location::from_local).collect())) {
                                return;
                            }
                        }
                    }
                }
                if !refresh {
                    continue;
                }
                let t = Instant::now();
                let fresh = collect::<P>();
                cached = Places { libraries: std::mem::take(&mut cached.libraries), recent: std::mem::take(&mut cached.recent), templates: std::mem::take(&mut cached.templates), recent_off: cached.recent_off, ..fresh };
                let basic = t.elapsed();
                if !send(Reply::Places(Arc::new(cached.clone()))) {
                    return;
                }
                let t = Instant::now();
                cached.libraries = P::libraries();
                let libraries = t.elapsed();
                let t = Instant::now();
                (cached.recent, cached.recent_off) = P::recent(RECENT_LIMIT);
                let recent = t.elapsed();
                let t = Instant::now();
                if !templates {
                    cached.templates = P::templates();
                    templates = true;
                }
                tracing::info!(?basic, ?libraries, ?recent, templates = ?t.elapsed(), "places collected");
                if let Some(path) = snapshot.as_deref().filter(|_| cached != stored) {
                    if let Some(bytes) = encode(&cached) {
                        store(path, &bytes);
                    }
                    stored = cached.clone();
                }
                if !send(Reply::Places(Arc::new(cached.clone()))) {
                    return;
                }
            }
        };
        Self { requests, results, current, pending: Some(Box::new(run)) }
    }

    pub fn start_once(&mut self) -> bool {
        let Some(run) = self.pending.take() else { return false };
        if let Err(e) = std::thread::Builder::new().name("flit-places".into()).spawn(run) {
            tracing::warn!("cannot start flit-places: {e}");
        }
        self.refresh();
        true
    }

    pub fn refresh(&self) {
        let _ = self.requests.send(Job::Refresh);
    }

    pub fn library(&self, op: LibraryOp) {
        let _ = self.requests.send(Job::Library(op));
    }

    pub fn pin(&self, paths: Vec<FsPath>) {
        let _ = self.requests.send(Job::Pin(paths));
    }

    pub fn poll(&mut self) -> Vec<Reply> {
        let mut out = Vec::new();
        for r in self.results.try_iter() {
            if let Reply::Places(p) = &r {
                self.current = p.clone();
                out.retain(|x| !matches!(x, Reply::Places(_)));
            }
            out.push(r);
        }
        out
    }

    pub fn current(&self) -> Arc<Places> {
        self.current.clone()
    }
}

fn collect<P: Platform>() -> Places {
    let folders = KnownFolder::ALL
        .iter()
        .filter_map(|&k| {
            let home = || (k == KnownFolder::Home).then(|| Location::from_local(P::home()));
            Some((k, P::known_folder(k.shell_name()).or_else(home)?))
        })
        .collect();
    Places { folders, drives: P::drives(), wsl: P::wsl_distros(), recycle_bin: P::recycle_bin(), ..Default::default() }
}

fn load<T: DeserializeOwned>(path: &Path) -> Option<T> {
    let bytes = std::fs::read(path).ok()?;
    let (version, rest) = postcard::take_from_bytes::<u32>(&bytes).ok()?;
    (version == SNAPSHOT_VERSION).then(|| postcard::from_bytes(rest).ok()).flatten()
}

fn encode<T: Serialize>(value: &T) -> Option<Vec<u8>> {
    let mut bytes = postcard::to_stdvec(&SNAPSHOT_VERSION).ok()?;
    bytes.extend(postcard::to_stdvec(value).ok()?);
    Some(bytes)
}

fn store(path: &Path, bytes: &[u8]) {
    if let Err(e) = crate::settings::write_atomic(path, bytes) {
        tracing::warn!("cannot keep {}: {e}", path.display());
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct KeptIcon {
    pub key: u64,
    pub kind: IconKind,
    pub path: FsPath,
    pub size: u32,
    pub image: Bitmap,
}

impl KeptIcon {
    pub fn request(&self) -> IconRequest {
        IconRequest { key: self.key, kind: self.kind, path: self.path.clone(), size: self.size, gen: 0, row: 0 }
    }
}

pub struct ChromeIcons {
    pub kept: Vec<KeptIcon>,
    changed: bool,
    writer: Option<(Sender<Vec<u8>>, std::thread::JoinHandle<()>)>,
}

impl ChromeIcons {
    pub fn load(file: Option<PathBuf>) -> Self {
        let kept = file.as_deref().and_then(load).unwrap_or_default();
        let writer = file.and_then(|path| {
            let (tx, rx) = unbounded::<Vec<u8>>();
            let write = move || {
                while let Ok(mut bytes) = rx.recv() {
                    while let Ok(newer) = rx.recv_timeout(ICONS_SETTLE) {
                        bytes = newer;
                    }
                    store(&path, &bytes);
                }
            };
            std::thread::Builder::new().name("flit-chrome-icons".into()).spawn(write).ok().map(|h| (tx, h))
        });
        Self { kept, changed: false, writer }
    }

    pub fn keep(&mut self, req: &IconRequest, image: &Bitmap) {
        if req.gen != 0 || req.kind == IconKind::Thumbnail {
            return;
        }
        self.kept.retain(|k| k.key != req.key);
        self.kept.push(KeptIcon { key: req.key, kind: req.kind, path: req.path.clone(), size: req.size, image: image.clone() });
        if self.kept.len() > KEPT_ICONS {
            self.kept.remove(0);
        }
        self.changed = true;
    }

    pub fn flush(&mut self) {
        if !std::mem::take(&mut self.changed) {
            return;
        }
        if let Some(((w, _), bytes)) = self.writer.as_ref().zip(encode(&self.kept)) {
            let _ = w.send(bytes);
        }
    }

    pub fn save(&mut self) {
        self.flush();
        if let Some((w, h)) = self.writer.take() {
            drop(w);
            let _ = h.join();
        }
    }
}

#[cfg(test)]
#[path = "../test/places.rs"]
mod tests;
