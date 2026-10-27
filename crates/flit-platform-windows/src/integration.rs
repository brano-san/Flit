use flit_platform::PlatformError;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, WIN32_ERROR};
use windows::Win32::System::Registry::{
    RegCloseKey, RegDeleteKeyValueW, RegDeleteKeyW, RegDeleteTreeW, RegGetValueW, RegOpenKeyExW, RegQueryInfoKeyW, RegSetKeyValueW, HKEY, HKEY_CURRENT_USER, KEY_READ, REG_MULTI_SZ, REG_SZ, REG_VALUE_TYPE, RRF_NOEXPAND,
    RRF_RT_ANY,
};
use windows::Win32::UI::Shell::{SHChangeNotify, SHCNE_ASSOCCHANGED, SHCNF_IDLIST};

const WIN_E: &str = r"CLSID\{52205fd8-5dfb-447d-801a-d0b52f2e83e1}\shell\opennewwindow\command";
const HOME: &str = "::{f874310e-b6b7-47dc-bc84-b9e6b38f5903}";
const OLD_VERBS: [&str; 3] = [r"Directory\shell\Flit", r"Directory\Background\shell\Flit", r"Drive\shell\Flit"];
const OLD_VERB_BACKUP: &str = "OpenInFlit";
const MANAGER_BACKUP: &str = "DefaultManager";
const ABSENT: &str = "absent";
const CREATED: &str = "created";

pub struct Root {
    pub hive: HKEY,
    pub classes: String,
    pub backup: String,
}

impl Root {
    pub fn user() -> Self {
        Root { hive: HKEY_CURRENT_USER, classes: r"Software\Classes".into(), backup: r"Software\Flit\Integration".into() }
    }

    fn class(&self, key: &str) -> String {
        format!(r"{}\{key}", self.classes)
    }

    fn backup_of(&self, name: &str) -> String {
        format!(r"{}\{name}", self.backup)
    }
}

