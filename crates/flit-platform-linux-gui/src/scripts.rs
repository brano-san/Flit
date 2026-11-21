use flit_core::i18n::S;
use flit_core::provider::{CommandRun, Launch};
use flit_core::ui::{Mark, MenuItem};
use flit_core::FsPath;
use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const ICON_PX: u32 = 32;
const DEPTH: usize = 4;

#[derive(Clone, Debug, PartialEq)]
pub enum Run {
    Exec { line: String, base: PathBuf },
    Shell(String),
    Script(PathBuf),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Count {
    Any,
    One,
    Many,
    NotEmpty,
    Nothing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Nautilus,
    Kde,
    Nemo,
    Thunar,
}

impl Source {
    const ORDER: [Source; 4] = [Source::Nautilus, Source::Kde, Source::Nemo, Source::Thunar];
}

#[derive(Clone, Debug, PartialEq)]
pub struct Action {
    pub source: Source,
    pub submenu: String,
    pub separator: Option<String>,
    pub name: String,
    pub icon: String,
    pub run: Run,
    pub mimes: Vec<String>,
    pub exts: Vec<String>,
    pub count: Count,
    pub files: bool,
    pub dirs: bool,
    pub removable: bool,
}

impl Action {
    fn new(source: Source, name: String, icon: String, run: Run) -> Self {
        Self { source, submenu: String::new(), separator: None, name, icon, run, mimes: Vec::new(), exts: Vec::new(), count: Count::NotEmpty, files: true, dirs: true, removable: false }
    }
}

pub struct Item {
    pub path: PathBuf,
    pub dir: bool,
    pub mime: String,
    pub removable: bool,
}

fn groups(text: &str) -> HashMap<String, HashMap<String, String>> {
    let mut out: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut current = String::new();
    for line in text.lines().map(str::trim) {
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            current = name.to_owned();
        } else if let Some((k, v)) = line.split_once('=') {
            out.entry(current.clone()).or_default().insert(k.trim().to_owned(), v.trim().to_owned());
        }
    }
    out
}

fn localized(group: &HashMap<String, String>, key: &str, lang: &str) -> Option<String> {
    let short = lang.split(['_', '.', '@']).next().unwrap_or("");
    let country = lang.split(['.', '@']).next().unwrap_or("");
    [format!("{key}[{country}]"), format!("{key}[{short}]"), key.to_owned()].iter().find_map(|k| group.get(k).cloned())
}

fn list(v: Option<&String>) -> Vec<String> {
    v.map(|s| s.split(';').map(str::trim).filter(|x| !x.is_empty()).map(str::to_owned).collect()).unwrap_or_default()
}

fn executable(p: &Path) -> bool {
    std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

pub fn nautilus_scripts(dir: &Path) -> Vec<Action> {
    let mut out = Vec::new();
    walk_scripts(dir, "", 0, &mut out);
    out.sort_by_cached_key(|a| a.name.to_lowercase());
    out
}

fn walk_scripts(dir: &Path, prefix: &str, depth: usize, out: &mut Vec<Action>) {
    let Ok(list) = std::fs::read_dir(dir) else { return };
    for e in list.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let path = e.path();
        if name.starts_with('.') {
            continue;
        }
        if path.is_dir() && depth < DEPTH {
            walk_scripts(&path, &format!("{prefix}{name} / "), depth + 1, out);
        } else if executable(&path) {
            let mut a = Action::new(Source::Nautilus, format!("{prefix}{name}"), String::new(), Run::Script(path));
            a.count = Count::Any;
            out.push(a);
        }
    }
}

pub fn service_menu(text: &str, lang: &str) -> Vec<Action> {
    let g = groups(text);
    let Some(main) = g.get("Desktop Entry") else { return Vec::new() };
    let service = main.get("Type").is_some_and(|t| t == "Service") || main.get("ServiceTypes").is_some_and(|t| t.contains("KonqPopupMenu"));
    if !service {
        return Vec::new();
    }
    let mimes = list(main.get("MimeType"));
    let any_file = mimes.iter().any(|m| m == "all/all" || m == "all/allfiles" || m == "application/octet-stream");
    let kept: Vec<String> = mimes.iter().filter(|m| !matches!(m.as_str(), "all/all" | "all/allfiles" | "application/octet-stream" | "inode/directory")).cloned().collect();
    let files = any_file || !kept.is_empty();
    let dirs = mimes.iter().any(|m| m == "all/all" || m == "inode/directory");
    let count = match main.get("X-KDE-RequiredNumberOfUrls").map(String::as_str) {
        Some("1") => Count::One,
        _ => Count::NotEmpty,
    };
    let submenu = localized(main, "X-KDE-Submenu", lang).unwrap_or_default();
    list(main.get("Actions"))
        .iter()
        .filter_map(|id| {
            let a = g.get(&format!("Desktop Action {id}"))?;
            let mut action = Action::new(Source::Kde, localized(a, "Name", lang)?, a.get("Icon").cloned().unwrap_or_default(), Run::Exec { line: a.get("Exec")?.clone(), base: PathBuf::new() });
            action.submenu = submenu.clone();
            action.mimes = if any_file { Vec::new() } else { kept.clone() };
            (action.files, action.dirs, action.count) = (files, dirs, count);
            Some(action)
        })
        .collect()
}

