use crate::err;
use crate::fs::{map_attrs, open_dir};
use crate::walk_index::{file_id, fixed_drives, volume_id, Handle, WalkIndex};
use flit_core::listing::attr;
use flit_core::{name, FsPath};
use flit_platform::{IndexSource, PlatformError};
use flit_search::index::{Checkpoint, IndexRecord, IndexSink};
use flit_search::proto::{VolumeInfo, VolumeState};
use std::collections::{HashMap, HashSet};
use std::mem::{offset_of, size_of};
use windows::core::PCWSTR;
use windows::Win32::Foundation::*;
use windows::Win32::Storage::FileSystem::*;
use windows::Win32::System::Ioctl::*;
use windows::Win32::System::IO::{DeviceIoControl, OVERLAPPED};

pub(crate) const FRN_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;
const FIRST_USER_RECORD: u64 = 16;
const IO_BUFFER: usize = 1 << 20;
const MFT_CHUNK: u64 = 4 << 20;
const SECTOR_STRIDE: usize = 512;
const IO_ALIGN: usize = 4096;
const ATTR_STANDARD_INFORMATION: u32 = 0x10;
const ATTR_DATA: u32 = 0x80;
const ATTR_REPARSE_POINT: u32 = 0xC0;
const ATTR_END: u32 = 0xFFFF_FFFF;
const RECORD_IN_USE: u16 = 1;

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at.checked_add(2)?)?.try_into().ok()?))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at.checked_add(4)?)?.try_into().ok()?))
}

fn u64_at(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(at..at.checked_add(8)?)?.try_into().ok()?))
}

fn i64_at(b: &[u8], at: usize) -> Option<i64> {
    u64_at(b, at).map(|v| v as i64)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct UsnRecord<'a> {
    pub frn: u64,
    pub parent: u64,
    pub usn: i64,
    pub time: i64,
    pub reason: u32,
    pub attrs: u32,
    pub name: &'a [u8],
}

impl UsnRecord<'_> {
    fn is_dir(&self) -> bool {
        self.attrs & FILE_ATTRIBUTE_DIRECTORY.0 != 0
    }

    fn name_into(&self, wide: &mut Vec<u16>, out: &mut Vec<u8>) {
        wide.clear();
        wide.extend(self.name.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])));
        out.clear();
        name::push_utf16(out, wide);
    }
}

fn narrow_id(rec: &[u8], at: usize) -> Option<Option<u64>> {
    let low = u64_at(rec, at)?;
    let high = u64_at(rec, at + 8)?;
    Some((high == 0).then_some(low))
}

fn usn_record(buf: &[u8]) -> Option<(Option<UsnRecord<'_>>, usize)> {
    let len = u32_at(buf, 0)? as usize;
    if len < 8 || len > buf.len() {
        return None;
    }
    let rec = &buf[..len];
    let parsed = match u16_at(rec, 4)? {
        2 => {
            type R = USN_RECORD_V2;
            let (at, n) = (u16_at(rec, offset_of!(R, FileNameOffset))? as usize, u16_at(rec, offset_of!(R, FileNameLength))? as usize);
            Some(UsnRecord {
                frn: u64_at(rec, offset_of!(R, FileReferenceNumber))?,
                parent: u64_at(rec, offset_of!(R, ParentFileReferenceNumber))?,
                usn: i64_at(rec, offset_of!(R, Usn))?,
                time: i64_at(rec, offset_of!(R, TimeStamp))?,
                reason: u32_at(rec, offset_of!(R, Reason))?,
                attrs: u32_at(rec, offset_of!(R, FileAttributes))?,
                name: rec.get(at..at + n)?,
            })
        }
        3 => {
            type R = USN_RECORD_V3;
            let (at, n) = (u16_at(rec, offset_of!(R, FileNameOffset))? as usize, u16_at(rec, offset_of!(R, FileNameLength))? as usize);
            let ids = narrow_id(rec, offset_of!(R, FileReferenceNumber))?.zip(narrow_id(rec, offset_of!(R, ParentFileReferenceNumber))?);
            match ids {
                Some((frn, parent)) => Some(UsnRecord {
                    frn,
                    parent,
                    usn: i64_at(rec, offset_of!(R, Usn))?,
                    time: i64_at(rec, offset_of!(R, TimeStamp))?,
                    reason: u32_at(rec, offset_of!(R, Reason))?,
                    attrs: u32_at(rec, offset_of!(R, FileAttributes))?,
                    name: rec.get(at..at + n)?,
                }),
                None => None,
            }
        }
        _ => None,
    };
    Some((parsed, len))
}