pub type Entry = (String, &'static str, String);

pub fn entries(exe: &str) -> Vec<Entry> {
    let open = format!("\"{exe}\" \"%1\"");
    vec![
        (r"Folder\shell\open\command".into(), "", open.clone()),
        (r"Folder\shell\open\command".into(), "DelegateExecute", String::new()),
        (r"Folder\shell\explore\command".into(), "", open),
        (r"Folder\shell\explore\command".into(), "DelegateExecute", String::new()),
        (WIN_E.into(), "", format!("\"{exe}\" \"{HOME}\"")),
        (WIN_E.into(), "DelegateExecute", String::new()),
    ]
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

fn fail(e: WIN32_ERROR, what: &str) -> PlatformError {
    PlatformError::new(i64::from(e.0), format!("{what}: {}", windows::core::Error::from(e.to_hresult()).message()))
}

pub fn read(hive: HKEY, key: &str, name: &str) -> Option<(u32, Vec<u8>)> {
    let (k, n) = (wide(key), wide(name));
    let (mut kind, mut len) = (REG_VALUE_TYPE(0), 0u32);
    unsafe { RegGetValueW(hive, PCWSTR(k.as_ptr()), PCWSTR(n.as_ptr()), RRF_RT_ANY | RRF_NOEXPAND, Some(&mut kind), None, Some(&mut len)) }.ok().ok()?;
    let mut data = vec![0u8; len as usize];
    unsafe { RegGetValueW(hive, PCWSTR(k.as_ptr()), PCWSTR(n.as_ptr()), RRF_RT_ANY | RRF_NOEXPAND, Some(&mut kind), Some(data.as_mut_ptr().cast()), Some(&mut len)) }.ok().ok()?;
    data.truncate(len as usize);
    Some((kind.0, data))
}

pub fn write(hive: HKEY, key: &str, name: &str, kind: u32, data: &[u8]) -> Result<(), PlatformError> {
    let (k, n) = (wide(key), wide(name));
    let r = unsafe { RegSetKeyValueW(hive, PCWSTR(k.as_ptr()), PCWSTR(n.as_ptr()), kind, Some(data.as_ptr().cast()), data.len() as u32) };
    if r.is_err() {
        return Err(fail(r, key));
    }
    Ok(())
}

fn sz(s: &str) -> Vec<u8> {
    wide(s).iter().flat_map(|c| c.to_le_bytes()).collect()
}

fn multi_sz(items: &[String]) -> Vec<u8> {
    let mut out: Vec<u8> = items.iter().flat_map(|s| sz(s)).collect();
    out.extend([0, 0]);
    out
}

fn strings(data: &[u8]) -> Vec<String> {
    let units: Vec<u16> = data.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    units.split(|&c| c == 0).filter(|s| !s.is_empty()).map(String::from_utf16_lossy).collect()
}

fn exists(hive: HKEY, key: &str) -> bool {
    let k = wide(key);
    let mut h = HKEY::default();
    let ok = unsafe { RegOpenKeyExW(hive, PCWSTR(k.as_ptr()), None, KEY_READ, &mut h) }.is_ok();
    if ok {
        unsafe {
            let _ = RegCloseKey(h);
        }
    }
    ok
}

fn empty(hive: HKEY, key: &str) -> bool {
    let k = wide(key);
    let mut h = HKEY::default();
    if unsafe { RegOpenKeyExW(hive, PCWSTR(k.as_ptr()), None, KEY_READ, &mut h) }.is_err() {
        return false;
    }
    let (mut subkeys, mut values) = (0u32, 0u32);
    let r = unsafe { RegQueryInfoKeyW(h, None, None, None, Some(&mut subkeys), None, None, Some(&mut values), None, None, None, None) };
    unsafe {
        let _ = RegCloseKey(h);
    }
    r.is_ok() && subkeys == 0 && values == 0
}

fn ancestors(key: &str) -> impl Iterator<Item = &str> {
    key.match_indices('\\').map(|(i, _)| &key[..i]).chain([key])
}

fn slot(key: &str, name: &str) -> String {
    format!("{key}|{name}")
}

pub fn set(root: &Root, on: bool, exe: &str) -> Result<bool, PlatformError> {
    let plan = entries(exe);
    let backup = root.backup_of(MANAGER_BACKUP);
    if on {
        let stale: Vec<&Entry> = plan.iter().filter(|(key, name, data)| read(root.hive, &root.class(key), name) != Some((REG_SZ.0, sz(data)))).collect();
        if stale.is_empty() && exists(root.hive, &backup) {
            return Ok(false);
        }
        if !exists(root.hive, &backup) {
            save(root, &plan, &backup)?;
        }
        for (key, name, data) in stale {
            write(root.hive, &root.class(key), name, REG_SZ.0, &sz(data))?;
        }
        return Ok(true);
    }
    if !exists(root.hive, &backup) {
        return Ok(false);
    }
    restore(root, &plan, &backup)?;
    Ok(true)
}

fn save(root: &Root, plan: &[Entry], backup: &str) -> Result<(), PlatformError> {
    let (mut absent, mut created) = (Vec::new(), Vec::new());
    for (key, name, _) in plan {
        match read(root.hive, &root.class(key), name) {
            Some((kind, data)) => write(root.hive, backup, &slot(key, name), kind, &data)?,
            None => absent.push(slot(key, name)),
        }
        for k in ancestors(key) {
            if !created.iter().any(|c| c == k) && !exists(root.hive, &root.class(k)) {
                created.push(k.to_owned());
            }
        }
    }
    write(root.hive, backup, CREATED, REG_MULTI_SZ.0, &multi_sz(&created))?;
    write(root.hive, backup, ABSENT, REG_MULTI_SZ.0, &multi_sz(&absent))
}

fn restore(root: &Root, plan: &[Entry], backup: &str) -> Result<(), PlatformError> {
    let list = |name| read(root.hive, backup, name).map(|(_, d)| strings(&d)).unwrap_or_default();
    let (absent, mut created) = (list(ABSENT), list(CREATED));
    for (key, name, _) in plan {
        let class = root.class(key);
        match read(root.hive, backup, &slot(key, name)) {
            Some((kind, data)) => write(root.hive, &class, name, kind, &data)?,
            None if absent.contains(&slot(key, name)) => {
                let (k, n) = (wide(&class), wide(name));
                let r = unsafe { RegDeleteKeyValueW(root.hive, PCWSTR(k.as_ptr()), PCWSTR(n.as_ptr())) };
                if r.is_err() && r != ERROR_FILE_NOT_FOUND {
                    return Err(fail(r, &class));
                }
            }
            None => {}
        }
    }
    created.sort_by_key(|k| std::cmp::Reverse(k.len()));
    for k in created.iter().map(|k| root.class(k)) {
        if empty(root.hive, &k) {
            let w = wide(&k);
            unsafe {
                let _ = RegDeleteKeyW(root.hive, PCWSTR(w.as_ptr()));
            }
        }
    }
    delete_tree(root.hive, backup);
    prune_backup(root);
    Ok(())
}

fn prune_backup(root: &Root) {
    for k in ancestors(&root.backup).skip(1).collect::<Vec<_>>().into_iter().rev().take_while(|k| empty(root.hive, k)) {
        delete_tree(root.hive, k);
    }
}

fn launches_flit(command: &str) -> bool {
    let Some(rest) = command.strip_prefix('"') else { return false };
    let Some((exe, args)) = rest.split_once('"') else { return false };
    let file = exe.rsplit(['\\', '/']).next().unwrap_or_default();
    file.eq_ignore_ascii_case("flit.exe") && args.trim() == "\"%V\""
}

fn old_verb_entries() -> Vec<Entry> {
    OLD_VERBS.iter().flat_map(|k| [((*k).to_owned(), "MUIVerb", String::new()), ((*k).to_owned(), "Icon", String::new()), (format!(r"{k}\command"), "", String::new())]).collect()
}

pub fn forget_open_in_flit(root: &Root) -> Result<bool, PlatformError> {
    let ours: Vec<&str> = OLD_VERBS.into_iter().filter(|k| read(root.hive, &root.class(&format!(r"{k}\command")), "").is_some_and(|(_, d)| launches_flit(&strings(&d).concat()))).collect();
    let backup = root.backup_of(OLD_VERB_BACKUP);
    if exists(root.hive, &backup) {
        restore(root, &old_verb_entries(), &backup)?;
        return Ok(true);
    }
    for k in &ours {
        delete_tree(root.hive, &root.class(k));
    }
    Ok(!ours.is_empty())
}

pub fn tidy() {
    if forget_open_in_flit(&Root::user()).unwrap_or(false) {
        tracing::info!("removed the «Open in Flit» commands left by an earlier version");
        unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None) };
    }
}

pub fn delete_tree(hive: HKEY, key: &str) {
    let k = wide(key);
    unsafe {
        let _ = RegDeleteTreeW(hive, PCWSTR(k.as_ptr()));
        let _ = RegDeleteKeyW(hive, PCWSTR(k.as_ptr()));
    }
}

pub fn integrate(on: bool) -> Result<(), PlatformError> {
    let exe = std::env::current_exe().map_err(|e| PlatformError::new(-1, e.to_string()))?;
    let changed = set(&Root::user(), on, &exe.to_string_lossy())?;
    crate::shellwin::enable(on);
    if changed {
        unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None) };
    }
    Ok(())
}

#[cfg(test)]
#[path = "../test/integration.rs"]
mod tests;
