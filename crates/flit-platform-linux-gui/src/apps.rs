use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

const SETTLE: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Entry {
    pub id: String,
    pub path: PathBuf,
    pub name: String,
    pub exec: String,
    pub mimes: Vec<String>,
    pub terminal: bool,
    pub hidden: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Assoc {
    pub defaults: HashMap<String, Vec<String>>,
    pub added: HashMap<String, Vec<String>>,
    pub removed: HashMap<String, Vec<String>>,
}

fn localized<'a>(fields: &HashMap<&str, &'a str>, key: &str, lang: &str) -> Option<&'a str> {
    let short = lang.split(['_', '.', '@']).next().unwrap_or("");
    let country = lang.split(['.', '@']).next().unwrap_or("");
    [format!("{key}[{country}]"), format!("{key}[{short}]"), key.to_owned()].iter().find_map(|k| fields.get(k.as_str()).copied())
}

pub fn parse_desktop(id: &str, path: &Path, text: &str, lang: &str) -> Option<Entry> {
    let mut fields = HashMap::new();
    let mut inside = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            inside = line == "[Desktop Entry]";
        } else if inside {
            if let Some((k, v)) = line.split_once('=').map(|(k, v)| (k.trim(), v.trim())) {
                if !k.ends_with(']') || k.starts_with("Name[") {
                    fields.insert(k, v);
                }
            }
        }
    }
    if fields.get("Type") != Some(&"Application") {
        return None;
    }
    let yes = |k: &str| fields.get(k) == Some(&"true");
    Some(Entry {
        id: id.to_owned(),
        path: path.to_path_buf(),
        name: localized(&fields, "Name", lang)?.to_owned(),
        exec: fields.get("Exec")?.to_string(),
        mimes: fields.get("MimeType").map(|m| m.split(';').filter(|s| !s.is_empty()).map(str::to_owned).collect()).unwrap_or_default(),
        terminal: yes("Terminal"),
        hidden: yes("Hidden"),
    })
}

pub fn parse_mimeapps(text: &str, into: &mut Assoc) {
    let mut section = "";
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            section = line;
            continue;
        }
        let Some((mime, apps)) = line.split_once('=') else { continue };
        let target = match section {
            "[Default Applications]" => &mut into.defaults,
            "[Added Associations]" => &mut into.added,
            "[Removed Associations]" => &mut into.removed,
            _ => continue,
        };
        let list = target.entry(mime.trim().to_owned()).or_default();
        for app in apps.split(';').map(str::trim).filter(|a| !a.is_empty()) {
            if !list.iter().any(|a| a == app) {
                list.push(app.to_owned());
            }
        }
    }
}

pub fn handlers<'a>(mime: &str, entries: &'a [Entry], assoc: &Assoc) -> Vec<&'a Entry> {
    let removed = assoc.removed.get(mime).cloned().unwrap_or_default();
    let by_id = |id: &String| entries.iter().find(|e| &e.id == id && !e.hidden);
    let mut out: Vec<&Entry> = Vec::new();
    let named = assoc.defaults.get(mime).into_iter().chain(assoc.added.get(mime)).flatten().filter_map(by_id);
    let declared = entries.iter().filter(|e| !e.hidden && e.mimes.iter().any(|m| m == mime));
    for e in named.chain(declared) {
        if !removed.contains(&e.id) && !out.iter().any(|o| o.id == e.id) {
            out.push(e);
        }
    }
    out
}

