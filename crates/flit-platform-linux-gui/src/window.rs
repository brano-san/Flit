use crate::input::{self, Clicks};
use flit_core::input::{CursorIcon, Event, HitArea, MouseButton};
use flit_core::render::Rect;
use flit_core::ui::{drop_effect, DragEvent, ShellMenu};
use flit_core::FsPath;
use flit_platform::{Placement, PlatformError, Waker, Window, WindowHandler, WindowId, WindowOptions, WindowSystem};
use std::cell::{Cell, RefCell};
use std::ffi::CStr;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::data_transfer::{DataTransferId, DataTransferSendBuilder, TypeHint};
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, Ime, WindowEvent};
use winit::event_loop::run_on_demand::EventLoopExtRunOnDemand;
use winit::event_loop::{ActiveEventLoop, ControlFlow, DndAction, EventLoop, EventLoopProxy, OwnedDisplayHandle};
use winit::keyboard::ModifiersState;
use winit::platform::wayland::{ActiveEventLoopExtWayland, WindowAttributesWayland};
use winit::platform::startup_notify::WindowExtStartupNotify;
use winit::platform::x11::WindowAttributesX11;
use winit::raw_window_handle::{HasDisplayHandle, RawDisplayHandle};
use winit::monitor::Fullscreen;
use winit::window::{ImeCapabilities, ImeEnableRequest, ImeRequest, ImeRequestData, PlatformWindowAttributes, ResizeDirection, Theme, WindowAttributes, WindowLevel};

const APP_ID: &str = "flit";
const DEFAULT_SIZE: (f64, f64) = (1100.0, 720.0);
const MIN_SIZE: (f64, f64) = (520.0, 320.0);
const COMPACT_SIZE: (f64, f64) = (560.0, 380.0);
const RESIZE_BORDER: f32 = 5.0;
const X11_KEYBOARD_LIBRARY: &CStr = c"libxkbcommon-x11.so.0";

const DARK_UNKNOWN: u8 = 0;
const DARK_NO: u8 = 1;
const DARK_YES: u8 = 2;

static DISPLAY: OnceLock<OwnedDisplayHandle> = OnceLock::new();
static PROXY: OnceLock<EventLoopProxy> = OnceLock::new();
static PREFERRED_DARK: AtomicU8 = AtomicU8::new(DARK_UNKNOWN);
static THEME_CHANGED: AtomicBool = AtomicBool::new(false);
static SHUTDOWN: AtomicBool = AtomicBool::new(false);
static QUEUED: Mutex<Vec<(WindowId, Event)>> = Mutex::new(Vec::new());
static TOKEN: Mutex<Option<String>> = Mutex::new(None);

pub(crate) fn activation_token() -> Option<String> {
    TOKEN.lock().unwrap_or_else(|e| e.into_inner()).take()
}

pub(crate) fn display() -> Option<OwnedDisplayHandle> {
    DISPLAY.get().cloned()
}

pub(crate) fn preferred_dark() -> Option<bool> {
    match PREFERRED_DARK.load(Ordering::Acquire) {
        DARK_UNKNOWN => None,
        v => Some(v == DARK_YES),
    }
}

fn wake_loop() {
    if let Some(proxy) = PROXY.get() {
        proxy.wake_up();
    }
}

fn watch_desktop() {
    crate::shell::dark_preference(|dark| {
        PREFERRED_DARK.store(if dark { DARK_YES } else { DARK_NO }, Ordering::Release);
        THEME_CHANGED.store(true, Ordering::Release);
        wake_loop();
    });
    let shutdown = crate::instance::on_shutdown_signal(|signal| {
        tracing::info!("signal {signal}, saving the session");
        SHUTDOWN.store(true, Ordering::Release);
        wake_loop();
    });
    if let Err(e) = shutdown {
        tracing::warn!("cannot handle shutdown signals: {e}");
    }
}

enum Command {
    Open(WindowOptions),
    Close(WindowId),
    Paste(WindowId),
    Copy(String),
    CopyFiles(String),
    Drag(winit::window::WindowId, String),
}

type Commands = Rc<RefCell<Vec<Command>>>;

