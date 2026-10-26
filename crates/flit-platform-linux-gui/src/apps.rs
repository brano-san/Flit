use std::collections::HashMap;
use std::path::{Path, PathBuf};

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

fn localized<'a>(fields: &'a HashMap<String, String>, key: &str, lang: &str) -> Option<&'a String> {
    let short = lang.split(['_', '.', '@']).next().unwrap_or("");
    let country = lang.split(['.', '@']).next().unwrap_or("");
    [format!("{key}[{country}]"), format!("{key}[{short}]"), key.to_owned()].iter().find_map(|k| fields.get(k))
}

pub fn parse_desktop(id: &str, path: &Path, text: &str, lang: &str) -> Option<Entry> {
    let mut fields = HashMap::new();
    let mut inside = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            inside = line == "[Desktop Entry]";
        } else if inside {
            if let Some((k, v)) = line.split_once('=') {
                fields.insert(k.trim().to_owned(), v.trim().to_owned());
            }
        }
    }
    if fields.get("Type").map(String::as_str) != Some("Application") {
        return None;
    }
    let yes = |k: &str| fields.get(k).is_some_and(|v| v == "true");
    Some(Entry {
        id: id.to_owned(),
        path: path.to_path_buf(),
        name: localized(&fields, "Name", lang)?.clone(),
        exec: fields.get("Exec")?.clone(),
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

fn collect(dir: &Path, prefix: &str, lang: &str, seen: &mut Vec<String>, out: &mut Vec<Entry>) {
    let Ok(list) = std::fs::read_dir(dir) else { return };
    for e in list.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let path = e.path();
        if path.is_dir() {
            collect(&path, &format!("{prefix}{name}-"), lang, seen, out);
        } else if let Some(stem) = name.strip_suffix(".desktop") {
            let id = format!("{prefix}{stem}.desktop");
            if seen.contains(&id) {
                continue;
            }
            seen.push(id.clone());
            if let Some(entry) = std::fs::read_to_string(&path).ok().and_then(|t| parse_desktop(&id, &path, &t, lang)) {
                out.push(entry);
            }
        }
    }
}

pub fn entries() -> Vec<Entry> {
    let lang = std::env::var("LC_MESSAGES").or_else(|_| std::env::var("LANG")).unwrap_or_default();
    let (mut seen, mut out) = (Vec::new(), Vec::new());
    for d in data_dirs() {
        collect(&d.join("applications"), "", &lang, &mut seen, &mut out);
    }
    out
}

pub fn associations() -> Assoc {
    let mut files = Vec::new();
    let desktops: Vec<String> = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default().split(':').filter(|d| !d.is_empty()).map(str::to_lowercase).collect();
    let system = std::env::var("XDG_CONFIG_DIRS").unwrap_or_else(|_| "/etc/xdg".into());
    let configs = crate::xdg::config_home().into_iter().chain(system.split(':').map(PathBuf::from));
    for c in configs {
        files.extend(desktops.iter().map(|d| c.join(format!("{d}-mimeapps.list"))));
        files.push(c.join("mimeapps.list"));
    }
    for d in data_dirs() {
        files.push(d.join("applications/mimeapps.list"));
        files.push(d.join("applications/defaults.list"));
    }
    let mut assoc = Assoc::default();
    for f in files {
        if let Ok(text) = std::fs::read_to_string(f) {
            parse_mimeapps(&text, &mut assoc);
        }
    }
    assoc
}

pub fn open_with(files: &[flit_core::FsPath], entries: &[Entry], assoc: &Assoc) -> (Vec<flit_core::ui::MenuItem>, Vec<flit_core::provider::Launch>) {
    let paths: Vec<PathBuf> = files.iter().filter_map(|f| flit_platform_linux::host(f).map(Path::to_path_buf)).collect();
    if paths.is_empty() || paths.len() != files.len() || paths.iter().any(|p| p.is_dir()) {
        return (Vec::new(), Vec::new());
    }
    let mimes: Vec<String> = paths
        .iter()
        .map(|p| p.extension().and_then(|e| crate::mime::mime_of(e.as_encoded_bytes())).unwrap_or_else(|| "application/octet-stream".into()))
        .collect();
    let apps: Vec<&Entry> = handlers(&mimes[0], entries, assoc).into_iter().filter(|e| mimes.iter().all(|m| handlers(m, entries, assoc).iter().any(|h| h.id == e.id))).collect();
    let items = apps.iter().map(|e| flit_core::ui::MenuItem::Command { label: e.name.clone(), keys: String::new(), checked: false, stay: false }).collect();
    let launches = apps.iter().filter_map(|e| Some(flit_core::provider::Launch::With { program: crate::xdg::fs_path(&e.path)?, args: files.to_vec() })).collect();
    (items, launches)
}

pub fn menu_with_apps(mut items: Vec<flit_core::ui::MenuItem>, files: &[flit_core::FsPath]) -> (Vec<flit_core::ui::MenuItem>, Vec<flit_core::provider::Launch>) {
    if files.is_empty() {
        return (items, Vec::new());
    }
    let (apps, launches) = open_with(files, &entries(), &associations());
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
