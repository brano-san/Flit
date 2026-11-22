use crate::fs::fs_error;
use crate::{com_sta, take_pwstr};
use flit_core::listing::{attr, ListingBuilder};
use flit_core::path::PathKind;
use flit_core::{FsError, FsErrorKind, FsPath, Meta};
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use windows::core::{Interface, GUID, PCWSTR};
use windows::Win32::Foundation::{PROPERTYKEY, WIN32_ERROR};
use windows::Win32::System::SystemServices::{SFGAO_FOLDER, SFGAO_HIDDEN, SFGAO_STREAM};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{BHID_EnumItems, IEnumShellItems, ILClone, ILGetSize, IShellFolder, IShellItem, IShellItem2, SHBindToObject, SHCreateItemFromIDList, SHCreateItemFromParsingName, SHGetIDListFromObject, SHParseDisplayName, SIGDN_DESKTOPABSOLUTEPARSING, SIGDN_NORMALDISPLAY};

const STORAGE: GUID = GUID::from_u128(0xb725f130_47ef_101a_a5f1_02608c9eebac);
const SIZE: PROPERTYKEY = PROPERTYKEY { fmtid: STORAGE, pid: 12 };
const MODIFIED: PROPERTYKEY = PROPERTYKEY { fmtid: STORAGE, pid: 14 };
const FILE_NAME: PROPERTYKEY = PROPERTYKEY { fmtid: GUID::from_u128(0x41cf5ae0_f75a_4806_bd87_59c7d9248eb9), pid: 100 };
const BATCH: usize = 32;
const FTP_FOLDERS: usize = 64;

pub(crate) trait Node: Sized {
    fn name(&self) -> Vec<u8>;
    fn meta(&self) -> Meta;
    fn children(&self, each: &mut dyn FnMut(Self) -> bool) -> Result<(), FsError>;
}

pub(crate) fn not_found() -> FsError {
    FsError { kind: FsErrorKind::NotFound, code: 2 }
}

pub(crate) fn walk<N: Node>(mut node: N, path: &[&[u8]]) -> Result<N, FsError> {
    for want in path {
        let mut found = None;
        node.children(&mut |child| {
            let hit = child.name().eq_ignore_ascii_case(want);
            if hit {
                found = Some(child);
            }
            !hit
        })?;
        node = found.ok_or_else(not_found)?;
    }
    Ok(node)
}

fn list<N: Node>(node: &N, out: &mut ListingBuilder<'_>, mut listed: impl FnMut(&[u8], &N)) -> Result<(), FsError> {
    let (mut n, mut going) = (0usize, true);
    node.children(&mut |child| {
        let name = child.name();
        out.push(&name, child.meta());
        listed(&name, &child);
        n += 1;
        if n.is_multiple_of(BATCH) {
            going = out.batch_done();
        }
        going
    })?;
    if going && !n.is_multiple_of(BATCH) {
        out.batch_done();
    }
    Ok(())
}

fn hr_error(e: windows::core::Error) -> FsError {
    let code = e.code().0 as u32;
    match code >> 16 {
        0x8007 => fs_error(WIN32_ERROR(code & 0xFFFF)),
        _ => FsError { kind: FsErrorKind::Other, code: code as i64 },
    }
}

struct Item {
    raw: IShellItem,
    ftp: bool,
}

fn utf8(w: &[u16]) -> Vec<u8> {
    let mut b = Vec::with_capacity(w.len());
    flit_core::name::push_utf16(&mut b, w);
    b
}

impl Node for Item {
    fn name(&self) -> Vec<u8> {
        unsafe {
            let file = self.raw.cast::<IShellItem2>().ok().and_then(|i| i.GetString(&FILE_NAME).ok()).and_then(|p| take_pwstr(p));
            let shown = || self.raw.GetDisplayName(SIGDN_NORMALDISPLAY).ok().and_then(|p| take_pwstr(p));
            file.filter(|w| !w.is_empty()).or_else(shown).map(|w| utf8(&w)).unwrap_or_default()
        }
    }

