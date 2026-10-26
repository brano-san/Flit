use crate::listing::{attr, Meta};
use crate::provider::Conflict;
use crate::FsPath;

pub const TOO_LARGE: i64 = 0x544C_0001;
pub const FAT_MAX_FILE: u64 = 0xFFFF_FFFF;

pub fn max_file_size(file_system: &str) -> Option<u64> {
    matches!(file_system.to_ascii_uppercase().as_str(), "FAT" | "FAT12" | "FAT16" | "FAT32").then_some(FAT_MAX_FILE)
}

pub fn exceeds(size: u64, limit: Option<u64>) -> bool {
    limit.is_some_and(|l| size > l)
}

pub fn short_of_space(dest: &FsPath, need: u64, free: u64) -> Conflict {
    let marked = |size| Meta { size, modified: 0, created: TOO_LARGE, attrs: attr::DIR };
    Conflict { source: dest.clone(), target: dest.clone(), from: marked(need), to: marked(free) }
}

pub fn over_limit(src: &FsPath, dst: &FsPath, size: u64, limit: u64) -> Conflict {
    let marked = |size| Meta { size, modified: 0, created: TOO_LARGE, attrs: 0 };
    Conflict { source: src.clone(), target: dst.clone(), from: marked(size), to: marked(limit) }
}

pub fn too_large(c: &Conflict) -> bool {
    c.to.created == TOO_LARGE
}

pub fn is_short_of_space(c: &Conflict) -> bool {
    too_large(c) && c.to.attrs & attr::DIR != 0
}

#[cfg(test)]
#[path = "../test/limits.rs"]
mod tests;
