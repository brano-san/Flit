use crate::walk_index::Handle;
use flit_core::listing::{attr, Meta};
use flit_core::path::PathKind;
use flit_core::{FsError, FsErrorKind, FsPath, ListingBuilder};
use flit_platform::Fs;
use std::cell::RefCell;
use std::mem::offset_of;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use windows::core::{PCWSTR, PWSTR};
use windows::Wdk::Foundation::OBJECT_ATTRIBUTES;
use windows::Wdk::Storage::FileSystem::{
    FileFullDirectoryInformation, FileIdBothDirectoryInformation, NtOpenFile, NtQueryDirectoryFile, FILE_DIRECTORY_FILE, FILE_FULL_DIR_INFORMATION, FILE_ID_BOTH_DIR_INFORMATION, FILE_OPEN_FOR_BACKUP_INTENT,
    FILE_SYNCHRONOUS_IO_NONALERT,
};
use windows::Win32::Foundation::*;
use windows::Win32::Storage::FileSystem::*;
use windows::Win32::System::Ioctl::{PropertyStandardQuery, StorageDeviceSeekPenaltyProperty, DEVICE_SEEK_PENALTY_DESCRIPTOR, IOCTL_STORAGE_QUERY_PROPERTY, STORAGE_PROPERTY_QUERY};
use windows::Win32::System::IO::{DeviceIoControl, IO_STATUS_BLOCK};

const BUFFER: usize = 64 * 1024;
pub(crate) const DRIVE_FIXED: u32 = 3;

pub struct NtFs;

pub(crate) fn fs_error(code: WIN32_ERROR) -> FsError {
    let kind = match code {
        ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND | ERROR_INVALID_NAME | ERROR_BAD_PATHNAME => FsErrorKind::NotFound,
        ERROR_ACCESS_DENIED | ERROR_SHARING_VIOLATION => FsErrorKind::AccessDenied,
        ERROR_DIRECTORY => FsErrorKind::NotADirectory,
        ERROR_NOT_READY | ERROR_BAD_NETPATH | ERROR_BAD_NET_NAME | ERROR_NETNAME_DELETED | ERROR_UNEXP_NET_ERR | ERROR_DEV_NOT_EXIST | ERROR_NO_MEDIA_IN_DRIVE => FsErrorKind::Unavailable,
        _ => FsErrorKind::Other,
    };
    FsError { kind, code: code.0 as i64 }
}

fn nt_error(st: NTSTATUS) -> FsError {
    let code = unsafe { windows::Win32::Foundation::RtlNtStatusToDosError(st) };
    FsError { code: st.0 as u32 as i64, ..fs_error(WIN32_ERROR(code)) }
}

pub(crate) fn open_dir(dir: &FsPath, access: u32, overlapped: bool) -> Result<HANDLE, FsError> {
    let wide = dir.to_wide(true);
    let flags = if overlapped { FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED } else { FILE_FLAG_BACKUP_SEMANTICS };
    unsafe {
        CreateFileW(PCWSTR(wide.as_ptr()), access, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, None, OPEN_EXISTING, flags, None)
            .map_err(|e| fs_error(WIN32_ERROR::from_error(&e).unwrap_or(ERROR_GEN_FAILURE)))
    }
}

const REPARSE_TAG_NAME_SURROGATE: u32 = 0x2000_0000;

pub(crate) fn map_attrs(a: u32, reparse_tag: u32) -> u32 {
    let mut out = 0;
    if a & FILE_ATTRIBUTE_DIRECTORY.0 != 0 {
        out |= attr::DIR;
    }
    if a & FILE_ATTRIBUTE_HIDDEN.0 != 0 {
        out |= attr::HIDDEN;
    }
    if a & FILE_ATTRIBUTE_SYSTEM.0 != 0 {
        out |= attr::SYSTEM;
    }
    if a & FILE_ATTRIBUTE_READONLY.0 != 0 {
        out |= attr::READONLY;
    }
    if a & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 && reparse_tag & REPARSE_TAG_NAME_SURROGATE != 0 {
        out |= attr::LINK;
    }
    out
}

fn volumes(out: &mut ListingBuilder<'_>) -> Result<(), FsError> {
    let mask = unsafe { GetLogicalDrives() };
    for i in 0..26u8 {
        if mask & (1 << i) == 0 {
            continue;
        }
        let name = [b'A' + i, b':'];
        let root: Vec<u16> = [b'A' + i, b':', b'\\'].iter().map(|&c| c as u16).chain(Some(0)).collect();
        let mut meta = Meta { attrs: attr::DIR | attr::VOLUME, ..Default::default() };
        unsafe {
            if GetDriveTypeW(PCWSTR(root.as_ptr())) == DRIVE_FIXED {
                let mut total = 0u64;
                if GetDiskFreeSpaceExW(PCWSTR(root.as_ptr()), None, Some(&mut total), None).is_ok() {
                    meta.size = total;
                    meta.attrs |= attr::SIZE_KNOWN;
                }
            }
        }
        out.push(&name, meta);
    }
    out.batch_done();
    Ok(())
}

