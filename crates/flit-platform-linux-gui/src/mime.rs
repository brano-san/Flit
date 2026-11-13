use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

fn mime_dirs() -> Vec<PathBuf> {
    let system = std::env::var("XDG_DATA_DIRS").ok().filter(|v| !v.is_empty()).unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    crate::xdg::data_home().into_iter().chain(system.split(':').filter(|d| d.starts_with('/')).map(PathBuf::from)).map(|d| d.join("mime")).collect()
}

pub fn parse_globs(text: &str, into: &mut HashMap<String, (u32, String)>) {
    for line in text.lines().filter(|l| !l.starts_with('#')) {
        let mut f = line.split(':');
        let (Some(weight), Some(mime), Some(glob)) = (f.next(), f.next(), f.next()) else { continue };
        let Some(ext) = glob.strip_prefix("*.").filter(|e| !e.contains(['*', '?', '['])) else { continue };
        let weight = weight.parse().unwrap_or(50);
        let slot = into.entry(ext.to_lowercase()).or_insert((0, String::new()));
        if weight > slot.0 {
            *slot = (weight, mime.to_owned());
        }
    }
}

pub fn parse_comment(xml: &str) -> Option<String> {
    let mut rest = xml;
    while let Some(at) = rest.find("<comment") {
        rest = &rest[at + 8..];
        let close = rest.find('>')?;
        let (attrs, body) = (&rest[..close], &rest[close + 1..]);
        if !attrs.contains("xml:lang") {
            let text = &body[..body.find("</comment>")?];
            return Some(text.replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'"));
        }
    }
    None
}

fn globs() -> &'static HashMap<String, (u32, String)> {
    static GLOBS: OnceLock<HashMap<String, (u32, String)>> = OnceLock::new();
    GLOBS.get_or_init(|| {
        let mut map = HashMap::new();
        for dir in mime_dirs().iter().rev() {
            if let Ok(text) = std::fs::read_to_string(dir.join("globs2")) {
                parse_globs(&text, &mut map);
            }
        }
        map
    })
}

pub fn mime_of(ext: &[u8]) -> Option<String> {
    let ext = std::str::from_utf8(ext).ok()?.to_lowercase();
    globs().get(&ext).map(|g| g.1.clone())
}

pub fn parse_subclasses(text: &str, into: &mut HashMap<String, Vec<String>>) {
    for (child, parent) in text.lines().filter(|l| !l.starts_with('#')).filter_map(|l| l.split_once(' ')) {
        let list = into.entry(child.trim().to_owned()).or_default();
        if !list.iter().any(|p| p == parent.trim()) {
            list.push(parent.trim().to_owned());
        }
    }
}

pub fn parents(mime: &str) -> Vec<String> {
    static SUBCLASSES: OnceLock<HashMap<String, Vec<String>>> = OnceLock::new();
    let map = SUBCLASSES.get_or_init(|| {
        let mut map = HashMap::new();
        for dir in mime_dirs().iter().rev() {
            if let Ok(text) = std::fs::read_to_string(dir.join("subclasses")) {
                parse_subclasses(&text, &mut map);
            }
        }
        map
    });
    map.get(mime).cloned().unwrap_or_default()
}

pub const OCTET_STREAM: &str = "application/octet-stream";

pub fn lineage(mime: &str, parents: impl Fn(&str) -> Vec<String>) -> Vec<String> {
    let mut out = vec![mime.to_owned()];
    let mut k = 0;
    while k < out.len() {
        let mut next = parents(&out[k]);
        if out[k].starts_with("text/") && out[k] != "text/plain" {
            next.push("text/plain".into());
        }
        for p in next {
            if p != OCTET_STREAM && !out.contains(&p) {
                out.push(p);
            }
        }
        k += 1;
    }
    out
}

pub fn sniff(head: &[u8]) -> &'static str {
    let text = !head.contains(&0) && std::str::from_utf8(head).map_or_else(|e| e.error_len().is_none(), |_| true);
    if text { "text/plain" } else { OCTET_STREAM }
}

pub fn mime_of_file(path: &std::path::Path) -> String {
    if let Some(m) = path.extension().and_then(|e| mime_of(e.as_encoded_bytes())) {
        return m;
    }
    let mut head = [0u8; 4096];
    let read = std::fs::File::open(path).and_then(|mut f| std::io::Read::read(&mut f, &mut head));
    read.map_or(OCTET_STREAM, |n| sniff(&head[..n])).to_owned()
}

pub fn parse_generic_icons(text: &str) -> HashMap<String, String> {
    text.lines().filter_map(|l| l.split_once(':')).map(|(m, i)| (m.to_owned(), i.to_owned())).collect()
}

pub fn icon_names(mime: &str) -> Vec<String> {
    static GENERIC: OnceLock<HashMap<String, String>> = OnceLock::new();
    let generic = GENERIC.get_or_init(|| {
        let mut map = HashMap::new();
        for dir in mime_dirs().iter().rev() {
            if let Ok(text) = std::fs::read_to_string(dir.join("generic-icons")) {
                map.extend(parse_generic_icons(&text));
            }
        }
        map
    });
    let media = mime.split('/').next().unwrap_or("application");
    let mut names = vec![mime.replace('/', "-")];
    names.extend(generic.get(mime).cloned());
    names.push(format!("{media}-x-generic"));
    names
}

pub fn type_name(ext: &[u8]) -> Option<String> {
    static NAMES: Mutex<Option<HashMap<String, Option<String>>>> = Mutex::new(None);
    let ext = std::str::from_utf8(ext).ok()?.to_lowercase();
    let mime = &globs().get(&ext)?.1;
    let mut names = NAMES.lock().unwrap_or_else(|e| e.into_inner());
    names
        .get_or_insert_with(HashMap::new)
        .entry(mime.clone())
        .or_insert_with(|| mime_dirs().iter().find_map(|d| parse_comment(&std::fs::read_to_string(d.join(format!("{mime}.xml"))).ok()?)))
        .clone()
}

#[cfg(test)]
#[path = "../test/mime.rs"]
mod tests;
