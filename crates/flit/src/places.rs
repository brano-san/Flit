use crossbeam_channel::{unbounded, Receiver, Sender};
use flit_core::ui::{KnownFolder, Places};
use flit_core::Location;
use flit_platform::{Platform, Wake};
use std::sync::Arc;

pub struct PlacesWorker {
    requests: Sender<()>,
    results: Receiver<Places>,
    current: Arc<Places>,
    pending: Option<Box<dyn FnOnce() + Send>>,
}

impl PlacesWorker {
    pub fn new<P: Platform>(wake: Wake) -> Self {
        let (requests, jobs) = unbounded::<()>();
        let (done, results) = unbounded();
        let run = move || {
            while jobs.recv().is_ok() {
                while jobs.try_recv().is_ok() {}
                if done.send(collect::<P>()).is_err() {
                    return;
                }
                wake();
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
        let _ = self.requests.send(());
    }

    pub fn poll(&mut self) -> Option<Arc<Places>> {
        let latest = self.results.try_iter().last()?;
        self.current = Arc::new(latest);
        Some(self.current.clone())
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
    Places { folders, drives: P::drives(), wsl: P::wsl_distros() }
}
