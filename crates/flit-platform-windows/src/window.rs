use crate::context_menu::{ShellUi, WM_SHELL_UI};
use crate::prevhost::WM_HOSTED;
use flit_core::preview::HostCmd;
use crate::err;
use flit_core::input::{CursorIcon, Event, HitArea, Key, Mods, MouseButton, Preedit};
use flit_core::a11y::Tree;
use flit_core::render::{Color, Rect};
use flit_core::theme::SystemColors;
use crate::uia::{WM_A11Y_FOCUS, WM_A11Y_INVOKE, WM_A11Y_SELECT};
use windows::Win32::UI::Accessibility::{IRawElementProviderSimple, UiaClientsAreListening, UiaDisconnectProvider, UiaReturnRawElementProvider, UiaRootObjectId, HCF_HIGHCONTRASTON, HIGHCONTRASTW};
use flit_core::ui::{DragEvent, ShellMenu};
use flit_core::FsPath;
use flit_platform::{DropReply, Placement, PlatformError, Waker, Window, WindowHandler, WindowId, WindowOptions, WindowSystem};
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use windows::core::{w, BOOL, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE};
use windows::Win32::Graphics::Gdi::{GetSysColor, SYS_COLOR_INDEX, COLOR_BTNFACE, COLOR_GRAYTEXT, COLOR_HIGHLIGHT, COLOR_HIGHLIGHTTEXT, COLOR_HOTLIGHT, COLOR_WINDOW, COLOR_WINDOWTEXT, ClientToScreen, GetMonitorInfoW, MonitorFromWindow, ScreenToClient, ValidateRect, MONITORINFO, MONITOR_DEFAULTTONEAREST};
use windows::Win32::System::DataExchange::{AddClipboardFormatListener, CloseClipboard, EmptyClipboard, GetClipboardData, GetClipboardSequenceNumber, SetClipboardData};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::System::Ole::CF_UNICODETEXT;
use windows::Win32::UI::HiDpi::{GetDpiForSystem, GetDpiForWindow, GetSystemMetricsForDpi, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::Input::Ime::{ImmGetCompositionStringW, ImmGetContext, ImmReleaseContext, ImmSetCandidateWindow, ImmSetCompositionWindow, CANDIDATEFORM, CFS_EXCLUDE, CFS_POINT, COMPOSITIONFORM, GCS_COMPSTR, GCS_CURSORPOS, GCS_RESULTSTR, IME_COMPOSITION_STRING, ISC_SHOWUICOMPOSITIONWINDOW};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::System::Threading::WaitForSingleObjectEx;
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::WindowsAndMessaging::*;

const WM_WAKE: u32 = WM_APP + 1;
const WM_DESTROY_REQUEST: u32 = WM_APP + 2;
const WM_OPEN_WINDOW: u32 = WM_APP + 3;
const TIMER_ID: usize = 1;
const COMPACT: (f32, f32) = (560.0, 380.0);

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Mode {
    #[default]
    Normal,
    Full,
    Compact,
}

thread_local! {
    static OPEN: RefCell<Vec<(isize, WindowId)>> = const { RefCell::new(Vec::new()) };
}

#[derive(Default)]
pub(crate) struct FrameSync {
    pub waitable: Cell<HANDLE>,
    pub waited: Cell<bool>,
}

pub struct Win32Window {
    id: WindowId,
    hub: HWND,
    hwnd: Cell<HWND>,
    redraw: Cell<bool>,
    size: Cell<(u32, u32)>,
    dpi: Cell<u32>,
    cursor: Cell<CursorIcon>,
    tracking: Cell<bool>,
    nc_tracking: Cell<bool>,
    high_surrogate: Cell<u16>,
    ime_area: Cell<Rect>,
    mode: Cell<Mode>,
    restore: Cell<Option<(WINDOWPLACEMENT, isize)>>,
    pub(crate) sync: Rc<FrameSync>,
    a11y: RefCell<Option<crate::uia::Access>>,
}

impl Win32Window {
    fn new(id: WindowId, hub: HWND) -> Self {
        Self {
            id,
            hub,
            hwnd: Cell::new(HWND::default()),
            redraw: Cell::new(true),
            size: Cell::new((0, 0)),
            dpi: Cell::new(96),
            cursor: Cell::new(CursorIcon::Default),
            tracking: Cell::new(false),
            nc_tracking: Cell::new(false),
            high_surrogate: Cell::new(0),
            ime_area: Cell::new(Rect::default()),
            mode: Cell::new(Mode::Normal),
            restore: Cell::new(None),
            sync: Rc::new(FrameSync::default()),
            a11y: RefCell::new(None),
        }
    }

    pub(crate) fn hwnd(&self) -> HWND {
        self.hwnd.get()
    }

    fn leave_mode(&self) {
        let Some((wp, style)) = self.restore.take() else { return };
        self.mode.set(Mode::Normal);
        let h = self.hwnd();
        unsafe {
            SetWindowLongPtrW(h, GWL_STYLE, style);
            let _ = SetWindowPos(h, Some(HWND_NOTOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_FRAMECHANGED | SWP_NOACTIVATE);
            let _ = SetWindowPlacement(h, &wp);
        }
    }

    fn enter_mode(&self, mode: Mode) {
        self.leave_mode();
        let h = self.hwnd();
        unsafe {
            let mut wp = WINDOWPLACEMENT { length: size_of::<WINDOWPLACEMENT>() as u32, ..Default::default() };
            let _ = GetWindowPlacement(h, &mut wp);
            let style = GetWindowLongPtrW(h, GWL_STYLE);
            self.restore.set(Some((wp, style)));
            self.mode.set(mode);
            if mode == Mode::Full {
                let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
                let _ = GetMonitorInfoW(MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST), &mut mi);
                let r = mi.rcMonitor;
                SetWindowLongPtrW(h, GWL_STYLE, style & !(WS_OVERLAPPEDWINDOW.0 as isize));
                let _ = SetWindowPos(h, Some(HWND_TOP), r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_FRAMECHANGED | SWP_NOOWNERZORDER);
                return;
            }
            if IsZoomed(h).as_bool() {
                let _ = ShowWindow(h, SW_RESTORE);
            }
            let mut r = RECT::default();
            let _ = GetWindowRect(h, &mut r);
            let sc = self.scale();
            let _ = SetWindowPos(h, Some(HWND_TOPMOST), r.left, r.top, (COMPACT.0 * sc) as i32, (COMPACT.1 * sc) as i32, SWP_NOOWNERZORDER);
        }
    }

    fn post(&self, msg: u32, wp: usize) {
        unsafe {
            let _ = PostMessageW(Some(self.hwnd()), msg, WPARAM(wp), LPARAM(0));
        }
    }
}

#[derive(Clone)]
pub struct Win32Waker {
    hwnd: isize,
    pending: Arc<AtomicBool>,
}

impl Waker for Win32Waker {
    fn wake(&self) {
        if !self.pending.swap(true, Ordering::AcqRel) {
            unsafe {
                let _ = PostMessageW(Some(HWND(self.hwnd as _)), WM_WAKE, WPARAM(0), LPARAM(0));
            }
        }
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

impl Window for Win32Window {
    fn id(&self) -> WindowId {
        self.id
    }

    fn activate(&self) {
        unsafe {
            if IsIconic(self.hwnd()).as_bool() {
                let _ = ShowWindow(self.hwnd(), SW_RESTORE);
            }
            let _ = SetForegroundWindow(self.hwnd());
        }
    }

    fn open_window(&self, opts: WindowOptions) {
        let p = Box::into_raw(Box::new(opts));
        unsafe {
            if PostMessageW(Some(self.hub), WM_OPEN_WINDOW, WPARAM(0), LPARAM(p as isize)).is_err() {
                drop(Box::from_raw(p));
            }
        }
    }

    fn size(&self) -> (u32, u32) {
        self.size.get()
    }

    fn scale(&self) -> f32 {
        self.dpi.get() as f32 / 96.0
    }

    fn request_redraw(&self) {
        self.redraw.set(true);
    }

    fn set_title(&self, title: &str) {
        let w = wide(title);
        unsafe {
            let _ = SetWindowTextW(self.hwnd(), PCWSTR(w.as_ptr()));
        }
    }

    fn set_cursor(&self, cursor: CursorIcon) {
        self.cursor.set(cursor);
        unsafe {
            SetCursor(Some(load_cursor(cursor)));
        }
    }

    fn minimize(&self) {
        self.post(WM_SYSCOMMAND, SC_MINIMIZE as usize);
    }

    fn toggle_maximize(&self) {
        if self.mode.get() != Mode::Normal {
            return self.leave_mode();
        }
        let cmd = if self.is_maximized() { SC_RESTORE } else { SC_MAXIMIZE };
        self.post(WM_SYSCOMMAND, cmd as usize);
    }

    fn is_maximized(&self) -> bool {
        unsafe { IsZoomed(self.hwnd()).as_bool() }
    }

    fn close(&self) {
        self.post(WM_DESTROY_REQUEST, 0);
    }

    fn placement(&self) -> Placement {
        let mut wp = WINDOWPLACEMENT { length: size_of::<WINDOWPLACEMENT>() as u32, ..Default::default() };
        match self.restore.get() {
            Some((saved, _)) => wp = saved,
            None => unsafe {
                let _ = GetWindowPlacement(self.hwnd(), &mut wp);
            },
        }
        let r = wp.rcNormalPosition;
        Placement {
            x: r.left,
            y: r.top,
            width: r.right - r.left,
            height: r.bottom - r.top,
            maximized: wp.showCmd == SW_SHOWMAXIMIZED.0 as u32,
        }
    }

    fn clipboard_text(&self) -> Option<String> {
        unsafe {
            crate::clipboard::open(Some(self.hwnd())).ok()?;
            let text = (|| {
                let h = GetClipboardData(CF_UNICODETEXT.0 as u32).ok()?;
                let units = GlobalSize(HGLOBAL(h.0)) / 2;
                let p = GlobalLock(HGLOBAL(h.0)) as *const u16;
                if p.is_null() {
                    return None;
                }
                let all = std::slice::from_raw_parts(p, units);
                let n = all.iter().position(|&c| c == 0).unwrap_or(units);
                let s = String::from_utf16_lossy(&all[..n]);
                let _ = GlobalUnlock(HGLOBAL(h.0));
                Some(s)
            })();
            let _ = CloseClipboard();
            text
        }
    }

    fn set_clipboard_text(&self, text: &str) {
        let w = wide(text);
        unsafe {
            if crate::clipboard::open(Some(self.hwnd())).is_err() {
                return;
            }
            let _ = EmptyClipboard();
            if let Ok(h) = GlobalAlloc(GMEM_MOVEABLE, w.len() * 2) {
                let p = GlobalLock(h) as *mut u16;
                let owned_by_clipboard = !p.is_null() && {
                    std::ptr::copy_nonoverlapping(w.as_ptr(), p, w.len());
                    let _ = GlobalUnlock(h);
                    SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(h.0))).is_ok()
                };
                if !owned_by_clipboard {
                    let _ = GlobalFree(Some(h));
                }
            }
            let _ = CloseClipboard();
        }
    }

    fn set_ime_area(&self, area: Rect) {
        self.ime_area.set(area);
    }

    fn is_dark_mode(&self) -> bool {
        crate::dark_mode()
    }

    fn set_dark(&self, dark: bool) {
        apply_dark(self.hwnd(), dark);
    }

    fn set_clipboard_files(&self, paths: &[FsPath], cut: bool) {
        crate::clipboard::write_files(self.hwnd(), paths, cut);
    }

    fn shell_menu(&self, menu: ShellMenu) {
        crate::context_menu::request(self.hwnd(), ShellUi::Menu(menu));
    }

    fn share(&self, paths: Vec<FsPath>) {
        crate::context_menu::request(self.hwnd(), ShellUi::Share(paths));
    }

    fn properties(&self, paths: Vec<FsPath>) {
        crate::context_menu::request(self.hwnd(), ShellUi::Properties(paths));
    }

    fn start_drag(&self, paths: Vec<FsPath>) {
        crate::context_menu::request(self.hwnd(), ShellUi::Drag(paths));
    }

    fn set_timer(&self, ms: u32) {
        unsafe { SetTimer(Some(self.hwnd()), TIMER_ID, ms, None) };
    }

    fn to_screen(&self, x: f32, y: f32) -> (i32, i32) {
        let mut p = POINT { x: x as i32, y: y as i32 };
        unsafe {
            let _ = ClientToScreen(self.hwnd(), &mut p);
        }
        (p.x, p.y)
    }

    fn window_at(&self, x: f32, y: f32) -> Option<(WindowId, f32, f32)> {
        let (sx, sy) = self.to_screen(x, y);
        unsafe {
            let root = GetAncestor(WindowFromPoint(POINT { x: sx, y: sy }), GA_ROOT);
            let id = OPEN.with_borrow(|open| open.iter().find(|(h, _)| *h == root.0 as isize).map(|&(_, id)| id))?;
            let mut p = POINT { x: sx, y: sy };
            let _ = ScreenToClient(root, &mut p);
            Some((id, p.x as f32, p.y as f32))
        }
    }

    fn toggle_fullscreen(&self) {
        if self.mode.get() == Mode::Full {
            self.leave_mode();
        } else {
            self.enter_mode(Mode::Full);
        }
    }

    fn set_compact(&self, on: Option<bool>) {
        let compact = self.mode.get() == Mode::Compact;
        match on.unwrap_or(!compact) {
            true if !compact => self.enter_mode(Mode::Compact),
            false if compact => self.leave_mode(),
            _ => {}
        }
    }

    fn host(&self, cmd: HostCmd) {
        crate::prevhost::command(self.hwnd(), cmd);
    }
}

pub(crate) fn high_contrast() -> Option<SystemColors> {
    if crate::d3d::capture_path().is_some() && std::env::var_os("FLIT_CAPTURE_CONTRAST").is_some() {
        let c = Color::rgb;
        return Some(SystemColors { window: c(0x000000), window_text: c(0xFFFFFF), highlight: c(0x1AEBFF), highlight_text: c(0x000000), hotlight: c(0x75E9FC), gray_text: c(0x3FF23F), btn_face: c(0x000000) });
    }
    let mut hc = HIGHCONTRASTW { cbSize: size_of::<HIGHCONTRASTW>() as u32, ..Default::default() };
    unsafe { SystemParametersInfoW(SPI_GETHIGHCONTRAST, hc.cbSize, Some(&mut hc as *mut HIGHCONTRASTW as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0)) }.ok()?;
    if !hc.dwFlags.contains(HCF_HIGHCONTRASTON) {
        return None;
    }
    let c = |i: SYS_COLOR_INDEX| Color(unsafe { GetSysColor(i) } | 0xFF00_0000);
    Some(SystemColors { window: c(COLOR_WINDOW), window_text: c(COLOR_WINDOWTEXT), highlight: c(COLOR_HIGHLIGHT), highlight_text: c(COLOR_HIGHLIGHTTEXT), hotlight: c(COLOR_HOTLIGHT), gray_text: c(COLOR_GRAYTEXT), btn_face: c(COLOR_BTNFACE) })
}

fn load_cursor(c: CursorIcon) -> HCURSOR {
    let id = match c {
        CursorIcon::Default => IDC_ARROW,
        CursorIcon::Text => IDC_IBEAM,
        CursorIcon::Hand => IDC_HAND,
        CursorIcon::ResizeHorizontal => IDC_SIZEWE,
        CursorIcon::ResizeVertical | CursorIcon::Scroll => IDC_SIZENS,
    };
    unsafe { LoadCursorW(None, id).unwrap_or_default() }
}

pub(crate) fn apply_dark(hwnd: HWND, dark: bool) {
    let v = BOOL::from(dark);
    unsafe {
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &v as *const BOOL as _, 4);
    }
}

