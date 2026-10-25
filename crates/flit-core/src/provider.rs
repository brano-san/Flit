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
    Copy { items: Vec<FsPath>, dest: FsPath },
    Move { items: Vec<FsPath>, dest: FsPath },
    Link { items: Vec<FsPath>, dest: FsPath },
    Rename { item: FsPath, name: Vec<u8> },
    NewFolder { dir: FsPath, name: Vec<u8> },
    MoveTo(Vec<(FsPath, FsPath)>),
    Restore(Vec<(FsPath, FsPath)>),
    Paste { dest: FsPath },
    Delete(Vec<FsPath>),
    Purge(Vec<(FsPath, FsPath)>),
    EmptyBin,
}

impl FileOp {
    pub fn map_paths(&self, f: impl Fn(&FsPath) -> Option<FsPath>) -> Option<FileOp> {
        let all = |v: &[FsPath]| v.iter().map(&f).collect::<Option<Vec<_>>>();
        let pairs = |v: &[(FsPath, FsPath)]| v.iter().map(|(a, b)| Some((f(a)?, f(b)?))).collect::<Option<Vec<_>>>();
        Some(match self {
            FileOp::Trash(v) => FileOp::Trash(all(v)?),
            FileOp::Copy { items, dest } => FileOp::Copy { items: all(items)?, dest: f(dest)? },
            FileOp::Move { items, dest } => FileOp::Move { items: all(items)?, dest: f(dest)? },
            FileOp::Link { items, dest } => FileOp::Link { items: all(items)?, dest: f(dest)? },
            FileOp::Rename { item, name } => FileOp::Rename { item: f(item)?, name: name.clone() },
            FileOp::NewFolder { dir, name } => FileOp::NewFolder { dir: f(dir)?, name: name.clone() },
            FileOp::MoveTo(v) => FileOp::MoveTo(pairs(v)?),
            FileOp::Restore(v) => FileOp::Restore(pairs(v)?),
            FileOp::Paste { dest } => FileOp::Paste { dest: f(dest)? },
            FileOp::Delete(v) => FileOp::Delete(all(v)?),
            FileOp::Purge(v) => FileOp::Purge(pairs(v)?),
            FileOp::EmptyBin => FileOp::EmptyBin,
        })
    }
}

pub type OpId = u64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpReport {
    pub op: FileOp,
    pub items: Vec<(FsPath, FsPath)>,
}

#[derive(Debug)]
pub enum ProviderRequest {
    List { tab: TabId, gen: u64, path: FsPath, view: ViewSpec, case: CaseMode, out: Arc<SnapshotSlot>, cancel: GenToken },
    Watch { tab: TabId, gen: u64, path: Option<FsPath> },
    DirSizes { tab: TabId, gen: u64, dir: FsPath, items: Vec<(u32, Vec<u8>)>, cancel: GenToken },
    Op { tab: TabId, id: OpId, op: FileOp },
    Open { tab: TabId, path: FsPath },
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProviderEvent {
    Changes { tab: TabId, gen: u64, changes: Vec<DirChange> },
    Rescan { tab: TabId, gen: u64 },
    DirSize { tab: TabId, gen: u64, idx: u32, size: u64 },
    OpDone { tab: TabId, id: OpId, error: Option<String>, report: Option<OpReport> },
}

#[cfg(test)]
#[path = "../test/provider.rs"]
mod tests;
