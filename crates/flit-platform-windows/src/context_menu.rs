use crate::err;
use flit_core::input::Event;
use flit_core::path::PathKind;
use flit_core::ui::{MenuItem, ShellMenu, SHELL_VERBS};
use flit_core::FsPath;
use flit_platform::PlatformError;
use std::cell::RefCell;
use std::ffi::CStr;
use std::sync::Once;
use std::time::{Duration, Instant};
use windows::core::{w, Interface, HSTRING, PCSTR, PCWSTR, PSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{ClientToScreen, InvalidateRect};
use windows::Win32::System::Com::IDataObject;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL, VK_SHIFT};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

pub(crate) const WM_SHELL_UI: u32 = WM_APP + 0x40;

const FIRST_ID: u32 = 1;
const LAST_ID: u32 = 0x7FFF;
const FLIT_FIRST: u32 = 0x8000;
const COVERED: [&str; 11] = ["open", "cut", "copy", "paste", "delete", "rename", "link", "properties", "copyaspath", "opennewwindow", "opennewtab"];
const WINDOWS_VERBS: [&str; 29] = [
    "edit",
    "openas",
    "print",
    "printto",
    "find",
    "explore",
    "runas",
    "runasuser",
    "opennewprocess",
    "pintohome",
    "pintohomefile",
    "unpinfromhome",
    "pintostartscreen",
    "windows.share",
    "windows.modernshare",
    "previousversions",
    "setdesktopwallpaper",
    "rotate90",
    "rotate270",
    "play",
    "enqueue",
    "mount",
    "eject",
    "format",
    "undelete",
    "empty",
    "cmd",
    "powershell",
    "wsl",
];
const CMIC_MASK_UNICODE: u32 = 0x4000;
const THIS_PC: &str = "::{20D04FE0-3AEA-1069-A2D8-08002B30309D}";
const RECYCLE_BIN: &str = "::{645FF040-5081-101B-9F08-00AA002F954E}";
const PREWARM_DELAY: Duration = Duration::from_millis(500);

