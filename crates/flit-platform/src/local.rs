use crate::{FileOps, Fs, Job, PlatformError, Provider, Shell, Wake, Watcher};
use crossbeam_channel::{bounded, unbounded, Sender};
use flit_core::archive::ArchiveOp;
use flit_core::digest::HashKind;
use flit_core::listing::{attr, DirSnapshot, Listing, ListingBuilder};
use flit_core::provider::{FileOp, GenToken, Launch, OpCtl, OpId, OpReport, SnapshotSlot, Stage};
use flit_core::sort::{self, CaseMode, SortKeys, ViewSpec};
use flit_core::{FsError, FsErrorKind, FsPath, Location, ProviderEvent, ProviderRequest, TabId};
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

const PARTIAL_EVERY: Duration = Duration::from_millis(16);
const PARTIAL_COST_FACTOR: u32 = 4;
const QUEUE_POLL: Duration = Duration::from_millis(50);

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

enum ShellJob {
    Open(FsPath),
    Launch(Launch),
}

struct HashMsg {
    tab: TabId,
    id: OpId,
    file: FsPath,
    kinds: Vec<HashKind>,
    ctl: OpCtl,
}

struct OpMsg {
    tab: TabId,
    id: OpId,
    op: FileOp,
    ctl: OpCtl,
}

#[derive(Clone)]
struct Reply {
    tab: TabId,
    id: OpId,
    events: Sender<ProviderEvent>,
    wake: Wake,
}

impl Reply {
    fn send(&self, ev: ProviderEvent) {
        let _ = self.events.send(ev);
        (self.wake)();
    }

    fn done(&self, (error, report): (Option<String>, Option<OpReport>)) {
        self.send(ProviderEvent::OpDone { tab: self.tab, id: self.id, error, report });
    }

    fn job(&self, ctl: OpCtl) -> Job {
        let (p, c) = (self.clone(), self.clone());
        Job::new(ctl, Box::new(move |progress| p.send(ProviderEvent::OpProgress { tab: p.tab, id: p.id, progress })), Box::new(move |conflict| c.send(ProviderEvent::OpConflict { tab: c.tab, id: c.id, conflict })))
    }
}

#[derive(Default)]
struct Gate {
    busy: Mutex<HashSet<Vec<u8>>>,
    freed: Condvar,
}

struct Turn<'a> {
    gate: &'a Gate,
    key: Vec<u8>,
}

impl Drop for Turn<'_> {
    fn drop(&mut self) {
        self.gate.busy.lock().unwrap_or_else(|e| e.into_inner()).remove(&self.key);
        self.gate.freed.notify_all();
    }
}

impl Gate {
    fn enter(&self, key: Vec<u8>, job: &Job) -> Option<Turn<'_>> {
        let mut busy = self.busy.lock().unwrap_or_else(|e| e.into_inner());
        while busy.contains(&key) {
            if job.cancelled() {
                return None;
            }
            if job.progress().stage != Stage::Queued {
                job.update(|p| p.stage = Stage::Queued);
            }
            busy = self.freed.wait_timeout(busy, QUEUE_POLL).unwrap_or_else(|e| e.into_inner()).0;
        }
        busy.insert(key.clone());
        Some(Turn { gate: self, key })
    }
}

fn volume(p: &FsPath) -> Vec<u8> {
    p.root_bytes().to_ascii_lowercase()
}

fn transfer_volume(op: &FileOp) -> Option<Vec<u8>> {
    let crosses = |a: &FsPath, b: &FsPath| volume(a) != volume(b);
    match op {
        FileOp::Copy { dest, .. } => Some(volume(dest)),
        FileOp::Move { items, dest } if items.iter().any(|i| crosses(i, dest)) => Some(volume(dest)),
        FileOp::MoveTo(pairs) if pairs.iter().any(|(a, b)| crosses(a, b)) => pairs.first().map(|(_, b)| volume(b)),
        FileOp::Archive(ArchiveOp::Compress { archive, .. }) => Some(volume(archive)),
        FileOp::Archive(ArchiveOp::Extract { archives, dest, .. }) => dest.as_ref().or(archives.first()).map(volume),
        _ => None,
    }
}

fn quick(op: &FileOp) -> bool {
    matches!(op, FileOp::Rename { .. } | FileOp::NewFolder { .. } | FileOp::NewFile { .. } | FileOp::NewStream { .. } | FileOp::Link { .. } | FileOp::Restore(_))
}

enum WatchCmd {
    Arm { tab: TabId, gen: u64, path: FsPath, cancel: GenToken, armed: Sender<()> },
    Stop { tab: TabId, gen: u64 },
}

