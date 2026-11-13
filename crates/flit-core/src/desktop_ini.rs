use crate::hash::FxHashMap;
use crate::name;
use crate::path::{FsPath, PathKind};
use std::sync::RwLock;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FolderKind {
    #[default]
    Unknown,
    Generic,
    Documents,
    Pictures,
    Music,
    Videos,
}

impl FolderKind {
    pub fn from_u8(v: u8) -> Self {
        [Self::Unknown, Self::Generic, Self::Documents, Self::Pictures, Self::Music, Self::Videos].get(v as usize).copied().unwrap_or_default()
    }

    fn from_type(t: &str) -> Self {
        let t = t.to_ascii_lowercase();
        let kinds = [("picture", Self::Pictures), ("photo", Self::Pictures), ("video", Self::Videos), ("music", Self::Music), ("document", Self::Documents), ("generic", Self::Generic)];
        kinds.into_iter().find(|(k, _)| t.contains(k)).map_or(Self::Unknown, |(_, v)| v)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DesktopIni {
    pub icon: bool,
    pub kind: FolderKind,
    pub name: Option<String>,
}

pub fn parse(bytes: &[u8]) -> DesktopIni {
    let text = match bytes {
        [0xFF, 0xFE, rest @ ..] => String::from_utf16_lossy(&rest.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect::<Vec<_>>()),
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8_lossy(rest).into_owned(),
        _ => String::from_utf8_lossy(bytes).into_owned(),
    };
    let mut out = DesktopIni::default();
    let mut section = String::new();
    for line in text.lines().map(str::trim) {
        if let Some(s) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            section = s.trim().to_ascii_lowercase();
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        let (key, value) = (key.trim().to_ascii_lowercase(), value.trim());
        match (section.as_str(), key.as_str()) {
            (".shellclassinfo", "iconresource" | "iconfile") => out.icon |= !value.is_empty(),
            (".shellclassinfo", "localizedresourcename") => out.name = Some(value.trim_matches('"').trim().to_owned()).filter(|v| !v.is_empty()),
            ("viewstate", "foldertype") => out.kind = FolderKind::from_type(value),
            _ => {}
        }
    }
    out
}

type Labels = FxHashMap<String, Option<Box<str>>>;

static LABELS: RwLock<Option<Labels>> = RwLock::new(None);

fn label_key(dir: &FsPath) -> Option<String> {
    (dir.kind() == PathKind::Drive).then(|| name::display(dir.as_bytes()).to_lowercase())
}

pub fn remember(dir: &FsPath, label: Option<&str>) {
    let Some(key) = label_key(dir) else { return };
    let mut labels = LABELS.write().unwrap_or_else(|e| e.into_inner());
    labels.get_or_insert_with(Labels::default).insert(key, label.filter(|l| !l.is_empty()).map(Box::from));
}

pub fn known(dir: &FsPath) -> bool {
    let Some(key) = label_key(dir) else { return true };
    LABELS.read().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(|m| m.contains_key(&key))
}

pub fn label(dir: &FsPath) -> Option<String> {
    let labels = LABELS.read().unwrap_or_else(|e| e.into_inner());
    labels.as_ref()?.get(&label_key(dir)?)?.as_deref().map(str::to_owned)
}

#[cfg(test)]
#[path = "../test/desktop_ini.rs"]
mod tests;
