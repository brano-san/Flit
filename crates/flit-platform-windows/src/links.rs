use crate::fileops::std_path;
use flit_core::path::PathKind;
use flit_core::provider::{LinkChoices, LinkKind};
use flit_core::FsPath;
use std::ffi::OsString;
use std::os::windows::ffi::OsStrExt;
use windows::core::{Interface, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Storage::FileSystem::*;
use windows::Win32::System::Com::{CoCreateInstance, IPersistFile, CLSCTX_INPROC_SERVER};
use windows::Win32::System::IO::DeviceIoControl;
use windows::Win32::System::Ioctl::FSCTL_SET_REPARSE_POINT;
use windows::Win32::System::SystemServices::IO_REPARSE_TAG_MOUNT_POINT;
use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};

fn error(code: WIN32_ERROR) -> windows::core::Error {
    code.to_hresult().into()
}

fn filesystem(path: &FsPath) -> bool {
    matches!(path.kind(), PathKind::Drive | PathKind::Unc | PathKind::Verbatim)
}

fn volume(path: &FsPath) -> Option<(Vec<u16>, bool)> {
    if !filesystem(path) {
        return None;
    }
    let mut root = [0u16; 32768];
    unsafe { GetVolumePathNameW(PCWSTR(path.to_wide(true).as_ptr()), &mut root) }.ok()?;
    let local = matches!(unsafe { GetDriveTypeW(PCWSTR(root.as_ptr())) }, 2 | 3 | 6);
    identify_volume(&root, local, |root, name| unsafe { GetVolumeNameForVolumeMountPointW(PCWSTR(root.as_ptr()), name) })
}

fn identify_volume(root: &[u16], local: bool, query: impl FnOnce(&[u16], &mut [u16]) -> windows::core::Result<()>) -> Option<(Vec<u16>, bool)> {
    let mut name = [0u16; 64];
    query(root, &mut name).ok()?;
    Some((name[..name.iter().position(|&c| c == 0)?].to_vec(), local))
}

fn same_volume(source_path: &FsPath, destination_path: &FsPath, source: Option<&(Vec<u16>, bool)>, destination: Option<&(Vec<u16>, bool)>) -> bool {
    filesystem(source_path) && filesystem(destination_path) && matches!((source, destination), (Some((a, _)), Some((b, _))) if a == b)
}

fn item_choices(source_path: &FsPath, destination_path: &FsPath, file: bool, directory: bool, source: Option<&(Vec<u16>, bool)>, destination: Option<&(Vec<u16>, bool)>) -> LinkChoices {
    if !filesystem(source_path) || !filesystem(destination_path) {
        return LinkChoices::default();
    }
    let same = same_volume(source_path, destination_path, source, destination);
    let local = source.is_some_and(|(_, local)| *local) && destination.is_some_and(|(_, local)| *local);
    LinkChoices([true, true, directory && same && local, file && same])
}

pub(crate) fn choices(items: &[FsPath], dest: &FsPath) -> LinkChoices {
    if items.is_empty() || !filesystem(dest) || !std_path(dest).is_dir() {
        return LinkChoices::default();
    }
    let destination = volume(dest);
    let mut result = [true; 4];
    for item in items {
        let Ok(meta) = std::fs::metadata(std_path(item)) else { return LinkChoices::default() };
        if !filesystem(item) {
            return LinkChoices::default();
        }
        let source = volume(item);
        let choices = item_choices(item, dest, meta.is_file(), meta.is_dir(), source.as_ref(), destination.as_ref());
        for (allowed, item) in result.iter_mut().zip(choices.0) {
            *allowed &= item;
        }
    }
    LinkChoices(result)
}

fn plain_target(target: &FsPath) -> Vec<u16> {
    let mut wide = target.to_wide(false);
    wide.pop();
    wide
}

fn junction_data(target: &FsPath) -> windows::core::Result<Vec<u8>> {
    let full = plain_target(target);
    let prefix: Vec<u16> = r"\\?\".encode_utf16().collect();
    let print = full.strip_prefix(prefix.as_slice()).unwrap_or(&full);
    let substitute: Vec<u16> = r"\??\".encode_utf16().chain(print.iter().copied()).collect();
    let data_length = 8 + (substitute.len() + print.len() + 2) * 2;
    if data_length + 8 > 16384 {
        return Err(error(ERROR_FILENAME_EXCED_RANGE));
    }
    let mut bytes = Vec::with_capacity(data_length + 8);
    bytes.extend_from_slice(&IO_REPARSE_TAG_MOUNT_POINT.to_le_bytes());
    for field in [data_length as u16, 0, 0, (substitute.len() * 2) as u16, ((substitute.len() + 1) * 2) as u16, (print.len() * 2) as u16] {
        bytes.extend_from_slice(&field.to_le_bytes());
    }
    for ch in substitute.iter().copied().chain([0]).chain(print.iter().copied()).chain([0]) {
        bytes.extend_from_slice(&ch.to_le_bytes());
    }
    Ok(bytes)
}

