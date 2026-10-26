use crate::names::{self, fold};
use crate::{corrupt, io_error, password_error, std_path};
use flit_core::archive::{self, Encoding, Format, Secret};
use flit_core::path::FsPath;
use flit_platform::PlatformError;
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

const TICKS_PER_SEC: i64 = 10_000_000;
const UNIX_EPOCH_SECS: i64 = 11_644_473_600;
const MAX_CENTRAL_DIRECTORY: u64 = 1 << 30;
const CACHED: usize = 8;

#[derive(Clone, Debug)]
pub struct Item {
    pub path: String,
    pub key: String,
    pub size: u64,
    pub modified: i64,
    pub dir: bool,
    pub ordinal: usize,
    pub zip_name: String,
}

#[derive(Debug)]
pub struct Index {
    pub format: Format,
    pub items: Vec<Item>,
    pub rejected: Vec<String>,
    pub declared: bool,
    pub entries: usize,
    pub packed: u64,
    pub sevenz: Option<sevenz_rust2::Archive>,
}

struct Raw {
    name: Vec<u8>,
    utf8: bool,
    size: u64,
    modified: i64,
    dir: bool,
    link: bool,
}

pub fn unix_ticks(secs: i64) -> i64 {
    if secs <= 0 { 0 } else { (secs + UNIX_EPOCH_SECS) * TICKS_PER_SEC }
}

pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468
}

pub fn dos_ticks(date: u16, time: u16, offset_min: i32) -> i64 {
    if date == 0 {
        return 0;
    }
    let (y, m, d) = (1980 + (date >> 9) as i64, ((date >> 5) & 15).clamp(1, 12) as u32, (date & 31).max(1) as u32);
    let secs = days_from_civil(y, m, d) * 86_400 + (time >> 11) as i64 * 3600 + ((time >> 5) & 63) as i64 * 60 + (time & 31) as i64 * 2;
    (secs - offset_min as i64 * 60 + UNIX_EPOCH_SECS) * TICKS_PER_SEC
}

fn le16(b: &[u8], at: usize) -> u16 {
    b.get(at..at + 2).map_or(0, |s| u16::from_le_bytes([s[0], s[1]]))
}

