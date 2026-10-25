use crossbeam_channel::{unbounded, Receiver, Sender};
use flit_core::library::LibraryOp;
use flit_core::ui::{KnownFolder, Places};
use flit_core::{FsPath, Location};
use flit_platform::{Platform, Wake};
use std::sync::Arc;
use std::time::Instant;

const RECENT_LIMIT: usize = 24;

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
    std::fs::metadata(&*p.display()).is_ok_and(|m| m.is_dir())
}

impl PlacesWorker {
    pub fn new<P: Platform>(wake: Wake) -> Self {
        let (requests, jobs) = unbounded::<Job>();
        let (done, results) = unbounded();
        let again = requests.clone();
        let run = move || {
            P::watch_wsl(Box::new(move || {
                let _ = again.send(Job::Refresh);
            }));
            let send = |reply: Reply| {
                let sent = done.send(reply).is_ok();
                wake();
                sent
            };
            let mut cached = Places::default();
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
                if !send(Reply::Places(Arc::new(cached.clone()))) {
                    return;
                }
            }
        };
        Self { requests, results, current: Arc::default(), pending: Some(Box::new(run)) }
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
    Places { folders, drives: P::drives(), wsl: P::wsl_distros(), recycle_bin: cfg!(windows), ..Default::default() }
}