pub struct LinuxWindow {
    id: WindowId,
    raw: Arc<dyn winit::window::Window>,
    commands: Commands,
    size: Cell<(u32, u32)>,
    normal_size: Cell<(u32, u32)>,
    scale: Cell<f32>,
    cursor: Cell<CursorIcon>,
    edge: Cell<Option<ResizeDirection>>,
    timer: Cell<Option<Instant>>,
    ime_area: Cell<Rect>,
    compact_from: Cell<Option<(u32, u32)>>,
}

impl LinuxWindow {
    fn new(id: WindowId, raw: Arc<dyn winit::window::Window>, commands: Commands) -> Self {
        let size = raw.surface_size();
        Self {
            id,
            scale: Cell::new(raw.scale_factor() as f32),
            raw,
            commands,
            size: Cell::new((size.width, size.height)),
            normal_size: Cell::new((size.width, size.height)),
            cursor: Cell::new(CursorIcon::Default),
            edge: Cell::new(None),
            timer: Cell::new(None),
            ime_area: Cell::new(Rect::default()),
            compact_from: Cell::new(None),
        }
    }

    pub(crate) fn winit(&self) -> &Arc<dyn winit::window::Window> {
        &self.raw
    }

    pub(crate) fn is_wayland(&self) -> bool {
        self.raw.display_handle().is_ok_and(|h| matches!(h.as_raw(), RawDisplayHandle::Wayland(_)))
    }

    fn push(&self, command: Command) {
        self.commands.borrow_mut().push(command);
    }

    fn free_size(&self) -> bool {
        !self.raw.is_maximized() && self.raw.fullscreen().is_none() && self.compact_from.get().is_none()
    }

    fn resized(&self, size: PhysicalSize<u32>) {
        self.size.set((size.width, size.height));
        if self.free_size() {
            self.normal_size.set((size.width, size.height));
        }
    }

    fn track_edge(&self, x: f32, y: f32) {
        let (w, h) = self.size.get();
        let edge = if self.free_size() { input::resize_edge(x, y, (w as f32, h as f32), RESIZE_BORDER * self.scale.get()) } else { None };
        if edge == self.edge.replace(edge) {
            return;
        }
        let icon = edge.map_or_else(|| input::cursor(self.cursor.get()), winit::cursor::CursorIcon::from);
        self.raw.set_cursor(icon.into());
    }

    fn leave_compact(&self) {
        let Some((w, h)) = self.compact_from.take() else { return };
        self.raw.set_window_level(WindowLevel::Normal);
        let _ = self.raw.request_surface_size(PhysicalSize::new(w, h).into());
    }
}

impl Window for LinuxWindow {
    fn id(&self) -> WindowId {
        self.id
    }

    fn size(&self) -> (u32, u32) {
        self.size.get()
    }

    fn scale(&self) -> f32 {
        self.scale.get()
    }

    fn request_redraw(&self) {
        self.raw.request_redraw();
    }

    fn set_title(&self, title: &str) {
        self.raw.set_title(title);
    }

    fn set_cursor(&self, cursor: CursorIcon) {
        if self.cursor.replace(cursor) != cursor && self.edge.get().is_none() {
            self.raw.set_cursor(input::cursor(cursor).into());
        }
    }

    fn minimize(&self) {
        self.raw.set_minimized(true);
    }

    fn toggle_maximize(&self) {
        if self.compact_from.get().is_some() {
            return self.leave_compact();
        }
        if self.raw.fullscreen().is_some() {
            return self.raw.set_fullscreen(None);
        }
        self.raw.set_maximized(!self.raw.is_maximized());
    }

    fn is_maximized(&self) -> bool {
        self.raw.is_maximized()
    }

    fn close(&self) {
        self.push(Command::Close(self.id));
    }

    fn activate(&self) {
        self.raw.focus_window();
    }

    fn open_window(&self, opts: WindowOptions) {
        self.push(Command::Open(opts));
    }

    fn placement(&self) -> Placement {
        let at = self.raw.outer_position().unwrap_or_default();
        let (width, height) = self.compact_from.get().unwrap_or(self.normal_size.get());
        Placement { x: at.x, y: at.y, width: width as i32, height: height as i32, maximized: self.raw.is_maximized() }
    }

