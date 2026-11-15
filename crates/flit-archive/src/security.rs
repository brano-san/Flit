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

#[cfg(not(target_os = "linux"))]
pub(crate) fn preserve(from: &File, path: &Path) -> io::Result<()> {
    std::fs::set_permissions(path, from.metadata()?.permissions())
}
