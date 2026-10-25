use crate::err;
use flit_core::input::Event;
use flit_core::path::PathKind;
use flit_core::ui::{ShellMenu, SHELL_VERBS};
use flit_core::FsPath;
use flit_platform::PlatformError;
use std::cell::RefCell;
use std::sync::Once;
use std::time::Duration;
use windows::core::{Interface, PCSTR, PCWSTR, PSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::System::Com::IDataObject;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL, VK_SHIFT};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

pub(crate) const WM_SHELL_UI: u32 = WM_APP + 0x40;

const FIRST_ID: u32 = 1;
const LAST_ID: u32 = 0x7FFF;
const CMIC_MASK_UNICODE: u32 = 0x4000;
const THIS_PC: &str = "::{20D04FE0-3AEA-1069-A2D8-08002B30309D}";
const PREWARM_DELAY: Duration = Duration::from_millis(500);

pub(crate) enum ShellUi {
    Menu(ShellMenu),
    Properties(Vec<FsPath>),
    Drag(Vec<FsPath>),
}

thread_local! {
    static QUEUE: RefCell<Vec<(isize, ShellUi)>> = const { RefCell::new(Vec::new()) };
    static ACTIVE: RefCell<Option<(Option<IContextMenu3>, Option<IContextMenu2>)>> = const { RefCell::new(None) };
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
    if p.kind() == PathKind::Computer {
        THIS_PC.encode_utf16().chain(Some(0)).collect()
    } else {
        p.to_wide(false)
    }
}

impl Pidls {
    pub(crate) fn parse(paths: &[FsPath]) -> Result<Self, PlatformError> {
        let mut out = Pidls { abs: Vec::with_capacity(paths.len()) };
        for p in paths {
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
    ACTIVE.with_borrow(|active| {
        let (v3, v2) = active.as_ref()?;
        unsafe {
            if let Some(m) = v3 {
                let mut r = LRESULT(0);
                m.HandleMenuMsg2(msg, wp, lp, Some(&mut r)).ok()?;
                return Some(r);
            }
            v2.as_ref()?.HandleMenuMsg(msg, wp, lp).ok()?;
            Some(LRESULT(if msg == WM_INITMENUPOPUP { 0 } else { 1 }))
        }
    })
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

unsafe fn query(hwnd: HWND, m: &ShellMenu) -> Result<(IContextMenu, HMENU), PlatformError> {
    let menu = build(hwnd, m)?;
    let hmenu = CreatePopupMenu().map_err(err)?;
    let mut flags = CMF_NORMAL | CMF_CANRENAME | CMF_ASYNCVERBSTATE;
    if m.extended {
        flags |= CMF_EXTENDEDVERBS;
    }
    if !m.items.is_empty() {
        flags |= CMF_ITEMMENU;
    }
    let queried = menu.QueryContextMenu(hmenu, 0, FIRST_ID, LAST_ID, flags);
    if queried.is_err() {
        let _ = DestroyMenu(hmenu);
        return Err(err(queried.into()));
    }
    Ok((menu, hmenu))
}

pub(crate) fn prewarm() {
    static STARTED: Once = Once::new();
    STARTED.call_once(|| {
        let warm = || {
            std::thread::sleep(PREWARM_DELAY);
            crate::com_sta();
            let Some(exe) = std::env::current_exe().ok().and_then(|p| FsPath::parse(p.to_str()?)) else { return };
            let Some(dir) = exe.parent() else { return };
            let menu = |dir: &FsPath, items: Vec<FsPath>| ShellMenu { dir: dir.clone(), items, x: 0.0, y: 0.0, extended: false };
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

unsafe fn show_menu(hwnd: HWND, m: &ShellMenu, send: &dyn Fn(Event)) -> Result<(), PlatformError> {
    let (menu, hmenu) = query(hwnd, m)?;
    let mut pt = POINT { x: m.x as i32, y: m.y as i32 };
    let _ = ClientToScreen(hwnd, &mut pt);
    ACTIVE.set(Some((menu.cast().ok(), menu.cast().ok())));
    let cmd = TrackPopupMenuEx(hmenu, (TPM_RETURNCMD | TPM_RIGHTBUTTON).0, pt.x, pt.y, hwnd, None).0;
    ACTIVE.set(None);
    let result = if cmd >= FIRST_ID as i32 {
        let id = (cmd as u32 - FIRST_ID) as usize;
        match verb(&menu, id).filter(|v| SHELL_VERBS.iter().any(|s| v.eq_ignore_ascii_case(s))) {
            Some(v) => {
                send(Event::ShellVerb(v));
                Ok(())
            }
            None => invoke(hwnd, &menu, id, m, pt),
        }
    } else {
        Ok(())
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
    unsafe {
        if let [one] = paths {
            let name = parse_name(one);
            return SHObjectProperties(Some(hwnd), SHOP_FILEPATH, PCWSTR(name.as_ptr()), PCWSTR::null()).ok().map_err(err);
        }
        let data: IDataObject = Pidls::parse(paths)?.ui_object(hwnd)?;
        SHMultiFileProperties(&data, 0).map_err(err)
    }
}

pub(crate) fn run(hwnd: HWND, req: ShellUi, send: &dyn Fn(Event)) {
    let result = match &req {
        ShellUi::Menu(m) => unsafe { show_menu(hwnd, m, send) },
        ShellUi::Properties(paths) => properties(hwnd, paths),
        ShellUi::Drag(paths) => crate::dragdrop::start_drag(hwnd, paths),
    };
    if let Err(e) = result {
        tracing::warn!("shell: {e}");
    }
}

#[cfg(test)]
#[path = "../test/context_menu.rs"]
mod tests;