    fn clipboard_text(&self) -> Option<String> {
        self.push(Command::Paste(self.id));
        None
    }

    fn set_clipboard_text(&self, text: &str) {
        self.push(Command::Copy(text.to_owned()));
    }

    fn set_ime_area(&self, area: Rect) {
        if self.ime_area.replace(area) == area {
            return;
        }
        let data = ImeRequestData::default().with_cursor_area(PhysicalPosition::new(area.x, area.y).into(), PhysicalSize::new(area.w, area.h).into());
        let _ = self.raw.request_ime_update(ImeRequest::Update(data));
    }

    fn is_dark_mode(&self) -> bool {
        preferred_dark().or_else(|| self.raw.theme().map(|t| t == Theme::Dark)).unwrap_or(false)
    }

    fn set_clipboard_files(&self, paths: &[FsPath], cut: bool) {
        self.push(Command::CopyFiles(crate::shell::remember_files(paths, cut)));
    }

    fn shell_menu(&self, menu: ShellMenu) {
        let id = self.id;
        let start = Instant::now();
        background("flit-menu", move || {
            let (items, launches) = crate::apps::menu_with_apps(menu.menu, &menu.items);
            tracing::debug!(elapsed = ?start.elapsed(), apps = launches.len(), "menu ready");
            if !items.is_empty() {
                QUEUED.lock().unwrap_or_else(|e| e.into_inner()).push((id, Event::ShowMenu { items, x: menu.x, y: menu.y, launches }));
                wake_loop();
            }
        });
    }

    fn properties(&self, _paths: Vec<FsPath>) {}

    fn start_drag(&self, paths: Vec<FsPath>) {
        self.push(Command::Drag(self.raw.id(), crate::shell::uri_list(&paths)));
    }

    fn set_timer(&self, ms: u32) {
        self.timer.set(Some(Instant::now() + Duration::from_millis(ms.into())));
    }

    fn to_screen(&self, x: f32, y: f32) -> (i32, i32) {
        let inset = self.raw.surface_position();
        match self.raw.outer_position() {
            Ok(at) => (at.x + inset.x + x as i32, at.y + inset.y + y as i32),
            Err(_) => (x as i32, y as i32),
        }
    }

    fn toggle_fullscreen(&self) {
        let full = self.raw.fullscreen().is_some();
        self.raw.set_fullscreen((!full).then_some(Fullscreen::Borderless(None)));
    }

    fn set_dark(&self, dark: bool) {
        self.raw.set_theme(Some(if dark { Theme::Dark } else { Theme::Light }));
    }

    fn set_compact(&self, on: Option<bool>) {
        let compact = self.compact_from.get().is_some();
        if on.unwrap_or(!compact) == compact {
            return;
        }
        if compact {
            return self.leave_compact();
        }
        self.raw.set_fullscreen(None);
        self.raw.set_maximized(false);
        self.compact_from.set(Some(self.normal_size.get()));
        self.raw.set_window_level(WindowLevel::AlwaysOnTop);
        let _ = self.raw.request_surface_size(LogicalSize::new(COMPACT_SIZE.0, COMPACT_SIZE.1).into());
    }
}

#[derive(Clone)]
pub struct LinuxWaker {
    proxy: EventLoopProxy,
    pending: Arc<AtomicBool>,
}

impl Waker for LinuxWaker {
    fn wake(&self) {
        if !self.pending.swap(true, Ordering::AcqRel) {
            self.proxy.wake_up();
        }
    }
}

pub struct LinuxWindows {
    event_loop: EventLoop,
    pending: Arc<AtomicBool>,
}

fn library_loads(name: &CStr) -> bool {
    let handle = unsafe { libc::dlopen(name.as_ptr(), libc::RTLD_LAZY | libc::RTLD_LOCAL) };
    !handle.is_null() && unsafe { libc::dlclose(handle) } == 0
}

impl WindowSystem for LinuxWindows {
    type Window = LinuxWindow;
    type Waker = LinuxWaker;

