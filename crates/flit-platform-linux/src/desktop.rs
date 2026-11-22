use std::fs::OpenOptions;
use std::io::Read;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

#[path = "desktop_entry.rs"]
mod desktop_entry;

use desktop_entry::name;

const MAX_BYTES: u64 = 64 << 10;

pub fn data_dirs() -> Vec<PathBuf> {
    let absolute = |key| std::env::var_os(key).map(PathBuf::from).filter(|p| p.is_absolute());
    let home = absolute("XDG_DATA_HOME").or_else(|| Some(absolute("HOME")?.join(".local/share")));
    let system = std::env::var("XDG_DATA_DIRS").ok().filter(|v| !v.is_empty()).unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    home.into_iter().chain(system.split(':').filter(|d| d.starts_with('/')).map(PathBuf::from)).collect()
}

fn trusted(path: &Path, real: &Path, mode: u32, data: &[PathBuf]) -> bool {
    let installed = |p: &Path| data.iter().any(|d| p.starts_with(d.join("applications")));
    installed(path) || installed(real) || mode & 0o100 != 0
}

pub fn trusted_launcher(path: &Path, data: &[PathBuf]) -> bool {
    let real = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let mode = std::fs::metadata(&real).map_or(0, |m| m.permissions().mode());
    trusted(path, &real, mode, data)
}

pub(crate) fn label(path: &Path, data: &[PathBuf], lang: &str, system: &str) -> Option<String> {
    let open = |path: &Path| OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_CLOEXEC).open(path);
    let (file, real) = match open(path) {
        Ok(file) => (file, None),
        Err(e) if e.raw_os_error() == Some(libc::ELOOP) => {
            let real = std::fs::canonicalize(path).ok()?;
            (open(&real).ok()?, Some(real))
        }
        Err(_) => return None,
    };
    let meta = file.metadata().ok()?;
    if !meta.is_file() || meta.len() > MAX_BYTES {
        return None;
    }
    let mode = meta.permissions().mode();
    if !trusted(path, path, mode, data) && !real.or_else(|| std::fs::canonicalize(path).ok()).is_some_and(|real| trusted(path, &real, mode, data)) {
        return None;
    }
    let mut text = String::new();
    (&file).take(MAX_BYTES).read_to_string(&mut text).ok()?;
    if file.metadata().ok()?.len() > MAX_BYTES {
        return None;
    }
    name(&text, lang, system)
}

#[cfg(test)]
#[path = "../test/desktop.rs"]
mod tests;