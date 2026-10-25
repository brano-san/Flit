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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DesktopIni {
    pub icon: bool,
    pub kind: FolderKind,
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
            ("viewstate", "foldertype") => out.kind = FolderKind::from_type(value),
            _ => {}
        }
    }
    out
}

#[cfg(test)]
#[path = "../test/desktop_ini.rs"]
mod tests;
