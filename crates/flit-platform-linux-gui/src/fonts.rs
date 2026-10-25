#![allow(dead_code)]

use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::fs::File;
use std::os::fd::AsRawFd;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

pub const SANS: &[u8] = include_bytes!("../assets/FlitSans.ttf");
pub const ICONS: &[u8] = include_bytes!("../assets/FlitIcons.ttf");

const CACHE_HEADER: &str = "flit-fonts 1";
const WEIGHT_REGULAR: c_int = 80;
const WEIGHT_SEMIBOLD: c_int = 180;

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct FontFile {
    pub path: PathBuf,
    pub index: u32,
}

pub type Pair = [FontFile; 2];

#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Resolved {
    pub ui: Option<Pair>,
    pub mono: Option<Pair>,
    pub fallback: Vec<FontFile>,
    pub family: Option<(String, Pair)>,
}

impl Resolved {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = format!("{CACHE_HEADER}\n").into_bytes();
        let mut line = |kind: &str, f: &FontFile| {
            let path = f.path.as_os_str().as_bytes();
            if !path.contains(&b'\n') && !path.is_empty() {
                out.extend_from_slice(format!("{kind}\t{}\t", f.index).as_bytes());
                out.extend_from_slice(path);
                out.push(b'\n');
            }
        };
        let pairs = [("ui", &self.ui), ("mono", &self.mono), ("fam", &self.family.as_ref().map(|f| f.1.clone()))];
        for (kind, pair) in pairs {
            if let Some([regular, bold]) = pair {
                line(kind, regular);
                line(&format!("{kind}b"), bold);
            }
        }
        for f in &self.fallback {
            line("fb", f);
        }
        if let Some((name, _)) = &self.family {
            if !name.contains('\n') {
                out.extend_from_slice(format!("name\t{name}\n").as_bytes());
            }
        }
        out
    }

    pub fn decode(data: &[u8]) -> Option<Self> {
        let mut lines = data.split(|&b| b == b'\n');
        if lines.next()? != CACHE_HEADER.as_bytes() {
            return None;
        }
        let mut r = Resolved::default();
        let (mut slots, mut name) = (std::collections::HashMap::new(), None);
        for line in lines.filter(|l| !l.is_empty()) {
            let mut parts = line.splitn(3, |&b| b == b'\t');
            let kind = std::str::from_utf8(parts.next()?).ok()?;
            if kind == "name" {
                name = Some(String::from_utf8(parts.next()?.to_vec()).ok()?);
                continue;
            }
            let index = std::str::from_utf8(parts.next()?).ok()?.parse().ok()?;
            let path = PathBuf::from(std::ffi::OsString::from_vec(parts.next()?.to_vec()));
            let file = FontFile { path, index };
            if kind == "fb" {
                r.fallback.push(file);
            } else {
                slots.insert(kind.to_owned(), file);
            }
        }
        let mut pair = |k: &str| Some([slots.remove(k)?, slots.remove(&format!("{k}b"))?]);
        r.ui = pair("ui");
        r.mono = pair("mono");
        r.family = name.zip(pair("fam"));
        Some(r)
    }
}

pub fn cache_file() -> Option<PathBuf> {
    let base = match std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from) {
        Some(p) if p.is_absolute() => p,
        _ => PathBuf::from(std::env::var_os("HOME")?).join(".cache"),
    };
    Some(base.join("flit").join("fonts"))
}

pub fn load_cache(path: &Path) -> Option<Resolved> {
    Resolved::decode(&std::fs::read(path).ok()?)
}

pub fn save_cache(path: &Path, r: &Resolved) {
    let Some(dir) = path.parent() else { return };
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    let ok = std::fs::create_dir_all(dir).is_ok() && std::fs::write(&tmp, r.encode()).is_ok() && std::fs::rename(&tmp, path).is_ok();
    if !ok {
        let _ = std::fs::remove_file(&tmp);
    }
}

pub fn map_file(path: &Path) -> Option<&'static [u8]> {
    static MAPS: Mutex<Vec<(PathBuf, &'static [u8])>> = Mutex::new(Vec::new());
    let mut maps = MAPS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, data)) = maps.iter().find(|(p, _)| p == path) {
        return Some(data);
    }
    let file = File::open(path).ok()?;
    let len = usize::try_from(file.metadata().ok()?.len()).ok().filter(|&n| n >= 12)?;
    let ptr = unsafe { libc::mmap(std::ptr::null_mut(), len, libc::PROT_READ, libc::MAP_PRIVATE, file.as_raw_fd(), 0) };
    if ptr == libc::MAP_FAILED {
        return None;
    }
    let data: &'static [u8] = unsafe { std::slice::from_raw_parts(ptr as *const u8, len) };
    maps.push((path.to_owned(), data));
    Some(data)
}