pub(crate) struct Node<H> {
    window: Win32Window,
    queue: RefCell<VecDeque<Event>>,
    ready: Cell<bool>,
    shared: *const Shared<H>,
}

struct Shared<H> {
    handler: RefCell<H>,
    nodes: RefCell<Vec<Rc<Node<H>>>>,
    dead: RefCell<Vec<Rc<Node<H>>>>,
    woken: Cell<bool>,
    next_id: Cell<WindowId>,
    hinst: HINSTANCE,
    hub: HWND,
    wake_pending: Arc<AtomicBool>,
}

impl<H: WindowHandler<Win32>> Node<H> {
    fn shared(&self) -> &Shared<H> {
        unsafe { &*self.shared }
    }

    fn send(&self, ev: Event) {
        self.queue.borrow_mut().push_back(ev);
        self.shared().flush();
    }

    fn hit_test(&self, x: f32, y: f32) -> HitArea {
        if !self.ready.get() {
            return HitArea::Client;
        }
        self.shared().handler.try_borrow_mut().map_or(HitArea::Client, |mut h| h.hit_test(&self.window, x, y))
    }

    fn drag(&self, ev: DragEvent) -> DropReply {
        if !self.ready.get() {
            return DropReply::default();
        }
        let sh = self.shared();
        let reply = sh.handler.try_borrow_mut().map_or_else(|_| DropReply::default(), |mut h| h.drag(&self.window, ev));
        sh.flush();
        if self.window.redraw.get() {
            self.redraw_now();
        }
        reply
    }