    fn new() -> Result<Self, PlatformError> {
        if std::env::var_os("WAYLAND_DISPLAY").is_none() && !library_loads(X11_KEYBOARD_LIBRARY) {
            return Err(PlatformError::new(-1, "Flit on X11 needs libxkbcommon-x11.so.0 (package libxkbcommon-x11-0 on Debian and Ubuntu)"));
        }
        let event_loop = EventLoop::new().map_err(|e| PlatformError::new(-1, format!("cannot connect to the display server: {e}")))?;
        let _ = DISPLAY.set(event_loop.owned_display_handle());
        let _ = PROXY.set(event_loop.create_proxy());
        watch_desktop();
        Ok(Self { event_loop, pending: Arc::default() })
    }

    fn waker(&self) -> LinuxWaker {
        LinuxWaker { proxy: self.event_loop.create_proxy(), pending: self.pending.clone() }
    }

    fn run<H: WindowHandler<Self>>(mut self, first: WindowOptions, handler: H) -> Result<(), PlatformError> {
        let commands: Commands = Rc::new(RefCell::new(vec![Command::Open(first)]));
        let mut app = Loop { handler, nodes: Vec::new(), commands, pending: self.pending.clone(), next_id: 1, mods: ModifiersState::empty(), started: false, error: None, incoming: None };
        self.event_loop.run_app_on_demand(&mut app).map_err(|e| PlatformError::new(-1, format!("event loop: {e}")))?;
        app.error.map_or(Ok(()), Err)
    }
}

struct Node {
    window: LinuxWindow,
    clicks: Clicks,
}

struct Loop<H> {
    handler: H,
    nodes: Vec<Node>,
    commands: Commands,
    pending: Arc<AtomicBool>,
    next_id: WindowId,
    mods: ModifiersState,
    started: bool,
    error: Option<PlatformError>,
    incoming: Option<Incoming>,
}

fn platform_attributes(el: &dyn ActiveEventLoop) -> Box<dyn PlatformWindowAttributes> {
    if el.is_wayland() {
        Box::new(WindowAttributesWayland::default().with_name(APP_ID, APP_ID))
    } else {
        Box::new(WindowAttributesX11::default().with_name(APP_ID, APP_ID))
    }
}

fn enable_ime(raw: &dyn winit::window::Window) {
    let data = ImeRequestData::default().with_cursor_area(PhysicalPosition::new(0, 0).into(), PhysicalSize::new(0, 0).into());
    if let Some(request) = ImeEnableRequest::new(ImeCapabilities::new().with_cursor_area(), data) {
        let _ = raw.request_ime_update(ImeRequest::Enable(request));
    }
}

fn background(name: &str, work: impl FnOnce() + Send + 'static) {
    if let Err(e) = std::thread::Builder::new().name(name.into()).spawn(work) {
        tracing::warn!("cannot start {name}: {e}");
    }
}

fn paste(el: &dyn ActiveEventLoop, window: WindowId) {
    match el.clipboard() {
        Ok(Some(id)) => {
            if let Err(e) = el.fetch_data_transfer(id, &TypeHint::Plaintext) {
                tracing::debug!("clipboard has no text: {e}");
            }
        }
        Ok(None) => {}
        Err(e) => {
            tracing::debug!("winit cannot read the clipboard ({e}), asking a helper program");
            background("flit-paste", move || {
                if let Some(text) = crate::shell::clipboard_text() {
                    QUEUED.lock().unwrap_or_else(|e| e.into_inner()).push((window, Event::Pasted(text)));
                    wake_loop();
                }
            });
        }
    }
}

fn copy(el: &dyn ActiveEventLoop, text: String) {
    let data = DataTransferSendBuilder::new(text.clone()).with_type(TypeHint::Plaintext, |text: &String, _| Some(text.clone())).build();
    if let Err(e) = el.set_clipboard(data) {
        tracing::debug!("winit cannot set the clipboard ({e}), asking a helper program");
        background("flit-copy", move || {
            if !crate::shell::set_clipboard_text(&text) {
                tracing::warn!("cannot set the clipboard: no wl-copy, xclip or xsel");
            }
        });
    }
}

fn drag_out(el: &dyn ActiveEventLoop, source: winit::window::WindowId, list: String) {
    let data = DataTransferSendBuilder::new(list).with_type(TypeHint::UriList, |list: &String, _| Some(list.clone())).build();
    if let Err(e) = el.start_drag(source, data, &[DndAction::Copy, DndAction::Move], None) {
        tracing::info!("cannot drag files out of Flit here: {e}");
    }
}

