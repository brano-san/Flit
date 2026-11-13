use flit_core::i18n::S;
use flit_core::provider::{AppChoice, Launch};
use flit_core::ui::{Mark, MenuImage, MenuItem};
use flit_core::FsPath;
use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const SETTLE: Duration = Duration::from_secs(2);
const ICON_PX: u32 = 32;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Entry {
    pub id: String,
    pub path: PathBuf,
    pub name: String,
    pub exec: String,
    pub mimes: Vec<String>,
    pub terminal: bool,
    pub hidden: bool,
    pub no_display: bool,
    pub try_exec: String,
    pub only_show_in: Vec<String>,
    pub not_show_in: Vec<String>,
    pub dir: String,
    pub icon: String,
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
    let text = |k: &str| fields.get(k).map(|v| v.to_string()).unwrap_or_default();
    let list = |k: &str| -> Vec<String> { fields.get(k).map(|m| m.split(';').filter(|s| !s.is_empty()).map(str::to_owned).collect()).unwrap_or_default() };
    Some(Entry {
        id: id.to_owned(),
        path: path.to_path_buf(),
        name: localized(&fields, "Name", lang)?.to_owned(),
        exec: fields.get("Exec")?.to_string(),
        mimes: list("MimeType"),
        terminal: yes("Terminal"),
        hidden: yes("Hidden"),
        no_display: yes("NoDisplay"),
        try_exec: text("TryExec"),
        only_show_in: list("OnlyShowIn"),
        not_show_in: list("NotShowIn"),
        dir: text("Path"),
        icon: text("Icon"),
    })
}

pub fn shown_in(e: &Entry, desktops: &[String]) -> bool {
    for d in desktops {
        if e.only_show_in.iter().any(|o| o.eq_ignore_ascii_case(d)) {
            return true;
        }
        if e.not_show_in.iter().any(|o| o.eq_ignore_ascii_case(d)) {
            return false;
        }
    }
    e.only_show_in.is_empty()
}

pub fn runnable(e: &Entry) -> bool {
    e.try_exec.is_empty() || crate::shell::on_path(OsStr::new(&e.try_exec)).is_some()
}

pub fn current_desktops() -> Vec<String> {
    std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default().split(':').filter(|d| !d.is_empty()).map(str::to_owned).collect()
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
    let declared = entries.iter().filter(|e| !e.hidden && !e.no_display && e.mimes.iter().any(|m| m == mime));
    for e in named.chain(declared) {
        if !removed.contains(&e.id) && !out.iter().any(|o| o.id == e.id) {
            out.push(e);
        }
    }
    out
}

