use flit_core::listing::{attr, Meta};
use flit_core::path::PathKind;
use flit_core::{FsError, FsErrorKind, FsPath, ListingBuilder};
use flit_platform::Fs;
use windows::core::PCWSTR;
use windows::Wdk::Storage::FileSystem::{NtQueryDirectoryFile, FileIdBothDirectoryInformation, FILE_ID_BOTH_DIR_INFORMATION};
use windows::Win32::Foundation::*;
use windows::Win32::Storage::FileSystem::*;
use windows::Win32::System::Ioctl::{PropertyStandardQuery, StorageDeviceSeekPenaltyProperty, DEVICE_SEEK_PENALTY_DESCRIPTOR, IOCTL_STORAGE_QUERY_PROPERTY, STORAGE_PROPERTY_QUERY};
use windows::Win32::System::IO::{DeviceIoControl, IO_STATUS_BLOCK};

const BUFFER: usize = 64 * 1024;
const DRIVE_FIXED: u32 = 3;

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

pub(crate) fn map_attrs(a: u32) -> u32 {
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
    if a & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
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

impl Fs for NtFs {
    fn read_dir(&self, dir: &FsPath, out: &mut ListingBuilder<'_>) -> Result<(), FsError> {
        if dir.kind() == PathKind::Computer {
            return volumes(out);
        }
        let h = open_dir(dir, FILE_LIST_DIRECTORY.0, false)?;
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
                let e = unsafe { &*(base.add(off) as *const FILE_ID_BOTH_DIR_INFORMATION) };
                let name = unsafe { std::slice::from_raw_parts(e.FileName.as_ptr(), e.FileNameLength as usize / 2) };
                let dots = name == [b'.' as u16] || name == [b'.' as u16, b'.' as u16];
                if !dots {
                    let meta = Meta { size: e.EndOfFile as u64, modified: e.LastWriteTime, created: e.CreationTime, attrs: map_attrs(e.FileAttributes) };
                    out.push_utf16(name, meta);
                }
                if e.NextEntryOffset == 0 {
                    break;
                }
                off += e.NextEntryOffset as usize;
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
}
