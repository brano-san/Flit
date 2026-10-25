use crate::fs::fs_error;
use crate::{com_sta, err, take_pwstr, NtFs};
use flit_core::library::{self, Library, LibraryOp};
use flit_core::sort::CaseMode;
use flit_core::{FsError, FsPath, ListingBuilder};
use flit_platform::{Fs, PlatformError};
use std::io::Write as _;
use windows::core::{GUID, PCWSTR};
use windows::Win32::Foundation::{ERROR_GEN_FAILURE, WIN32_ERROR};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER, STGM_READ, STGM_READWRITE};
use windows::Win32::UI::Shell::{
    IShellItem, IShellItemArray, IShellLibrary, SHChangeNotify, SHCreateItemFromParsingName, SHGetKnownFolderPath, ShellLibrary, DSFT_DETECT, FOLDERID_Libraries, KF_FLAG_DEFAULT, LFF_FORCEFILESYSTEM, LSF_FAILIFTHERE,
    SHCNE_CREATE, SHCNF_PATHW, SIGDN_FILESYSPATH, SIGDN_NORMALDISPLAY,
};

pub(crate) struct Classic {
    pub file: &'static str,
    name: i32,
    icon: i32,
    folder_type: &'static str,
    description: i32,
    folder: &'static str,
}

pub(crate) const DEFAULTS: [Classic; 4] = [
    Classic { file: "Documents", name: -34575, icon: -1002, folder_type: "7d49d726-3c21-4f05-99aa-fdc2c9474656", description: -34577, folder: "FDD39AD0-238F-46AF-ADB4-6C85480369C7" },
    Classic { file: "Music", name: -34584, icon: -1004, folder_type: "94d6ddcc-4a68-4175-a374-bd584a510b78", description: -34586, folder: "4BD8D571-6D19-48D3-BE97-422220080E43" },
    Classic { file: "Pictures", name: -34595, icon: -1003, folder_type: "b3690e58-e961-423b-b687-386ebfd83239", description: -34597, folder: "33E28130-4E1E-4676-835A-98395C3BC3BB" },
    Classic { file: "Videos", name: -34620, icon: -1005, folder_type: "5fa96407-7e77-483c-ac93-691d05850de8", description: -34622, folder: "18989B1D-99B5-455B-841C-AB7C74E4DDFC" },
];

impl Classic {
    pub(crate) fn xml(&self) -> String {
        let Classic { name, icon, folder_type, description, folder, .. } = self;
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\r\n<libraryDescription xmlns=\"http://schemas.microsoft.com/windows/2009/library\"><name>@shell32.dll,{name}</name><version>4</version><isLibraryPinned>true</isLibraryPinned><iconReference>imageres.dll,{icon}</iconReference><templateInfo><folderType>{{{folder_type}}}</folderType></templateInfo><searchConnectorDescriptionList><searchConnectorDescription publisher=\"Microsoft\" product=\"Windows\"><description>@shell32.dll,{description}</description><isDefaultSaveLocation>true</isDefaultSaveLocation><simpleLocation><url>knownfolder:{{{folder}}}</url></simpleLocation></searchConnectorDescription></searchConnectorDescriptionList></libraryDescription>\r\n"
        )
    }
}

fn wide(p: &FsPath) -> Vec<u16> {
    p.display().encode_utf16().chain(Some(0)).collect()
}

fn item(p: &FsPath) -> windows::core::Result<IShellItem> {
    unsafe { SHCreateItemFromParsingName(PCWSTR(wide(p).as_ptr()), None) }
}

fn path_of(item: &IShellItem) -> Option<FsPath> {
    FsPath::from_utf16(&unsafe { take_pwstr(item.GetDisplayName(SIGDN_FILESYSPATH).ok()?) }?)
}

fn load(path: &FsPath, write: bool) -> windows::core::Result<IShellLibrary> {
    com_sta();
    unsafe {
        let lib: IShellLibrary = CoCreateInstance(&ShellLibrary, None, CLSCTX_INPROC_SERVER)?;
        lib.LoadLibraryFromItem(&item(path)?, if write { STGM_READWRITE.0 } else { STGM_READ.0 })?;
        Ok(lib)
    }
}

fn folders_of(lib: &IShellLibrary) -> Vec<FsPath> {
    let Ok(items) = (unsafe { lib.GetFolders::<IShellItemArray>(LFF_FORCEFILESYSTEM) }) else { return Vec::new() };
    let n = unsafe { items.GetCount() }.unwrap_or(0);
    (0..n).filter_map(|k| path_of(&unsafe { items.GetItemAt(k) }.ok()?)).collect()
}