    fn sync_a11y(&self, h: &mut H) {
        let access = self.window.a11y.borrow();
        let Some(acc) = access.as_ref() else { return };
        if unsafe { UiaClientsAreListening() }.as_bool() {
            if let Some(tree) = h.accessibility(&self.window) {
                acc.update(tree);
            }
        }
    }

    fn root_provider(&self, hwnd: HWND) -> IRawElementProviderSimple {
        let tree = if self.ready.get() { self.shared().handler.try_borrow_mut().ok().and_then(|mut h| h.accessibility(&self.window)) } else { None };
        if self.window.a11y.borrow().is_none() {
            let first = crate::uia::Access::new(hwnd, tree.clone().unwrap_or_else(|| Tree::new(String::new(), Rect::default())));
            *self.window.a11y.borrow_mut() = Some(first);
        } else if let (Some(acc), Some(tree)) = (self.window.a11y.borrow().as_ref(), tree) {
            acc.update(tree);
        }
        self.window.a11y.borrow().as_ref().map(|a| a.root.clone()).expect("accessibility root")
    }

    fn redraw_now(&self) {
        let sh = self.shared();
        match sh.handler.try_borrow_mut() {
            Ok(mut h) if self.ready.get() => {
                self.window.redraw.set(false);
                h.redraw(&self.window);
                self.sync_a11y(&mut *h);
            }
            _ => self.window.redraw.set(true),
        }
        sh.flush();
    }
}