fn junction(target: &FsPath, path: &FsPath) -> windows::core::Result<()> {
    let data = junction_data(target)?;
    let wide = path.to_wide(true);
    unsafe { CreateDirectoryW(PCWSTR(wide.as_ptr()), None) }?;
    let created = (|| {
        let handle = unsafe { CreateFileW(PCWSTR(wide.as_ptr()), GENERIC_WRITE.0, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, None, OPEN_EXISTING, FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS, None) }?;
        let mut returned = 0;
        let result = unsafe { DeviceIoControl(handle, FSCTL_SET_REPARSE_POINT, Some(data.as_ptr() as _), data.len() as u32, None, 0, Some(&mut returned), None) };
        unsafe { let _ = CloseHandle(handle); }
        result
    })();
    if created.is_err() {
        unsafe { let _ = RemoveDirectoryW(PCWSTR(wide.as_ptr())); }
    }
    created
}

pub(crate) fn create(kind: LinkKind, target: &FsPath, path: &FsPath) -> windows::core::Result<()> {
    let parent = path.parent().ok_or_else(|| error(ERROR_INVALID_NAME))?;
    if kind != LinkKind::Shortcut && !choices(std::slice::from_ref(target), &parent).allows(kind) {
        return Err(error(ERROR_INVALID_PARAMETER));
    }
    let to = path.to_wide(true);
    match kind {
        LinkKind::Symbolic => {
            let flags = SYMBOLIC_LINK_FLAG_ALLOW_UNPRIVILEGED_CREATE | if std_path(target).is_dir() { SYMBOLIC_LINK_FLAG_DIRECTORY } else { SYMBOLIC_LINK_FLAGS(0) };
            let from: Vec<u16> = plain_target(target).into_iter().chain([0]).collect();
            if unsafe { CreateSymbolicLinkW(PCWSTR(to.as_ptr()), PCWSTR(from.as_ptr()), flags) } { Ok(()) } else { Err(windows::core::Error::from_thread()) }
        }
        LinkKind::Junction => junction(target, path),
        LinkKind::Hard => unsafe { CreateHardLinkW(PCWSTR(to.as_ptr()), PCWSTR(target.to_wide(true).as_ptr()), None) },
        LinkKind::Shortcut => unsafe {
            let reserved = std::fs::File::create_new(std_path(path)).map_err(|e| error(WIN32_ERROR(e.raw_os_error().unwrap_or(ERROR_GEN_FAILURE.0 as i32) as u32)))?;
            drop(reserved);
            let result = (|| {
                let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
                link.SetPath(PCWSTR(target.to_wide(false).as_ptr()))?;
                if let Some(dir) = target.parent() {
                    link.SetWorkingDirectory(PCWSTR(dir.to_wide(false).as_ptr()))?;
                }
                link.cast::<IPersistFile>()?.Save(PCWSTR(path.to_wide(false).as_ptr()), true)
            })();
            if result.is_err() {
                let _ = std::fs::remove_file(std_path(path));
            }
            result
        },
    }
}

struct Folder(HANDLE);

impl Drop for Folder {
    fn drop(&mut self) {
        unsafe { let _ = CloseHandle(self.0); }
    }
}

fn hold_folder(dir: &FsPath) -> windows::core::Result<(Folder, String)> {
    let wide = dir.to_wide(true);
    let handle = unsafe { CreateFileW(PCWSTR(wide.as_ptr()), FILE_READ_ATTRIBUTES.0, FILE_SHARE_READ | FILE_SHARE_WRITE, None, OPEN_EXISTING, FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT, None) }?;
    let folder = Folder(handle);
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    unsafe { GetFileInformationByHandle(folder.0, &mut info) }?;
    if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 || info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY.0 == 0 {
        return Err(error(ERROR_ACCESS_DENIED));
    }
    let id = format!("{:08x}{:08x}{:08x}", info.dwVolumeSerialNumber, info.nFileIndexHigh, info.nFileIndexLow);
    Ok((folder, id))
}

