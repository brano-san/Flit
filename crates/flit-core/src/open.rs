use crate::location::Location;
use crate::name;
use crate::path::FsPath;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Hash, Serialize, Deserialize)]
pub enum OpenTarget {
    #[default]
    Default,
    NewTab,
    NewWindow,
    CurrentTab,
}

#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Open {
    pub paths: Vec<Vec<u8>>,
    pub select: Vec<Vec<u8>>,
    pub target: OpenTarget,
    pub cwd: Option<Vec<u8>>,
}

impl Open {
    pub fn location(loc: &Location, target: OpenTarget) -> Self {
        Self { paths: vec![loc.to_bytes()], target, ..Default::default() }
    }

    pub fn is_empty(&self) -> bool {
        self.paths.is_empty() && self.select.is_empty()
    }
}

fn eq_ci(a: &[u8], b: &str) -> bool {
    a.eq_ignore_ascii_case(b.as_bytes())
}

fn unquote(b: &[u8]) -> &[u8] {
    let b = b.trim_ascii();
    let b = b.strip_prefix(b"\"").unwrap_or(b);
    b.strip_suffix(b"\"").unwrap_or(b).trim_ascii()
}

pub fn parse_args(args: &[Vec<u8>], cwd: Option<Vec<u8>>) -> Open {
    let mut open = Open { cwd, ..Default::default() };
    let mut dangling: Option<bool> = None;
    for arg in args {
        if let Some(select) = dangling.take() {
            let v = unquote(arg);
            if !v.is_empty() && v[0] != b'/' && !v.starts_with(b"--") {
                if select { open.select.push(v.to_vec()) } else { open.paths.push(v.to_vec()) }
                continue;
            }
        }
        let mut tok: &[u8] = arg;
        loop {
            if tok.is_empty() {
                break;
            }
            match tok {
                t if eq_ci(t, "--new-tab") => open.target = OpenTarget::NewTab,
                t if eq_ci(t, "--new-window") => open.target = OpenTarget::NewWindow,
                t if eq_ci(t, "--current-tab") => open.target = OpenTarget::CurrentTab,
                _ => {}
            }
            if tok.starts_with(b"--") {
                break;
            }
            let (switch, rest) = match tok.iter().position(|&c| c == b',') {
                Some(i) if tok[0] == b'/' => (&tok[..i], &tok[i + 1..]),
                _ => (tok, &b""[..]),
            };
            let s = switch;
            if tok[0] == b'/' && (eq_ci(s, "/n") || eq_ci(s, "/e") || eq_ci(s, "/separate")) {
                if eq_ci(s, "/n") {
                    open.target = OpenTarget::NewWindow;
                }
                tok = rest;
                continue;
            }
            if tok[0] == b'/' && (eq_ci(s, "/select") || eq_ci(s, "/root")) {
                let v = unquote(rest);
                if v.is_empty() {
                    dangling = Some(eq_ci(s, "/select"));
                } else if eq_ci(s, "/select") {
                    open.select.push(v.to_vec())
                } else {
                    open.paths.push(v.to_vec())
                }
                break;
            }
            if tok[0] == b'/' && eq_ci(s, "/idlist") {
                break;
            }
            let v = unquote(tok);
            if !v.is_empty() {
                open.paths.push(v.to_vec());
            }
            break;
        }
    }
    open
}

