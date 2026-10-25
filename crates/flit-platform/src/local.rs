use crate::{Fs, PlatformError, Provider, Shell, Trash, Wake, Watcher};
use crossbeam_channel::{unbounded, Sender};
use flit_core::listing::{attr, DirSnapshot, Listing, ListingBuilder};
use flit_core::provider::{FileOp, GenToken, SnapshotSlot};
use flit_core::sort::{self, CaseMode, SortKeys, ViewSpec};
use flit_core::{FsError, FsErrorKind, FsPath, Location, ProviderEvent, ProviderRequest, TabId};
use rayon::prelude::*;
use std::marker::PhantomData;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const PARTIAL_EVERY: Duration = Duration::from_millis(16);

enum Job {
    List { gen: u64, path: FsPath, view: ViewSpec, case: CaseMode, out: Arc<SnapshotSlot>, cancel: GenToken },
    DirSizes { tab: TabId, gen: u64, dir: FsPath, items: Vec<(u32, Vec<u8>)>, cancel: GenToken },
}

enum ShellJob {
    Open(FsPath),
    Op(u64, FileOp),
}

pub struct LocalProvider<F: Fs, W: Watcher, S> {
    jobs: Sender<Job>,
    shell: Sender<ShellJob>,
    watcher: Mutex<W>,
    _types: PhantomData<fn() -> (F, S)>,
}

impl<F: Fs, W: Watcher, S: Shell + Trash + 'static> LocalProvider<F, W, S> {
    pub fn new(fs: F, threads: usize, events: Sender<ProviderEvent>, wake: Wake) -> Result<Self, PlatformError> {
        let fs = Arc::new(fs);
        let (jobs, rx) = unbounded::<Job>();
        for k in 0..threads.max(1) {
            let (rx, fs, events, wake) = (rx.clone(), fs.clone(), events.clone(), wake.clone());
            std::thread::Builder::new()
                .name(format!("flit-fs-{k}"))
                .spawn(move || {
                    for job in rx {
                        run_job(&*fs, job, &events, &wake);
                    }
                })
                .map_err(|e| PlatformError::new(-1, format!("cannot start file system worker: {e}")))?;
        }
        let (shell, shell_rx) = unbounded::<ShellJob>();
        let (shell_events, shell_wake) = (events.clone(), wake.clone());
        std::thread::Builder::new()
            .name("flit-shell-ops".into())
            .spawn(move || {
                let mut sh = S::new();
                for job in shell_rx {
                    let result = match &mut sh {
                        Ok(sh) => match &job {
                            ShellJob::Open(p) => sh.open(p),
                            ShellJob::Op(_, FileOp::Trash(paths)) => sh.trash(paths),
                        },
                        Err(e) => Err(e.clone()),
                    };
                    let ev = match (job, result) {
                        (ShellJob::Op(id, _), r) => ProviderEvent::OpDone { id, error: r.err().map(|e| e.to_string()) },
                        (ShellJob::Open(_), Err(e)) => ProviderEvent::Error { message: e.to_string() },
                        (ShellJob::Open(_), Ok(())) => continue,
                    };
                    let _ = shell_events.send(ev);
                    shell_wake();
                }
            })
            .map_err(|e| PlatformError::new(-1, format!("cannot start shell worker: {e}")))?;
        let watcher = Mutex::new(W::new(events, wake)?);
        Ok(Self { jobs, shell, watcher, _types: PhantomData })
    }
}