impl<H: WindowHandler<Win32>> Shared<H> {
    fn nodes(&self) -> Vec<Rc<Node<H>>> {
        self.nodes.borrow().clone()
    }

    fn flush(&self) {
        let Ok(mut h) = self.handler.try_borrow_mut() else { return };
        loop {
            let dead = std::mem::take(&mut *self.dead.borrow_mut());
            for n in &dead {
                h.destroyed(&n.window);
            }
            if self.woken.replace(false) {
                for n in self.nodes() {
                    h.woken(&n.window);
                }
            }
            let mut idle = dead.is_empty();
            for n in self.nodes() {
                loop {
                    let next = n.queue.borrow_mut().pop_front();
                    let Some(ev) = next else { break };
                    idle = false;
                    h.event(&n.window, ev);
                }
            }
            if idle && !self.woken.get() && self.dead.borrow().is_empty() {
                break;
            }
        }
    }

    unsafe fn open(&self, opts: WindowOptions) -> Result<(), PlatformError> {
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        let node = Rc::new(Node { window: Win32Window::new(id, self.hub), queue: RefCell::default(), ready: Cell::new(false), shared: self });
        let sys = GetDpiForSystem() as f32 / 96.0;
        let title = wide(&opts.title);
        let hidden = crate::d3d::hidden();
        let (ex, at) = if hidden { (WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW, -32000) } else { (WS_EX_APPWINDOW, CW_USEDEFAULT) };
        let hwnd = CreateWindowExW(
            WS_EX_NOREDIRECTIONBITMAP | ex,
            WINDOW_CLASS,
            PCWSTR(title.as_ptr()),
            WS_OVERLAPPEDWINDOW,
            at,
            at,
            (1100.0 * sys) as i32,
            (720.0 * sys) as i32,
            None,
            None,
            Some(self.hinst),
            Some(Rc::as_ptr(&node) as *const _),
        )
        .map_err(err)?;
        OPEN.with_borrow_mut(|open| open.push((hwnd.0 as isize, id)));
        let mut maximized = false;
        if let Some(p) = opts.placement.filter(|p| !hidden && p.width > 100 && p.height > 100) {
            let wp = WINDOWPLACEMENT {
                length: size_of::<WINDOWPLACEMENT>() as u32,
                showCmd: SW_HIDE.0 as u32,
                rcNormalPosition: RECT { left: p.x, top: p.y, right: p.x + p.width, bottom: p.y + p.height },
                ..Default::default()
            };
            let _ = SetWindowPlacement(hwnd, &wp);
            maximized = p.maximized;
        }
        node.window.dpi.set(GetDpiForWindow(hwnd));
        apply_dark(hwnd, opts.dark);
        let _ = AddClipboardFormatListener(hwnd);
        crate::context_menu::prewarm();
        let _ = SetWindowPos(hwnd, None, 0, 0, 0, 0, SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
        update_size(&node.window);
        self.nodes.borrow_mut().push(node.clone());
        if let Ok(mut h) = self.handler.try_borrow_mut() {
            h.created(&node.window);
        }
        if let Some(colors) = high_contrast() {
            node.queue.borrow_mut().push_back(Event::Contrast(Some(colors)));
        }
        let tabs = if hidden { std::env::var("FLIT_CAPTURE_TABS").ok().and_then(|v| v.parse().ok()).unwrap_or(0) } else { 0 };
        for _ in 0..tabs {
            node.queue.borrow_mut().push_back(Event::Key { key: Key::Tab, mods: Mods::default(), pressed: true });
        }
        node.ready.set(true);
        self.flush();
        node.redraw_now();
        if hidden {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            if std::env::var_os("FLIT_CAPTURE_SETTINGS").is_some() {
                node.queue.borrow_mut().push_back(Event::Key { key: Key::Char(','), mods: Mods { ctrl: true, ..Mods::default() }, pressed: true });
                self.woken.set(true);
            }
            if std::env::var_os("FLIT_CAPTURE_PROPERTIES").is_some() {
                let mut q = node.queue.borrow_mut();
                q.push_back(Event::Key { key: Key::Down, mods: Mods::default(), pressed: true });
                q.push_back(Event::Key { key: Key::Enter, mods: Mods { alt: true, ..Mods::default() }, pressed: true });
                self.woken.set(true);
            }
        } else {
            let _ = ShowWindow(hwnd, if maximized { SW_SHOWMAXIMIZED } else { SW_SHOW });
        }
        crate::dragdrop::register(hwnd, Rc::as_ptr(&node) as *const (), drag_thunk::<H>);
        Ok(())
    }

    fn destroyed(&self, hwnd: HWND) {
        let mut nodes = self.nodes.borrow_mut();
        if let Some(i) = nodes.iter().position(|n| n.window.hwnd() == hwnd) {
            self.dead.borrow_mut().push(nodes.remove(i));
        }
        let last = nodes.is_empty();
        drop(nodes);
        self.flush();
        if last {
            unsafe { PostQuitMessage(0) };
        }
    }
}

const WINDOW_CLASS: PCWSTR = w!("FlitWindow");
const HUB_CLASS: PCWSTR = w!("FlitHub");

type HubDispatch = unsafe fn(*const (), u32, LPARAM);

thread_local! {
    static HUB: Cell<Option<(*const (), HubDispatch)>> = const { Cell::new(None) };
}

unsafe extern "system" fn hub_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg != WM_WAKE && msg != WM_OPEN_WINDOW {
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    match HUB.get() {
        Some((ctx, dispatch)) => dispatch(ctx, msg, lp),
        None if msg == WM_OPEN_WINDOW => drop(Box::from_raw(lp.0 as *mut WindowOptions)),
        None => {}
    }
    LRESULT(0)
}

unsafe fn hub_dispatch<H: WindowHandler<Win32>>(ctx: *const (), msg: u32, lp: LPARAM) {
    let sh = &*(ctx as *const Shared<H>);
    if msg == WM_WAKE {
        sh.wake_pending.store(false, Ordering::Release);
        sh.woken.set(true);
        sh.flush();
        return;
    }
    if sh.handler.try_borrow_mut().is_err() {
        let _ = PostMessageW(Some(sh.hub), WM_OPEN_WINDOW, WPARAM(0), lp);
        return;
    }
    let opts = *Box::from_raw(lp.0 as *mut WindowOptions);
    if let Err(e) = sh.open(opts) {
        tracing::error!("cannot open window: {e}");
    }
}

pub struct Win32 {
    hinst: HINSTANCE,
    hub: HWND,
    wake_pending: Arc<AtomicBool>,
}

impl WindowSystem for Win32 {
    type Window = Win32Window;
    type Waker = Win32Waker;

