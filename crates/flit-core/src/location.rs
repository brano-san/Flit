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

pub fn valid_distro(name: &str) -> bool {
    (1..=64).contains(&name.len()) && name != "." && name != ".." && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

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

    pub fn recycle_bin() -> Self {
        Location::Local(FsPath::recycle_bin())
    }

    pub fn is_recycle_bin(&self) -> bool {
        self.path().kind() == PathKind::RecycleBin
    }

    pub fn provider(&self) -> ProviderId {
        match self {
            Location::Local(_) => ProviderId::Local,
            Location::Wsl { distro, .. } => ProviderId::Wsl(distro.clone()),
        }
    }

    pub fn canonicalize(&mut self, distros: &[String]) {
        if let Location::Wsl { distro, .. } = self {
            if let Some(known) = distros.iter().find(|d| d.eq_ignore_ascii_case(distro)) {
                distro.clone_from(known);
            }
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
            let distro = std::str::from_utf8(distro).ok().filter(|d| valid_distro(d))?;
            let path = FsPath::parse_exact(path).filter(|p| p.kind() == PathKind::Unix)?;
            return Some(Location::Wsl { distro: distro.to_owned(), path });
        }
        FsPath::parse_exact(b).map(Self::from_local)
    }

    pub fn from_local(p: FsPath) -> Self {
        if let Some((server, share, rest)) = p.unc_parts() {
            if WSL_HOSTS.iter().any(|h| server.eq_ignore_ascii_case(h)) {
                if let Some(distro) = std::str::from_utf8(share).ok().filter(|d| valid_distro(d)) {
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
            Location::Local(p) if p.kind() == PathKind::RecycleBin => None,
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
            Location::Local(p) if p.kind() == PathKind::Computer && std::str::from_utf8(child).is_ok_and(valid_distro) => {
                Location::Wsl { distro: String::from_utf8_lossy(child).into_owned(), path: FsPath::parse("/").unwrap() }
            }
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
#[path = "../test/location.rs"]
mod tests;
