use crate::hash::FxHashMap;
use crate::i18n::S;
use crate::listing::Meta;
use crate::path::FsPath;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, RwLock};

pub const MAX_TAGS: usize = 32;
pub const MAX_NAME: usize = 64;
pub const COLORS: [(S, u32); 10] = [
    (S::ThemeTagColorRed, 0xE81123),
    (S::ThemeTagColorOrange, 0xF7630C),
    (S::ThemeTagColorYellow, 0xFFB900),
    (S::ThemeTagColorGreen, 0x16C60C),
    (S::ThemeTagColorTeal, 0x00B7C3),
    (S::ThemeTagColorBlue, 0x0078D4),
    (S::ThemeTagColorPurple, 0x8764B8),
    (S::ThemeTagColorPink, 0xE3008C),
    (S::ThemeTagColorBrown, 0x8E562E),
    (S::ThemeTagColorGray, 0x7A7574),
];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tag {
    pub name: String,
    pub color: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FileKey {
    pub volume: u32,
    pub id: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tagged {
    pub mask: u32,
    pub path: FsPath,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TagStore {
    pub tags: Vec<Tag>,
    pub files: FxHashMap<FileKey, Tagged>,
}

impl Default for TagStore {
    fn default() -> Self {
        Self { tags: vec![Tag { name: "Important".into(), color: COLORS[0].1 }], files: FxHashMap::default() }
    }
}

pub fn remove_bit(mask: u32, k: usize) -> u32 {
    let low = mask & ((1u32 << k) - 1);
    let high = mask.checked_shr(k as u32 + 1).unwrap_or(0) << k;
    low | high
}

pub fn bits(mask: u32) -> impl Iterator<Item = usize> {
    (0..MAX_TAGS).filter(move |&k| mask & (1 << k) != 0)
}

pub fn find(tags: &[Tag], name: &str) -> Option<usize> {
    let want = name.trim().to_lowercase();
    tags.iter().position(|t| t.name.to_lowercase() == want)
}

pub fn validate_name(tags: &[Tag], name: &str, except: Option<usize>) -> Result<String, &'static str> {
    let name = name.trim();
    if name.is_empty() {
        return Err(S::ErrorsTagsEmpty.en());
    }
    if name.starts_with('.') || name.ends_with('.') {
        return Err(S::ErrorsTagsDot.en());
    }
    if name.chars().any(|c| c.is_control() || c == '"') {
        return Err(S::ErrorsTagsCharacters.en());
    }
    if name.chars().count() > MAX_NAME {
        return Err(S::ErrorsTagsTooLong.en());
    }
    if find(tags, name).is_some_and(|k| Some(k) != except) {
        return Err(S::ErrorsTagsExists.en());
    }
    Ok(name.to_owned())
}

pub fn color_name(color: u32) -> &'static str {
    COLORS.iter().find(|c| c.1 == color).map_or(S::ThemeTagColorCustom, |c| c.0).t()
}

pub fn query_for(name: &str) -> String {
    if name.contains(char::is_whitespace) { format!("tag:\"{name}\"") } else { format!("tag:{name}") }
}

pub fn has_tag_term(text: &str) -> bool {
    let mut quoted = false;
    let mut words = Vec::new();
    let mut start = None;
    for (i, c) in text.char_indices() {
        if c == '"' {
            quoted = !quoted;
        }
        match (c.is_whitespace() && !quoted, start) {
            (true, Some(s)) => {
                words.push(&text[s..i]);
                start = None;
            }
            (false, None) => start = Some(i),
            _ => {}
        }
    }
    words.extend(start.map(|s| &text[s..]));
    words.iter().any(|w| w.trim_start_matches('!').get(..4).is_some_and(|p| p.eq_ignore_ascii_case("tag:")))
}

impl TagStore {
    pub fn add(&mut self, name: &str, color: u32) -> Result<usize, &'static str> {
        let name = validate_name(&self.tags, name, None)?;
        if self.tags.len() >= MAX_TAGS {
            return Err(S::ErrorsTagsTooMany.en());
        }
        self.tags.push(Tag { name, color });
        Ok(self.tags.len() - 1)
    }

    pub fn rename(&mut self, k: usize, name: &str) -> Result<(), &'static str> {
        let name = validate_name(&self.tags, name, Some(k))?;
        self.tags.get_mut(k).ok_or(S::ErrorsTagsGone.en())?.name = name;
        Ok(())
    }

    pub fn recolor(&mut self, k: usize, color: u32) {
        if let Some(t) = self.tags.get_mut(k) {
            t.color = color;
        }
    }

    pub fn delete(&mut self, k: usize) -> bool {
        if k >= self.tags.len() {
            return false;
        }
        self.tags.remove(k);
        self.files.retain(|_, f| {
            f.mask = remove_bit(f.mask, k);
            f.mask != 0
        });
        true
    }

    pub fn update(&mut self, key: FileKey, path: FsPath, f: impl FnOnce(u32) -> u32) -> u32 {
        let mask = f(self.files.get(&key).map_or(0, |t| t.mask)) & self.all();
        if mask == 0 {
            self.files.remove(&key);
        } else {
            self.files.insert(key, Tagged { mask, path });
        }
        mask
    }

    pub fn all(&self) -> u32 {
        ((1u64 << self.tags.len()) - 1) as u32
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct TaggedFile {
    pub path: FsPath,
    pub mask: u32,
    pub meta: Meta,
    pub present: bool,
}

#[derive(Debug, Default)]
pub struct Tags {
    pub version: u64,
    pub defs: Vec<Tag>,
    masks: FxHashMap<FileKey, u32>,
    volumes: Vec<u32>,
    pub files: Vec<TaggedFile>,
}

impl Tags {
    pub fn new(version: u64, store: &TagStore, seen: &FxHashMap<FileKey, (Meta, bool)>) -> Self {
        let masks: FxHashMap<FileKey, u32> = store.files.iter().map(|(k, f)| (*k, f.mask)).collect();
        let mut volumes: Vec<u32> = masks.keys().map(|k| k.volume).collect();
        volumes.sort_unstable();
        volumes.dedup();
        let files = store
            .files
            .iter()
            .map(|(k, f)| {
                let (meta, present) = seen.get(k).copied().unwrap_or((Meta::default(), true));
                TaggedFile { path: f.path.clone(), mask: f.mask, meta, present: present && !crate::recycle::in_bin(&f.path) }
            })
            .collect();
        Self { version, defs: store.tags.clone(), masks, volumes, files }
    }

    #[inline]
    pub fn mask(&self, key: FileKey) -> u32 {
        self.masks.get(&key).copied().unwrap_or(0)
    }

    pub fn any(&self) -> bool {
        !self.masks.is_empty()
    }

    pub fn has_volume(&self, volume: u32) -> bool {
        self.volumes.binary_search(&volume).is_ok()
    }

    pub fn count(&self, k: usize) -> usize {
        self.masks.values().filter(|&&m| m & (1 << k) != 0).count()
    }
}

#[derive(Clone, Default)]
pub struct TagCell(Arc<RwLock<Arc<Tags>>>);

impl TagCell {
    pub fn get(&self) -> Arc<Tags> {
        self.0.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn set(&self, tags: Arc<Tags>) {
        *self.0.write().unwrap_or_else(|e| e.into_inner()) = tags;
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum TagRequest {
    Set { items: Vec<FsPath>, tag: usize, on: bool },
    Clear(Vec<FsPath>),
    Create { name: String, color: u32, assign: Vec<FsPath> },
    Rename { tag: usize, name: String },
    Recolor { tag: usize, color: u32 },
    Delete(usize),
    Moved { pairs: Vec<(FsPath, FsPath)>, copy: bool },
    Command { name: String, items: Vec<FsPath> },
}

#[derive(Clone, Debug)]
pub enum TagEvent {
    Changed { tags: Arc<Tags>, files: Vec<(FsPath, u32)>, dropped: Option<usize> },
    Status(String),
}

#[cfg(test)]
#[path = "../test/tags.rs"]
mod tests;
