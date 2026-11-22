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
    if text {
        "text/plain"
    } else {
        OCTET_STREAM
    }
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
    names.get_or_insert_with(HashMap::new).entry(mime.clone()).or_insert_with(|| mime_dirs().iter().find_map(|d| parse_comment(&std::fs::read_to_string(d.join(format!("{mime}.xml"))).ok()?))).clone()
}

const CONTENT_HEAD: usize = 16 * 1024;
const CONTENT_CACHE_LIMIT: usize = 2048;

struct ContentApi {
    guess: unsafe extern "C" fn(*const libc::c_char, *const u8, usize, *mut libc::c_int) -> *mut libc::c_char,
    mime: unsafe extern "C" fn(*const libc::c_char) -> *mut libc::c_char,
    free: unsafe extern "C" fn(*mut libc::c_void),
    _gio: usize,
    _glib: usize,
}

fn content_api() -> Option<&'static ContentApi> {
    static API: OnceLock<Option<ContentApi>> = OnceLock::new();
    API.get_or_init(|| unsafe {
        let gio = libc::dlopen(c"libgio-2.0.so.0".as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL);
        if gio.is_null() {
            return None;
        }
        let glib = libc::dlopen(c"libglib-2.0.so.0".as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL);
        if glib.is_null() {
            libc::dlclose(gio);
            return None;
        }
        let guess = libc::dlsym(gio, c"g_content_type_guess".as_ptr());
        let mime = libc::dlsym(gio, c"g_content_type_get_mime_type".as_ptr());
        let free = libc::dlsym(glib, c"g_free".as_ptr());
        if guess.is_null() || mime.is_null() || free.is_null() {
            libc::dlclose(glib);
            libc::dlclose(gio);
            return None;
        }
        Some(ContentApi {
            guess: std::mem::transmute::<*mut libc::c_void, unsafe extern "C" fn(*const libc::c_char, *const u8, usize, *mut libc::c_int) -> *mut libc::c_char>(guess),
            mime: std::mem::transmute::<*mut libc::c_void, unsafe extern "C" fn(*const libc::c_char) -> *mut libc::c_char>(mime),
            free: std::mem::transmute::<*mut libc::c_void, unsafe extern "C" fn(*mut libc::c_void)>(free),
            _gio: gio as usize,
            _glib: glib as usize,
        })
    })
    .as_ref()
}

fn content_mime(data: &[u8]) -> Option<String> {
    content_mime_with(data, content_api())
}

fn content_mime_with(data: &[u8], api: Option<&ContentApi>) -> Option<String> {
    guess_mime(None, Some(data), api).map(|(mime, _)| mime)
}

fn guess_mime(name: Option<&std::ffi::CStr>, data: Option<&[u8]>, api: Option<&ContentApi>) -> Option<(String, bool)> {
    let api = api?;
    let mut uncertain = 0;
    let name = name.map_or(std::ptr::null(), std::ffi::CStr::as_ptr);
    let (data, len) = data.map_or((std::ptr::null(), 0), |bytes| (bytes.as_ptr(), bytes.len()));
    unsafe {
        let content = (api.guess)(name, data, len, &mut uncertain);
        if content.is_null() {
            return None;
        }
        let mime = (api.mime)(content);
        let out = (!mime.is_null()).then(|| std::ffi::CStr::from_ptr(mime).to_str().ok().map(str::to_owned)).flatten();
        (api.free)(mime.cast());
        (api.free)(content.cast());
        out.map(|mime| (mime, uncertain == 0))
    }
}

#[derive(Clone, PartialEq, Eq)]
struct ContentKey {
    path: PathBuf,
    modified: (i64, i64),
    size: u64,
}

impl ContentKey {
    fn of(path: &std::path::Path, m: &std::fs::Metadata) -> Option<Self> {
        use std::os::unix::fs::MetadataExt;
        (m.is_file() && m.len() > 0 && m.mode() & 0o111 == 0).then(|| Self { path: path.to_owned(), modified: (m.mtime(), m.mtime_nsec()), size: m.len() })
    }
}

#[derive(Default)]
struct ContentCache {
    entries: std::collections::VecDeque<(ContentKey, String)>,
}

impl ContentCache {
    fn get(&self, key: &ContentKey) -> Option<&String> {
        self.entries.iter().find(|(k, _)| k == key).map(|(_, m)| m)
    }

    fn insert(&mut self, key: ContentKey, mime: String) {
        self.entries.retain(|(k, _)| k.path != key.path);
        if self.entries.len() >= CONTENT_CACHE_LIMIT {
            self.entries.pop_front();
        }
        self.entries.push_back((key, mime));
    }
}

fn content_head(path: &std::path::Path) -> Option<(ContentKey, Vec<u8>)> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW).open(path).ok()?;
    let key = ContentKey::of(path, &file.metadata().ok()?)?;
    let mut data = Vec::with_capacity(CONTENT_HEAD);
    file.take(CONTENT_HEAD as u64).read_to_end(&mut data).ok()?;
    Some((key, data))
}

