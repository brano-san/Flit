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
const KEPT_MAX_PX: u32 = 64;
const ICONS_SETTLE: std::time::Duration = std::time::Duration::from_millis(500);
pub const PLACES_FILE: &str = "places.bin";
pub const ICONS_FILE: &str = "icons.bin";

enum Job {
    Refresh,
    Library(LibraryOp),
    Pin(Vec<FsPath>),
}

pub enum Reply {
    Places(Arc<Places>),
    Failed(String),
    Pin(Vec<Location>, Vec<Location>),
}

pub struct PlacesWorker {
    requests: Sender<Job>,
    results: Receiver<Reply>,
    current: Arc<Places>,
    pending: Option<Box<dyn FnOnce() + Send>>,
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
                            let kinds: Vec<_> = paths.into_iter().filter_map(|p| std::fs::metadata(flit_platform::std_path(&p)).ok().map(|m| (m.is_dir(), Location::from_local(p)))).collect();
                            let (folders, files): (Vec<_>, Vec<_>) = kinds.into_iter().partition(|(dir, _)| *dir);
                            if !send(Reply::Pin(folders.into_iter().map(|(_, l)| l).collect(), files.into_iter().map(|(_, l)| l).collect())) {
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

    pub fn start_once(&mut self) {
        let Some(run) = self.pending.take() else { return };
        if let Err(e) = std::thread::Builder::new().name("flit-places".into()).spawn(run) {
            tracing::warn!("cannot start flit-places: {e}");
        }
        self.refresh();
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

pub fn worth_keeping(req: &IconRequest) -> bool {
    req.size <= KEPT_MAX_PX
        && match req.kind {
            IconKind::Type | IconKind::Folder | IconKind::Volume => true,
            IconKind::Place | IconKind::File => req.gen == 0,
            IconKind::CustomFolder | IconKind::Thumbnail => false,
        }
}

fn remember(kept: &mut Vec<KeptIcon>, icon: KeptIcon) {
    kept.retain(|k| k.key != icon.key);
    kept.push(icon);
    if kept.len() > KEPT_ICONS {
        kept.remove(0);
    }
}

pub struct KeptIcons {
    pub kept: Vec<KeptIcon>,
    writer: Option<(Sender<KeptIcon>, std::thread::JoinHandle<()>)>,
}

impl KeptIcons {
    pub fn load(file: Option<PathBuf>) -> Self {
        let kept: Vec<KeptIcon> = file.as_deref().and_then(load).unwrap_or_default();
        let mut on_disk = kept.clone();
        let writer = file.and_then(|path| {
            let (tx, rx) = unbounded::<KeptIcon>();
            let write = move || {
                while let Ok(icon) = rx.recv() {
                    remember(&mut on_disk, icon);
                    while let Ok(icon) = rx.recv_timeout(ICONS_SETTLE) {
                        remember(&mut on_disk, icon);
                    }
                    if let Some(bytes) = encode(&on_disk) {
                        store(&path, &bytes);
                    }
                }
            };
            std::thread::Builder::new().name("flit-kept-icons".into()).spawn(write).ok().map(|h| (tx, h))
        });
        Self { kept, writer }
    }

    pub fn keep(&mut self, req: &IconRequest, image: &Bitmap) {
        if !worth_keeping(req) {
            return;
        }
        let icon = KeptIcon { key: req.key, kind: req.kind, path: req.path.clone(), size: req.size, image: image.clone() };
        if let Some((w, _)) = &self.writer {
            let _ = w.send(icon.clone());
        }
        remember(&mut self.kept, icon);
    }

    pub fn finish(&mut self) {
        if let Some((w, h)) = self.writer.take() {
            drop(w);
            let _ = h.join();
        }
    }
}
#[cfg(test)]
#[path = "../test/places.rs"]
mod tests;