thread_local! {
    static SCAN_BUFFER: RefCell<Vec<u64>> = RefCell::new(vec![0; BUFFER / 8]);
}

pub(crate) fn open_relative(parent: &Handle, name: &[u16], access: u32, options: u32) -> Option<Handle> {
    let bytes = (name.len() * 2) as u16;
    let object = UNICODE_STRING { Length: bytes, MaximumLength: bytes, Buffer: PWSTR(name.as_ptr() as *mut u16) };
    let attrs = OBJECT_ATTRIBUTES { Length: size_of::<OBJECT_ATTRIBUTES>() as u32, RootDirectory: parent.0, ObjectName: &object, Attributes: OBJ_CASE_INSENSITIVE, ..Default::default() };
    let (mut h, mut iosb) = (HANDLE::default(), IO_STATUS_BLOCK::default());
    let share = (FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE).0;
    let st = unsafe { NtOpenFile(&mut h, access | SYNCHRONIZE.0, &attrs, &mut iosb, share, options | FILE_SYNCHRONOUS_IO_NONALERT.0) };
    st.is_ok().then(|| Handle(h))
}

fn open_child(parent: &Handle, name: &[u16]) -> Option<Handle> {
    open_relative(parent, name, FILE_LIST_DIRECTORY.0, FILE_DIRECTORY_FILE.0 | FILE_OPEN_FOR_BACKUP_INTENT.0)
}

fn scan(dir: &Handle, keep_going: &dyn Fn() -> bool, mut subdir: impl FnMut(&[u16])) -> u64 {
    SCAN_BUFFER.with_borrow_mut(|buf| {
        let mut bytes = 0;
        let mut first = true;
        while keep_going() {
            let mut iosb = IO_STATUS_BLOCK::default();
            let st = unsafe { NtQueryDirectoryFile(dir.0, None, None, None, &mut iosb, buf.as_mut_ptr() as _, BUFFER as u32, FileFullDirectoryInformation, false, None, first) };
            if st.is_err() {
                break;
            }
            first = false;
            let base = buf.as_ptr() as *const u8;
            let mut off = 0usize;
            loop {
                let e = unsafe { base.add(off) } as *const FILE_FULL_DIR_INFORMATION;
                let (next, attrs, tag, size, len) = unsafe { ((*e).NextEntryOffset, (*e).FileAttributes, (*e).EaSize, (*e).EndOfFile as u64, (*e).FileNameLength as usize / 2) };
                if attrs & FILE_ATTRIBUTE_DIRECTORY.0 == 0 {
                    bytes += size;
                } else if map_attrs(attrs, tag) & attr::LINK == 0 {
                    let name = unsafe { std::slice::from_raw_parts(base.add(off + offset_of!(FILE_FULL_DIR_INFORMATION, FileName)) as *const u16, len) };
                    if name != [b'.' as u16] && name != [b'.' as u16, b'.' as u16] {
                        subdir(name);
                    }
                }
                if next == 0 {
                    break;
                }
                off += next as usize;
            }
        }
        bytes
    })
}

struct Subdirs {
    parent: Handle,
    names: Vec<u16>,
    ends: Vec<usize>,
}

impl Subdirs {
    fn open(&self, k: usize) -> Option<Handle> {
        let start = k.checked_sub(1).map_or(0, |p| self.ends[p]);
        open_child(&self.parent, &self.names[start..self.ends[k]])
    }
}

struct Tree<'a> {
    idx: u32,
    bytes: AtomicU64,
    open: AtomicUsize,
    keep_going: &'a (dyn Fn() -> bool + Sync),
    report: &'a (dyn Fn(u32, u64) + Sync),
}

impl Tree<'_> {
    fn enter(&self, open: impl FnOnce() -> Option<Handle>) -> Option<Handle> {
        (self.keep_going)().then(open).flatten()
    }

    fn visit<'s>(&'s self, s: &rayon::Scope<'s>, dir: Option<Handle>) {
        if let Some(parent) = dir {
            let mut subdirs = Subdirs { parent, names: Vec::new(), ends: Vec::new() };
            let bytes = scan(&subdirs.parent, self.keep_going, |name| {
                subdirs.names.extend_from_slice(name);
                subdirs.ends.push(subdirs.names.len());
            });
            self.bytes.fetch_add(bytes, Ordering::Relaxed);
            self.open.fetch_add(subdirs.ends.len(), Ordering::Relaxed);
            let subdirs = Arc::new(subdirs);
            for k in 0..subdirs.ends.len() {
                let subdirs = subdirs.clone();
                s.spawn(move |s| self.visit(s, self.enter(move || subdirs.open(k))));
            }
        }
        if self.open.fetch_sub(1, Ordering::AcqRel) == 1 {
            (self.report)(self.idx, self.bytes.load(Ordering::Relaxed));
        }
    }
}