#[repr(C)]
struct FcFontSet {
    nfont: c_int,
    sfont: c_int,
    fonts: *mut *mut c_void,
}

macro_rules! fc_api {
    ($($field:ident = $sym:literal: fn($($arg:ty),*) $(-> $ret:ty)?;)*) => {
        struct Api {
            $($field: unsafe extern "C" fn($($arg),*) $(-> $ret)?,)*
        }

        impl Api {
            fn load(lib: *mut c_void) -> Option<Self> {
                Some(Self {
                    $($field: unsafe {
                        let sym = libc::dlsym(lib, concat!($sym, "\0").as_ptr() as *const c_char);
                        if sym.is_null() {
                            return None;
                        }
                        std::mem::transmute::<*mut c_void, unsafe extern "C" fn($($arg),*) $(-> $ret)?>(sym)
                    },)*
                })
            }
        }
    };
}

fc_api! {
    init = "FcInitLoadConfigAndFonts": fn() -> *mut c_void;
    pattern_create = "FcPatternCreate": fn() -> *mut c_void;
    pattern_destroy = "FcPatternDestroy": fn(*mut c_void);
    add_string = "FcPatternAddString": fn(*mut c_void, *const c_char, *const u8) -> c_int;
    add_integer = "FcPatternAddInteger": fn(*mut c_void, *const c_char, c_int) -> c_int;
    add_bool = "FcPatternAddBool": fn(*mut c_void, *const c_char, c_int) -> c_int;
    get_string = "FcPatternGetString": fn(*mut c_void, *const c_char, c_int, *mut *const u8) -> c_int;
    get_integer = "FcPatternGetInteger": fn(*mut c_void, *const c_char, c_int, *mut c_int) -> c_int;
    get_bool = "FcPatternGetBool": fn(*mut c_void, *const c_char, c_int, *mut c_int) -> c_int;
    config_substitute = "FcConfigSubstitute": fn(*mut c_void, *mut c_void, c_int) -> c_int;
    default_substitute = "FcDefaultSubstitute": fn(*mut c_void);
    font_match = "FcFontMatch": fn(*mut c_void, *mut c_void, *mut c_int) -> *mut c_void;
    font_sort = "FcFontSort": fn(*mut c_void, *mut c_void, c_int, *mut *mut c_void, *mut c_int) -> *mut FcFontSet;
    font_list = "FcFontList": fn(*mut c_void, *mut c_void, *mut c_void) -> *mut FcFontSet;
    font_set_destroy = "FcFontSetDestroy": fn(*mut FcFontSet);
    object_set_create = "FcObjectSetCreate": fn() -> *mut c_void;
    object_set_add = "FcObjectSetAdd": fn(*mut c_void, *const c_char) -> c_int;
    object_set_destroy = "FcObjectSetDestroy": fn(*mut c_void);
    config_destroy = "FcConfigDestroy": fn(*mut c_void);
}

pub struct Fontconfig {
    api: Api,
    config: *mut c_void,
}

unsafe impl Send for Fontconfig {}
unsafe impl Sync for Fontconfig {}

fn key(s: &str) -> *const c_char {
    s.as_ptr() as *const c_char
}

const FAMILY: &str = "family\0";
const FAMILYLANG: &str = "familylang\0";
const FILE: &str = "file\0";
const INDEX: &str = "index\0";
const WEIGHT: &str = "weight\0";
const SLANT: &str = "slant\0";
const OUTLINE: &str = "outline\0";
const COLOR: &str = "color\0";
const FONTFORMAT: &str = "fontformat\0";

impl Drop for Fontconfig {
    fn drop(&mut self) {
        unsafe { (self.api.config_destroy)(self.config) }
    }
}

struct Pattern<'a>(&'a Api, *mut c_void);

impl Drop for Pattern<'_> {
    fn drop(&mut self) {
        unsafe { (self.0.pattern_destroy)(self.1) }
    }
}

