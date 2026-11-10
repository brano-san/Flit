use crate::err;
use flit_core::input::Event;
use flit_core::path::PathKind;
use flit_core::ui::{MenuImage, MenuItem, ShellMenu, SHELL_VERBS};
use flit_core::FsPath;
use flit_platform::PlatformError;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::ffi::CStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use windows::core::{w, Interface, PCSTR, PCWSTR, PSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{ClientToScreen, GetObjectW, BITMAP, HGDIOBJ};
use windows::Win32::System::Com::IDataObject;
use windows::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL, THREAD_PRIORITY_NORMAL};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL, VK_SHIFT};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

pub(crate) const WM_SHELL_UI: u32 = WM_APP + 0x40;

const FIRST_ID: u32 = 1;
const LAST_ID: u32 = 0x7FFF;
const WORKER_CLASS: PCWSTR = w!("FlitShellMenuWorker");
const PREP_TIMER: usize = 2;
const STALE_TIMER: usize = 3;
const PREFETCH: usize = 1;
const PREP_REST: Duration = Duration::from_millis(90);
const PREP_STALE: Duration = Duration::from_secs(20);
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
const LAST_MENU_BITMAP: isize = 11;
const MAX_IMAGES: usize = 256;
const THIS_PC: &str = "::{20D04FE0-3AEA-1069-A2D8-08002B30309D}";
const RECYCLE_BIN: &str = "::{645FF040-5081-101B-9F08-00AA002F954E}";

pub(crate) enum ShellUi {
    Menu(ShellMenu),
    Properties(Vec<FsPath>),
    Drag(Vec<FsPath>),
    Verb(Vec<FsPath>, &'static CStr),
    Done(Option<u64>, Event),
}

enum Job {
    Menu { owner: isize, gen: u64, menu: ShellMenu, pt: POINT },
    Adopt { owner: isize, prep: u64, menu: ShellMenu, pt: POINT },
    Pick { owner: isize, cmd: Option<u32>, shift: bool, ctrl: bool },
    Warm(Vec<ShellMenu>),
}

struct Prepared {
    gen: u64,
    menu: ShellMenu,
    hmenu: HMENU,
    active: Active,
}

struct Ready {
    gen: u64,
    menu: ShellMenu,
    items: Vec<MenuItem>,
    ids: Vec<u32>,
    at: Instant,
}

struct Shown {
    hmenu: HMENU,
    menu: ShellMenu,
    pt: POINT,
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
    let open_with = |verb: &str| verb.eq_ignore_ascii_case("openas") || verb.eq_ignore_ascii_case("openwith");
    if item.usable && (open_with(item.verb) || item.children.is_some_and(|children| children.iter().any(|v| open_with(v)))) {
        return Place::App;
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
    static SHOWN: RefCell<Option<Shown>> = const { RefCell::new(None) };
    static PREPARED: RefCell<Option<Prepared>> = const { RefCell::new(None) };
    static BUSY: Cell<bool> = const { Cell::new(false) };
    static IMAGES: RefCell<HashMap<(String, i32, i32), Option<MenuImage>>> = RefCell::new(HashMap::new());
}

static CURRENT: AtomicU64 = AtomicU64::new(0);
static JOBS: Mutex<VecDeque<Job>> = Mutex::new(VecDeque::new());
static DONE: Mutex<Vec<(isize, Option<u64>, Event)>> = Mutex::new(Vec::new());
static WORKER: OnceLock<isize> = OnceLock::new();
static PREP_GEN: AtomicU64 = AtomicU64::new(0);
static WANT: Mutex<Option<(u64, isize, ShellMenu)>> = Mutex::new(None);
static READY: Mutex<Option<Ready>> = Mutex::new(None);

pub(crate) fn request(hwnd: HWND, req: ShellUi) {
    QUEUE.with_borrow_mut(|q| q.push((hwnd.0 as isize, req)));
    unsafe {
        let _ = PostMessageW(Some(hwnd), WM_SHELL_UI, WPARAM(0), LPARAM(0));
    }
}

pub(crate) fn take(hwnd: HWND) -> Vec<ShellUi> {
    let key = hwnd.0 as isize;
    let mut out: Vec<ShellUi> = QUEUE.with_borrow_mut(|q| {
        let (mine, rest) = std::mem::take(q).into_iter().partition(|(h, _)| *h == key);
        *q = rest;
        mine.into_iter().map(|(_, r)| r).collect()
    });
    let mut done = DONE.lock().unwrap_or_else(|e| e.into_inner());
    let (mine, rest): (Vec<_>, Vec<_>) = done.drain(..).partition(|(h, _, _)| *h == key);
    *done = rest;
    out.extend(mine.into_iter().map(|(_, gen, ev)| ShellUi::Done(gen, ev)));
    out
}

fn deliver(owner: isize, gen: Option<u64>, ev: Event) {
    DONE.lock().unwrap_or_else(|e| e.into_inner()).push((owner, gen, ev));
    unsafe {
        let _ = PostMessageW(Some(HWND(owner as _)), WM_SHELL_UI, WPARAM(0), LPARAM(0));
    }
}

fn worker() -> isize {
    *WORKER.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new().name("flit-shell-menu".into()).spawn(move || unsafe {
            crate::com_sta();
            crate::prevhost::register(WORKER_CLASS, Some(worker_proc));
            let hwnd = CreateWindowExW(WINDOW_EX_STYLE(0), WORKER_CLASS, PCWSTR::null(), WINDOW_STYLE(0), 0, 0, 0, 0, Some(HWND_MESSAGE), None, Some(crate::prevhost::hinst()), None);
            let _ = tx.send(hwnd.as_ref().map_or(0, |h| h.0 as isize));
            if hwnd.is_err() {
                return;
            }
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        });
        if let Err(e) = &spawned {
            tracing::warn!("shell menu thread: {e}");
        }
        spawned.ok().and_then(|_| rx.recv().ok()).unwrap_or(0)
    })
}

