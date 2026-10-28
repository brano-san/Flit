use crate::listing::{check_meta, check_name, EntryBatch};
use crate::transport::ProtoError;
use flit_core::listing::{DirChange, DirStats};
use flit_core::path::PathKind;
use flit_core::provider::{Conflict, FileOp, OpProgress, Resolve};
use flit_core::{FsError, FsPath, Meta};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub const VERSION: u32 = 4;

pub type ReqId = u64;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum Request {
    List { id: ReqId, path: FsPath },
    Stat { id: ReqId, path: FsPath },
    Watch { id: ReqId, path: FsPath },
    DirSizes { id: ReqId, dir: FsPath, items: Vec<(u32, Vec<u8>)> },
    Op { id: ReqId, op: FileOp, policy: Option<Resolve> },
    Answer { id: ReqId, choice: Option<Resolve>, all: bool },
    Cancel { id: ReqId },
    Shutdown,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum Event {
    Entries { id: ReqId, batch: EntryBatch },
    ListDone { id: ReqId, error: Option<FsError> },
    Stat { id: ReqId, result: Result<Meta, FsError> },
    Changes { id: ReqId, changes: Vec<DirChange> },
    Rescan { id: ReqId },
    DirSize { id: ReqId, idx: u32, stats: DirStats },
    OpProgress { id: ReqId, progress: OpProgress },
    OpConflict { id: ReqId, conflict: Conflict },
    OpDone { id: ReqId, error: Option<String>, items: Vec<(FsPath, FsPath)> },
}

impl Event {
    pub fn validate(&self, windows_names: bool) -> Result<(), ProtoError> {
        match self {
            Event::Entries { batch, .. } => batch.entries(windows_names).map(drop),
            Event::Changes { changes, .. } => changes.iter().try_for_each(|c| {
                check_name(&c.name, windows_names)?;
                c.meta.as_ref().map_or(Ok(()), check_meta)
            }),
            Event::Stat { result: Ok(meta), .. } => check_meta(meta),
            Event::OpConflict { conflict, .. } => check_meta(&conflict.from).and_then(|()| check_meta(&conflict.to)),
            _ => Ok(()),
        }
    }
}

pub fn agent_runs(op: &FileOp) -> bool {
    matches!(op, FileOp::Copy { .. } | FileOp::Move { .. } | FileOp::MoveTo(_) | FileOp::Rename { .. } | FileOp::NewFolder { .. } | FileOp::Delete(_) | FileOp::Trash(_) | FileOp::Restore(_))
}

fn under(set: &HashSet<&[u8]>, p: &FsPath) -> bool {
    let b = p.as_bytes();
    p.kind() == PathKind::Unix && (set.contains(b) || b.iter().enumerate().rev().filter(|&(_, &c)| c == b'/').any(|(i, _)| set.contains(&b[..i.max(1)])))
}

struct Scope {
    sources: Vec<FsPath>,
    targets: Vec<FsPath>,
    trash: bool,
}

fn scope(op: &FileOp) -> Option<Scope> {
    let parent = |p: &FsPath| p.parent().unwrap_or_else(|| p.clone());
    let (sources, targets) = match op {
        FileOp::Copy { items, dest } | FileOp::Move { items, dest } => (items.clone(), vec![dest.clone()]),
        FileOp::MoveTo(pairs) | FileOp::Restore(pairs) => (pairs.iter().map(|(a, _)| a.clone()).collect(), pairs.iter().map(|(_, b)| parent(b)).collect()),
        FileOp::Rename { item, .. } => (vec![item.clone()], vec![parent(item)]),
        FileOp::NewFolder { dir, .. } => (vec![dir.clone()], vec![dir.clone()]),
        FileOp::Delete(items) => (items.clone(), items.clone()),
        FileOp::Trash(items) => (items.clone(), Vec::new()),
        _ => return None,
    };
    Some(Scope { sources, targets, trash: matches!(op, FileOp::Trash(_)) })
}

fn sets(s: &Scope) -> (HashSet<&[u8]>, HashSet<&[u8]>) {
    (s.sources.iter().map(FsPath::as_bytes).collect(), s.targets.iter().map(FsPath::as_bytes).collect())
}

pub fn check_report(op: &FileOp, items: &[(FsPath, FsPath)]) -> Result<(), ProtoError> {
    let Some(scope) = scope(op) else {
        return if items.is_empty() { Ok(()) } else { Err(ProtoError::Invalid("a report for an operation the agent does not run")) };
    };
    let (sources, targets) = sets(&scope);
    let trashed = |to: &FsPath| to.kind() == PathKind::Unix && to.parent().is_some_and(|p| p.file_name() == b"files");
    let fits = |(from, to): &(FsPath, FsPath)| under(&sources, from) && if scope.trash { trashed(to) } else { under(&targets, to) };
    if items.iter().all(fits) { Ok(()) } else { Err(ProtoError::Invalid("a reported path lies outside the operation")) }
}

pub fn check_conflict(op: &FileOp, c: &Conflict) -> Result<(), ProtoError> {
    let asks = matches!(op, FileOp::Copy { .. } | FileOp::Move { .. } | FileOp::MoveTo(_) | FileOp::Restore(_));
    let scope = scope(op).filter(|_| asks).ok_or(ProtoError::Invalid("a conflict for an operation that cannot have one"))?;
    let (sources, targets) = sets(&scope);
    if under(&sources, &c.source) && under(&targets, &c.target) { Ok(()) } else { Err(ProtoError::Invalid("a conflict path lies outside the operation")) }
}

#[cfg(test)]
#[path = "../test/provider.rs"]
mod tests;