fn le32(b: &[u8], at: usize) -> u32 {
    b.get(at..at + 4).map_or(0, |s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

fn le64(b: &[u8], at: usize) -> u64 {
    b.get(at..at + 8).map_or(0, |s| u64::from_le_bytes(s.try_into().unwrap_or([0; 8])))
}

fn read_at(f: &mut File, at: u64, len: usize) -> std::io::Result<Vec<u8>> {
    let mut buf = vec![0; len];
    f.seek(SeekFrom::Start(at))?;
    f.read_exact(&mut buf)?;
    Ok(buf)
}

fn zip_entries(f: &mut File, len: u64, offset_min: i32) -> Result<Vec<Raw>, PlatformError> {
    let bad = || corrupt("The ZIP central directory is damaged");
    let tail_len = len.min(22 + 65_535 + 20) as usize;
    let tail = read_at(f, len - tail_len as u64, tail_len).map_err(io_error)?;
    let at = (0..tail.len().saturating_sub(21)).rev().find(|&i| le32(&tail, i) == 0x0605_4b50).ok_or_else(bad)?;
    let eocd_abs = len - tail_len as u64 + at as u64;
    let (mut count, mut cd_size, mut cd_off, mut cd_end) = (le16(&tail, at + 10) as u64, le32(&tail, at + 12) as u64, le32(&tail, at + 16) as u64, eocd_abs);
    if count == 0xFFFF || cd_size == 0xFFFF_FFFF || cd_off == 0xFFFF_FFFF {
        let locator = read_at(f, eocd_abs.checked_sub(20).ok_or_else(bad)?, 20).map_err(io_error)?;
        if le32(&locator, 0) == 0x0706_4b50 {
            let z64_abs = le64(&locator, 8);
            let z64 = read_at(f, z64_abs, 56).map_err(io_error)?;
            if le32(&z64, 0) != 0x0606_4b50 {
                return Err(bad());
            }
            (count, cd_size, cd_off, cd_end) = (le64(&z64, 32), le64(&z64, 40), le64(&z64, 48), z64_abs);
        }
    }
    if cd_size > MAX_CENTRAL_DIRECTORY || cd_size > cd_end {
        return Err(bad());
    }
    let start = cd_end - cd_size;
    let delta = start.saturating_sub(cd_off);
    let cd = read_at(f, cd_off + delta, cd_size as usize).map_err(io_error)?;
    let mut out = Vec::with_capacity(count.min(1 << 20) as usize);
    let mut p = 0usize;
    while out.len() < count as usize && le32(&cd, p) == 0x0201_4b50 {
        let made_by = le16(&cd, p + 4) >> 8;
        let flags = le16(&cd, p + 8);
        let (time, date) = (le16(&cd, p + 12), le16(&cd, p + 14));
        let mut size = le32(&cd, p + 24) as u64;
        let (nlen, xlen, clen) = (le16(&cd, p + 28) as usize, le16(&cd, p + 30) as usize, le16(&cd, p + 32) as usize);
        let ext = le32(&cd, p + 38);
        let name_at = p + 46;
        let raw = cd.get(name_at..name_at + nlen).ok_or_else(bad)?;
        let extra = cd.get(name_at + nlen..name_at + nlen + xlen).unwrap_or(&[]);
        let (mut name, mut utf8, mut modified) = (raw.to_vec(), flags & 0x800 != 0, 0i64);
        let mut q = 0usize;
        while q + 4 <= extra.len() {
            let (id, sz) = (le16(extra, q), le16(extra, q + 2) as usize);
            let data = extra.get(q + 4..q + 4 + sz).unwrap_or(&[]);
            match id {
                0x0001 if le32(&cd, p + 24) == 0xFFFF_FFFF => size = le64(data, 0),
                0x7075 if data.len() > 5 && le32(data, 1) == crc32fast::hash(raw) => {
                    name = data[5..].to_vec();
                    utf8 = true;
                }
                0x000A if le16(data, 4) == 1 && le16(data, 6) >= 24 => modified = le64(data, 8) as i64,
                0x5455 if modified == 0 && data.first().is_some_and(|f| f & 1 != 0) => modified = unix_ticks(le32(data, 1) as i32 as i64),
                _ => {}
            }
            q += 4 + sz;
        }
        let unix = (ext >> 16) & 0o170_000;
        let dir = name.last().is_some_and(|&c| c == b'/' || c == b'\\') || (made_by == 3 && unix == 0o040_000) || (made_by != 3 && ext & 0x10 != 0);
        let link = made_by == 3 && unix == 0o120_000;
        if modified == 0 {
            modified = dos_ticks(date, time, offset_min);
        }
        out.push(Raw { name, utf8, size, modified, dir, link });
        p = name_at + nlen + xlen + clen;
    }
    if out.len() < count as usize && count != 0xFFFF {
        return Err(bad());
    }
    Ok(out)
}

fn sevenz_index(path: &Path, password: Option<&Secret>) -> Result<(Vec<Raw>, sevenz_rust2::Archive), PlatformError> {
    let mut f = BufReader::new(File::open(path).map_err(io_error)?);
    let pw = password.map_or_else(sevenz_rust2::Password::empty, |p| sevenz_rust2::Password::new(&p.0));
    let a = sevenz_rust2::Archive::read(&mut f, &pw).map_err(|e| sevenz_error(e, password.is_some()))?;
    let raws = a
        .files
        .iter()
        .map(|e| {
            let unix = if e.windows_attributes & 0x8000 != 0 { (e.windows_attributes >> 16) & 0o170_000 } else { 0 };
            let link = e.windows_attributes & 0x400 != 0 || unix == 0o120_000;
            let modified = if e.has_last_modified_date { u64::from(e.last_modified_date) as i64 } else { 0 };
            Raw { name: e.name.clone().into_bytes(), utf8: true, size: e.size, modified, dir: e.is_directory, link }
        })
        .collect();
    Ok((raws, a))
}

pub fn sevenz_error(e: sevenz_rust2::Error, had_password: bool) -> PlatformError {
    match e {
        sevenz_rust2::Error::PasswordRequired => password_error(had_password),
        sevenz_rust2::Error::MaybeBadPassword(_) => password_error(true),
        e => corrupt(&format!("The 7z archive cannot be read: {e}")),
    }
}

fn tar_index(r: impl Read) -> Result<Vec<Raw>, PlatformError> {
    let mut a = tar::Archive::new(r);
    let mut out = Vec::new();
    for e in a.entries().map_err(io_error)? {
        let e = e.map_err(|e| corrupt(&format!("The TAR archive is damaged: {e}")))?;
        let h = e.header();
        let kind = h.entry_type();
        let regular = kind.is_file() || kind.is_contiguous() || kind.is_dir();
        out.push(Raw { name: e.path_bytes().into_owned(), utf8: false, size: e.size(), modified: unix_ticks(h.mtime().unwrap_or(0) as i64), dir: kind.is_dir(), link: !regular });
    }
    Ok(out)
}

struct GzHead {
    name: Option<Vec<u8>>,
    modified: i64,
}

fn gz_head(b: &[u8]) -> Option<GzHead> {
    if b.len() < 10 || b[0] != 0x1f || b[1] != 0x8b || b[2] != 8 {
        return None;
    }
    let flags = b[3];
    let mut p = 10usize;
    if flags & 4 != 0 {
        p += 2 + le16(b, p) as usize;
    }
    let name = (flags & 8 != 0).then(|| b.get(p..).and_then(|rest| rest.iter().position(|&c| c == 0).map(|end| rest[..end].to_vec()))).flatten();
    Some(GzHead { name, modified: unix_ticks(le32(b, 4) as i64) })
}

fn gz_has_tar(f: &mut File) -> bool {
    let mut head = [0u8; 512];
    let _ = f.seek(SeekFrom::Start(0));
    let ok = flate2::read::MultiGzDecoder::new(BufReader::new(&mut *f)).read_exact(&mut head).is_ok();
    ok && &head[257..262] == b"ustar"
}

pub fn sniff(path: &Path, name: &[u8]) -> Result<Format, PlatformError> {
    let mut f = File::open(path).map_err(io_error)?;
    let mut head = [0u8; 512];
    let n = f.read(&mut head).map_err(io_error)?;
    let head = &head[..n];
    Ok(match head {
        [b'7', b'z', 0xBC, 0xAF, 0x27, 0x1C, ..] => Format::SevenZ,
        [b'R', b'a', b'r', b'!', 0x1A, 0x07, ..] => Format::Rar,
        [0x1f, 0x8b, ..] if archive::format_of(name) == Some(Format::TarGz) || gz_has_tar(&mut f) => Format::TarGz,
        [0x1f, 0x8b, ..] => Format::Gz,
        _ if head.len() >= 262 && &head[257..262] == b"ustar" => Format::Tar,
        [b'P', b'K', ..] => Format::Zip,
        [_, _, b'-', b'l', b'h' | b'z', _, b'-', ..] => Format::Lzh,
        _ => match archive::format_of(name) {
            Some(Format::Tar) => Format::Tar,
            Some(Format::Zip) => Format::Zip,
            _ => return Err(corrupt("This file is not an archive Flit can open")),
        },
    })
}

fn rar_index(path: &Path, password: Option<&Secret>, offset_min: i32) -> Result<Vec<Raw>, PlatformError> {
    let list = match password {
        Some(p) => unrar_ng::Archive::with_password(path, p.0.as_bytes()).open_for_listing(),
        None => unrar_ng::Archive::new(path).open_for_listing(),
    }
    .map_err(|e| rar_error(e, password.is_some()))?;
    let mut out = Vec::new();
    for h in list {
        let h = h.map_err(|e| rar_error(e, password.is_some()))?;
        let name = h.filename.to_string_lossy().into_owned().into_bytes();
        let modified = dos_ticks((h.file_time >> 16) as u16, h.file_time as u16, offset_min);
        out.push(Raw { name, utf8: true, size: h.unpacked_size, modified, dir: h.is_directory(), link: h.file_attr & 0x400 != 0 });
    }
    Ok(out)
}

pub fn lzh_error(e: impl std::fmt::Display) -> PlatformError {
    corrupt(&format!("The LZH archive cannot be read: {e}"))
}

fn lzh_index(path: &Path, offset_min: i32) -> Result<Vec<Raw>, PlatformError> {
    let mut r = delharc::parse_file(path).map_err(lzh_error)?;
    let mut out = Vec::new();
    loop {
        let h = r.header();
        let modified = if h.level < 2 { dos_ticks((h.last_modified >> 16) as u16, h.last_modified as u16, offset_min) } else { unix_ticks(h.last_modified as i64) };
        out.push(Raw { name: h.parse_pathname_to_str().into_bytes(), utf8: true, size: h.original_size, modified, dir: h.is_directory(), link: false });
        if !r.seek_next_file().map_err(lzh_error)? {
            return Ok(out);
        }
    }
}

pub fn rar_error(e: unrar_ng::error::UnrarError, had_password: bool) -> PlatformError {
    use unrar_ng::error::Code;
    match e.code {
        Code::MissingPassword => password_error(had_password),
        Code::BadPassword => password_error(true),
        code => corrupt(&format!("The RAR archive cannot be read ({code:?})")),
    }
}

fn build(archive: &FsPath, path: &Path, len: u64, offset_min: i32) -> Result<Index, PlatformError> {
    let format = sniff(path, archive.file_name())?;
    let password = archive::password(archive);
    let (raws, sevenz) = match format {
        Format::Zip => (zip_entries(&mut File::open(path).map_err(io_error)?, len, offset_min)?, None),
        Format::SevenZ => {
            let (raws, a) = sevenz_index(path, password.as_ref())?;
            (raws, Some(a))
        }
        Format::Tar => (tar_index(BufReader::new(File::open(path).map_err(io_error)?))?, None),
        Format::TarGz => (tar_index(flate2::read::MultiGzDecoder::new(BufReader::new(File::open(path).map_err(io_error)?)))?, None),
        Format::Rar => (rar_index(path, password.as_ref(), offset_min)?, None),
        Format::Lzh => (lzh_index(path, offset_min)?, None),
        Format::Gz => {
            let mut f = File::open(path).map_err(io_error)?;
            let head = read_at(&mut f, 0, len.min(64 * 1024) as usize).map_err(io_error)?;
            let tail = read_at(&mut f, len.saturating_sub(4), 4.min(len as usize)).map_err(io_error)?;
            let h = gz_head(&head).ok_or_else(|| corrupt("The GZip header is damaged"))?;
            let stem = archive::stem(archive.file_name()).to_vec();
            let name = h.name.map(|n| n.rsplit(|&c| c == b'/' || c == b'\\').next().unwrap_or(&[]).to_vec()).filter(|n| !n.is_empty()).unwrap_or(stem);
            (vec![Raw { name, utf8: false, size: le32(&tail, 0) as u64, modified: h.modified, dir: false, link: false }], None)
        }
    };
    let chosen = archive::chosen_encoding(archive);
    let detected = names::detect(raws.iter().filter(|r| !r.utf8).map(|r| r.name.as_slice()));
    if let Some(e) = detected {
        archive::set_detected_encoding(archive, e);
    }
    let legacy = chosen.or(detected);
    let mut index = Index { format, items: Vec::with_capacity(raws.len()), rejected: Vec::new(), declared: format != Format::Gz, entries: raws.len(), packed: len, sevenz };
    for (ordinal, r) in raws.into_iter().enumerate() {
        let text = match (r.utf8, legacy) {
            (true, _) | (false, None) => String::from_utf8_lossy(&r.name).into_owned(),
            (false, Some(e)) => names::decode(&r.name, e),
        };
        let zip_name = if format == Format::Zip {
            if r.utf8 { String::from_utf8_lossy(&r.name).into_owned() } else { names::cp437(&r.name) }
        } else {
            String::new()
        };
        match names::safe_path(&text).filter(|_| !r.link) {
            Some(path) => {
                let key = fold(&path);
                index.items.push(Item { path, key, size: if r.dir { 0 } else { r.size }, modified: r.modified, dir: r.dir, ordinal, zip_name });
            }
            None if text.trim_matches(['/', '\\', '.']).is_empty() => {}
            None => index.rejected.push(text),
        }
    }
    Ok(index)
}

struct Cached {
    key: Vec<u8>,
    len: u64,
    modified: Option<SystemTime>,
    encoding: Option<Encoding>,
    password: Option<Secret>,
    index: Arc<Index>,
}

static CACHE: Mutex<Vec<Cached>> = Mutex::new(Vec::new());

pub fn open(archive: &FsPath, offset_min: i32) -> Result<Arc<Index>, PlatformError> {
    let path = std_path(archive);
    let meta = std::fs::metadata(&path).map_err(io_error)?;
    let (key, len, modified) = (archive.as_bytes().to_ascii_lowercase(), meta.len(), meta.modified().ok());
    let (encoding, password) = (archive::chosen_encoding(archive), archive::password(archive));
    let same = |c: &Cached| c.key == key && c.len == len && c.modified == modified && c.encoding == encoding && c.password == password;
    {
        let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(i) = cache.iter().position(same) {
            let hit = cache.remove(i);
            let index = hit.index.clone();
            cache.push(hit);
            return Ok(index);
        }
    }
    let index = Arc::new(build(archive, &path, len, offset_min)?);
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    cache.retain(|c| c.key != key);
    if cache.len() >= CACHED {
        cache.remove(0);
    }
    cache.push(Cached { key, len, modified, encoding, password, index: index.clone() });
    Ok(index)
}

pub fn forget(archive: &FsPath) {
    let key = archive.as_bytes().to_ascii_lowercase();
    CACHE.lock().unwrap_or_else(|e| e.into_inner()).retain(|c| c.key != key);
}
