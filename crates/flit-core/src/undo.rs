use crate::archive::ArchiveOp;
use crate::i18n::S;
use crate::location::ProviderId;
use crate::path::{FsPath, PathKind};
use crate::provider::{AccessChange, FileOp, OpReport};
use std::collections::HashSet;

const DEPTH: usize = 100;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    Run(FileOp),
    Refuse(&'static str),
    Checked(Replay),
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Replay {
    pub steps: Vec<FileOp>,
    pub checks: Vec<(FsPath, Option<Stamp>)>,
    pub recheck_tree: bool,
    pub captured_ns: Option<i128>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Stamp {
    pub size: u64,
    pub modified: i128,
    pub device: u64,
    pub file: u64,
    pub access: Option<crate::provider::Access>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub replaced: Vec<(FsPath, Option<FsPath>)>,
    pub checked: Option<Step>,
    pub unverified: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    Fresh,
    Undo,
    Redo,
}

pub fn check_paths(steps: &[FileOp]) -> Option<Vec<(FsPath, bool)>> {
    let mut seen = HashSet::new();
    let mut checks = Vec::new();
    for step in steps {
        let pairs = match step {
            FileOp::MoveTo(pairs) | FileOp::Restore(pairs) => pairs.clone(),
            FileOp::Rename { item, name } => {
                let target = item.parent().map_or_else(|| item.clone(), |p| p.join(name));
                vec![(item.clone(), target)]
            }
            FileOp::Trash(paths) | FileOp::Delete(paths) => paths.iter().map(|p| (p.clone(), p.clone())).collect(),
            FileOp::Access(changes) => changes.iter().map(|c| (c.item.clone(), c.item.clone())).collect(),
            _ => return None,
        };
        for (from, to) in pairs {
            let key = |p: &FsPath| if p.case_sensitive() { p.as_bytes().to_vec() } else { p.as_bytes().to_ascii_lowercase() };
            if seen.insert(key(&from)) {
                checks.push((from.clone(), true));
            }
            if !to.same(&from) && seen.insert(key(&to)) {
                checks.push((to, false));
            }
        }
    }
    Some(checks)
}

pub fn inverse(report: &OpReport) -> Option<Step> {
    if let Some(step) = &report.undo.checked {
        return Some(step.clone());
    }
    let mut steps = inverse_ops(report)?;
    if steps.len() == 1 {
        return steps.pop();
    }
    Some(Step::Checked(Replay {
        steps: steps
            .into_iter()
            .filter_map(|step| match step {
                Step::Run(op) => Some(op),
                _ => None,
            })
            .collect(),
        checks: Vec::new(),
        recheck_tree: false,
        captured_ns: None,
    }))
}

pub fn inverse_ops(report: &OpReport) -> Option<Vec<Step>> {
    let items = &report.items;
    if items.is_empty() {
        return None;
    }
    if report.undo.replaced.iter().any(|(_, backup)| backup.is_none()) {
        return Some(vec![Step::Refuse(S::ErrorsOpsUndoBackup.en())]);
    }
    if report.undo.unverified {
        return Some(vec![Step::Refuse(S::ErrorsOpsUndoSystem.en())]);
    }
    if report.undo.replaced.iter().any(|(target, _)| items.iter().filter(|(_, to)| to.same(target)).take(2).count() > 1) {
        return Some(vec![Step::Refuse(S::ErrorsOpsUndoUnavailable.en())]);
    }
    let created = || FileOp::Trash(items.iter().map(|(_, to)| to.clone()).collect());
    let mut steps = vec![Step::Run(match &report.op {
        FileOp::Archive(ArchiveOp::Open { .. }) => return None,
        FileOp::LinkAs { .. } => FileOp::Delete(items.iter().map(|(_, to)| to.clone()).collect()),
        FileOp::Link { .. } if items.iter().all(|(_, to)| to.kind() == PathKind::Unix) => FileOp::Delete(items.iter().map(|(_, to)| to.clone()).collect()),
        FileOp::Copy { .. } | FileOp::Link { .. } | FileOp::NewFolder { .. } | FileOp::NewFile { .. } | FileOp::Restore(_) | FileOp::Archive(_) => created(),
        FileOp::Segments { .. } => created(),
        FileOp::Dedup { keep, backups, .. } => {
            if backups.is_empty() {
                return None;
            }
            FileOp::RestoreDedup { keep: keep.clone(), backups: backups.clone() }
        }
        FileOp::RestoreDedup { keep, .. } => FileOp::Dedup { keep: keep.clone(), candidates: items.iter().map(|(_, to)| to.clone()).collect(), backups: Vec::new() },
        FileOp::Move { .. } | FileOp::MoveTo(_) | FileOp::Flatten(_) => FileOp::MoveTo(items.iter().map(|(from, to)| (to.clone(), from.clone())).collect()),
        FileOp::Rename { .. } => {
            let (old, new) = &items[0];
            FileOp::Rename { item: new.clone(), name: old.file_name().to_vec() }
        }
        FileOp::Trash(_) => {
            let recycled: Vec<_> = items.iter().filter(|(from, to)| from != to).map(|(from, to)| (to.clone(), from.clone())).collect();
            if recycled.is_empty() {
                return Some(vec![Step::Refuse(S::ErrorsOpsNotRecycled.en())]);
            }
            FileOp::Restore(recycled)
        }
        FileOp::Access(changes) => FileOp::Access(changes.iter().rev().map(|c| AccessChange { item: c.item.clone(), from: c.to, to: c.from }).collect()),
        FileOp::Paste { .. }
        | FileOp::PasteLink { .. }
        | FileOp::PasteLinkAs { .. }
        | FileOp::NewStream { .. }
        | FileOp::Delete(_)
        | FileOp::Purge(_)
        | FileOp::EmptyBin
        | FileOp::FolderIcon { .. }
        | FileOp::InstallFonts(_)
        | FileOp::LockScreen(_)
        | FileOp::SetAccess { .. }
        | FileOp::SetAttributes { .. }
        | FileOp::PasteDropped { .. }
        | FileOp::GrantAccess(_) => return None,
    })];
    if !report.undo.replaced.is_empty() {
        let restore = report.undo.replaced.iter().filter_map(|(to, backup)| Some((backup.clone()?, to.clone()))).collect();
        steps.push(Step::Run(FileOp::Restore(restore)));
    }
    Some(steps)
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub id: u64,
    pub provider: ProviderId,
    label: S,
    pub example: FsPath,
    pub step: Step,
    paths: Vec<FsPath>,
}

#[derive(Default, Debug)]
pub struct Journal {
    undo: Vec<Entry>,
    redo: Vec<Entry>,
    current: Option<(Origin, usize, Entry)>,
    id: u64,
}

impl Journal {
    pub fn record(&mut self, origin: Origin, provider: ProviderId, report: &OpReport) {
        if let (Origin::Undo, FileOp::RestoreDedup { keep, backups }) = (origin, &report.op) {
            let remaining: Vec<_> = backups.iter().filter(|b| !report.items.iter().any(|(_, target)| *target == b.target)).cloned().collect();
            if !remaining.is_empty() && !report.items.is_empty() {
                self.id += 1;
                let paths = remaining.iter().map(|b| b.target.clone()).chain([keep.clone()]).collect();
                let example = remaining[0].target.clone();
                let step = Step::Run(FileOp::RestoreDedup { keep: keep.clone(), backups: remaining });
                self.undo.push(Entry { id: self.id, provider: provider.clone(), label: operation_label(&report.op), example, step, paths });
            }
        }
        let Some(step) = inverse(report) else {
            if origin != Origin::Fresh {
                self.retry();
            }
            return;
        };
        let entry = if origin != Origin::Fresh {
            self.current.take().map(|(_, _, mut entry)| {
                entry.step = step.clone();
                entry
            })
        } else {
            None
        };
        let entry = entry.unwrap_or_else(|| {
            self.id += 1;
            let paths = report.items.iter().flat_map(|(a, b)| [a.clone(), b.clone()]).collect();
            Entry { id: self.id, provider, label: operation_label(&report.op), example: report.items[0].0.clone(), step, paths }
        });
        let stack = match origin {
            Origin::Fresh => {
                self.redo.clear();
                &mut self.undo
            }
            Origin::Undo => &mut self.redo,
            Origin::Redo => &mut self.undo,
        };
        stack.push(entry);
        if stack.len() > DEPTH {
            stack.remove(0);
        }
    }

    pub fn entries(&self, origin: Origin) -> &[Entry] {
        match origin {
            Origin::Undo => &self.undo,
            Origin::Redo => &self.redo,
            Origin::Fresh => &[],
        }
    }

    pub fn running(&self) -> bool {
        self.current.is_some()
    }

    pub fn blockers(&self, origin: Origin, id: u64) -> Vec<u64> {
        let entries = self.entries(origin);
        let Some(at) = entries.iter().position(|entry| entry.id == id) else { return Vec::new() };
        let mut affected = vec![&entries[at]];
        for entry in &entries[at + 1..] {
            if affected.iter().any(|earlier| earlier.overlaps(entry)) {
                affected.push(entry);
            }
        }
        affected[1..].iter().rev().map(|entry| entry.id).collect()
    }

    pub fn begin(&mut self, origin: Origin, id: Option<u64>) -> Option<(ProviderId, Step)> {
        if self.running() {
            return None;
        }
        let stack = self.stack_mut(origin)?;
        let at = match id {
            Some(id) => stack.iter().position(|entry| entry.id == id)?,
            None => stack.len().checked_sub(1)?,
        };
        let entry = &stack[at];
        let step = (entry.provider.clone(), entry.step.clone());
        if !matches!(step.1, Step::Refuse(_)) {
            let entry = stack.remove(at);
            self.current = Some((origin, at, entry));
        } else if id.is_none() {
            stack.remove(at);
        }
        Some(step)
    }

    fn stack_mut(&mut self, origin: Origin) -> Option<&mut Vec<Entry>> {
        match origin {
            Origin::Undo => Some(&mut self.undo),
            Origin::Redo => Some(&mut self.redo),
            Origin::Fresh => None,
        }
    }

    pub fn retry(&mut self) {
        if let Some((origin, at, entry)) = self.current.take() {
            if let Some(stack) = self.stack_mut(origin) {
                stack.insert(at.min(stack.len()), entry);
            }
        }
    }

    #[cfg(test)]
    fn take(&mut self, origin: Origin) -> Option<(ProviderId, Step)> {
        let entry = self.stack_mut(origin)?.pop()?;
        Some((entry.provider, entry.step))
    }

    pub fn depth(&self) -> (usize, usize) {
        (self.undo.len(), self.redo.len())
    }
}

impl Entry {
    pub fn label(&self) -> String {
        format!("{} · {}", self.label.t(), self.example.display())
    }

    fn overlaps(&self, other: &Self) -> bool {
        self.provider == other.provider && self.paths.iter().any(|a| other.paths.iter().any(|b| a.same(b) || a.is_inside(b) || b.is_inside(a)))
    }
}

fn operation_label(op: &FileOp) -> S {
    match op {
        FileOp::Copy { .. } => S::ActionsFileCopy,
        FileOp::Move { .. } | FileOp::MoveTo(_) | FileOp::Flatten(_) => S::UiOpsUndoHistoryMove,
        FileOp::Rename { .. } => S::ActionsFileRename,
        FileOp::Trash(_) => S::ActionsItemTrash,
        FileOp::Restore(_) => S::ActionsBinRestore,
        FileOp::NewFolder { .. } => S::ActionsFileNewFolder,
        FileOp::NewFile { .. } => S::ActionsFileNewFile,
        FileOp::Archive(ArchiveOp::Compress { .. }) => S::ActionsArchiveCompress,
        _ => S::UiOpsUndoHistoryOperation,
    }
}

#[cfg(test)]
#[path = "../test/undo.rs"]
mod tests;
