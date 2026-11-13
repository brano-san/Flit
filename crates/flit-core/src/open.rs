use crate::location::Location;
use crate::name;
use crate::path::{FsPath, PathKind};
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
    pub tag: Option<String>,
    pub properties: bool,
    pub activation: Option<String>,
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
    if let Some(k) = args.iter().position(|a| eq_ci(a, "-tag")) {
        let name = args.get(k + 1).map(|n| name::display(unquote(n)).into_owned());
        let paths = args.iter().skip(k + 2).map(|a| unquote(a).to_vec()).filter(|a| !a.is_empty()).collect();
        return Open { paths, cwd, tag: Some(name.unwrap_or_default()), ..Default::default() };
    }
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

const THIS_PC: [&str; 5] = [
    "::{20D04FE0-3AEA-1069-A2D8-08002B30309D}",
    "shell:::{20D04FE0-3AEA-1069-A2D8-08002B30309D}",
    "shell:MyComputerFolder",
    "shell:::{52205fd8-5dfb-447d-801a-d0b52f2e83e1}",
    "::{52205fd8-5dfb-447d-801a-d0b52f2e83e1}",
];

const RECYCLE_BIN: [&str; 3] = ["shell:RecycleBinFolder", "::{645FF040-5081-101B-9F08-00AA002F954E}", "shell:::{645FF040-5081-101B-9F08-00AA002F954E}"];

const HOME: [&str; 2] = [crate::path::HOME_SHELL, "::{f874310e-b6b7-47dc-bc84-b9e6b38f5903}"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResolveError {
    UnknownShellLocation(String),
    Invalid(String),
}

pub fn resolve(raw: &[u8], cwd: Option<&[u8]>, shell_folder: &dyn Fn(&str) -> Option<Location>) -> Result<Location, ResolveError> {
    resolve_for(raw, cwd, shell_folder, false)
}

pub fn resolve_for(raw: &[u8], cwd: Option<&[u8]>, shell_folder: &dyn Fn(&str) -> Option<Location>, unix: bool) -> Result<Location, ResolveError> {
    let expanded = if unix { expand_unix(unquote(raw), &|n| std::env::var(n).ok()) } else { expand_environment(unquote(raw)) };
    let raw = expanded.as_deref().unwrap_or_else(|| unquote(raw));
    let text = name::display(raw);
    if unix && strip_prefix_ci(raw, b"trash:").is_some() {
        return Ok(Location::recycle_bin());
    }
    if THIS_PC.iter().any(|t| text.eq_ignore_ascii_case(t)) {
        return Ok(Location::computer());
    }
    if RECYCLE_BIN.iter().any(|t| text.eq_ignore_ascii_case(t)) {
        return Ok(Location::recycle_bin());
    }
    if HOME.iter().any(|t| text.eq_ignore_ascii_case(t)) {
        return Ok(Location::home_page());
    }
    if strip_prefix_ci(raw, b"shell:").is_some() || raw.starts_with(b"::{") {
        return shell_folder(&text).ok_or_else(|| ResolveError::UnknownShellLocation(text.into_owned()));
    }
    let path = match strip_prefix_ci(raw, b"file://") {
        Some(url) => file_url_path(&percent_decode(url)),
        None => Some(raw.to_vec()),
    };
    path.and_then(|p| resolve_path(&p, cwd.and_then(FsPath::parse_bytes))).ok_or_else(|| ResolveError::Invalid(text.into_owned()))
}

/// Expands Windows-style `%NAME%` variables before a path is parsed.  Keeping
/// this next to path resolution makes the address bar and command-line opens
/// behave identically, while leaving unknown variables untouched.
fn expand_environment(raw: &[u8]) -> Option<Vec<u8>> {
    let text = std::str::from_utf8(raw).ok()?;
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    let mut changed = false;
    while let Some(open) = rest.find('%') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let Some(close) = after.find('%') else {
            out.push('%');
            out.push_str(after);
            break;
        };
        let name = &after[..close];
        let value = (!name.is_empty()).then(|| std::env::vars_os().find_map(|(key, value)| key.to_string_lossy().eq_ignore_ascii_case(name).then(|| value.to_string_lossy().into_owned()))).flatten();
        match value {
            Some(value) => {
                out.push_str(&value);
                changed = true;
            }
            None => {
                out.push('%');
                out.push_str(name);
                out.push('%');
            }
        }
        rest = &after[close + 1..];
    }
    if rest.is_empty() || !changed {
        return changed.then_some(out.into_bytes());
    }
    out.push_str(rest);
    Some(out.into_bytes())
}

