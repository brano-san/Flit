use crate::err;
use crate::fileops::item_path;
use flit_core::{attr, FsPath, Meta};
use flit_platform::PlatformError;
use windows::core::{w, Interface, GUID, HSTRING, PCWSTR};
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};
use windows::Win32::System::Search::{ICondition, IQueryParser, IQueryParserManager, QueryParserManager, SQRO_DONT_SPLIT_WORDS};
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::UI::Shell::{BHID_EnumItems, IEnumShellItems, ISearchFolderItemFactory, IShellItem, IShellItem2, IShellItemArray, SHCreateItemFromParsingName, SHCreateShellItemArrayFromShellItem, SearchFolderItemFactory};

const STORAGE: GUID = GUID::from_u128(0xb725f130_47ef_101a_a5f1_02608c9eebac);
const SIZE: PROPERTYKEY = PROPERTYKEY { fmtid: STORAGE, pid: 12 };
const ATTRIBUTES: PROPERTYKEY = PROPERTYKEY { fmtid: STORAGE, pid: 13 };
const MODIFIED: PROPERTYKEY = PROPERTYKEY { fmtid: STORAGE, pid: 14 };
const ENGLISH: u16 = 0x0409;

fn meta(item: &IShellItem) -> Meta {
    let Ok(item) = item.cast::<IShellItem2>() else { return Meta::default() };
    unsafe {
        let raw = item.GetUInt32(&ATTRIBUTES).unwrap_or(0);
        let modified = item.GetFileTime(&MODIFIED).map_or(0, |t| ((t.dwHighDateTime as i64) << 32) | t.dwLowDateTime as i64);
        let mut attrs = 0;
        for (win, ours) in [(0x10, attr::DIR), (0x2, attr::HIDDEN), (0x4, attr::SYSTEM), (0x1, attr::READONLY), (0x400, attr::LINK)] {
            if raw & win != 0 {
                attrs |= ours;
            }
        }
        let size = if attrs & attr::DIR != 0 { 0 } else { item.GetUInt64(&SIZE).unwrap_or(0) };
        Meta { size, modified, created: 0, attrs }
    }
}

unsafe fn condition(aqs: &str) -> Result<ICondition, PlatformError> {
    let manager: IQueryParserManager = CoCreateInstance(&QueryParserManager, None, CLSCTX_INPROC_SERVER).map_err(err)?;
    let parser: IQueryParser = manager.CreateLoadedParser(w!("SystemIndex"), ENGLISH).map_err(err)?;
    manager.InitializeOptions(false, true, &parser).map_err(err)?;
    let solution = parser.Parse(&HSTRING::from(aqs), None).map_err(err)?;
    let mut parsed = None;
    solution.GetQuery(Some(&mut parsed), None).map_err(err)?;
    let parsed = parsed.ok_or_else(|| PlatformError::new(-1, "Windows Search could not read the query"))?;
    let now = GetLocalTime();
    solution.Resolve(&parsed, SQRO_DONT_SPLIT_WORDS, Some(&now)).map_err(err)
}

pub(crate) fn search(folder: Option<&FsPath>, query: &str, keep_going: &dyn Fn() -> bool, found: &mut dyn FnMut(FsPath, Meta)) -> Result<(), PlatformError> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let factory: ISearchFolderItemFactory = CoCreateInstance(&SearchFolderItemFactory, None, CLSCTX_INPROC_SERVER).map_err(err)?;
        factory.SetDisplayName(w!("Flit")).map_err(err)?;
        if let Some(f) = folder {
            let wide = f.to_wide(false);
            let item: IShellItem = SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None).map_err(|e| PlatformError::new(e.code().0 as i64, format!("{}: {}", f.display(), e.message())))?;
            let scope: IShellItemArray = SHCreateShellItemArrayFromShellItem(&item).map_err(err)?;
            factory.SetScope(&scope).map_err(err)?;
        }
        factory.SetCondition(&condition(query)?).map_err(err)?;
        let root: IShellItem = factory.GetShellItem().map_err(err)?;
        let items: IEnumShellItems = root.BindToHandler(None, &BHID_EnumItems).map_err(err)?;
        while keep_going() {
            let mut one = [None];
            let mut n = 0;
            if items.Next(&mut one, Some(&mut n)).is_err() || n == 0 {
                break;
            }
            let Some(item) = one[0].take() else { break };
            if let Some(path) = item_path(&item) {
                found(path, meta(&item));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "../test/winsearch.rs"]
mod tests;