pub fn handlers_by_lineage<'a>(mime: &str, entries: &'a [Entry], assoc: &Assoc, parents: impl Fn(&str) -> Vec<String>) -> Vec<&'a Entry> {
    let mut out: Vec<&Entry> = Vec::new();
    for m in crate::mime::lineage(mime, parents) {
        for e in handlers(&m, entries, assoc) {
            if !out.iter().any(|o| o.id == e.id) {
                out.push(e);
            }
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

pub fn commands(exec: &str, files: &[PathBuf], name: &str, desktop: &Path) -> Vec<Vec<String>> {
    let single = crate::shell::split_args(exec).iter().any(|a| a == "%f" || a == "%u");
    if single && files.len() > 1 {
        return files.iter().map(|f| command(exec, std::slice::from_ref(f), name, desktop)).collect();
    }
    vec![command(exec, files, name, desktop)]
}

pub(crate) fn data_dirs() -> Vec<PathBuf> {
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
        Self::load_for(data, config, &current_desktops())
    }

    pub fn load_for(data: &[PathBuf], config: &[PathBuf], desktops: &[String]) -> Self {
        let lang = std::env::var("LC_MESSAGES").or_else(|_| std::env::var("LANG")).unwrap_or_default();
        let (mut seen, mut entries, mut stamps) = (HashSet::new(), Vec::new(), Vec::new());
        for d in data {
            collect(&d.join("applications"), "", &lang, &mut seen, &mut entries, &mut stamps);
        }
        for e in &mut entries {
            e.hidden |= !runnable(e);
            e.no_display |= !shown_in(e, desktops);
        }
        let mut files = Vec::new();
        for c in config {
            files.extend(desktops.iter().map(|d| c.join(format!("{}-mimeapps.list", d.to_lowercase()))));
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

fn menu_image(icon: &str) -> Option<MenuImage> {
    if icon.is_empty() {
        return None;
    }
    let bitmap = match icon.starts_with('/') {
        true => crate::icons::render(Path::new(icon), ICON_PX),
        false => crate::icons::icon(&[icon.trim_end_matches(".png").trim_end_matches(".svg").to_owned()], ICON_PX),
    };
    bitmap.map(MenuImage::pixels)
}

pub fn choices(entries: &[Entry]) -> Vec<AppChoice> {
    let mut out: Vec<AppChoice> = entries.iter().filter(|e| !e.hidden && !e.no_display).filter_map(|e| Some(AppChoice { name: e.name.clone(), program: crate::xdg::fs_path(&e.path)? })).collect();
    out.sort_by_cached_key(|a| a.name.to_lowercase());
    out
}

pub fn open_with(files: &[FsPath], entries: &[Entry], assoc: &Assoc) -> OpenWith {
    let paths: Vec<PathBuf> = files.iter().filter_map(|f| flit_platform_linux::host(f).map(Path::to_path_buf)).collect();
    if paths.is_empty() || paths.len() != files.len() || paths.iter().any(|p| p.is_dir()) {
        return (Vec::new(), Vec::new());
    }
    let mut mimes: Vec<String> = Vec::new();
    for p in &paths {
        let mime = crate::mime::mime_of_file(p);
        if !mimes.contains(&mime) {
            mimes.push(mime);
        }
    }
    let sets: Vec<Vec<&Entry>> = mimes.iter().map(|m| handlers_by_lineage(m, entries, assoc, crate::mime::parents)).collect();
    let apps: Vec<(&Entry, FsPath)> = sets[0].iter().copied().filter(|e| sets[1..].iter().all(|s| s.iter().any(|h| h.id == e.id))).filter_map(|e| Some((e, crate::xdg::fs_path(&e.path)?))).collect();
    let marked = mimes.len() == 1;
    let mut items: Vec<MenuItem> = apps
        .iter()
        .enumerate()
        .map(|(k, (e, _))| {
            let keys = if k == 0 && marked { S::OpenWithDefault.t().to_owned() } else { String::new() };
            MenuItem::Command { label: e.name.clone(), keys, checked: false, stay: false, mark: Mark::None, icon: "", image: menu_image(&e.icon), disabled: false }
        })
        .collect();
    let mut launches: Vec<Launch> = apps.iter().map(|(_, program)| Launch::With { program: program.clone(), args: files.to_vec() }).collect();
    if !items.is_empty() {
        items.push(MenuItem::Separator);
    }
    items.push(MenuItem::command(S::OpenWithOther.t(), String::new()));
    launches.push(Launch::ChooseApp { files: files.to_vec(), mime: mimes[0].clone(), apps: choices(entries) });
    (items, launches)
}

pub type OpenWith = (Vec<MenuItem>, Vec<Launch>);

pub fn menu_with_apps(items: Vec<MenuItem>, files: &[FsPath]) -> OpenWith {
    if files.is_empty() {
        return (items, Vec::new());
    }
    let c = catalog();
    with_apps(items, open_with(files, &c.entries, &c.assoc))
}

pub fn with_apps(mut items: Vec<MenuItem>, (apps, launches): OpenWith) -> OpenWith {
    let commands = apps.iter().filter(|i| matches!(i, MenuItem::Command { .. })).count();
    if !apps.is_empty() && commands == launches.len() {
        if !items.is_empty() {
            items.push(MenuItem::Separator);
        }
        items.push(MenuItem::Submenu { label: S::MenuOpenWith.t().to_owned(), items: apps });
        return (items, launches);
    }
    (items, Vec::new())
}

fn spawn(argv: &[String], dir: Option<&Path>, token: Option<&str>) -> io::Result<()> {
    let mut cmd = std::process::Command::new(&argv[0]);
    cmd.args(&argv[1..]);
    if let Some(d) = dir {
        cmd.current_dir(d);
    }
    crate::activation::pass(&mut cmd, token);
    crate::shell::detach(cmd)
}

pub fn launch(desktop: &Path, files: &[PathBuf], token: Option<&str>) -> io::Result<()> {
    let text = std::fs::read_to_string(desktop)?;
    let id = desktop.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let entry = parse_desktop(&id, desktop, &text, "").ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "not an application entry"))?;
    let runs = commands(&entry.exec, files, &entry.name, desktop);
    if runs.iter().any(Vec::is_empty) {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "empty Exec"));
    }
    let dir = Some(Path::new(&entry.dir)).filter(|d| !entry.dir.is_empty() && d.is_dir());
    for argv in runs {
        if entry.terminal {
            crate::shell::run_in_terminal_argv(dir, argv.into_iter().map(Into::into).collect()).map_err(|e| io::Error::other(e.message))?;
        } else {
            spawn(&argv, dir, token)?;
        }
    }
    Ok(())
}