pub(crate) enum ShellUi {
    Menu(ShellMenu),
    Properties(Vec<FsPath>),
    Drag(Vec<FsPath>),
    Verb(Vec<FsPath>, &'static CStr),
}

struct Active {
    menu: Option<IContextMenu>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Place {
    Duplicate,
    App,
    Windows,
}

pub(crate) struct ShellItem<'a> {
    pub verb: &'a str,
    pub usable: bool,
    pub children: Option<&'a [String]>,
}

fn named(list: &[&str], verb: &str) -> bool {
    list.iter().any(|v| v.eq_ignore_ascii_case(verb))
}

pub(crate) fn place(item: &ShellItem) -> Place {
    if named(&COVERED, item.verb) {
        return Place::Duplicate;
    }
    let app_verb = |v: &String| !v.is_empty() && !named(&WINDOWS_VERBS, v) && !named(&COVERED, v);
    match item.children {
        _ if !item.usable || named(&WINDOWS_VERBS, item.verb) => Place::Windows,
        Some(children) if item.verb.is_empty() && !children.iter().any(app_verb) => Place::Windows,
        _ => Place::App,
    }
}

thread_local! {
    static QUEUE: RefCell<Vec<(isize, ShellUi)>> = const { RefCell::new(Vec::new()) };
    static ACTIVE: RefCell<Option<Active>> = const { RefCell::new(None) };
    static TOGGLES: RefCell<Option<Toggles>> = const { RefCell::new(None) };
}

struct Toggles {
    ids: Vec<u32>,
    send: *const (dyn Fn(Event) + 'static),
}

pub(crate) fn toggle_ids(items: &[MenuItem]) -> Vec<u32> {
    fn walk(items: &[MenuItem], next: &mut u32, out: &mut Vec<u32>) {
        for item in items {
            match item {
                MenuItem::Command { stay, .. } => {
                    if *stay {
                        out.push(*next);
                    }
                    *next += 1;
                }
                MenuItem::Submenu { items, .. } => walk(items, next, out),
                MenuItem::Header(_) | MenuItem::Separator => {}
            }
        }
    }
    let (mut out, mut next) = (Vec::new(), FLIT_FIRST);
    walk(items, &mut next, &mut out);
    out
}

unsafe fn toggle_under(msg: &MSG) -> bool {
    let under = WindowFromPoint(msg.pt);
    let hmenu = HMENU(SendMessageW(under, MN_GETHMENU, None, None).0 as _);
    if hmenu.is_invalid() {
        return false;
    }
    let at = MenuItemFromPoint(None, hmenu, msg.pt);
    if at < 0 {
        return false;
    }
    let id = GetMenuItemID(hmenu, at);
    let Some(send) = TOGGLES.with_borrow(|t| t.as_ref().filter(|t| t.ids.contains(&id)).map(|t| t.send)) else { return false };
    let checked = GetMenuState(hmenu, id, MF_BYCOMMAND) & MF_CHECKED.0 != 0;
    CheckMenuItem(hmenu, id, (MF_BYCOMMAND | if checked { MF_UNCHECKED } else { MF_CHECKED }).0);
    let _ = InvalidateRect(Some(under), None, true);
    (*send)(Event::MenuCommand((id - FLIT_FIRST) as usize));
    true
}

unsafe extern "system" fn menu_filter(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code == MSGF_MENU as i32 {
        let msg = &*(lp.0 as *const MSG);
        if msg.message == WM_LBUTTONUP && toggle_under(msg) {
            return LRESULT(1);
        }
    }
    CallNextHookEx(None, code, wp, lp)
}

pub(crate) fn request(hwnd: HWND, req: ShellUi) {
    QUEUE.with_borrow_mut(|q| q.push((hwnd.0 as isize, req)));
    unsafe {
        let _ = PostMessageW(Some(hwnd), WM_SHELL_UI, WPARAM(0), LPARAM(0));
    }
}

pub(crate) fn take(hwnd: HWND) -> Vec<ShellUi> {
    QUEUE.with_borrow_mut(|q| {
        let (mine, rest) = std::mem::take(q).into_iter().partition(|(h, _)| *h == hwnd.0 as isize);
        *q = rest;
        mine.into_iter().map(|(_, r)| r).collect()
    })
}

pub(crate) struct Pidls {
    abs: Vec<*mut ITEMIDLIST>,
}

impl Drop for Pidls {
    fn drop(&mut self) {
        for &p in &self.abs {
            unsafe { ILFree(Some(p)) };
        }
    }
}

fn parse_name(p: &FsPath) -> Vec<u16> {
    match p.kind() {
        PathKind::Computer => THIS_PC.encode_utf16().chain(Some(0)).collect(),
        PathKind::RecycleBin => RECYCLE_BIN.encode_utf16().chain(Some(0)).collect(),
        _ => p.to_wide(false),
    }
}

pub(crate) fn is_recycled(p: &FsPath) -> bool {
    crate::recycle::info_file(p).is_some()
}

fn in_bin(p: &FsPath) -> bool {
    p.kind() == PathKind::RecycleBin || is_recycled(p)
}

impl Pidls {
    pub(crate) fn parse(paths: &[FsPath]) -> Result<Self, PlatformError> {
        let mut out = Pidls { abs: Vec::with_capacity(paths.len()) };
        let recycled: Vec<FsPath> = paths.iter().filter(|p| is_recycled(p)).cloned().collect();
        let mut found = match recycled.is_empty() {
            true => Vec::new(),
            false => unsafe { crate::fileops::recycled_items(&recycled)? },
        }
        .into_iter();
        for p in paths {
            if is_recycled(p) {
                let item = found.next().flatten().ok_or_else(|| PlatformError::new(-1, flit_core::tr_en!(flit_core::i18n::S::ErrorsEngineNotInBin, path = p.display())))?;
                out.abs.push(unsafe { SHGetIDListFromObject(&item) }.map_err(err)?);
                continue;
            }
            if p.kind() == PathKind::Shell {
                let item = crate::namespace::item(p).map_err(|e| PlatformError::new(e.code, format!("{}: {e}", p.display())))?;
                out.abs.push(unsafe { SHGetIDListFromObject(&item) }.map_err(err)?);
                continue;
            }
            let mut pidl = std::ptr::null_mut();
            let name = parse_name(p);
            unsafe { SHParseDisplayName(PCWSTR(name.as_ptr()), None, &mut pidl, 0, None) }.map_err(|e| PlatformError::new(e.code().0 as i64, format!("{}: {}", p.display(), e.message())))?;
            out.abs.push(pidl);
        }
        if out.abs.is_empty() {
            return Err(PlatformError::new(-1, "nothing selected"));
        }
        Ok(out)
    }

