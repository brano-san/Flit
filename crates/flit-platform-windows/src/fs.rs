use crate::walk_index::Handle;
use flit_core::desktop_ini::DesktopIni;
use flit_core::listing::{attr, DirStats, Meta};
use flit_core::path::PathKind;
use flit_core::{FsError, FsErrorKind, FsPath, ListingBuilder};
use flit_platform::Fs;
use std::cell::RefCell;
use std::mem::offset_of;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
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
        ERROR_NOT_READY | ERROR_NO_MEDIA_IN_DRIVE => FsErrorKind::NoMedia,
        ERROR_BAD_NETPATH | ERROR_BAD_NET_NAME | ERROR_NETNAME_DELETED | ERROR_UNEXP_NET_ERR | ERROR_DEV_NOT_EXIST => FsErrorKind::Unavailable,
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

fn push_streams(dir: &FsPath, name: &[u16], file: Meta, out: &mut ListingBuilder<'_>) {
    let mut path = dir.to_wide(true);
    path.pop();
    if path.last() != Some(&(b'\\' as u16)) {
        path.push(b'\\' as u16);
    }
    path.extend_from_slice(name);
    path.push(0);
    let mut data = WIN32_FIND_STREAM_DATA::default();
    let Ok(h) = (unsafe { FindFirstStreamW(PCWSTR(path.as_ptr()), FindStreamInfoStandard, &mut data as *mut _ as *mut _, None) }) else { return };
    loop {
        let full = &data.cStreamName[..data.cStreamName.iter().position(|&c| c == 0).unwrap_or(data.cStreamName.len())];
        let stream = full.strip_prefix(&[b':' as u16]).and_then(|s| s.strip_suffix(&":$DATA".encode_utf16().collect::<Vec<u16>>()[..])).unwrap_or_default();
        if !stream.is_empty() {
            let shown: Vec<u16> = name.iter().copied().chain(Some(b':' as u16)).chain(stream.iter().copied()).collect();
            out.push_utf16(&shown, Meta { size: data.StreamSize as u64, attrs: (file.attrs & !attr::LINK) | attr::STREAM, ..file });
        }
        if unsafe { FindNextStreamW(h, &mut data as *mut _ as *mut _) }.is_err() {
            break;
        }
    }
    unsafe {
        let _ = FindClose(h);
    }
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
        let mut free = None;
        unsafe {
            if GetDriveTypeW(PCWSTR(root.as_ptr())) == DRIVE_FIXED {
                let (mut total, mut avail) = (0u64, 0u64);
                if GetDiskFreeSpaceExW(PCWSTR(root.as_ptr()), Some(&mut avail), Some(&mut total), None).is_ok() {
                    meta.size = total;
                    meta.attrs |= attr::SIZE_KNOWN;
                    free = Some(avail);
                }
            }
        }
        out.push(&name, meta);
        if let Some(free) = free {
            let fs = FsPath::parse_bytes(&[b'A' + i, b':', b'\\']).and_then(|p| crate::engine::volume_file_system(&p)).unwrap_or_default();
            out.listing.push_drive_info(free, &fs);
        }
    }
    for distro in crate::places::wsl_distros() {
        out.push(distro.as_bytes(), Meta { attrs: attr::DIR | attr::VOLUME, ..Default::default() });
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

fn scan(dir: &Handle, keep_going: &dyn Fn() -> bool, mut subdir: impl FnMut(&[u16])) -> (u64, u64) {
    SCAN_BUFFER.with_borrow_mut(|buf| {
        let (mut bytes, mut files) = (0, 0);
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
                    files += 1;
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
        (bytes, files)
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
    files: AtomicU64,
    folders: AtomicU64,
    partial: AtomicBool,
    open: AtomicUsize,
    keep_going: &'a (dyn Fn() -> bool + Sync),
    report: &'a (dyn Fn(u32, DirStats) + Sync),
}

impl Tree<'_> {
    fn enter(&self, open: impl FnOnce() -> Option<Handle>) -> Option<Handle> {
        (self.keep_going)().then(open).flatten()
    }

    fn visit<'s>(&'s self, s: &rayon::Scope<'s>, dir: Option<Handle>) {
        if let Some(parent) = dir {
            let mut subdirs = Subdirs { parent, names: Vec::new(), ends: Vec::new() };
            let (bytes, files) = scan(&subdirs.parent, self.keep_going, |name| {
                subdirs.names.extend_from_slice(name);
                subdirs.ends.push(subdirs.names.len());
            });
            self.bytes.fetch_add(bytes, Ordering::Relaxed);
            self.files.fetch_add(files, Ordering::Relaxed);
            self.folders.fetch_add(subdirs.ends.len() as u64, Ordering::Relaxed);
            self.open.fetch_add(subdirs.ends.len(), Ordering::Relaxed);
            let subdirs = Arc::new(subdirs);
            for k in 0..subdirs.ends.len() {
                let subdirs = subdirs.clone();
                s.spawn(move |s| self.visit(s, self.enter(move || subdirs.open(k))));
            }
        } else if (self.keep_going)() {
            self.partial.store(true, Ordering::Relaxed);
        }
        if self.open.fetch_sub(1, Ordering::AcqRel) == 1 {
            let count = |a: &AtomicU64| a.load(Ordering::Relaxed).min(u32::MAX as u64) as u32;
            (self.report)(self.idx, DirStats { size: self.bytes.load(Ordering::Relaxed), folders: count(&self.folders), files: count(&self.files), partial: self.partial.load(Ordering::Relaxed) });
        }
    }
}

const SERIAL_BUFFERS: usize = 64;
const PARALLEL_BUFFER: usize = 1 << 20;
const LIST_WORKERS: usize = 6;

fn query_dir(h: HANDLE, buf: &mut [u64], first: bool) -> Result<bool, FsError> {
    let mut iosb = IO_STATUS_BLOCK::default();
    let st = unsafe { NtQueryDirectoryFile(h, None, None, None, &mut iosb, buf.as_mut_ptr() as _, (buf.len() * 8) as u32, FileIdBothDirectoryInformation, false, None, first) };
    if st == STATUS_NO_MORE_FILES || (first && st == STATUS_NO_SUCH_FILE) {
        return Ok(false);
    }
    if st.is_err() {
        return Err(if first && st == STATUS_INVALID_PARAMETER { fs_error(ERROR_DIRECTORY) } else { nt_error(st) });
    }
    Ok(true)
}

type EntryVisit<'a> = dyn FnMut(&[u16], Meta, u64, i64) + 'a;

fn each_entry(buf: &[u64], f: &mut EntryVisit) {
    let base = buf.as_ptr() as *const u8;
    let mut off = 0usize;
    loop {
        let e = unsafe { base.add(off) } as *const FILE_ID_BOTH_DIR_INFORMATION;
        let (next, name, meta, id, accessed) = unsafe {
            let name_at = base.add(off + offset_of!(FILE_ID_BOTH_DIR_INFORMATION, FileName)) as *const u16;
            let name = std::slice::from_raw_parts(name_at, (*e).FileNameLength as usize / 2);
            let attrs = map_attrs((*e).FileAttributes, (*e).EaSize);
            ((*e).NextEntryOffset, name, Meta { size: (*e).EndOfFile as u64, modified: (*e).LastWriteTime, created: (*e).CreationTime, attrs }, (*e).FileId as u64, (*e).LastAccessTime)
        };
        if name != [b'.' as u16] && name != [b'.' as u16, b'.' as u16] {
            f(name, meta, id, accessed);
        }
        if next == 0 {
            return;
        }
        off += next as usize;
    }
}

fn list_part(buf: &[u64], case: flit_core::sort::CaseMode, tagging: &Option<(Arc<flit_core::tags::Tags>, u32)>) -> (flit_core::listing::Listing, flit_core::sort::SortKeys) {
    let mut listing = flit_core::listing::Listing::default();
    let mut keys = flit_core::sort::SortKeys::new(case);
    each_entry(buf, &mut |name, meta, id, accessed| {
        let i = listing.push_utf16(name, meta);
        listing.set_accessed(i, accessed);
        keys.push(listing.name(i));
        if let Some((tags, volume)) = tagging {
            listing.set_tags(i, tags.mask(flit_core::tags::FileKey { volume: *volume, id }));
        }
    });
    (listing, keys)
}

fn list_parallel(h: HANDLE, out: &mut ListingBuilder<'_>) -> Result<(), FsError> {
    use std::collections::BTreeMap;
    use std::sync::mpsc;
    let workers = std::thread::available_parallelism().map_or(1, |n| n.get()).clamp(1, LIST_WORKERS);
    let (case, tagging) = (out.keys.case(), out.tagging());
    let (job_tx, job_rx) = mpsc::sync_channel::<(usize, Vec<u64>)>(workers * 2);
    let job_rx = std::sync::Mutex::new(job_rx);
    let (done_tx, done_rx) = mpsc::channel();
    std::thread::scope(|s| {
        for _ in 0..workers {
            let (job_rx, done_tx, tagging) = (&job_rx, done_tx.clone(), &tagging);
            s.spawn(move || loop {
                let job = job_rx.lock().unwrap_or_else(|e| e.into_inner()).recv();
                let Ok((seq, buf)) = job else { return };
                let part = list_part(&buf, case, tagging);
                if done_tx.send((seq, buf, part)).is_err() {
                    return;
                }
            });
        }
        drop(done_tx);
        let mut free = Vec::new();
        let mut ready = BTreeMap::new();
        let (mut sent, mut next, mut reading) = (0usize, 0usize, true);
        let mut result = Ok(());
        while reading || next < sent {
            while reading && sent - next < workers * 3 {
                let mut buf = free.pop().unwrap_or_else(|| vec![0u64; PARALLEL_BUFFER / 8]);
                match query_dir(h, &mut buf, false) {
                    Ok(true) => {
                        if job_tx.send((sent, buf)).is_err() {
                            reading = false;
                        }
                        sent += 1;
                    }
                    Ok(false) => reading = false,
                    Err(e) => {
                        result = Err(e);
                        reading = false;
                    }
                }
            }
            if next == sent {
                break;
            }
            let Ok((seq, buf, part)) = done_rx.recv() else { break };
            free.push(buf);
            ready.insert(seq, part);
            while let Some((listing, keys)) = ready.remove(&next) {
                out.append(listing, keys);
                next += 1;
                if !out.batch_done() {
                    reading = false;
                    sent = next;
                    break;
                }
            }
        }
        drop(job_tx);
        result
    })
}

impl Fs for NtFs {
    fn read_dir(&self, dir: &FsPath, out: &mut ListingBuilder<'_>) -> Result<(), FsError> {
        match dir.kind() {
            PathKind::Computer => return volumes(out),
            PathKind::RecycleBin => return crate::recycle::list(&crate::recycle::roots(), out),
            PathKind::Shell => return crate::namespace::read_dir(dir, out),
            _ if flit_core::library::is_library(dir) => return crate::library::list(dir, out),
            _ => {}
        }
        if let Some(listed) = flit_archive::read_dir(dir, out, <crate::Windows as flit_platform::Platform>::utc_offset_minutes()) {
            return listed;
        }
        let h = match open_dir(dir, FILE_LIST_DIRECTORY.0, false) {
            Err(e) if crate::network::needs_logon(dir, &e) && crate::network::logon(dir) => open_dir(dir, FILE_LIST_DIRECTORY.0, false)?,
            opened => opened?,
        };
        if let Some(v) = (out.wants_ids() && dir.kind() == PathKind::Drive).then(|| crate::tags::volume_serial(h)).flatten() {
            out.set_volume(v);
        }
        let mut buf = vec![0u64; BUFFER / 8];
        let mut reads = 0;
        let result = loop {
            if reads == SERIAL_BUFFERS && !out.wants_streams() {
                break list_parallel(h, out);
            }
            match query_dir(h, &mut buf, reads == 0) {
                Ok(true) => {}
                Ok(false) => break Ok(()),
                Err(e) => break Err(e),
            }
            reads += 1;
            each_entry(&buf, &mut |name, meta, id, accessed| {
                out.push_utf16_id(name, meta, id);
                out.set_accessed(accessed);
                if out.wants_streams() && meta.attrs & attr::DIR == 0 {
                    push_streams(dir, name, meta, out);
                }
            });
            if !out.batch_done() {
                break Ok(());
            }
        };
        unsafe {
            let _ = CloseHandle(h);
        }
        result
    }

    fn hash(&self, file: &FsPath, kinds: &[flit_core::digest::HashKind], job: &flit_platform::Job) -> Result<Vec<String>, flit_platform::PlatformError> {
        crate::digest::hash_file(file, kinds, job)
    }

    fn desktop_ini(&self, dir: &FsPath) -> Option<DesktopIni> {
        crate::shell::desktop_ini(dir)
    }

    fn remote(&self, path: &FsPath) -> bool {
        crate::network::remote(path)
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

    fn volume_size(&self, path: &FsPath) -> u64 {
        let mut total = 0u64;
        let ok = unsafe { GetDiskFreeSpaceExW(PCWSTR(path.to_wide(true).as_ptr()), None, Some(&mut total), None) }.is_ok();
        if ok { total } else { 0 }
    }

    fn file_system(&self, path: &FsPath) -> Option<String> {
        crate::engine::volume_file_system(path)
    }

    fn cluster_size(&self, path: &FsPath) -> u64 {
        let wide = path.to_wide(true);
        let mut root = [0u16; 1024];
        let (mut sectors, mut bytes, mut free, mut total) = (0u32, 0u32, 0u32, 0u32);
        unsafe {
            if GetVolumePathNameW(PCWSTR(wide.as_ptr()), &mut root).is_err() || GetDiskFreeSpaceW(PCWSTR(root.as_ptr()), Some(&mut sectors), Some(&mut bytes), Some(&mut free), Some(&mut total)).is_err() {
                return 0;
            }
        }
        sectors as u64 * bytes as u64
    }

    fn dir_sizes(&self, dir: &FsPath, items: &[(u32, Vec<u8>)], keep_going: &(dyn Fn() -> bool + Sync), report: &(dyn Fn(u32, DirStats) + Sync)) {
        let trees: Vec<Tree> = items
            .iter()
            .map(|&(idx, _)| Tree { idx, bytes: AtomicU64::new(0), files: AtomicU64::new(0), folders: AtomicU64::new(0), partial: AtomicBool::new(false), open: AtomicUsize::new(1), keep_going, report })
            .collect();
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