const LOCAL_MEMORY: [&str; 2] = ["tmpfs", "ramfs"];
const MOUNTS_FRESH: std::time::Duration = std::time::Duration::from_secs(2);

fn content_readable(path: &std::path::Path, mounts: &[flit_platform_linux::mounts::Mount]) -> bool {
    let Some(mount) = mounts.iter().filter(|m| path.starts_with(m.path())).max_by_key(|m| m.point.len()) else { return true };
    !(mount.is_network() || (mount.is_pseudo() && !LOCAL_MEMORY.contains(&mount.fstype.as_str())))
}

fn current_mounts() -> std::sync::Arc<Vec<flit_platform_linux::mounts::Mount>> {
    static MOUNTS: Mutex<Option<(std::time::Instant, std::sync::Arc<Vec<flit_platform_linux::mounts::Mount>>)>> = Mutex::new(None);
    let mut slot = MOUNTS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, mounts)) = slot.as_ref().filter(|(at, _)| at.elapsed() < MOUNTS_FRESH) {
        return mounts.clone();
    }
    let fresh = std::sync::Arc::new(flit_platform_linux::mounts::read().unwrap_or_default());
    *slot = Some((std::time::Instant::now(), fresh.clone()));
    fresh
}

fn content_names_in(cache: &Mutex<Option<ContentCache>>, path: &std::path::Path, readable: bool, read: impl FnOnce(&ContentKey) -> String) -> Vec<String> {
    if !readable {
        return Vec::new();
    }
    let Some(key) = std::fs::symlink_metadata(path).ok().and_then(|m| ContentKey::of(path, &m)) else { return Vec::new() };
    let known = cache.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(ContentCache::default).get(&key).cloned();
    let mime = match known {
        Some(mime) => mime,
        None => {
            let mime = read(&key);
            cache.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(ContentCache::default).insert(key, mime.clone());
            mime
        }
    };
    icon_names(&mime)
}

#[derive(Default)]
struct PickerTypes {
    registered: std::collections::HashSet<String>,
    aliases: HashMap<String, String>,
}

impl PickerTypes {
    fn resolve(&self, mime: String) -> Option<String> {
        let (media, subtype) = mime.split_once('/')?;
        let valid = |part: &str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_alphanumeric() || b"!#$&-^_.+".contains(&b));
        if !valid(media) || (subtype != "*" && !valid(subtype)) {
            return None;
        }
        if let Some(canonical) = self.aliases.get(&mime) {
            return Some(canonical.clone());
        }
        (subtype == "*" || mime == "text/plain" || mime == OCTET_STREAM || self.registered.is_empty() || self.registered.contains(&mime)).then_some(mime)
    }
}

pub(crate) fn picker_filter_type(mime: String) -> Option<String> {
    static TYPES: OnceLock<PickerTypes> = OnceLock::new();
    TYPES
        .get_or_init(|| {
            let mut types = PickerTypes::default();
            for dir in mime_dirs().iter().rev() {
                if let Ok(text) = std::fs::read_to_string(dir.join("types")) {
                    types.registered.extend(text.lines().map(str::to_owned));
                }
                if let Ok(text) = std::fs::read_to_string(dir.join("aliases")) {
                    types.aliases.extend(text.lines().filter_map(|line| line.split_once(' ')).map(|(alias, mime)| (alias.to_owned(), mime.to_owned())));
                }
            }
            types
        })
        .resolve(mime)
}

pub(crate) fn picker_mime(path: &std::path::Path) -> Option<std::sync::Arc<[String]>> {
    let name = std::ffi::CString::new(path.file_name()?.as_encoded_bytes()).ok()?;
    let api = content_api();
    let certain = |data: Option<&[u8]>| guess_mime(Some(&name), data, api).and_then(|(mime, certain)| certain.then_some(mime));
    let mime = certain(None).or_else(|| path.extension().and_then(|ext| mime_of(ext.as_encoded_bytes()))).or_else(|| {
        if !content_readable(path, &current_mounts()) || !std::fs::symlink_metadata(path).ok()?.is_file() {
            return None;
        }
        let (_, data) = content_head(path)?;
        certain(Some(&data)).or_else(|| (sniff(&data) == "text/plain").then(|| "text/plain".to_owned()))
    })?;
    let mut types = lineage(&mime, parents);
    if mime != OCTET_STREAM && !mime.starts_with("inode/") {
        types.push(OCTET_STREAM.into());
    }
    Some(types.into())
}

pub fn content_icon_names(path: &std::path::Path) -> Vec<String> {
    static CACHE: Mutex<Option<ContentCache>> = Mutex::new(None);
    let readable = content_readable(path, &current_mounts());
    content_names_in(&CACHE, path, readable, |key| content_head(path).filter(|(actual, _)| actual == key).map(|(_, data)| content_mime(&data).unwrap_or_else(|| sniff(&data).to_owned())).unwrap_or_else(|| OCTET_STREAM.into()))
}

#[cfg(test)]
#[path = "../test/mime.rs"]
mod tests;
