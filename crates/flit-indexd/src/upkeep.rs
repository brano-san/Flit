use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const CHECK: Duration = Duration::from_secs(60);
pub const REPLACED_EXIT: i32 = 75;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stamp {
    dev: u64,
    ino: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Binary {
    Same,
    Replaced,
    Gone,
}

pub fn stamp(path: &Path) -> Option<Stamp> {
    let m = std::fs::metadata(path).ok()?;
    Some(Stamp { dev: m.dev(), ino: m.ino() })
}

pub fn check(path: &Path, start: Stamp) -> Binary {
    match std::fs::metadata(path) {
        Ok(m) if (Stamp { dev: m.dev(), ino: m.ino() }) == start => Binary::Same,
        Ok(_) => Binary::Replaced,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Binary::Gone,
        Err(_) => Binary::Same,
    }
}

pub fn watch_binary() {
    let Some(path) = std::env::current_exe().ok() else { return };
    let Some(start) = stamp(&path) else { return };
    let _ = std::thread::Builder::new().name("flit-indexd-update".into()).spawn(move || loop {
        std::thread::sleep(CHECK);
        match check(&path, start) {
            Binary::Same => {}
            Binary::Replaced => {
                eprintln!("flit-indexd: {} was updated; exiting so that systemd starts the new version", path.display());
                std::process::exit(REPLACED_EXIT);
            }
            Binary::Gone => {
                eprintln!("flit-indexd: {} was removed; exiting", path.display());
                std::process::exit(0);
            }
        }
    });
}

fn snapshot_file(name: &str) -> bool {
    let Some((stem, ext)) = name.split_once('.') else { return false };
    stem.len() == 16 && stem.bytes().all(|b| b.is_ascii_hexdigit()) && (ext == "idx" || ext == "tmp")
}

pub fn old_snapshots() -> Option<PathBuf> {
    std::env::var_os("XDG_STATE_HOME").map(PathBuf::from).filter(|d| d.is_absolute()).map(|d| d.join("indexd"))
}

pub fn remove_old_snapshots(dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    let mut removed = 0;
    for e in entries.flatten() {
        let name = e.file_name();
        if e.file_type().is_ok_and(|t| t.is_file()) && name.to_str().is_some_and(snapshot_file) && std::fs::remove_file(e.path()).is_ok() {
            removed += 1;
        }
    }
    let _ = std::fs::remove_dir(dir);
    removed
}

#[cfg(test)]
#[path = "../test/upkeep.rs"]
mod tests;
