use crate::name;
use crate::path::{FsPath, PathKind, COMPUTER_LABEL};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum Location {
    Local(FsPath),
    Wsl { distro: String, path: FsPath },
}

#[derive(Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub enum ProviderId {
    Local,
    Wsl(String),
}

pub const WSL_SCHEME: &str = "wsl://";
const WSL_HOSTS: [&[u8]; 2] = [b"wsl.localhost", b"wsl$"];

fn wsl_escaped(b: u8) -> bool {
    matches!(b, 0x01..=0x1F | b'"' | b'*' | b':' | b'<' | b'>' | b'?' | b'\\' | b'|')
}

fn wsl_unescape(component: &[u8], out: &mut Vec<u8>) {
    let mut i = 0;
    while i < component.len() {
        let (cp, n) = name::decode(&component[i..]);
        if (0xF001..=0xF07F).contains(&cp) && wsl_escaped((cp - 0xF000) as u8) {
            out.push((cp - 0xF000) as u8);
        } else {
            out.extend_from_slice(&component[i..i + n]);
        }
        i += n;
    }
}

fn wsl_escape(component: &[u8], out: &mut Vec<u8>) -> Option<()> {
    let s = std::str::from_utf8(component).ok()?;
    for c in s.chars() {
        if c.is_ascii() && wsl_escaped(c as u8) {
            name::push_cp(out, 0xF000 + c as u32);
        } else {
            let mut b = [0; 4];
            out.extend_from_slice(c.encode_utf8(&mut b).as_bytes());
        }
    }
    Some(())
}

impl Location {
    pub fn computer() -> Self {
        Location::Local(FsPath::computer())
    }

    pub fn provider(&self) -> ProviderId {
        match self {
            Location::Local(_) => ProviderId::Local,
            Location::Wsl { distro, .. } => ProviderId::Wsl(distro.clone()),
        }
    }

    pub fn path(&self) -> &FsPath {
        match self {
            Location::Local(p) | Location::Wsl { path: p, .. } => p,
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::parse_bytes(s.as_bytes())
    }

    pub fn parse_bytes(b: &[u8]) -> Option<Self> {
        let t = b.trim_ascii();
        let t = t.strip_prefix(b"\"").unwrap_or(t);
        let t = t.strip_suffix(b"\"").unwrap_or(t);
        if t.len() >= WSL_SCHEME.len() && t[..WSL_SCHEME.len()].eq_ignore_ascii_case(WSL_SCHEME.as_bytes()) {
            return Self::parse_exact(t);
        }
        FsPath::parse_bytes(t).map(Self::from_local)
    }

    pub fn parse_exact(b: &[u8]) -> Option<Self> {
        if b.len() >= WSL_SCHEME.len() && b[..WSL_SCHEME.len()].eq_ignore_ascii_case(WSL_SCHEME.as_bytes()) {
            let rest = &b[WSL_SCHEME.len()..];
            let (distro, path) = match rest.iter().position(|&c| c == b'/') {
                Some(i) => (&rest[..i], &rest[i..]),
                None => (rest, &b"/"[..]),
            };
            let distro = std::str::from_utf8(distro).ok().filter(|d| !d.is_empty() && !d.contains('\\'))?;
            let path = FsPath::parse_exact(path).filter(|p| p.kind() == PathKind::Unix)?;
            return Some(Location::Wsl { distro: distro.to_owned(), path });
        }
        FsPath::parse_exact(b).map(Self::from_local)
    }

    pub fn from_local(p: FsPath) -> Self {
        if let Some((server, share, rest)) = p.unc_parts() {
            if WSL_HOSTS.iter().any(|h| server.eq_ignore_ascii_case(h)) {
                if let Ok(distro) = std::str::from_utf8(share) {
                    let mut linux = b"/".to_vec();
                    for (i, c) in rest.split(|&c| c == b'\\').filter(|c| !c.is_empty()).enumerate() {
                        if i > 0 {
                            linux.push(b'/');
                        }
                        wsl_unescape(c, &mut linux);
                    }
                    if let Some(path) = FsPath::parse_exact(&linux) {
                        return Location::Wsl { distro: distro.to_owned(), path };
                    }
                }
            }
        }
        Location::Local(p)
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        match self {
            Location::Local(p) => p.as_bytes().to_vec(),
            Location::Wsl { distro, path } => [WSL_SCHEME.as_bytes(), distro.as_bytes(), path.as_bytes()].concat(),
        }
    }

    pub fn to_windows(&self) -> Option<FsPath> {
        match self {
            Location::Local(p) => Some(p.clone()),
            Location::Wsl { distro, path } => {
                let mut b = format!(r"\\wsl.localhost\{distro}").into_bytes();
                for c in path.components() {
                    b.push(b'\\');
                    wsl_escape(c, &mut b)?;
                }
                FsPath::parse_exact(&b)
            }
        }
    }

    pub fn display(&self) -> Cow<'_, str> {
        match self {
            Location::Local(p) => p.display(),
            Location::Wsl { .. } => Cow::Owned(name::display(&self.to_bytes()).into_owned()),
        }
    }

    pub fn is_root(&self) -> bool {
        self.path().is_root()
    }

    pub fn join(&self, child: &[u8]) -> Self {
        match self {
            Location::Local(p) => Location::Local(p.join(child)),
            Location::Wsl { distro, path } => Location::Wsl { distro: distro.clone(), path: path.join(child) },
        }
    }

