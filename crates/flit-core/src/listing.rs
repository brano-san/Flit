use crate::name;
use crate::path::FsPath;
use crate::sort::{CaseMode, SortKeys, ViewSpec};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::time::Duration;

pub mod attr {
    pub const DIR: u32 = 1 << 0;
    pub const HIDDEN: u32 = 1 << 1;
    pub const SYSTEM: u32 = 1 << 2;
    pub const READONLY: u32 = 1 << 3;
    pub const LINK: u32 = 1 << 4;
    pub const VOLUME: u32 = 1 << 5;
    pub const SIZE_KNOWN: u32 = 1 << 6;
    pub const DELETED: u32 = 1 << 31;
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Meta {
    pub size: u64,
    pub modified: i64,
    pub created: i64,
    pub attrs: u32,
}

#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct Entry {
    pub size: u64,
    pub modified: i64,
    pub created: i64,
    pub name_off: u32,
    pub attrs: u32,
    pub name_len: u16,
    pub ext_len: u16,
}

impl Entry {
    #[inline]
    pub fn is_dir(&self) -> bool {
        self.attrs & attr::DIR != 0
    }
    #[inline]
    pub fn meta(&self) -> Meta {
        Meta { size: self.size, modified: self.modified, created: self.created, attrs: self.attrs }
    }
    fn set_meta(&mut self, m: Meta) {
        self.size = m.size;
        self.modified = m.modified;
        self.created = m.created;
        self.attrs = m.attrs;
    }
}

#[derive(Clone, Default, Debug)]
pub struct Listing {
    pub names: Vec<u8>,
    pub entries: Vec<Entry>,
}

impl Listing {
    #[inline]
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    #[inline]
    pub fn name(&self, i: u32) -> &[u8] {
        let e = &self.entries[i as usize];
        &self.names[e.name_off as usize..e.name_off as usize + e.name_len as usize]
    }
    #[inline]
    pub fn ext(&self, i: u32) -> &[u8] {
        let n = self.name(i);
        &n[n.len() - self.entries[i as usize].ext_len as usize..]
    }

    fn finish_push(&mut self, off: usize, meta: Meta) -> u32 {
        let len = (self.names.len() - off).min(u16::MAX as usize);
        let name = &self.names[off..off + len];
        let ext_len = if meta.attrs & attr::DIR != 0 { 0 } else { name::ext_len(name) };
        self.entries.push(Entry {
            size: meta.size,
            modified: meta.modified,
            created: meta.created,
            name_off: off as u32,
            attrs: meta.attrs,
            name_len: len as u16,
            ext_len: ext_len as u16,
        });
        (self.entries.len() - 1) as u32
    }

    pub fn push(&mut self, name: &[u8], meta: Meta) -> u32 {
        let off = self.names.len();
        self.names.extend_from_slice(name);
        self.finish_push(off, meta)
    }

    pub fn push_utf16(&mut self, name: &[u16], meta: Meta) -> u32 {
        let off = self.names.len();
        name::push_utf16(&mut self.names, name);
        self.finish_push(off, meta)
    }

    pub fn set_meta(&mut self, i: u32, m: Meta) {
        self.entries[i as usize].set_meta(m);
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsError {
    pub kind: FsErrorKind,
    pub code: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FsErrorKind {
    NotFound,
    AccessDenied,
    NotADirectory,
    Unavailable,
    Other,
}

impl fmt::Display for FsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let what = match self.kind {
            FsErrorKind::NotFound => "Path not found",
            FsErrorKind::AccessDenied => "Access denied",
            FsErrorKind::NotADirectory => "Not a folder",
            FsErrorKind::Unavailable => "Location is not available",
            FsErrorKind::Other => "Error",
        };
        write!(f, "{what} (0x{:08X})", self.code as u32)
    }
}

pub struct ListingBuilder<'a> {
    pub listing: Listing,
    pub keys: SortKeys,
    on_batch: Option<&'a mut dyn FnMut(&ListingBuilder<'_>) -> bool>,
}

impl<'a> ListingBuilder<'a> {
    pub fn new(case: CaseMode, on_batch: &'a mut dyn FnMut(&ListingBuilder<'_>) -> bool) -> Self {
        Self { listing: Listing::default(), keys: SortKeys::new(case), on_batch: Some(on_batch) }
    }

    #[inline]
    pub fn push_utf16(&mut self, name: &[u16], meta: Meta) {
        let i = self.listing.push_utf16(name, meta);
        self.keys.push(self.listing.name(i));
    }

    #[inline]
    pub fn push(&mut self, name: &[u8], meta: Meta) {
        let i = self.listing.push(name, meta);
        self.keys.push(self.listing.name(i));
    }

    pub fn batch_done(&mut self) -> bool {
        let Some(cb) = self.on_batch.take() else { return true };
        let go = cb(self);
        self.on_batch = Some(cb);
        go
    }
}

#[derive(Clone, Debug)]
pub struct DirSnapshot {
    pub gen: u64,
    pub path: FsPath,
    pub listing: Listing,
    pub keys: SortKeys,
    pub order: Vec<u32>,
    pub view: ViewSpec,
    pub done: bool,
    pub error: Option<FsError>,
    pub elapsed: Duration,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChangeKind {
    Added,
    Removed,
    Modified,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirChange {
    pub kind: ChangeKind,
    pub name: Vec<u8>,
    pub meta: Option<Meta>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_entry() {
        assert_eq!(std::mem::size_of::<Entry>(), 40);
    }

    #[test]
    fn single_name_buffer() {
        let mut l = Listing::default();
        let file = Meta { size: 5, ..Default::default() };
        let dir = Meta { attrs: attr::DIR, ..Default::default() };
        l.push(b"a.txt", file);
        l.push_utf16(&"Папка.d".encode_utf16().collect::<Vec<_>>(), dir);
        l.push_utf16(&[b'x' as u16, 0xD800, b'.' as u16, b'r' as u16, b's' as u16], file);
        assert_eq!(l.name(0), b"a.txt");
        assert_eq!(l.ext(0), b"txt");
        assert_eq!(l.name(1), "Папка.d".as_bytes());
        assert_eq!(l.ext(1), b"", "directories have no extension");
        assert_eq!(l.ext(2), b"rs");
        assert_eq!(name::display(l.name(2)), "x\u{FFFD}.rs");
        assert_eq!(l.names.len(), 5 + "Папка.d".len() + 7);
    }

    #[test]
    fn builder_batches_and_cancel() {
        let mut batches = 0;
        let mut cb = |b: &ListingBuilder<'_>| {
            batches += 1;
            b.listing.len() < 2
        };
        let mut b = ListingBuilder::new(CaseMode::Insensitive, &mut cb);
        b.push(b"one", Meta::default());
        assert!(b.batch_done());
        b.push(b"two", Meta::default());
        assert!(!b.batch_done());
        assert_eq!(b.keys.len(), 2);
        drop(b);
        assert_eq!(batches, 2);
    }
}
