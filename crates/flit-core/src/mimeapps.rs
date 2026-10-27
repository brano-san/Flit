pub const DEFAULTS: &str = "[Default Applications]";
pub const FOLDER: &str = "inode/directory";
pub const FOLDER_TYPES: [&str; 2] = [FOLDER, "x-directory/normal"];
pub const FLIT_DESKTOP: &str = "flit.desktop";
const FILE: &str = "file\n";
const NO_FILE: &str = "nofile\n";

fn group_of(line: &str) -> Option<&str> {
    let l = line.trim();
    (l.starts_with('[') && l.ends_with(']')).then_some(l)
}

fn key_of<'a>(line: &'a str, mime: &str) -> Option<&'a str> {
    let (k, v) = line.split_once('=')?;
    (k.trim() == mime).then(|| v.trim())
}

pub fn default_app(text: &str, mime: &str) -> Option<String> {
    let mut inside = false;
    for line in text.lines() {
        if let Some(g) = group_of(line) {
            inside = g == DEFAULTS;
        } else if inside {
            if let Some(v) = key_of(line, mime) {
                return Some(v.to_owned());
            }
        }
    }
    None
}

pub fn set_default_app(text: &str, mime: &str, app: Option<&str>) -> String {
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    let entry = app.map(|a| format!("{mime}={a}"));
    match lines.iter().position(|l| group_of(l) == Some(DEFAULTS)) {
        None => {
            if let Some(e) = entry {
                if lines.last().is_some_and(|l| !l.trim().is_empty()) {
                    lines.push(String::new());
                }
                lines.extend([DEFAULTS.to_owned(), e]);
            }
        }
        Some(s) => {
            let end = lines[s + 1..].iter().position(|l| group_of(l).is_some()).map_or(lines.len(), |k| s + 1 + k);
            match ((s + 1..end).find(|&k| key_of(&lines[k], mime).is_some()), entry) {
                (Some(k), Some(e)) => lines[k] = e,
                (Some(k), None) => {
                    lines.remove(k);
                }
                (None, Some(e)) => {
                    let at = (s + 1..end).rev().find(|&k| !lines[k].trim().is_empty()).map_or(s + 1, |k| k + 1);
                    lines.insert(at, e);
                }
                (None, None) => {}
            }
        }
    }
    lines.iter().map(|l| format!("{l}\n")).collect()
}

pub fn backup(original: Option<&str>) -> String {
    match original {
        Some(t) => format!("{FILE}{t}"),
        None => NO_FILE.to_owned(),
    }
}

pub fn ours(original: Option<&str>) -> String {
    FOLDER_TYPES.iter().fold(original.unwrap_or_default().to_owned(), |text, mime| set_default_app(&text, mime, Some(FLIT_DESKTOP)))
}

pub fn original(backup: &str) -> Option<Option<&str>> {
    match backup.strip_prefix(FILE) {
        Some(t) => Some(Some(t)),
        None if backup == NO_FILE => Some(None),
        None => None,
    }
}

pub fn restore(backup: &str, current: Option<&str>) -> Option<String> {
    let Some(original) = original(backup) else { return current.map(str::to_owned) };
    let current = current.unwrap_or_default();
    if current == ours(original) {
        return original.map(str::to_owned);
    }
    Some(FOLDER_TYPES.iter().fold(current.to_owned(), |text, mime| {
        let previous = original.and_then(|t| default_app(t, mime));
        set_default_app(&text, mime, previous.as_deref())
    }))
}

#[cfg(test)]
#[path = "../test/mimeapps.rs"]
mod tests;
