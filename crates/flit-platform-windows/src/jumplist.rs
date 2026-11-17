use flit_core::i18n::S;
use flit_core::path::PathKind;
use flit_core::{FsPath, Location};
use std::path::{Path, PathBuf};
use windows::core::{Interface, GUID, HSTRING, PCWSTR};
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::UI::Shell::Common::{IObjectArray, IObjectCollection};
use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;
use windows::Win32::UI::Shell::{DestinationList, EnumerableObjectCollection, ICustomDestinationList, IShellLinkW, ShellLink};

const PKEY_TITLE: PROPERTYKEY = PROPERTYKEY { fmtid: GUID::from_u128(0xf29f85e0_4ff9_1068_ab91_08002b27b3d9), pid: 2 };
const FOLDER_ICON: i32 = 3;
pub(crate) const NEW_WINDOW: &str = "--new-window";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Entry {
    pub title: String,
    pub args: String,
}

fn folder(loc: &Location) -> Option<FsPath> {
    loc.to_windows().filter(|p| matches!(p.kind(), PathKind::Drive | PathKind::Unc))
}

pub(crate) fn args_for(path: &FsPath) -> String {
    let shown = path.display();
    let tail = if shown.ends_with('\\') { "\\" } else { "" };
    format!("\"{shown}{tail}\"")
}

fn entry(path: &FsPath) -> Entry {
    let title = if path.is_root() { path.display().into_owned() } else { flit_core::name::display(path.file_name()).into_owned() };
    Entry { title, args: args_for(path) }
}

pub(crate) fn plan(pinned: &[Location], frequent: &[Location], removed: &[String], slots: usize) -> (Vec<Entry>, Vec<Entry>) {
    let mut seen: Vec<String> = Vec::new();
    let mut pick = |locs: &[Location], room: usize| -> Vec<Entry> {
        let mut out = Vec::new();
        for e in locs.iter().filter_map(folder).map(|p| entry(&p)) {
            if out.len() == room {
                break;
            }
            let key = e.args.to_lowercase();
            if seen.contains(&key) || removed.iter().any(|r| r.eq_ignore_ascii_case(&e.args)) {
                continue;
            }
            seen.push(key);
            out.push(e);
        }
        out
    };
    let pinned = pick(pinned, slots);
    let frequent = pick(frequent, slots.saturating_sub(pinned.len()));
    (pinned, frequent)
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

fn shell32() -> PathBuf {
    let root = std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
    root.join(r"System32\shell32.dll")
}

unsafe fn link(exe: &Path, e: &Entry, icon: (&Path, i32)) -> windows::core::Result<IShellLinkW> {
    let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
    link.SetPath(PCWSTR(wide(&exe.to_string_lossy()).as_ptr()))?;
    link.SetArguments(PCWSTR(wide(&e.args).as_ptr()))?;
    link.SetIconLocation(PCWSTR(wide(&icon.0.to_string_lossy()).as_ptr()), icon.1)?;
    let store: IPropertyStore = link.cast()?;
    store.SetValue(&PKEY_TITLE, &PROPVARIANT::from(e.title.as_str()))?;
    store.Commit()?;
    Ok(link)
}

unsafe fn removed_args(removed: &IObjectArray) -> Vec<String> {
    let count = removed.GetCount().unwrap_or(0);
    (0..count)
        .filter_map(|i| {
            let link: IShellLinkW = removed.GetAt(i).ok()?;
            let mut buf = [0u16; 1024];
            link.GetArguments(&mut buf).ok()?;
            let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            Some(String::from_utf16_lossy(&buf[..n]))
        })
        .collect()
}

unsafe fn collection(exe: &Path, entries: &[Entry], icon: (&Path, i32)) -> windows::core::Result<IObjectArray> {
    let items: IObjectCollection = CoCreateInstance(&EnumerableObjectCollection, None, CLSCTX_INPROC_SERVER)?;
    for e in entries {
        items.AddObject(&link(exe, e, icon)?)?;
    }
    items.cast()
}

pub fn update(pinned: &[Location], frequent: &[Location]) {
    crate::com_sta();
    let Ok(exe) = std::env::current_exe() else { return };
    if let Err(e) = unsafe { build(&exe, pinned, frequent) } {
        tracing::info!("cannot update the jump list: {}", e.message());
    }
}

unsafe fn build(exe: &Path, pinned: &[Location], frequent: &[Location]) -> windows::core::Result<()> {
    let list: ICustomDestinationList = CoCreateInstance(&DestinationList, None, CLSCTX_INPROC_SERVER)?;
    if crate::aumid::adopted() {
        list.SetAppID(&HSTRING::from(crate::toast::APP_ID))?;
    }
    let mut slots = 0u32;
    let removed: IObjectArray = list.BeginList(&mut slots)?;
    let (pinned, frequent) = plan(pinned, frequent, &removed_args(&removed), slots as usize);
    let folder_icon = shell32();
    for (name, entries) in [(S::SidebarSectionBookmarks.t(), &pinned), (S::UiJumpListFrequent.t(), &frequent)] {
        if entries.is_empty() {
            continue;
        }
        let items = collection(exe, entries, (&folder_icon, FOLDER_ICON))?;
        if let Err(e) = list.AppendCategory(PCWSTR(wide(name).as_ptr()), &items) {
            tracing::info!("the jump list refused the {name} category: {}", e.message());
        }
    }
    let tasks = [Entry { title: S::MenuNewWindow.t().to_owned(), args: NEW_WINDOW.into() }];
    list.AddUserTasks(&collection(exe, &tasks, (exe, 0))?)?;
    list.CommitList()
}

#[cfg(test)]
#[path = "../test/jumplist.rs"]
mod tests;
