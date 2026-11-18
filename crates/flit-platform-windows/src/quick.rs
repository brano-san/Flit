use flit_core::path::PathKind;
use flit_core::quick_sync::{QuickAccess, Snapshot};
use flit_core::{FsPath, Location};
use windows::core::{w, Interface};
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::UI::Shell::PropertiesSystem::PSGetPropertyKeyFromName;
use windows::Win32::UI::Shell::{BHID_EnumItems, IEnumShellItems, IShellItem, IShellItem2, SHCreateItemFromParsingName, SIGDN_FILESYSPATH};


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

const MAX_PINNED: usize = 64;
const MAX_ITEMS: usize = 1024;

pub(crate) struct Read {
    pub items: Vec<(FsPath, bool)>,
    pub complete: bool,
}

pub(crate) fn gather<E>(mut next: impl FnMut() -> Result<Option<(Option<FsPath>, bool)>, E>) -> Result<Read, E> {
    let mut items = Vec::new();
    let mut pinned = 0;
    for _ in 0..MAX_ITEMS {
        let Some((path, is_pinned)) = next()? else { return Ok(Read { items, complete: true }) };
        if is_pinned {
            if pinned == MAX_PINNED {
                return Ok(Read { items, complete: false });
            }
            pinned += 1;
        }
        if let Some(path) = path {
            items.push((path, is_pinned));
        }
    }
    Ok(Read { items, complete: false })
}

unsafe fn read() -> windows::core::Result<Read> {
    let mut pinned_key = PROPERTYKEY::default();
    PSGetPropertyKeyFromName(w!("System.Home.IsPinned"), &mut pinned_key)?;
    let frequent: IShellItem = SHCreateItemFromParsingName(w!("shell:::{3936E9E4-D92C-4EEE-A85A-BC16D5EA0819}"), None)?;
    let items: IEnumShellItems = frequent.BindToHandler(None, &BHID_EnumItems)?;
    gather(|| {
        let mut one = [None];
        items.Next(&mut one, None)?;
        let Some(item) = one[0].take() else { return Ok(None) };
        let pinned = item.cast::<IShellItem2>().and_then(|i| i.GetBool(&pinned_key)).is_ok_and(|b| b.as_bool());
        let path = item.GetDisplayName(SIGDN_FILESYSPATH).ok().and_then(|p| crate::take_pwstr(p)).and_then(|w| FsPath::from_utf16(&w));
        Ok(Some((path, pinned)))
    })
}

pub fn pinned_folders(known: &[FsPath]) -> Vec<Location> {
    crate::com_sta();
    match unsafe { read() } {
        Ok(read) => imported(read.items, known),
        Err(e) => {
            tracing::info!("cannot read the folders pinned to Quick access: {}", e.message());
            Vec::new()
        }
    }
}

pub(crate) const PINNED_STORE: &str = r"Microsoft\Windows\Recent\AutomaticDestinations\f01b4d95cf55d32a.automaticDestinations-ms";

pub(crate) fn stamp_of(file: &std::path::Path) -> Option<(u64, u128)> {
    let meta = std::fs::metadata(file).ok()?;
    let at = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_nanos();
    Some((meta.len(), at))
}

pub(crate) struct ShellQuickAccess {
    store: Option<std::path::PathBuf>,
}

impl ShellQuickAccess {
    pub(crate) fn new() -> Self {
        Self { store: std::env::var_os("APPDATA").map(|d| std::path::PathBuf::from(d).join(PINNED_STORE)) }
    }
}

impl QuickAccess for ShellQuickAccess {
    fn pinned(&mut self) -> Option<Snapshot> {
        crate::com_sta();
        match unsafe { read() } {
            Ok(read) => Some(Snapshot { pinned: read.items.into_iter().filter(|(_, pinned)| *pinned).map(|(p, _)| p).collect(), complete: read.complete }),
            Err(e) => {
                tracing::info!("cannot read the folders pinned to Quick access: {}", e.message());
                None
            }
        }
    }

    fn stamp(&mut self) -> Option<(u64, u128)> {
        stamp_of(self.store.as_deref()?)
    }

    fn set_pinned(&mut self, folder: &FsPath, pin: bool) -> bool {
        crate::com_sta();
        let verb = if pin { c"pintohome" } else { c"unpinfromhome" };
        match crate::context_menu::invoke_verb_quietly(std::slice::from_ref(folder), verb) {
            Ok(()) => true,
            Err(e) => {
                tracing::info!("cannot {} {} in Quick access: {e}", verb.to_string_lossy(), folder.display());
                false
            }
        }
    }

    fn is_dir(&mut self, path: &FsPath) -> bool {
        std::fs::metadata(crate::fileops::std_path(path)).is_ok_and(|m| m.is_dir())
    }
}

#[cfg(test)]
#[path = "../test/quick.rs"]
mod tests;
