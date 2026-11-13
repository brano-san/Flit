use super::*;
use crate::listing::Listing;
use crate::sort::{CaseMode, SortKeys};
use std::time::Duration;

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
