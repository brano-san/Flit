use flit_core::ui::options::SavedLayout;
use std::path::{Path, PathBuf};

const EXT: &str = "json";

pub fn dir(state: Option<PathBuf>) -> Option<PathBuf> {
    state.map(|d| d.join("layouts"))
}

fn file(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}.{EXT}"))
}

pub fn list(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut names: Vec<String> = entries
        .filter_map(|e| {
            let p = e.ok()?.path();
            (p.extension()? == EXT).then(|| p.file_stem()?.to_str().map(str::to_owned)).flatten()
        })
        .collect();
    names.sort_by_key(|n| n.to_lowercase());
    names
}

pub fn save(dir: &Path, name: &str, layout: &SavedLayout) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let text = serde_json::to_string_pretty(layout).map_err(std::io::Error::other)?;
    std::fs::write(file(dir, name), text)
}

pub fn load(dir: &Path, name: &str) -> Option<SavedLayout> {
    serde_json::from_str(&std::fs::read_to_string(file(dir, name)).ok()?).ok()
}

pub fn delete(dir: &Path, name: &str) -> std::io::Result<()> {
    std::fs::remove_file(file(dir, name))
}

#[cfg(test)]
#[path = "../test/layouts.rs"]
mod tests;
