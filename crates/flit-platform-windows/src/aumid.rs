use crate::toast::APP_ID;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use windows::core::{Interface, GUID, HSTRING, PCWSTR};
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::Storage::FileSystem::{ReplaceFileW, REPLACEFILE_IGNORE_MERGE_ERRORS};
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::Com::{CoCreateInstance, IPersistFile, CLSCTX_INPROC_SERVER, STGM, STGM_READ, STGM_READWRITE};
use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;
use windows::Win32::UI::Shell::{
    IShellLinkW, SHGetKnownFolderPath, SetCurrentProcessExplicitAppUserModelID, ShellLink, FOLDERID_CommonStartMenu, FOLDERID_Desktop, FOLDERID_PublicDesktop, FOLDERID_StartMenu, FOLDERID_UserPinned, KF_FLAG_DEFAULT,
};

const PKEY_APP_ID: PROPERTYKEY = PROPERTYKEY { fmtid: GUID::from_u128(0x9f4c2855_9f79_4b39_a8d0_e1d42de1d5f3), pid: 5 };
const MAX_DEPTH: usize = 6;
const MAX_LINKS: usize = 5000;
const START_DELAY: std::time::Duration = std::time::Duration::from_secs(5);

static ADOPTED: AtomicBool = AtomicBool::new(false);

pub fn adopt() -> Result<(), String> {
    match unsafe { SetCurrentProcessExplicitAppUserModelID(&HSTRING::from(APP_ID)) } {
        Ok(()) => {
            ADOPTED.store(true, Ordering::Relaxed);
            Ok(())
        }
        Err(e) => Err(e.message()),
    }
}

pub(crate) fn adopted() -> bool {
    ADOPTED.load(Ordering::Relaxed)
}

