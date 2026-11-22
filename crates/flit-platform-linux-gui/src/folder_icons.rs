use flit_core::ui::Bitmap;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::io::Read;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

const KEEP: usize = 1024;
const MAX_DIRECTORY: u64 = 64 * 1024;
const MAX_IMAGE: u64 = 16 << 20;
const GIO_WAIT: Duration = Duration::from_secs(3);
const GIO_FRESH: Duration = Duration::from_secs(30);
const GIO_OUTPUT: u64 = 1 << 20;
pub const GIO_BATCH: usize = 100;

const LIST_ICONS: [(&str, &[&str]); 8] = [
    ("XDG_DESKTOP_DIR", &["folder-desktop", "user-desktop"]),
    ("XDG_DOWNLOAD_DIR", &["folder-download"]),
    ("XDG_DOCUMENTS_DIR", &["folder-documents"]),
    ("XDG_PICTURES_DIR", &["folder-pictures"]),
    ("XDG_MUSIC_DIR", &["folder-music"]),
    ("XDG_VIDEOS_DIR", &["folder-videos"]),
    ("XDG_TEMPLATES_DIR", &["folder-templates"]),
    ("XDG_PUBLICSHARE_DIR", &["folder-publicshare"]),
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    Theme(String),
    Image(PathBuf),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Stamp {
    dev: u64,
    ino: u64,
    len: u64,
    mtime: (i64, i64),
    ctime: (i64, i64),
}

impl Stamp {
    fn of(m: &std::fs::Metadata) -> Self {
        Stamp { dev: m.dev(), ino: m.ino(), len: m.len(), mtime: (m.mtime(), m.mtime_nsec()), ctime: (m.ctime(), m.ctime_nsec()) }
    }

    fn settled(self) -> bool {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
        now - self.ctime.0.max(self.mtime.0) >= 2
    }
}

struct Known {
    own: Option<(Option<Stamp>, bool, Option<Source>)>,
    gvfs: Option<(Option<Source>, Instant)>,
    used: Instant,
}

type UserDirs = (Option<PathBuf>, Option<Stamp>, Option<PathBuf>, Vec<(&'static str, PathBuf)>);

static KNOWN: Mutex<Option<HashMap<PathBuf, Known>>> = Mutex::new(None);
static USER_DIRS: Mutex<Option<UserDirs>> = Mutex::new(None);

fn known() -> MutexGuard<'static, Option<HashMap<PathBuf, Known>>> {
    KNOWN.lock().unwrap_or_else(|e| e.into_inner())
}

fn entry<'a>(map: &'a mut HashMap<PathBuf, Known>, dir: &Path) -> &'a mut Known {
    if !map.contains_key(dir) && map.len() >= KEEP {
        if let Some(oldest) = map.iter().min_by_key(|(_, k)| k.used).map(|(p, _)| p.clone()) {
            map.remove(&oldest);
        }
    }
    let k = map.entry(dir.to_path_buf()).or_insert_with(|| Known { own: None, gvfs: None, used: Instant::now() });
    k.used = Instant::now();
    k
}

pub fn forget(dir: &Path) {
    if let Some(map) = known().as_mut() {
        map.remove(dir);
    }
}

#[cfg(test)]
pub(crate) fn remembered(dir: &Path) -> bool {
    known().as_ref().is_some_and(|m| m.contains_key(dir))
}

#[cfg(test)]
pub(crate) fn remembered_count() -> usize {
    known().as_ref().map_or(0, HashMap::len)
}

fn source(value: &str) -> Option<Source> {
    let value = value.trim();
    if value.is_empty() || value.chars().any(char::is_control) {
        return None;
    }
    if value.starts_with('/') {
        return Some(Source::Image(PathBuf::from(value)));
    }
    if value.contains('/') {
        return None;
    }
    Some(Source::Theme(value.trim_end_matches(".png").trim_end_matches(".svg").to_owned()))
}

fn unescape(value: &str) -> Option<String> {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        out.push(match chars.next()? {
            's' => ' ',
            'n' => '\n',
            't' => '\t',
            'r' => '\r',
            '\\' => '\\',
            _ => return None,
        });
    }
    Some(out)
}

