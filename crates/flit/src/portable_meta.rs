use flit_core::FsPath;
use flit_core::i18n::S;
use flit_core::portable_meta::{Metadata, Request};
use flit_platform::std_path;
use std::fs::{self, File};
use std::io::{self, Read, Write};

#[path = "portable_xmp.rs"]
mod xmp;
#[cfg(windows)]
#[path = "portable_windows.rs"]
mod storage;
#[cfg(target_os = "linux")]
use flit_platform_linux_gui::portable_metadata as storage;

pub fn read(path: &FsPath) -> io::Result<Metadata> {
    storage::read(path)
}

fn write(path: &FsPath, before: &Metadata, value: &Metadata) -> io::Result<()> {
    if !value.valid() {
        return Err(io::Error::other(S::PortableMetaInvalid.en()));
    }
    if before == value {
        return Ok(());
    }
    storage::write(path, before, value)
}

const MAX_XMP_BYTES: u64 = 128 * 1024;

fn imported(sidecar: &FsPath, before: &Metadata) -> io::Result<Metadata> {
    let path = std_path(sidecar);
    if !fs::symlink_metadata(&path)?.is_file() {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let file = File::open(path)?;
    let mut text = String::new();
    if file.take(MAX_XMP_BYTES + 1).read_to_string(&mut text)? as u64 > MAX_XMP_BYTES {
        return Err(io::ErrorKind::InvalidData.into());
    }
    xmp::read(&text, before).ok_or_else(|| io::Error::other(S::PortableMetaInvalidXmp.en()))
}

fn export(sidecar: &FsPath, value: &Metadata) -> io::Result<()> {
    if !value.valid() {
        return Err(io::Error::other(S::PortableMetaInvalid.en()));
    }
    let path = std_path(sidecar);
    let text = xmp::write(value);
    if text.len() as u64 > MAX_XMP_BYTES {
        return Err(io::Error::other(S::PortableMetaTooLarge.en()));
    }
    let mut file = File::create_new(&path)?;
    let result = file.write_all(text.as_bytes()).and_then(|()| file.sync_data());
    if result.is_err() {
        drop(file);
        let _ = fs::remove_file(path);
    }
    result
}

pub fn request(req: Request) -> io::Result<Metadata> {
    match req {
        Request::Read(path) => read(&path),
        Request::Store { path, before, after } => {
            write(&path, &before, &after)?;
            Ok(after)
        }
        Request::Import { path, sidecar, before } => {
            let value = imported(&sidecar, &before)?;
            write(&path, &before, &value)?;
            Ok(value)
        }
        Request::Export { path, sidecar, value } => {
            export(&sidecar, &value)?;
            read(&path)
        }
    }
}

#[cfg(test)]
#[path = "../test/portable_meta.rs"]
mod tests;