    fn new() -> Result<Self, PlatformError> {
        unsafe {
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            let hinst: HINSTANCE = GetModuleHandleW(None).map_err(err)?.into();
            let wc = WNDCLASSEXW { cbSize: size_of::<WNDCLASSEXW>() as u32, lpfnWndProc: Some(hub_proc), hInstance: hinst, lpszClassName: HUB_CLASS, ..Default::default() };
            RegisterClassExW(&wc);
            let hub = CreateWindowExW(WINDOW_EX_STYLE(0), HUB_CLASS, PCWSTR::null(), WINDOW_STYLE(0), 0, 0, 0, 0, Some(HWND_MESSAGE), None, Some(hinst), None).map_err(err)?;
            Ok(Self { hinst, hub, wake_pending: Arc::new(AtomicBool::new(false)) })
        }
    }

    fn waker(&self) -> Win32Waker {
        Win32Waker { hwnd: self.hub.0 as isize, pending: self.wake_pending.clone() }
    }

    fn run<H: WindowHandler<Self>>(self, first: WindowOptions, handler: H) -> Result<(), PlatformError> {
        unsafe {
            let wc = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                style: CS_DBLCLKS,
                lpfnWndProc: Some(wndproc::<H>),
                hInstance: self.hinst,
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                hIcon: LoadIconW(Some(self.hinst), PCWSTR(1 as _)).unwrap_or_default(),
                lpszClassName: WINDOW_CLASS,
                ..Default::default()
            };
            RegisterClassExW(&wc);
            let shared = Shared {
                handler: RefCell::new(handler),
                nodes: RefCell::default(),
                dead: RefCell::default(),
                woken: Cell::new(false),
                next_id: Cell::new(1),
                hinst: self.hinst,
                hub: self.hub,
                wake_pending: self.wake_pending.clone(),
            };
            HUB.set(Some((&shared as *const Shared<H> as *const (), hub_dispatch::<H> as HubDispatch)));
            let result = shared.open(first).and_then(|()| message_loop(&shared));
            HUB.set(None);
            for n in shared.nodes() {
                SetWindowLongPtrW(n.window.hwnd(), GWLP_USERDATA, 0);
            }
            result
        }
    }
}

unsafe fn drag_thunk<H: WindowHandler<Win32>>(node: *const (), ev: DragEvent) -> DropReply {
    (*(node as *const Node<H>)).drag(ev)
}

unsafe fn update_size(w: &Win32Window) -> bool {
    let mut rc = RECT::default();
    let _ = GetClientRect(w.hwnd(), &mut rc);
    let size = ((rc.right - rc.left).max(0) as u32, (rc.bottom - rc.top).max(0) as u32);
    let changed = size != w.size.get();
    w.size.set(size);
    changed
}

unsafe fn pump(msg: &mut MSG) -> bool {
    while PeekMessageW(msg, None, 0, 0, PM_REMOVE).as_bool() {
        if msg.message == WM_QUIT {
            return false;
        }
        let _ = TranslateMessage(msg);
        DispatchMessageW(msg);
    }
    true
}

unsafe fn message_loop<H: WindowHandler<Win32>>(sh: &Shared<H>) -> Result<(), PlatformError> {
    let mut msg = MSG::default();
    loop {
        if !pump(&mut msg) && sh.nodes().is_empty() {
            return Ok(());
        }
        let dirty: Vec<Rc<Node<H>>> = sh.nodes().into_iter().filter(|n| n.window.redraw.get()).collect();
        if dirty.is_empty() {
            let _ = WaitMessage();
            continue;
        }
        let paced: Vec<&Rc<Node<H>>> = dirty.iter().filter(|n| !n.window.sync.waitable.get().is_invalid()).collect();
        if !paced.is_empty() {
            let handles: Vec<HANDLE> = paced.iter().map(|n| n.window.sync.waitable.get()).collect();
            let r = MsgWaitForMultipleObjectsEx(Some(&handles), 1000, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
            let k = r.0.wrapping_sub(WAIT_OBJECT_0.0) as usize;
            if k == handles.len() {
                continue;
            }
            for (i, n) in paced.iter().enumerate() {
                if i == k || WaitForSingleObjectEx(handles[i], 0, false) == WAIT_OBJECT_0 {
                    n.window.sync.waited.set(true);
                }
            }
            if !pump(&mut msg) && sh.nodes().is_empty() {
                return Ok(());
            }
        }
        for n in &dirty {
            let s = &n.window.sync;
            if n.window.redraw.get() && (s.waited.get() || s.waitable.get().is_invalid()) {
                n.redraw_now();
            }
        }
    }
}

fn frame_thickness(dpi: u32) -> i32 {
    unsafe { GetSystemMetricsForDpi(SM_CXFRAME, dpi) + GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi) }
}

