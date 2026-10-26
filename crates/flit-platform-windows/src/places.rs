use flit_core::provider::Template;
use flit_core::ui::{Drive, NewTemplate, Recent};
use flit_core::location::valid_distro;
use flit_core::FsPath;
use flit_core::i18n::S;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use windows::core::{w, Interface, PCWSTR, PWSTR};
use windows::Win32::Foundation::ERROR_NO_MORE_ITEMS;
use windows::Win32::Storage::FileSystem::{GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW, FILE_ATTRIBUTE_DIRECTORY, WIN32_FIND_DATAW};
use windows::Win32::System::Com::{CoCreateInstance, IPersistFile, CLSCTX_INPROC_SERVER, STGM_READ};
use windows::Win32::System::Registry::{
    RegCloseKey, RegEnumKeyExW, RegGetValueW, RegNotifyChangeKeyValue, RegOpenKeyExW, HKEY, HKEY_CLASSES_ROOT, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_NOTIFY, KEY_READ, REG_NOTIFY_CHANGE_LAST_SET, REG_NOTIFY_CHANGE_NAME, REG_ROUTINE_FLAGS, RRF_RT_ANY,
    RRF_RT_REG_BINARY, RRF_RT_REG_DWORD, RRF_RT_REG_SZ,
};
use windows::Win32::UI::Shell::{AssocQueryStringW, IShellLinkW, SHGetKnownFolderPath, SHLoadIndirectString, ShellLink, ASSOCF_NONE, ASSOCSTR_FRIENDLYDOCNAME, FOLDERID_Recent, KF_FLAG_DEFAULT};

const DRIVE_REMOVABLE: u32 = 2;
const DRIVE_FIXED: u32 = 3;
const DRIVE_REMOTE: u32 = 4;
const DRIVE_CDROM: u32 = 5;

