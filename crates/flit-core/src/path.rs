use crate::name;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub enum PathKind {
    Computer,
    Drive,
    Unc,
    Verbatim,
    Unix,
    RecycleBin,
    Home,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct FsPath {
    s: Vec<u8>,
    root: u16,
    kind: PathKind,
}

pub const COMPUTER_LABEL: &str = "This PC";
pub const RECYCLE_BIN_LABEL: &str = "Recycle Bin";
pub const RECYCLE_BIN_SHELL: &str = "shell:RecycleBinFolder";
pub const HOME_LABEL: &str = "Home";
pub const HOME_SHELL: &str = "shell:::{f874310e-b6b7-47dc-bc84-b9e6b38f5903}";
const UNIX_HOST: bool = cfg!(unix);

impl FsPath {
    pub fn computer() -> Self {
        Self { s: Vec::new(), root: 0, kind: PathKind::Computer }
    }

    pub fn recycle_bin() -> Self {
        Self { s: RECYCLE_BIN_SHELL.as_bytes().to_vec(), root: RECYCLE_BIN_SHELL.len() as u16, kind: PathKind::RecycleBin }
    }

    pub fn home_page() -> Self {
        Self { s: HOME_SHELL.as_bytes().to_vec(), root: HOME_SHELL.len() as u16, kind: PathKind::Home }
    }

    pub fn parse(input: &str) -> Option<Self> {
        Self::parse_bytes(input.as_bytes())
    }

    pub fn from_utf16(w: &[u16]) -> Option<Self> {
        let mut b = Vec::with_capacity(w.len());
        name::push_utf16(&mut b, w);
        Self::parse_bytes(&b)
    }

    pub fn parse_bytes(input: &[u8]) -> Option<Self> {
        Self::parse_exact(trim(input))
    }

    pub fn parse_exact(input: &[u8]) -> Option<Self> {
        Self::parse_on(input, UNIX_HOST)
    }

    fn parse_on(input: &[u8], unix_host: bool) -> Option<Self> {
        if input.contains(&0) {
            return None;
        }
        if input.is_empty() || input.eq_ignore_ascii_case(COMPUTER_LABEL.as_bytes()) {
            return Some(Self::computer());
        }
        if input.eq_ignore_ascii_case(RECYCLE_BIN_SHELL.as_bytes()) || input.eq_ignore_ascii_case(RECYCLE_BIN_LABEL.as_bytes()) {
            return Some(Self::recycle_bin());
        }
        if input.eq_ignore_ascii_case(HOME_SHELL.as_bytes()) {
            return Some(Self::home_page());
        }
        if input[0] == b'/' && (unix_host || input.get(1) != Some(&b'/')) {
            return Self::with_segments(b"/".to_vec(), PathKind::Unix, b'/', &input[1..]);
        }
        let win: Vec<u8> = input.iter().map(|&c| if c == b'/' { b'\\' } else { c }).collect();
        let mut rest: &[u8] = &win;
        let mut verbatim = false;
        if rest.starts_with(br"\\?\") || rest.starts_with(br"\\.\") {
            verbatim = true;
            rest = &rest[4..];
        }
        if rest.len() >= 2 && rest[0].is_ascii_alphabetic() && rest[1] == b':' {
            let root = vec![rest[0].to_ascii_uppercase(), b':', b'\\'];
            return Self::with_segments(root, PathKind::Drive, b'\\', &rest[2..]);
        }
        let unc_body = if verbatim {
            rest.strip_prefix(br"UNC\")
        } else {
            rest.strip_prefix(br"\\")
        };
        if let Some(body) = unc_body {
            let mut parts = body.splitn(3, |&c| c == b'\\');
            let server = parts.next().filter(|s| !s.is_empty())?;
            let share = parts.next().filter(|s| !s.is_empty())?;
            let mut root = br"\\".to_vec();
            root.extend_from_slice(server);
            root.push(b'\\');
            root.extend_from_slice(share);
            root.push(b'\\');
            return Self::with_segments(root, PathKind::Unc, b'\\', parts.next().unwrap_or(b""));
        }
        if verbatim {
            let mut parts = rest.splitn(2, |&c| c == b'\\');
            let first = parts.next().filter(|s| !s.is_empty())?;
            let mut root = br"\\?\".to_vec();
            root.extend_from_slice(first);
            root.push(b'\\');
            return Self::with_segments(root, PathKind::Verbatim, b'\\', parts.next().unwrap_or(b""));
        }
        None
    }

    fn with_segments(root: Vec<u8>, kind: PathKind, sep: u8, rest: &[u8]) -> Option<Self> {
        let mut p = Self { root: u16::try_from(root.len()).ok()?, s: root, kind };
        for seg in rest.split(|&c| c == sep) {
            match seg {
                b"" | b"." => {}
                b".." => {
                    if let Some(up) = p.parent_in_root() {
                        p = up;
                    }
                }
                _ => p.push(seg),
            }
        }
        Some(p)
    }

    pub fn kind(&self) -> PathKind {
        self.kind
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.s
    }

    pub fn root_bytes(&self) -> &[u8] {
        &self.s[..self.root as usize]
    }

    pub fn is_root(&self) -> bool {
        self.s.len() == self.root as usize
    }

    pub fn case_sensitive(&self) -> bool {
        self.kind == PathKind::Unix
    }

    pub fn same(&self, other: &Self) -> bool {
        if self.case_sensitive() { self.s == other.s } else { self.s.eq_ignore_ascii_case(&other.s) }
    }

    pub fn is_inside(&self, folder: &Self) -> bool {
        let (p, f) = (&self.s, &folder.s);
        let head = p.get(..f.len()).filter(|_| p.len() > f.len());
        head.is_some_and(|h| if folder.case_sensitive() { h == f.as_slice() } else { h.eq_ignore_ascii_case(f) }) && (folder.is_root() || p[f.len()] == folder.sep())
    }

    pub fn sep(&self) -> u8 {
        if self.kind == PathKind::Unix { b'/' } else { b'\\' }
    }

    pub fn display(&self) -> Cow<'_, str> {
        match self.kind {
            PathKind::Computer => Cow::Borrowed(COMPUTER_LABEL),
            PathKind::RecycleBin => Cow::Borrowed(RECYCLE_BIN_LABEL),
            PathKind::Home => Cow::Borrowed(HOME_LABEL),
            _ => name::display(&self.s),
        }
    }

    fn push(&mut self, seg: &[u8]) {
        if !self.is_root() {
            self.s.push(self.sep());
        }
        self.s.extend_from_slice(seg);
    }

    pub fn join(&self, child: &[u8]) -> Self {
        if matches!(self.kind, PathKind::Computer | PathKind::RecycleBin | PathKind::Home) {
            let mut b = child.to_vec();
            if child.first() != Some(&b'/') {
                b.push(b'\\');
            }
            return Self::parse_bytes(&b).unwrap_or_else(Self::computer);
        }
        let mut p = self.clone();
        p.push(child);
        p
    }

    fn parent_in_root(&self) -> Option<Self> {
        if self.is_root() {
            return None;
        }
        let sep = self.sep();
        let cut = self.s.iter().rposition(|&c| c == sep).unwrap_or(0).max(self.root as usize);
        let mut p = self.clone();
        p.s.truncate(cut);
        Some(p)
    }

    pub fn parent(&self) -> Option<Self> {
        match self.kind {
            PathKind::Computer | PathKind::RecycleBin | PathKind::Home => None,
            PathKind::Unix => self.parent_in_root(),
            _ => self.parent_in_root().or_else(|| Some(Self::computer())),
        }
    }

    pub fn file_name(&self) -> &[u8] {
        if self.is_root() {
            return self.root_label();
        }
        let sep = self.sep();
        let start = self.s.iter().rposition(|&c| c == sep).map_or(0, |p| p + 1).max(self.root as usize);
        &self.s[start..]
    }

    fn root_label(&self) -> &[u8] {
        match self.kind {
            PathKind::Computer => COMPUTER_LABEL.as_bytes(),
            PathKind::RecycleBin => RECYCLE_BIN_LABEL.as_bytes(),
            PathKind::Home => HOME_LABEL.as_bytes(),
            PathKind::Unix => b"/",
            _ => &self.s[..self.root as usize - 1],
        }
    }

    pub fn unc_parts(&self) -> Option<(&[u8], &[u8], &[u8])> {
        if self.kind != PathKind::Unc {
            return None;
        }
        let root = &self.s[2..self.root as usize - 1];
        let i = root.iter().position(|&c| c == b'\\')?;
        Some((&root[..i], &root[i + 1..], &self.s[self.root as usize..]))
    }

    pub fn components(&self) -> impl Iterator<Item = &[u8]> {
        let sep = self.sep();
        self.s[self.root as usize..].split(move |&c| c == sep).filter(|c| !c.is_empty())
    }

    pub fn crumbs(&self) -> Vec<(String, FsPath)> {
        let mut out = Vec::new();
        let mut p = Some(self.clone());
        while let Some(cur) = p {
            out.push((name::display(cur.file_name()).into_owned(), cur.clone()));
            p = cur.parent();
        }
        out.reverse();
        out
    }

    pub fn to_wide(&self, verbatim: bool) -> Vec<u16> {
        let mut w = Vec::with_capacity(self.s.len() + 9);
        match self.kind {
            PathKind::Drive if verbatim => w.extend(r"\\?\".encode_utf16()),
            PathKind::Unc if verbatim => {
                w.extend(r"\\?\UNC".encode_utf16());
                name::to_utf16(&self.s[1..], &mut w);
                w.push(0);
                return w;
            }
            _ => {}
        }
        name::to_utf16(&self.s, &mut w);
        w.push(0);
        w
    }
}

impl Serialize for FsPath {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.s.serialize(s)
    }
}

impl<'de> Deserialize<'de> for FsPath {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let b = Vec::<u8>::deserialize(d)?;
        FsPath::parse_exact(&b).ok_or_else(|| serde::de::Error::custom("invalid path"))
    }
}

fn trim(mut b: &[u8]) -> &[u8] {
    while let [first, rest @ ..] = b {
        if first.is_ascii_whitespace() || *first == b'"' { b = rest } else { break }
    }
    while let [rest @ .., last] = b {
        if last.is_ascii_whitespace() || *last == b'"' { b = rest } else { break }
    }
    b
}

#[cfg(test)]
#[path = "../test/path.rs"]
mod tests;
