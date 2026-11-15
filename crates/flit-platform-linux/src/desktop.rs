use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Read;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

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

fn locales(lang: &str) -> Vec<String> {
    let lang = lang.replace('-', "_");
    let (base, modifier) = lang.split_once('@').map_or((lang.as_str(), None), |(b, m)| (b, Some(m)));
    let base = base.split('.').next().unwrap_or_default();
    let short = base.split('_').next().unwrap_or_default();
    if short.is_empty() || matches!(short, "C" | "POSIX") {
        return Vec::new();
    }
    let mut out = Vec::new();
    if let Some(modifier) = modifier {
        out.push(format!("{base}@{modifier}"));
    }
    out.push(base.to_owned());
    if base != short {
        if let Some(modifier) = modifier {
            out.push(format!("{short}@{modifier}"));
        }
        out.push(short.to_owned());
    }
    out
}

fn unescape(text: &str) -> Option<String> {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        out.push(if c == '\\' {
            match chars.next()? {
                's' => ' ',
                'n' => '\n',
                't' => '\t',
                'r' => '\r',
                '\\' => '\\',
                _ => return None,
            }
        } else {
            c
        });
    }
    (!out.trim().is_empty()).then_some(out)
}

pub(crate) fn name(text: &str, lang: &str, system: &str) -> Option<String> {
    let mut fields = HashMap::new();
    let mut inside = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            if inside {
                break;
            }
            inside = line == "[Desktop Entry]";
        } else if inside && !line.starts_with('#') {
            if let Some((key, value)) = line.split_once('=') {
                fields.insert(key.trim(), value.trim());
            }
        }
    }
    if !matches!(fields.get("Type"), Some(&"Application" | &"Link")) {
        return None;
    }
    let base = fields.get("Name").and_then(|v| unescape(v))?;
    locales(lang).into_iter().chain(locales(system)).find_map(|l| fields.get(format!("Name[{l}]").as_str()).and_then(|v| unescape(v))).or(Some(base))
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