    fn meta(&self) -> Meta {
        unsafe {
            let flags = self.raw.GetAttributes(SFGAO_FOLDER | SFGAO_STREAM | SFGAO_HIDDEN).unwrap_or_default();
            let dir = flags & SFGAO_FOLDER == SFGAO_FOLDER && flags & SFGAO_STREAM != SFGAO_STREAM;
            let mut m = Meta { attrs: if dir { attr::DIR } else { 0 }, ..Default::default() };
            if flags & SFGAO_HIDDEN == SFGAO_HIDDEN {
                m.attrs |= attr::HIDDEN;
            }
            if let Ok(rich) = self.raw.cast::<IShellItem2>() {
                m.modified = rich.GetFileTime(&MODIFIED).map_or(0, |t| ((t.dwHighDateTime as i64) << 32) | t.dwLowDateTime as i64);
                if !dir {
                    m.size = rich.GetUInt64(&SIZE).unwrap_or(0);
                }
            }
            m
        }
    }

    fn children(&self, each: &mut dyn FnMut(Self) -> bool) -> Result<(), FsError> {
        let items: IEnumShellItems = unsafe { self.raw.BindToHandler(None, &BHID_EnumItems) }.map_err(hr_error)?;
        enumerate(self.ftp, || {
            let (mut one, mut n) = ([None], 0u32);
            unsafe { items.Next(&mut one, Some(&mut n)) }.map_err(hr_error)?;
            Ok(one[0].take().filter(|_| n != 0).map(|raw| Item { raw, ftp: self.ftp }))
        }, each)
    }
}

fn enumerate<T>(ftp: bool, mut next: impl FnMut() -> Result<Option<T>, FsError>, each: &mut dyn FnMut(T) -> bool) -> Result<(), FsError> {
    loop {
        match next() {
            Ok(Some(child)) => {
                if !each(child) {
                    return Ok(());
                }
            }
            Ok(None) => return Ok(()),
            Err(e) if ftp => return Err(e),
            Err(_) => return Ok(()),
        }
    }
}

fn parse(name: &str) -> Result<IShellItem, FsError> {
    let w: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    unsafe { SHCreateItemFromParsingName(PCWSTR(w.as_ptr()), None) }.map_err(hr_error)
}

fn start(p: &FsPath) -> Result<(IShellItem, Vec<&[u8]>), FsError> {
    let root = p.shell_root().ok_or_else(not_found)?;
    let mut rest: Vec<&[u8]> = p.components().collect();
    if let Some(ftp) = crate::network::ftp_root(p) {
        return Ok((parse_ftp(ftp.as_str())?, rest));
    }
    if p.is_network() && !rest.is_empty() {
        let server = String::from_utf8_lossy(rest.remove(0)).into_owned();
        return Ok((parse(&format!(r"\\{server}"))?, rest));
    }
    Ok((parse(&String::from_utf8_lossy(root))?, rest))
}

fn parse_ftp(uri: &str) -> Result<IShellItem, FsError> {
    let wide: Vec<u16> = uri.encode_utf16().chain(Some(0)).collect();
    let mut pidl = std::ptr::null_mut();
    unsafe { SHParseDisplayName(PCWSTR(wide.as_ptr()), None, &mut pidl, 0, None) }.map_err(hr_error)?;
    let result = unsafe {
        SHBindToObject::<_, _, IShellFolder>(None, pidl, None).and_then(|_| SHCreateItemFromIDList(pidl))
    };
    unsafe { CoTaskMemFree(Some(pidl.cast())) };
    result.map_err(hr_error)
}

struct Folder {
    path: FsPath,
    items: Vec<(Vec<u8>, IShellItem)>,
}

thread_local! {
    static LAST: RefCell<Option<Folder>> = const { RefCell::new(None) };
}

pub(crate) fn item(p: &FsPath) -> Result<IShellItem, FsError> {
    com_sta();
    if p.kind() != PathKind::Shell {
        let w = p.to_wide(false);
        return unsafe { SHCreateItemFromParsingName(PCWSTR(w.as_ptr()), None) }.map_err(hr_error);
    }
    let Some(parent) = p.parent().filter(|d| d.kind() == PathKind::Shell && !d.is_network()) else {
        let (raw, rest) = start(p)?;
        return walk(Item { raw, ftp: p.is_ftp() }, &rest).map(|i| i.raw);
    };
    let cached = LAST.with_borrow(|last| last.as_ref().filter(|f| f.path == parent).map(|f| f.items.iter().find(|(n, _)| n.eq_ignore_ascii_case(p.file_name())).map(|(_, i)| i.clone())));
    if let Some(found) = cached {
        return found.ok_or_else(not_found);
    }
    let (raw, rest) = start(&parent)?;
    let folder = walk(Item { raw, ftp: p.is_ftp() }, &rest)?;
    let mut items = Vec::new();
    folder.children(&mut |child| {
        items.push((child.name(), child.raw));
        true
    })?;
    let found = items.iter().find(|(n, _)| n.eq_ignore_ascii_case(p.file_name())).map(|(_, i)| i.clone());
    LAST.set(Some(Folder { path: parent, items }));
    found.ok_or_else(not_found)
}