fn send_job(job: Job) {
    let hwnd = worker();
    if hwnd == 0 {
        return;
    }
    JOBS.lock().unwrap_or_else(|e| e.into_inner()).push_back(job);
    unsafe {
        let _ = PostMessageW(Some(HWND(hwnd as _)), WM_SHELL_UI, WPARAM(0), LPARAM(0));
    }
}

fn locked<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

pub(crate) fn prefetch(owner: HWND, menu: Option<ShellMenu>) {
    let gen = PREP_GEN.fetch_add(1, Ordering::SeqCst) + 1;
    {
        let mut ready = locked(&READY);
        if ready.as_ref().is_some_and(|r| !menu.as_ref().is_some_and(|m| m.same_target(&r.menu))) {
            *ready = None;
        }
    }
    *locked(&WANT) = menu.map(|m| (gen, owner.0 as isize, m));
    let hwnd = worker();
    if hwnd != 0 {
        unsafe {
            let _ = PostMessageW(Some(HWND(hwnd as _)), WM_SHELL_UI, WPARAM(PREFETCH), LPARAM(0));
        }
    }
}

pub(crate) fn warm_up(menus: Vec<ShellMenu>) {
    send_job(Job::Warm(menus));
}

fn take_ready(m: &ShellMenu) -> Option<Ready> {
    let mut ready = locked(&READY);
    match ready.take() {
        Some(r) if r.menu.same_target(m) && r.at.elapsed() < PREP_STALE => Some(r),
        _ => None,
    }
}

unsafe extern "system" fn worker_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_SHELL_UI if wp.0 == PREFETCH => {
            SetTimer(Some(hwnd), PREP_TIMER, PREP_REST.as_millis() as u32, None);
        }
        WM_SHELL_UI => serve(),
        WM_TIMER if wp.0 == PREP_TIMER => {
            if BUSY.replace(true) {
                SetTimer(Some(hwnd), PREP_TIMER, PREP_REST.as_millis() as u32, None);
            } else {
                let _ = KillTimer(Some(hwnd), PREP_TIMER);
                prepare(hwnd);
                BUSY.set(false);
                serve();
            }
        }
        WM_TIMER if wp.0 == STALE_TIMER => {
            let _ = KillTimer(Some(hwnd), STALE_TIMER);
            drop_prepared();
        }
        _ => return DefWindowProcW(hwnd, msg, wp, lp),
    }
    LRESULT(0)
}

fn drop_prepared() {
    if let Some(p) = PREPARED.take() {
        let _ = unsafe { DestroyMenu(p.hmenu) };
        let mut ready = locked(&READY);
        if ready.as_ref().is_some_and(|r| r.gen == p.gen) {
            *ready = None;
        }
    }
}

