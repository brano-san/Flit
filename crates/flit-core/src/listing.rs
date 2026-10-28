use crate::name;
use crate::path::FsPath;
use crate::sort::{CaseMode, SortKeys, ViewSpec};
use crate::tags::{FileKey, Tags};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

pub mod attr {
    pub const DIR: u32 = 1 << 0;
    pub const HIDDEN: u32 = 1 << 1;
    pub const SYSTEM: u32 = 1 << 2;
    pub const READONLY: u32 = 1 << 3;
    pub const LINK: u32 = 1 << 4;
    pub const VOLUME: u32 = 1 << 5;
    pub const SIZE_KNOWN: u32 = 1 << 6;
    pub const ARCHIVED: u32 = 1 << 7;
    pub const STREAM: u32 = 1 << 8;
    pub const NESTED: u32 = 1 << 9;
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
    pub accessed: i64,
    pub name_off: u32,
    pub attrs: u32,
    pub name_len: u16,
    pub ext_len: u16,
    pub tags: u32,
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

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirStats {
    pub size: u64,
    pub folders: u32,
    pub files: u32,
    pub partial: bool,
}

const STATS_KNOWN: u64 = 1 << 63;
const STATS_PARTIAL: u64 = 1 << 62;
const STATS_COUNT: u64 = (1 << 31) - 1;

impl DirStats {
    fn pack(self) -> u64 {
        STATS_KNOWN | if self.partial { STATS_PARTIAL } else { 0 } | (self.folders as u64).min(STATS_COUNT) << 31 | (self.files as u64).min(STATS_COUNT)
    }

    fn unpack(v: u64, size: u64) -> Option<Self> {
        (v & STATS_KNOWN != 0).then_some(Self { size, folders: (v >> 31 & STATS_COUNT) as u32, files: (v & STATS_COUNT) as u32, partial: v & STATS_PARTIAL != 0 })
    }

    pub fn items(self) -> u64 {
        self.folders as u64 + self.files as u64
    }

    pub fn add(&mut self, o: DirStats) {
        self.size += o.size;
        self.folders = self.folders.saturating_add(o.folders);
        self.files = self.files.saturating_add(o.files);
        self.partial |= o.partial;
    }
}

#[derive(Clone, Default, Debug)]
pub struct Listing {
    pub names: Vec<u8>,
    pub entries: Vec<Entry>,
    pub items: Vec<u64>,
    pub volume: u64,
    pub total: u64,
    pub nested: u32,
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
    pub fn tail(&self, i: u32) -> &[u8] {
        let e = &self.entries[i as usize];
        let start = e.name_off as usize + e.name_len as usize;
        let end = self.entries.get(i as usize + 1).map_or(self.names.len(), |next| next.name_off as usize);
        &self.names[start..end.max(start)]
    }
    #[inline]
    pub fn folder(&self, i: u32) -> &[u8] {
        if self.entries[i as usize].attrs & attr::NESTED != 0 { self.tail(i) } else { &[] }
    }

    pub fn stats(&self, i: u32) -> Option<DirStats> {
        DirStats::unpack(*self.items.get(i as usize)?, self.entries[i as usize].size)
    }

    pub fn set_stats(&mut self, i: u32, s: DirStats) {
        if self.items.len() <= i as usize {
            self.items.resize(i as usize + 1, 0);
        }
        self.items[i as usize] = s.pack();
    }

    pub fn drive(&self, i: u32) -> Option<(u64, &[u8])> {
        let t = self.tail(i);
        (self.entries[i as usize].attrs & attr::VOLUME != 0 && t.len() >= 8).then(|| (u64::from_le_bytes(t[..8].try_into().unwrap_or_default()), &t[8..]))
    }

    pub fn push_drive_info(&mut self, free: u64, file_system: &str) {
        self.names.extend_from_slice(&free.to_le_bytes());
        self.names.extend_from_slice(file_system.as_bytes());
    }

    pub fn push_nested(&mut self, name: &[u8], meta: Meta, accessed: i64, folder: &[u8]) -> u32 {
        let i = self.push(name, Meta { attrs: meta.attrs | attr::NESTED, ..meta });
        self.entries[i as usize].accessed = accessed;
        self.names.extend_from_slice(folder);
        self.nested += 1;
        i
    }

    pub fn set_accessed(&mut self, i: u32, t: i64) {
        self.entries[i as usize].accessed = t;
    }