    fn first(&self) -> *const ITEMIDLIST {
        self.abs[0]
    }

    pub(crate) fn ui_object<T: Interface>(&self, hwnd: HWND) -> Result<T, PlatformError> {
        unsafe {
            let parent: IShellFolder = SHBindToParent(self.first(), None).map_err(err)?;
            let parent_id = ILClone(self.first());
            let _ = ILRemoveLastID(Some(parent_id));
            let children: Vec<*const ITEMIDLIST> =
                self.abs.iter().filter(|&&p| ILIsParent(parent_id, p, true).as_bool()).map(|&p| ILFindLastID(p) as *const ITEMIDLIST).collect();
            ILFree(Some(parent_id));
            parent.GetUIObjectOf(hwnd, &children, None).map_err(err)
        }
    }

    pub(crate) fn folder(&self) -> Result<IShellFolder, PlatformError> {
        unsafe {
            let desktop = SHGetDesktopFolder().map_err(err)?;
            if ILGetSize(Some(self.first())) <= 2 {
                return Ok(desktop);
            }
            desktop.BindToObject(self.first(), None).map_err(err)
        }
    }
}


pub(crate) fn forward(msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
    if !matches!(msg, WM_INITMENUPOPUP | WM_DRAWITEM | WM_MEASUREITEM | WM_MENUCHAR) {
        return None;
    }
    let menu = ACTIVE.with_borrow(|a| a.as_ref()?.menu.clone())?;
    unsafe {
        if let Ok(m) = menu.cast::<IContextMenu3>() {
            let mut r = LRESULT(0);
            m.HandleMenuMsg2(msg, wp, lp, Some(&mut r)).ok()?;
            return Some(r);
        }
        menu.cast::<IContextMenu2>().ok()?.HandleMenuMsg(msg, wp, lp).ok()?;
        Some(LRESULT(if msg == WM_INITMENUPOPUP { 0 } else { 1 }))
    }
}

fn verb(menu: &IContextMenu, id: usize) -> Option<String> {
    let mut buf = [0u16; 128];
    unsafe { menu.GetCommandString(id, GCS_VERBW, None, PSTR(buf.as_mut_ptr() as *mut u8), buf.len() as u32) }.ok()?;
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..n]))
}

fn build(hwnd: HWND, m: &ShellMenu) -> Result<IContextMenu, PlatformError> {
    if m.items.is_empty() {
        let folder = Pidls::parse(std::slice::from_ref(&m.dir))?.folder()?;
        unsafe { folder.CreateViewObject(hwnd) }.map_err(err)
    } else {
        Pidls::parse(&m.items)?.ui_object(hwnd)
    }
}

fn key_down(vk: i32) -> bool {
    unsafe { GetKeyState(vk) < 0 }
}