pub fn directory_icon(text: &[u8]) -> Option<Source> {
    let text = std::str::from_utf8(text).ok()?;
    let mut inside = false;
    let mut found = None;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            inside = line == "[Desktop Entry]";
            continue;
        }
        if !inside || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            if key.trim() == "Icon" && found.is_none() {
                found = Some(value.trim());
            }
        }
    }
    source(&unescape(found?)?)
}

fn file_uri(uri: &[u8]) -> Option<Vec<u8>> {
    let rest = uri.strip_prefix(b"file://")?;
    let rest = rest.strip_prefix(b"localhost").unwrap_or(rest);
    if !rest.starts_with(b"/") || rest.iter().any(|&b| b == b'?' || b == b'#' || b.is_ascii_whitespace()) {
        return None;
    }
    let path = flit_platform_linux::percent_decode(rest);
    (!path.contains(&0)).then_some(path)
}

fn unescape_gio(value: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(value.len());
    let mut i = 0;
    while i < value.len() {
        if value[i] == b'\\' {
            let hex = std::str::from_utf8(value.get(i + 2..i + 4)?).ok()?;
            if value.get(i + 1) != Some(&b'x') {
                return None;
            }
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 4;
        } else {
            out.push(value[i]);
            i += 1;
        }
    }
    Some(out)
}

pub fn custom_icon(value: &[u8]) -> Option<Source> {
    let value = unescape_gio(value.trim_ascii())?;
    if value.starts_with(b"file:") {
        return file_uri(&value).map(|p| Source::Image(PathBuf::from(OsStr::from_bytes(&p))));
    }
    if value.windows(3).any(|w| w == b"://") {
        return None;
    }
    source(std::str::from_utf8(&value).ok()?)
}

pub fn parse_gio(out: &[u8], asked: &[PathBuf]) -> Vec<Option<Source>> {
    let mut seen = vec![0u8; asked.len()];
    let mut icons = vec![None; asked.len()];
    let mut current = None;
    for line in out.split(|&b| b == b'\n') {
        if let Some(uri) = line.strip_prefix(b"uri: ") {
            current = file_uri(uri).and_then(|p| asked.iter().position(|a| a.as_os_str().as_bytes() == p.as_slice()));
            if let Some(i) = current {
                seen[i] = seen[i].saturating_add(1);
            }
            continue;
        }
        if let (Some(i), Some(value)) = (current, line.strip_prefix(b"  metadata::custom-icon: ")) {
            icons[i] = custom_icon(value);
        }
    }
    icons.into_iter().zip(seen).map(|(icon, n)| icon.filter(|_| n == 1)).collect()
}

fn gio() -> Option<PathBuf> {
    #[cfg(test)]
    crate::temp::tool_path()?;
    crate::shell::on_path(OsStr::new("gio"))
}

pub fn run_gio(program: &Path, dirs: &[PathBuf]) -> Option<Vec<u8>> {
    let mut child = Command::new(program).args(["info", "-a", "metadata::custom-icon", "--"]).args(dirs).env("LC_ALL", "C").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    let stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut out = Vec::new();
        let _ = stdout.take(GIO_OUTPUT).read_to_end(&mut out);
        out
    });
    let deadline = Instant::now() + GIO_WAIT;
    let finished = loop {
        match child.try_wait() {
            Ok(Some(_)) => break true,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break false;
            }
        }
    };
    let out = reader.join().ok()?;
    finished.then_some(out)
}

pub fn prepare(dirs: &[PathBuf]) {
    let now = Instant::now();
    let mut stale: Vec<PathBuf> = {
        let mut guard = known();
        let map = guard.get_or_insert_with(HashMap::new);
        dirs.iter().filter(|d| !map.get(*d).and_then(|k| k.gvfs.as_ref()).is_some_and(|(_, at)| now.duration_since(*at) < GIO_FRESH)).cloned().collect()
    };
    stale.sort();
    stale.dedup();
    if stale.is_empty() {
        return;
    }
    let program = gio();
    for chunk in stale.chunks(GIO_BATCH) {
        let asked: Vec<PathBuf> = chunk.iter().filter(|d| !d.as_os_str().as_bytes().contains(&b'\n')).cloned().collect();
        let found = program.as_deref().filter(|_| !asked.is_empty()).and_then(|p| run_gio(p, &asked)).map(|out| parse_gio(&out, &asked)).unwrap_or_else(|| vec![None; asked.len()]);
        let mut guard = known();
        let map = guard.get_or_insert_with(HashMap::new);
        for dir in chunk {
            let icon = asked.iter().position(|a| a == dir).and_then(|i| found[i].clone());
            entry(map, dir).gvfs = Some((icon, Instant::now()));
        }
    }
}