fn lparam_point(lp: LPARAM) -> POINT {
    POINT { x: (lp.0 & 0xFFFF) as i16 as i32, y: ((lp.0 >> 16) & 0xFFFF) as i16 as i32 }
}

fn mods() -> Mods {
    let down = |vk: VIRTUAL_KEY| unsafe { GetKeyState(vk.0 as i32) } < 0;
    Mods { ctrl: down(VK_CONTROL), shift: down(VK_SHIFT), alt: down(VK_MENU) }
}

fn map_key(vk: u16) -> Key {
    let vk = VIRTUAL_KEY(vk);
    match vk {
        VK_UP => Key::Up,
        VK_DOWN => Key::Down,
        VK_LEFT => Key::Left,
        VK_RIGHT => Key::Right,
        VK_PRIOR => Key::PageUp,
        VK_NEXT => Key::PageDown,
        VK_HOME => Key::Home,
        VK_END => Key::End,
        VK_RETURN => Key::Enter,
        VK_ESCAPE => Key::Escape,
        VK_BACK => Key::Backspace,
        VK_DELETE => Key::Delete,
        VK_INSERT => Key::Insert,
        VK_TAB => Key::Tab,
        VK_SPACE => Key::Space,
        VK_BROWSER_BACK => Key::BrowserBack,
        VK_BROWSER_FORWARD => Key::BrowserForward,
        VK_APPS => Key::Apps,
        VK_OEM_3 => Key::Char('`'),
        VK_OEM_COMMA => Key::Char(','),
        VK_OEM_PERIOD => Key::Char('.'),
        VK_OEM_PLUS => Key::Char('+'),
        VK_OEM_MINUS | VK_SUBTRACT => Key::Char('-'),
        VK_ADD => Key::NumPlus,
        _ if (VK_F1.0..=VK_F24.0).contains(&vk.0) => Key::F((vk.0 - VK_F1.0 + 1) as u8),
        _ if (b'A' as u16..=b'Z' as u16).contains(&vk.0) || (b'0' as u16..=b'9' as u16).contains(&vk.0) => Key::Char(vk.0 as u8 as char),
        _ => Key::Other,
    }
}

pub(crate) fn preedit_from_utf16(units: &[u16], cursor: i32) -> Preedit {
    let text = String::from_utf16_lossy(units);
    let cursor = usize::try_from(cursor).ok().filter(|&c| c <= units.len()).map(|c| String::from_utf16_lossy(&units[..c]).len());
    Preedit::new(text, cursor)
}

unsafe fn composition(hwnd: HWND) -> Preedit {
    let himc = ImmGetContext(hwnd);
    if himc.is_invalid() {
        return Preedit::default();
    }
    let bytes = ImmGetCompositionStringW(himc, GCS_COMPSTR, None, 0);
    let mut units = vec![0u16; usize::try_from(bytes).unwrap_or(0) / 2];
    let got = if units.is_empty() { 0 } else { ImmGetCompositionStringW(himc, GCS_COMPSTR, Some(units.as_mut_ptr() as _), (units.len() * 2) as u32) };
    units.truncate(usize::try_from(got).unwrap_or(0) / 2);
    let cursor = ImmGetCompositionStringW(himc, GCS_CURSORPOS, None, 0);
    let _ = ImmReleaseContext(hwnd, himc);
    preedit_from_utf16(&units, cursor)
}

fn is_caption_button(wp: WPARAM) -> bool {
    matches!(wp.0 as u32, HTMINBUTTON | HTMAXBUTTON | HTCLOSE)
}

