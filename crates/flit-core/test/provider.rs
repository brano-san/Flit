use super::*;
use crate::listing::{attr, Listing};
use crate::sort::{CaseMode, SortKeys};
use std::time::Duration;

#[test]
fn legacy_link_operation_bytes_remain_compatible_and_typed_links_roundtrip() {
    let dest = FsPath::parse(r"C:\out").unwrap();
    let source = FsPath::parse(r"C:\a").unwrap();
    for (op, bytes) in [
        (FileOp::Link { items: vec![source.clone()], dest: dest.clone() }, vec![3, 1, 4, 67, 58, 92, 97, 6, 67, 58, 92, 111, 117, 116]),
        (FileOp::PasteLink { dest: dest.clone() }, vec![19, 6, 67, 58, 92, 111, 117, 116]),
        (FileOp::GrantAccess(dest.clone()), vec![24, 6, 67, 58, 92, 111, 117, 116]),
    ] {
        assert_eq!(postcard::to_stdvec(&op).unwrap(), bytes);
        assert_eq!(postcard::from_bytes::<FileOp>(&bytes).unwrap(), op);
    }
    for kind in [LinkKind::Shortcut, LinkKind::Symbolic, LinkKind::Junction, LinkKind::Hard] {
        for op in [FileOp::LinkAs { items: vec![source.clone()], dest: dest.clone(), kind }, FileOp::PasteLinkAs { dest: dest.clone(), kind }] {
            let bytes = postcard::to_stdvec(&op).unwrap();
            assert_eq!(postcard::from_bytes::<FileOp>(&bytes).unwrap(), op);
            assert_eq!(op.map_paths(|p| Some(p.clone())), Some(op));
        }
    }
}

fn snap(gen: u64) -> Arc<DirSnapshot> {
    let path = FsPath::parse(r"C:\x").unwrap();
    Arc::new(DirSnapshot { gen, path, listing: Listing::default(), keys: SortKeys::new(CaseMode::Insensitive), order: Vec::new(), view: ViewSpec::default(), done: true, error: None, elapsed: Duration::ZERO })
}

#[test]
fn stale_generation_never_replaces_newer_snapshot() {
    let slot = SnapshotSlot::default();
    slot.publish(snap(5));
    slot.publish(snap(4));
    assert_eq!(slot.take().unwrap().gen, 5);
    slot.publish(snap(6));
    slot.publish(snap(7));
    assert_eq!(slot.take().unwrap().gen, 7);
    assert!(slot.take().is_none());
}

#[test]
fn failures_wait_for_an_answer_and_skip_all_stops_asking_except_when_the_disk_is_full() {
    let ctl = OpCtl::new(Some(Resolve::Replace));
    let asking = |ctl: &OpCtl, fatal: bool, choice: Recover, all: bool| {
        let (c, answer) = (ctl.clone(), ctl.clone());
        std::thread::spawn(move || c.recover(fatal, move || answer.answer_failure(choice, all))).join().unwrap()
    };
    assert_eq!(asking(&ctl, false, Recover::Retry, true), Some(Recover::Retry));
    assert!(!ctl.skips_failures(), "retrying never becomes a policy");
    assert_eq!(asking(&ctl, false, Recover::Skip, false), Some(Recover::Skip));
    assert!(!ctl.skips_failures());
    assert_eq!(asking(&ctl, false, Recover::Skip, true), Some(Recover::Skip));
    assert!(ctl.skips_failures() && ctl.policy() == Some(Resolve::Replace), "skip all leaves the conflict policy alone");
    assert_eq!(ctl.recover(false, || panic!("asked after skip all")), Some(Recover::Skip));
    assert_eq!(asking(&ctl, true, Recover::Retry, false), Some(Recover::Retry), "a full disk is asked even after skip all");
    let (c, cancel) = (ctl.clone(), ctl.clone());
    assert_eq!(std::thread::spawn(move || c.recover(true, move || cancel.cancel())).join().unwrap(), None);
    assert_eq!(ctl.recover(true, || panic!("asked after cancel")), None);
}

fn files(from: (u64, i64), to: (u64, i64)) -> Conflict {
    let meta = |(size, modified): (u64, i64)| Meta { size, modified, ..Default::default() };
    Conflict { source: FsPath::parse(r"C:\a\f.txt").unwrap(), target: FsPath::parse(r"D:\b\f.txt").unwrap(), from: meta(from), to: meta(to) }
}

const SECOND: i64 = 10_000_000;

#[test]
fn replace_older_replaces_only_what_the_source_has_newer_and_merges_folders() {
    let t = 133_000_000_000 * SECOND / 10_000;
    assert_eq!(Resolve::ReplaceOlder.decide(&files((5, t + 60 * SECOND), (5, t))), Some(Resolve::Replace));
    assert_eq!(Resolve::ReplaceOlder.decide(&files((5, t), (5, t + 60 * SECOND))), Some(Resolve::Skip), "a newer item at the destination stays");
    assert_eq!(Resolve::ReplaceOlder.decide(&files((9, t + SECOND), (5, t))), Some(Resolve::Skip), "FAT rounds times to two seconds, so that is not newer");
    assert_eq!(Resolve::ReplaceOlder.decide(&files((5, 0), (5, t))), Some(Resolve::Skip), "an unknown date is never newer");
    let mut folders = files((0, t + 60 * SECOND), (0, t));
    folders.from.attrs = attr::DIR;
    assert_eq!(Resolve::ReplaceOlder.decide(&folders), Some(Resolve::Skip), "a newer folder never replaces a file");
    folders.to.attrs = attr::DIR;
    folders.from.modified = 0;
    assert_eq!(Resolve::ReplaceOlder.decide(&folders), Some(Resolve::Replace), "two folders merge so that their files are compared one by one");
}

