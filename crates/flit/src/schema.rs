use std::path::{Path, PathBuf};

pub type Step = fn(Vec<u8>) -> Option<Vec<u8>>;

pub const NEWER: &str = "newer";
pub const BAD: &str = "bad";
pub const PLAUSIBLE_NEWER: u32 = 16;

pub fn aside(path: &Path, kind: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_owned();
    name.push(".");
    name.push(kind);
    path.with_file_name(name)
}

fn set_aside(path: &Path, kind: &str, why: &str) {
    let to = aside(path, kind);
    tracing::warn!("{} {why}; it is kept as {}", path.display(), to.display());
    let _ = std::fs::rename(path, to);
}

pub fn version(bytes: &[u8]) -> Option<u32> {
    postcard::take_from_bytes::<u32>(bytes).ok().map(|(v, _)| v)
}

pub fn migrate(mut bytes: Vec<u8>, from: u32, current: u32, steps: &[Step]) -> Option<Vec<u8>> {
    let oldest = current.checked_sub(steps.len() as u32)?;
    let first = from.checked_sub(oldest)? as usize;
    for step in steps.get(first..)? {
        bytes = step(bytes)?;
    }
    Some(bytes)
}

pub fn load<T>(path: &Path, current: u32, steps: &[Step], decode: impl FnOnce(&[u8]) -> Option<T>) -> Option<T> {
    let bytes = std::fs::read(path).ok()?;
    let Some(v) = version(&bytes) else {
        set_aside(path, BAD, "has no readable version");
        return None;
    };
    if v > current && v - current <= PLAUSIBLE_NEWER {
        set_aside(path, NEWER, &format!("was written by a newer Flit (version {v}, this one reads up to {current})"));
        return None;
    }
    match migrate(bytes, v, current, steps).as_deref().and_then(decode) {
        Some(t) => Some(t),
        None => {
            set_aside(path, BAD, &format!("is not readable as version {v}"));
            None
        }
    }
}

#[cfg(test)]
#[path = "../test/schema.rs"]
pub mod tests;
