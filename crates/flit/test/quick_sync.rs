use super::*;
use flit_core::quick_sync::Snapshot;
use flit_core::FsPath;
use std::sync::{Arc, Mutex};

fn p(s: &str) -> FsPath {
    FsPath::parse(s).unwrap()
}

fn l(s: &str) -> Location {
    Location::from_local(p(s))
}

#[derive(Default)]
struct State {
    pinned: Vec<FsPath>,
    stamp: u64,
    calls: usize,
}

#[derive(Clone, Default)]
struct Shared(Arc<Mutex<State>>);

impl QuickAccess for Shared {
    fn pinned(&mut self) -> Option<Snapshot> {
        let mut s = self.0.lock().unwrap();
        s.calls += 1;
        Some(Snapshot { pinned: s.pinned.clone(), complete: true })
    }
    fn stamp(&mut self) -> Option<(u64, u128)> {
        let mut s = self.0.lock().unwrap();
        s.calls += 1;
        Some((s.stamp, 0))
    }
    fn set_pinned(&mut self, folder: &FsPath, pin: bool) -> bool {
        let mut s = self.0.lock().unwrap();
        s.calls += 1;
        s.pinned.retain(|x| !x.same(folder));
        if pin {
            s.pinned.push(folder.clone());
        }
        s.stamp += 1;
        true
    }
    fn is_dir(&mut self, _: &FsPath) -> bool {
        true
    }
}

fn worker() -> (QuickSyncWorker, Receiver<Job>) {
    let (tx, rx) = unbounded();
    (QuickSyncWorker { jobs: Some(tx), changes: None, start: None, on: None }, rx)
}

fn kinds(rx: &Receiver<Job>) -> Vec<String> {
    rx.try_iter().map(|j| format!("{j:?}")).collect()
}

#[test]
fn the_setting_already_on_at_start_resumes_and_turning_it_on_later_merges() {
    let (mut w, rx) = worker();
    w.follow(true, &[l(r"C:\Work")]);
    w.follow(true, &[l(r"C:\Work")]);
    w.follow(false, &[]);
    w.follow(true, &[l(r"C:\Work")]);
    let jobs = kinds(&rx);
    assert_eq!(jobs.len(), 3, "{jobs:?}");
    assert!(jobs[0].starts_with("Resume") && jobs[1].starts_with("Disable") && jobs[2].starts_with("Enable"), "{jobs:?}");
}

#[test]
fn with_the_setting_off_nothing_reaches_quick_access() {
    let (mut w, rx) = worker();
    w.follow(false, &[l(r"C:\Work")]);
    w.pinned(&[], &[l(r"C:\Work")]);
    assert!(kinds(&rx).is_empty());
    let (mut w, rx) = worker();
    w.follow(true, &[]);
    let _ = kinds(&rx);
    w.pinned(&[l(r"C:\Work")], &[l(r"C:\Work")]);
    assert!(kinds(&rx).is_empty(), "an unchanged list is not sent");
}

#[test]
fn the_background_thread_merges_pins_and_brings_explorer_changes() {
    let shared = Shared::default();
    shared.0.lock().unwrap().pinned = vec![p(r"E:\Music")];
    let (jobs, rx) = unbounded();
    let (tx, changes) = unbounded();
    let quick = shared.clone();
    let thread = std::thread::spawn(move || serve(quick, rx, tx, Arc::new(|| {}), Duration::from_millis(10)));
    jobs.send(Job::Enable(vec![l(r"C:\Work")])).unwrap();
    let merged = changes.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(merged.added, [p(r"E:\Music")]);
    jobs.send(Job::Pinned(vec![l(r"C:\Work"), l(r"E:\Music")], vec![l(r"C:\Work"), l(r"E:\Music"), l(r"D:\Photos")])).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    assert!(changes.try_recv().is_err(), "Flit's own pin does not come back");
    {
        let mut s = shared.0.lock().unwrap();
        s.pinned.retain(|x| !x.same(&p(r"C:\Work")));
        s.stamp += 1;
    }
    let change = changes.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(change.removed, [p(r"C:\Work")]);
    jobs.send(Job::Disable).unwrap();
    std::thread::sleep(Duration::from_millis(50));
    let calls = shared.0.lock().unwrap().calls;
    shared.0.lock().unwrap().stamp += 1;
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(shared.0.lock().unwrap().calls, calls, "a disabled sync stops polling");
    drop(jobs);
    thread.join().unwrap();
}