pub struct LocalProvider {
    jobs: Sender<ListJob>,
    sizes: Sender<SizesJob>,
    ops: Sender<OpMsg>,
    shell: Sender<(TabId, ShellJob)>,
    hashes: Sender<HashMsg>,
    watch: Sender<WatchCmd>,
    pending_watch: Mutex<HashMap<TabId, (u64, FsPath)>>,
    busy: Arc<AtomicUsize>,
}

fn spawn(name: String, body: impl FnOnce() + Send + 'static) -> Result<(), PlatformError> {
    std::thread::Builder::new().name(name.clone()).spawn(body).map(drop).map_err(|e| PlatformError::new(-1, format!("cannot start {name}: {e}")))
}

impl LocalProvider {
    pub fn new<F: Fs, W: Watcher, S: Shell + FileOps + 'static>(fs: F, threads: usize, events: Sender<ProviderEvent>, wake: Wake) -> Result<Self, PlatformError> {
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
        let (shell, shell_rx) = unbounded::<(TabId, ShellJob)>();
        let (open_events, open_wake) = (events.clone(), wake.clone());
        spawn("flit-shell-open".into(), move || {
            let mut sh = S::new();
            for (tab, job) in shell_rx {
                let event = match sh.as_mut().map_err(|e| e.clone()).and_then(|sh| run_shell(sh, job)) {
                    Ok(None) => continue,
                    Ok(Some(path)) => ProviderEvent::Navigate { tab, path },
                    Err(e) => ProviderEvent::OpDone { tab, id: 0, error: Some(e.to_string()), report: None },
                };
                let _ = open_events.send(event);
                open_wake();
            }
        })?;
        let (hashes, hash_rx) = unbounded::<HashMsg>();
        let (hash_fs, hash_events, hash_wake) = (fs.clone(), events.clone(), wake.clone());
        spawn("flit-hash".into(), move || {
            for HashMsg { tab, id, file, kinds, ctl } in hash_rx {
                let (fs, reply) = (hash_fs.clone(), Reply { tab, id, events: hash_events.clone(), wake: hash_wake.clone() });
                let failed = reply.clone();
                let started = spawn(format!("flit-hash-{id}"), move || {
                    let job = reply.job(ctl);
                    let result = fs.hash(&file, &kinds, &job).map_err(|e| if job.cancelled() { CANCELLED.to_owned() } else { e.message });
                    reply.send(ProviderEvent::Hashed { tab, id, result });
                });
                if let Err(e) = started {
                    failed.send(ProviderEvent::Hashed { tab, id, result: Err(e.message) });
                }
            }
        })?;
        let (ops, ops_rx) = unbounded::<OpMsg>();
        let busy = Arc::new(AtomicUsize::new(0));
        let (gate, running) = (Arc::new(Gate::default()), busy.clone());
        spawn("flit-shell-ops".into(), move || {
            let mut sh = S::new();
            for OpMsg { tab, id, op, ctl } in ops_rx {
                let reply = Reply { tab, id, events: events.clone(), wake: wake.clone() };
                let sh = match &mut sh {
                    Ok(sh) => sh,
                    Err(e) => {
                        reply.done((Some(e.to_string()), None));
                        continue;
                    }
                };
                let (op, cut) = match resolve_paste(sh, op) {
                    Ok(resolved) => resolved,
                    Err(e) => {
                        reply.done((Some(e), None));
                        continue;
                    }
                };
                let job = reply.job(ctl);
                if quick(&op) {
                    reply.done(perform(sh, op, cut, &job));
                    continue;
                }
                running.fetch_add(1, Ordering::AcqRel);
                let (gate, running, failed) = (gate.clone(), running.clone(), reply.clone());
                let started = spawn(format!("flit-op-{id}"), move || {
                    let turn = transfer_volume(&op).map(|key| gate.enter(key, &job));
                    let result = match (turn.as_ref().is_some_and(Option::is_none), S::new()) {
                        (true, _) => (Some(CANCELLED.into()), Some(OpReport { op, items: Vec::new() })),
                        (false, Ok(mut sh)) => perform(&mut sh, op, cut, &job),
                        (false, Err(e)) => (Some(e.to_string()), None),
                    };
                    drop(turn);
                    reply.done(result);
                    running.fetch_sub(1, Ordering::AcqRel);
                });
                if let Err(e) = started {
                    failed.done((Some(e.to_string()), None));
                }
            }
        })?;
        Ok(Self { jobs, sizes, ops, shell, hashes, watch, pending_watch: Mutex::default(), busy })
    }

    pub fn wait_idle(&self, limit: Duration) {
        let end = Instant::now() + limit;
        while self.busy.load(Ordering::Acquire) > 0 && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

pub const CANCELLED: &str = "The operation was cancelled";

fn is_shortcut(p: &FsPath) -> bool {
    let b = p.as_bytes();
    b.len() > 4 && b[b.len() - 4..].eq_ignore_ascii_case(b".lnk")
}

fn run_shell<S: Shell>(sh: &mut S, job: ShellJob) -> Result<Option<FsPath>, PlatformError> {
    match job {
        ShellJob::Open(p) if is_shortcut(&p) => match sh.shortcut_folder(&p) {
            Some(dir) => Ok(Some(dir)),
            None => sh.open(&p).map(|()| None),
        },
        ShellJob::Open(p) => sh.open(&p).map(|()| None),
        ShellJob::Launch(l) => sh.launch(&l).map(|()| None),
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
            ProviderRequest::Op { tab, id, op, ctl } => self.ops.send(OpMsg { tab, id, op, ctl }).map_err(drop),
            ProviderRequest::Open { tab, path } => self.shell.send((tab, ShellJob::Open(path))).map_err(drop),
            ProviderRequest::Launch { tab, launch } => self.shell.send((tab, ShellJob::Launch(launch))).map_err(drop),
            ProviderRequest::Hash { tab, id, file, kinds, ctl } => self.hashes.send(HashMsg { tab, id, file, kinds, ctl }).map_err(drop),
        };
    }
}

