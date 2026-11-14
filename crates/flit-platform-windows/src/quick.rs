use flit_core::path::PathKind;
use flit_core::{FsPath, Location};
use windows::core::{w, Interface};
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::UI::Shell::PropertiesSystem::PSGetPropertyKeyFromName;
use windows::Win32::UI::Shell::{BHID_EnumItems, IEnumShellItems, IShellItem, IShellItem2, SHCreateItemFromParsingName, SIGDN_FILESYSPATH};

const MAX_READ: usize = 64;

pub(crate) fn imported(items: Vec<(FsPath, bool)>, known: &[FsPath]) -> Vec<Location> {
    let same = |a: &FsPath, b: &FsPath| a.as_bytes().eq_ignore_ascii_case(b.as_bytes());
    let mut out: Vec<FsPath> = Vec::new();
    for (path, pinned) in items {
        if !pinned || !matches!(path.kind(), PathKind::Drive | PathKind::Unc) || known.iter().any(|k| same(k, &path)) || out.iter().any(|o| same(o, &path)) {
            continue;
        }
        out.push(path);
    }
    out.into_iter().map(Location::from_local).collect()
}

unsafe fn read() -> windows::core::Result<Vec<(FsPath, bool)>> {
    let mut pinned_key = PROPERTYKEY::default();
    PSGetPropertyKeyFromName(w!("System.Home.IsPinned"), &mut pinned_key)?;
    let frequent: IShellItem = SHCreateItemFromParsingName(w!("shell:::{3936E9E4-D92C-4EEE-A85A-BC16D5EA0819}"), None)?;
    let items: IEnumShellItems = frequent.BindToHandler(None, &BHID_EnumItems)?;
    let mut out = Vec::new();
    while out.len() < MAX_READ {
        let mut one = [None];
        if items.Next(&mut one, None).is_err() {
            break;
        }
        let Some(item) = one[0].take() else { break };
        let pinned = item.cast::<IShellItem2>().and_then(|i| i.GetBool(&pinned_key)).is_ok_and(|b| b.as_bool());
        let path = item.GetDisplayName(SIGDN_FILESYSPATH).ok().and_then(|p| crate::take_pwstr(p)).and_then(|w| FsPath::from_utf16(&w));
        if let Some(path) = path {
            out.push((path, pinned));
        }
    }
    Ok(out)
}

pub fn pinned_folders(known: &[FsPath]) -> Vec<Location> {
    crate::com_sta();
    match unsafe { read() } {
        Ok(items) => imported(items, known),
        Err(e) => {
            tracing::info!("cannot read the folders pinned to Quick access: {}", e.message());
            Vec::new()
        }
    }
}

#[cfg(test)]
#[path = "../test/quick.rs"]
mod tests;