impl<F: Fs, W: Watcher, S: Shell + Trash + 'static> Provider for LocalProvider<F, W, S> {
    fn submit(&self, req: ProviderRequest) {
        let _ = match req {
            ProviderRequest::List { gen, path, view, case, out, cancel, .. } => self.jobs.send(Job::List { gen, path, view, case, out, cancel }).map_err(drop),
            ProviderRequest::DirSizes { tab, gen, dir, items, cancel } => self.jobs.send(Job::DirSizes { tab, gen, dir, items, cancel }).map_err(drop),
            ProviderRequest::Watch { tab, gen, path } => {
                if let Ok(mut w) = self.watcher.lock() {
                    w.watch(tab, gen, path.as_ref());
                }
                Ok(())
            }
            ProviderRequest::Op { id, op } => self.shell.send(ShellJob::Op(id, op)).map_err(drop),
            ProviderRequest::Open { path } => self.shell.send(ShellJob::Open(path)).map_err(drop),
        };
    }
}

fn run_job<F: Fs>(fs: &F, job: Job, events: &Sender<ProviderEvent>, wake: &Wake) {
    match job {
        Job::List { gen, path, view, case, out, cancel } => list(fs, gen, &path, view, case, &out, &cancel, wake),
        Job::DirSizes { tab, gen, dir, items, cancel } => {
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
    let mut last: Option<Instant> = None;
    let mut on_batch = |b: &ListingBuilder<'_>| {
        if !cancel.is_current(gen) {
            return false;
        }
        let now = Instant::now();
        if last.is_none_or(|t| now - t >= PARTIAL_EVERY) {
            last = Some(now);
            out.publish(snapshot(gen, path, b.listing.clone(), b.keys.clone(), view, false, None, start.elapsed()));
            wake();
        }
        true
    };
    let mut b = ListingBuilder::new(case, &mut on_batch);
    let result = fs.read_dir(path, &mut b);
    let listing = std::mem::take(&mut b.listing);
    let keys = std::mem::replace(&mut b.keys, SortKeys::new(case));
    drop(b);
    if cancel.is_current(gen) {
        out.publish(snapshot(gen, path, listing, keys, view, true, result.err(), start.elapsed()));
        wake();
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
        ProviderRequest::Op { id, op: FileOp::Trash(paths) } => ProviderRequest::Op { id, op: FileOp::Trash(paths.iter().map(unc).collect::<Option<_>>()?) },
        ProviderRequest::Open { path } => ProviderRequest::Open { path: unc(&path)? },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use flit_core::listing::Meta;
    use flit_core::ui::{Bitmap, IconRequest};
    use std::collections::HashMap;

    struct MemFs(HashMap<Vec<u8>, Vec<(&'static str, bool, u64)>>);

    impl Fs for MemFs {
        fn read_dir(&self, dir: &FsPath, out: &mut ListingBuilder<'_>) -> Result<(), FsError> {
            let items = self.0.get(dir.as_bytes()).ok_or(FsError { kind: flit_core::FsErrorKind::NotFound, code: 2 })?;
            for chunk in items.chunks(2) {
                for &(n, d, s) in chunk {
                    out.push(n.as_bytes(), Meta { size: s, attrs: if d { attr::DIR } else { 0 }, ..Default::default() });
                }
                if !out.batch_done() {
                    break;
                }
            }
            Ok(())
        }
        fn seek_penalty(&self, _: &FsPath) -> Option<bool> {
            Some(false)
        }
    }

    struct NoWatch;
    impl Watcher for NoWatch {
        fn new(_: Sender<ProviderEvent>, _: Wake) -> Result<Self, PlatformError> {
            Ok(NoWatch)
        }
        fn watch(&mut self, _: TabId, _: u64, _: Option<&FsPath>) {}
    }

    struct NoShell;
    impl Shell for NoShell {
        fn new() -> Result<Self, PlatformError> {
            Ok(NoShell)
        }
        fn icon(&mut self, _: &IconRequest) -> Option<Bitmap> {
            None
        }
        fn type_name(&mut self, _: &[u8]) -> Option<String> {
            None
        }
        fn open(&mut self, _: &FsPath) -> Result<(), PlatformError> {
            Err(PlatformError::new(5, "denied"))
        }
    }
    impl Trash for NoShell {
        fn trash(&mut self, _: &[FsPath]) -> Result<(), PlatformError> {
            Ok(())
        }
    }

    fn p(s: &str) -> FsPath {
        FsPath::parse(s).unwrap()
    }

    fn fs() -> MemFs {
        let mut m = HashMap::new();
        m.insert(p(r"C:\r").as_bytes().to_vec(), vec![("b.txt", false, 5), ("sub", true, 0), ("a.txt", false, 7), ("c", false, 1), ("d", false, 1)]);
        m.insert(p(r"C:\r\sub").as_bytes().to_vec(), vec![("x", false, 100), ("deep", true, 0)]);
        m.insert(p(r"C:\r\sub\deep").as_bytes().to_vec(), vec![("y", false, 20)]);
        MemFs(m)
    }

    #[test]
    fn list_streams_sorted_snapshots_and_sizes() {
        let (tx, rx) = unbounded();
        let provider: LocalProvider<MemFs, NoWatch, NoShell> = LocalProvider::new(fs(), 2, tx, Arc::new(|| {})).unwrap();
        let slot = Arc::new(SnapshotSlot::default());
        let cancel = GenToken::default();
        cancel.set(3);
        provider.submit(ProviderRequest::List { tab: 1, gen: 3, path: p(r"C:\r"), view: ViewSpec::default(), case: CaseMode::Insensitive, out: slot.clone(), cancel: cancel.clone() });
        let snap = loop {
            if let Some(s) = slot.take().filter(|s| s.done) {
                break s;
            }
            std::thread::yield_now();
        };
        let names: Vec<_> = snap.order.iter().map(|&i| String::from_utf8_lossy(snap.listing.name(i)).into_owned()).collect();
        assert_eq!(names, ["sub", "a.txt", "b.txt", "c", "d"]);
        assert_eq!(snap.gen, 3);
        provider.submit(ProviderRequest::DirSizes { tab: 1, gen: 3, dir: p(r"C:\r"), items: vec![(1, b"sub".to_vec())], cancel: cancel.clone() });
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), ProviderEvent::DirSize { tab: 1, gen: 3, idx: 1, size: 120 });
        provider.submit(ProviderRequest::Open { path: p(r"C:\r\a.txt") });
        assert!(matches!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), ProviderEvent::Error { .. }));
        provider.submit(ProviderRequest::Op { id: 9, op: FileOp::Trash(vec![p(r"C:\r\c")]) });
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), ProviderEvent::OpDone { id: 9, error: None });
    }

    #[test]
    fn cancelled_list_publishes_nothing() {
        let slot = SnapshotSlot::default();
        let cancel = GenToken::default();
        cancel.set(2);
        list(&fs(), 1, &p(r"C:\r"), ViewSpec::default(), CaseMode::Insensitive, &slot, &cancel, &(Arc::new(|| {}) as Wake));
        assert!(slot.take().is_none());
    }

    #[test]
    fn wsl_requests_map_to_unc() {
        let req = via_unc("Ubuntu", ProviderRequest::Open { path: FsPath::parse("/home/a:b").unwrap() }).unwrap();
        let ProviderRequest::Open { path } = req else { panic!() };
        assert_eq!(path.display(), "\\\\wsl.localhost\\Ubuntu\\home\\a\u{F03A}b");
    }

    #[test]
    fn unmappable_wsl_paths_never_reach_win32() {
        let bad = FsPath::parse_bytes(b"/tmp/\xFF").unwrap();
        assert!(via_unc("Ubuntu", ProviderRequest::Op { id: 1, op: FileOp::Trash(vec![bad.clone()]) }).is_none());
        let out = Arc::new(SnapshotSlot::default());
        let req = ProviderRequest::List { tab: 1, gen: 3, path: bad, view: ViewSpec::default(), case: CaseMode::Sensitive, out: out.clone(), cancel: GenToken::default() };
        assert!(via_unc("Ubuntu", req).is_none());
        let snap = out.take().unwrap();
        assert!(snap.done && snap.error.is_some() && snap.gen == 3);
    }
}