fn same_dir(item: &FsPath, dir: &FsPath) -> bool {
    item.parent().is_some_and(|p| p.as_bytes().eq_ignore_ascii_case(dir.as_bytes()))
}

fn resolve_paste<S: FileOps>(sh: &mut S, op: FileOp) -> Result<(FileOp, bool), String> {
    Ok(match op {
        FileOp::Paste { dest } => match sh.clipboard_files().map_err(|e| e.to_string())? {
            (items, true) => (FileOp::Move { items: items.into_iter().filter(|i| !same_dir(i, &dest)).collect(), dest }, true),
            (items, false) => (FileOp::Copy { items, dest }, false),
        },
        op => (op, false),
    })
}

fn perform<S: FileOps>(sh: &mut S, op: FileOp, cut: bool, job: &Job) -> (Option<String>, Option<OpReport>) {
    if matches!(&op, FileOp::Move { items, .. } if items.is_empty()) {
        return (None, Some(OpReport { op, items: Vec::new() }));
    }
    let out = sh.file_op(&op, job);
    if cut && out.error.is_none() {
        sh.clear_clipboard();
    }
    (out.error.map(|e| e.to_string()), Some(OpReport { op, items: out.items }))
}

#[cfg(test)]
fn run_op<S: FileOps>(sh: &mut S, op: FileOp) -> (Option<String>, Option<OpReport>) {
    match resolve_paste(sh, op) {
        Ok((op, cut)) => perform(sh, op, cut, &Job::detached(flit_core::provider::Resolve::Skip)),
        Err(e) => (Some(e), None),
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
    let keep_going = || cancel.is_current(gen);
    fs.dir_sizes(&dir, &items, &keep_going, &|idx, size| {
        if keep_going() {
            let _ = events.send(ProviderEvent::DirSize { tab, gen, idx, size });
            wake();
        }
    });
}

pub(crate) fn dir_sizes<F: Fs + ?Sized>(fs: &F, dir: &FsPath, items: &[(u32, Vec<u8>)], keep_going: &(dyn Fn() -> bool + Sync), report: &(dyn Fn(u32, u64) + Sync)) {
    let one = |(idx, name): &(u32, Vec<u8>)| {
        if keep_going() {
            report(*idx, tree_size(fs, &dir.join(name), keep_going));
        }
    };
    if fs.seek_penalty(dir) == Some(true) {
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

fn tree_size<F: Fs + ?Sized>(fs: &F, root: &FsPath, keep_going: &(dyn Fn() -> bool + Sync)) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        let mut each_batch = |_: &ListingBuilder<'_>| keep_going();
        let mut b = ListingBuilder::new(CaseMode::Sensitive, &mut each_batch);
        if fs.read_dir(&dir, &mut b).is_err() || !keep_going() {
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
        ProviderRequest::Op { tab, id, op, ctl } => ProviderRequest::Op { tab, id, op: op.map_paths(unc)?, ctl },
        ProviderRequest::Open { tab, path } => ProviderRequest::Open { tab, path: unc(&path)? },
        ProviderRequest::Launch { tab, launch } => ProviderRequest::Launch { tab, launch: launch.map_paths(unc)? },
        ProviderRequest::Hash { tab, id, file, kinds, ctl } => ProviderRequest::Hash { tab, id, file: unc(&file)?, kinds, ctl },
    })
}

#[cfg(test)]
#[path = "../test/local.rs"]
mod tests;