struct Incoming {
    id: DataTransferId,
    at: (f32, f32),
    entered: bool,
    dropped: bool,
}

fn dnd_actions(effect: u32) -> Vec<DndAction> {
    let mut out = Vec::new();
    if effect & drop_effect::COPY != 0 {
        out.push(DndAction::Copy);
    }
    if effect & drop_effect::MOVE != 0 {
        out.push(DndAction::Move);
    }
    out
}

fn copy_files(el: &dyn ActiveEventLoop, list: String) {
    let data = DataTransferSendBuilder::new(list.clone())
        .with_type(TypeHint::UriList, |list: &String, _| Some(list.clone()))
        .with_type(TypeHint::Plaintext, |list: &String, _| Some(list.clone()))
        .build();
    if let Err(e) = el.set_clipboard(data) {
        tracing::debug!("winit cannot set the file clipboard ({e}), asking a helper program");
        background("flit-copy", move || {
            if !crate::shell::set_clipboard_files(&list) {
                tracing::info!("the file clipboard stays inside Flit: no wl-copy or xclip");
            }
        });
    }
}

impl<H: WindowHandler<LinuxWindows>> Loop<H> {
    fn settle(&mut self, el: &dyn ActiveEventLoop) {
        if !self.started {
            return;
        }
        loop {
            let commands = std::mem::take(&mut *self.commands.borrow_mut());
            if commands.is_empty() {
                break;
            }
            for command in commands {
                match command {
                    Command::Open(opts) => self.open(el, opts),
                    Command::Close(id) => self.close(id),
                    Command::Paste(id) => paste(el, id),
                    Command::Copy(text) => copy(el, text),
                    Command::CopyFiles(list) => copy_files(el, list),
                    Command::Drag(source, list) => drag_out(el, source, list),
                }
            }
        }
        if self.nodes.is_empty() {
            el.exit();
        }
    }

    fn open(&mut self, el: &dyn ActiveEventLoop, opts: WindowOptions) {
        let mut attrs = WindowAttributes::default()
            .with_title(opts.title)
            .with_decorations(false)
            .with_min_surface_size(LogicalSize::new(MIN_SIZE.0, MIN_SIZE.1))
            .with_theme(Some(if opts.dark { Theme::Dark } else { Theme::Light }))
            .with_platform_attributes(platform_attributes(el));
        attrs = match opts.placement.filter(|p| p.width > 100 && p.height > 100) {
            Some(p) => attrs.with_surface_size(PhysicalSize::new(p.width as u32, p.height as u32)).with_position(PhysicalPosition::new(p.x, p.y)).with_maximized(p.maximized),
            None => attrs.with_surface_size(LogicalSize::new(DEFAULT_SIZE.0, DEFAULT_SIZE.1)),
        };
        let raw: Arc<dyn winit::window::Window> = match el.create_window(attrs) {
            Ok(raw) => Arc::from(raw),
            Err(e) => {
                tracing::error!("cannot open window: {e}");
                self.error.get_or_insert_with(|| PlatformError::new(-1, format!("cannot open a window: {e}")));
                return;
            }
        };
        enable_ime(&*raw);
        let id = self.next_id;
        self.next_id += 1;
        self.nodes.push(Node { window: LinuxWindow::new(id, raw, self.commands.clone()), clicks: Clicks::default() });
        let node = self.nodes.last().unwrap();
        self.handler.created(&node.window);
    }

    fn close(&mut self, id: WindowId) {
        let Some(i) = self.nodes.iter().position(|n| n.window.id == id) else { return };
        let node = self.nodes.remove(i);
        self.handler.destroyed(&node.window);
    }

    fn fire_timers(&mut self) {
        let now = Instant::now();
        for node in &self.nodes {
            if node.window.timer.get().is_some_and(|at| at <= now) {
                node.window.timer.set(None);
                self.handler.event(&node.window, Event::Timer);
            }
        }
    }

