use crate::listing::{DirChange, DirSnapshot};
use crate::path::FsPath;
use crate::sort::{CaseMode, ViewSpec};
use arc_swap::ArcSwapOption;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

pub type TabId = u64;

#[derive(Default)]
pub struct SnapshotSlot(ArcSwapOption<DirSnapshot>);

impl SnapshotSlot {
    pub fn publish(&self, snap: Arc<DirSnapshot>) {
        self.0.rcu(|cur| match cur {
            Some(newer) if newer.gen > snap.gen => Some(newer.clone()),
            _ => Some(snap.clone()),
        });
    }
    pub fn take(&self) -> Option<Arc<DirSnapshot>> {
        self.0.swap(None)
    }
}

impl fmt::Debug for SnapshotSlot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SnapshotSlot")
    }
}

#[derive(Clone, Default, Debug)]
pub struct GenToken(Arc<AtomicU64>);

impl GenToken {
    pub fn set(&self, gen: u64) {
        self.0.store(gen, Ordering::Release);
    }
    pub fn is_current(&self, gen: u64) -> bool {
        self.0.load(Ordering::Acquire) == gen
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum FileOp {
    Trash(Vec<FsPath>),
}

#[derive(Debug)]
pub enum ProviderRequest {
    List { tab: TabId, gen: u64, path: FsPath, view: ViewSpec, case: CaseMode, out: Arc<SnapshotSlot>, cancel: GenToken },
    Watch { tab: TabId, gen: u64, path: Option<FsPath> },
    DirSizes { tab: TabId, gen: u64, dir: FsPath, items: Vec<(u32, Vec<u8>)>, cancel: GenToken },
    Op { tab: TabId, op: FileOp },
    Open { tab: TabId, path: FsPath },
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProviderEvent {
    Changes { tab: TabId, gen: u64, changes: Vec<DirChange> },
    Rescan { tab: TabId, gen: u64 },
    DirSize { tab: TabId, gen: u64, idx: u32, size: u64 },
    OpDone { tab: TabId, error: Option<String> },
}

#[cfg(test)]
#[path = "../test/provider.rs"]
mod tests;