    pub fn mark_deleted(&mut self, i: u32) {
        let e = &mut self.entries[i as usize];
        if e.attrs & attr::DELETED == 0 {
            if e.attrs & attr::NESTED == 0 {
                self.total = self.total.saturating_sub(e.size);
            }
            e.attrs |= attr::DELETED;
        }
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
        if meta.attrs & attr::NESTED == 0 {
            self.total += meta.size;
        }
        self.entries.push(Entry {
            size: meta.size,
            modified: meta.modified,
            created: meta.created,
            accessed: 0,
            name_off: off as u32,
            attrs: meta.attrs,
            name_len: len as u16,
            ext_len: ext_len as u16,
            tags: 0,
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
        let e = &mut self.entries[i as usize];
        if e.attrs & (attr::NESTED | attr::DELETED) == 0 {
            self.total = self.total.saturating_sub(e.size) + m.size;
        }
        let nested = e.attrs & attr::NESTED;
        e.set_meta(Meta { attrs: m.attrs | nested, ..m });
    }

    pub fn set_tags(&mut self, i: u32, mask: u32) {
        self.entries[i as usize].tags = mask;
    }

    pub fn append(&mut self, part: Listing) {
        let base = self.names.len() as u32;
        self.total += part.total;
        self.nested += part.nested;
        self.names.extend_from_slice(&part.names);
        self.entries.extend(part.entries.into_iter().map(|mut e| {
            e.name_off += base;
            e
        }));
    }

    pub fn shrink(&mut self) {
        self.names.shrink_to_fit();
        self.entries.shrink_to_fit();
    }
}

pub const NOT_RESPONDING_CODE: i64 = 0x79;

impl FsError {
    pub fn not_responding() -> Self {
        Self { kind: FsErrorKind::Unavailable, code: NOT_RESPONDING_CODE }
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
    NoMedia,
}

impl fmt::Display for FsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use crate::i18n::S;
        if self.code == crate::archive::PASSWORD_CODE {
            return f.write_str(S::ErrorsArchivePasswordNeeded.t());
        }
        let what = match self.kind {
            _ if self.code == NOT_RESPONDING_CODE => S::ErrorsFsNotResponding,
            FsErrorKind::NotFound => S::ErrorsFsNotFound,
            FsErrorKind::AccessDenied => S::ErrorsFsAccessDenied,
            FsErrorKind::NotADirectory => S::ErrorsFsNotADirectory,
            FsErrorKind::Unavailable => S::ErrorsFsUnavailable,
            FsErrorKind::Other => S::ErrorsFsOther,
            FsErrorKind::NoMedia => S::ErrorsFsNoMedia,
        };
        crate::i18n::fill_into(f, S::ErrorsFsCoded.t(), &[("what", &what), ("code", &format_args!("{:08X}", self.code as u32))]);
        Ok(())
    }
}

pub struct ListingBuilder<'a> {
    pub listing: Listing,
    pub keys: SortKeys,
    on_batch: Option<&'a mut dyn FnMut(&ListingBuilder<'_>) -> bool>,
    tags: Option<Arc<Tags>>,
    volume: Option<u32>,
    streams: bool,
}

impl<'a> ListingBuilder<'a> {
    pub fn new(case: CaseMode, on_batch: &'a mut dyn FnMut(&ListingBuilder<'_>) -> bool) -> Self {
        Self { listing: Listing::default(), keys: SortKeys::new(case), on_batch: Some(on_batch), tags: None, volume: None, streams: false }
    }

    pub fn with_streams(mut self, on: bool) -> Self {
        self.streams = on;
        self
    }

    pub fn wants_streams(&self) -> bool {
        self.streams
    }

    pub fn with_tags(mut self, tags: Arc<Tags>) -> Self {
        self.tags = tags.any().then_some(tags);
        self
    }

    pub fn wants_ids(&self) -> bool {
        self.tags.is_some()
    }

    pub fn set_volume(&mut self, volume: u32) {
        self.volume = self.tags.as_ref().filter(|t| t.has_volume(volume)).map(|_| volume);
    }

    #[inline]
    pub fn push_utf16(&mut self, name: &[u16], meta: Meta) {
        let i = self.listing.push_utf16(name, meta);
        self.keys.push(self.listing.name(i));
    }

    #[inline]
    pub fn push_utf16_id(&mut self, name: &[u16], meta: Meta, id: u64) {
        self.push_utf16(name, meta);
        if let (Some(t), Some(volume)) = (&self.tags, self.volume) {
            let i = self.listing.len() as u32 - 1;
            self.listing.set_tags(i, t.mask(FileKey { volume, id }));
        }
    }

    #[inline]
    pub fn push(&mut self, name: &[u8], meta: Meta) {
        let i = self.listing.push(name, meta);
        self.keys.push(self.listing.name(i));
    }

    pub fn push_nested(&mut self, name: &[u8], meta: Meta, accessed: i64, folder: &[u8]) {
        let i = self.listing.push_nested(name, meta, accessed, folder);
        self.keys.push(self.listing.name(i));
    }

    pub fn set_accessed(&mut self, t: i64) {
        if let Some(e) = self.listing.entries.last_mut() {
            e.accessed = t;
        }
    }

    pub fn tagging(&self) -> Option<(Arc<Tags>, u32)> {
        self.tags.clone().zip(self.volume)
    }

    pub fn append(&mut self, listing: Listing, keys: SortKeys) {
        self.listing.append(listing);
        self.keys.append(keys);
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
#[path = "../test/listing.rs"]
mod tests;