fn has_bus_name(name: &str) -> bool {
    let Some(bus) = crate::dbus::session_address().and_then(|addr| crate::udisks::Bus::open(&addr).ok()) else { return false };
    let service = "org.freedesktop.DBus";
    let reply = bus.call(service, "/org/freedesktop/DBus", service, "NameHasOwner", &[crate::atspi::V::S(name.to_owned())], 1000);
    matches!(reply.as_deref(), Ok([crate::atspi::Arg::I(1)]))
}

fn exec_condition(command: &str, base: &Path) -> bool {
    use std::process::{Command, Stdio};
    let relative = command.strip_prefix('<').and_then(|s| s.strip_suffix('>'));
    let argv = crate::shell::split_args(relative.unwrap_or(command));
    let Some((program, args)) = argv.split_first() else { return false };
    let program = if relative.is_some() { base.join(program) } else { PathBuf::from(program) };
    Command::new(program).args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
}

fn setting_equal(actual: &str, kind: &str, value: &str) -> bool {
    let actual = actual.trim();
    match kind {
        "s" | "string" => crate::shell::gsettings_string(actual).as_deref() == Some(value),
        "b" | "boolean" => matches!(value, "true" | "false") && actual == value,
        "i" | "int32" => actual.parse::<i32>().is_ok_and(|v| value.parse() == Ok(v)),
        "u" | "uint32" => actual.strip_prefix("uint32 ").and_then(|s| s.parse::<u32>().ok()).is_some_and(|v| value.parse() == Ok(v)),
        "d" | "double" => actual.contains(['.', 'e', 'E']) && actual.parse::<f64>().is_ok_and(|v| value.parse() == Ok(v)),
        _ => false,
    }
}

pub fn nemo_action(text: &str, base: &Path, lang: &str) -> Option<Action> {
    nemo_with(text, base, lang, |p| crate::shell::on_path(p.as_ref()).is_some(), |s| exec_condition(s, base), has_bus_name, |s, k| crate::shell::gsettings(&[s, k]))
}

fn nemo_with(text: &str, base: &Path, lang: &str, program: impl Fn(&str) -> bool, exec_ok: impl Fn(&str) -> bool, bus: impl Fn(&str) -> bool, setting: impl Fn(&str, &str) -> Option<String>) -> Option<Action> {
    let g = groups(text);
    let a = g.get("Nemo Action")?;
    if a.get("Active").is_some_and(|v| v.eq_ignore_ascii_case("false")) {
        return None;
    }
    let exec = a.get("Exec")?.clone();
    let name = localized(a, "Name", lang)?.replace('_', "");
    let mut action = Action::new(Source::Nemo, name, a.get("Icon-Name").cloned().unwrap_or_default(), Run::Exec { line: exec, base: base.to_path_buf() });
    for dep in list(a.get("Dependencies")) {
        let name = dep.strip_prefix('!').unwrap_or(&dep);
        if name.is_empty() || (name.contains('/') && !Path::new(name).is_absolute()) || program(name) == dep.starts_with('!') {
            return None;
        }
    }
    for cond in list(a.get("Conditions")) {
        let pass = match cond.split_once(char::is_whitespace) {
            None if cond == "removable" => { action.removable = true; true }
            Some(("exec", cmd)) => exec_ok(cmd.trim()),
            Some(("dbus", name)) if name.split_whitespace().count() == 1 => bus(name.trim()),
            Some(("gsettings", args)) => {
                let words = crate::shell::split_args(args);
                matches!(words.as_slice(), [s, k, t, v] if setting(s, k).is_some_and(|a| setting_equal(&a, t, v)))
            }
            _ => false,
        };
        if !pass { return None; }
    }
    action.separator = a.get("Separator").cloned();
    action.count = match a.get("Selection").map(|s| s.to_ascii_lowercase()).as_deref() {
        Some("m") => Count::Many,
        Some("any") => Count::Any,
        Some("none") => Count::Nothing,
        Some("notnone") => Count::NotEmpty,
        _ => Count::One,
    };
    let exts = list(a.get("Extensions")).iter().map(|e| e.trim_start_matches('.').to_ascii_lowercase()).collect::<Vec<_>>();
    let any = exts.iter().any(|e| e == "any");
    action.dirs = any || exts.iter().any(|e| e == "dir");
    action.files = any || exts.iter().any(|e| e != "dir") || a.contains_key("Mimetypes");
    action.exts = if any { Vec::new() } else { exts.into_iter().filter(|e| e != "dir" && e != "nodirs").collect() };
    action.mimes = list(a.get("Mimetypes"));
    if a.contains_key("Mimetypes") && !action.exts.is_empty() {
        action.mimes.clear();
    }
    Some(action)
}