pub(crate) fn usn_records(mut buf: &[u8]) -> impl Iterator<Item = UsnRecord<'_>> {
    std::iter::from_fn(move || loop {
        let (parsed, len) = usn_record(buf)?;
        buf = &buf[len..];
        if parsed.is_some() {
            return parsed;
        }
    })
}

pub(crate) struct Hidden {
    root: u64,
    dirs: HashSet<u64>,
}

impl Hidden {
    pub fn new(root: u64) -> Self {
        Self { root, dirs: HashSet::new() }
    }

    pub fn admit(&mut self, frn: u64, parent: u64, dir: bool) -> bool {
        if frn == self.root {
            return false;
        }
        let metafile = frn & FRN_MASK < FIRST_USER_RECORD;
        let under_metafile = parent != self.root && parent & FRN_MASK < FIRST_USER_RECORD;
        let hidden = metafile || under_metafile || self.dirs.contains(&parent);
        if hidden && dir {
            self.dirs.insert(frn);
        }
        !hidden
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Change {
    Remove(u64),
    Upsert { frn: u64, parent: u64, name: Vec<u8>, attrs: u32, time: i64 },
}

#[derive(Default)]
pub(crate) struct Coalesce {
    slot: HashMap<u64, usize>,
    changes: Vec<Option<Change>>,
    wide: Vec<u16>,
}

impl Coalesce {
    pub fn push(&mut self, r: &UsnRecord<'_>, hidden: &mut Hidden) {
        if !hidden.admit(r.frn, r.parent, r.is_dir()) {
            return;
        }
        let renaming_away = r.reason & USN_REASON_RENAME_OLD_NAME != 0 && r.reason & USN_REASON_RENAME_NEW_NAME == 0;
        let change = if r.reason & USN_REASON_FILE_DELETE != 0 {
            Some(Change::Remove(r.frn))
        } else if renaming_away {
            None
        } else {
            let mut name = Vec::new();
            r.name_into(&mut self.wide, &mut name);
            Some(Change::Upsert { frn: r.frn, parent: r.parent, name, attrs: r.attrs, time: r.time })
        };
        match self.slot.get(&r.frn) {
            Some(&k) => self.changes[k] = change,
            None => {
                self.slot.insert(r.frn, self.changes.len());
                self.changes.push(change);
            }
        }
    }

    pub fn finish(self) -> impl Iterator<Item = Change> {
        self.changes.into_iter().flatten()
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct RecordMeta {
    pub sequence: u16,
    pub base: u64,
    pub size: Option<u64>,
    pub modified: Option<i64>,
    pub reparse_tag: u32,
}

pub(crate) fn fixup(rec: &mut [u8]) -> bool {
    let (Some(at), Some(count)) = (u16_at(rec, 4), u16_at(rec, 6)) else { return false };
    let (at, count) = (at as usize, count as usize);
    if count < 2 || at + count * 2 > rec.len() || (count - 1) * SECTOR_STRIDE > rec.len() {
        return false;
    }
    let check = [rec[at], rec[at + 1]];
    for i in 1..count {
        let end = i * SECTOR_STRIDE - 2;
        if rec[end..end + 2] != check {
            return false;
        }
        rec[end] = rec[at + 2 * i];
        rec[end + 1] = rec[at + 2 * i + 1];
    }
    true
}

fn attributes(rec: &[u8]) -> impl Iterator<Item = (u32, &[u8])> {
    let used = u32_at(rec, 24).map_or(0, |u| (u as usize).min(rec.len()));
    let mut at = u16_at(rec, 20).map_or(usize::MAX, |a| a as usize);
    std::iter::from_fn(move || {
        let ty = u32_at(rec, at)?;
        let len = u32_at(rec, at + 4)? as usize;
        if ty == ATTR_END || len < 16 || at + len > used {
            return None;
        }
        let a = &rec[at..at + len];
        at += len;
        Some((ty, a))
    })
}

fn resident_value(a: &[u8]) -> Option<&[u8]> {
    if a[8] != 0 {
        return None;
    }
    let len = u32_at(a, 16)? as usize;
    let at = u16_at(a, 20)? as usize;
    a.get(at..at.checked_add(len)?)
}

fn unnamed(a: &[u8]) -> bool {
    a[9] == 0
}

fn first_extent(a: &[u8]) -> bool {
    a[8] != 0 && i64_at(a, 16) == Some(0)
}

pub(crate) fn parse_record(rec: &[u8]) -> Option<RecordMeta> {
    if rec.get(..4)? != b"FILE" || u16_at(rec, 22)? & RECORD_IN_USE == 0 {
        return None;
    }
    let mut m = RecordMeta { sequence: u16_at(rec, 16)?, base: u64_at(rec, 32)? & FRN_MASK, ..Default::default() };
    for (ty, a) in attributes(rec) {
        match ty {
            ATTR_STANDARD_INFORMATION => m.modified = resident_value(a).and_then(|v| i64_at(v, 8)).or(m.modified),
            ATTR_DATA if unnamed(a) && a[8] == 0 => m.size = resident_value(a).map(|v| v.len() as u64),
            ATTR_DATA if unnamed(a) && first_extent(a) => m.size = u64_at(a, 48),
            ATTR_REPARSE_POINT => m.reparse_tag = resident_value(a).and_then(|v| u32_at(v, 0)).unwrap_or(0),
            _ => {}
        }
    }
    Some(m)
}

pub(crate) fn data_runs(rec: &[u8]) -> Option<&[u8]> {
    let (_, a) = attributes(rec).find(|&(ty, a)| ty == ATTR_DATA && unnamed(a) && first_extent(a))?;
    a.get(u16_at(a, 32)? as usize..)
}

fn le_unsigned(b: &[u8]) -> u64 {
    b.iter().rev().fold(0, |v, &x| (v << 8) | x as u64)
}

fn le_signed(b: &[u8]) -> i64 {
    let shift = 64 - 8 * b.len() as u32;
    ((le_unsigned(b) << shift) as i64) >> shift
}

pub(crate) fn runs(list: &[u8]) -> Option<Vec<(Option<u64>, u64)>> {
    let mut out = Vec::new();
    let mut at = 0usize;
    let mut lcn = 0i64;
    loop {
        let head = *list.get(at)?;
        if head == 0 {
            return Some(out);
        }
        let (len_size, off_size) = ((head & 0xF) as usize, (head >> 4) as usize);
        if len_size == 0 || len_size > 8 || off_size > 8 {
            return None;
        }
        let len = le_unsigned(list.get(at + 1..at + 1 + len_size)?);
        let off = list.get(at + 1 + len_size..at + 1 + len_size + off_size)?;
        at += 1 + len_size + off_size;
        if off_size == 0 {
            out.push((None, len));
            continue;
        }
        lcn = lcn.checked_add(le_signed(off))?;
        if lcn < 0 {
            return None;
        }
        out.push((Some(lcn as u64), len));
    }
}

struct Aligned {
    raw: Vec<u8>,
    at: usize,
    len: usize,
}

impl Aligned {
    fn new(len: usize) -> Self {
        let raw = vec![0u8; len + IO_ALIGN];
        let at = raw.as_ptr().align_offset(IO_ALIGN);
        Self { raw, at, len }
    }

    fn bytes(&mut self, len: usize) -> &mut [u8] {
        &mut self.raw[self.at..self.at + len.min(self.len)]
    }
}

fn read_at(volume: HANDLE, offset: u64, buf: &mut [u8]) -> Result<(), PlatformError> {
    let mut ov = OVERLAPPED::default();
    ov.Anonymous.Anonymous.Offset = offset as u32;
    ov.Anonymous.Anonymous.OffsetHigh = (offset >> 32) as u32;
    let mut got = 0u32;
    unsafe { ReadFile(volume, Some(buf), Some(&mut got), Some(&mut ov)) }.map_err(err)?;
    if got as usize != buf.len() {
        return Err(PlatformError::new(-1, "short read from the volume"));
    }
    Ok(())
}

fn volume_data(volume: HANDLE) -> Result<NTFS_VOLUME_DATA_BUFFER, PlatformError> {
    let mut vd = NTFS_VOLUME_DATA_BUFFER::default();
    let mut got = 0u32;
    unsafe { DeviceIoControl(volume, FSCTL_GET_NTFS_VOLUME_DATA, None, 0, Some(&mut vd as *mut _ as _), size_of::<NTFS_VOLUME_DATA_BUFFER>() as u32, Some(&mut got), None) }.map_err(err)?;
    Ok(vd)
}

fn query_journal(volume: HANDLE) -> windows::core::Result<USN_JOURNAL_DATA_V0> {
    let mut j = USN_JOURNAL_DATA_V0::default();
    let mut got = 0u32;
    unsafe { DeviceIoControl(volume, FSCTL_QUERY_USN_JOURNAL, None, 0, Some(&mut j as *mut _ as _), size_of::<USN_JOURNAL_DATA_V0>() as u32, Some(&mut got), None) }?;
    Ok(j)
}

#[derive(Debug, Clone, Copy, Default)]
struct Meta {
    size: u64,
    modified: i64,
    tag: u32,
    sequence: u16,
    known: bool,
    sized: bool,
}

fn cancelled() -> PlatformError {
    PlatformError::new(-1, "cancelled")
}

fn read_mft<S: IndexSink>(volume: HANDLE, sink: &mut S) -> Result<Vec<Meta>, PlatformError> {
    let vd = volume_data(volume)?;
    let cluster = vd.BytesPerCluster as u64;
    let rec_size = vd.BytesPerFileRecordSegment as usize;
    if cluster == 0 || rec_size < 2 * SECTOR_STRIDE || rec_size % SECTOR_STRIDE != 0 || vd.MftStartLcn <= 0 {
        return Err(PlatformError::new(-1, "unexpected NTFS geometry"));
    }
    let count = (vd.MftValidDataLength.max(0) as u64 / rec_size as u64) as usize;
    let chunk = (MFT_CHUNK / cluster).max(rec_size as u64 / cluster + 1);
    let mut buf = Aligned::new((chunk * cluster) as usize);
    let first = (rec_size as u64).div_ceil(cluster) * cluster;
    let rec0 = buf.bytes(first as usize);
    read_at(volume, vd.MftStartLcn as u64 * cluster, rec0)?;
    let rec0 = &mut rec0[..rec_size];
    let extents = if fixup(rec0) { data_runs(rec0).and_then(runs) } else { None };
    let extents = extents.ok_or_else(|| PlatformError::new(-1, "cannot decode the $MFT data runs"))?;
    let mut meta = vec![Meta::default(); count];
    let mut extension: Vec<(usize, u64)> = Vec::new();
    let mut pending: Vec<u8> = Vec::with_capacity((chunk * cluster) as usize + rec_size);
    let mut next = 0usize;
    'extents: for (lcn, len) in extents {
        let Some(lcn) = lcn else { return Err(PlatformError::new(-1, "sparse $MFT extent")) };
        let mut done = 0u64;
        while done < len {
            let n = chunk.min(len - done);
            let bytes = buf.bytes((n * cluster) as usize);
            read_at(volume, (lcn + done) * cluster, bytes)?;
            pending.extend_from_slice(bytes);
            done += n;
            let whole = pending.len() / rec_size * rec_size;
            for rec in pending[..whole].chunks_exact_mut(rec_size) {
                if next >= count {
                    break 'extents;
                }
                if let Some(m) = if fixup(rec) { parse_record(rec) } else { None } {
                    if m.base == 0 {
                        meta[next] = Meta { size: m.size.unwrap_or(0), modified: m.modified.unwrap_or(0), tag: m.reparse_tag, sequence: m.sequence, known: true, sized: m.size.is_some() };
                    } else if let Some(size) = m.size {
                        extension.push((m.base as usize, size));
                    }
                }
                next += 1;
            }
            pending.drain(..whole);
            sink.progress(next as u64, 2 * count as u64);
            if sink.cancelled() {
                return Err(cancelled());
            }
        }
    }
    for (base, size) in extension {
        if let Some(m) = meta.get_mut(base).filter(|m| m.known && !m.sized) {
            m.size = size;
            m.sized = true;
        }
    }
    Ok(meta)
}

struct Ntfs {
    volume: Handle,
    root: Handle,
    root_id: u64,
    journal: u64,
    hidden: Hidden,
}

fn open_ntfs(root: &FsPath) -> Result<Ntfs, PlatformError> {
    let device: Vec<u16> = format!(r"\\.\{}", volume_id(root)).encode_utf16().chain(Some(0)).collect();
    let share = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;
    let volume = Handle(unsafe { CreateFileW(PCWSTR(device.as_ptr()), GENERIC_READ.0, share, None, OPEN_EXISTING, FILE_FLAGS_AND_ATTRIBUTES(0), None) }.map_err(err)?);
    let journal = query_journal(volume.0).map_err(err)?;
    let dir = open_dir(root, FILE_READ_ATTRIBUTES.0, false).map_err(|e| PlatformError::new(e.code, format!("cannot open {}", root.display())))?;
    let root_id = file_id(root).ok_or_else(|| PlatformError::new(-1, format!("cannot identify {}", root.display())))?;
    Ok(Ntfs { volume, root: Handle(dir), root_id, journal: journal.UsnJournalID, hidden: Hidden::new(root_id) })
}

impl Ntfs {
    fn prepare<S: IndexSink>(&self, sink: &mut S) -> Result<(USN_JOURNAL_DATA_V0, Vec<Meta>), PlatformError> {
        let journal = query_journal(self.volume.0).map_err(err)?;
        Ok((journal, read_mft(self.volume.0, sink)?))
    }

    fn emit<S: IndexSink>(&mut self, journal: USN_JOURNAL_DATA_V0, meta: &[Meta], sink: &mut S) -> Result<Checkpoint, PlatformError> {
        self.journal = journal.UsnJournalID;
        self.hidden = Hidden::new(self.root_id);
        sink.upsert(&IndexRecord { id: self.root_id, parent: self.root_id, name: b".", size: 0, modified: 0, attrs: attr::DIR });
        let total = meta.len() as u64;
        let mut input = MFT_ENUM_DATA_V0 { StartFileReferenceNumber: 0, LowUsn: 0, HighUsn: i64::MAX };
        let mut buf = vec![0u64; IO_BUFFER / 8];
        let (mut wide, mut leaf) = (Vec::new(), Vec::new());
        loop {
            let mut got = 0u32;
            let r = unsafe { DeviceIoControl(self.volume.0, FSCTL_ENUM_USN_DATA, Some(&input as *const _ as _), size_of::<MFT_ENUM_DATA_V0>() as u32, Some(buf.as_mut_ptr() as _), IO_BUFFER as u32, Some(&mut got), None) };
            if let Err(e) = r {
                if WIN32_ERROR::from_error(&e) == Some(ERROR_HANDLE_EOF) {
                    break;
                }
                return Err(err(e));
            }
            let bytes = unsafe { std::slice::from_raw_parts(buf.as_ptr() as *const u8, (got as usize).min(IO_BUFFER)) };
            let Some(after) = u64_at(bytes, 0) else { break };
            for r in usn_records(&bytes[8..]) {
                if !self.hidden.admit(r.frn, r.parent, r.is_dir()) {
                    continue;
                }
                let m = meta.get((r.frn & FRN_MASK) as usize).filter(|m| m.known && m.sequence == (r.frn >> 48) as u16).copied().unwrap_or_default();
                r.name_into(&mut wide, &mut leaf);
                let size = if r.is_dir() { 0 } else { m.size };
                sink.upsert(&IndexRecord { id: r.frn, parent: r.parent, name: &leaf, size, modified: m.modified, attrs: map_attrs(r.attrs, m.tag) });
            }
            if after & FRN_MASK <= input.StartFileReferenceNumber & FRN_MASK {
                break;
            }
            input.StartFileReferenceNumber = after;
            sink.progress(total + (after & FRN_MASK).min(total), 2 * total.max(1));
            if sink.cancelled() {
                return Err(cancelled());
            }
        }
        Ok(Checkpoint { journal: journal.UsnJournalID, next: journal.NextUsn })
    }

    fn stat(&self, frn: u64) -> Option<(u64, i64, u32)> {
        let desc = FILE_ID_DESCRIPTOR { dwSize: size_of::<FILE_ID_DESCRIPTOR>() as u32, Type: FileIdType, Anonymous: FILE_ID_DESCRIPTOR_0 { FileId: frn as i64 } };
        let share = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;
        let h = Handle(unsafe { OpenFileById(self.root.0, &desc, FILE_READ_ATTRIBUTES.0, share, None, FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT) }.ok()?);
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        unsafe { GetFileInformationByHandle(h.0, &mut info) }.ok()?;
        let mut tag = FILE_ATTRIBUTE_TAG_INFO::default();
        if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
            let _ = unsafe { GetFileInformationByHandleEx(h.0, FileAttributeTagInfo, &mut tag as *mut _ as _, size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32) };
        }
        let dir = info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY.0 != 0;
        let size = if dir { 0 } else { ((info.nFileSizeHigh as u64) << 32) | info.nFileSizeLow as u64 };
        let modified = ((info.ftLastWriteTime.dwHighDateTime as i64) << 32) | info.ftLastWriteTime.dwLowDateTime as i64;
        Some((size, modified, map_attrs(info.dwFileAttributes, tag.ReparseTag)))
    }

    fn changes<S: IndexSink>(&mut self, from: Checkpoint, sink: &mut S) -> Result<Option<Checkpoint>, PlatformError> {
        let Ok(journal) = query_journal(self.volume.0) else { return Ok(None) };
        if journal.UsnJournalID != from.journal || from.next < journal.FirstUsn || from.next < journal.LowestValidUsn {
            return Ok(None);
        }
        let mut pending = Coalesce::default();
        let mut next = from.next;
        let mut buf = vec![0u64; IO_BUFFER / 8];
        loop {
            let input = READ_USN_JOURNAL_DATA_V0 { StartUsn: next, ReasonMask: u32::MAX, ReturnOnlyOnClose: 0, Timeout: 0, BytesToWaitFor: 0, UsnJournalID: from.journal };
            let mut got = 0u32;
            let r = unsafe { DeviceIoControl(self.volume.0, FSCTL_READ_USN_JOURNAL, Some(&input as *const _ as _), size_of::<READ_USN_JOURNAL_DATA_V0>() as u32, Some(buf.as_mut_ptr() as _), IO_BUFFER as u32, Some(&mut got), None) };
            if let Err(e) = r {
                return match WIN32_ERROR::from_error(&e) {
                    Some(ERROR_JOURNAL_ENTRY_DELETED | ERROR_JOURNAL_DELETE_IN_PROGRESS | ERROR_JOURNAL_NOT_ACTIVE) => Ok(None),
                    _ => Err(err(e)),
                };
            }
            let bytes = unsafe { std::slice::from_raw_parts(buf.as_ptr() as *const u8, (got as usize).min(IO_BUFFER)) };
            let Some(after) = i64_at(bytes, 0) else { break };
            for r in usn_records(&bytes[8..]) {
                pending.push(&r, &mut self.hidden);
            }
            let caught_up = bytes.len() <= 8 || after <= next;
            next = next.max(after);
            if caught_up {
                break;
            }
        }
        for change in pending.finish() {
            match change {
                Change::Remove(frn) => sink.remove(frn),
                Change::Upsert { frn, parent, name, attrs, time } => {
                    let (size, modified, attrs) = self.stat(frn).unwrap_or((0, time, map_attrs(attrs, 0)));
                    sink.upsert(&IndexRecord { id: frn, parent, name: &name, size, modified, attrs });
                }
            }
        }
        Ok(Some(Checkpoint { journal: from.journal, next }))
    }
}

enum Mode {
    Mft(Box<Ntfs>),
    Walk,
}

pub struct MftIndex {
    volumes: Vec<(FsPath, VolumeInfo)>,
    walk: WalkIndex,
    modes: HashMap<String, Mode>,
}

fn describe(root: &FsPath) -> VolumeInfo {
    let wide = root.to_wide(true);
    let (mut label, mut fs) = ([0u16; 261], [0u16; 261]);
    let _ = unsafe { GetVolumeInformationW(PCWSTR(wide.as_ptr()), Some(&mut label), None, None, None, Some(&mut fs)) };
    let text = |w: &[u16]| String::from_utf16_lossy(&w[..w.iter().position(|&c| c == 0).unwrap_or(w.len())]);
    VolumeInfo { id: volume_id(root), label: text(&label), file_system: text(&fs), files: 0, state: VolumeState::Scanning { progress: 0.0 } }
}

impl MftIndex {
    pub fn with_roots(roots: Vec<FsPath>) -> Self {
        let volumes = roots.iter().map(|r| (r.clone(), describe(r))).collect();
        Self { volumes, walk: WalkIndex::with_roots(roots), modes: HashMap::new() }
    }

    fn volume(&self, id: &str) -> Result<&(FsPath, VolumeInfo), PlatformError> {
        self.volumes.iter().find(|(_, v)| v.id == id).ok_or_else(|| PlatformError::new(-1, format!("unknown volume {id}")))
    }

    fn ntfs(&self, id: &str) -> Result<Option<Ntfs>, PlatformError> {
        let (root, info) = self.volume(id)?;
        Ok(if info.file_system == "NTFS" { open_ntfs(root).ok() } else { None })
    }
}

impl IndexSource for MftIndex {
    fn new() -> Result<Self, PlatformError> {
        Ok(Self::with_roots(fixed_drives()))
    }

    fn volumes(&self) -> Result<Vec<VolumeInfo>, PlatformError> {
        Ok(self.volumes.iter().map(|(_, v)| v.clone()).collect())
    }

    fn scan<S: IndexSink>(&mut self, volume: &str, sink: &mut S) -> Result<Checkpoint, PlatformError> {
        self.modes.remove(volume);
        if let Some(mut n) = self.ntfs(volume)? {
            if let Ok((journal, meta)) = n.prepare(sink) {
                let cp = n.emit(journal, &meta, sink)?;
                self.modes.insert(volume.to_owned(), Mode::Mft(Box::new(n)));
                return Ok(cp);
            }
        }
        let cp = self.walk.scan(volume, sink)?;
        self.modes.insert(volume.to_owned(), Mode::Walk);
        Ok(cp)
    }

    fn changes<S: IndexSink>(&mut self, volume: &str, from: Checkpoint, sink: &mut S) -> Result<Option<Checkpoint>, PlatformError> {
        if !self.modes.contains_key(volume) {
            match self.ntfs(volume)? {
                Some(n) if n.journal == from.journal => {
                    self.modes.insert(volume.to_owned(), Mode::Mft(Box::new(n)));
                }
                _ => return Ok(None),
            }
        }
        match self.modes.get_mut(volume) {
            Some(Mode::Mft(n)) => n.changes(from, sink),
            Some(Mode::Walk) => self.walk.changes(volume, from, sink),
            None => Ok(None),
        }
    }
}

#[cfg(test)]
#[path = "../test/mft.rs"]
mod tests;
