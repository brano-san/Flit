use flit_core::disk_changes::{Change, ChangeKind, Error, Filter, MAX_ROWS, Report};
use flit_core::FsPath;
use flit_core::i18n::S;
use flit_core::path::PathKind;
use std::collections::{HashMap, VecDeque};
use std::mem::size_of;
use windows::Win32::Foundation::*;
use windows::Win32::Storage::FileSystem::*;
use windows::Win32::System::IO::DeviceIoControl;
use windows::Win32::System::Ioctl::*;
use windows::core::PCWSTR;

const BYTES: usize = 256 * 1024;
const CREATE: u32 = USN_REASON_FILE_CREATE;
const DELETE: u32 = USN_REASON_FILE_DELETE;
const OLD_NAME: u32 = USN_REASON_RENAME_OLD_NAME;
const NEW_NAME: u32 = USN_REASON_RENAME_NEW_NAME;
const CLOSE: u32 = USN_REASON_CLOSE;
const MAX_OPEN_FILES: usize = 250_000;

fn failure(e: windows::core::Error) -> Error {
    match WIN32_ERROR::from_error(&e) {
        Some(ERROR_ACCESS_DENIED | ERROR_PRIVILEGE_NOT_HELD) => Error::Denied,
        Some(ERROR_JOURNAL_DELETE_IN_PROGRESS | ERROR_JOURNAL_NOT_ACTIVE | ERROR_JOURNAL_ENTRY_DELETED) => Error::Failed(S::DiskChangesJournalUnavailable.en().into()),
        _ => Error::Failed(crate::error_reason(&e)),
    }
}

pub fn read(root: &FsPath, filter: &Filter, cancelled: &dyn Fn() -> bool) -> Result<Report, Error> {
    if root.kind() != PathKind::Drive || !root.is_root() {
        return Err(Error::Failed(S::DiskChangesLocalOnly.en().into()));
    }
    let device: Vec<u16> = format!(r"\\.\{}", crate::walk_index::volume_id(root)).encode_utf16().chain(Some(0)).collect();
    let share = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;
    let handle = crate::walk_index::Handle(unsafe { CreateFileW(PCWSTR(device.as_ptr()), GENERIC_READ.0, share, None, OPEN_EXISTING, FILE_FLAGS_AND_ATTRIBUTES(0), None) }.map_err(failure)?);
    let journal = crate::mft::query_journal(handle.0).map_err(failure)?;
    let mut reports = Rows::new(filter);
    let mut next = journal.FirstUsn.max(journal.LowestValidUsn);
    let mut buf = vec![0u8; BYTES];
    let mask = (if filter.created { CREATE } else { 0 }) | (if filter.deleted { DELETE } else { 0 }) | (if filter.renamed { OLD_NAME | NEW_NAME } else { 0 });
    while next < journal.NextUsn && mask != 0 {
        if cancelled() {
            return Err(Error::Cancelled);
        }
        let input = READ_USN_JOURNAL_DATA_V0 { StartUsn: next, ReasonMask: mask | CLOSE, ReturnOnlyOnClose: 0, Timeout: 0, BytesToWaitFor: 0, UsnJournalID: journal.UsnJournalID };
        let mut got = 0;
        unsafe { DeviceIoControl(handle.0, FSCTL_READ_USN_JOURNAL, Some(&input as *const _ as _), size_of::<READ_USN_JOURNAL_DATA_V0>() as u32, Some(buf.as_mut_ptr().cast()), buf.len() as u32, Some(&mut got), None) }.map_err(failure)?;
        let Some(page) = buf.get(..got as usize) else {
            return Err(Error::Failed(S::DiskChangesJournalUnavailable.en().into()));
        };
        let Some((cursor, records)) = page.split_first_chunk::<8>() else {
            return Err(Error::Failed(S::DiskChangesJournalUnavailable.en().into()));
        };
        let after = i64::from_le_bytes(*cursor);
        for record in crate::mft::usn_records(records).take_while(|r| r.usn < journal.NextUsn) {
            reports.push(&record);
        }
        if after <= next {
            break;
        }
        next = after;
    }
    Ok(reports.finish())
}

struct Rows<'a> {
    filter: &'a Filter,
    text: String,
    rows: VecDeque<Change>,
    open: HashMap<u64, u32>,
    overflow: bool,
}

impl<'a> Rows<'a> {
    fn new(filter: &'a Filter) -> Self {
        Self { filter, text: filter.name.to_lowercase(), rows: VecDeque::new(), open: HashMap::new(), overflow: false }
    }

    fn push(&mut self, r: &crate::mft::UsnRecord<'_>) {
        let events = r.reason & (CREATE | DELETE | OLD_NAME | NEW_NAME);
        let previous = self.open.get(&r.frn).copied().unwrap_or(0);
        if r.reason & CLOSE != 0 {
            self.open.remove(&r.frn);
        } else if events != 0 {
            if self.open.len() >= MAX_OPEN_FILES {
                self.open.clear();
                self.overflow = true;
            }
            self.open.insert(r.frn, previous | events);
        }
        let fresh = events & !previous;
        if r.time < self.filter.since || r.time > self.filter.until || !r.name.len().is_multiple_of(2) {
            return;
        }
        let label = String::from_utf16_lossy(&r.name.chunks_exact(2).map(|b| u16::from_le_bytes([b[0], b[1]])).collect::<Vec<_>>());
        if label.is_empty() || !label.to_lowercase().contains(&self.text) {
            return;
        }
        let possible = [(CREATE, self.filter.created, ChangeKind::Created), (DELETE, self.filter.deleted, ChangeKind::Deleted), (OLD_NAME, self.filter.renamed, ChangeKind::RenameFrom), (NEW_NAME, self.filter.renamed, ChangeKind::RenameTo)];
        for (mask, enabled, kind) in possible {
            if !enabled || mask & fresh == 0 {
                continue;
            }
            if self.rows.len() >= MAX_ROWS {
                self.overflow = true;
                self.rows.pop_front();
            }
            self.rows.push_back(Change { at: r.time, kind, name: label.clone(), id: r.frn, parent: r.parent });
        }
    }

    fn finish(self) -> Report {
        Report { changes: self.rows.into_iter().rev().collect(), truncated: self.overflow }
    }
}

#[cfg(test)]
#[path = "../test/disk_changes.rs"]
mod tests;

#[path = "disk_changes_elevated.rs"]
mod elevated;
pub use elevated::{from_command_line, read_elevated};