#[derive(Default)]
struct FolderPidls(Mutex<HashMap<Vec<u8>, Vec<u8>>>);

impl FolderPidls {
    fn remember(&self, name: &[u8], item: &IShellItem) {
        let Ok(pidl) = (unsafe { SHGetIDListFromObject(item) }) else { return };
        let bytes = unsafe {
            let bytes = std::slice::from_raw_parts(pidl.cast::<u8>(), ILGetSize(Some(pidl)) as usize).to_vec();
            CoTaskMemFree(Some(pidl.cast()));
            bytes
        };
        self.0.lock().unwrap_or_else(|e| e.into_inner()).insert(name.to_vec(), bytes);
    }

    fn clone_pidl(&self, name: &[u8]) -> Option<*mut ITEMIDLIST> {
        let items = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let pidl = unsafe { ILClone(items.get(name)?.as_ptr().cast()) };
        (!pidl.is_null()).then_some(pidl)
    }
}

static FTP_PIDLS: Mutex<VecDeque<(FsPath, Arc<FolderPidls>)>> = Mutex::new(VecDeque::new());

fn remember_folder(dir: &FsPath, item: &IShellItem) -> Arc<FolderPidls> {
    let ids = Arc::new(FolderPidls::default());
    ids.remember(b"", item);
    let mut folders = FTP_PIDLS.lock().unwrap_or_else(|e| e.into_inner());
    folders.retain(|(path, _)| path != dir);
    folders.push_front((dir.clone(), Arc::clone(&ids)));
    folders.truncate(FTP_FOLDERS);
    ids
}

pub(crate) fn cached_ftp_pidl(path: &FsPath) -> Option<*mut ITEMIDLIST> {
    let parent = path.parent();
    let (own, siblings) = {
        let folders = FTP_PIDLS.lock().unwrap_or_else(|e| e.into_inner());
        let get = |p: &FsPath| folders.iter().find(|(dir, _)| dir == p).map(|(_, ids)| Arc::clone(ids));
        (get(path), parent.as_ref().and_then(get))
    };
    own.and_then(|ids| ids.clone_pidl(b"")).or_else(|| siblings?.clone_pidl(path.file_name()))
}

pub(crate) fn read_dir(dir: &FsPath, out: &mut ListingBuilder<'_>) -> Result<(), FsError> {
    let folder = Item { raw: item(dir)?, ftp: dir.is_ftp() };
    let pidls = folder.ftp.then(|| remember_folder(dir, &folder.raw));
    list(&folder, out, |name, child| {
        if let Some(ids) = &pidls {
            ids.remember(name, &child.raw);
        }
    })
}

#[cfg(test)]
#[path = "../test/namespace.rs"]
mod tests;

pub(crate) fn devices() -> Vec<flit_core::ui::Drive> {
    com_sta();
    let Ok(pc) = parse(flit_core::path::SHELL_COMPUTER) else { return Vec::new() };
    let mut out = Vec::new();
    let _ = Item { raw: pc, ftp: false }.children(&mut |child| {
        let parsing = unsafe { child.raw.GetDisplayName(SIGDN_DESKTOPABSOLUTEPARSING).ok().and_then(|p| take_pwstr(p)) };
        if parsing.is_some_and(|w| is_device(&String::from_utf16_lossy(&w))) {
            let name = child.name();
            if let Some(path) = FsPath::device(&name) {
                out.push(device_drive(path, String::from_utf8_lossy(&name).into_owned()));
            }
        }
        true
    });
    out
}

pub(crate) fn is_device(parsing: &str) -> bool {
    parsing.contains(r"\\?\")
}

pub(crate) fn device_drive(path: FsPath, label: String) -> flit_core::ui::Drive {
    flit_core::ui::Drive { path, label, total: 0, free: 0, file_system: String::new(), removable: false, unmounted: false, kind: None, remote: None }
}
