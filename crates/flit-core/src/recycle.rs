use crate::listing::{Listing, ListingBuilder, Meta};
use crate::path::FsPath;

pub const BIN_FOLDER: &[u8] = b"$Recycle.Bin";

pub fn push(out: &mut ListingBuilder<'_>, name: &[u16], meta: Meta, recycled: &FsPath, origin: &FsPath) {
    out.push_utf16(name, meta);
    let names = &mut out.listing.names;
    names.extend_from_slice(recycled.as_bytes());
    names.push(0);
    names.extend_from_slice(origin.as_bytes());
}

fn split(l: &Listing, i: u32) -> (&[u8], &[u8]) {
    let t = l.tail(i);
    match t.iter().position(|&b| b == 0) {
        Some(z) => (&t[..z], &t[z + 1..]),
        None => (t, &[]),
    }
}

fn path(b: &[u8]) -> Option<FsPath> {
    Some(b).filter(|b| !b.is_empty()).and_then(FsPath::parse_exact)
}

pub fn recycled(l: &Listing, i: u32) -> Option<FsPath> {
    path(split(l, i).0)
}

pub fn origin(l: &Listing, i: u32) -> &[u8] {
    split(l, i).1
}

pub fn original(l: &Listing, i: u32) -> Option<FsPath> {
    Some(path(origin(l, i))?.join(l.name(i)))
}

pub fn in_bin(p: &FsPath) -> bool {
    let parts: Vec<&[u8]> = p.components().collect();
    let trash = |c: &[u8]| c == b"Trash" || c.starts_with(b".Trash-");
    let freedesktop = || parts.windows(2).any(|w| trash(w[0]) && w[1] == b"files") || parts.windows(3).any(|w| w[0] == b".Trash" && w[2] == b"files");
    parts.iter().any(|c| c.eq_ignore_ascii_case(BIN_FOLDER)) || (p.case_sensitive() && freedesktop())
}

#[cfg(test)]
#[path = "../test/recycle.rs"]
mod tests;
