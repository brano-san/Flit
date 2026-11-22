use flit_core::i18n::S;
use flit_core::open::{Open, OpenTarget, Route};
use flit_core::ui::App;
use flit_core::{FsPath, Location};
use std::ffi::OsString;
use std::path::Path;
use std::time::Instant;

pub const HANDLER: &str = "--flit-link";

#[derive(Debug, PartialEq, Eq)]
pub enum Link {
    Folder(FsPath),
    SavedSearch(String),
}

pub fn is_link(bytes: &[u8]) -> bool {
    bytes.get(..5).is_some_and(|head| head.eq_ignore_ascii_case(b"flit:"))
}

pub fn cli(args: &[OsString]) -> Option<Result<Open, S>> {
    let route = match args {
        [mode, route] if mode == HANDLER => Some(route),
        [route] if is_link(route.as_encoded_bytes()) => Some(route),
        _ if args.iter().any(|arg| arg == HANDLER || is_link(arg.as_encoded_bytes())) => None,
        _ => return None,
    };

    Some(route.and_then(|route| route.to_str()).and_then(|uri| {
        parse(uri, cfg!(windows), &|_| true)?;
        Some(Open { paths: vec![uri.as_bytes().to_vec()], ..Default::default() })
    }).ok_or(S::SettingsPageSystemLinkInvalid))
}

pub fn handles(open: &Open) -> bool {
    open.paths.iter().chain(&open.select).any(|path| is_link(path))
}

pub fn resolve(open: &Open) -> Option<Link> {
    match (open.paths.as_slice(), open.select.is_empty(), open.properties, &open.tag) {
        ([raw], true, false, None) => std::str::from_utf8(raw).ok().and_then(|uri| parse(uri, cfg!(windows), &local_directory)),
        _ => None,
    }
}

pub fn open_in(app: &mut App, link: Option<Link>) {
    if !app.protocol_links {
        return app.status(S::SettingsPageSystemLinkDisabled.t());
    }
    match link {
        Some(Link::Folder(path)) => {
            let route = Route { folder: Location::Local(path), select: None };
            app.dispatch(vec![route], OpenTarget::NewTab, Instant::now());
        }
        Some(Link::SavedSearch(query)) if app.open_saved_search(&query) => {}
        _ => app.status(S::SettingsPageSystemLinkInvalid.t()),
    }
}

fn parse(uri: &str, windows: bool, directory: &dyn Fn(&Path) -> bool) -> Option<Link> {
    if uri.len() > 4096 || !is_link(uri.as_bytes()) || uri.chars().any(|c| c.is_whitespace() || c.is_control() || matches!(c, '"' | '#')) {
        return None;
    }
    let (action, field) = uri[5..].strip_prefix("//").unwrap_or(&uri[5..]).split_once('?')?;
    let (key, raw) = field.split_once('=')?;
    if raw.contains(['&', '?']) {
        return None;
    }
    let search = action.eq_ignore_ascii_case("search") && key == "q";
    let value = decode(raw, search)?;
    if value.trim().is_empty() || value.chars().any(char::is_control) {
        return None;
    }
    if search {
        return Some(Link::SavedSearch(value));
    }
    if !action.eq_ignore_ascii_case("open") || key != "path" {
        return None;
    }
    let path = folder(&value, windows)?;
    directory(Path::new(&value)).then_some(Link::Folder(path))
}

fn decode(raw: &str, form_spaces: bool) -> Option<String> {
    let mut raw = raw.bytes();
    let mut decoded = Vec::with_capacity(raw.len());
    while let Some(b) = raw.next() {
        match b {
            b'%' => {
                let hi = char::from(raw.next()?).to_digit(16)?;
                let lo = char::from(raw.next()?).to_digit(16)?;
                decoded.push((hi * 16 + lo) as u8);
            }
            b'+' if form_spaces => decoded.push(b' '),
            _ => decoded.push(b),
        }
    }
    String::from_utf8(decoded).ok()
}

fn folder(text: &str, windows: bool) -> Option<FsPath> {
    let separator = |c: char| c == '/' || (windows && c == '\\');
    let rest = if windows {
        match text.as_bytes() {
            [drive, b':', b'/' | b'\\', ..] if drive.is_ascii_alphabetic() => &text[3..],
            _ => return None,
        }
    } else {
        text.strip_prefix('/').filter(|rest| !rest.contains('\\'))?
    };
    if rest.starts_with(separator) || rest.split(separator).any(|part| {
        part == "." || part == ".." || (windows && (part.ends_with([' ', '.']) || part.contains([':', '"', '<', '>', '|', '*', '?'])))
    }) {
        return None;
    }
    FsPath::parse_exact(text.as_bytes())
}

fn local_directory(path: &Path) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows::Win32::Storage::FileSystem::GetDriveTypeW;
        let root: Vec<_> = path.as_os_str().encode_wide().take(2).chain([u16::from(b'\\'), 0]).collect();
        let kind = unsafe { GetDriveTypeW(windows::core::PCWSTR(root.as_ptr())) };
        if !matches!(kind, 2 | 3 | 5 | 6) {
            return false;
        }
    }
    let mut part = std::path::PathBuf::new();
    for component in path.components() {
        part.push(component);
        if matches!(component, std::path::Component::Prefix(_)) {
            continue;
        }
        let Ok(meta) = std::fs::symlink_metadata(&part) else { return false };
        if !meta.is_dir() {
            return false;
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
            if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
                return false;
            }
        }
    }
    true
}

#[cfg(test)]
#[path = "../test/system_link.rs"]
mod tests;

