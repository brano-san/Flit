use flit_core::provider::Template;
use flit_core::ui::{Drive, NewTemplate};
use flit_core::FsPath;
use std::path::PathBuf;
use windows::core::{w, PCWSTR, PWSTR};
use windows::Win32::Foundation::ERROR_NO_MORE_ITEMS;
use windows::Win32::Storage::FileSystem::{GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW};
use windows::Win32::System::Registry::{RegCloseKey, RegEnumKeyExW, RegGetValueW, RegOpenKeyExW, HKEY, HKEY_CLASSES_ROOT, HKEY_CURRENT_USER, KEY_READ, REG_ROUTINE_FLAGS, RRF_RT_ANY, RRF_RT_REG_BINARY, RRF_RT_REG_SZ};
use windows::Win32::UI::Shell::{AssocQueryStringW, SHLoadIndirectString, ASSOCF_NONE, ASSOCSTR_FRIENDLYDOCNAME};

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
    (0..26u8)
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
            let name = match wide_str(&label) {
                l if !l.is_empty() => l,
                _ => match kind {
                    DRIVE_FIXED => "Local Disk",
                    DRIVE_REMOVABLE => "USB Drive",
                    DRIVE_CDROM => "CD Drive",
                    DRIVE_REMOTE => "Network Drive",
                    _ => "Drive",
                }
                .to_owned(),
            };
            let path = FsPath::parse(&format!("{letter}:\\"))?;
            Some(Drive { path, label: format!("{name} ({letter}:)"), total, free })
        })
        .collect()
}

pub fn wsl_distros() -> Vec<String> {
    let mut key = HKEY::default();
    let mut out = Vec::new();
    unsafe {
        if RegOpenKeyExW(HKEY_CURRENT_USER, w!(r"Software\Microsoft\Windows\CurrentVersion\Lxss"), None, KEY_READ, &mut key).is_err() {
            return out;
        }
        for index in 0.. {
            let mut sub = [0u16; 256];
            let mut len = sub.len() as u32;
            if RegEnumKeyExW(key, index, Some(PWSTR(sub.as_mut_ptr())), &mut len, None, None, None, None).is_err() {
                break;
            }
            let mut name = [0u16; 256];
            let mut size = (name.len() * 2) as u32;
            let read = RegGetValueW(key, PCWSTR(sub.as_ptr()), w!("DistributionName"), RRF_RT_REG_SZ, None, Some(name.as_mut_ptr() as _), Some(&mut size));
            if read.is_ok() {
                let distro = wide_str(&name);
                if !distro.is_empty() && !out.contains(&distro) {
                    out.push(distro);
                }
            }
        }
        let _ = RegCloseKey(key);
    }
    out
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
