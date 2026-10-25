use super::*;
use crate::temp::Temp;
use crossbeam_channel::{unbounded, Receiver};
use flit_core::sort::CaseMode;
use std::os::unix::ffi::OsStrExt;
use std::sync::Arc;
use std::time::Instant;

fn fs_path(p: &Path) -> FsPath {
    FsPath::parse_exact(p.as_os_str().as_bytes()).unwrap()
}

fn watcher() -> (LinuxGuiWatcher, Receiver<ProviderEvent>) {
    let (tx, rx) = unbounded();
    (LinuxGuiWatcher::new(tx, Arc::new(|| {})).unwrap(), rx)
}

#[test]
fn read_dir_lists_folders_with_hidden_dotfiles() {
    let t = Temp::new("read");
    t.file("visible", 3);
    t.file(".dot", 1);
    std::fs::create_dir(t.path().join("sub")).unwrap();
    let mut go = |_: &ListingBuilder<'_>| true;
    let mut b = ListingBuilder::new(CaseMode::Sensitive, &mut go);
    LinuxGuiFs.read_dir(&fs_path(t.path()), &mut b).unwrap();
    let l = b.listing;
    let attrs = |n: &[u8]| (0..l.len() as u32).find(|&i| l.name(i) == n).map(|i| l.entries[i as usize].attrs).unwrap();
    assert_ne!(attrs(b".dot") & flit_core::attr::HIDDEN, 0);
    assert_eq!(attrs(b"visible") & flit_core::attr::HIDDEN, 0);
    assert_ne!(attrs(b"sub") & flit_core::attr::DIR, 0);
}

#[test]
fn volume_key_follows_the_device_of_the_nearest_existing_folder() {
    let t = Temp::new("volume");
    let fs = LinuxGuiFs;
    let here = fs.volume_key(&fs_path(t.path()));
    assert!(here.starts_with(b"dev:"));
    assert_eq!(fs.volume_key(&fs_path(&t.path().join("not/yet/there"))), here);
    assert_eq!(fs.volume_key(&FsPath::parse("/tmp").unwrap()), here);
    assert_ne!(fs.volume_key(&FsPath::parse("/proc").unwrap()), here);
    assert_eq!(fs.volume_key(&FsPath::computer()), FsPath::computer().root_bytes());
}

#[test]
fn changes_are_coalesced_into_few_events() {
    let t = Temp::new("coalesce");
    let (mut w, rx) = watcher();
    w.watch(7, 1, Some(&fs_path(t.path())));
    for k in 0..200 {
        t.file(&format!("f{k}"), 1);
    }
    let mut names = std::collections::HashSet::new();
    let mut events = 0;
    while let Ok(e) = rx.recv_timeout(Duration::from_millis(300)) {
        if let ProviderEvent::Changes { tab: 7, gen: 1, changes } = e {
            events += 1;
            names.extend(changes.into_iter().map(|c| c.name));
        }
    }
    assert_eq!(names.len(), 200);
    assert!(events <= 20, "{events} events for 200 files");
}

#[test]
fn switching_folders_stops_the_old_watch() {
    let t = Temp::new("switch");
    std::fs::create_dir(t.path().join("a")).unwrap();
    std::fs::create_dir(t.path().join("b")).unwrap();
    let (mut w, rx) = watcher();
    w.watch(3, 1, Some(&fs_path(&t.path().join("a"))));
    w.watch(3, 2, Some(&fs_path(&t.path().join("b"))));
    t.file("a/x", 1);
    t.file("b/y", 1);
    let gens: Vec<u64> = std::iter::from_fn(|| rx.recv_timeout(Duration::from_millis(200)).ok())
        .filter_map(|e| match e {
            ProviderEvent::Changes { tab: 3, gen, .. } => Some(gen),
            _ => None,
        })
        .collect();
    assert!(!gens.is_empty() && gens.iter().all(|&g| g == 2), "{gens:?}");
    w.watch(3, 2, None);
    t.file("b/z", 1);
    assert!(rx.recv_timeout(Duration::from_millis(150)).is_err());
}

#[test]
fn failed_watch_reports_status_and_rescans_after_own_operations() {
    let (w, rx) = watcher();
    w.blind(41, 9, &io::Error::from_raw_os_error(libc::ENOSPC));
    match rx.recv_timeout(Duration::from_millis(100)).unwrap() {
        ProviderEvent::Status { tab: 41, gen: 9, text } => assert_eq!(text, WATCH_LIMIT),
        other => panic!("{other:?}"),
    }
    let (mut w2, rx2) = watcher();
    w2.watch(42, 3, Some(&FsPath::parse("/tmp/flit-gui-test-missing-folder").unwrap()));
    assert!(matches!(rx2.recv_timeout(Duration::from_millis(100)).unwrap(), ProviderEvent::Status { tab: 42, gen: 3, .. }));
    rescan_blind();
    assert!(matches!(rx.recv_timeout(Duration::from_millis(100)).unwrap(), ProviderEvent::Rescan { tab: 41, gen: 9 }));
    assert!(matches!(rx2.recv_timeout(Duration::from_millis(100)).unwrap(), ProviderEvent::Rescan { tab: 42, gen: 3 }));
    rescan_blind();
    assert!(rx.try_recv().is_err() && rx2.try_recv().is_err());
}

#[test]
fn measure_listing_and_change_latency() {
    let t = Temp::new("measure");
    for k in 0..10_000 {
        std::fs::write(t.path().join(format!("file-{k:05}.txt")), b"").unwrap();
    }
    let dir = fs_path(t.path());
    let mut first = Vec::new();
    let mut full = Vec::new();
    for _ in 0..7 {
        let start = Instant::now();
        let mut first_at = None;
        let mut go = |_: &ListingBuilder<'_>| {
            first_at.get_or_insert_with(|| start.elapsed());
            true
        };
        let mut b = ListingBuilder::new(CaseMode::Sensitive, &mut go);
        LinuxGuiFs.read_dir(&dir, &mut b).unwrap();
        assert_eq!(b.listing.len(), 10_000);
        drop(b);
        full.push(start.elapsed());
        first.push(first_at.unwrap());
    }
    first.sort();
    full.sort();
    let (mut w, rx) = watcher();
    w.watch(1, 1, Some(&dir));
    let mut latency = Vec::new();
    for k in 0..15 {
        let start = Instant::now();
        std::fs::write(t.path().join(format!("new-{k}")), b"x").unwrap();
        loop {
            if let ProviderEvent::Changes { .. } = rx.recv_timeout(Duration::from_secs(2)).unwrap() {
                break;
            }
        }
        latency.push(start.elapsed());
        std::thread::sleep(Duration::from_millis(30));
    }
    latency.sort();
    eprintln!("measure: 10k listing first batch median {:?}, full median {:?}; external change to event median {:?}, max {:?}", first[3], full[3], latency[7], latency[14]);
}
