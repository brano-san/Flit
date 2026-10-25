use crate::err;
use crate::fileops::item_path;
use flit_core::{attr, FsPath, Meta};
use flit_platform::PlatformError;
use windows::core::{Interface, GUID, PCWSTR};
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
use windows::Win32::UI::Shell::{BHID_EnumItems, IEnumShellItems, IShellItem, IShellItem2, SHCreateItemFromParsingName};

const STORAGE: GUID = GUID::from_u128(0xb725f130_47ef_101a_a5f1_02608c9eebac);
const SIZE: PROPERTYKEY = PROPERTYKEY { fmtid: STORAGE, pid: 12 };
const ATTRIBUTES: PROPERTYKEY = PROPERTYKEY { fmtid: STORAGE, pid: 13 };
const MODIFIED: PROPERTYKEY = PROPERTYKEY { fmtid: STORAGE, pid: 14 };

fn encode(s: &str, out: &mut String) {
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
}

pub(crate) fn url(folder: Option<&FsPath>, query: &str) -> String {
    let mut u = String::from("search-ms:query=");
    encode(query, &mut u);
    if let Some(f) = folder {
        u.push_str("&crumb=location:");
        encode(&f.display(), &mut u);
    }
    u.push('&');
    u
}

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

pub(crate) fn search(folder: Option<&FsPath>, query: &str, keep_going: &dyn Fn() -> bool, found: &mut dyn FnMut(FsPath, Meta)) -> Result<(), PlatformError> {
    let wide: Vec<u16> = url(folder, query).encode_utf16().chain(Some(0)).collect();
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let root: IShellItem = SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None).map_err(err)?;
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