pub(crate) fn folders(path: &FsPath) -> Vec<FsPath> {
    load(path, false).map(|lib| folders_of(&lib)).unwrap_or_default()
}

pub(crate) fn read(path: &FsPath) -> Option<Library> {
    let lib = load(path, false).ok()?;
    let name = unsafe { take_pwstr(item(path).ok()?.GetDisplayName(SIGDN_NORMALDISPLAY).ok()?) }.map(|w| String::from_utf16_lossy(&w)).unwrap_or_else(|| library::stem(path));
    let save = unsafe { lib.GetDefaultSaveFolder::<IShellItem>(DSFT_DETECT) }.ok().and_then(|i| path_of(&i));
    Some(Library { name, path: path.clone(), folders: folders_of(&lib), save })
}

fn libraries_dir() -> Option<std::path::PathBuf> {
    com_sta();
    let w = unsafe { take_pwstr(SHGetKnownFolderPath(&FOLDERID_Libraries, KF_FLAG_DEFAULT, None).ok()?) }?;
    Some(String::from_utf16_lossy(&w).into())
}

pub(crate) fn libraries() -> Vec<Library> {
    let Some(dir) = libraries_dir() else { return Vec::new() };
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut paths: Vec<FsPath> = entries.filter_map(|e| FsPath::parse(e.ok()?.path().to_str()?)).filter(library::is_library).collect();
    paths.sort_by_key(|p| p.display().to_lowercase());
    paths.iter().filter_map(read).collect()
}

pub(crate) fn list(dir: &FsPath, out: &mut ListingBuilder<'_>) -> Result<(), FsError> {
    let lib = load(dir, false).map_err(|e| fs_error(WIN32_ERROR::from_error(&e).unwrap_or(ERROR_GEN_FAILURE)))?;
    for folder in folders_of(&lib) {
        let mut go = |_: &ListingBuilder<'_>| true;
        let mut part = ListingBuilder::new(CaseMode::Insensitive, &mut go);
        if NtFs.read_dir(&folder, &mut part).is_err() {
            continue;
        }
        let l = part.listing;
        for i in 0..l.len() as u32 {
            library::push(out, l.name(i), l.entries[i as usize].meta(), &folder);
        }
        if !out.batch_done() {
            break;
        }
    }
    Ok(())
}

fn change(path: &FsPath, f: impl FnOnce(&IShellLibrary) -> windows::core::Result<()>) -> Result<(), PlatformError> {
    let lib = load(path, true).map_err(err)?;
    f(&lib).and_then(|_| unsafe { lib.Commit() }).map_err(err)
}

fn restore(dir: &std::path::Path, defaults: &[Classic]) -> Result<usize, PlatformError> {
    let mut made = 0;
    for d in defaults {
        let path = dir.join(format!("{}.library-ms", d.file));
        let file = match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(PlatformError::new(-1, format!("cannot create {}: {e}", path.display()))),
        };
        let mut file = file;
        file.write_all(d.xml().as_bytes()).map_err(|e| PlatformError::new(-1, format!("cannot write {}: {e}", path.display())))?;
        let w: Vec<u16> = path.as_os_str().to_string_lossy().encode_utf16().chain(Some(0)).collect();
        unsafe { SHChangeNotify(SHCNE_CREATE, SHCNF_PATHW, Some(w.as_ptr().cast()), None) };
        made += 1;
    }
    Ok(made)
}

pub(crate) fn apply(op: &LibraryOp) -> Result<(), PlatformError> {
    match op {
        LibraryOp::Create(name) => unsafe {
            com_sta();
            let lib: IShellLibrary = CoCreateInstance(&ShellLibrary, None, CLSCTX_INPROC_SERVER).map_err(err)?;
            let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
            lib.SaveInKnownFolder(&FOLDERID_Libraries as *const GUID, PCWSTR(wide.as_ptr()), LSF_FAILIFTHERE).map(drop).map_err(err)
        },
        LibraryOp::RestoreDefaults => {
            let dir = libraries_dir().ok_or_else(|| PlatformError::new(-1, "the Libraries folder is unknown"))?;
            restore(&dir, &DEFAULTS).map(drop)
        }
        LibraryOp::Add { library, folder } => change(library, |l| unsafe { l.AddFolder(&item(folder)?) }),
        LibraryOp::Remove { library, folder } => change(library, |l| unsafe { l.RemoveFolder(&item(folder)?) }),
        LibraryOp::SetSave { library, folder } => change(library, |l| unsafe { l.SetDefaultSaveFolder(DSFT_DETECT, &item(folder)?) }),
    }
}

#[cfg(test)]
#[path = "../test/library.rs"]
mod tests;