unsafe fn query_into(hwnd: HWND, m: &ShellMenu, hmenu: HMENU, at: u32) -> Result<IContextMenu, PlatformError> {
    let menu = build(hwnd, m)?;
    let mut flags = CMF_NORMAL | CMF_CANRENAME | CMF_ASYNCVERBSTATE;
    if m.extended {
        flags |= CMF_EXTENDEDVERBS;
    }
    if !m.items.is_empty() {
        flags |= CMF_ITEMMENU;
    }
    let queried = menu.QueryContextMenu(hmenu, at, FIRST_ID, LAST_ID, flags);
    crate::crash::rearm();
    queried.ok().map_err(err)?;
    Ok(menu)
}

unsafe fn query(hwnd: HWND, m: &ShellMenu) -> Result<(IContextMenu, HMENU), PlatformError> {
    let hmenu = CreatePopupMenu().map_err(err)?;
    match query_into(hwnd, m, hmenu, 0) {
        Ok(menu) => Ok((menu, hmenu)),
        Err(e) => {
            let _ = DestroyMenu(hmenu);
            Err(e)
        }
    }
}

pub(crate) fn prewarm() {
    static STARTED: Once = Once::new();
    STARTED.call_once(|| {
        let warm = || {
            std::thread::sleep(PREWARM_DELAY);
            crate::com_sta();
            let Some(exe) = std::env::current_exe().ok().and_then(|p| FsPath::parse(p.to_str()?)) else { return };
            let Some(dir) = exe.parent() else { return };
            let menu = |dir: &FsPath, items: Vec<FsPath>| ShellMenu { dir: dir.clone(), items, x: 0.0, y: 0.0, extended: false, menu: Vec::new(), inline: false, shell: true };
            for m in [menu(&dir, vec![exe.clone()]), menu(&dir, Vec::new()), menu(&dir, vec![dir.clone()])] {
                if let Ok((_, hmenu)) = unsafe { query(HWND::default(), &m) } {
                    let _ = unsafe { DestroyMenu(hmenu) };
                }
            }
        };
        if let Err(e) = std::thread::Builder::new().name("flit-shell-warm".into()).spawn(warm) {
            tracing::warn!("shell warm-up: {e}");
        }
    });
}

fn menu_text(label: &str, keys: &str) -> HSTRING {
    let label = label.replace('&', "&&");
    HSTRING::from(if keys.is_empty() { label } else { format!("{label}\t{keys}") })
}

unsafe fn append(hmenu: HMENU, items: &[MenuItem], next: &mut u32) -> windows::core::Result<()> {
    for item in items {
        match item {
            MenuItem::Command { label, keys, checked, disabled, .. } => {
                let flags = MF_STRING | if *checked { MF_CHECKED } else { MF_STRING } | if *disabled { MF_GRAYED } else { MF_STRING };
                AppendMenuW(hmenu, flags, *next as usize, &menu_text(label, keys))?;
                *next += 1;
            }
            MenuItem::Submenu { label, items } => {
                let sub = CreatePopupMenu()?;
                append(sub, items, next)?;
                AppendMenuW(hmenu, MF_POPUP, sub.0 as usize, &menu_text(label, ""))?;
            }
            MenuItem::Header(label) => AppendMenuW(hmenu, MF_STRING | MF_GRAYED, 0, &menu_text(label, ""))?,
            MenuItem::Separator => AppendMenuW(hmenu, MF_SEPARATOR, 0, None)?,
        }
    }
    Ok(())
}

unsafe fn is_separator(hmenu: HMENU, i: i32) -> bool {
    let mut info = MENUITEMINFOW { cbSize: size_of::<MENUITEMINFOW>() as u32, fMask: MIIM_FTYPE, ..Default::default() };
    GetMenuItemInfoW(hmenu, i as u32, true, &mut info).is_ok() && info.fType.0 & MFT_SEPARATOR.0 != 0
}

unsafe fn id_verb(menu: &IContextMenu, id: u32) -> String {
    (FIRST_ID..=LAST_ID).contains(&id).then(|| verb(menu, (id - FIRST_ID) as usize)).flatten().unwrap_or_default()
}

