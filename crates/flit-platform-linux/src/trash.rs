use crate::fs::{filetime, scan};
use flit_core::listing::{attr, Meta};
use flit_core::{FsPath, ListingBuilder};
use std::ffi::{OsStr, OsString};
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::{Path, PathBuf};

const STICKY: u32 = 0o1000;
const MAX_INFO: u64 = 64 << 10;

pub struct Trash {
    home: PathBuf,
    uid: u32,
}

fn device(mut p: &Path) -> io::Result<u64> {
    loop {
        match fs::symlink_metadata(p) {
            Ok(m) => return Ok(m.dev()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => p = p.parent().ok_or(e)?,
            Err(e) => return Err(e),
        }
    }
}

fn mount_point(path: &Path, dev: u64) -> PathBuf {
    let mut top = path.parent().unwrap_or(Path::new("/"));
    while let Some(up) = top.parent() {
        if fs::symlink_metadata(up).map(|m| m.dev()).ok() != Some(dev) {
            break;
        }
        top = up;
    }
    top.to_path_buf()
}

fn private_dirs(p: &Path) -> io::Result<()> {
    DirBuilder::new().recursive(true).mode(0o700).create(p)
}

pub fn percent_encode(path: &[u8]) -> String {
    path.iter().map(|&b| if b.is_ascii_alphanumeric() || b"-_.~/".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

pub fn percent_decode(s: &[u8]) -> Vec<u8> {
    let hex = |b: Option<&u8>| b.and_then(|&b| (b as char).to_digit(16)).map(|d| d as u8);
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        match (s[i], hex(s.get(i + 1)), hex(s.get(i + 2))) {
            (b'%', Some(high), Some(low)) => {
                out.push(high << 4 | low);
                i += 3;
            }
            (b, ..) => {
                out.push(b);
                i += 1;
            }
        }
    }
    out
}

fn invalid(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, what.to_owned())
}

pub fn local_time(secs: i64) -> String {
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&secs, &mut tm) };
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min, tm.tm_sec)
}

pub fn trash_time(s: &[u8]) -> Option<i64> {
    parse_local_time(s).map(|t| filetime(t, 0))
}

fn parse_local_time(s: &[u8]) -> Option<i64> {
    let s = std::str::from_utf8(s).ok()?;
    let num = |r: std::ops::Range<usize>| s.get(r)?.parse::<i32>().ok();
    if s.len() < 19 || s.as_bytes()[4] != b'-' || s.as_bytes()[10] != b'T' {
        return None;
    }
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    tm.tm_year = num(0..4)? - 1900;
    tm.tm_mon = num(5..7)? - 1;
    tm.tm_mday = num(8..10)?;
    tm.tm_hour = num(11..13)?;
    tm.tm_min = num(14..16)?;
    tm.tm_sec = num(17..19)?;
    tm.tm_isdst = -1;
    let t = unsafe { libc::mktime(&mut tm) };
    (t != -1).then_some(t)
}

