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

fn junction_data(target: &FsPath) -> windows::core::Result<Vec<u8>> {
    let canonical = std::fs::canonicalize(std_path(target)).map_err(|e| error(WIN32_ERROR(e.raw_os_error().unwrap_or(ERROR_INVALID_NAME.0 as i32) as u32)))?;
    let print: Vec<u16> = canonical.as_os_str().encode_wide().collect();
    let prefix: Vec<u16> = r"\\?\".encode_utf16().collect();
    let print = print.strip_prefix(prefix.as_slice()).unwrap_or(&print);
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
    if !choices(std::slice::from_ref(target), &parent).allows(kind) {
        return Err(error(ERROR_INVALID_PARAMETER));
    }
    let (to, from) = (path.to_wide(true), target.to_wide(true));
    match kind {
        LinkKind::Symbolic => {
            let flags = SYMBOLIC_LINK_FLAG_ALLOW_UNPRIVILEGED_CREATE | if std_path(target).is_dir() { SYMBOLIC_LINK_FLAG_DIRECTORY } else { SYMBOLIC_LINK_FLAGS(0) };
            if unsafe { CreateSymbolicLinkW(PCWSTR(to.as_ptr()), PCWSTR(from.as_ptr()), flags) } { Ok(()) } else { Err(windows::core::Error::from_thread()) }
        }
        LinkKind::Junction => junction(target, path),
        LinkKind::Hard => unsafe { CreateHardLinkW(PCWSTR(to.as_ptr()), PCWSTR(from.as_ptr()), None) },
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

fn with_elevation(job: &flit_platform::Job, target: &FsPath, make: impl FnOnce() -> windows::core::Result<()>, elevate: impl FnOnce() -> windows::core::Result<()>) -> windows::core::Result<()> {
    let result = make();
    if result.as_ref().is_err_and(|e| WIN32_ERROR::from_error(e) == Some(ERROR_PRIVILEGE_NOT_HELD)) && job.asks_failures() && !job.cancelled() {
        let failure = flit_core::provider::Failure { path: target.clone(), error: flit_core::i18n::S::ErrorsEngineSymlinkPrivilege.en().to_owned(), fatal: false };
        match job.fail(failure) {
            Some(flit_core::provider::Recover::Retry) if !job.cancelled() => return elevate(),
            None => return Err(error(ERROR_CANCELLED)),
            _ => {}
        }
    }
    result
}

pub(crate) fn create_for_job(kind: LinkKind, target: &FsPath, path: &FsPath, job: &flit_platform::Job) -> windows::core::Result<()> {
    if kind == LinkKind::Symbolic {
        with_elevation(job, target, || create(kind, target, path), || elevated(target, path))
    } else {
        create(kind, target, path)
    }
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

pub fn symbolic_from_command_line(args: impl IntoIterator<Item = OsString>) -> i32 {
    let mut args = args.into_iter();
    let (Some(target), Some(path)) = (args.next().and_then(decoded), args.next().and_then(decoded)) else { return ERROR_INVALID_PARAMETER.0 as i32 };
    if args.next().is_some() {
        return ERROR_INVALID_PARAMETER.0 as i32;
    }
    match create(LinkKind::Symbolic, &target, &path) {
        Ok(()) => 0,
        Err(e) => WIN32_ERROR::from_error(&e).unwrap_or(ERROR_GEN_FAILURE).0 as i32,
    }
}

pub(crate) fn elevated(target: &FsPath, path: &FsPath) -> windows::core::Result<()> {
    elevated_with(target, path, crate::update::run_elevated)
}

fn elevated_with(target: &FsPath, path: &FsPath, launch: impl FnOnce(&[String]) -> Result<u32, flit_platform::PlatformError>) -> windows::core::Result<()> {
    let args = vec!["--create-symbolic-link".into(), encoded(target), encoded(path)];
    let exe = std::env::current_exe().map_err(|_| error(ERROR_INVALID_NAME))?;
    if exe.as_os_str().encode_wide().count() + args.iter().map(|s| s.len()).sum::<usize>() + 16 >= 32767 {
        return Err(error(ERROR_FILENAME_EXCED_RANGE));
    }
    let code = launch(&args).map_err(|e| error(WIN32_ERROR(e.code as u32)))?;
    if code == 0 { Ok(()) } else { Err(error(WIN32_ERROR(code))) }
}

#[cfg(test)]
#[path = "../test/links.rs"]
mod tests;