pub fn desktop_id(desktop: &Path, data: &[PathBuf]) -> String {
    let relative = data.iter().find_map(|d| desktop.strip_prefix(d.join("applications")).ok());
    match relative {
        Some(rel) => rel.to_string_lossy().replace('/', "-"),
        None => desktop.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
    }
}

pub fn use_app(config: &Path, mime: &str, id: &str) -> io::Result<()> {
    let list = config.join("mimeapps.list");
    let current = match std::fs::read_to_string(&list) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };
    std::fs::create_dir_all(config)?;
    std::fs::write(&list, flit_core::mimeapps::use_app(&current, mime, id))
}

fn config_home() -> io::Result<PathBuf> {
    crate::xdg::config_home().ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no HOME or XDG_CONFIG_HOME"))
}

pub fn set_default(desktop: &Path, mime: &str) -> io::Result<()> {
    use_app(&config_home()?, mime, &desktop_id(desktop, &data_dirs()))
}

pub fn with_files(command: &str) -> String {
    let command = command.replace(['\n', '\r'], " ");
    let codes = crate::shell::split_args(&command).iter().any(|a| matches!(a.as_str(), "%f" | "%F" | "%u" | "%U"));
    if codes { command.trim().to_owned() } else { format!("{} %F", command.trim()) }
}

fn program_name(exec: &str) -> String {
    let program = crate::shell::split_args(exec).into_iter().next().unwrap_or_default();
    Path::new(&program).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or(program)
}

pub fn user_app_text(exec: &str, mime: &str) -> String {
    format!("[Desktop Entry]\nType=Application\nName={}\nExec={exec}\nMimeType={mime};\nNoDisplay=true\nTerminal=false\n", program_name(exec))
}

pub fn write_user_app(dir: &Path, exec: &str, mime: &str) -> io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let base: String = program_name(exec).chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')).collect();
    let base = if base.is_empty() { "app".to_owned() } else { base };
    let seed = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or_default() ^ u128::from(std::process::id());
    for attempt in 0..64u128 {
        let mut n = seed.wrapping_mul(6364136223846793005).wrapping_add(attempt.wrapping_mul(1442695040888963407));
        let tag: String = (0..6)
            .map(|_| {
                let c = b"abcdefghijklmnopqrstuvwxyz0123456789"[(n % 36) as usize];
                n /= 36;
                c as char
            })
            .collect();
        let path = dir.join(format!("userapp-{base}-{tag}.desktop"));
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut f) => {
                f.write_all(user_app_text(exec, mime).as_bytes())?;
                return Ok(path);
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(io::ErrorKind::AlreadyExists, "no free name for a new application entry"))
}

pub fn run_custom_in(apps: &Path, config: &Path, command: &str, files: &[PathBuf], mime: &str, remember: bool, token: Option<&str>) -> io::Result<()> {
    let exec = with_files(command);
    if !remember {
        let runs = commands(&exec, files, "", Path::new(""));
        if runs.iter().any(Vec::is_empty) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "the command is empty"));
        }
        return runs.iter().try_for_each(|argv| spawn(argv, None, token));
    }
    let desktop = write_user_app(apps, &exec, mime)?;
    let id = desktop.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    use_app(config, mime, &id)?;
    launch(&desktop, files, token)
}

pub fn run_custom(command: &str, files: &[PathBuf], mime: &str, remember: bool, token: Option<&str>) -> io::Result<()> {
    let apps = crate::xdg::data_home().ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no HOME or XDG_DATA_HOME"))?.join("applications");
    run_custom_in(&apps, &config_home()?, command, files, mime, remember, token)
}

#[cfg(test)]
#[path = "../test/apps.rs"]
mod tests;