unsafe fn verbs_under(menu: &IContextMenu, hmenu: HMENU, out: &mut Vec<String>) {
    for i in 0..GetMenuItemCount(Some(hmenu)) {
        let mut info = MENUITEMINFOW { cbSize: size_of::<MENUITEMINFOW>() as u32, fMask: MIIM_ID | MIIM_SUBMENU, ..Default::default() };
        if GetMenuItemInfoW(hmenu, i as u32, true, &mut info).is_err() {
            continue;
        }
        out.push(id_verb(menu, info.wID));
        if !info.hSubMenu.is_invalid() {
            verbs_under(menu, info.hSubMenu, out);
        }
    }
}

unsafe fn place_at(menu: &IContextMenu, hmenu: HMENU, i: i32) -> Option<Place> {
    let mut info = MENUITEMINFOW { cbSize: size_of::<MENUITEMINFOW>() as u32, fMask: MIIM_ID | MIIM_SUBMENU | MIIM_STATE | MIIM_FTYPE, ..Default::default() };
    GetMenuItemInfoW(hmenu, i as u32, true, &mut info).ok()?;
    if info.fType.0 & MFT_SEPARATOR.0 != 0 {
        return None;
    }
    let sub = !info.hSubMenu.is_invalid();
    let mut children = Vec::new();
    if sub {
        verbs_under(menu, info.hSubMenu, &mut children);
    }
    let verb = id_verb(menu, info.wID);
    let usable = info.fState.0 & MFS_DISABLED.0 == 0 && (sub || (FIRST_ID..=LAST_ID).contains(&info.wID));
    Some(place(&ShellItem { verb: &verb, usable, children: sub.then_some(children.as_slice()) }))
}

unsafe fn move_item(from: HMENU, i: i32, to: HMENU) -> windows::core::Result<()> {
    let mut kind = MENUITEMINFOW { cbSize: size_of::<MENUITEMINFOW>() as u32, fMask: MIIM_FTYPE, ..Default::default() };
    GetMenuItemInfoW(from, i as u32, true, &mut kind)?;
    let text = kind.fType.0 & (MFT_OWNERDRAW.0 | MFT_BITMAP.0) == 0;
    let mut buf = [0u16; 512];
    let common = MIIM_STATE | MIIM_ID | MIIM_SUBMENU | MIIM_CHECKMARKS | MIIM_DATA;
    let mut info = MENUITEMINFOW {
        cbSize: size_of::<MENUITEMINFOW>() as u32,
        fMask: common | if text { MIIM_FTYPE | MIIM_STRING | MIIM_BITMAP } else { MIIM_TYPE },
        dwTypeData: windows::core::PWSTR(buf.as_mut_ptr()),
        cch: buf.len() as u32 - 1,
        ..Default::default()
    };
    GetMenuItemInfoW(from, i as u32, true, &mut info)?;
    RemoveMenu(from, i as u32, MF_BYPOSITION)?;
    InsertMenuItemW(to, GetMenuItemCount(Some(to)).max(0) as u32, true, &info)
}

unsafe fn lift_apps(menu: &IContextMenu, more: HMENU, hmenu: HMENU) -> usize {
    let mut lifted = 0;
    let mut i = 0;
    while i < GetMenuItemCount(Some(more)) {
        if place_at(menu, more, i) == Some(Place::App) && move_item(more, i, hmenu).is_ok() {
            lifted += 1;
            continue;
        }
        i += 1;
    }
    tidy_separators(more);
    lifted
}

unsafe fn drop_covered(menu: &IContextMenu, hmenu: HMENU, from: i32) {
    for i in (from..GetMenuItemCount(Some(hmenu))).rev() {
        if place_at(menu, hmenu, i) == Some(Place::Duplicate) {
            let _ = DeleteMenu(hmenu, i as u32, MF_BYPOSITION);
        }
    }
    tidy_separators(hmenu);
}

