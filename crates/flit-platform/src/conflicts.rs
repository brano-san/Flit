use crate::Fs;
use flit_core::listing::attr;
use flit_core::provider::{Conflict, FileOp, OpCtl};
use flit_core::sort::CaseMode;
use flit_core::{FsPath, ListingBuilder};

const MAX_REVIEW_ROWS: usize = 5000;

pub fn collect(fs: &dyn Fs, op: &FileOp, ctl: &OpCtl) -> Vec<Conflict> {
    if ctl.policy().is_some() || ctl.is_cancelled() {
        return Vec::new();
    }
    let mut pending: Vec<(FsPath, FsPath)> = match op {
        FileOp::Copy { items, dest } | FileOp::Move { items, dest } => items.iter().rev().map(|source| (source.clone(), dest.join(source.file_name()))).collect(),
        FileOp::MoveTo(pairs) => pairs.iter().rev().cloned().collect(),
        _ => return Vec::new(),
    };
    let mut conflicts = Vec::new();
    while let Some((source, target)) = pending.pop() {
        if ctl.is_cancelled() {
            return Vec::new();
        }
        if source.same(&target) || target.is_inside(&source) {
            continue;
        }
        let Some(to) = fs.operation_meta(&target) else { continue };
        let Some(from) = fs.operation_meta(&source) else { continue };
        if conflicts.len() == MAX_REVIEW_ROWS {
            return Vec::new();
        }
        let folder = |attrs| attrs & (attr::DIR | attr::LINK) == attr::DIR;
        if folder(from.attrs) && folder(to.attrs) {
            let mut each = |_: &ListingBuilder<'_>| !ctl.is_cancelled();
            let mut listing = ListingBuilder::new(CaseMode::Sensitive, &mut each);
            if fs.read_dir(&source, &mut listing).is_ok() {
                for index in (0..listing.listing.len() as u32).rev() {
                    if listing.listing.entries[index as usize].attrs & attr::STREAM == 0 {
                        let name = listing.listing.name(index);
                        pending.push((source.join(name), target.join(name)));
                    }
                }
            }
        }
        conflicts.push(Conflict { source, target, from, to });
    }
    conflicts
}

#[cfg(test)]
#[path = "../test/conflicts.rs"]
mod tests;