impl Fontconfig {
    pub fn load() -> Option<Self> {
        let lib = unsafe { libc::dlopen(c"libfontconfig.so.1".as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
        if lib.is_null() {
            return None;
        }
        let api = Api::load(lib)?;
        let config = unsafe { (api.init)() };
        (!config.is_null()).then_some(Self { api, config })
    }

    fn string(&self, p: *mut c_void, obj: &str, n: c_int) -> Option<&[u8]> {
        let mut s = std::ptr::null();
        let found = unsafe { (self.api.get_string)(p, key(obj), n, &mut s) } == 0 && !s.is_null();
        found.then(|| unsafe { CStr::from_ptr(s as *const c_char) }.to_bytes())
    }

    fn integer(&self, p: *mut c_void, obj: &str) -> Option<c_int> {
        let mut v = 0;
        (unsafe { (self.api.get_integer)(p, key(obj), 0, &mut v) } == 0).then_some(v)
    }

    fn boolean(&self, p: *mut c_void, obj: &str) -> Option<bool> {
        let mut v = 0;
        (unsafe { (self.api.get_bool)(p, key(obj), 0, &mut v) } == 0).then_some(v != 0)
    }

    fn file(&self, p: *mut c_void) -> Option<FontFile> {
        let path = PathBuf::from(std::ffi::OsStr::from_bytes(self.string(p, FILE, 0)?));
        Some(FontFile { path, index: self.integer(p, INDEX).unwrap_or(0) as u32 })
    }

    fn query(&self, family: &str, weight: c_int) -> Option<Pattern<'_>> {
        let name = CString::new(family).ok()?;
        let p = Pattern(&self.api, unsafe { (self.api.pattern_create)() });
        unsafe {
            (self.api.add_string)(p.1, key(FAMILY), name.as_ptr() as *const u8);
            (self.api.add_integer)(p.1, key(WEIGHT), weight);
            (self.api.add_integer)(p.1, key(SLANT), 0);
            (self.api.add_bool)(p.1, key(OUTLINE), 1);
            (self.api.config_substitute)(self.config, p.1, 0);
            (self.api.default_substitute)(p.1);
        }
        Some(p)
    }

    fn matched(&self, family: &str, weight: c_int, exact: bool) -> Option<FontFile> {
        let q = self.query(family, weight)?;
        let mut result = 0;
        let raw = unsafe { (self.api.font_match)(self.config, q.1, &mut result) };
        if raw.is_null() {
            return None;
        }
        let m = Pattern(&self.api, raw);
        let named = (0..).map_while(|n| self.string(m.1, FAMILY, n)).any(|f| f.eq_ignore_ascii_case(family.as_bytes()));
        if exact && !named {
            return None;
        }
        self.file(m.1)
    }

    pub fn family(&self, family: &str) -> Option<Pair> {
        Some([self.matched(family, WEIGHT_REGULAR, true)?, self.matched(family, WEIGHT_SEMIBOLD, true)?])
    }

    fn generic(&self, family: &str) -> Option<Pair> {
        Some([self.matched(family, WEIGHT_REGULAR, false)?, self.matched(family, WEIGHT_SEMIBOLD, false)?])
    }

    pub fn fallback(&self) -> Vec<FontFile> {
        let Some(q) = self.query("sans-serif", WEIGHT_REGULAR) else { return Vec::new() };
        let mut result = 0;
        let set = unsafe { (self.api.font_sort)(self.config, q.1, 1, std::ptr::null_mut(), &mut result) };
        let mut out: Vec<FontFile> = Vec::new();
        for p in self.patterns(set) {
            let sfnt = matches!(self.string(p, FONTFORMAT, 0), Some(b"TrueType" | b"CFF"));
            if !sfnt || self.boolean(p, OUTLINE) == Some(false) || self.boolean(p, COLOR) == Some(true) {
                continue;
            }
            if let Some(f) = self.file(p).filter(|f| !out.contains(f)) {
                out.push(f);
            }
        }
        unsafe { (self.api.font_set_destroy)(set) };
        out
    }

    fn patterns(&self, set: *mut FcFontSet) -> impl Iterator<Item = *mut c_void> + '_ {
        let fonts: &[*mut c_void] = if set.is_null() {
            &[]
        } else {
            unsafe { std::slice::from_raw_parts((*set).fonts, (*set).nfont.max(0) as usize) }
        };
        fonts.iter().copied()
    }

