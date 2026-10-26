use flit_core::ui::Bitmap;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

#[derive(Clone, Debug, PartialEq)]
pub struct Dir {
    pub path: String,
    pub size: u32,
    pub scale: u32,
    pub min: u32,
    pub max: u32,
    pub threshold: u32,
    pub kind: Kind,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Fixed,
    Scalable,
    Threshold,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Theme {
    pub roots: Vec<PathBuf>,
    pub inherits: Vec<String>,
    pub dirs: Vec<Dir>,
}

impl Dir {
    fn matches(&self, size: u32) -> bool {
        match self.kind {
            Kind::Fixed => self.size == size,
            Kind::Scalable => (self.min..=self.max).contains(&size),
            Kind::Threshold => size + self.threshold >= self.size && size <= self.size + self.threshold,
        }
    }

    fn distance(&self, size: u32) -> u32 {
        match self.kind {
            Kind::Fixed => self.size.abs_diff(size),
            Kind::Scalable if size < self.min => self.min - size,
            Kind::Scalable if size > self.max => size - self.max,
            Kind::Scalable => 0,
            Kind::Threshold if size + self.threshold < self.size => self.size - self.threshold - size,
            Kind::Threshold if size > self.size + self.threshold => size - self.size - self.threshold,
            Kind::Threshold => 0,
        }
    }
}

pub fn parse_index(text: &str) -> Theme {
    let mut sections: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut current = String::new();
    for line in text.lines().map(str::trim) {
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            current = name.to_owned();
        } else if let Some((k, v)) = line.split_once('=') {
            sections.entry(current.clone()).or_default().insert(k.trim().to_owned(), v.trim().to_owned());
        }
    }
    let head = sections.get("Icon Theme").cloned().unwrap_or_default();
    let list = |key: &str| head.get(key).map(|v| v.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned).collect::<Vec<_>>()).unwrap_or_default();
    let mut names = list("Directories");
    names.extend(list("ScaledDirectories"));
    let dirs = names
        .into_iter()
        .filter_map(|path| {
            let s = sections.get(&path)?;
            let num = |k: &str| s.get(k).and_then(|v| v.parse::<u32>().ok());
            let size = num("Size")?;
            let kind = match s.get("Type").map(String::as_str) {
                Some("Fixed") => Kind::Fixed,
                Some("Scalable") => Kind::Scalable,
                _ => Kind::Threshold,
            };
            Some(Dir { size, scale: num("Scale").unwrap_or(1), min: num("MinSize").unwrap_or(size), max: num("MaxSize").unwrap_or(size), threshold: num("Threshold").unwrap_or(2), kind, path })
        })
        .collect();
    Theme { roots: Vec::new(), inherits: list("Inherits"), dirs }
}

pub fn base_dirs() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = crate::xdg::data_home().into_iter().map(|d| d.join("icons")).collect();
    out.extend(crate::xdg::home().map(|h| h.join(".icons")));
    let system = std::env::var("XDG_DATA_DIRS").ok().filter(|v| !v.is_empty()).unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    out.extend(system.split(':').filter(|d| d.starts_with('/')).map(|d| Path::new(d).join("icons")));
    out
}

pub fn load_theme(bases: &[PathBuf], name: &str) -> Option<Theme> {
    let roots: Vec<PathBuf> = bases.iter().map(|b| b.join(name)).filter(|r| r.is_dir()).collect();
    let index = roots.iter().find_map(|r| std::fs::read_to_string(r.join("index.theme")).ok())?;
    Some(Theme { roots, ..parse_index(&index) })
}

pub fn chain(bases: &[PathBuf], first: &str) -> Vec<Theme> {
    let mut names = vec![first.to_owned()];
    let mut themes = Vec::new();
    let mut k = 0;
    while k < names.len() && k < 16 {
        if let Some(t) = load_theme(bases, &names[k]) {
            for i in &t.inherits {
                if !names.contains(i) {
                    names.push(i.clone());
                }
            }
            themes.push(t);
        }
        k += 1;
    }
    if !names.iter().any(|n| n == "hicolor") {
        themes.extend(load_theme(bases, "hicolor"));
    }
    themes
}

pub fn find(themes: &[Theme], names: &[String], size: u32) -> Option<PathBuf> {
    let exts: &[&str] = if crate::svg::available() { &["png", "svg"] } else { &["png"] };
    let file = |t: &Theme, d: &Dir, name: &str| t.roots.iter().flat_map(|r| exts.iter().map(move |e| r.join(&d.path).join(format!("{name}.{e}")))).find(|p| p.is_file());
    for name in names {
        for t in themes {
            let dirs = t.dirs.iter().filter(|d| d.scale == 1);
            if let Some(p) = dirs.clone().filter(|d| d.matches(size)).find_map(|d| file(t, d, name)) {
                return Some(p);
            }
            let mut near: Vec<&Dir> = dirs.collect();
            near.sort_by_key(|d| (d.distance(size), d.kind != Kind::Scalable, u32::MAX - d.size));
            if let Some(p) = near.into_iter().find_map(|d| file(t, d, name)) {
                return Some(p);
            }
        }
    }
    names.iter().find_map(|n| exts.iter().map(|e| PathBuf::from(format!("/usr/share/pixmaps/{n}.{e}"))).find(|p| p.is_file()))
}

pub fn render(path: &Path, size: u32) -> Option<Bitmap> {
    if path.extension().is_some_and(|e| e == "svg") {
        return crate::svg::render(&std::fs::read(path).ok()?, size);
    }
    crate::thumb::decode_png(path).map(|b| crate::thumb::fit(b, size))
}

pub fn setting(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|l| l.split_once('=').filter(|(k, _)| k.trim() == key).map(|(_, v)| v.trim().trim_matches(['"', '\'']).to_owned())).filter(|v| !v.is_empty())
}

fn theme_name() -> String {
    let config = |file: &str, key: &str| crate::xdg::config_home().and_then(|c| std::fs::read_to_string(c.join(file)).ok()).and_then(|t| setting(&t, key));
    let kde = || std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|d| d.contains("KDE")).then(|| config("kdeglobals", "Theme")).flatten();
    let gnome = || crate::shell::gsettings(&["org.gnome.desktop.interface", "icon-theme"]).map(|v| v.trim().trim_matches('\'').to_owned()).filter(|v| !v.is_empty());
    kde().or_else(|| config("gtk-3.0/settings.ini", "gtk-icon-theme-name")).or_else(gnome).unwrap_or_else(|| "Adwaita".into())
}

fn themes() -> &'static [Theme] {
    static THEMES: OnceLock<Vec<Theme>> = OnceLock::new();
    THEMES.get_or_init(|| chain(&base_dirs(), &theme_name()))
}

type Rendered = HashMap<(Vec<String>, u32), Option<Bitmap>>;

pub fn icon(names: &[String], size: u32) -> Option<Bitmap> {
    static CACHE: Mutex<Option<Rendered>> = Mutex::new(None);
    let key = (names.to_vec(), size);
    if let Some(hit) = CACHE.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).get(&key) {
        return hit.clone();
    }
    let made = find(themes(), names, size).and_then(|p| render(&p, size));
    CACHE.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(key, made.clone());
    made
}

#[cfg(test)]
#[path = "../test/icons.rs"]
mod tests;
