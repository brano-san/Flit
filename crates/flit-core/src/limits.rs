use crate::i18n::S;
use crate::listing::{attr, Meta};
use crate::provider::Conflict;
use crate::FsPath;

pub const TOO_LARGE: i64 = 0x544C_0001;
pub const FAT_MAX_FILE: u64 = 0xFFFF_FFFF;

pub fn max_file_size(file_system: &str) -> Option<u64> {
    matches!(file_system.to_ascii_uppercase().as_str(), "FAT" | "FAT12" | "FAT16" | "FAT32" | "VFAT" | "MSDOS" | "UMSDOS").then_some(FAT_MAX_FILE)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BadName {
    Char(u8),
    End,
    Reserved,
}

const FAT_FORBIDDEN: &[u8] = b"\\/:*?\"<>|";
const DEVICES: [&[u8]; 4] = [b"CON", b"PRN", b"AUX", b"NUL"];

pub fn windows_names(file_system: &str) -> bool {
    max_file_size(file_system).is_some() || file_system.eq_ignore_ascii_case("exfat")
}

pub fn bad_name(name: &[u8]) -> Option<BadName> {
    if let Some(&b) = name.iter().find(|&&b| b < 0x20 || FAT_FORBIDDEN.contains(&b)) {
        return Some(BadName::Char(b));
    }
    if name.ends_with(b".") || name.ends_with(b" ") {
        return Some(BadName::End);
    }
    let stem = name.split(|&b| b == b'.').next().unwrap_or_default().trim_ascii_end();
    let numbered = |port: &[u8]| stem.len() == 4 && stem[..3].eq_ignore_ascii_case(port) && (b'1'..=b'9').contains(&stem[3]);
    (DEVICES.iter().any(|d| stem.eq_ignore_ascii_case(d)) || numbered(b"COM") || numbered(b"LPT")).then_some(BadName::Reserved)
}

pub fn fat_safe(name: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = name.iter().map(|&b| if b < 0x20 || FAT_FORBIDDEN.contains(&b) { b'_' } else { b }).collect();
    while out.last().is_some_and(|&b| b == b'.' || b == b' ') {
        out.pop();
    }
    if out.is_empty() {
        out.push(b'_');
    }
    if bad_name(&out) == Some(BadName::Reserved) {
        out.insert(0, b'_');
    }
    out
}

pub fn bad_name_error(path: &FsPath, bad: BadName) -> String {
    let shown = path.display();
    match bad {
        BadName::Char(b) if b < 0x20 => crate::tr_en!(S::ErrorsOpsExtraFatChar, path = shown, symbol = format!("U+{b:04X}")),
        BadName::Char(b) => crate::tr_en!(S::ErrorsOpsExtraFatChar, path = shown, symbol = b as char),
        BadName::End => crate::tr_en!(S::ErrorsOpsExtraFatEnd, path = shown),
        BadName::Reserved => crate::tr_en!(S::ErrorsOpsExtraFatReserved, path = shown, name = crate::name::display(path.file_name())),
    }
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