    pub fn families(&self) -> Vec<String> {
        let p = Pattern(&self.api, unsafe { (self.api.pattern_create)() });
        let os = unsafe { (self.api.object_set_create)() };
        unsafe {
            (self.api.add_bool)(p.1, key(OUTLINE), 1);
            (self.api.object_set_add)(os, key(FAMILY));
            (self.api.object_set_add)(os, key(FAMILYLANG));
            (self.api.object_set_add)(os, key(FONTFORMAT));
        }
        let set = unsafe { (self.api.font_list)(self.config, p.1, os) };
        let mut out = Vec::new();
        for pat in self.patterns(set) {
            if !matches!(self.string(pat, FONTFORMAT, 0), Some(b"TrueType" | b"CFF")) {
                continue;
            }
            for n in 0.. {
                let Some(name) = self.string(pat, FAMILY, n) else { break };
                let lang = self.string(pat, FAMILYLANG, n);
                if lang.is_none_or(|l| l == b"en") {
                    out.push(String::from_utf8_lossy(name).into_owned());
                }
            }
        }
        unsafe {
            (self.api.font_set_destroy)(set);
            (self.api.object_set_destroy)(os);
        }
        out.retain(|s| !s.is_empty());
        out.sort_by_key(|s| s.to_lowercase());
        out.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
        out
    }

    pub fn resolve(&self, family: Option<&str>) -> Resolved {
        Resolved {
            ui: self.generic("sans-serif"),
            mono: self.generic("monospace"),
            fallback: self.fallback(),
            family: family.and_then(|name| Some((name.to_owned(), self.family(name)?))),
        }
    }
}

pub struct Live {
    pub fc: Option<Fontconfig>,
    pub resolved: Resolved,
    pub init: Duration,
    pub total: Duration,
}

pub struct Fonts {
    pub cached: Option<Resolved>,
    live: OnceLock<Live>,
    family: Mutex<Option<(String, Pair)>>,
}

static FONTS: OnceLock<Fonts> = OnceLock::new();

pub fn fonts() -> &'static Fonts {
    FONTS.get_or_init(|| {
        let cached = cache_file().and_then(|p| load_cache(&p));
        let family = cached.as_ref().and_then(|c| c.family.clone());
        let hint = family.as_ref().map(|f| f.0.clone());
        let spawned = std::thread::Builder::new().name("flit-fonts".into()).spawn(move || {
            let live = resolve_live(hint.as_deref());
            fonts().publish(live);
        });
        let fonts = Fonts { cached, live: OnceLock::new(), family: Mutex::new(family) };
        if spawned.is_err() {
            let _ = fonts.live.set(resolve_live(None));
        }
        fonts
    })
}

fn resolve_live(family: Option<&str>) -> Live {
    let start = Instant::now();
    let fc = Fontconfig::load();
    let init = start.elapsed();
    let resolved = fc.as_ref().map(|fc| fc.resolve(family)).unwrap_or_default();
    Live { fc, resolved, init, total: start.elapsed() }
}

impl Fonts {
    fn publish(&self, live: Live) {
        if live.fc.is_some() {
            let mut family = self.family.lock().unwrap_or_else(|e| e.into_inner());
            if family.as_ref().map(|f| &f.0) == live.resolved.family.as_ref().map(|f| &f.0) {
                family.clone_from(&live.resolved.family);
            }
            let mut fresh = live.resolved.clone();
            fresh.family.clone_from(&family);
            drop(family);
            if self.cached.as_ref() != Some(&fresh) {
                if let Some(path) = cache_file() {
                    save_cache(&path, &fresh);
                }
            }
        }
        let _ = self.live.set(live);
    }

    pub fn live(&self) -> Option<&Live> {
        self.live.get()
    }

    pub fn wait(&self) -> &Live {
        self.live.wait()
    }

    pub fn remembered(&self, name: &str) -> Option<Pair> {
        let family = self.family.lock().unwrap_or_else(|e| e.into_inner());
        family.as_ref().filter(|f| f.0 == name).map(|f| f.1.clone())
    }

    pub fn remember(&self, name: &str, pair: &Pair) {
        let mut family = self.family.lock().unwrap_or_else(|e| e.into_inner());
        if family.as_ref().is_some_and(|f| f.0 == name && &f.1 == pair) {
            return;
        }
        *family = Some((name.to_owned(), pair.clone()));
        let Some(live) = self.live() else { return };
        let mut r = live.resolved.clone();
        r.family.clone_from(&family);
        drop(family);
        if let Some(path) = cache_file() {
            save_cache(&path, &r);
        }
    }
}

pub fn prefetch() {
    fonts();
}

#[cfg(test)]
#[path = "../test/fonts.rs"]
mod tests;

#[cfg(test)]
#[path = "../test/subset.rs"]
pub(crate) mod subset;