fn wide_str(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

pub fn drives() -> Vec<Drive> {
    let mask = unsafe { GetLogicalDrives() };
    let mut out: Vec<Drive> = (0..26u8)
        .filter(|i| mask & (1 << i) != 0)
        .filter_map(|i| {
            let letter = (b'A' + i) as char;
            let root: Vec<u16> = format!("{letter}:\\").encode_utf16().chain(Some(0)).collect();
            let root = PCWSTR(root.as_ptr());
            let kind = unsafe { GetDriveTypeW(root) };
            let mut label = [0u16; 261];
            let (mut total, mut free) = (0u64, 0u64);
            if kind != DRIVE_REMOTE {
                unsafe {
                    let _ = GetVolumeInformationW(root, Some(&mut label), None, None, None, None);
                    let _ = GetDiskFreeSpaceExW(root, None, Some(&mut total), Some(&mut free));
                }
            }
            if kind == DRIVE_REMOTE {
                if let Some((remote, online)) = crate::network::connection(&format!("{letter}:")) {
                    return network_drive(letter, remote, online);
                }
            }
            let volume_label = wide_str(&label);
            let default_kind = volume_label.is_empty().then_some(match kind {
                DRIVE_FIXED => S::SidebarDriveLocal,
                DRIVE_REMOVABLE => S::SidebarDriveUsb,
                DRIVE_CDROM => S::SidebarDriveCd,
                DRIVE_REMOTE => S::SidebarDriveNetwork,
                _ => S::SidebarDriveOther,
            });
            let name = default_kind.map_or(volume_label, |k| k.en().to_owned());
            let path = FsPath::parse(&format!("{letter}:\\"))?;
            Some(Drive { path, label: flit_core::tr_en!(S::SidebarDriveLabel, name = name, letter = letter), total, free, removable: matches!(kind, DRIVE_REMOVABLE | DRIVE_CDROM), unmounted: false, kind: default_kind, remote: None })
        })
        .collect();
    for (local, remote) in crate::network::remembered() {
        let letter = local.chars().next().filter(|c| c.is_ascii_uppercase() && local.len() == 2);
        if let Some(d) = letter.filter(|&c| mask & (1 << (c as u8 - b'A')) == 0).and_then(|c| network_drive(c, remote, false)) {
            out.push(d);
        }
    }
    out.sort_by(|a, b| a.path.as_bytes().cmp(b.path.as_bytes()));
    out.push(Drive { path: FsPath::network(), label: S::PathNetwork.en().to_owned(), total: 0, free: 0, removable: false, unmounted: false, kind: Some(S::PathNetwork), remote: None });
    out
}

pub(crate) fn share_name(remote: &str) -> String {
    match remote.trim_end_matches('\\').rsplit_once('\\') {
        Some((parent, last)) if parent.len() > 2 => format!("{last} ({parent})"),
        _ => remote.to_owned(),
    }
}

fn network_drive(letter: char, remote: String, online: bool) -> Option<Drive> {
    let path = FsPath::parse(&format!("{letter}:\\"))?;
    let label = flit_core::tr_en!(S::SidebarDriveLabel, name = share_name(&remote), letter = letter);
    Some(Drive { path, label, total: 0, free: 0, removable: false, unmounted: !online, kind: None, remote: Some(remote) })
}

fn dword(sub: &str, name: &str) -> Option<u32> {
    let mut v = 0u32;
    query(HKEY_CURRENT_USER, Some(sub), Some(name), RRF_RT_REG_DWORD, Some((&mut v as *mut u32).cast()), 4)?;
    Some(v)
}

pub fn recent_off() -> bool {
    dword(r"Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced", "Start_TrackDocs") == Some(0) || dword(r"Software\Microsoft\Windows\CurrentVersion\Policies\Explorer", "NoRecentDocsHistory") == Some(1)
}

fn ticks(t: SystemTime) -> i64 {
    t.duration_since(UNIX_EPOCH).map_or(0, |d| (d.as_nanos() / 100) as i64 + 116_444_736_000_000_000)
}

fn shortcut(link: &Path) -> Option<(FsPath, bool)> {
    let wide: Vec<u16> = link.as_os_str().to_string_lossy().encode_utf16().chain(Some(0)).collect();
    let mut buf = [0u16; 1024];
    let mut found = WIN32_FIND_DATAW::default();
    unsafe {
        let sl: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        sl.cast::<IPersistFile>().ok()?.Load(PCWSTR(wide.as_ptr()), STGM_READ).ok()?;
        sl.GetPath(&mut buf, &mut found, 0).ok()?;
    }
    let n = buf.iter().position(|&c| c == 0)?;
    Some((FsPath::from_utf16(&buf[..n])?, found.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY.0 != 0))
}

pub fn recent_files(dir: &Path, limit: usize) -> Vec<Recent> {
    crate::com_sta();
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let is_link = |p: &Path| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("lnk"));
    let mut links: Vec<(SystemTime, PathBuf)> = entries.filter_map(|e| e.ok()).filter(|e| is_link(&e.path())).filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path()))).collect();
    links.sort_by_key(|l| std::cmp::Reverse(l.0));
    let mut out: Vec<Recent> = Vec::with_capacity(limit);
    for (at, link) in links {
        if out.len() == limit {
            break;
        }
        let Some((target, dir)) = shortcut(&link) else { continue };
        if out.iter().any(|r| r.target == target) {
            continue;
        }
        let name = flit_core::name::display(target.file_name()).into_owned();
        out.push(Recent { name, target, dir, at: ticks(at) });
    }
    out
}

pub fn recent(limit: usize) -> (Vec<Recent>, bool) {
    crate::com_sta();
    let dir = unsafe { SHGetKnownFolderPath(&FOLDERID_Recent, KF_FLAG_DEFAULT, None) }.ok().and_then(|p| unsafe { crate::take_pwstr(p) });
    let files = dir.map(|w| recent_files(Path::new(&String::from_utf16_lossy(&w)), limit)).unwrap_or_default();
    (files, recent_off())
}

const LXSS: PCWSTR = w!(r"Software\Microsoft\Windows\CurrentVersion\Lxss");
const INSTALLED: u32 = 1;

pub fn watch_lxss(changed: Box<dyn Fn() + Send>) {
    let watcher = move || {
        let mut key = HKEY::default();
        if unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, LXSS, None, KEY_NOTIFY, &mut key) }.is_err() {
            return;
        }
        let key = Key(key);
        while unsafe { RegNotifyChangeKeyValue(key.0, true, REG_NOTIFY_CHANGE_NAME | REG_NOTIFY_CHANGE_LAST_SET, None, false) }.is_ok() {
            changed();
        }
    };
    if let Err(e) = std::thread::Builder::new().name("flit-lxss-watch".into()).spawn(watcher) {
        tracing::warn!("cannot watch the WSL distribution list: {e}");
    }
}