fn folder_id(dir: &FsPath) -> windows::core::Result<String> {
    hold_folder(dir).map(|(_, id)| id)
}

fn create_held(target: &FsPath, path: &FsPath, id: &str) -> windows::core::Result<()> {
    let parent = path.parent().ok_or_else(|| error(ERROR_INVALID_NAME))?;
    let (folder, actual) = hold_folder(&parent)?;
    if !actual.eq_ignore_ascii_case(id) {
        return Err(error(ERROR_ACCESS_DENIED));
    }
    let made = create(LinkKind::Symbolic, target, path);
    drop(folder);
    made
}

fn encoded(path: &FsPath) -> String {
    let mut wide = path.to_wide(false);
    wide.pop();
    wide.into_iter().map(|unit| format!("{unit:04x}")).collect()
}

fn decoded(arg: OsString) -> Option<FsPath> {
    let text = arg.to_str()?;
    if text.is_empty() || text.len() % 4 != 0 || text.len() > 32767 * 4 || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let units = text.as_bytes().chunks_exact(4).map(|chunk| u16::from_str_radix(std::str::from_utf8(chunk).ok()?, 16).ok()).collect::<Option<Vec<_>>>()?;
    if units.contains(&0) {
        return None;
    }
    FsPath::from_utf16(&units).filter(filesystem)
}

fn decoded_id(arg: &OsString) -> Option<String> {
    let text = arg.to_str()?;
    (text.len() == 24 && text.bytes().all(|b| b.is_ascii_hexdigit())).then(|| text.to_owned())
}

pub fn symbolic_from_command_line(args: impl IntoIterator<Item = OsString>) -> i32 {
    let args: Vec<OsString> = args.into_iter().collect();
    if args.is_empty() || !args.len().is_multiple_of(3) {
        return ERROR_INVALID_PARAMETER.0 as i32;
    }
    let mut links = Vec::with_capacity(args.len() / 3);
    for triple in args.chunks_exact(3) {
        let (Some(id), Some(target), Some(path)) = (decoded_id(&triple[0]), decoded(triple[1].clone()), decoded(triple[2].clone())) else { return ERROR_INVALID_PARAMETER.0 as i32 };
        links.push((id, target, path));
    }
    let mut first = 0;
    for (id, target, path) in links {
        if let Err(e) = create_held(&target, &path, &id) {
            if first == 0 {
                first = WIN32_ERROR::from_error(&e).unwrap_or(ERROR_GEN_FAILURE).0 as i32;
            }
        }
    }
    first
}

const COMMAND_LIMIT: usize = 32000;

pub(crate) fn elevated(links: &[(FsPath, FsPath)]) -> windows::core::Result<()> {
    elevated_with(links, crate::update::run_elevated)
}

pub(crate) fn elevated_with(links: &[(FsPath, FsPath)], mut launch: impl FnMut(&[String]) -> Result<u32, flit_platform::PlatformError>) -> windows::core::Result<()> {
    let exe = std::env::current_exe().map_err(|_| error(ERROR_INVALID_NAME))?;
    let base = exe.as_os_str().encode_wide().count() + 64;
    let mut batches: Vec<Vec<String>> = Vec::new();
    let mut length = base;
    for (target, path) in links {
        let id = folder_id(&path.parent().ok_or_else(|| error(ERROR_INVALID_NAME))?)?;
        let triple = [id, encoded(target), encoded(path)];
        let size: usize = triple.iter().map(|s| s.len() + 3).sum();
        if base + size >= COMMAND_LIMIT {
            return Err(error(ERROR_FILENAME_EXCED_RANGE));
        }
        if batches.is_empty() || length + size >= COMMAND_LIMIT {
            batches.push(vec!["--create-symbolic-link".into()]);
            length = base;
        }
        length += size;
        if let Some(batch) = batches.last_mut() {
            batch.extend(triple);
        }
    }
    let mut first = None;
    for args in batches {
        let code = launch(&args).map_err(|e| error(WIN32_ERROR(e.code as u32)))?;
        if code != 0 {
            first.get_or_insert(code);
        }
    }
    match first {
        Some(code) => Err(error(WIN32_ERROR(code))),
        None => Ok(()),
    }
}

#[cfg(test)]
#[path = "../test/links.rs"]
mod tests;