    fn button(&mut self, i: usize, pressed: bool, position: PhysicalPosition<f64>, button: MouseButton) {
        let (handler, node) = (&mut self.handler, &mut self.nodes[i]);
        let w = &node.window;
        let (x, y) = (position.x as f32, position.y as f32);
        if !pressed {
            return handler.event(w, Event::MouseUp { x, y, button });
        }
        if let (MouseButton::Left, Some(edge)) = (button, w.edge.get()) {
            let _ = w.raw.drag_resize_window(edge);
            return;
        }
        let clicks = node.clicks.press(button, Instant::now(), x, y, w.scale.get());
        match (handler.hit_test(w, x, y), button) {
            (HitArea::Caption, MouseButton::Left) if clicks == 2 => w.toggle_maximize(),
            (HitArea::Caption, MouseButton::Left) => {
                let _ = w.raw.drag_window();
            }
            (HitArea::Caption, MouseButton::Right) => w.raw.show_window_menu(position.into()),
            _ => handler.event(w, Event::MouseDown { x, y, button, clicks }),
        }
    }

    fn drag_in(&mut self, el: &dyn ActiveEventLoop, i: usize, event: WindowEvent) -> Option<WindowEvent> {
        let (handler, w, mods) = (&mut self.handler, &self.nodes[i].window, input::mods(self.mods));
        let all = drop_effect::COPY | drop_effect::MOVE;
        let mut deliver = |ev: DragEvent, id: DataTransferId| {
            let effect = handler.drag(w, ev).effect;
            let _ = el.set_valid_dnd_actions(id, &dnd_actions(effect));
        };
        match event {
            WindowEvent::DragEntered { id, position } => {
                let at = position.map_or((0.0, 0.0), |p| (p.x as f32, p.y as f32));
                self.incoming = Some(Incoming { id, at, entered: false, dropped: false });
                let _ = el.set_valid_dnd_actions(id, &[DndAction::Copy, DndAction::Move]);
                if let Err(e) = el.fetch_data_transfer(id, &TypeHint::UriList) {
                    tracing::debug!("the dragged data has no file list: {e}");
                }
            }
            WindowEvent::DataTransferReceived { id, value, .. } if self.incoming.as_ref().is_some_and(|d| d.id == id && !d.entered) => {
                let d = self.incoming.as_mut()?;
                let paths = value.try_as_bytes().map(|b| crate::shell::parse_file_list(&b).0).unwrap_or_default();
                d.entered = true;
                let (at, dropped) = (d.at, d.dropped);
                deliver(DragEvent::Enter { paths, x: at.0, y: at.1, mods, allowed: all }, id);
                if dropped {
                    deliver(DragEvent::Drop { x: at.0, y: at.1, mods, allowed: all }, id);
                    self.incoming = None;
                }
            }
            WindowEvent::DragPosition { id, position, .. } => {
                let d = self.incoming.as_mut().filter(|d| d.id == id)?;
                d.at = (position.x as f32, position.y as f32);
                if d.entered {
                    let at = d.at;
                    deliver(DragEvent::Over { x: at.0, y: at.1, mods, allowed: all }, id);
                }
            }
            WindowEvent::DragDropped { id, .. } => {
                let d = self.incoming.as_mut().filter(|d| d.id == id)?;
                if !d.entered {
                    d.dropped = true;
                    return None;
                }
                let at = d.at;
                self.incoming = None;
                deliver(DragEvent::Drop { x: at.0, y: at.1, mods, allowed: all }, id);
            }
            WindowEvent::DragLeft { id } => {
                if self.incoming.take().is_some_and(|d| d.id == id && d.entered) {
                    deliver(DragEvent::Leave, id);
                }
            }
            other => return Some(other),
        }
        None
    }

