use flit_core::FsPath;
use std::path::Path;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Link {
    Folder(FsPath),
    SavedSearch(String),
}

pub(crate) fn is_link(bytes: &[u8]) -> bool {
    bytes.get(..5).is_some_and(|head| head.eq_ignore_ascii_case(b"flit:"))
}

pub(crate) fn parse(uri: &str, windows: bool, directory: &dyn Fn(&Path) -> bool) -> Option<Link> {
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
    if rest.starts_with(separator) || rest.split(separator).any(|part| part == "." || part == ".." || (windows && (part.ends_with([' ', '.']) || part.contains([':', '"', '<', '>', '|', '*', '?'])))) {
        return None;
    }
    FsPath::parse_exact(text.as_bytes())
}
