use crate::{Fs, PlatformError, Provider, Shell, Trash, Wake, Watcher};
use crossbeam_channel::{bounded, unbounded, Sender};
use flit_core::listing::{attr, DirSnapshot, Listing, ListingBuilder};
use flit_core::provider::{FileOp, GenToken, SnapshotSlot};
use flit_core::sort::{self, CaseMode, SortKeys, ViewSpec};
use flit_core::{FsError, FsErrorKind, FsPath, Location, ProviderEvent, ProviderRequest, TabId};
use rayon::prelude::*;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const PARTIAL_EVERY: Duration = Duration::from_millis(16);
const PARTIAL_COST_FACTOR: u32 = 4;

struct ListJob {
    tab: TabId,
    gen: u64,
    path: FsPath,
    view: ViewSpec,
    case: CaseMode,
    out: Arc<SnapshotSlot>,
    cancel: GenToken,
    watch: Option<FsPath>,
}

struct SizesJob {
    tab: TabId,
    gen: u64,
    dir: FsPath,
    items: Vec<(u32, Vec<u8>)>,
    cancel: GenToken,
}

enum WatchCmd {
    Arm { tab: TabId, gen: u64, path: FsPath, cancel: GenToken, armed: Sender<()> },
    Stop { tab: TabId, gen: u64 },
}

enum ShellJob {
    Open(TabId, FsPath),
    Op(TabId, FileOp),
}

pub struct LocalProvider {
    jobs: Sender<ListJob>,
    sizes: Sender<SizesJob>,
    shell: Sender<ShellJob>,
    watch: Sender<WatchCmd>,
    pending_watch: Mutex<HashMap<TabId, (u64, FsPath)>>,
}

fn spawn(name: String, body: impl FnOnce() + Send + 'static) -> Result<(), PlatformError> {
    std::thread::Builder::new().name(name.clone()).spawn(body).map(drop).map_err(|e| PlatformError::new(-1, format!("cannot start {name}: {e}")))
}

impl LocalProvider {
    pub fn new<F: Fs, W: Watcher, S: Shell + Trash + 'static>(fs: F, threads: usize, events: Sender<ProviderEvent>, wake: Wake) -> Result<Self, PlatformError> {
        let fs = Arc::new(fs);
        let mut watcher = W::new(events.clone(), wake.clone())?;
        let (watch, watch_rx) = unbounded::<WatchCmd>();
        spawn("flit-watch-ctl".into(), move || {
            for cmd in watch_rx {
                match cmd {
                    WatchCmd::Arm { tab, gen, path, cancel, armed } => {
                        if cancel.is_current(gen) {
                            watcher.watch(tab, gen, Some(&path));
                        }
                        let _ = armed.send(());
                    }
                    WatchCmd::Stop { tab, gen } => watcher.watch(tab, gen, None),
                }
            }
        })?;
        let (jobs, rx) = unbounded::<ListJob>();
        for k in 0..threads.max(1) {
            let (rx, fs, wake, watch) = (rx.clone(), fs.clone(), wake.clone(), watch.clone());
            spawn(format!("flit-fs-{k}"), move || {
                for job in rx {
                    run_list(&*fs, job, &watch, &wake);
                }
            })?;
        }
        let (sizes, sizes_rx) = unbounded::<SizesJob>();
        let (sizes_fs, sizes_events, sizes_wake) = (fs.clone(), events.clone(), wake.clone());
        spawn("flit-sizes".into(), move || {
            for job in sizes_rx {
                run_sizes(&*sizes_fs, job, &sizes_events, &sizes_wake);
            }
        })?;
        let (shell, shell_rx) = unbounded::<ShellJob>();
        spawn("flit-shell-ops".into(), move || {
            let mut sh = S::new();
            for job in shell_rx {
                let result = match &mut sh {
                    Ok(sh) => match &job {
                        ShellJob::Open(_, p) => sh.open(p),
                        ShellJob::Op(_, FileOp::Trash(paths)) => sh.trash(paths),
                    },
                    Err(e) => Err(e.clone()),
                };
                let ev = match (job, result) {
                    (ShellJob::Op(tab, _), r) | (ShellJob::Open(tab, _), r @ Err(_)) => ProviderEvent::OpDone { tab, error: r.err().map(|e| e.to_string()) },
                    (ShellJob::Open(..), Ok(())) => continue,
                };
                let _ = events.send(ev);
                wake();
            }
        })?;
        Ok(Self { jobs, sizes, shell, watch, pending_watch: Mutex::default() })
    }
}

impl Provider for LocalProvider {
    fn submit(&self, req: ProviderRequest) {
        let mut pending = self.pending_watch.lock().unwrap_or_else(|e| e.into_inner());
        let _ = match req {
            ProviderRequest::List { tab, gen, path, view, case, out, cancel } => {
                let watch = pending.remove(&tab).filter(|(g, _)| *g == gen).map(|(_, p)| p);
                self.jobs.send(ListJob { tab, gen, path, view, case, out, cancel, watch }).map_err(drop)
            }
            ProviderRequest::DirSizes { tab, gen, dir, items, cancel } => self.sizes.send(SizesJob { tab, gen, dir, items, cancel }).map_err(drop),
            ProviderRequest::Watch { tab, gen, path: Some(path) } => {
                pending.insert(tab, (gen, path));
                Ok(())
            }
            ProviderRequest::Watch { tab, gen, path: None } => {
                pending.remove(&tab);
                self.watch.send(WatchCmd::Stop { tab, gen }).map_err(drop)
            }
            ProviderRequest::Op { tab, op } => self.shell.send(ShellJob::Op(tab, op)).map_err(drop),
            ProviderRequest::Open { tab, path } => self.shell.send(ShellJob::Open(tab, path)).map_err(drop),
        };
    }
}