unsafe fn prepare(worker: HWND) {
    let want = locked(&WANT).clone();
    let current = |m: &ShellMenu| PREPARED.with_borrow(|p| p.as_ref().is_some_and(|p| p.menu.same_target(m)));
    if want.as_ref().is_some_and(|(_, _, m)| current(m)) {
        return;
    }
    drop_prepared();
    let Some((gen, owner, menu)) = want else { return };
    let started = Instant::now();
    let Ok(hmenu) = CreatePopupMenu() else { return };
    let active = match fill(HWND(owner as _), &menu, hmenu) {
        Ok(a) => a,
        Err(e) => {
            tracing::warn!("shell menu prefetch: {e}");
            let _ = DestroyMenu(hmenu);
            return;
        }
    };
    let (items, ids) = shell_items(active.menu.as_ref(), hmenu);
    tracing::info!("context menu prepared in {:.1} ms, {} commands", started.elapsed().as_secs_f64() * 1e3, ids.len());
    PREPARED.set(Some(Prepared { gen, menu: menu.clone(), hmenu, active }));
    if locked(&WANT).as_ref().is_none_or(|w| w.0 != gen) {
        drop_prepared();
        return;
    }
    *locked(&READY) = Some(Ready { gen, menu, items, ids, at: Instant::now() });
    SetTimer(Some(worker), STALE_TIMER, PREP_STALE.as_millis() as u32, None);
}

unsafe fn adopt(owner: isize, prep: u64, m: &ShellMenu, pt: POINT) -> Result<(), PlatformError> {
    release();
    let (hmenu, active) = match PREPARED.take() {
        Some(p) if p.gen == prep => (p.hmenu, p.active),
        other => {
            if let Some(p) = other {
                let _ = DestroyMenu(p.hmenu);
            }
            let hmenu = CreatePopupMenu().map_err(err)?;
            match fill(HWND(owner as _), m, hmenu) {
                Ok(a) => (hmenu, a),
                Err(e) => {
                    let _ = DestroyMenu(hmenu);
                    return Err(e);
                }
            }
        }
    };
    ACTIVE.set(Some(active));
    SHOWN.set(Some(Shown { hmenu, menu: m.clone(), pt }));
    Ok(())
}

