use crate::settings::write_atomic;
use crate::system_journal::{Group, Journal, Native, Target, Value};
use std::fs;
use std::io::{self, ErrorKind};
use std::path::Path;
#[cfg(target_os = "linux")]
use std::path::PathBuf;

const KEY: &str = "org.freedesktop.impl.portal.FileChooser";

fn read(path: &Path) -> io::Result<Option<String>> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Ok(meta) if meta.is_file() => fs::read_to_string(path).map(Some),
        Ok(_) => Err(io::Error::new(ErrorKind::InvalidInput, "the portal configuration must be a regular file")),
        Err(e) => Err(e),
    }
}

fn ranges(text: &str) -> (Option<std::ops::Range<usize>>, Option<usize>) {
    let mut preferred = false;
    let mut offset = 0;
    let mut key = None;
    let mut section = None;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            preferred = trimmed == "[preferred]";
            if preferred {
                section = Some(offset + line.len());
            }
        } else if preferred && trimmed.split_once('=').is_some_and(|(name, _)| name.trim_end() == KEY) {
            key = Some(offset..offset + line.len());
        }
        offset += line.len();
    }
    (key, section)
}

fn updated(text: &str, new: Option<&str>) -> String {
    let (key, section) = ranges(text);
    let eol = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut out = text.to_owned();
    if let Some(range) = key {
        let line = new.map_or_else(String::new, |value| format!("{KEY}={value}{}", if text[range.clone()].ends_with('\n') { eol } else { "" }));
        out.replace_range(range, &line);
    } else if let Some(value) = new {
        let start = section.unwrap_or(text.len());
        let sep = if start > 0 && text.as_bytes()[start - 1] != b'\n' { eol } else { "" };
        let head = if section.is_none() { format!("[preferred]{eol}") } else { String::new() };
        out.insert_str(start, &format!("{sep}{head}{KEY}={value}{eol}"));
    }
    out
}

fn selected(text: &str) -> Option<&str> {
    let range = ranges(text).0?;
    text[range].split_once('=').map(|(_, value)| value.trim())
}

pub fn configure_at(journal: &Journal, path: &Path, on: bool, startup: bool, fallback: impl FnOnce() -> io::Result<String>) -> io::Result<Option<bool>> {
    let before = read(path)?;
    if on {
        if startup && !journal.inspect()?.allows_startup(Group::Portal) {
            return Ok(None);
        }
        if before.as_deref().and_then(selected) == Some("flit") {
            return Ok(Some(false));
        }
        let source = match &before {
            Some(text) => text.clone(),
            None => fallback()?,
        };
        let installed = Value { kind: 0, bytes: updated(&source, Some("flit")).into_bytes() };
        journal.apply(&Native, Group::Portal, &[(Target::File(path.to_path_buf()), installed)], startup)?;
    } else {
        journal.undo(&Native, Some(Group::Portal))?;
        strip(path)?;
    }
    Ok(Some(read(path)? != before))
}

pub fn strip(path: &Path) -> io::Result<()> {
    let original = match read(path) {
        Ok(Some(text)) => text,
        Ok(None) => return Ok(()),
        Err(error) if matches!(error.kind(), ErrorKind::InvalidInput | ErrorKind::InvalidData) => return Ok(()),
        Err(error) => return Err(error),
    };
    let mut text = original.clone();
    while let Some(value) = selected(&text).filter(|value| value.split(';').any(|name| name.trim() == "flit")) {
        let remaining: Vec<_> = value.split(';').filter(|name| name.trim() != "flit").collect();
        let remaining = remaining.iter().any(|name| !name.trim().is_empty()).then(|| remaining.join(";"));
        text = updated(&text, remaining.as_deref());
    }
    if text != original {
        write_atomic(path, text.as_bytes())?;
    }
    Ok(())
}

#[cfg(target_os = "linux")]
pub fn path() -> io::Result<PathBuf> {
    let config_home = flit_platform_linux_gui::xdg::config_home().ok_or_else(|| io::Error::new(ErrorKind::NotFound, "missing user configuration directory"))?;
    Ok(config_home.join("xdg-desktop-portal").join("portals.conf"))
}

#[cfg(target_os = "linux")]
pub fn restore<P: flit_platform::ProfilePlatform>() -> io::Result<()> {
    let path = path()?;
    crate::system_changes::journal::<P>()?.undo(&Native, Some(Group::Portal))?;
    strip(&path)
}

#[cfg(target_os = "linux")]
pub fn configure<P: flit_platform::ProfilePlatform>(on: bool, startup: bool) -> io::Result<Option<bool>> {
    let path = path()?;
    let journal = crate::system_changes::journal::<P>()?;
    let desktop_files: Vec<_> = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default().split(':')
        .filter(|name| !name.is_empty() && name.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-')))
        .map(|name| format!("{}-portals.conf", name.to_ascii_lowercase())).collect();
    if on && desktop_files.iter().any(|file| path.with_file_name(file).exists()) {
        return Err(io::Error::new(ErrorKind::AlreadyExists, "a desktop-specific user portals.conf takes precedence"));
    }
    configure_at(&journal, &path, on, startup, || {
        let config_dirs = std::env::var_os("XDG_CONFIG_DIRS").filter(|s| !s.is_empty()).unwrap_or_else(|| "/etc/xdg".into());
        let data_dirs = std::env::var_os("XDG_DATA_DIRS").filter(|s| !s.is_empty()).unwrap_or_else(|| "/usr/local/share:/usr/share".into());
        let data_home = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")));
        let dirs = std::env::split_paths(&config_dirs).chain([PathBuf::from("/etc")]).chain(data_home)
            .chain(std::env::split_paths(&data_dirs)).chain([PathBuf::from("/usr/share")]).filter(|p| p.is_absolute());
        for dir in dirs {
            let dir = dir.join("xdg-desktop-portal");
            for file in desktop_files.iter().map(String::as_str).chain(std::iter::once("portals.conf")) {
                match fs::read_to_string(dir.join(file)) {
                    Ok(text) => return Ok(text),
                    Err(e) if e.kind() == ErrorKind::NotFound => {}
                    Err(e) => return Err(e),
                }
            }
        }
        Ok("[preferred]\ndefault=*\n".into())
    })
}

#[cfg(test)]
#[path = "../test/portal_config.rs"]
mod tests;