fn lxss() -> Vec<(String, Option<FsPath>)> {
    let mut key = HKEY::default();
    let mut out: Vec<(String, Option<FsPath>)> = Vec::new();
    unsafe {
        if RegOpenKeyExW(HKEY_CURRENT_USER, LXSS, None, KEY_READ, &mut key).is_err() {
            return out;
        }
        let key = Key(key);
        for index in 0.. {
            let mut sub = [0u16; 256];
            let mut len = sub.len() as u32;
            if RegEnumKeyExW(key.0, index, Some(PWSTR(sub.as_mut_ptr())), &mut len, None, None, None, None).is_err() {
                break;
            }
            let text = |value: PCWSTR| {
                let mut buf = [0u16; 1024];
                let mut size = (buf.len() * 2) as u32;
                RegGetValueW(key.0, PCWSTR(sub.as_ptr()), value, RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr() as _), Some(&mut size)).is_ok().then(|| wide_str(&buf))
            };
            let (mut state, mut state_size) = (INSTALLED, 4u32);
            let _ = RegGetValueW(key.0, PCWSTR(sub.as_ptr()), w!("State"), RRF_RT_REG_DWORD, None, Some(&mut state as *mut u32 as _), Some(&mut state_size));
            match text(w!("DistributionName")) {
                Some(distro) if state == INSTALLED && valid_distro(&distro) && !out.iter().any(|(d, _)| *d == distro) => out.push((distro, text(w!("ShortcutPath")).and_then(|p| FsPath::parse(&p)))),
                _ => {}
            }
        }
    }
    out
}

pub fn date_patterns() -> (String, String) {
    let read = |name: &str, fallback: &str| string(HKEY_CURRENT_USER, Some(r"Control Panel\International"), Some(name)).unwrap_or_else(|| fallback.to_owned());
    (read("sShortDate", "yyyy-MM-dd"), read("sShortTime", "HH:mm"))
}

pub fn ui_language() -> String {
    let id = unsafe { windows::Win32::Globalization::GetUserDefaultUILanguage() };
    language_code(id).into()
}

pub(crate) fn language_code(langid: u16) -> &'static str {
    const ZH_CN: u16 = 0x0804;
    const ZH_SG: u16 = 0x1004;
    match (langid & 0x3FF, langid) {
        (0x19, _) => "ru",
        (0x09, _) => "en",
        (0x07, _) => "de",
        (0x0C, _) => "fr",
        (0x0A, _) => "es",
        (0x22, _) => "uk",
        (_, ZH_CN | ZH_SG) => "zh-CN",
        _ => "",
    }
}

