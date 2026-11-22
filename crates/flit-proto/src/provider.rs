use crate::listing::{check_meta, check_name, EntryBatch};
use crate::transport::ProtoError;
use flit_core::listing::{DirChange, DirStats};
use flit_core::path::PathKind;
use flit_core::provider::{Conflict, Failure, FileOp, OpProgress, Recover, Resolve};
use flit_core::{FsError, FsPath, Meta};
use flit_core::undo::Replay;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub const VERSION: u32 = 8;
pub const MAX_FAILURE_TEXT: usize = 4096;

pub type ReqId = u64;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum Request {
    List { id: ReqId, path: FsPath },
    Stat { id: ReqId, path: FsPath },
    Watch { id: ReqId, path: FsPath },
    DirSizes { id: ReqId, dir: FsPath, items: Vec<(u32, Vec<u8>)> },
    Op { id: ReqId, op: FileOp, policy: Option<Resolve> },
    Answer { id: ReqId, choice: Option<Resolve>, policy: Option<Resolve> },
    Cancel { id: ReqId },
    Shutdown,
    Recover { id: ReqId, choice: Option<Recover>, all: bool },
    Lang { code: String },
    Pause { id: ReqId, paused: bool },
    Replay { id: ReqId, plan: Replay },
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
    OpFailure { id: ReqId, failure: Failure },
    OpPerformed { id: ReqId, op: FileOp },
    OpReplaced { id: ReqId, targets: Vec<(FsPath, Option<FsPath>)> },
    OpUndo { id: ReqId, undo: Undo },
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum Undo {
    Checked(Replay),
    Unavailable,
    BackupUnavailable,
    NotRecycled,
    StampUnavailable,
}

pub fn check_replay(plan: &Replay) -> Result<(), ProtoError> {
    let linux = |path: &FsPath| path.kind() == PathKind::Unix && flit_core::archive::split(path).is_none();
    let checks_match = flit_core::undo::check_paths(&plan.steps).is_some_and(|paths| {
        paths.len() == plan.checks.len() && paths.iter().zip(&plan.checks).all(|((path, exists), (checked, stamp))| path == checked && *exists == stamp.is_some())
    });
    if !checks_match || plan.steps.is_empty() || plan.steps.iter().any(|op| {
        !matches!(op, FileOp::MoveTo(_) | FileOp::Rename { .. } | FileOp::Trash(_) | FileOp::Restore(_) | FileOp::Access(_))
            || !op.all_paths(linux)
    }) || plan.checks.iter().any(|(path, _)| !linux(path)) {
        return Err(ProtoError::Invalid("invalid paths or operations in an undo plan"));
    }
    Ok(())
}

pub fn check_replacements(op: &FileOp, targets: &[(FsPath, Option<FsPath>)]) -> Result<(), ProtoError> {
    let Some(scope) = scope(op).filter(|_| matches!(op, FileOp::Copy { .. } | FileOp::Move { .. } | FileOp::MoveTo(_) | FileOp::Flatten(_) | FileOp::Restore(_))) else {
        return Err(ProtoError::Invalid("replacements for an operation that cannot replace items"));
    };
    let (_, allowed) = sets(&scope);
    if targets.iter().all(|(target, backup)| under(&allowed, target) && backup.as_ref().is_none_or(|p| {
        p.kind() == PathKind::Unix && p.parent().is_some_and(|dir| dir.file_name() == b"files")
    })) {
        Ok(())
    } else {
        Err(ProtoError::Invalid("a replacement path lies outside the operation"))
    }
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
            Event::OpFailure { failure, .. } if failure.error.len() > MAX_FAILURE_TEXT => Err(ProtoError::Invalid("a failure message is too long")),
            Event::OpUndo { undo: Undo::Checked(plan), .. } => check_replay(plan),
            _ => Ok(()),
        }
    }
}

pub fn agent_runs(op: &FileOp) -> bool {
    matches!(op, FileOp::Copy { .. } | FileOp::Move { .. } | FileOp::MoveTo(_) | FileOp::Rename { .. } | FileOp::NewFolder { .. } | FileOp::Delete(_) | FileOp::Trash(_) | FileOp::Restore(_) | FileOp::SetAccess { .. } | FileOp::Access(_))
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
        FileOp::NewFolder { dir, .. } | FileOp::NewFile { dir, .. } => (vec![dir.clone()], vec![dir.clone()]),
        FileOp::Link { items, dest } => (items.clone(), vec![dest.clone()]),
        FileOp::Flatten(dirs) => (dirs.clone(), dirs.clone()),
        FileOp::Delete(items) => (items.clone(), items.clone()),
        FileOp::Trash(items) => (items.clone(), Vec::new()),
        FileOp::SetAccess { items, .. } => (items.clone(), items.clone()),
        FileOp::Access(changes) => {
            let items: Vec<FsPath> = changes.iter().map(|c| c.item.clone()).collect();
            (items.clone(), items)
        }
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

pub fn check_performed(op: &FileOp, performed: &FileOp) -> Result<(), ProtoError> {
    let (Some(scope), FileOp::Access(changes)) = (scope(op).filter(|_| matches!(op, FileOp::SetAccess { .. } | FileOp::Access(_))), performed) else {
        return Err(ProtoError::Invalid("a performed operation for an operation that does not report one"));
    };
    let (sources, _) = sets(&scope);
    if changes.iter().all(|c| under(&sources, &c.item)) { Ok(()) } else { Err(ProtoError::Invalid("a performed path lies outside the operation")) }
}

pub fn check_conflict(op: &FileOp, c: &Conflict) -> Result<(), ProtoError> {
    let asks = matches!(op, FileOp::Copy { .. } | FileOp::Move { .. } | FileOp::MoveTo(_) | FileOp::Restore(_) | FileOp::Link { .. } | FileOp::Flatten(_));
    let scope = scope(op).filter(|_| asks).ok_or(ProtoError::Invalid("a conflict for an operation that cannot have one"))?;
    let (sources, targets) = sets(&scope);
    if under(&sources, &c.source) && under(&targets, &c.target) { Ok(()) } else { Err(ProtoError::Invalid("a conflict path lies outside the operation")) }
}

pub fn check_failure(op: &FileOp, f: &Failure) -> Result<(), ProtoError> {
    let asks = matches!(op, FileOp::Copy { .. } | FileOp::Move { .. } | FileOp::MoveTo(_) | FileOp::Restore(_) | FileOp::Delete(_) | FileOp::Link { .. } | FileOp::Flatten(_) | FileOp::NewFile { .. });
    let scope = scope(op).filter(|_| asks).ok_or(ProtoError::Invalid("a failure for an operation that does not ask about one"))?;
    let (sources, targets) = sets(&scope);
    if under(&sources, &f.path) || under(&targets, &f.path) { Ok(()) } else { Err(ProtoError::Invalid("a failure path lies outside the operation")) }
}

#[cfg(test)]
#[path = "../test/provider.rs"]
mod tests;
