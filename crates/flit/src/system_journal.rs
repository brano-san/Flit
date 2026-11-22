use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const JOURNAL: &str = "system-changes.json";
const LOCK: &str = "system-changes.lock";
const PROTOCOL_MIME: &str = "x-scheme-handler/flit";
const LOCK_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Group {
    Autostart,
    Archive,
    Protocol,
    Portal,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Target {
    File(PathBuf),
    MimeDefault(PathBuf),
    Registry { key: String, name: String },
}

impl Target {
    pub fn label(&self) -> String {
        match self {
            Self::File(path) => path.display().to_string(),
            Self::MimeDefault(path) => format!("{} · {PROTOCOL_MIME}", path.display()),
            Self::Registry { key, name } if name.is_empty() => format!(r"HKCU\{key}"),
            Self::Registry { key, name } => format!(r"HKCU\{key}\{name}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Value {
    pub kind: u32,
    pub bytes: Vec<u8>,
}

pub trait System {
    fn read(&self, target: &Target) -> io::Result<Option<Value>>;
    fn write(&self, target: &Target, value: Option<&Value>) -> io::Result<()>;
    fn registry_key_exists(&self, _key: &str) -> io::Result<bool> {
        Ok(true)
    }
    fn remove_empty_registry_key(&self, _key: &str) -> io::Result<()> {
        Ok(())
    }
}

#[derive(Deserialize, Serialize)]
pub struct Change {
    pub group: Group,
    pub target: Target,
    previous: Option<Value>,
    applied: Value,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    created_registry_keys: Vec<String>,
}

#[derive(Deserialize, Serialize)]
pub struct Record {
    version: u32,
    suspended: bool,
    #[serde(default)]
    enabled: Vec<Group>,
    #[serde(default)]
    archive_offered: bool,
    pub changes: Vec<Change>,
}

impl Default for Record {
    fn default() -> Self {
        Self { version: 1, suspended: false, enabled: Vec::new(), archive_offered: false, changes: Vec::new() }
    }
}

impl Record {
    pub fn allows_startup(&self, group: Group) -> bool {
        (!self.suspended && group != Group::Archive) || self.enabled.contains(&group)
    }

    fn should_offer_archives(&self) -> bool {
        !self.suspended && !self.archive_offered && !self.changes.iter().any(|change| change.group == Group::Archive)
    }
}

pub struct Journal {
    path: PathBuf,
}

impl Journal {
    pub fn new(directory: &Path) -> Self {
        Self { path: directory.join(JOURNAL) }
    }

    fn lock(&self) -> io::Result<File> {
        let path = self.path.with_file_name(LOCK);
        fs::create_dir_all(path.parent().unwrap_or(Path::new(".")))?;
        let deadline = Instant::now() + LOCK_TIMEOUT;
        loop {
            match try_lock(&path) {
                Err(e) if e.kind() == io::ErrorKind::WouldBlock && Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                result => return result,
            }
        }
    }

    pub fn inspect(&self) -> io::Result<Record> {
        let _lock = self.lock()?;
        load(&self.path)
    }

    #[cfg(any(windows, test))]
    pub fn offer_archive_registration(&self) -> io::Result<bool> {
        Ok(self.inspect()?.should_offer_archives())
    }

    pub fn mark_archive_offer_shown(&self) -> io::Result<()> {
        let _lock = self.lock()?;
        let mut record = load(&self.path)?;
        if record.should_offer_archives() {
            record.archive_offered = true;
            self.store(&record)?;
        }
        Ok(())
    }

    pub fn apply(&self, system: &impl System, group: Group, desired: &[(Target, Value)], startup: bool) -> io::Result<()> {
        let _lock = self.lock()?;
        let mut record = load(&self.path)?;
        if startup && !record.allows_startup(group) {
            return Ok(());
        }
        if !startup && !record.enabled.contains(&group) {
            record.enabled.push(group);
            self.store(&record)?;
        }
        for (target, value) in desired {
            let previous = system.read(target)?;
            if previous.as_ref() == Some(value) {
                continue;
            }
            let mut created_registry_keys = Vec::new();
            if let Target::Registry { key, .. } = target {
                let mut path = key.as_str();
                while !system.registry_key_exists(path)? {
                    created_registry_keys.push(path.to_owned());
                    let Some((parent, _)) = path.rsplit_once('\\') else { break };
                    path = parent;
                }
            }
            record.changes.push(Change { group, target: target.clone(), previous, applied: value.clone(), created_registry_keys });
            self.store(&record)?;
            system.write(target, Some(value))?;
        }
        Ok(())
    }

    pub fn undo(&self, system: &impl System, group: Option<Group>) -> io::Result<()> {
        let _lock = self.lock()?;
        let mut record = load(&self.path)?;
        match group {
            None if !record.suspended || !record.enabled.is_empty() => {
                record.suspended = true;
                record.enabled.clear();
                self.store(&record)?;
            }
            Some(group) if record.enabled.contains(&group) => {
                record.enabled.retain(|enabled| *enabled != group);
                self.store(&record)?;
            }
            _ => {}
        }
        while let Some(index) = record.changes.iter().rposition(|c| group.is_none_or(|g| c.group == g)) {
            let change = &record.changes[index];
            let current = system.read(&change.target)?;
            let still_applied = current.as_ref() == Some(&change.applied);
            if still_applied {
                system.write(&change.target, change.previous.as_ref())?;
            }
            if still_applied || current == change.previous {
                for key in &change.created_registry_keys {
                    system.remove_empty_registry_key(key)?;
                }
                record.changes.remove(index);
            } else {
                let target = change.target.clone();
                record.changes.retain(|c| c.target != target);
            }
            self.store(&record)?;
        }
        Ok(())
    }

    fn store(&self, record: &Record) -> io::Result<()> {
        replace(&self.path, &serde_json::to_vec(record)?)
    }
}

fn load(path: &Path) -> io::Result<Record> {
    let bytes = match read_file(path)? {
        Some(bytes) => bytes,
        None => return Ok(Record::default()),
    };
    let record: Record = serde_json::from_slice(&bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    if record.version != 1 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "unsupported system journal version"));
    }
    Ok(record)
}

fn read_file(path: &Path) -> io::Result<Option<Vec<u8>>> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_file() => fs::read(path).map(Some),
        Ok(_) => Err(io::Error::new(io::ErrorKind::InvalidInput, "system file is not a regular file")),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

fn remove(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

fn mime_text(path: &Path) -> io::Result<Option<String>> {
    read_file(path)?.map(String::from_utf8).transpose().map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

fn read_mime(path: &Path) -> io::Result<Option<Value>> {
    Ok(mime_text(path)?.map(|text| match flit_core::mimeapps::default_app(&text, PROTOCOL_MIME) {
        Some(app) => Value { kind: 0, bytes: app.into_bytes() },
        None => Value { kind: 1, bytes: Vec::new() },
    }))
}

fn write_mime(path: &Path, value: Option<&Value>) -> io::Result<()> {
    let current = mime_text(path)?;
    let old = current.as_deref().unwrap_or_default();
    let app = value.filter(|v| v.kind == 0).map(|v| std::str::from_utf8(&v.bytes)).transpose().map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let next = flit_core::mimeapps::set_default_app(old, PROTOCOL_MIME, app);
    if value.is_none() && next.trim() == flit_core::mimeapps::DEFAULTS {
        return remove(path);
    }
    if old != next {
        replace(path, next.as_bytes())?;
    }
    Ok(())
}

fn replace(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path.parent().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no parent"))?;
    fs::create_dir_all(parent)?;
    let mut temp_name = path.file_name().unwrap_or_default().to_owned();
    temp_name.push(format!(".{}.tmp", std::process::id()));
    let tmp = path.with_file_name(temp_name);
    let result = (|| {
        let mut file = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        let write = file.write_all(bytes).and_then(|()| file.sync_all());
        drop(file);
        write.and_then(|()| fs::rename(&tmp, path)).inspect_err(|_| {
            let _ = fs::remove_file(&tmp);
        })
    })();
    #[cfg(unix)]
    if result.is_ok() {
        File::open(parent)?.sync_all()?;
    }
    result
}

#[cfg(windows)]
fn try_lock(path: &Path) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    OpenOptions::new().read(true).write(true).create(true).truncate(false).share_mode(0).open(path).map_err(|e| if e.raw_os_error() == Some(32) { io::Error::new(io::ErrorKind::WouldBlock, e) } else { e })
}

#[cfg(unix)]
fn try_lock(path: &Path) -> io::Result<File> {
    flit_platform_linux_gui::instance::try_file_lock(path)?.ok_or_else(|| io::ErrorKind::WouldBlock.into())
}

pub struct Native;

impl System for Native {
    fn read(&self, target: &Target) -> io::Result<Option<Value>> {
        match target {
            Target::File(path) => read_file(path).map(|value| value.map(|bytes| Value { kind: 0, bytes })),
            Target::MimeDefault(path) => read_mime(path),
            #[cfg(windows)]
            Target::Registry { key, name } => registry::read(key, name),
            #[cfg(not(windows))]
            Target::Registry { .. } => Err(io::ErrorKind::Unsupported.into()),
        }
    }

    #[cfg(windows)]
    fn registry_key_exists(&self, path: &str) -> io::Result<bool> {
        registry::key_exists(path)
    }

    #[cfg(windows)]
    fn remove_empty_registry_key(&self, path: &str) -> io::Result<()> {
        registry::remove_empty_key(path)
    }

    fn write(&self, target: &Target, value: Option<&Value>) -> io::Result<()> {
        match target {
            Target::File(path) => match value {
                Some(value) => replace(path, &value.bytes),
                None => remove(path),
            },
            Target::MimeDefault(path) => write_mime(path, value),
            #[cfg(windows)]
            Target::Registry { key, name } => registry::write(key, name, value),
            #[cfg(not(windows))]
            Target::Registry { .. } => Err(io::ErrorKind::Unsupported.into()),
        }
    }
}

#[cfg(windows)]
mod registry {
    use super::*;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, WIN32_ERROR};
    use windows::Win32::UI::Shell::SHDeleteEmptyKeyW;
    use windows::Win32::System::Registry::{RegCloseKey, RegCreateKeyExW, RegDeleteKeyValueW, RegOpenKeyExW, RegQueryInfoKeyW, RegQueryValueExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_VALUE_TYPE};

    struct Key(HKEY);

    impl Drop for Key {
        fn drop(&mut self) {
            let _ = unsafe { RegCloseKey(self.0) };
        }
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain([0]).collect()
    }

    fn result(code: WIN32_ERROR) -> io::Result<()> {
        if code.is_ok() {
            Ok(())
        } else {
            Err(io::Error::from_raw_os_error(code.0 as i32))
        }
    }

    fn open(path: PCWSTR) -> io::Result<Option<Key>> {
        let mut key = HKEY::default();
        let opened = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, path, None, KEY_QUERY_VALUE, &mut key) };
        if opened == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        result(opened)?;
        Ok(Some(Key(key)))
    }

    pub fn key_exists(path: &str) -> io::Result<bool> {
        open(PCWSTR(wide(path).as_ptr())).map(|key| key.is_some())
    }

    fn is_empty(path: PCWSTR) -> io::Result<bool> {
        let Some(key) = open(path)? else { return Ok(false) };
        let (mut subkeys, mut values) = (0, 0);
        result(unsafe { RegQueryInfoKeyW(key.0, None, None, None, Some(&mut subkeys), None, None, Some(&mut values), None, None, None, None) })?;
        Ok(subkeys == 0 && values == 0)
    }

    pub fn remove_empty_key(path: &str) -> io::Result<()> {
        let path = wide(path);
        let path = PCWSTR(path.as_ptr());
        if !is_empty(path)? {
            return Ok(());
        }
        let code = unsafe { SHDeleteEmptyKeyW(HKEY_CURRENT_USER, path) };
        if code.is_ok() || code == ERROR_FILE_NOT_FOUND || !is_empty(path)? {
            return Ok(());
        }
        result(code)
    }

    pub fn read(path: &str, name: &str) -> io::Result<Option<Value>> {
        let Some(key) = open(PCWSTR(wide(path).as_ptr()))? else { return Ok(None) };
        let name = wide(name);
        let name = PCWSTR(name.as_ptr());
        let mut kind = REG_VALUE_TYPE(0);
        let mut size = 0u32;
        let code = unsafe { RegQueryValueExW(key.0, name, None, Some(&mut kind), None, Some(&mut size)) };
        if code == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        result(code)?;
        loop {
            let mut bytes = vec![0; size as usize];
            let code = unsafe { RegQueryValueExW(key.0, name, None, Some(&mut kind), Some(bytes.as_mut_ptr()), Some(&mut size)) };
            match code {
                ERROR_FILE_NOT_FOUND => return Ok(None),
                ERROR_MORE_DATA => continue,
                _ => result(code)?,
            }
            bytes.truncate(size as usize);
            return Ok(Some(Value { kind: kind.0, bytes }));
        }
    }

    pub fn write(path: &str, name: &str, value: Option<&Value>) -> io::Result<()> {
        let (path, name) = (wide(path), wide(name));
        let (path, name) = (PCWSTR(path.as_ptr()), PCWSTR(name.as_ptr()));
        if let Some(value) = value {
            let mut key = HKEY::default();
            result(unsafe { RegCreateKeyExW(HKEY_CURRENT_USER, path, None, PCWSTR::null(), REG_OPTION_NON_VOLATILE, KEY_SET_VALUE, None, &mut key, None) })?;
            let key = Key(key);
            result(unsafe { RegSetValueExW(key.0, name, None, REG_VALUE_TYPE(value.kind), Some(&value.bytes)) })
        } else {
            let code = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, path, name) };
            if code == ERROR_FILE_NOT_FOUND {
                Ok(())
            } else {
                result(code)
            }
        }
    }
}

#[cfg(test)]
#[path = "../test/system_journal.rs"]
mod tests;
