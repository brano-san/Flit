use crate::fs::{map_attrs, DRIVE_FIXED};
use crate::walk_index::Handle;
use flit_core::listing::Meta;
use flit_core::path::PathKind;
use flit_core::tags::FileKey;
use flit_core::FsPath;
use flit_platform::Resolved;
use std::collections::HashMap;
use windows::core::PCWSTR;
use windows::Win32::Foundation::*;
use windows::Win32::Storage::FileSystem::*;

const DRIVE_REMOVABLE: u32 = 2;
const SHARE: FILE_SHARE_MODE = FILE_SHARE_MODE(FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0 | FILE_SHARE_DELETE.0);
const FLAGS: FILE_FLAGS_AND_ATTRIBUTES = FILE_FLAGS_AND_ATTRIBUTES(FILE_FLAG_BACKUP_SEMANTICS.0 | FILE_FLAG_OPEN_REPARSE_POINT.0);

fn open(path: &FsPath) -> Option<Handle> {
    let wide = path.to_wide(true);
    unsafe { CreateFileW(PCWSTR(wide.as_ptr()), FILE_READ_ATTRIBUTES.0, SHARE, None, OPEN_EXISTING, FLAGS, None) }.ok().map(Handle)
}

fn local_drive(letter: u8) -> bool {
    let root = [letter as u16, b':' as u16, b'\\' as u16, 0];
    matches!(unsafe { GetDriveTypeW(PCWSTR(root.as_ptr())) }, DRIVE_FIXED | DRIVE_REMOVABLE)
}

fn is_ntfs(h: &Handle) -> bool {
    let mut fs = [0u16; 16];
    unsafe { GetVolumeInformationByHandleW(h.0, None, None, None, None, Some(&mut fs)) }.is_ok() && fs.starts_with(&[b'N' as u16, b'T' as u16, b'F' as u16, b'S' as u16, 0])
}

fn info(h: &Handle) -> Option<BY_HANDLE_FILE_INFORMATION> {
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    unsafe { GetFileInformationByHandle(h.0, &mut info) }.ok().map(|()| info)
}

fn key_of(info: &BY_HANDLE_FILE_INFORMATION) -> FileKey {
    FileKey { volume: info.dwVolumeSerialNumber, id: ((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64 }
}

pub(crate) fn volume_serial(dir: HANDLE) -> Option<u32> {
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    unsafe { GetFileInformationByHandle(dir, &mut info) }.ok().map(|()| info.dwVolumeSerialNumber)
}

pub(crate) fn file_key(path: &FsPath) -> Option<FileKey> {
    if path.kind() != PathKind::Drive || !local_drive(path.as_bytes()[0]) {
        return None;
    }
    let h = open(path).filter(is_ntfs)?;
    info(&h).map(|i| key_of(&i))
}

fn time(t: FILETIME) -> i64 {
    ((t.dwHighDateTime as i64) << 32) | t.dwLowDateTime as i64
}

fn final_path(h: &Handle) -> Option<FsPath> {
    let mut buf = vec![0u16; 1024];
    loop {
        let n = unsafe { GetFinalPathNameByHandleW(h.0, &mut buf, GETFINALPATHNAMEBYHANDLE_FLAGS(FILE_NAME_NORMALIZED.0 | VOLUME_NAME_DOS.0)) } as usize;
        if n == 0 {
            return None;
        }
        if n < buf.len() {
            let w = &buf[..n];
            let w = w.strip_prefix(&[b'\\' as u16, b'\\' as u16, b'?' as u16, b'\\' as u16][..]).unwrap_or(w);
            return FsPath::from_utf16(w).filter(|p| p.kind() == PathKind::Drive);
        }
        buf.resize(n + 1, 0);
    }
}

fn volume_roots() -> HashMap<u32, Handle> {
    let mask = unsafe { GetLogicalDrives() };
    let mut roots = HashMap::new();
    for i in 0..26u8 {
        if mask & (1 << i) == 0 || !local_drive(b'A' + i) {
            continue;
        }
        let Some(h) = FsPath::parse(&format!("{}:\\", (b'A' + i) as char)).and_then(|r| open(&r)).filter(is_ntfs) else { continue };
        if let Some(info) = info(&h) {
            roots.entry(info.dwVolumeSerialNumber).or_insert(h);
        }
    }
    roots
}

fn resolve(root: &Handle, id: u64) -> Resolved {
    let desc = FILE_ID_DESCRIPTOR { dwSize: size_of::<FILE_ID_DESCRIPTOR>() as u32, Type: FileIdType, Anonymous: FILE_ID_DESCRIPTOR_0 { FileId: id as i64 } };
    let h = match unsafe { OpenFileById(root.0, &desc, FILE_READ_ATTRIBUTES.0, SHARE, None, FLAGS) } {
        Ok(h) => Handle(h),
        Err(e) if [ERROR_INVALID_PARAMETER, ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND].map(|c| c.to_hresult()).contains(&e.code()) => return Resolved::Gone,
        Err(_) => return Resolved::Unavailable,
    };
    let (Some(path), Some(i)) = (final_path(&h), info(&h)) else { return Resolved::Unavailable };
    let dir = i.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY.0 != 0;
    let size = if dir { 0 } else { ((i.nFileSizeHigh as u64) << 32) | i.nFileSizeLow as u64 };
    Resolved::Found(path, Meta { size, modified: time(i.ftLastWriteTime), created: time(i.ftCreationTime), attrs: map_attrs(i.dwFileAttributes, 0) })
}

pub(crate) fn resolve_files(keys: &[FileKey]) -> Vec<Resolved> {
    let roots = volume_roots();
    keys.iter().map(|k| roots.get(&k.volume).map_or(Resolved::Unavailable, |r| resolve(r, k.id))).collect()
}

#[cfg(test)]
#[path = "../test/tags.rs"]
mod tests;