const THIS_PC: [&str; 4] = [
    "::{20D04FE0-3AEA-1069-A2D8-08002B30309D}",
    "shell:::{20D04FE0-3AEA-1069-A2D8-08002B30309D}",
    "shell:MyComputerFolder",
    "shell:::{52205fd8-5dfb-447d-801a-d0b52f2e83e1}",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResolveError {
    UnknownShellLocation(String),
    Invalid(String),
}

pub fn resolve(raw: &[u8], cwd: Option<&[u8]>, shell_folder: &dyn Fn(&str) -> Option<Location>) -> Result<Location, ResolveError> {
    let raw = unquote(raw);
    let text = name::display(raw);
    if THIS_PC.iter().any(|t| text.eq_ignore_ascii_case(t)) {
        return Ok(Location::computer());
    }
    if text.len() >= 6 && text[..6].eq_ignore_ascii_case("shell:") || text.starts_with("::{") {
        return shell_folder(&text).ok_or_else(|| ResolveError::UnknownShellLocation(text.into_owned()));
    }
    if text.len() >= 8 && text[..8].eq_ignore_ascii_case("file:///") {
        let decoded = percent_decode(&raw[8..]);
        let local = if decoded.get(1) == Some(&b':') { decoded } else { [b"/".as_slice(), &decoded].concat() };
        return Location::parse_bytes(&local).ok_or_else(|| ResolveError::Invalid(text.into_owned()));
    }
    if let Some(loc) = Location::parse_bytes(raw) {
        return Ok(loc);
    }
    let base = cwd.and_then(FsPath::parse_bytes).ok_or_else(|| ResolveError::Invalid(text.clone().into_owned()))?;
    let joined = [base.as_bytes(), &[base.sep()], raw].concat();
    Location::parse_bytes(&joined).ok_or_else(|| ResolveError::Invalid(text.into_owned()))
}

fn percent_decode(b: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Route {
    pub folder: Location,
    pub select: Option<Vec<u8>>,
}

pub fn routes(open: &Open, shell_folder: &dyn Fn(&str) -> Option<Location>) -> (Vec<Route>, Vec<ResolveError>) {
    let cwd = open.cwd.as_deref();
    let (mut ok, mut failed) = (Vec::new(), Vec::new());
    for raw in &open.paths {
        match resolve(raw, cwd, shell_folder) {
            Ok(folder) => ok.push(Route { folder, select: None }),
            Err(e) => failed.push(e),
        }
    }
    for raw in &open.select {
        match resolve(raw, cwd, shell_folder) {
            Ok(item) => match item.parent() {
                Some(folder) => ok.push(Route { select: Some(item.file_name().to_vec()), folder }),
                None => ok.push(Route { folder: item, select: None }),
            },
            Err(e) => failed.push(e),
        }
    }
    (ok, failed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Vec<Vec<u8>> {
        a.iter().map(|s| s.as_bytes().to_vec()).collect()
    }

    fn strs(v: &[Vec<u8>]) -> Vec<String> {
        v.iter().map(|b| String::from_utf8_lossy(b).into_owned()).collect()
    }

    #[test]
    fn explorer_forms() {
        let o = parse_args(&args(&[r"C:\Windows"]), None);
        assert_eq!(strs(&o.paths), [r"C:\Windows"]);
        assert_eq!(o.target, OpenTarget::Default);

        let o = parse_args(&args(&[r"/select,C:\a b\c,d.txt"]), None);
        assert_eq!(strs(&o.select), [r"C:\a b\c,d.txt"]);
        assert!(o.paths.is_empty());

        let o = parse_args(&args(&["/n,/e,/select,C:\\x\\y.txt"]), None);
        assert_eq!(o.target, OpenTarget::NewWindow);
        assert_eq!(strs(&o.select), [r"C:\x\y.txt"]);

        let o = parse_args(&args(&["/e,", r"C:\Users"]), None);
        assert_eq!(strs(&o.paths), [r"C:\Users"]);

        let o = parse_args(&args(&["/root,\"C:\\Program Files\""]), None);
        assert_eq!(strs(&o.paths), [r"C:\Program Files"]);

        let o = parse_args(&args(&["/SELECT,", "/E,C:\\"]), None);
        assert!(o.select.is_empty(), "empty /select, ignored");

        let o = parse_args(&args(&["/select,", r"C:\a b\c.txt"]), None);
        assert_eq!(strs(&o.select), [r"C:\a b\c.txt"]);
        assert!(o.paths.is_empty());

        let o = parse_args(&args(&["shell:Downloads", "::{20D04FE0-3AEA-1069-A2D8-08002B30309D}"]), None);
        assert_eq!(strs(&o.paths), ["shell:Downloads", "::{20D04FE0-3AEA-1069-A2D8-08002B30309D}"]);

        let o = parse_args(&args(&["/idlist,:1234:5678", r"C:\"]), None);
        assert_eq!(strs(&o.paths), [r"C:\"]);

        let o = parse_args(&args(&["--new-window", "/home/user", "--current-tab"]), Some(b"/".to_vec()));
        assert_eq!(strs(&o.paths), ["/home/user"], "Linux paths are not switches");
        assert_eq!(o.target, OpenTarget::CurrentTab);
    }

    #[test]
    fn resolution() {
        let none = |_: &str| None;
        let shell = |n: &str| (n.eq_ignore_ascii_case("shell:Downloads")).then(|| Location::parse(r"C:\Users\u\Downloads").unwrap());
        let cwd = Some(&br"D:\work"[..]);
        let r = |s: &str| resolve(s.as_bytes(), cwd, &shell);
        assert_eq!(r(r"C:\x").unwrap(), Location::parse(r"C:\x").unwrap());
        assert_eq!(r(r"src\lib").unwrap(), Location::parse(r"D:\work\src\lib").unwrap());
        assert_eq!(r(r"..\other").unwrap(), Location::parse(r"D:\other").unwrap());
        assert_eq!(r("::{20d04fe0-3aea-1069-a2d8-08002b30309d}").unwrap(), Location::computer());
        assert_eq!(r("shell:downloads").unwrap(), Location::parse(r"C:\Users\u\Downloads").unwrap());
        assert!(matches!(r("shell:RecycleBinFolder"), Err(ResolveError::UnknownShellLocation(_))));
        assert_eq!(r("file:///C:/My%20Files/x").unwrap(), Location::parse(r"C:\My Files\x").unwrap());
        assert!(r("file:///C:/Docs/%00x").is_err());
        assert_eq!(r(r"\\wsl$\Ubuntu\etc").unwrap(), Location::parse("wsl://Ubuntu/etc").unwrap());
        assert!(resolve(b"relative", None, &none).is_err());
    }

    #[test]
    fn select_routes_to_parent() {
        let o = parse_args(&args(&[r"/select,C:\dir\file.txt", r"D:\"]), None);
        let (routes, failed) = routes(&o, &|_: &str| None);
        assert!(failed.is_empty());
        assert_eq!(routes[0], Route { folder: Location::parse(r"D:\").unwrap(), select: None });
        assert_eq!(routes[1], Route { folder: Location::parse(r"C:\dir").unwrap(), select: Some(b"file.txt".to_vec()) });
    }

    #[test]
    fn open_serializes() {
        let o = Open { paths: vec![b"C:\\x".to_vec(), vec![0xED, 0xA0, 0x80]], select: vec![b"/tmp/a:b".to_vec()], target: OpenTarget::NewTab, cwd: Some(b"C:\\".to_vec()) };
        let bytes = postcard::to_allocvec(&o).unwrap();
        assert_eq!(postcard::from_bytes::<Open>(&bytes).unwrap(), o);
    }
}
