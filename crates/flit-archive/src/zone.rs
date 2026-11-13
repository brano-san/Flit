use std::ffi::OsString;
use std::path::{Path, PathBuf};

fn stream(p: &Path) -> PathBuf {
    let mut s = OsString::from(p.as_os_str());
    s.push(":Zone.Identifier");
    PathBuf::from(s)
}

fn text(bytes: &[u8]) -> String {
    match bytes {
        [0xFF, 0xFE, rest @ ..] => String::from_utf16_lossy(&rest.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect::<Vec<_>>()),
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

pub fn zone_id(bytes: &[u8]) -> Option<u32> {
    text(bytes).lines().find_map(|l| l.split_once('=').filter(|(k, _)| k.trim().eq_ignore_ascii_case("ZoneId")).and_then(|(_, v)| v.trim().parse().ok()))
}

pub fn read(archive: &Path) -> Option<Vec<u8>> {
    if !cfg!(windows) {
        return None;
    }
    let bytes = std::fs::read(stream(archive)).ok()?;
    zone_id(&bytes).map(|_| bytes)
}

pub fn mark(file: &Path, zone: &[u8]) {
    let _ = std::fs::write(stream(file), zone);
}