pub fn command(exec: &str, files: &[PathBuf], name: &str, desktop: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let uri = |p: &PathBuf| format!("file://{}", flit_platform_linux::percent_encode(p.as_os_str().as_encoded_bytes()));
    let mut used = false;
    for arg in crate::shell::split_args(exec) {
        match arg.as_str() {
            "%f" => {
                out.extend(files.first().map(|f| f.to_string_lossy().into_owned()));
                used = true;
            }
            "%F" => {
                out.extend(files.iter().map(|f| f.to_string_lossy().into_owned()));
                used = true;
            }
            "%u" => {
                out.extend(files.first().map(uri));
                used = true;
            }
            "%U" => {
                out.extend(files.iter().map(uri));
                used = true;
            }
            "%c" => out.push(name.to_owned()),
            "%k" => out.push(desktop.to_string_lossy().into_owned()),
            "%i" | "%d" | "%D" | "%n" | "%N" | "%v" | "%m" => {}
            _ => out.push(arg.replace("%%", "%")),
        }
    }
    if !used {
        out.extend(files.iter().map(|f| f.to_string_lossy().into_owned()));
    }
    out
}

fn data_dirs() -> Vec<PathBuf> {
    let system = std::env::var("XDG_DATA_DIRS").ok().filter(|v| !v.is_empty()).unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    crate::xdg::data_home().into_iter().chain(system.split(':').filter(|d| d.starts_with('/')).map(PathBuf::from)).collect()
}

fn config_dirs() -> Vec<PathBuf> {
    let system = std::env::var("XDG_CONFIG_DIRS").unwrap_or_else(|_| "/etc/xdg".into());
    crate::xdg::config_home().into_iter().chain(system.split(':').map(PathBuf::from)).collect()
}

type Stamps = Vec<(PathBuf, Option<SystemTime>)>;

fn stamp(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

fn collect(dir: &Path, prefix: &str, lang: &str, seen: &mut HashSet<String>, out: &mut Vec<Entry>, stamps: &mut Stamps) {
    stamps.push((dir.to_path_buf(), stamp(dir)));
    let Ok(list) = std::fs::read_dir(dir) else { return };
    for e in list.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let path = e.path();
        if e.file_type().is_ok_and(|t| t.is_dir() || t.is_symlink() && path.is_dir()) {
            collect(&path, &format!("{prefix}{name}-"), lang, seen, out, stamps);
        } else if let Some(stem) = name.strip_suffix(".desktop") {
            let id = format!("{prefix}{stem}.desktop");
            if !seen.insert(id.clone()) {
                continue;
            }
            if let Some(entry) = std::fs::read_to_string(&path).ok().and_then(|t| parse_desktop(&id, &path, &t, lang)) {
                out.push(entry);
            }
        }
    }
}

pub struct Catalog {
    pub entries: Vec<Entry>,
    pub assoc: Assoc,
    stamps: Stamps,
    racy: bool,
}

impl Catalog {
    pub fn load(data: &[PathBuf], config: &[PathBuf]) -> Self {
        let lang = std::env::var("LC_MESSAGES").or_else(|_| std::env::var("LANG")).unwrap_or_default();
        let (mut seen, mut entries, mut stamps) = (HashSet::new(), Vec::new(), Vec::new());
        for d in data {
            collect(&d.join("applications"), "", &lang, &mut seen, &mut entries, &mut stamps);
        }
        let desktops: Vec<String> = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default().split(':').filter(|d| !d.is_empty()).map(str::to_lowercase).collect();
        let mut files = Vec::new();
        for c in config {
            files.extend(desktops.iter().map(|d| c.join(format!("{d}-mimeapps.list"))));
            files.push(c.join("mimeapps.list"));
        }
        for d in data {
            files.push(d.join("applications/mimeapps.list"));
            files.push(d.join("applications/defaults.list"));
        }
        let mut assoc = Assoc::default();
        for f in files {
            if let Ok(text) = std::fs::read_to_string(&f) {
                parse_mimeapps(&text, &mut assoc);
            }
            stamps.push((f.clone(), stamp(&f)));
        }
        let settled = SystemTime::now() - SETTLE;
        let racy = stamps.iter().any(|(_, t)| t.is_some_and(|t| t > settled));
        Self { entries, assoc, stamps, racy }
    }

    pub fn fresh(&self) -> bool {
        !self.racy && self.stamps.iter().all(|(p, t)| stamp(p) == *t)
    }
}

