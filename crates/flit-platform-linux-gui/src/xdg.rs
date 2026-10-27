use flit_core::provider::Template;
use flit_core::ui::NewTemplate;
use flit_core::{FsPath, Location};
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

const APP: &str = "flit";

const USER_DIRS: [(&str, &str, &str, &str); 8] = [
    ("shell:Desktop", "XDG_DESKTOP_DIR", "Desktop", "user-desktop"),
    ("shell:Downloads", "XDG_DOWNLOAD_DIR", "Downloads", "folder-download"),
    ("shell:Personal", "XDG_DOCUMENTS_DIR", "Documents", "folder-documents"),
    ("shell:My Pictures", "XDG_PICTURES_DIR", "Pictures", "folder-pictures"),
    ("shell:My Music", "XDG_MUSIC_DIR", "Music", "folder-music"),
    ("shell:My Video", "XDG_VIDEOS_DIR", "Videos", "folder-videos"),
    ("shell:Templates", "XDG_TEMPLATES_DIR", "Templates", "folder-templates"),
    ("shell:Public", "XDG_PUBLICSHARE_DIR", "Public", "folder-publicshare"),
];

fn absolute_var(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).map(PathBuf::from).filter(|p| p.is_absolute())
}

pub fn home() -> Option<PathBuf> {
    absolute_var("HOME")
}

fn base(var: &str, under_home: &str) -> Option<PathBuf> {
    absolute_var(var).or_else(|| Some(home()?.join(under_home)))
}

pub fn config_home() -> Option<PathBuf> {
    base("XDG_CONFIG_HOME", ".config")
}

pub fn data_home() -> Option<PathBuf> {
    base("XDG_DATA_HOME", ".local/share")
}

pub fn state_dir() -> Option<PathBuf> {
    Some(base("XDG_STATE_HOME", ".local/state")?.join(APP))
}

pub fn cache_dir() -> Option<PathBuf> {
    Some(base("XDG_CACHE_HOME", ".cache")?.join(APP))
}

pub fn config_dir() -> Option<PathBuf> {
    Some(config_home()?.join(APP))
}

pub fn default_manager(on: bool) -> std::io::Result<()> {
    use flit_core::mimeapps;
    let missing = || std::io::Error::new(std::io::ErrorKind::NotFound, "no HOME or XDG_CONFIG_HOME");
    let list = config_home().ok_or_else(missing)?.join("mimeapps.list");
    let saved = config_dir().ok_or_else(missing)?.join("default-manager.backup");
    let current = std::fs::read_to_string(&list).ok();
    if on {
        if !saved.exists() {
            std::fs::create_dir_all(saved.parent().unwrap_or(Path::new("/")))?;
            std::fs::write(&saved, mimeapps::backup(current.as_deref()))?;
        }
        std::fs::create_dir_all(list.parent().unwrap_or(Path::new("/")))?;
        let text = mimeapps::set_default_app(current.as_deref().unwrap_or_default(), mimeapps::FOLDER, Some(mimeapps::FLIT_DESKTOP));
        return std::fs::write(&list, text);
    }
    let Ok(backup) = std::fs::read_to_string(&saved) else { return Ok(()) };
    match mimeapps::restore(&backup, current.as_deref()) {
        Some(text) => std::fs::write(&list, text)?,
        None => {
            let _ = std::fs::remove_file(&list);
        }
    }
    std::fs::remove_file(&saved)
}

pub fn runtime_dir() -> Option<PathBuf> {
    absolute_var("XDG_RUNTIME_DIR")
}

fn unquote(v: &[u8]) -> Option<Vec<u8>> {
    let inner = v.strip_prefix(b"\"")?.strip_suffix(b"\"")?;
    let mut out = Vec::with_capacity(inner.len());
    let mut bytes = inner.iter();
    while let Some(&b) = bytes.next() {
        out.push(if b == b'\\' { *bytes.next()? } else { b });
    }
    Some(out)
}

pub fn parse_user_dirs(text: &[u8], home: &[u8]) -> Vec<(String, Vec<u8>)> {
    text.split(|&b| b == b'\n')
        .filter_map(|line| {
            let line = line.trim_ascii();
            if line.starts_with(b"#") {
                return None;
            }
            let eq = line.iter().position(|&b| b == b'=')?;
            let key = std::str::from_utf8(&line[..eq]).ok()?.trim().to_owned();
            let value = unquote(line[eq + 1..].trim_ascii())?;
            let path = match value.strip_prefix(b"$HOME") {
                Some(rest) if rest.is_empty() || rest.starts_with(b"/") => [home, rest].concat(),
                Some(_) => return None,
                None if value.starts_with(b"/") => value,
                None => return None,
            };
            Some((key, path))
        })
        .collect()
}

fn user_dirs_file() -> Option<PathBuf> {
    Some(config_home()?.join("user-dirs.dirs"))
}

pub fn user_dir(shell_name: &str) -> Option<PathBuf> {
    let home = home()?;
    let (_, key, fallback, _) = USER_DIRS.iter().find(|(s, k, f, _)| s.eq_ignore_ascii_case(shell_name) || k.eq_ignore_ascii_case(shell_name) || f.eq_ignore_ascii_case(shell_name.trim_start_matches("shell:")))?;
    let configured = user_dirs_file().and_then(|f| std::fs::read(f).ok()).and_then(|text| parse_user_dirs(&text, home.as_os_str().as_bytes()).into_iter().rev().find(|(k, _)| k == key).map(|(_, p)| p));
    Some(configured.map_or_else(|| home.join(fallback), |p| PathBuf::from(OsStr::from_bytes(&p))))
}

pub fn user_dir_icon(path: &Path) -> Option<&'static str> {
    USER_DIRS.iter().find(|(shell, ..)| user_dir(shell).as_deref() == Some(path)).map(|(.., icon)| *icon)
}

pub fn fs_path(p: &Path) -> Option<FsPath> {
    FsPath::parse_exact(p.as_os_str().as_bytes())
}

pub fn known_folder(name: &str) -> Option<Location> {
    let path = if name.eq_ignore_ascii_case("shell:Profile") { home()? } else { user_dir(name)? };
    std::fs::metadata(&path).is_ok_and(|m| m.is_dir()).then(|| fs_path(&path).map(Location::from_local)).flatten()
}

pub fn templates_in(dir: &Path) -> Vec<NewTemplate> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<NewTemplate> = entries
        .filter_map(Result::ok)
        .filter(|e| e.metadata().is_ok_and(|m| m.is_file()) && !e.file_name().as_bytes().starts_with(b"."))
        .filter_map(|e| {
            let name = e.file_name();
            let bytes = name.as_bytes();
            let dot = bytes.iter().rposition(|&b| b == b'.').filter(|&k| k > 0);
            let (stem, ext) = dot.map_or((bytes, &b""[..]), |k| bytes.split_at(k));
            let label = String::from_utf8_lossy(stem).into_owned();
            Some(NewTemplate { label: label.clone(), ext: String::from_utf8(ext.to_vec()).ok()?, stem: label, template: Template::Copy(fs_path(&e.path())?) })
        })
        .collect();
    out.sort_by_cached_key(|t| t.label.to_lowercase());
    out
}

pub fn templates() -> Vec<NewTemplate> {
    user_dir("shell:Templates").filter(|d| home().is_none_or(|h| h != *d)).map(|d| templates_in(&d)).unwrap_or_default()
}

#[cfg(test)]
#[path = "../test/xdg.rs"]
mod tests;
