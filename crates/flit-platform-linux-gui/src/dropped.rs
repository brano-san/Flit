use flit_core::FsPath;
use std::io::Write;
use winit::data_transfer::TypeHint;
use std::path::{Path, PathBuf};

const WEB_SCHEMES: [&str; 3] = ["http://", "https://", "ftp://"];
const MAX_STEM: usize = 80;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Content {
    Url(String),
    Text(String),
    Image(Vec<u8>),
}

pub fn web_urls(uri_list: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(uri_list)
        .lines()
        .map(str::trim)
        .filter(|l| !l.starts_with('#'))
        .filter(|l| WEB_SCHEMES.iter().any(|s| l.len() > s.len() && l[..s.len()].eq_ignore_ascii_case(s)))
        .map(str::to_owned)
        .collect()
}

fn clean(name: &str) -> String {
    let kept: String = name.chars().map(|c| if c == '/' || c.is_control() { ' ' } else { c }).collect();
    let trimmed = kept.split_whitespace().collect::<Vec<_>>().join(" ");
    let cut: String = trimmed.chars().take(MAX_STEM).collect();
    cut.trim_matches('.').trim().to_owned()
}

pub fn url_stem(url: &str) -> Option<String> {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let rest = rest.split(['?', '#']).next().unwrap_or(rest);
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let host = host.rsplit('@').next().unwrap_or(host);
    let last = path.trim_end_matches('/').rsplit('/').next().filter(|s| !s.is_empty());
    let stem = clean(&last.map_or_else(|| host.to_owned(), |l| format!("{host} {}", String::from_utf8_lossy(&flit_platform_linux::percent_decode(l.as_bytes())))));
    (!stem.is_empty()).then_some(stem)
}

pub fn link_file(url: &str, name: &str) -> String {
    let one_line = |s: &str| s.replace(['\n', '\r'], " ");
    format!("[Desktop Entry]\nType=Link\nName={}\nURL={}\nIcon=text-html\n", one_line(name), one_line(url))
}

pub fn free_name(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let base = clean(stem);
    let name = flit_core::ui::unique_name(&base, |n| dir.join(format!("{n}.{ext}")).exists());
    dir.join(format!("{name}.{ext}"))
}

pub fn save(dir: &Path, content: &Content, text_stem: &str, image_stem: &str) -> std::io::Result<PathBuf> {
    let (path, bytes) = match content {
        Content::Url(url) => {
            let stem = url_stem(url).unwrap_or_else(|| text_stem.to_owned());
            (free_name(dir, &stem, "desktop"), link_file(url, &stem).into_bytes())
        }
        Content::Text(text) => (free_name(dir, text_stem, "txt"), text.clone().into_bytes()),
        Content::Image(png) => (free_name(dir, image_stem, "png"), png.clone()),
    };
    std::fs::File::create_new(&path)?.write_all(&bytes)?;
    Ok(path)
}

pub const PNG: TypeHint = TypeHint::Image { extension_hint: Some("png") };

#[derive(Debug, PartialEq, Eq)]
pub enum Dragged {
    Files(Vec<FsPath>),
    Content(Content),
}

pub fn wanted(offered: &[TypeHint]) -> Vec<TypeHint> {
    if offered.is_empty() {
        return vec![TypeHint::UriList];
    }
    [TypeHint::UriList, PNG, TypeHint::Plaintext].into_iter().filter(|w| offered.contains(w)).collect()
}

pub fn interpret(kind: Option<TypeHint>, bytes: &[u8]) -> Option<Dragged> {
    match kind.unwrap_or(TypeHint::UriList) {
        TypeHint::UriList => {
            let paths = crate::shell::parse_file_list(bytes).0;
            if !paths.is_empty() {
                return Some(Dragged::Files(paths));
            }
            web_urls(bytes).into_iter().next().map(|u| Dragged::Content(Content::Url(u)))
        }
        TypeHint::Plaintext => {
            let text = String::from_utf8_lossy(bytes).into_owned();
            let single = web_urls(text.as_bytes()).into_iter().next().filter(|u| u == text.trim());
            match single {
                Some(url) => Some(Dragged::Content(Content::Url(url))),
                None => (!text.trim().is_empty()).then_some(Dragged::Content(Content::Text(text))),
            }
        }
        hint if hint == PNG => (!bytes.is_empty()).then(|| Dragged::Content(Content::Image(bytes.to_vec()))),
        _ => None,
    }
}

#[cfg(test)]
#[path = "../test/dropped.rs"]
mod tests;
