use crate::listing::{Listing, ListingBuilder, Meta};
use crate::path::{FsPath, PathKind};

pub const EXT: &[u8] = b".library-ms";

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Library {
    pub name: String,
    pub path: FsPath,
    pub folders: Vec<FsPath>,
    pub save: Option<FsPath>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum LibraryOp {
    Create(String),
    RestoreDefaults,
    Add { library: FsPath, folder: FsPath },
    Remove { library: FsPath, folder: FsPath },
    SetSave { library: FsPath, folder: FsPath },
}

pub fn is_library(p: &FsPath) -> bool {
    let b = p.as_bytes();
    matches!(p.kind(), PathKind::Drive | PathKind::Unc | PathKind::Verbatim) && b.len() > EXT.len() && b[b.len() - EXT.len()..].eq_ignore_ascii_case(EXT)
}

pub fn stem(p: &FsPath) -> String {
    let n = p.file_name();
    crate::name::display(&n[..n.len().saturating_sub(EXT.len())]).into_owned()
}

pub fn push(out: &mut ListingBuilder<'_>, name: &[u8], meta: Meta, folder: &FsPath) {
    out.push(name, meta);
    out.listing.names.extend_from_slice(folder.as_bytes());
}

pub fn folder(l: &Listing, i: u32) -> &[u8] {
    l.tail(i)
}

pub fn search_scope(folders: &[FsPath]) -> Option<String> {
    if folders.is_empty() {
        return None;
    }
    let mut re = String::from("path:regex:^(?:");
    for (k, f) in folders.iter().enumerate() {
        if k > 0 {
            re.push('|');
        }
        for c in f.display().trim_end_matches('\\').chars() {
            match c {
                c if c.is_whitespace() => re.push_str(&format!("\\x{{{:X}}}", c as u32)),
                c if c.is_ascii() && !c.is_ascii_alphanumeric() => {
                    re.push('\\');
                    re.push(c);
                }
                c => re.push(c),
            }
        }
    }
    re.push_str(")\\\\");
    Some(re)
}

#[cfg(test)]
#[path = "../test/library.rs"]
mod tests;