fn tag<'a>(block: &'a str, name: &str) -> Option<&'a str> {
    let open = format!("<{name}>");
    let at = block.find(&open)? + open.len();
    let end = block[at..].find(&format!("</{name}>"))?;
    Some(&block[at..at + end])
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

pub fn thunar_actions(xml: &str) -> Vec<Action> {
    xml.split("<action>")
        .skip(1)
        .filter_map(|block| {
            let block = block.split("</action>").next()?;
            let name = unescape(tag(block, "name")?.trim());
            let command = unescape(tag(block, "command")?.trim());
            let mut a = Action::new(Source::Thunar, name, unescape(tag(block, "icon").unwrap_or_default().trim()), Run::Shell(command));
            let has = |t: &str| block.contains(&format!("<{t}/>")) || block.contains(&format!("<{t}></{t}>"));
            let kinds = [("audio-files", "audio/*"), ("image-files", "image/*"), ("text-files", "text/*"), ("video-files", "video/*")];
            let other = has("other-files");
            a.mimes = if other { Vec::new() } else { kinds.iter().filter(|(t, _)| has(t)).map(|(_, m)| m.to_string()).collect() };
            a.files = other || !a.mimes.is_empty();
            a.dirs = has("directories");
            let patterns: Vec<String> = unescape(tag(block, "patterns").unwrap_or("*")).split(';').map(|p| p.trim().to_ascii_lowercase()).filter(|p| !p.is_empty()).collect();
            if !patterns.iter().any(|p| p == "*") {
                a.exts = patterns.iter().filter_map(|p| p.strip_prefix("*.")).map(str::to_owned).collect();
            }
            Some(a)
        })
        .collect()
}

fn mime_fits(wanted: &str, mime: &str) -> bool {
    let lineage = crate::mime::lineage(mime, crate::mime::parents);
    lineage.iter().any(|m| m == wanted || wanted.strip_suffix("/*").is_some_and(|top| m.split('/').next() == Some(top)))
}

pub fn applies(a: &Action, items: &[Item]) -> bool {
    let count_ok = match a.count {
        Count::Any => true,
        Count::One => items.len() == 1,
        Count::Many => items.len() > 1,
        Count::NotEmpty => !items.is_empty(),
        Count::Nothing => items.is_empty(),
    };
    count_ok && (!a.removable || matches!(items, [i] if i.removable))
        && items.iter().all(|i| {
            if i.dir {
                return a.dirs;
            }
            let ext = i.path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
            a.files && (a.exts.is_empty() || a.exts.contains(&ext)) && (a.mimes.is_empty() || a.mimes.iter().any(|m| mime_fits(m, &i.mime)))
        })
}

fn uri(p: &Path) -> String {
    format!("file://{}", flit_platform_linux::percent_encode(p.as_os_str().as_encoded_bytes()))
}

