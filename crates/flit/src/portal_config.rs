use crate::settings::write_atomic;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, ErrorKind};
use std::path::Path;
#[cfg(target_os = "linux")]
use std::path::PathBuf;

const KEY: &str = "org.freedesktop.impl.portal.FileChooser";

#[derive(Serialize, Deserialize)]
struct Saved {
    original: Option<String>,
    installed: String,
    previous: Option<String>,
}

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

fn set(path: &Path, backup: &Path, on: bool, fallback: impl FnOnce() -> io::Result<String>) -> io::Result<bool> {
    let saved: Option<Saved> = read(backup)?.map(|text| serde_json::from_str(&text)).transpose()?;
    if !on && saved.is_none() {
        return Ok(false);
    }
    let live = read(path)?;
    if on {
        let saved = match saved {
            Some(saved) if live.as_ref() == Some(&saved.installed) => return Ok(false),
            Some(saved) if live == saved.original => saved,
            Some(_) => return Err(io::Error::new(ErrorKind::AlreadyExists, "the portal configuration was changed outside Flit")),
            None => {
                let source = match &live {
                    Some(text) => text.clone(),
                    None => fallback()?,
                };
                let installed = updated(&source, Some("flit"));
                let previous = selected(&source).map(str::to_owned);
                let saved = Saved { original: live, installed, previous };
                write_atomic(backup, &serde_json::to_vec(&saved)?)?;
                saved
            }
        };
        write_atomic(path, saved.installed.as_bytes())?;
        return Ok(true);
    }
    let saved = saved.unwrap();
    let mut changed = false;
    if live.as_ref() == Some(&saved.installed) {
        if let Some(text) = saved.original {
            write_atomic(path, text.as_bytes())?;
        } else {
            fs::remove_file(path)?;
        }
        changed = true;
    } else if let Some(text) = live.filter(|text| selected(text) == Some("flit")) {
        if saved.previous.as_deref() != Some("flit") {
            write_atomic(path, updated(&text, saved.previous.as_deref()).as_bytes())?;
            changed = true;
        }
    }
    fs::remove_file(backup)?;
    Ok(changed)
}

#[cfg(target_os = "linux")]
fn paths(settings: &Path) -> io::Result<(PathBuf, PathBuf)> {
    let state = settings.parent().ok_or_else(|| io::Error::new(ErrorKind::InvalidInput, "missing Flit configuration directory"))?;
    let config_home = state.parent().ok_or_else(|| io::Error::new(ErrorKind::InvalidInput, "missing user configuration directory"))?;
    Ok((config_home.join("xdg-desktop-portal").join("portals.conf"), state.join("portal-file-chooser.json")))
}

fn restore_on_uninstall(path: &Path, backup: &Path) -> io::Result<()> {
    set(path, backup, false, || unreachable!())?;
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
pub fn restore(settings: &Path) -> io::Result<()> {
    let (path, backup) = paths(settings)?;
    restore_on_uninstall(&path, &backup)
}

#[cfg(target_os = "linux")]
pub fn configure(settings: &Path, on: bool) -> io::Result<bool> {
    let (path, backup) = paths(settings)?;
    let desktop_files: Vec<_> = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default().split(':')
        .filter(|name| !name.is_empty() && name.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-')))
        .map(|name| format!("{}-portals.conf", name.to_ascii_lowercase())).collect();
    if on && desktop_files.iter().any(|file| path.with_file_name(file).exists()) {
        return Err(io::Error::new(ErrorKind::AlreadyExists, "a desktop-specific user portals.conf takes precedence"));
    }
    set(&path, &backup, on, || {
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