fn run_list<F: Fs>(fs: &F, job: ListJob, watch: &Sender<WatchCmd>, wake: &Wake) {
    let ListJob { tab, gen, path, view, case, out, cancel, watch: arm } = job;
    if !cancel.is_current(gen) {
        return;
    }
    if let Some(dir) = arm {
        let (armed, wait) = bounded(1);
        if watch.send(WatchCmd::Arm { tab, gen, path: dir, cancel: cancel.clone(), armed }).is_ok() {
            let _ = wait.recv();
        }
    }
    list(fs, gen, &path, view, case, &out, &cancel, wake);
}

fn run_sizes<F: Fs>(fs: &F, job: SizesJob, events: &Sender<ProviderEvent>, wake: &Wake) {
    let SizesJob { tab, gen, dir, items, cancel } = job;
    let one = |(idx, name): &(u32, Vec<u8>)| {
        if !cancel.is_current(gen) {
            return;
        }
        let size = tree_size(fs, &dir.join(name), &cancel, gen);
        if cancel.is_current(gen) {
            let _ = events.send(ProviderEvent::DirSize { tab, gen, idx: *idx, size });
            wake();
        }
    };
    if fs.seek_penalty(&dir) == Some(true) {
        items.iter().for_each(one);
    } else {
        items.par_iter().for_each(one);
    }
}

#[allow(clippy::too_many_arguments)]
fn snapshot(gen: u64, path: &FsPath, listing: Listing, keys: SortKeys, view: ViewSpec, done: bool, error: Option<FsError>, elapsed: Duration) -> Arc<DirSnapshot> {
    let mut order = Vec::new();
    sort::sort(&listing, &keys, view, &mut order);
    Arc::new(DirSnapshot { gen, path: path.clone(), listing, keys, order, view, done, error, elapsed })
}

#[allow(clippy::too_many_arguments)]
fn list<F: Fs>(fs: &F, gen: u64, path: &FsPath, view: ViewSpec, case: CaseMode, out: &SnapshotSlot, cancel: &GenToken, wake: &Wake) {
    if !cancel.is_current(gen) {
        return;
    }
    let start = Instant::now();
    let mut next_partial = start;
    let mut on_batch = |b: &ListingBuilder<'_>| {
        if !cancel.is_current(gen) {
            return false;
        }
        let began = Instant::now();
        if began >= next_partial {
            let snap = snapshot(gen, path, b.listing.clone(), b.keys.clone(), view, false, None, start.elapsed());
            if cancel.is_current(gen) {
                out.publish(snap);
                wake();
            }
            let done = Instant::now();
            next_partial = done + PARTIAL_EVERY.max((done - began) * PARTIAL_COST_FACTOR);
        }
        true
    };
    let mut b = ListingBuilder::new(case, &mut on_batch);
    let result = fs.read_dir(path, &mut b);
    let listing = std::mem::take(&mut b.listing);
    let keys = std::mem::replace(&mut b.keys, SortKeys::new(case));
    drop(b);
    if cancel.is_current(gen) {
        let snap = snapshot(gen, path, listing, keys, view, true, result.err(), start.elapsed());
        if cancel.is_current(gen) {
            out.publish(snap);
            wake();
        }
    }
}

fn tree_size<F: Fs>(fs: &F, root: &FsPath, cancel: &GenToken, gen: u64) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        let mut keep_going = |_: &ListingBuilder<'_>| cancel.is_current(gen);
        let mut b = ListingBuilder::new(CaseMode::Sensitive, &mut keep_going);
        if fs.read_dir(&dir, &mut b).is_err() || !cancel.is_current(gen) {
            continue;
        }
        let l = &b.listing;
        for i in 0..l.len() as u32 {
            let e = &l.entries[i as usize];
            if e.is_dir() {
                if e.attrs & attr::LINK == 0 {
                    stack.push(dir.join(l.name(i)));
                }
            } else {
                total += e.size;
            }
        }
    }
    total
}

pub fn via_unc(distro: &str, req: ProviderRequest) -> Option<ProviderRequest> {
    let unc = |path: &FsPath| Location::Wsl { distro: distro.to_owned(), path: path.clone() }.to_windows();
    Some(match req {
        ProviderRequest::List { tab, gen, path, view, case, out, cancel } => match unc(&path) {
            Some(path) => ProviderRequest::List { tab, gen, path, view, case, out, cancel },
            None => {
                let error = FsError { kind: FsErrorKind::Unavailable, code: 0 };
                out.publish(snapshot(gen, &path, Listing::default(), SortKeys::new(case), view, true, Some(error), Duration::ZERO));
                return None;
            }
        },
        ProviderRequest::Watch { tab, gen, path } => ProviderRequest::Watch { tab, gen, path: path.and_then(|p| unc(&p)) },
        ProviderRequest::DirSizes { tab, gen, dir, items, cancel } => ProviderRequest::DirSizes { tab, gen, dir: unc(&dir)?, items, cancel },
        ProviderRequest::Op { tab, op: FileOp::Trash(paths) } => ProviderRequest::Op { tab, op: FileOp::Trash(paths.iter().map(unc).collect::<Option<_>>()?) },
        ProviderRequest::Open { tab, path } => ProviderRequest::Open { tab, path: unc(&path)? },
    })
}

#[cfg(test)]
#[path = "../test/local.rs"]
mod tests;