pub fn expand_unix(raw: &[u8], env: &dyn Fn(&str) -> Option<String>) -> Option<Vec<u8>> {
    let text = std::str::from_utf8(raw).ok()?;
    let mut out = String::with_capacity(text.len());
    let (mut rest, mut changed) = (text, false);
    if let Some(tail) = text.strip_prefix('~').filter(|t| t.is_empty() || t.starts_with('/')) {
        if let Some(home) = env("HOME") {
            out.push_str(&home);
            rest = tail;
            changed = true;
        }
    }
    while let Some(i) = rest.find('$') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let braced = after.strip_prefix('{').and_then(|b| b.split_once('}'));
        let (name, tail) = match (braced, after.starts_with('{')) {
            (Some(split), _) => split,
            (None, true) => ("", after),
            (None, false) => after.split_at(after.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).unwrap_or(after.len())),
        };
        let valid = name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        match valid.then(|| env(name)).flatten() {
            Some(value) => {
                out.push_str(&value);
                changed = true;
                rest = tail;
            }
            None => {
                out.push('$');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    changed.then(|| out.into_bytes())
}

pub fn typed_as_path(text: &str) -> bool {
    let t = text.trim();
    t.starts_with(['/', '~', '$']) && !t.contains(char::is_whitespace)
}

fn strip_prefix_ci<'a>(b: &'a [u8], prefix: &[u8]) -> Option<&'a [u8]> {
    b.get(..prefix.len()).filter(|p| p.eq_ignore_ascii_case(prefix)).map(|_| &b[prefix.len()..])
}

fn file_url_path(url: &[u8]) -> Option<Vec<u8>> {
    let (host, path) = url.split_at(url.iter().position(|&c| c == b'/')?);
    if !host.is_empty() && !host.eq_ignore_ascii_case(b"localhost") {
        return Some([br"\\", url].concat());
    }
    let drive = &path[1..];
    Some(if matches!(drive, [d, b':', ..] if d.is_ascii_alphabetic()) { drive } else { path }.to_vec())
}

fn resolve_path(raw: &[u8], cwd: Option<FsPath>) -> Option<Location> {
    let is_sep = |c: &u8| matches!(c, b'\\' | b'/');
    if let Some(c) = cwd.as_ref().filter(|c| matches!(c.kind(), PathKind::Drive | PathKind::Unc | PathKind::Verbatim)) {
        if raw.first().is_some_and(is_sep) && !raw.get(1).is_some_and(is_sep) {
            return Location::parse_bytes(&[c.root_bytes(), raw].concat());
        }
        if let [d, b':', rest @ ..] = raw {
            if c.kind() == PathKind::Drive && c.as_bytes()[0].eq_ignore_ascii_case(d) && rest.first().is_some_and(|r| !is_sep(r)) {
                return Location::parse_bytes(&[c.as_bytes(), b"\\", rest].concat());
            }
        }
    }
    if let Some(loc) = Location::parse_bytes(raw) {
        return Some(loc);
    }
    let base = cwd.filter(|_| !raw.get(..2).is_some_and(|p| p.iter().all(is_sep)))?;
    Location::parse_bytes(&[base.as_bytes(), &[base.sep()], raw].concat())
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
    routes_for(open, shell_folder, false)
}

pub fn routes_for(open: &Open, shell_folder: &dyn Fn(&str) -> Option<Location>, unix: bool) -> (Vec<Route>, Vec<ResolveError>) {
    let cwd = open.cwd.as_deref();
    let (mut ok, mut failed) = (Vec::new(), Vec::new());
    for raw in &open.paths {
        match resolve_for(raw, cwd, shell_folder, unix) {
            Ok(folder) => ok.push(Route { folder, select: None }),
            Err(e) => failed.push(e),
        }
    }
    for raw in &open.select {
        match resolve_for(raw, cwd, shell_folder, unix) {
            Ok(item) => match item.parent() {
                Some(folder) => ok.push(Route { select: Some(item.file_name().to_vec()), folder }),
                None => ok.push(Route { folder: item, select: None }),
            },
            Err(e) => failed.push(e),
        }
    }
    (ok, failed)
}

pub fn explorer_only(open: &Open, shell_folder: &dyn Fn(&str) -> Option<Location>) -> Vec<String> {
    if open.tag.is_some() {
        return Vec::new();
    }
    let (routes, errors) = routes(open, shell_folder);
    let shell: Vec<String> = errors.iter().filter_map(|e| if let ResolveError::UnknownShellLocation(s) = e { Some(s.clone()) } else { None }).collect();
    if routes.is_empty() && shell.len() == errors.len() {
        shell
    } else {
        Vec::new()
    }
}

#[cfg(test)]
#[path = "../test/open.rs"]
mod tests;
