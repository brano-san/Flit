use flit_core::i18n::S;
use flit_core::portable_meta::{Metadata, MAX_METADATA};
use flit_core::FsPath;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::AsRawHandle;
use windows::Win32::Foundation::{FILETIME, HANDLE};
use windows::Win32::Storage::FileSystem::SetFileTime;
use std::path::PathBuf;

const MAX_STREAM: u64 = 4 * 1024 * 1024;

fn stream_path(path: &FsPath) -> io::Result<PathBuf> {
    let base = flit_platform::std_path(path);
    let facts = std::fs::symlink_metadata(&base)?;
    if !facts.is_file() || facts.file_attributes() & 0x400 != 0 || path.file_name().contains(&b':') {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let mut stream = base.into_os_string();
    stream.push(":flit.metadata");
    Ok(stream.into())
}

fn read_records(file: &mut File) -> io::Result<(u64, Metadata)> {
    let mut bytes = Vec::new();
    if file.take(MAX_STREAM + 1).read_to_end(&mut bytes)? as u64 > MAX_STREAM {
        return Err(io::ErrorKind::InvalidData.into());
    }
    let complete = bytes.iter().rposition(|&b| b == b'\n').map_or(0, |k| k + 1);
    if complete == 0 { return Ok((0, Metadata::default())); }
    let mut lines = bytes[..complete - 1].rsplit(|&b| b == b'\n');
    let meta: Metadata = serde_json::from_slice(lines.next().unwrap_or_default()).map_err(io::Error::other)?;
    if !meta.valid() {
        return Err(io::ErrorKind::InvalidData.into());
    }
    Ok((complete as u64, meta))
}

pub fn read(path: &FsPath) -> io::Result<Metadata> {
    let stream = stream_path(path)?;
    let file = OpenOptions::new().read(true).share_mode(7).open(stream);
    match file {
        Ok(mut file) => Ok(read_records(&mut file)?.1),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Metadata::default()),
        Err(e) => Err(e),
    }
}

pub fn write(path: &FsPath, before: &Metadata, after: &Metadata) -> io::Result<()> {
    let stream = stream_path(path)?;
    let mut data = serde_json::to_vec(after).map_err(io::Error::other)?;
    data.push(b'\n');
    if data.len() as u64 > MAX_METADATA { return Err(io::ErrorKind::InvalidInput.into()); }
    let mut file = OpenOptions::new().read(true).write(true).create(true).truncate(false).share_mode(1).custom_flags(0x0020_0000).open(stream)?;
    let preserve = FILETIME { dwLowDateTime: u32::MAX, dwHighDateTime: u32::MAX };
    unsafe { SetFileTime(HANDLE(file.as_raw_handle()), None, Some(&preserve), Some(&preserve)) }
        .map_err(|e| io::Error::from_raw_os_error(e.code().0 & 0xffff))?;
    let (len, current) = read_records(&mut file)?;
    if &current != before { return Err(io::Error::other(S::PortableMetaChanged.en())); }
    if len + data.len() as u64 > MAX_STREAM / 2 {
        file.seek(SeekFrom::Start(0))?;
        file.write_all(&data)?;
        file.sync_data()?;
        file.set_len(data.len() as u64)?;
        return file.sync_data();
    }
    if file.metadata()?.len() != len {
        file.set_len(len)?;
    }
    file.seek(SeekFrom::Start(len))?;
    file.write_all(&data)?;
    file.sync_data()
}
