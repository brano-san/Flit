use flit_core::portable_meta::Metadata;
use flit_core::i18n::S;
use flit_core::FsPath;
use std::ffi::CStr;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;

const NAMES: [&CStr; 3] = [c"user.baloo.rating", c"user.xdg.comment", c"user.xdg.tags"];

fn open(path: &FsPath) -> io::Result<File> {
    let path = crate::host(path).ok_or(io::ErrorKind::InvalidInput)?;
    let file = OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK).open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    Ok(file)
}

pub fn other_user_attribute(path: &FsPath) -> io::Result<Option<Vec<u8>>> {
    let file = open(path)?;
    let mut names = vec![0u8; 64 * 1024];
    let count = unsafe { libc::flistxattr(file.as_raw_fd(), names.as_mut_ptr().cast(), names.len()) };
    if count < 0 {
        let e = io::Error::last_os_error();
        return if e.raw_os_error() == Some(libc::ENOTSUP) { Ok(None) } else { Err(e) };
    }
    Ok(names[..count as usize].split(|&b| b == 0).find(|&name| name.starts_with(b"user.") && !matches!(name, b"user.xdg.origin.url" | b"user.xdg.referrer.url")).map(<[u8]>::to_vec))
}

fn read_attr(file: &File, name: &CStr) -> io::Result<Option<Vec<u8>>> {
    let fd = file.as_raw_fd();
    let size = unsafe { libc::fgetxattr(fd, name.as_ptr(), std::ptr::null_mut(), 0) };
    if size < 0 {
        let e = io::Error::last_os_error();
        return if e.raw_os_error() == Some(libc::ENODATA) { Ok(None) } else { Err(e) };
    }
    if size as u64 > flit_core::portable_meta::MAX_METADATA {
        return Err(io::ErrorKind::InvalidData.into());
    }
    let mut bytes = vec![0u8; size as usize];
    let count = unsafe { libc::fgetxattr(fd, name.as_ptr(), bytes.as_mut_ptr().cast(), bytes.len()) };
    if count < 0 {
        return Err(io::Error::last_os_error());
    }
    if count as usize > bytes.len() {
        return Err(io::Error::other(S::PortableMetaChanged.en()));
    }
    bytes.truncate(count as usize);
    Ok(Some(bytes))
}

fn write_attr(file: &File, name: &CStr, value: Option<&[u8]>) -> io::Result<()> {
    let fd = file.as_raw_fd();
    let result = match value {
        Some(value) => unsafe { libc::fsetxattr(fd, name.as_ptr(), value.as_ptr().cast(), value.len(), 0) },
        None => unsafe { libc::fremovexattr(fd, name.as_ptr()) },
    };
    if result == 0 {
        return Ok(());
    }
    let e = io::Error::last_os_error();
    if value.is_none() && e.raw_os_error() == Some(libc::ENODATA) {
        Ok(())
    } else {
        Err(e)
    }
}

type Attributes = [Option<Vec<u8>>; 3];

fn raw(file: &File) -> io::Result<Attributes> {
    Ok([read_attr(file, NAMES[0])?, read_attr(file, NAMES[1])?, read_attr(file, NAMES[2])?])
}

fn describe(attrs: &Attributes) -> io::Result<Metadata> {
    let text = |index: usize| std::str::from_utf8(attrs[index].as_deref().unwrap_or_default()).map_err(|_| io::Error::from(io::ErrorKind::InvalidData));
    let rating = text(0)?;
    let rating = if rating.is_empty() { 0 } else { rating.parse::<u8>().map_err(|_| io::Error::from(io::ErrorKind::InvalidData))? };
    let meta = Metadata { rating, comment: text(1)?.to_owned(), tags: Metadata::tags_from_text(text(2)?) };
    if meta.valid() {
        Ok(meta)
    } else {
        Err(io::ErrorKind::InvalidData.into())
    }
}

pub fn read(path: &FsPath) -> io::Result<Metadata> {
    describe(&raw(&open(path)?)?)
}

pub fn write(path: &FsPath, before: &Metadata, after: &Metadata) -> io::Result<()> {
    let file = open(path)?;
    let old = raw(&file)?;
    if describe(&old)? != *before {
        return Err(io::Error::other(S::PortableMetaChanged.en()));
    }
    let mut next = [(after.rating != 0).then(|| after.rating.to_string().into_bytes()), (!after.comment.is_empty()).then(|| after.comment.as_bytes().to_vec()), (!after.tags.is_empty()).then(|| after.tags.join(",").into_bytes())];
    if before.rating == after.rating {
        next[0].clone_from(&old[0]);
    }
    if before.comment == after.comment {
        next[1].clone_from(&old[1]);
    }
    if before.tags == after.tags {
        next[2].clone_from(&old[2]);
    }

    for (i, attribute) in NAMES.iter().enumerate() {
        if old[i] == next[i] {
            continue;
        }
        if let Err(e) = write_attr(&file, attribute, next[i].as_deref()) {
            for k in (0..i).filter(|&k| old[k] != next[k]) {
                if read_attr(&file, NAMES[k])? == next[k] {
                    write_attr(&file, NAMES[k], old[k].as_deref())?;
                }
            }
            return Err(e);
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "../test/portable_metadata.rs"]
mod tests;