#[test]
fn skip_same_skips_only_files_with_the_same_size_and_date() {
    let t = 133_000_000_000 * SECOND / 10_000;
    assert!(files((7, t), (7, t + 2 * SECOND)).identical());
    assert!(!files((7, t), (8, t)).identical() && !files((7, t), (7, t + 3 * SECOND)).identical());
    assert_eq!(Resolve::SkipSame.decide(&files((7, t), (7, t))), Some(Resolve::Skip));
    assert_eq!(Resolve::SkipSame.decide(&files((7, t), (8, t))), None, "a different file is asked about");
    let ctl = OpCtl::new(Some(Resolve::SkipSame));
    assert_eq!(ctl.resolve(&files((7, t), (7, t)), || panic!("asked about an identical file")), Some(Resolve::Skip));
    let (c, answer) = (ctl.clone(), ctl.clone());
    let asked = std::thread::spawn(move || c.resolve(&files((7, t), (8, t)), move || answer.answer(Resolve::KeepBoth, false))).join().unwrap();
    assert_eq!((asked, ctl.policy()), (Some(Resolve::KeepBoth), Some(Resolve::SkipSame)), "answering one file keeps skipping identical ones");
}

#[test]
fn the_new_policies_never_replace_items_that_are_too_large_or_unreadable() {
    let dest = FsPath::parse(r"E:\").unwrap();
    let file = FsPath::parse(r"C:\a\big.iso").unwrap();
    let special = [crate::limits::over_limit(&file, &dest.join(b"big.iso"), 5 << 30, crate::limits::FAT_MAX_FILE), crate::archive::unreadable(&file, Meta::default())];
    for c in &special {
        for p in [Resolve::SkipSame, Resolve::ReplaceOlder] {
            assert_eq!(p.decide(c), Some(Resolve::Skip), "{p:?}");
        }
        for p in [Resolve::KeepBoth, Resolve::Replace, Resolve::Skip] {
            assert_eq!(p.decide(c), Some(p), "the older policies keep their meaning");
        }
    }
}

#[test]
fn an_answer_settled_elsewhere_carries_the_policy_for_the_rest() {
    let ctl = OpCtl::default();
    let (c, answer) = (ctl.clone(), ctl.clone());
    let t = 133_000_000_000 * SECOND / 10_000;
    let first = std::thread::spawn(move || c.resolve(&files((1, t), (2, t)), move || answer.settle(Resolve::Skip, Some(Resolve::ReplaceOlder)))).join().unwrap();
    assert_eq!(first, Some(Resolve::Skip));
    assert_eq!(ctl.resolve(&files((1, t + 60 * SECOND), (2, t)), || panic!("asked again")), Some(Resolve::Replace));
}

#[test]
fn a_paused_operation_waits_until_it_is_resumed_or_cancelled() {
    let ctl = OpCtl::default();
    ctl.wait_while_paused();
    ctl.pause(true);
    let (c, done) = (ctl.clone(), Arc::new(AtomicBool::new(false)));
    let d = done.clone();
    let waiting = std::thread::spawn(move || {
        c.wait_while_paused();
        d.store(true, Ordering::Release);
    });
    std::thread::sleep(Duration::from_millis(50));
    assert!(!done.load(Ordering::Acquire), "the operation does not go on while paused");
    ctl.pause(false);
    waiting.join().unwrap();
    ctl.pause(true);
    let c = ctl.clone();
    let waiting = std::thread::spawn(move || c.wait_while_paused());
    ctl.cancel();
    waiting.join().unwrap();
    assert!(ctl.is_paused() && ctl.is_cancelled(), "cancel ends the wait without resuming");
}

#[test]
fn dropped_file_bytes_are_local_only_and_the_legacy_wire_format_is_unchanged() {
    let dest = FsPath::parse("/tmp").unwrap();
    let data = Arc::new(DroppedFile { name: b"private.txt".to_vec(), bytes: b"synthetic private body".to_vec() });
    let legacy = [23, 4, b'/', b't', b'm', b'p', 0];
    for payload in [None, Some(data)] {
        let op = FileOp::PasteDropped { dest: dest.clone(), moving: false, data: payload };
        assert_eq!(postcard::to_stdvec(&op).unwrap(), legacy);
        assert_eq!(postcard::from_bytes::<FileOp>(&legacy).unwrap(), FileOp::PasteDropped { dest: dest.clone(), moving: false, data: None });
        assert_eq!(op.map_paths(|p| Some(p.clone())), Some(op));
    }
}

#[test]
fn dropped_content_debug_never_reveals_its_body() {
    let data = DroppedFile { name: b"private.txt".to_vec(), bytes: b"synthetic private body".to_vec() };
    let shown = format!("{data:?}");
    assert!(!shown.contains("synthetic private body"));
    assert!(!shown.contains("115, 121, 110, 116"));
    assert!(shown.contains("private.txt") && shown.contains("22"));
}