fn serve() {
    if BUSY.replace(true) {
        return;
    }
    loop {
        let Some(job) = JOBS.lock().unwrap_or_else(|e| e.into_inner()).pop_front() else { break };
        match job {
            Job::Menu { owner, gen, menu, pt } => {
                if let Err(e) = unsafe { show_menu(owner, gen, &menu, pt) } {
                    tracing::warn!("shell menu: {e}");
                    deliver(owner, Some(gen), Event::ShellItems { items: Vec::new(), shell: Vec::new() });
                }
            }
            Job::Adopt { owner, prep, menu, pt } => {
                if let Err(e) = unsafe { adopt(owner, prep, &menu, pt) } {
                    tracing::warn!("shell menu: {e}");
                }
            }
            Job::Pick { owner, cmd, shift, ctrl } => {
                if let Err(e) = unsafe { pick(owner, cmd, shift, ctrl) } {
                    tracing::warn!("shell: {e}");
                }
            }
            Job::Warm(menus) => warm(&menus),
        }
    }
    BUSY.set(false);
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


fn forward(menu: &IContextMenu, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
    if !matches!(msg, WM_INITMENUPOPUP | WM_DRAWITEM | WM_MEASUREITEM | WM_MENUCHAR) {
        return None;
    }
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

#[cfg(test)]
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
    worker();
}

fn warm(menus: &[ShellMenu]) {
    let thread = unsafe { GetCurrentThread() };
    let _ = unsafe { SetThreadPriority(thread, THREAD_PRIORITY_BELOW_NORMAL) };
    for m in menus {
        if !locked(&JOBS).is_empty() {
            break;
        }
        let started = Instant::now();
        let Ok(hmenu) = (unsafe { CreatePopupMenu() }) else { break };
        if let Ok(active) = unsafe { fill(HWND::default(), m, hmenu) } {
            let (_, ids) = unsafe { shell_items(active.menu.as_ref(), hmenu) };
            tracing::info!("shell menu warmed in {:.1} ms, {} commands", started.elapsed().as_secs_f64() * 1e3, ids.len());
        }
        let _ = unsafe { DestroyMenu(hmenu) };
    }
    let _ = unsafe { SetThreadPriority(thread, THREAD_PRIORITY_NORMAL) };
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
    if m.inline {
        match query_into(hwnd, m, hmenu, 0) {
            Ok(menu) => {
                drop_covered(&menu, hmenu, 0);
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

pub(crate) fn plain_label(text: &str) -> (String, String) {
    let (label, keys) = text.split_once('\t').unwrap_or((text, ""));
    let mut out = String::with_capacity(label.len());
    let mut chars = label.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '&' if chars.peek() == Some(&'&') => {
                out.push('&');
                chars.next();
            }
            '&' => {}
            c => out.push(c),
        }
    }
    (out.trim().to_owned(), keys.trim().to_owned())
}

unsafe fn item_text(hmenu: HMENU, i: i32) -> String {
    let mut buf = [0u16; 512];
    let mut info = MENUITEMINFOW { cbSize: size_of::<MENUITEMINFOW>() as u32, fMask: MIIM_STRING, dwTypeData: windows::core::PWSTR(buf.as_mut_ptr()), cch: buf.len() as u32 - 1, ..Default::default() };
    if GetMenuItemInfoW(hmenu, i as u32, true, &mut info).is_err() {
        return String::new();
    }
    String::from_utf16_lossy(&buf[..info.cch as usize])
}

pub(crate) unsafe fn item_image(hmenu: HMENU, i: i32, label: &str) -> Option<MenuImage> {
    let mut info = MENUITEMINFOW { cbSize: size_of::<MENUITEMINFOW>() as u32, fMask: MIIM_BITMAP | MIIM_CHECKMARKS, ..Default::default() };
    GetMenuItemInfoW(hmenu, i as u32, true, &mut info).ok()?;
    let hbm = [info.hbmpItem, info.hbmpUnchecked].into_iter().find(|h| !(-1..=LAST_MENU_BITMAP).contains(&(h.0 as isize)))?;
    let mut bm = BITMAP::default();
    if GetObjectW(HGDIOBJ(hbm.0), size_of::<BITMAP>() as i32, Some(&mut bm as *mut BITMAP as *mut _)) == 0 {
        return None;
    }
    let key = (label.to_owned(), bm.bmWidth, bm.bmHeight);
    if let Some(hit) = IMAGES.with_borrow(|c| c.get(&key).cloned()) {
        return hit;
    }
    let image = crate::shell::menu_bitmap(hbm).map(MenuImage::pixels);
    IMAGES.with_borrow_mut(|c| {
        if c.len() >= MAX_IMAGES {
            c.clear();
        }
        c.insert(key, image.clone());
    });
    image
}

pub(crate) unsafe fn convert(menu: Option<&IContextMenu>, hmenu: HMENU, from: i32, depth: usize, items: &mut Vec<MenuItem>, ids: &mut Vec<u32>) {
    for i in from.max(0)..GetMenuItemCount(Some(hmenu)) {
        let mut info = MENUITEMINFOW { cbSize: size_of::<MENUITEMINFOW>() as u32, fMask: MIIM_ID | MIIM_SUBMENU | MIIM_STATE | MIIM_FTYPE, ..Default::default() };
        if GetMenuItemInfoW(hmenu, i as u32, true, &mut info).is_err() {
            continue;
        }
        if info.fType.0 & MFT_SEPARATOR.0 != 0 {
            if !matches!(items.last(), None | Some(MenuItem::Separator)) {
                items.push(MenuItem::Separator);
            }
            continue;
        }
        let (label, keys) = plain_label(&item_text(hmenu, i));
        if label.is_empty() {
            continue;
        }
        if !info.hSubMenu.is_invalid() {
            if depth > 4 {
                continue;
            }
            if let Some(m) = menu {
                let _ = forward(m, WM_INITMENUPOPUP, WPARAM(info.hSubMenu.0 as usize), LPARAM(i as isize));
            }
            let mut sub = Vec::new();
            convert(menu, info.hSubMenu, 0, depth + 1, &mut sub, ids);
            if matches!(sub.last(), Some(MenuItem::Separator)) {
                sub.pop();
            }
            if !sub.is_empty() {
                items.push(MenuItem::Submenu { label, items: sub });
            }
            continue;
        }
        let disabled = info.fState.0 & MFS_DISABLED.0 != 0;
        let image = item_image(hmenu, i, &label);
        items.push(MenuItem::Command { label, keys, checked: info.fState.0 & MFS_CHECKED.0 != 0, stay: false, mark: flit_core::ui::Mark::None, icon: "", image, disabled });
        ids.push(info.wID);
    }
    if matches!(items.last(), Some(MenuItem::Separator)) {
        items.pop();
    }
}

pub(crate) unsafe fn shell_items(menu: Option<&IContextMenu>, hmenu: HMENU) -> (Vec<MenuItem>, Vec<u32>) {
    let (mut items, mut ids) = (Vec::new(), Vec::new());
    convert(menu, hmenu, 0, 0, &mut items, &mut ids);
    (items, ids)
}

unsafe fn show_menu(owner: isize, gen: u64, m: &ShellMenu, pt: POINT) -> Result<(), PlatformError> {
    release();
    if CURRENT.load(Ordering::SeqCst) != gen {
        return Ok(());
    }
    let started = Instant::now();
    let hmenu = CreatePopupMenu().map_err(err)?;
    let active = match fill(HWND(owner as _), m, hmenu) {
        Ok(a) => a,
        Err(e) => {
            let _ = DestroyMenu(hmenu);
            return Err(e);
        }
    };
    let (items, ids) = shell_items(active.menu.as_ref(), hmenu);
    ACTIVE.set(Some(active));
    tracing::info!("context menu shell items ready in {:.1} ms, {} commands, {}", started.elapsed().as_secs_f64() * 1e3, ids.len(), if m.inline { "inline" } else { "in a submenu" });
    SHOWN.set(Some(Shown { hmenu, menu: m.clone(), pt }));
    if CURRENT.load(Ordering::SeqCst) != gen {
        release();
        return Ok(());
    }
    deliver(owner, Some(gen), Event::ShellItems { items, shell: ids });
    Ok(())
}

fn release() {
    if let Some(s) = SHOWN.take() {
        let _ = unsafe { DestroyMenu(s.hmenu) };
    }
    ACTIVE.set(None);
}

unsafe fn pick(owner: isize, cmd: Option<u32>, shift: bool, ctrl: bool) -> Result<(), PlatformError> {
    let Some(shown) = SHOWN.take() else { return Ok(()) };
    let menu = ACTIVE.take().and_then(|a| a.menu);
    let result = match (cmd, menu) {
        (Some(cmd @ FIRST_ID..=LAST_ID), Some(menu)) => {
            let id = (cmd - FIRST_ID) as usize;
            match verb(&menu, id).filter(|v| SHELL_VERBS.iter().any(|s| v.eq_ignore_ascii_case(s))) {
                Some(v) => {
                    deliver(owner, None, Event::ShellVerb(v));
                    Ok(())
                }
                None => invoke(HWND(owner as _), &menu, id, &shown, shift, ctrl),
            }
        }
        _ => Ok(()),
    };
    let _ = DestroyMenu(shown.hmenu);
    result
}

unsafe fn invoke(hwnd: HWND, menu: &IContextMenu, id: usize, shown: &Shown, shift: bool, ctrl: bool) -> Result<(), PlatformError> {
    let dir = (shown.menu.dir.kind() != PathKind::Computer).then(|| shown.menu.dir.to_wide(false));
    let mut mask = CMIC_MASK_UNICODE | CMIC_MASK_PTINVOKE;
    if shift {
        mask |= CMIC_MASK_SHIFT_DOWN;
    }
    if ctrl {
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
        ptInvoke: shown.pt,
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

pub(crate) fn pick_now(hwnd: HWND, cmd: Option<u32>) {
    CURRENT.fetch_add(1, Ordering::SeqCst);
    send_job(Job::Pick { owner: hwnd.0 as isize, cmd, shift: key_down(VK_SHIFT.0 as i32), ctrl: key_down(VK_CONTROL.0 as i32) });
}

fn show(hwnd: HWND, m: &ShellMenu, send: &dyn Fn(Event)) {
    send(Event::ShowMenu { items: m.menu.clone(), x: m.x, y: m.y, launches: Vec::new(), kind: m.kind, shell: Vec::new(), pending: m.shell });
    if !m.shell {
        return;
    }
    let gen = CURRENT.fetch_add(1, Ordering::SeqCst) + 1;
    let mut pt = POINT { x: m.x as i32, y: m.y as i32 };
    let _ = unsafe { ClientToScreen(hwnd, &mut pt) };
    let ready = take_ready(m);
    *locked(&WANT) = None;
    let owner = hwnd.0 as isize;
    match ready {
        Some(r) => {
            tracing::info!("context menu complete from the prepared menu, {} commands", r.ids.len());
            send(Event::ShellItems { items: r.items, shell: r.ids });
            send_job(Job::Adopt { owner, prep: r.gen, menu: m.clone(), pt });
        }
        None => send_job(Job::Menu { owner, gen, menu: m.clone(), pt }),
    }
}

pub(crate) fn run(hwnd: HWND, req: ShellUi, send: &dyn Fn(Event)) {
    let result = match req {
        ShellUi::Menu(m) => {
            show(hwnd, &m, send);
            Ok(())
        }
        ShellUi::Properties(paths) => properties(hwnd, &paths),
        ShellUi::Drag(paths) => crate::dragdrop::start_drag(hwnd, &paths),
        ShellUi::Verb(paths, verb) => invoke_verb(hwnd, &paths, verb),
        ShellUi::Done(gen, ev) => {
            if gen.is_none_or(|g| g == CURRENT.load(Ordering::SeqCst)) {
                send(ev);
            }
            Ok(())
        }
    };
    if let Err(e) = result {
        tracing::warn!("shell: {e}");
    }
}

#[cfg(test)]
#[path = "../test/context_menu.rs"]
mod tests;