fn plain(path: &Path) -> String {
    let text = path.to_string_lossy();
    let text = text.strip_prefix(r"\\?\").unwrap_or(&text);
    text.replace('/', "\\").trim_end_matches('\\').to_lowercase()
}

pub(crate) fn same_exe(target: &Path, exe: &Path) -> bool {
    if target.as_os_str().is_empty() {
        return false;
    }
    if plain(target) == plain(exe) {
        return true;
    }
    matches!((std::fs::canonicalize(target), std::fs::canonicalize(exe)), (Ok(a), Ok(b)) if plain(&a) == plain(&b))
}

pub(crate) fn mentions(bytes: &[u8], name: &str) -> bool {
    if name.is_empty() || !name.is_ascii() {
        return true;
    }
    let lower: Vec<u8> = bytes.iter().map(u8::to_ascii_lowercase).collect();
    let narrow = name.to_ascii_lowercase().into_bytes();
    let wide: Vec<u8> = narrow.iter().flat_map(|&b| [b, 0]).collect();
    lower.windows(narrow.len()).any(|w| w == narrow.as_slice()) || lower.windows(wide.len()).any(|w| w == wide.as_slice())
}

pub(crate) fn needs_id(target: &Path, id: Option<&str>, exe: &Path) -> bool {
    same_exe(target, exe) && id != Some(APP_ID)
}

pub(crate) fn shortcuts(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack: Vec<(PathBuf, usize)> = roots.iter().map(|r| (r.clone(), 0)).collect();
    while let Some((dir, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else { continue };
            let path = entry.path();
            if kind.is_dir() {
                if depth < MAX_DEPTH {
                    stack.push((path, depth + 1));
                }
            } else if kind.is_file() && path.extension().is_some_and(|e| e.eq_ignore_ascii_case("lnk")) && !out.contains(&path) {
                out.push(path);
                if out.len() == MAX_LINKS {
                    return out;
                }
            }
        }
    }
    out
}

pub(crate) trait Links {
    fn inspect(&mut self, link: &Path) -> Result<(PathBuf, Option<String>), String>;
    fn write(&mut self, link: &Path) -> Result<(), String>;
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Outcome {
    pub fixed: Vec<PathBuf>,
    pub failed: Vec<(PathBuf, String)>,
}

pub(crate) fn fix(roots: &[PathBuf], exe: &Path, links: &mut impl Links) -> Outcome {
    let mut outcome = Outcome::default();
    let name = exe.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    for link in shortcuts(roots) {
        let Ok(bytes) = std::fs::read(&link) else { continue };
        if !mentions(&bytes, &name) {
            continue;
        }
        let (target, id) = match links.inspect(&link) {
            Ok(found) => found,
            Err(e) => {
                tracing::info!("cannot read the shortcut {}: {e}", link.display());
                continue;
            }
        };
        if !needs_id(&target, id.as_deref(), exe) {
            continue;
        }
        let result = if std::fs::metadata(&link).is_ok_and(|m| m.permissions().readonly()) { Err("the shortcut is read-only".to_owned()) } else { links.write(&link) };
        match result {
            Ok(()) => outcome.fixed.push(link),
            Err(e) => outcome.failed.push((link, e)),
        }
    }
    outcome
}

fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain([0]).collect()
}

pub(crate) struct ShellLinks;

impl ShellLinks {
    unsafe fn load(link: &Path, mode: STGM) -> windows::core::Result<IShellLinkW> {
        let shell: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        shell.cast::<IPersistFile>()?.Load(PCWSTR(wide(link).as_ptr()), mode)?;
        Ok(shell)
    }

    fn temporary(link: &Path) -> PathBuf {
        let mut name = link.file_name().unwrap_or_default().to_os_string();
        name.push(format!(".flit-{}.tmp", std::process::id()));
        link.with_file_name(name)
    }
}

impl Links for ShellLinks {
    fn inspect(&mut self, link: &Path) -> Result<(PathBuf, Option<String>), String> {
        unsafe {
            let shell = Self::load(link, STGM_READ).map_err(|e| e.message())?;
            let mut buf = vec![0u16; 32768];
            shell.GetPath(&mut buf, std::ptr::null_mut(), 0).map_err(|e| e.message())?;
            let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            let target = PathBuf::from(String::from_utf16_lossy(&buf[..n]));
            let value = shell.cast::<IPropertyStore>().and_then(|s| s.GetValue(&PKEY_APP_ID)).map_err(|e| e.message())?;
            let id = if value.is_empty() { None } else { Some(value.to_string()) };
            Ok((target, id))
        }
    }

    fn write(&mut self, link: &Path) -> Result<(), String> {
        let temp = Self::temporary(link);
        let result = unsafe {
            (|| -> windows::core::Result<()> {
                let shell = Self::load(link, STGM_READWRITE)?;
                let store: IPropertyStore = shell.cast()?;
                store.SetValue(&PKEY_APP_ID, &PROPVARIANT::from(APP_ID))?;
                store.Commit()?;
                shell.cast::<IPersistFile>()?.Save(PCWSTR(wide(&temp).as_ptr()), false)?;
                ReplaceFileW(PCWSTR(wide(link).as_ptr()), PCWSTR(wide(&temp).as_ptr()), PCWSTR::null(), REPLACEFILE_IGNORE_MERGE_ERRORS, None, None)
            })()
        };
        if result.is_err() {
            let _ = std::fs::remove_file(&temp);
        }
        result.map_err(|e| e.message())
    }
}

fn known(id: &GUID) -> Option<PathBuf> {
    let w = unsafe { crate::take_pwstr(SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok()?) }?;
    Some(PathBuf::from(String::from_utf16_lossy(&w)))
}

fn roots() -> Vec<PathBuf> {
    let pinned = known(&FOLDERID_UserPinned)
        .or_else(|| std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join(r"Microsoft\Internet Explorer\Quick Launch\User Pinned")))
        .map(|d| d.join("TaskBar"));
    [known(&FOLDERID_StartMenu), known(&FOLDERID_CommonStartMenu), pinned, known(&FOLDERID_Desktop), known(&FOLDERID_PublicDesktop)].into_iter().flatten().collect()
}

pub fn fix_shortcuts_in_background() {
    let _ = std::thread::Builder::new().name("flit-shortcuts".into()).spawn(|| {
        std::thread::sleep(START_DELAY);
        crate::com_sta();
        let Ok(exe) = std::env::current_exe() else { return };
        let outcome = fix(&roots(), &exe, &mut ShellLinks);
        for link in &outcome.fixed {
            tracing::info!("gave the shortcut {} the identity {APP_ID}", link.display());
        }
        for (link, e) in &outcome.failed {
            tracing::info!("left the shortcut {} without the identity {APP_ID}: {e}", link.display());
        }
    });
}

#[cfg(test)]
#[path = "../test/aumid.rs"]
mod tests;