impl Fs for NtFs {
    fn read_dir(&self, dir: &FsPath, out: &mut ListingBuilder<'_>) -> Result<(), FsError> {
        match dir.kind() {
            PathKind::Computer => return volumes(out),
            PathKind::RecycleBin => return crate::recycle::list(&crate::recycle::roots(), out),
            _ => {}
        }
        let h = open_dir(dir, FILE_LIST_DIRECTORY.0, false)?;
        if let Some(v) = (out.wants_ids() && dir.kind() == PathKind::Drive).then(|| crate::tags::volume_serial(h)).flatten() {
            out.set_volume(v);
        }
        let mut buf = vec![0u64; BUFFER / 8];
        let mut first = true;
        let result = loop {
            let mut iosb = IO_STATUS_BLOCK::default();
            let st = unsafe { NtQueryDirectoryFile(h, None, None, None, &mut iosb, buf.as_mut_ptr() as _, BUFFER as u32, FileIdBothDirectoryInformation, false, None, first) };
            if st == STATUS_NO_MORE_FILES || (first && st == STATUS_NO_SUCH_FILE) {
                break Ok(());
            }
            if st.is_err() {
                break Err(nt_error(st));
            }
            first = false;
            let base = buf.as_ptr() as *const u8;
            let mut off = 0usize;
            loop {
                let e = unsafe { base.add(off) } as *const FILE_ID_BOTH_DIR_INFORMATION;
                let (next, name, meta, id) = unsafe {
                    let name_at = base.add(off + offset_of!(FILE_ID_BOTH_DIR_INFORMATION, FileName)) as *const u16;
                    let name = std::slice::from_raw_parts(name_at, (*e).FileNameLength as usize / 2);
                    let attrs = map_attrs((*e).FileAttributes, (*e).EaSize);
                    ((*e).NextEntryOffset, name, Meta { size: (*e).EndOfFile as u64, modified: (*e).LastWriteTime, created: (*e).CreationTime, attrs }, (*e).FileId as u64)
                };
                let dots = name == [b'.' as u16] || name == [b'.' as u16, b'.' as u16];
                if !dots {
                    out.push_utf16_id(name, meta, id);
                }
                if next == 0 {
                    break;
                }
                off += next as usize;
            }
            if !out.batch_done() {
                break Ok(());
            }
        };
        unsafe {
            let _ = CloseHandle(h);
        }
        result
    }

    fn seek_penalty(&self, path: &FsPath) -> Option<bool> {
        if path.kind() != PathKind::Drive {
            return None;
        }
        let letter = path.as_bytes()[0];
        let dev: Vec<u16> = r"\\.\".encode_utf16().chain([letter as u16, b':' as u16, 0]).collect();
        unsafe {
            let h = CreateFileW(PCWSTR(dev.as_ptr()), 0, FILE_SHARE_READ | FILE_SHARE_WRITE, None, OPEN_EXISTING, FILE_FLAGS_AND_ATTRIBUTES(0), None).ok()?;
            let query = STORAGE_PROPERTY_QUERY { PropertyId: StorageDeviceSeekPenaltyProperty, QueryType: PropertyStandardQuery, ..Default::default() };
            let mut desc = DEVICE_SEEK_PENALTY_DESCRIPTOR::default();
            let mut n = 0u32;
            let r = DeviceIoControl(
                h,
                IOCTL_STORAGE_QUERY_PROPERTY,
                Some(&query as *const _ as _),
                size_of::<STORAGE_PROPERTY_QUERY>() as u32,
                Some(&mut desc as *mut _ as _),
                size_of::<DEVICE_SEEK_PENALTY_DESCRIPTOR>() as u32,
                Some(&mut n),
                None,
            );
            let _ = CloseHandle(h);
            r.ok().map(|()| desc.IncursSeekPenalty)
        }
    }

    fn dir_sizes(&self, dir: &FsPath, items: &[(u32, Vec<u8>)], keep_going: &(dyn Fn() -> bool + Sync), report: &(dyn Fn(u32, u64) + Sync)) {
        let trees: Vec<Tree> = items.iter().map(|&(idx, _)| Tree { idx, bytes: AtomicU64::new(0), open: AtomicUsize::new(1), keep_going, report }).collect();
        let walk = || {
            rayon::scope(|s| {
                for (tree, (_, name)) in trees.iter().zip(items).rev() {
                    let path = dir.join(name);
                    s.spawn(move |s| tree.visit(s, tree.enter(|| open_dir(&path, FILE_LIST_DIRECTORY.0, false).ok().map(Handle))));
                }
            })
        };
        let threads = if self.seek_penalty(dir) == Some(true) { 1 } else { 0 };
        match rayon::ThreadPoolBuilder::new().num_threads(threads).thread_name(|k| format!("flit-sizes-{k}")).build() {
            Ok(pool) => pool.install(walk),
            Err(_) => walk(),
        }
    }
}

#[cfg(test)]
#[path = "../test/fs.rs"]
mod tests;