unsafe extern "system" fn wndproc<H: WindowHandler<Win32>>(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        let cs = &*(lp.0 as *const CREATESTRUCTW);
        let state = cs.lpCreateParams as *const Node<H>;
        (*state).window.hwnd.set(hwnd);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, state as isize);
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Node<H>;
    if ptr.is_null() {
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    if msg == WM_NCDESTROY {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    Rc::increment_strong_count(ptr);
    let node = Rc::from_raw(ptr);
    match handle(&node, hwnd, msg, wp, lp) {
        Some(r) => r,
        None => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

unsafe fn handle<H: WindowHandler<Win32>>(s: &Node<H>, hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
    let w = &s.window;
    if let Some(r) = crate::context_menu::forward(msg, wp, lp) {
        return Some(r);
    }
    let client_xy = || {
        let p = lparam_point(lp);
        (p.x as f32, p.y as f32)
    };
    let screen_xy = || {
        let mut p = lparam_point(lp);
        let _ = ScreenToClient(hwnd, &mut p);
        (p.x as f32, p.y as f32)
    };
    match msg {
        WM_NCCALCSIZE if wp.0 != 0 => {
            let p = &mut *(lp.0 as *mut NCCALCSIZE_PARAMS);
            let top = p.rgrc[0].top;
            let r = DefWindowProcW(hwnd, msg, wp, lp);
            p.rgrc[0].top = top;
            if IsZoomed(hwnd).as_bool() && w.mode.get() != Mode::Full {
                p.rgrc[0].top += frame_thickness(w.dpi.get());
            }
            Some(r)
        }
        WM_NCHITTEST => {
            let def = DefWindowProcW(hwnd, msg, wp, lp);
            if def.0 as u32 != HTCLIENT {
                return Some(def);
            }
            let (x, y) = screen_xy();
            if !IsZoomed(hwnd).as_bool() && w.mode.get() != Mode::Full && y < frame_thickness(w.dpi.get()) as f32 / 2.0 {
                return Some(LRESULT(HTTOP as isize));
            }
            let area = s.hit_test(x, y);
            let ht = match area {
                HitArea::Client => HTCLIENT,
                HitArea::Caption => HTCAPTION,
                HitArea::Minimize => HTMINBUTTON,
                HitArea::Maximize => HTMAXBUTTON,
                HitArea::Close => HTCLOSE,
            };
            Some(LRESULT(ht as isize))
        }
        WM_NCMOUSEMOVE => {
            if !w.nc_tracking.get() {
                let mut t = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE | TME_NONCLIENT, hwndTrack: hwnd, dwHoverTime: 0 };
                let _ = TrackMouseEvent(&mut t);
                w.nc_tracking.set(true);
            }
            let (x, y) = screen_xy();
            s.send(Event::MouseMove { x, y });
            None
        }
        WM_NCMOUSELEAVE => {
            w.nc_tracking.set(false);
            s.send(Event::MouseLeave);
            None
        }
        WM_NCLBUTTONDOWN | WM_NCLBUTTONDBLCLK if is_caption_button(wp) => {
            let (x, y) = screen_xy();
            s.send(Event::MouseDown { x, y, button: MouseButton::Left, clicks: 1 });
            Some(LRESULT(0))
        }
        WM_NCLBUTTONUP if is_caption_button(wp) => {
            let (x, y) = screen_xy();
            s.send(Event::MouseUp { x, y, button: MouseButton::Left });
            Some(LRESULT(0))
        }
        WM_MOUSEMOVE => {
            if !w.tracking.get() {
                let mut t = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                let _ = TrackMouseEvent(&mut t);
                w.tracking.set(true);
            }
            let (x, y) = client_xy();
            s.send(Event::MouseMove { x, y });
            Some(LRESULT(0))
        }
        WM_MOUSELEAVE => {
            w.tracking.set(false);
            s.send(Event::MouseLeave);
            Some(LRESULT(0))
        }
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK | WM_RBUTTONDOWN | WM_RBUTTONDBLCLK | WM_MBUTTONDOWN | WM_MBUTTONDBLCLK | WM_XBUTTONDOWN
        | WM_XBUTTONDBLCLK => {
            let button = match msg {
                WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => MouseButton::Left,
                WM_RBUTTONDOWN | WM_RBUTTONDBLCLK => MouseButton::Right,
                WM_MBUTTONDOWN | WM_MBUTTONDBLCLK => MouseButton::Middle,
                _ if (wp.0 >> 16) & 0xFFFF == XBUTTON1 as usize => MouseButton::Back,
                _ => MouseButton::Forward,
            };
            let clicks = if matches!(msg, WM_LBUTTONDBLCLK | WM_RBUTTONDBLCLK | WM_MBUTTONDBLCLK | WM_XBUTTONDBLCLK) { 2 } else { 1 };
            SetCapture(hwnd);
            let (x, y) = client_xy();
            s.send(Event::Pointer { touch: GetMessageExtraInfo().0 as u32 & 0xFFFF_FF00 == 0xFF51_5700 });
            s.send(Event::MouseDown { x, y, button, clicks });
            Some(LRESULT(if matches!(msg, WM_XBUTTONDOWN | WM_XBUTTONDBLCLK) { 1 } else { 0 }))
        }
        WM_LBUTTONUP | WM_RBUTTONUP | WM_MBUTTONUP | WM_XBUTTONUP => {
            let button = match msg {
                WM_LBUTTONUP => MouseButton::Left,
                WM_RBUTTONUP => MouseButton::Right,
                WM_MBUTTONUP => MouseButton::Middle,
                _ if (wp.0 >> 16) & 0xFFFF == XBUTTON1 as usize => MouseButton::Back,
                _ => MouseButton::Forward,
            };
            const ANY: usize = 0x0001 | 0x0002 | 0x0010 | 0x0020 | 0x0040;
            if wp.0 & ANY == 0 {
                let _ = ReleaseCapture();
            }
            let (x, y) = client_xy();
            s.send(Event::MouseUp { x, y, button });
            Some(LRESULT(if msg == WM_XBUTTONUP { 1 } else { 0 }))
        }
        WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
            let delta = ((wp.0 >> 16) & 0xFFFF) as i16 as f32 / WHEEL_DELTA as f32;
            let ev = if msg == WM_MOUSEWHEEL { Event::Wheel { dx: 0.0, dy: delta } } else { Event::Wheel { dx: delta, dy: 0.0 } };
            s.send(ev);
            Some(LRESULT(0))
        }
        WM_KEYDOWN | WM_SYSKEYDOWN | WM_KEYUP | WM_SYSKEYUP => {
            let vk = wp.0 as u16;
            let sys = matches!(msg, WM_SYSKEYDOWN | WM_SYSKEYUP);
            if sys && (vk == VK_F4.0 || vk == VK_SPACE.0) {
                return None;
            }
            let pressed = matches!(msg, WM_KEYDOWN | WM_SYSKEYDOWN);
            s.send(Event::Key { key: map_key(vk), mods: mods(), pressed });
            Some(LRESULT(0))
        }
        WM_CHAR => {
            let u = wp.0 as u16;
            let c = if (0xD800..0xDC00).contains(&u) {
                w.high_surrogate.set(u);
                None
            } else if (0xDC00..0xE000).contains(&u) {
                let hi = w.high_surrogate.replace(0);
                char::decode_utf16([hi, u]).next().and_then(Result::ok)
            } else {
                char::from_u32(u as u32)
            };
            if let Some(c) = c {
                s.send(Event::Char(c));
            }
            Some(LRESULT(0))
        }
        WM_SYSCHAR if wp.0 != ' ' as usize => Some(LRESULT(0)),
        WM_SIZE => {
            if wp.0 as u32 != SIZE_MINIMIZED && update_size(w) {
                let (width, height) = w.size.get();
                s.send(Event::Resized { width, height });
                s.redraw_now();
            }
            Some(LRESULT(0))
        }
        WM_DPICHANGED => {
            let dpi = ((wp.0 >> 16) & 0xFFFF) as u32;
            w.dpi.set(dpi);
            s.send(Event::ScaleChanged(dpi as f32 / 96.0));
            let r = &*(lp.0 as *const RECT);
            let _ = SetWindowPos(hwnd, None, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
            Some(LRESULT(0))
        }
        WM_GETMINMAXINFO => {
            let mmi = &mut *(lp.0 as *mut MINMAXINFO);
            let sc = w.dpi.get() as f32 / 96.0;
            mmi.ptMinTrackSize = POINT { x: (520.0 * sc) as i32, y: (320.0 * sc) as i32 };
            Some(LRESULT(0))
        }
        WM_PAINT => {
            let _ = ValidateRect(Some(hwnd), None);
            s.redraw_now();
            Some(LRESULT(0))
        }
        WM_ERASEBKGND => Some(LRESULT(1)),
        WM_SETCURSOR if (lp.0 & 0xFFFF) as u32 == HTCLIENT => {
            SetCursor(Some(load_cursor(w.cursor.get())));
            Some(LRESULT(1))
        }
        WM_SETFOCUS | WM_KILLFOCUS => {
            s.send(Event::Focus(msg == WM_SETFOCUS));
            None
        }
        WM_GETOBJECT if lp.0 as i32 == UiaRootObjectId => Some(UiaReturnRawElementProvider(hwnd, wp, lp, &s.root_provider(hwnd))),
        WM_A11Y_INVOKE => {
            s.send(Event::Invoke(wp.0 as u64));
            Some(LRESULT(0))
        }
        WM_A11Y_FOCUS => {
            s.send(Event::A11yFocus(wp.0 as u64));
            Some(LRESULT(0))
        }
        WM_A11Y_SELECT => {
            s.send(Event::A11ySelect(wp.0 as u64));
            Some(LRESULT(0))
        }
        WM_SYSCOLORCHANGE => {
            s.send(Event::Contrast(high_contrast()));
            None
        }
        WM_SETTINGCHANGE if wp.0 as u32 == SPI_SETHIGHCONTRAST.0 => {
            s.send(Event::Contrast(high_contrast()));
            None
        }
        WM_SETTINGCHANGE => {
            if lp.0 != 0 && PCWSTR(lp.0 as *const u16).to_string().is_ok_and(|v| v == "ImmersiveColorSet") {
                s.send(Event::ThemeChanged { dark: crate::dark_mode() });
            }
            None
        }
        WM_SHELL_UI => {
            for req in crate::context_menu::take(hwnd) {
                crate::context_menu::run(hwnd, req, &|ev| s.send(ev));
            }
            Some(LRESULT(0))
        }
        WM_HOSTED => {
            for ev in crate::prevhost::take(hwnd) {
                s.send(Event::Hosted(ev));
            }
            Some(LRESULT(0))
        }
        WM_IME_SETCONTEXT => Some(DefWindowProcW(hwnd, msg, wp, LPARAM(lp.0 & !(ISC_SHOWUICOMPOSITIONWINDOW as isize)))),
        WM_IME_STARTCOMPOSITION => {
            let a = w.ime_area.get();
            let himc = ImmGetContext(hwnd);
            let cf = COMPOSITIONFORM { dwStyle: CFS_POINT, ptCurrentPos: POINT { x: a.x as i32, y: a.y as i32 }, ..Default::default() };
            let _ = ImmSetCompositionWindow(himc, &cf);
            let area = RECT { left: a.x as i32, top: a.y as i32, right: a.right() as i32, bottom: a.bottom() as i32 };
            let cand = CANDIDATEFORM { dwIndex: 0, dwStyle: CFS_EXCLUDE, ptCurrentPos: POINT { x: a.x as i32, y: a.bottom() as i32 }, rcArea: area };
            let _ = ImmSetCandidateWindow(himc, &cand);
            let _ = ImmReleaseContext(hwnd, himc);
            Some(LRESULT(0))
        }
        WM_IME_COMPOSITION => {
            let flags = IME_COMPOSITION_STRING(lp.0 as u32);
            let committed = (flags.0 & GCS_RESULTSTR.0) != 0;
            let r = if committed { DefWindowProcW(hwnd, msg, wp, lp) } else { LRESULT(0) };
            let preedit = if (flags.0 & GCS_COMPSTR.0) != 0 { composition(hwnd) } else { Preedit::default() };
            if committed || (flags.0 & GCS_COMPSTR.0) != 0 {
                s.send(Event::Preedit(preedit));
            }
            Some(r)
        }
        WM_IME_ENDCOMPOSITION => {
            s.send(Event::Preedit(Preedit::default()));
            Some(LRESULT(0))
        }
        WM_CLOSE => {
            s.send(Event::CloseRequested);
            Some(LRESULT(0))
        }
        WM_DESTROY_REQUEST => {
            let _ = DestroyWindow(hwnd);
            Some(LRESULT(0))
        }
        WM_ENDSESSION => {
            if wp.0 != 0 {
                s.send(Event::SessionEnding);
                let _ = DestroyWindow(hwnd);
            }
            Some(LRESULT(0))
        }
        WM_DESTROY => {
            if let Some(acc) = w.a11y.take() {
                let _ = UiaReturnRawElementProvider(hwnd, WPARAM(0), LPARAM(0), None::<&IRawElementProviderSimple>);
                let _ = UiaDisconnectProvider(&acc.root);
            }
            crate::dragdrop::revoke(hwnd);
            OPEN.with_borrow_mut(|open| open.retain(|(h, _)| *h != hwnd.0 as isize));
            crate::prevhost::forget(hwnd);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            s.shared().destroyed(hwnd);
            Some(LRESULT(0))
        }
        WM_DEVICECHANGE if matches!(wp.0 as u32, DBT_DEVICEARRIVAL | DBT_DEVICEREMOVECOMPLETE) => {
            s.send(Event::DevicesChanged);
            None
        }
        WM_TIMER if wp.0 == TIMER_ID => {
            let _ = KillTimer(Some(hwnd), TIMER_ID);
            s.send(Event::Timer);
            Some(LRESULT(0))
        }
        WM_CLIPBOARDUPDATE => {
            s.send(Event::ClipboardChanged(GetClipboardSequenceNumber()));
            Some(LRESULT(0))
        }
        _ => None,
    }
}

#[cfg(test)]
#[path = "../test/window.rs"]
mod tests;
