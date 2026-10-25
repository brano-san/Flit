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