unsafe fn tidy_separators(hmenu: HMENU) {
    let mut prev = true;
    let mut i = 0;
    while i < GetMenuItemCount(Some(hmenu)) {
        let sep = is_separator(hmenu, i);
        if sep && prev {
            let _ = DeleteMenu(hmenu, i as u32, MF_BYPOSITION);
            continue;
        }
        prev = sep;
        i += 1;
    }
    if prev && i > 0 {
        let _ = DeleteMenu(hmenu, i as u32 - 1, MF_BYPOSITION);
    }
}

unsafe fn fill(hwnd: HWND, m: &ShellMenu, hmenu: HMENU) -> Result<Active, PlatformError> {
    let mut active = Active { menu: None };
    let mut next = FLIT_FIRST;
    append(hmenu, &m.menu, &mut next).map_err(err)?;
    if !m.shell {
        return Ok(active);
    }
    let ours = GetMenuItemCount(Some(hmenu));
    if ours > 0 {
        AppendMenuW(hmenu, MF_SEPARATOR, 0, None).map_err(err)?;
    }
    if m.inline {
        match query_into(hwnd, m, hmenu, ours as u32 + 1) {
            Ok(menu) => {
                drop_covered(&menu, hmenu, ours);
                active.menu = Some(menu);
            }
            Err(e) => tracing::warn!("shell menu: {e}"),
        }
        return Ok(active);
    }
    let more = CreatePopupMenu().map_err(err)?;
    match query_into(hwnd, m, more, 0) {
        Ok(menu) => {
            if lift_apps(&menu, more, hmenu) > 0 {
                AppendMenuW(hmenu, MF_SEPARATOR, 0, None).map_err(err)?;
            }
            active.menu = Some(menu);
        }
        Err(e) => tracing::warn!("shell menu: {e}"),
    }
    if GetMenuItemCount(Some(more)) <= 0 {
        AppendMenuW(more, MF_GRAYED, 0, w!("No more options")).map_err(err)?;
    }
    AppendMenuW(hmenu, MF_POPUP, more.0 as usize, w!("Show more options")).map_err(err)?;
    Ok(active)
}