fn remove_any(p: &Path) -> io::Result<()> {
    if fs::symlink_metadata(p)?.is_dir() {
        fs::remove_dir_all(p)
    } else {
        fs::remove_file(p)
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

fn candidate(base: &OsStr, k: u32) -> OsString {
    let mut name = base.to_os_string();
    if k > 1 {
        name.push(format!(".{k}"));
    }
    name
}

impl Trash {
    pub fn new(home: PathBuf, uid: u32) -> Self {
        Self { home, uid }
    }

    pub fn from_env() -> Option<Self> {
        let data = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).filter(|p| p.is_absolute());
        let data = data.or_else(|| Some(PathBuf::from(std::env::var_os("HOME")?).join(".local/share")))?;
        Some(Self::new(data.join("Trash"), unsafe { libc::getuid() }))
    }

    fn place(&self, path: &Path, dev: u64) -> io::Result<(PathBuf, Vec<u8>)> {
        if device(&self.home)? == dev {
            return Ok((self.home.clone(), path.as_os_str().as_bytes().to_vec()));
        }
        let top = mount_point(path, dev);
        let relative = path.strip_prefix(&top).unwrap_or(path).as_os_str().as_bytes().to_vec();
        let shared = top.join(".Trash");
        let usable = fs::symlink_metadata(&shared).is_ok_and(|m| m.is_dir() && m.mode() & STICKY != 0);
        let per_user = shared.join(self.uid.to_string());
        if usable && private_dirs(&per_user).is_ok() {
            return Ok((per_user, relative));
        }
        Ok((top.join(format!(".Trash-{}", self.uid)), relative))
    }

    fn base_of(&self, dir: &Path) -> Option<PathBuf> {
        if dir == self.home {
            return self.home.parent().map(Path::to_path_buf);
        }
        let name = dir.file_name()?.as_bytes();
        let uid = self.uid.to_string();
        let parent = dir.parent()?;
        if name == format!(".Trash-{uid}").as_bytes() {
            return Some(parent.to_path_buf());
        }
        if name == uid.as_bytes() && parent.file_name()? == ".Trash" {
            return parent.parent().map(Path::to_path_buf);
        }
        None
    }

    pub fn entry(&self, trashed: &Path) -> io::Result<(PathBuf, PathBuf)> {
        self.record(trashed).map(|(info, original, _)| (info, original))
    }

    pub fn dirs(&self, tops: &[PathBuf]) -> Vec<PathBuf> {
        let mut out = vec![self.home.clone()];
        for top in tops {
            for dir in [top.join(".Trash").join(self.uid.to_string()), top.join(format!(".Trash-{}", self.uid))] {
                if !out.contains(&dir) && fs::symlink_metadata(dir.join("files")).is_ok_and(|m| m.is_dir()) {
                    out.push(dir);
                }
            }
        }
        out
    }

    pub fn list(&self, dirs: &[PathBuf], out: &mut ListingBuilder<'_>) {
        for dir in dirs {
            let files = dir.join("files");
            let mut found = Vec::new();
            let _ = scan(&files, |name, meta| {
                found.push((name.to_vec(), meta));
                true
            });
            for (name, meta) in found {
                let trashed = files.join(OsStr::from_bytes(&name));
                let Ok((_, original, deleted)) = self.record(&trashed) else { continue };
                let (Some(recycled), Some(origin), Some(shown)) = (FsPath::parse_exact(trashed.as_os_str().as_bytes()), original.parent().and_then(|p| FsPath::parse_exact(p.as_os_str().as_bytes())), original.file_name()) else { continue };
                let folder = meta.attrs & attr::DIR != 0;
                let attrs = meta.attrs & !attr::HIDDEN | if folder { 0 } else { attr::SIZE_KNOWN };
                out.push(shown.as_bytes(), Meta { size: if folder { 0 } else { meta.size }, modified: deleted, created: deleted, attrs });
                let names = &mut out.listing.names;
                names.extend_from_slice(recycled.as_bytes());
                names.push(0);
                names.extend_from_slice(origin.as_bytes());
            }
            if !out.batch_done() {
                return;
            }
        }
    }

    pub fn purge(&self, trashed: &Path) -> io::Result<()> {
        let (info, _) = self.entry(trashed)?;
        remove_any(trashed)?;
        fs::remove_file(info)
    }

    pub fn empty(&self, dirs: &[PathBuf]) -> (Vec<PathBuf>, Option<io::Error>) {
        let (mut removed, mut error) = (Vec::new(), None);
        for dir in dirs {
            let (files, info) = (dir.join("files"), dir.join("info"));
            for entry in fs::read_dir(&files).into_iter().flatten().flatten() {
                match remove_any(&entry.path()) {
                    Ok(()) => removed.push(entry.path()),
                    Err(e) => error = Some(e),
                }
            }
            for entry in fs::read_dir(&info).into_iter().flatten().flatten() {
                let name = entry.file_name();
                let payload = name.as_bytes().strip_suffix(b".trashinfo").map(|n| files.join(OsStr::from_bytes(n)));
                if payload.is_none_or(|p| fs::symlink_metadata(p).is_err()) {
                    let _ = fs::remove_file(entry.path());
                }
            }
            let _ = fs::remove_file(dir.join("directorysizes"));
        }
        (removed, error)
    }

    fn record(&self, trashed: &Path) -> io::Result<(PathBuf, PathBuf, i64)> {
        let not_trashed = || invalid("the item is not in a Trash folder Flit knows");
        let name = trashed.file_name().ok_or_else(not_trashed)?;
        let files = trashed.parent().filter(|f| f.file_name() == Some(OsStr::new("files"))).ok_or_else(not_trashed)?;
        let dir = files.parent().ok_or_else(not_trashed)?;
        let base = self.base_of(dir).ok_or_else(not_trashed)?;
        let mut info_name = name.to_os_string();
        info_name.push(".trashinfo");
        let info = dir.join("info").join(info_name);
        let mut text = Vec::new();
        File::open(&info)?.take(MAX_INFO).read_to_end(&mut text)?;
        let path = text.split(|&b| b == b'\n').find_map(|l| l.strip_prefix(b"Path=")).ok_or_else(|| invalid("the Trash entry has no Path line"))?;
        let original = PathBuf::from(OsString::from_vec(percent_decode(path.trim_ascii_end())));
        let deleted = text.split(|&b| b == b'\n').find_map(|l| l.strip_prefix(b"DeletionDate=")).and_then(|d| trash_time(d.trim_ascii())).unwrap_or(0);
        Ok((info, if original.is_absolute() { original } else { base.join(original) }, deleted))
    }

    pub fn trash(&self, path: &Path) -> io::Result<PathBuf> {
        let base = path.file_name().ok_or_else(|| io::Error::from_raw_os_error(libc::EINVAL))?;
        let dev = fs::symlink_metadata(path)?.dev();
        let (dir, original) = self.place(path, dev)?;
        let (files, info) = (dir.join("files"), dir.join("info"));
        private_dirs(&files)?;
        private_dirs(&info)?;
        let record = format!("[Trash Info]\nPath={}\nDeletionDate={}\n", percent_encode(&original), local_time(now_secs()));
        for k in 1.. {
            let name = candidate(base, k);
            let mut info_name = name.clone().into_vec();
            info_name.extend_from_slice(b".trashinfo");
            let info_path = info.join(OsStr::from_bytes(&info_name));
            let mut file = match OpenOptions::new().write(true).create_new(true).open(&info_path) {
                Ok(f) => f,
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            };
            let target = files.join(&name);
            if fs::symlink_metadata(&target).is_ok() {
                let _ = fs::remove_file(&info_path);
                continue;
            }
            let moved = file.write_all(record.as_bytes()).and_then(|()| fs::rename(path, &target));
            if let Err(e) = moved {
                let _ = fs::remove_file(&info_path);
                return Err(e);
            }
            return Ok(target);
        }
        unreachable!()
    }
}

#[cfg(test)]
#[path = "../test/trash.rs"]
mod tests;
