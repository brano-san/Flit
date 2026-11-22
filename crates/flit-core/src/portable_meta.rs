use crate::path::FsPath;
use serde::{Deserialize, Serialize};

pub const MAX_METADATA: u64 = 64 * 1024;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Metadata {
    pub rating: u8,
    pub comment: String,
    pub tags: Vec<String>,
}

impl Metadata {
    pub fn valid(&self) -> bool {
        self.rating <= 10 && self.comment.len() <= 4096 && xml_text(&self.comment) && self.tags.len() <= 128 && self.tags.iter().all(|tag| !tag.trim().is_empty() && tag.len() <= 128 && !tag.contains(',') && xml_text(tag))
    }

    pub fn tags_from_text(text: &str) -> Vec<String> {
        let mut tags = Vec::new();
        for tag in text.split(',').map(str::trim).filter(|t| !t.is_empty()) {
            if !tags.iter().any(|t| t == tag) {
                tags.push(tag.to_owned());
            }
        }
        tags
    }

    pub fn display_rating(&self) -> String {
        if self.rating == 0 {
            return String::new();
        }
        let mut text = "★".repeat(usize::from(self.rating.min(10) / 2));
        if !self.rating.is_multiple_of(2) {
            text.push('½');
        }
        text
    }
}

fn xml_text(text: &str) -> bool {
    text.chars().all(|c| !(c.is_control() && !matches!(c, '\t' | '\n' | '\r')) && c != '\u{fffe}' && c != '\u{ffff}')
}

pub fn xmp_path(path: &FsPath) -> Option<FsPath> {
    let mut name = path.file_name().to_vec();
    if name.eq_ignore_ascii_case(b"xmp") || name.to_ascii_lowercase().ends_with(b".xmp") {
        return None;
    }
    name.extend_from_slice(b".xmp");
    Some(path.parent()?.join(&name))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    Read(FsPath),
    Store { path: FsPath, before: Metadata, after: Metadata },
    Import { path: FsPath, sidecar: FsPath, before: Metadata },
    Export { path: FsPath, sidecar: FsPath, value: Metadata },
}

impl Request {
    pub fn path(&self) -> &FsPath {
        match self {
            Self::Read(path) | Self::Store { path, .. } | Self::Import { path, .. } | Self::Export { path, .. } => path,
        }
    }
}