unsafe fn show_menu(hwnd: HWND, m: &ShellMenu, send: &dyn Fn(Event)) -> Result<(), PlatformError> {
    let started = Instant::now();
    let hmenu = CreatePopupMenu().map_err(err)?;
    let active = match fill(hwnd, m, hmenu) {
        Ok(a) => a,
        Err(e) => {
            let _ = DestroyMenu(hmenu);
            return Err(e);
        }
    };
    ACTIVE.set(Some(active));
    let mut pt = POINT { x: m.x as i32, y: m.y as i32 };
    let _ = ClientToScreen(hwnd, &mut pt);
    tracing::info!("context menu ready in {:.1} ms, shell items {}", started.elapsed().as_secs_f64() * 1e3, if m.inline { "inline" } else { "in a submenu" });
    let ids = toggle_ids(&m.menu);
    let hook = match ids.is_empty() {
        true => None,
        false => {
            let send: *const (dyn Fn(Event) + '_) = send;
            TOGGLES.set(Some(Toggles { ids, send: std::mem::transmute::<*const (dyn Fn(Event) + '_), *const (dyn Fn(Event) + 'static)>(send) }));
            SetWindowsHookExW(WH_MSGFILTER, Some(menu_filter), None, windows::Win32::System::Threading::GetCurrentThreadId()).ok()
        }
    };
    let cmd = TrackPopupMenuEx(hmenu, (TPM_RETURNCMD | TPM_RIGHTBUTTON).0, pt.x, pt.y, hwnd, None).0 as u32;
    if let Some(h) = hook {
        let _ = UnhookWindowsHookEx(h);
    }
    TOGGLES.set(None);
    let menu = ACTIVE.take().and_then(|a| a.menu);
    let result = match (cmd, menu) {
        (FLIT_FIRST.., _) => {
            send(Event::MenuCommand((cmd - FLIT_FIRST) as usize));
            Ok(())
        }
        (FIRST_ID..=LAST_ID, Some(menu)) => {
            let id = (cmd - FIRST_ID) as usize;
            match verb(&menu, id).filter(|v| SHELL_VERBS.iter().any(|s| v.eq_ignore_ascii_case(s))) {
                Some(v) => {
                    send(Event::ShellVerb(v));
                    Ok(())
                }
                None => invoke(hwnd, &menu, id, m, pt),
            }
        }
        _ => Ok(()),
    };
    let _ = DestroyMenu(hmenu);
    result
}

unsafe fn invoke(hwnd: HWND, menu: &IContextMenu, id: usize, m: &ShellMenu, pt: POINT) -> Result<(), PlatformError> {
    let dir = (m.dir.kind() != PathKind::Computer).then(|| m.dir.to_wide(false));
    let mut mask = CMIC_MASK_UNICODE | CMIC_MASK_PTINVOKE;
    if key_down(VK_SHIFT.0 as i32) {
        mask |= CMIC_MASK_SHIFT_DOWN;
    }
    if key_down(VK_CONTROL.0 as i32) {
        mask |= CMIC_MASK_CONTROL_DOWN;
    }
    let info = CMINVOKECOMMANDINFOEX {
        cbSize: size_of::<CMINVOKECOMMANDINFOEX>() as u32,
        fMask: mask,
        hwnd,
        lpVerb: PCSTR(id as *const u8),
        lpVerbW: PCWSTR(id as *const u16),
        lpDirectoryW: dir.as_ref().map_or(PCWSTR::null(), |d| PCWSTR(d.as_ptr())),
        nShow: SW_SHOWNORMAL.0,
        ptInvoke: pt,
        ..Default::default()
    };
    menu.InvokeCommand(&info as *const CMINVOKECOMMANDINFOEX as *const CMINVOKECOMMANDINFO).map_err(err)
}

fn properties(hwnd: HWND, paths: &[FsPath]) -> Result<(), PlatformError> {
    if paths.iter().any(in_bin) {
        return invoke_verb(hwnd, paths, c"properties");
    }
    unsafe {
        if let [one] = paths {
            let name = parse_name(one);
            return SHObjectProperties(Some(hwnd), SHOP_FILEPATH, PCWSTR(name.as_ptr()), PCWSTR::null()).ok().map_err(err);
        }
        let data: IDataObject = Pidls::parse(paths)?.ui_object(hwnd)?;
        SHMultiFileProperties(&data, 0).map_err(err)
    }
}

pub(crate) fn invoke_verb(hwnd: HWND, paths: &[FsPath], verb: &CStr) -> Result<(), PlatformError> {
    let menu: IContextMenu = Pidls::parse(paths)?.ui_object(hwnd)?;
    unsafe {
        let hmenu = CreatePopupMenu().map_err(err)?;
        let _ = menu.QueryContextMenu(hmenu, 0, FIRST_ID, LAST_ID, CMF_NORMAL);
        let info = CMINVOKECOMMANDINFO { cbSize: size_of::<CMINVOKECOMMANDINFO>() as u32, hwnd, lpVerb: PCSTR(verb.as_ptr() as *const u8), nShow: SW_SHOWNORMAL.0, ..Default::default() };
        let invoked = menu.InvokeCommand(&info).map_err(err);
        let _ = DestroyMenu(hmenu);
        invoked
    }
}

pub(crate) fn run(hwnd: HWND, req: ShellUi, send: &dyn Fn(Event)) {
    let result = match &req {
        ShellUi::Menu(m) => unsafe { show_menu(hwnd, m, send) },
        ShellUi::Properties(paths) => properties(hwnd, paths),
        ShellUi::Drag(paths) => crate::dragdrop::start_drag(hwnd, paths),
        ShellUi::Verb(paths, verb) => invoke_verb(hwnd, paths, verb),
    };
    if let Err(e) = result {
        tracing::warn!("shell: {e}");
    }
}

#[cfg(test)]
#[path = "../test/context_menu.rs"]
mod tests;