    fn dispatch(&mut self, i: usize, event: WindowEvent) {
        let (handler, node) = (&mut self.handler, &mut self.nodes[i]);
        let w = &node.window;
        match event {
            WindowEvent::RedrawRequested => handler.redraw(w),
            WindowEvent::SurfaceResized(size) => {
                w.resized(size);
                handler.event(w, Event::Resized { width: size.width, height: size.height });
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                w.scale.set(scale_factor as f32);
                handler.event(w, Event::ScaleChanged(scale_factor as f32));
            }
            WindowEvent::CloseRequested => handler.event(w, Event::CloseRequested),
            WindowEvent::Focused(focused) => {
                node.clicks.reset();
                handler.event(w, Event::Focus(focused));
            }
            WindowEvent::ModifiersChanged(m) => self.mods = m.state(),
            WindowEvent::KeyboardInput { event, is_synthetic: false, .. } => {
                let pressed = event.state == ElementState::Pressed;
                handler.event(w, Event::Key { key: input::key(event.physical_key, &event.key_without_modifiers), mods: input::mods(self.mods), pressed });
                for c in event.text.iter().filter(|_| pressed).flat_map(|t| t.chars()) {
                    handler.event(w, Event::Char(c));
                }
            }
            WindowEvent::Ime(Ime::Preedit(text, _)) => handler.event(w, Event::Preedit(text)),
            WindowEvent::Ime(Ime::Commit(text)) => {
                for c in text.chars() {
                    handler.event(w, Event::Char(c));
                }
            }
            WindowEvent::PointerMoved { position, .. } => {
                let (x, y) = (position.x as f32, position.y as f32);
                w.track_edge(x, y);
                handler.event(w, Event::MouseMove { x, y });
            }
            WindowEvent::PointerLeft { .. } => {
                w.edge.set(None);
                handler.event(w, Event::MouseLeave);
            }
            WindowEvent::ActivationTokenDone { token, .. } => *TOKEN.lock().unwrap_or_else(|e| e.into_inner()) = Some(token.into_raw()),
            WindowEvent::PointerButton { state, position, button, .. } => {
                if state == ElementState::Pressed {
                    let _ = w.raw.request_activation_token();
                }
                if let Some(button) = button.mouse_button().and_then(input::mouse_button) {
                    self.button(i, state == ElementState::Pressed, position, button);
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let (dx, dy) = input::wheel(delta, w.scale.get());
                handler.event(w, Event::Wheel { dx, dy });
            }
            WindowEvent::ThemeChanged(theme) => handler.event(w, Event::ThemeChanged { dark: theme == Theme::Dark }),
            WindowEvent::DataTransferReceived { value, .. } => match value.try_as_string() {
                Ok(text) => handler.event(w, Event::Pasted(text)),
                Err(e) => tracing::debug!("clipboard text is unreadable: {e}"),
            },
            _ => {}
        }
    }
}

impl<H: WindowHandler<LinuxWindows>> ApplicationHandler for Loop<H> {
    fn can_create_surfaces(&mut self, el: &dyn ActiveEventLoop) {
        self.started = true;
        self.settle(el);
    }

    fn proxy_wake_up(&mut self, el: &dyn ActiveEventLoop) {
        self.pending.store(false, Ordering::Release);
        for node in &self.nodes {
            self.handler.woken(&node.window);
        }
        if THEME_CHANGED.swap(false, Ordering::AcqRel) {
            let dark = preferred_dark().unwrap_or(false);
            for node in &self.nodes {
                self.handler.event(&node.window, Event::ThemeChanged { dark });
            }
        }
        let queued = std::mem::take(&mut *QUEUED.lock().unwrap_or_else(|e| e.into_inner()));
        for (id, ev) in queued {
            if let Some(node) = self.nodes.iter().find(|n| n.window.id == id) {
                self.handler.event(&node.window, ev);
            }
        }
        if SHUTDOWN.swap(false, Ordering::AcqRel) {
            for node in &self.nodes {
                self.handler.event(&node.window, Event::SessionEnding);
                node.window.close();
            }
        }
        self.settle(el);
    }

    fn window_event(&mut self, el: &dyn ActiveEventLoop, id: winit::window::WindowId, event: WindowEvent) {
        if let Some(i) = self.nodes.iter().position(|n| n.window.raw.id() == id) {
            if let Some(event) = self.drag_in(el, i, event) {
                self.dispatch(i, event);
            }
        }
        self.settle(el);
    }

    fn about_to_wait(&mut self, el: &dyn ActiveEventLoop) {
        self.fire_timers();
        self.settle(el);
        let next = self.nodes.iter().filter_map(|n| n.window.timer.get()).min();
        el.set_control_flow(next.map_or(ControlFlow::Wait, ControlFlow::WaitUntil));
    }
}