pub fn os_version() -> String {
    const KEY: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
    let build = string(HKEY_LOCAL_MACHINE, Some(KEY), Some("CurrentBuildNumber")).unwrap_or_default();
    let name = if build.parse::<u32>().is_ok_and(|b| b >= 22000) { "Windows 11" } else { "Windows 10" };
    let edition = string(HKEY_LOCAL_MACHINE, Some(KEY), Some("EditionID")).unwrap_or_default();
    let release = string(HKEY_LOCAL_MACHINE, Some(KEY), Some("DisplayVersion")).unwrap_or_default();
    let mut ubr = 0u32;
    let _ = query(HKEY_LOCAL_MACHINE, Some(KEY), Some("UBR"), RRF_RT_REG_DWORD, Some((&mut ubr as *mut u32).cast()), 4);
    format!("{name} {edition} {release} (build {build}.{ubr})").split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn wsl_distros() -> Vec<String> {
    lxss().into_iter().map(|(distro, _)| distro).collect()
}

pub fn wsl_shortcut(distro: &str) -> Option<FsPath> {
    lxss().into_iter().find(|(d, _)| d.eq_ignore_ascii_case(distro))?.1
}

struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        let _ = unsafe { RegCloseKey(self.0) };
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn open_class(sub: &str) -> Option<Key> {
    let mut key = HKEY::default();
    unsafe { RegOpenKeyExW(HKEY_CLASSES_ROOT, PCWSTR(wide(sub).as_ptr()), None, KEY_READ, &mut key) }.ok().ok()?;
    Some(Key(key))
}

fn query(key: HKEY, sub: Option<&str>, name: Option<&str>, kind: REG_ROUTINE_FLAGS, data: Option<*mut std::ffi::c_void>, mut size: u32) -> Option<usize> {
    let (sub, name) = (sub.map(wide), name.map(wide));
    let ptr = |w: &Option<Vec<u16>>| w.as_ref().map_or(PCWSTR::null(), |w| PCWSTR(w.as_ptr()));
    unsafe { RegGetValueW(key, ptr(&sub), ptr(&name), kind, None, data, Some(&mut size)) }.ok().ok()?;
    Some(size as usize)
}

fn string(key: HKEY, sub: Option<&str>, name: Option<&str>) -> Option<String> {
    let mut buf = [0u16; 1024];
    query(key, sub, name, RRF_RT_REG_SZ, Some(buf.as_mut_ptr().cast()), (buf.len() * 2) as u32)?;
    Some(wide_str(&buf)).filter(|s| !s.is_empty())
}

fn indirect(s: String) -> Option<String> {
    if !s.starts_with('@') {
        return Some(s);
    }
    let mut buf = [0u16; 512];
    unsafe { SHLoadIndirectString(PCWSTR(wide(&s).as_ptr()), &mut buf, None) }.ok()?;
    Some(wide_str(&buf)).filter(|s| !s.is_empty())
}

fn friendly_name(ext: &str) -> Option<String> {
    let mut buf = [0u16; 512];
    let mut len = buf.len() as u32;
    unsafe { AssocQueryStringW(ASSOCF_NONE, ASSOCSTR_FRIENDLYDOCNAME, PCWSTR(wide(ext).as_ptr()), PCWSTR::null(), Some(PWSTR(buf.as_mut_ptr())), &mut len) }.ok().ok()?;
    Some(wide_str(&buf)).filter(|s| !s.is_empty())
}

fn template_dirs() -> Vec<PathBuf> {
    [("APPDATA", r"Microsoft\Windows\Templates"), ("ProgramData", r"Microsoft\Windows\Templates"), ("SystemRoot", "ShellNew")]
        .iter()
        .filter_map(|(var, sub)| Some(PathBuf::from(std::env::var_os(var)?).join(sub)))
        .collect()
}

fn template_file(name: &str, dirs: &[PathBuf]) -> Option<FsPath> {
    let found = std::iter::once(PathBuf::from(name)).filter(|p| p.is_absolute()).chain(dirs.iter().map(|d| d.join(name))).find(|p| p.is_file())?;
    FsPath::parse(found.to_str()?)
}

fn shell_new(key: HKEY, ext: &str, dirs: &[PathBuf]) -> Option<NewTemplate> {
    let has = |name: &str| query(key, None, Some(name), RRF_RT_ANY, None, 0).is_some();
    if has("Handler") || has("Command") {
        return None;
    }
    let mut data = vec![0u8; 4096];
    let template = if let Some(file) = string(key, None, Some("FileName")) {
        Template::Copy(template_file(&file, dirs)?)
    } else if let Some(n) = query(key, None, Some("Data"), RRF_RT_REG_BINARY, Some(data.as_mut_ptr().cast()), data.len() as u32) {
        data.truncate(n);
        Template::Data(data)
    } else if has("NullFile") {
        Template::Empty
    } else {
        return None;
    };
    let label = string(key, None, Some("MenuText")).and_then(indirect).or_else(|| friendly_name(ext))?;
    let stem = string(key, None, Some("ItemName")).and_then(indirect).unwrap_or_else(|| format!("New {label}"));
    Some(NewTemplate { label, ext: ext.to_owned(), stem, template })
}

fn class_template(ext: &str, dirs: &[PathBuf]) -> Option<NewTemplate> {
    let key = open_class(&format!(r"{ext}\ShellNew")).or_else(|| open_class(&format!(r"{ext}\{}\ShellNew", string(HKEY_CLASSES_ROOT, Some(ext), None)?)))?;
    shell_new(key.0, ext, dirs)
}

pub fn templates() -> Vec<NewTemplate> {
    let dirs = template_dirs();
    let mut out = Vec::new();
    for index in 0.. {
        let mut name = [0u16; 256];
        let mut len = name.len() as u32;
        let r = unsafe { RegEnumKeyExW(HKEY_CLASSES_ROOT, index, Some(PWSTR(name.as_mut_ptr())), &mut len, None, None, None, None) };
        if r == ERROR_NO_MORE_ITEMS {
            break;
        }
        if r.is_ok() && name[0] == u16::from(b'.') {
            out.extend(class_template(&String::from_utf16_lossy(&name[..len as usize]), &dirs));
        }
    }
    out.sort_by_cached_key(|t: &NewTemplate| t.label.to_lowercase());
    out
}

#[cfg(test)]
#[path = "../test/places.rs"]
mod tests;