fn gvfs(dir: &Path) -> Option<Source> {
    let cached = |map: &mut HashMap<PathBuf, Known>| map.get(dir).and_then(|k| k.gvfs.clone()).filter(|(_, at)| at.elapsed() < GIO_FRESH);
    if let Some((icon, _)) = cached(known().get_or_insert_with(HashMap::new)) {
        return icon;
    }
    prepare(&[dir.to_path_buf()]);
    cached(known().get_or_insert_with(HashMap::new)).and_then(|(icon, _)| icon)
}

fn read_own(path: &Path) -> (Option<Stamp>, Option<Source>) {
    let Ok(file) = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC).open(path) else { return (None, None) };
    let Ok(meta) = file.metadata() else { return (None, None) };
    let stamp = Some(Stamp::of(&meta));
    if !meta.is_file() || meta.len() > MAX_DIRECTORY {
        return (stamp, None);
    }
    let mut text = Vec::new();
    if file.take(MAX_DIRECTORY + 1).read_to_end(&mut text).is_err() || text.len() as u64 > MAX_DIRECTORY {
        return (stamp, None);
    }
    (stamp, directory_icon(&text))
}

fn own(dir: &Path) -> Option<Source> {
    let path = dir.join(".directory");
    let now = std::fs::metadata(&path).ok().map(|m| Stamp::of(&m));
    let hit = known().get_or_insert_with(HashMap::new).get(dir).and_then(|k| k.own.clone()).filter(|(s, settled, _)| *settled && *s == now);
    if let Some((_, _, icon)) = hit {
        return icon;
    }
    let (stamp, icon) = if now.is_some() { read_own(&path) } else { (None, None) };
    entry(known().get_or_insert_with(HashMap::new), dir).own = Some((stamp, stamp.is_none_or(Stamp::settled), icon.clone()));
    icon
}

pub fn xdg_names(dir: &Path, home: Option<&Path>, dirs: &[(&str, PathBuf)]) -> Vec<String> {
    if home == Some(dir) {
        return Vec::new();
    }
    let Some((key, _)) = dirs.iter().find(|(_, p)| p == dir) else { return Vec::new() };
    LIST_ICONS.iter().find(|(k, _)| k == key).map(|(_, names)| names.iter().map(|n| n.to_string()).collect()).unwrap_or_default()
}

fn user_dirs() -> (Option<PathBuf>, Vec<(&'static str, PathBuf)>) {
    let home = crate::xdg::home();
    let file = crate::xdg::user_dirs_file();
    let stamp = file.as_ref().and_then(|f| std::fs::metadata(f).ok()).map(|m| Stamp::of(&m));
    let mut guard = USER_DIRS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((h, s, f, dirs)) = guard.as_ref() {
        if *h == home && *s == stamp && *f == file {
            return (home, dirs.clone());
        }
    }
    let dirs = home.as_deref().map(|h| crate::xdg::user_dirs_from(&file.as_ref().and_then(|f| std::fs::read(f).ok()).unwrap_or_default(), h)).unwrap_or_default();
    *guard = Some((home.clone(), stamp, file, dirs.clone()));
    (home, dirs)
}

pub fn sources(dir: &Path) -> Vec<Source> {
    let mut out: Vec<Source> = own(dir).into_iter().chain(gvfs(dir)).collect();
    let (home, dirs) = user_dirs();
    out.extend(xdg_names(dir, home.as_deref(), &dirs).into_iter().map(Source::Theme));
    out
}

pub fn render(source: &Source, size: u32) -> Option<Bitmap> {
    match source {
        Source::Theme(name) => crate::icons::icon(std::slice::from_ref(name), size),
        Source::Image(path) => std::fs::metadata(path).ok().filter(|m| m.is_file() && m.len() <= MAX_IMAGE).and_then(|_| crate::icons::render(path, size)),
    }
}

pub fn icon(dir: &Path, size: u32) -> Option<Bitmap> {
    if !std::fs::metadata(dir).is_ok_and(|m| m.is_dir()) {
        return None;
    }
    sources(dir).iter().find_map(|s| render(s, size))
}

#[cfg(test)]
#[path = "../test/folder_icons.rs"]
mod tests;
