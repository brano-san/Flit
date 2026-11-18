use crossbeam_channel::{unbounded, Receiver, RecvTimeoutError, Sender};
use flit_core::quick_sync::{Change, QuickAccess, QuickSync};
use flit_core::Location;
use flit_platform::{Platform, Wake};
use std::time::Duration;

pub const POLL: Duration = Duration::from_secs(3);

type Start = Box<dyn FnOnce() -> Option<(Sender<Job>, Receiver<Change>)>>;

#[derive(Debug)]
pub enum Job {
    Enable(Vec<Location>),
    Resume,
    Disable,
    Pinned(Vec<Location>, Vec<Location>),
}

pub fn serve<Q: QuickAccess>(quick: Q, jobs: Receiver<Job>, changes: Sender<Change>, wake: Wake, poll: Duration) {
    let mut sync = QuickSync::new(quick);
    let send = |change: Change| {
        if change.is_empty() {
            return true;
        }
        let sent = changes.send(change).is_ok();
        wake();
        sent
    };
    loop {
        let job = if sync.enabled() { jobs.recv_timeout(poll) } else { jobs.recv().map_err(|_| RecvTimeoutError::Disconnected) };
        let alive = match job {
            Ok(Job::Enable(flit)) => send(sync.enable(&flit)),
            Ok(Job::Resume) => {
                sync.resume();
                true
            }
            Ok(Job::Disable) => {
                sync.disable();
                true
            }
            Ok(Job::Pinned(old, new)) => {
                sync.flit_changed(&old, &new);
                true
            }
            Err(RecvTimeoutError::Timeout) => sync.poll().is_none_or(send),
            Err(RecvTimeoutError::Disconnected) => false,
        };
        if !alive {
            return;
        }
    }
}

pub struct QuickSyncWorker {
    jobs: Option<Sender<Job>>,
    changes: Option<Receiver<Change>>,
    start: Option<Start>,
    on: Option<bool>,
}

impl QuickSyncWorker {
    pub fn new<P: Platform>(allowed: bool, wake: Wake) -> Self {
        let start: Start = Box::new(move || {
            let (jobs, rx) = unbounded();
            let (tx, changes) = unbounded();
            let run = move || {
                if let Some(quick) = P::quick_access() {
                    serve(quick, rx, tx, wake, POLL);
                }
            };
            match std::thread::Builder::new().name("flit-quick-access".into()).spawn(run) {
                Ok(_) => Some((jobs, changes)),
                Err(e) => {
                    tracing::warn!("cannot start the Quick access sync: {e}");
                    None
                }
            }
        });
        Self { jobs: None, changes: None, start: allowed.then_some(start), on: None }
    }

    pub fn enabled(&self) -> bool {
        self.on == Some(true)
    }

    fn send(&mut self, job: Job) {
        if self.jobs.is_none() {
            if let Some((jobs, changes)) = self.start.take().and_then(|s| s()) {
                self.jobs = Some(jobs);
                self.changes = Some(changes);
            }
        }
        if let Some(jobs) = &self.jobs {
            let _ = jobs.send(job);
        }
    }

    pub fn follow(&mut self, wanted: bool, pinned: &[Location]) {
        let first = self.on.is_none();
        if self.on == Some(wanted) || (first && !wanted) {
            self.on = Some(wanted);
            return;
        }
        self.on = Some(wanted);
        if !wanted {
            self.send(Job::Disable);
        } else if first {
            self.send(Job::Resume);
        } else {
            self.send(Job::Enable(pinned.to_vec()));
        }
    }

    pub fn pinned(&mut self, old: &[Location], new: &[Location]) {
        if self.enabled() && old != new {
            self.send(Job::Pinned(old.to_vec(), new.to_vec()));
        }
    }

    pub fn poll(&mut self) -> Vec<Change> {
        self.changes.as_ref().map(|c| c.try_iter().collect()).unwrap_or_default()
    }
}

#[cfg(test)]
#[path = "../test/quick_sync.rs"]
mod tests;