    pub fn parent(&self) -> Option<Self> {
        match self {
            Location::Local(p) => p.parent().map(Location::Local),
            Location::Wsl { distro, path } => match path.parent() {
                Some(up) => Some(Location::Wsl { distro: distro.clone(), path: up }),
                None => Some(Self::computer()),
            },
        }
    }

    pub fn file_name(&self) -> &[u8] {
        match self {
            Location::Wsl { distro, path } if path.is_root() => distro.as_bytes(),
            _ => self.path().file_name(),
        }
    }

    pub fn crumbs(&self) -> Vec<(String, Location)> {
        let mut out = Vec::new();
        let mut cur = Some(self.clone());
        while let Some(loc) = cur {
            let label = match &loc {
                Location::Local(p) if p.kind() == PathKind::Computer => COMPUTER_LABEL.to_owned(),
                Location::Wsl { distro, path } if path.is_root() => format!("{distro} (WSL)"),
                _ => name::display(loc.file_name()).into_owned(),
            };
            cur = loc.parent();
            out.push((label, loc));
        }
        out.reverse();
        out
    }
}

impl Serialize for Location {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.to_bytes().serialize(s)
    }
}

impl<'de> Deserialize<'de> for Location {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let b = Vec::<u8>::deserialize(d)?;
        Location::parse_exact(&b).ok_or_else(|| serde::de::Error::custom("invalid location"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wsl(distro: &str, path: &[u8]) -> Location {
        Location::Wsl { distro: distro.into(), path: FsPath::parse_exact(path).unwrap() }
    }

    #[test]
    fn unc_forms_map_to_wsl() {
        let want = wsl("Ubuntu", b"/home/user/src");
        assert_eq!(Location::parse(r"\\wsl.localhost\Ubuntu\home\user\src").unwrap(), want);
        assert_eq!(Location::parse(r"\\wsl$\Ubuntu\home\user\src").unwrap(), want);
        assert_eq!(Location::parse(r"\\WSL.LOCALHOST\Ubuntu\home\user\src\").unwrap(), want);
        assert_eq!(Location::parse("wsl://Ubuntu/home/user/src").unwrap(), want);
        assert_eq!(Location::parse(r"\\wsl$\Ubuntu").unwrap(), wsl("Ubuntu", b"/"));
        assert_eq!(Location::parse("wsl://Debian").unwrap(), wsl("Debian", b"/"));
        assert!(matches!(Location::parse(r"\\nas\share\x").unwrap(), Location::Local(_)));
        assert!(Location::parse("wsl:///etc").is_none(), "distro required");
    }

    #[test]
    fn wsl_to_windows_and_back() {
        let loc = wsl("Ubuntu-22.04", b"/home/u/My Docs");
        let win = loc.to_windows().unwrap();
        assert_eq!(win.as_bytes(), br"\\wsl.localhost\Ubuntu-22.04\home\u\My Docs");
        assert_eq!(Location::from_local(win), loc);
        assert_eq!(wsl("U", b"/").to_windows().unwrap().as_bytes(), br"\\wsl.localhost\U\");
    }

    #[test]
    fn linux_names_with_windows_forbidden_chars() {
        let loc = wsl("Ubuntu", b"/tmp/a:b\\c?d*e|f");
        assert_eq!(loc.file_name(), b"a:b\\c?d*e|f");
        let win = loc.to_windows().unwrap();
        let shown = name::display(win.file_name()).into_owned();
        assert_eq!(shown, "a\u{F03A}b\u{F05C}c\u{F03F}d\u{F02A}e\u{F07C}f");
        assert_eq!(Location::from_local(win), loc, "round trip through the 9P view");
        let raw = wsl("Ubuntu", b"/tmp/bad\xFF\xFEname");
        assert_eq!(raw.file_name(), b"bad\xFF\xFEname");
        assert!(raw.to_windows().is_none());
        assert_eq!(Location::parse_exact(&raw.to_bytes()).unwrap(), raw, "canonical form is lossless");
    }

    #[test]
    fn navigation() {
        let loc = wsl("Ubuntu", b"/home/u");
        assert_eq!(loc.parent().unwrap(), wsl("Ubuntu", b"/home"));
        assert_eq!(wsl("Ubuntu", b"/").parent().unwrap(), Location::computer());
        assert_eq!(loc.join(b"x:y"), wsl("Ubuntu", b"/home/u/x:y"));
        let labels: Vec<String> = loc.crumbs().into_iter().map(|c| c.0).collect();
        assert_eq!(labels, ["This PC", "Ubuntu (WSL)", "home", "u"]);
        assert_eq!(loc.provider(), ProviderId::Wsl("Ubuntu".into()));
        assert_eq!(Location::parse(r"C:\x").unwrap().provider(), ProviderId::Local);
        assert_eq!(loc.display(), "wsl://Ubuntu/home/u");
    }

    #[test]
    fn serde_roundtrip_and_validation() {
        let locs = [Location::computer(), Location::parse(r"C:\Users").unwrap(), wsl("Ubuntu", b"/a\xFFb"), Location::parse(r"\\srv\s\d").unwrap()];
        for l in locs {
            let bytes = postcard::to_allocvec(&l).unwrap();
            assert_eq!(postcard::from_bytes::<Location>(&bytes).unwrap(), l);
        }
        let forged = postcard::to_allocvec(&b"relative\\x".to_vec()).unwrap();
        assert!(postcard::from_bytes::<FsPath>(&forged).is_err());
    }
}
