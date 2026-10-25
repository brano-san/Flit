use std::ffi::{OsStr, OsString};
use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{self, Write};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::{Path, PathBuf};

const STICKY: u32 = 0o1000;

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

pub fn local_time(secs: i64) -> String {
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&secs, &mut tm) };
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min, tm.tm_sec)
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