pub fn catalog() -> Arc<Catalog> {
    static CATALOG: Mutex<Option<Arc<Catalog>>> = Mutex::new(None);
    let mut slot = CATALOG.lock().unwrap_or_else(|e| e.into_inner());
    match slot.as_ref().filter(|c| c.fresh()) {
        Some(c) => c.clone(),
        None => {
            let started = Instant::now();
            let c = Arc::new(Catalog::load(&data_dirs(), &config_dirs()));
            tracing::debug!(elapsed = ?started.elapsed(), apps = c.entries.len(), "application catalog loaded");
            *slot = Some(c.clone());
            c
        }
    }
}

pub fn open_with(files: &[flit_core::FsPath], entries: &[Entry], assoc: &Assoc) -> (Vec<flit_core::ui::MenuItem>, Vec<flit_core::provider::Launch>) {
    let paths: Vec<PathBuf> = files.iter().filter_map(|f| flit_platform_linux::host(f).map(Path::to_path_buf)).collect();
    if paths.is_empty() || paths.len() != files.len() || paths.iter().any(|p| p.is_dir()) {
        return (Vec::new(), Vec::new());
    }
    let mut mimes: Vec<String> = Vec::new();
    for p in &paths {
        let mime = p.extension().and_then(|e| crate::mime::mime_of(e.as_encoded_bytes())).unwrap_or_else(|| "application/octet-stream".into());
        if !mimes.contains(&mime) {
            mimes.push(mime);
        }
    }
    let sets: Vec<Vec<&Entry>> = mimes.iter().map(|m| handlers(m, entries, assoc)).collect();
    let apps: Vec<&Entry> = sets[0].iter().copied().filter(|e| sets[1..].iter().all(|s| s.iter().any(|h| h.id == e.id))).collect();
    let items = apps.iter().map(|e| flit_core::ui::MenuItem::command(e.name.clone(), String::new())).collect();
    let launches = apps.iter().filter_map(|e| Some(flit_core::provider::Launch::With { program: crate::xdg::fs_path(&e.path)?, args: files.to_vec() })).collect();
    (items, launches)
}

pub type OpenWith = (Vec<flit_core::ui::MenuItem>, Vec<flit_core::provider::Launch>);

pub fn menu_with_apps(items: Vec<flit_core::ui::MenuItem>, files: &[flit_core::FsPath]) -> OpenWith {
    if files.is_empty() {
        return (items, Vec::new());
    }
    let c = catalog();
    with_apps(items, open_with(files, &c.entries, &c.assoc))
}

pub fn with_apps(mut items: Vec<flit_core::ui::MenuItem>, (apps, launches): OpenWith) -> OpenWith {
    if !apps.is_empty() && apps.len() == launches.len() {
        if !items.is_empty() {
            items.push(flit_core::ui::MenuItem::Separator);
        }
        items.push(flit_core::ui::MenuItem::Submenu { label: flit_core::i18n::S::MenuOpenWith.t().to_owned(), items: apps });
        return (items, launches);
    }
    (items, Vec::new())
}

pub fn launch(desktop: &Path, files: &[PathBuf], token: Option<&str>) -> std::io::Result<()> {
    let text = std::fs::read_to_string(desktop)?;
    let id = desktop.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let entry = parse_desktop(&id, desktop, &text, "").ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "not an application entry"))?;
    let mut argv = command(&entry.exec, files, &entry.name, desktop);
    if argv.is_empty() {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "empty Exec"));
    }
    if entry.terminal {
        argv.insert(0, "x-terminal-emulator".into());
        argv.insert(1, "-e".into());
    }
    let mut cmd = std::process::Command::new(&argv[0]);
    cmd.args(&argv[1..]);
    if let Some(t) = token {
        cmd.env("XDG_ACTIVATION_TOKEN", t).env("DESKTOP_STARTUP_ID", t);
    }
    crate::shell::detach(cmd)
}

#[cfg(test)]
#[path = "../test/apps.rs"]
mod tests;
