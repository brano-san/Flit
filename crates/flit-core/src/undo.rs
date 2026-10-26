use crate::archive::ArchiveOp;
use crate::i18n::S;
use crate::location::ProviderId;
use crate::provider::{FileOp, OpReport};

const DEPTH: usize = 100;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    Run(FileOp),
    Refuse(&'static str),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    Fresh,
    Undo,
    Redo,
}

pub fn inverse(report: &OpReport) -> Option<Step> {
    let items = &report.items;
    if items.is_empty() {
        return None;
    }
    let created = || FileOp::Trash(items.iter().map(|(_, to)| to.clone()).collect());
    Some(Step::Run(match &report.op {
        FileOp::Archive(ArchiveOp::Open { .. }) => return None,
        FileOp::Copy { .. } | FileOp::Link { .. } | FileOp::NewFolder { .. } | FileOp::NewFile { .. } | FileOp::Restore(_) | FileOp::Archive(_) => created(),
        FileOp::Move { .. } | FileOp::MoveTo(_) | FileOp::Flatten(_) => FileOp::MoveTo(items.iter().map(|(from, to)| (to.clone(), from.clone())).collect()),
        FileOp::Rename { .. } => {
            let (old, new) = &items[0];
            FileOp::Rename { item: new.clone(), name: old.file_name().to_vec() }
        }
        FileOp::Trash(_) => {
            let recycled: Vec<_> = items.iter().filter(|(from, to)| from != to).map(|(from, to)| (to.clone(), from.clone())).collect();
            if recycled.is_empty() {
                return Some(Step::Refuse(S::ErrorsOpsNotRecycled.en()));
            }
            FileOp::Restore(recycled)
        }
        FileOp::Paste { .. } | FileOp::NewStream { .. } | FileOp::Delete(_) | FileOp::Purge(_) | FileOp::EmptyBin | FileOp::FolderIcon { .. } | FileOp::InstallFonts(_) | FileOp::LockScreen(_) => return None,
    }))
}

#[derive(Default, Debug)]
pub struct Journal {
    undo: Vec<(ProviderId, Step)>,
    redo: Vec<(ProviderId, Step)>,
}

impl Journal {
    pub fn record(&mut self, origin: Origin, provider: ProviderId, report: &OpReport) {
        let Some(step) = inverse(report) else { return };
        let stack = match origin {
            Origin::Fresh => {
                self.redo.clear();
                &mut self.undo
            }
            Origin::Undo => &mut self.redo,
            Origin::Redo => &mut self.undo,
        };
        stack.push((provider, step));
        if stack.len() > DEPTH {
            stack.remove(0);
        }
    }

    pub fn take(&mut self, origin: Origin) -> Option<(ProviderId, Step)> {
        match origin {
            Origin::Undo => self.undo.pop(),
            Origin::Redo => self.redo.pop(),
            Origin::Fresh => None,
        }
    }

    pub fn depth(&self) -> (usize, usize) {
        (self.undo.len(), self.redo.len())
    }
}

#[cfg(test)]
#[path = "../test/undo.rs"]
mod tests;
