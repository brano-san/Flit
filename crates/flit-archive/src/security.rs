use std::fs::{File, OpenOptions};
use std::io;
use std::path::Path;

pub(crate) fn create(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

#[cfg(unix)]
pub(crate) fn hard_links(file: &File) -> io::Result<u64> {
    use std::os::unix::fs::MetadataExt;
    Ok(file.metadata()?.nlink())
}

#[cfg(windows)]
pub(crate) fn hard_links(file: &File) -> io::Result<u64> {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION};
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    unsafe { GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info) }.map_err(|e| io::Error::from_raw_os_error(e.code().0 & 0xFFFF))?;
    Ok(info.nNumberOfLinks as u64)
}

#[cfg(target_os = "linux")]
pub(crate) fn preserve(from: &File, path: &Path) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::MetadataExt;
    let to = OpenOptions::new().write(true).open(path)?;
    let meta = from.metadata()?;
    let target = to.metadata()?;
    if (meta.uid(), meta.gid()) != (target.uid(), target.gid()) && unsafe { libc::fchown(to.as_raw_fd(), meta.uid(), meta.gid()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let name = c"system.posix_acl_access";
    let mut acl = vec![0u8; 65536];
    let size = unsafe { libc::fgetxattr(from.as_raw_fd(), name.as_ptr(), acl.as_mut_ptr().cast(), acl.len()) };
    if size >= 0 {
        if unsafe { libc::fsetxattr(to.as_raw_fd(), name.as_ptr(), acl.as_ptr().cast(), size as usize, 0) } != 0 {
            return Err(io::Error::last_os_error());
        }
    } else {
        let error = io::Error::last_os_error();
        if !matches!(error.raw_os_error(), Some(libc::ENODATA | libc::ENOTSUP)) {
            return Err(error);
        }
        if unsafe { libc::fremovexattr(to.as_raw_fd(), name.as_ptr()) } != 0 {
            let error = io::Error::last_os_error();
            if !matches!(error.raw_os_error(), Some(libc::ENODATA | libc::ENOTSUP)) {
                return Err(error);
            }
        }
    }
    to.set_permissions(meta.permissions())
}

#[cfg(windows)]
pub(crate) fn preserve(from: &File, path: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::AsRawHandle;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{LocalFree, ERROR_SUCCESS, HANDLE, HLOCAL};
    use windows::Win32::Security::Authorization::{GetSecurityInfo, SetNamedSecurityInfoW, SE_FILE_OBJECT};
    use windows::Win32::Security::{GetSecurityDescriptorControl, ACL, DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, SE_DACL_PROTECTED, UNPROTECTED_DACL_SECURITY_INFORMATION};
    let mut dacl: *mut ACL = std::ptr::null_mut();
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    let read = unsafe { GetSecurityInfo(HANDLE(from.as_raw_handle()), SE_FILE_OBJECT, DACL_SECURITY_INFORMATION, None, None, Some(&mut dacl), None, Some(&mut descriptor)) };
    if read != ERROR_SUCCESS {
        return Err(io::Error::from_raw_os_error(read.0 as i32));
    }
    let mut control = 0u16;
    let mut revision = 0u32;
    let applied = unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) }.map_err(|e| io::Error::from_raw_os_error(e.code().0 & 0xFFFF)).and_then(|()| {
        let inheritance = if control & SE_DACL_PROTECTED.0 != 0 { PROTECTED_DACL_SECURITY_INFORMATION } else { UNPROTECTED_DACL_SECURITY_INFORMATION };
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
        let written = unsafe { SetNamedSecurityInfoW(PCWSTR(wide.as_ptr()), SE_FILE_OBJECT, DACL_SECURITY_INFORMATION | inheritance, None, None, Some(dacl), None) };
        if written == ERROR_SUCCESS { Ok(()) } else { Err(io::Error::from_raw_os_error(written.0 as i32)) }
    });
    unsafe {
        let _ = LocalFree(Some(HLOCAL(descriptor.0)));
    }
    applied
}

#[cfg(not(any(target_os = "linux", windows)))]
pub(crate) fn preserve(from: &File, path: &Path) -> io::Result<()> {
    std::fs::set_permissions(path, from.metadata()?.permissions())
}

#[cfg(windows)]
pub(crate) fn finish(from: &File, path: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::fs::MetadataExt;
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{SetFileAttributesW, FILE_FLAGS_AND_ATTRIBUTES};
    const KEPT: u32 = 0x2 | 0x4 | 0x20 | 0x2000;
    let wanted = from.metadata()?.file_attributes() & KEPT;
    let current = std::fs::metadata(path)?.file_attributes();
    let next = (current & !KEPT) | wanted;
    if next == current {
        return Ok(());
    }
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    unsafe { SetFileAttributesW(PCWSTR(wide.as_ptr()), FILE_FLAGS_AND_ATTRIBUTES(next)) }.map_err(|e| io::Error::from_raw_os_error(e.code().0 & 0xFFFF))
}

#[cfg(not(windows))]
pub(crate) fn finish(_: &File, _: &Path) -> io::Result<()> {
    Ok(())
}