fn lossy(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn expand(token: &str, files: &[PathBuf], current: &Path, quote: bool, separator: Option<&str>) -> Vec<String> {
    let parent = |p: &PathBuf| p.parent().map(lossy).unwrap_or_default();
    let name = |p: &PathBuf| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let values: Option<Vec<String>> = match token {
        "%f" | "%p" => Some(files.first().map(|p| lossy(p)).into_iter().collect()),
        "%F" => Some(files.iter().map(|p| lossy(p)).collect()),
        "%u" => Some(files.first().map(|p| uri(p)).into_iter().collect()),
        "%U" => Some(files.iter().map(|p| uri(p)).collect()),
        "%d" => Some(files.first().map(parent).into_iter().collect()),
        "%D" => Some(files.iter().map(parent).collect()),
        "%n" => Some(files.first().map(name).into_iter().collect()),
        "%N" => Some(files.iter().map(name).collect()),
        "%P" => Some(vec![lossy(current)]),
        "%c" | "%k" | "%i" | "%m" | "%v" => Some(Vec::new()),
        _ => None,
    };
    match values {
        Some(v) if quote => vec![v.iter().map(|s| shell_quote(s)).collect::<Vec<_>>().join(" ")],
        Some(v) => match separator {
            Some(sep) if v.len() > 1 => vec![v.join(sep)],
            _ => v,
        },
        None => vec![token.replace("%%", "%")],
    }
}

fn single(line: &str) -> bool {
    crate::shell::split_args(line).iter().any(|t| matches!(t.as_str(), "%f" | "%u" | "%d" | "%n" | "%p"))
}

pub fn commands(a: &Action, files: &[PathBuf], current: &Path) -> Vec<CommandRun> {
    let runs: Vec<Vec<PathBuf>> = match &a.run {
        Run::Exec { line, .. } | Run::Shell(line) if single(line) && files.len() > 1 => files.iter().map(|f| vec![f.clone()]).collect(),
        _ => vec![files.to_vec()],
    };
    runs.iter()
        .map(|files| match &a.run {
            Run::Script(script) => {
                let names: Vec<String> = files.iter().map(|f| f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()).collect();
                let env = vec![
                    ("NAUTILUS_SCRIPT_SELECTED_FILE_PATHS".to_owned(), files.iter().map(|f| format!("{}\n", lossy(f))).collect()),
                    ("NAUTILUS_SCRIPT_SELECTED_URIS".to_owned(), files.iter().map(|f| format!("{}\n", uri(f))).collect()),
                    ("NAUTILUS_SCRIPT_CURRENT_URI".to_owned(), uri(current)),
                ];
                (std::iter::once(lossy(script)).chain(names).collect(), env)
            }
            Run::Shell(line) => {
                let text: Vec<String> = line.split(' ').map(|t| expand(t, files, current, true, None).join(" ")).collect();
                (vec!["sh".to_owned(), "-c".to_owned(), text.join(" ")], Vec::new())
            }
            Run::Exec { line, base } => {
                let line = match line.strip_prefix('<').and_then(|l| l.strip_suffix('>')) {
                    Some(inner) => {
                        let mut parts = crate::shell::split_args(inner);
                        if let Some(first) = parts.first_mut().filter(|p| !p.starts_with('/')) {
                            *first = lossy(&base.join(&*first));
                        }
                        return (parts.iter().flat_map(|t| expand(t, files, current, false, a.separator.as_deref())).collect(), Vec::new());
                    }
                    None => line,
                };
                (crate::shell::split_args(line).iter().flat_map(|t| expand(t, files, current, false, a.separator.as_deref())).collect(), Vec::new())
            }
        })
        .filter(|(argv, _): &CommandRun| !argv.is_empty())
        .collect()
}

#[derive(Default)]
struct NemoList {
    files: Vec<(PathBuf, String)>,
    lang: String,
    actions: Vec<Action>,
}

fn cached_nemo(files: Vec<(PathBuf, String)>, lang: &str) -> Vec<Action> {
    static LIST: std::sync::LazyLock<std::sync::Mutex<NemoList>> = std::sync::LazyLock::new(Default::default);
    let mut list = LIST.lock().unwrap_or_else(|e| e.into_inner());
    if list.files != files || list.lang != lang {
        let actions = files.iter().filter_map(|(path, text)| nemo_action(text, path.parent()?, lang)).collect();
        *list = NemoList { files, lang: lang.to_owned(), actions };
    }
    list.actions.clone()
}

pub fn load(data: &[PathBuf], config: Option<&Path>, lang: &str) -> Vec<Action> {
    let mut out = Vec::new();
    if let Some(home) = data.first() {
        out.extend(nautilus_scripts(&home.join("nautilus/scripts")));
    }
    let mut nemo = Vec::new();
    for (k, d) in data.iter().enumerate() {
        for sub in ["kio/servicemenus", "kservices5/ServiceMenus"] {
            let Ok(list) = std::fs::read_dir(d.join(sub)) else { continue };
            for e in list.flatten().filter(|e| e.path().extension().is_some_and(|x| x == "desktop")) {
                if k == 0 && sub == "kio/servicemenus" && !executable(&e.path()) {
                    continue;
                }
                out.extend(std::fs::read_to_string(e.path()).map(|t| service_menu(&t, lang)).unwrap_or_default());
            }
        }
        let dir = d.join("nemo/actions");
        let Ok(list) = std::fs::read_dir(&dir) else { continue };
        for e in list.flatten().filter(|e| e.path().extension().is_some_and(|x| x == "nemo_action")) {
            if let Ok(text) = std::fs::read_to_string(e.path()) {
                nemo.push((e.path(), text));
            }
        }
    }
    nemo.sort_by(|a, b| a.0.cmp(&b.0));
    out.extend(cached_nemo(nemo, lang));
    if let Some(c) = config {
        out.extend(std::fs::read_to_string(c.join("Thunar/uca.xml")).map(|t| thunar_actions(&t)).unwrap_or_default());
    }
    out
}

fn items_of(files: &[FsPath], mounts: &[flit_platform_linux::mounts::Mount]) -> Vec<Item> {
    files
        .iter()
        .filter_map(|f| {
            let path = flit_platform_linux::host(f)?.to_path_buf();
            let dir = path.is_dir();
            let mime = if dir { flit_core::mimeapps::FOLDER.to_owned() } else { crate::mime::mime_of_file(&path) };
            let removable = mounts.iter().filter(|m| path.starts_with(m.path())).max_by_key(|m| m.point.len()).is_some_and(|m| m.removable());
            Some(Item { path, dir, mime, removable })
        })
        .collect()
}

pub fn menu(actions: &[Action], items: &[Item], current: &Path) -> (Vec<MenuItem>, Vec<Launch>) {
    let files: Vec<PathBuf> = items.iter().map(|i| i.path.clone()).collect();
    let dir = crate::xdg::fs_path(current);
    let mut entries = Vec::new();
    let mut launches = Vec::new();
    for source in Source::ORDER {
        let mut groups: Vec<(&str, Vec<MenuItem>, Vec<Launch>)> = Vec::new();
        for a in actions.iter().filter(|a| a.source == source && applies(a, items)) {
            let runs = commands(a, &files, current);
            if runs.is_empty() {
                continue;
            }
            let image = crate::apps::icon_image(&a.icon, ICON_PX).map(flit_core::ui::MenuImage::pixels);
            let entry = MenuItem::Command { label: a.name.clone(), keys: String::new(), checked: false, stay: false, mark: Mark::None, icon: "", image, disabled: false };
            let launch = Launch::Command { runs, dir: dir.clone() };
            match groups.iter_mut().find(|(name, ..)| !a.submenu.is_empty() && *name == a.submenu) {
                Some((_, list, more)) => {
                    list.push(entry);
                    more.push(launch);
                }
                None => groups.push((&a.submenu, vec![entry], vec![launch])),
            }
        }
        if groups.is_empty() {
            continue;
        }
        if !entries.is_empty() {
            entries.push(MenuItem::Separator);
        }
        for (name, list, more) in groups {
            if name.is_empty() {
                entries.extend(list);
            } else {
                entries.push(MenuItem::Submenu { label: name.to_owned(), items: list });
            }
            launches.extend(more);
        }
    }
    (entries, launches)
}

pub fn with_scripts((mut items, mut launches): (Vec<MenuItem>, Vec<Launch>), current: &FsPath, files: &[FsPath]) -> (Vec<MenuItem>, Vec<Launch>) {
    let Some(here) = flit_platform_linux::host(current) else { return (items, launches) };
    let lang = std::env::var("LC_MESSAGES").or_else(|_| std::env::var("LANG")).unwrap_or_default();
    let actions = load(&crate::apps::data_dirs(), crate::xdg::config_home().as_deref(), &lang);
    let mounts = if actions.iter().any(|a| a.removable) { flit_platform_linux::mounts::read().unwrap_or_default() } else { Vec::new() };
    let (entries, more) = menu(&actions, &items_of(files, &mounts), here);
    if entries.is_empty() {
        return (items, launches);
    }
    if !items.is_empty() {
        items.push(MenuItem::Separator);
    }
    items.push(MenuItem::Submenu { label: S::MenuScripts.t().to_owned(), items: entries });
    launches.extend(more);
    (items, launches)
}

#[cfg(test)]
#[path = "../test/scripts.rs"]
mod tests;
