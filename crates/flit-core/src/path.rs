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
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct FsPath {
    s: Vec<u8>,
    root: u16,
    kind: PathKind,
}

pub const COMPUTER_LABEL: &str = "This PC";

impl FsPath {
    pub fn computer() -> Self {
        Self { s: Vec::new(), root: 0, kind: PathKind::Computer }
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
        if input.contains(&0) {
            return None;
        }
        if input.is_empty() || input.eq_ignore_ascii_case(COMPUTER_LABEL.as_bytes()) {
            return Some(Self::computer());
        }
        if input[0] == b'/' && input.get(1) != Some(&b'/') {
            return Some(Self::with_segments(b"/".to_vec(), PathKind::Unix, b'/', &input[1..]));
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
            return Some(Self::with_segments(root, PathKind::Drive, b'\\', &rest[2..]));
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
            return Some(Self::with_segments(root, PathKind::Unc, b'\\', parts.next().unwrap_or(b"")));
        }
        if verbatim {
            let mut parts = rest.splitn(2, |&c| c == b'\\');
            let first = parts.next().filter(|s| !s.is_empty())?;
            let mut root = br"\\?\".to_vec();
            root.extend_from_slice(first);
            root.push(b'\\');
            return Some(Self::with_segments(root, PathKind::Verbatim, b'\\', parts.next().unwrap_or(b"")));
        }
        None
    }

    fn with_segments(root: Vec<u8>, kind: PathKind, sep: u8, rest: &[u8]) -> Self {
        let mut p = Self { root: root.len() as u16, s: root, kind };
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
        p
    }

    pub fn kind(&self) -> PathKind {
        self.kind
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.s
    }

    pub fn is_root(&self) -> bool {
        self.s.len() == self.root as usize
    }

    pub fn sep(&self) -> u8 {
        if self.kind == PathKind::Unix { b'/' } else { b'\\' }
    }

    pub fn display(&self) -> Cow<'_, str> {
        match self.kind {
            PathKind::Computer => Cow::Borrowed(COMPUTER_LABEL),
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
        if self.kind == PathKind::Computer {
            let mut b = child.to_vec();
            b.push(b'\\');
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
            PathKind::Computer => None,
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
mod tests {
    use super::*;

    fn p(s: &str) -> FsPath {
        FsPath::parse(s).unwrap_or_else(|| panic!("parse {s}"))
    }

    #[test]
    fn drives() {
        let a = p("c:/Users//user/./x/..");
        assert_eq!(a.as_bytes(), br"C:\Users\user");
        assert_eq!(a.kind(), PathKind::Drive);
        assert_eq!(p("C:").as_bytes(), br"C:\");
        assert!(p(r"C:\").is_root());
        assert_eq!(p(r"C:\..\..").as_bytes(), br"C:\");
        assert_eq!(p(r#" "D:\Games" "#).as_bytes(), br"D:\Games");
    }

    #[test]
    fn verbatim_and_unc() {
        assert_eq!(p(r"\\?\C:\Windows").as_bytes(), br"C:\Windows");
        let u = p(r"\\server\share\dir\f.txt");
        assert_eq!(u.kind(), PathKind::Unc);
        assert_eq!(u.as_bytes(), br"\\server\share\dir\f.txt");
        assert_eq!(p(r"\\?\UNC\server\share\dir"), p(r"\\server\share\dir"));
        assert!(FsPath::parse(r"\\server").is_none());
        let v = p(r"\\?\Volume{1234}\dir");
        assert_eq!(v.kind(), PathKind::Verbatim);
        assert_eq!(v.parent().unwrap().as_bytes(), br"\\?\Volume{1234}\");
        assert!(FsPath::parse("relative\\path").is_none());
        assert!(FsPath::parse_bytes(b"C:\\dir\\a\0.txt").is_none());
        assert!(FsPath::parse_bytes(b"/tmp/\0x").is_none());
    }

    #[test]
    fn unix() {
        let u = p("/home//user/./docs/../x");
        assert_eq!(u.as_bytes(), b"/home/user/x");
        assert_eq!(u.parent().unwrap().as_bytes(), b"/home/user");
        assert_eq!(p("/").parent(), None);
        assert_eq!(p("/home").parent().unwrap().as_bytes(), b"/");
        assert_eq!(p("/").join(b"etc").as_bytes(), b"/etc");
    }

    #[test]
    fn parent_join_name() {
        let a = p(r"C:\Users\user");
        assert_eq!(a.file_name(), b"user");
        assert_eq!(a.parent().unwrap().as_bytes(), br"C:\Users");
        assert_eq!(p(r"C:\Users").parent().unwrap().as_bytes(), br"C:\");
        assert_eq!(p(r"C:\").parent().unwrap(), FsPath::computer());
        assert_eq!(FsPath::computer().parent(), None);
        assert_eq!(FsPath::computer().join(b"D:"), p(r"D:\"));
        assert_eq!(p(r"C:\").join(b"x").as_bytes(), br"C:\x");
        assert_eq!(p(r"C:\").file_name(), b"C:");
        assert_eq!(p(r"\\srv\sh").file_name(), br"\\srv\sh");
    }

    #[test]
    fn crumbs() {
        let c: Vec<_> = p(r"C:\Users\b").crumbs().into_iter().map(|(l, _)| l).collect();
        assert_eq!(c, ["This PC", "C:", "Users", "b"]);
        let c: Vec<_> = p("/usr/lib").crumbs().into_iter().map(|(l, _)| l).collect();
        assert_eq!(c, ["/", "usr", "lib"]);
    }

    #[test]
    fn wide() {
        let w = |s: &str, v| String::from_utf16(&p(s).to_wide(v)[..]).unwrap();
        assert_eq!(w(r"C:\a", true), "\\\\?\\C:\\a\0");
        assert_eq!(w(r"C:\a", false), "C:\\a\0");
        assert_eq!(w(r"\\srv\sh\a", true), "\\\\?\\UNC\\srv\\sh\\a\0");
    }

    #[test]
    fn invalid_utf16_component() {
        let mut w: Vec<u16> = r"C:\bad".encode_utf16().collect();
        w.push(0xD800);
        let path = FsPath::from_utf16(&w).unwrap();
        let back = path.to_wide(false);
        assert_eq!(&back[..back.len() - 1], &w[..]);
        assert_eq!(path.display(), "C:\\bad\u{FFFD}");
    }

    #[test]
    fn long_path() {
        let long = format!(r"C:\{}", ["segment_name_long"; 30].join("\\"));
        let path = p(&long);
        assert!(path.as_bytes().len() > 260);
        assert!(path.to_wide(true).starts_with(&r"\\?\".encode_utf16().collect::<Vec<_>>()));
    }
}
