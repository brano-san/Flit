use crate::apps::OpenWith;
use crate::input::{self, Clicks};
use flit_core::input::{CursorIcon, Event, HitArea, Mods, MouseButton, WindowButtons};
use flit_core::preview::{HostCmd, Hosted, MediaCmd};
use flit_core::render::Rect;
use flit_core::ui::{drop_effect, Bitmap, DragEvent, ShellMenu};
use flit_core::i18n::S;
use flit_core::FsPath;
use flit_platform::{Placement, PlatformError, Waker, Window, WindowHandler, WindowId, WindowOptions, WindowSystem};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::CStr;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::data_transfer::{DataTransferId, DataTransferSendBuilder, TransferType, TypeHint};
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, Ime, WindowEvent};
use winit::event_loop::run_on_demand::EventLoopExtRunOnDemand;
use winit::event_loop::{ActiveEventLoop, AsyncRequestSerial, ControlFlow, DndAction, EventLoop, EventLoopProxy, OwnedDisplayHandle};
use winit::keyboard::ModifiersState;
use winit::platform::wayland::{ActiveEventLoopExtWayland, WindowAttributesWayland};
use winit::platform::startup_notify::WindowExtStartupNotify;
use winit::platform::x11::WindowAttributesX11;
use winit::raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};
use winit::monitor::Fullscreen;
use winit::window::{ActivationToken, ImeCapabilities, ImeEnableRequest, ImeRequest, ImeRequestData, PlatformWindowAttributes, ResizeDirection, Theme, UserAttentionType, WindowAttributes, WindowLevel};

const APP_ID: &str = "flit";
const DEFAULT_SIZE: (f64, f64) = (1100.0, 720.0);
const MIN_SIZE: (f64, f64) = (520.0, 320.0);
const COMPACT_SIZE: (f64, f64) = (560.0, 380.0);
const RESIZE_BORDER: f32 = 5.0;
const X11_KEYBOARD_LIBRARY: &CStr = c"libxkbcommon-x11.so.0";
const READ_WAIT: Duration = Duration::from_secs(1);
const VERIFY_WAIT: Duration = Duration::from_millis(400);
const TAB_WINDOWS_WAIT: Duration = Duration::from_millis(250);

const DARK_UNKNOWN: u8 = 0;
const DARK_NO: u8 = 1;
const DARK_YES: u8 = 2;
const ANIM_YES: u8 = 1;
const ANIM_NO: u8 = 0;
static ANIMATIONS: AtomicU8 = AtomicU8::new(ANIM_YES);
static ANIMATIONS_DIRTY: AtomicBool = AtomicBool::new(false);
static MOUSE_DIRTY: AtomicBool = AtomicBool::new(false);
const TEXT_UNSEEN: u8 = 0;
const TEXT_NO: u8 = 1;
const TEXT_YES: u8 = 2;
static CLIPBOARD_TEXT: AtomicU8 = AtomicU8::new(TEXT_UNSEEN);
const LAUNCHER_STEP: u32 = 5;
const LAUNCHER_HIDDEN: u32 = 0;
static LAUNCHER_SENT: AtomicU32 = AtomicU32::new(LAUNCHER_HIDDEN);

pub(crate) fn launcher_step(percent: Option<u32>) -> Option<u32> {
    percent.map(|v| v.min(100) / LAUNCHER_STEP * LAUNCHER_STEP)
}

pub(crate) fn animations() -> bool {
    ANIMATIONS.load(Ordering::Acquire) == ANIM_YES
}

static DISPLAY: OnceLock<OwnedDisplayHandle> = OnceLock::new();
static PROXY: OnceLock<EventLoopProxy> = OnceLock::new();
static PREFERRED_DARK: AtomicU8 = AtomicU8::new(DARK_UNKNOWN);
static THEME_CHANGED: AtomicBool = AtomicBool::new(false);
static SHUTDOWN: AtomicBool = AtomicBool::new(false);
static A11Y_REFRESH: AtomicBool = AtomicBool::new(false);
static DEVICES_CHANGED: AtomicBool = AtomicBool::new(false);
static QUEUED: Mutex<Vec<(WindowId, Event)>> = Mutex::new(Vec::new());
static ENCODED_IMAGE: Mutex<Option<Vec<u8>>> = Mutex::new(None);
static READS: Mutex<Vec<crossbeam_channel::Sender<Option<crate::clipboard::Payload>>>> = Mutex::new(Vec::new());
static STARTUP: Mutex<Option<String>> = Mutex::new(None);
static PREFETCHED: Mutex<Option<(Vec<FsPath>, OpenWith)>> = Mutex::new(None);

pub(crate) static LOOP_RUNNING: AtomicBool = AtomicBool::new(false);
pub(crate) static TURNS: AtomicU64 = AtomicU64::new(0);

pub(crate) fn turned_since(running: &AtomicBool, turns: &AtomicU64, seen: u64, wait: Duration) -> bool {
    let until = Instant::now() + wait;
    while running.load(Ordering::Acquire) && turns.load(Ordering::Acquire) == seen {
        if Instant::now() >= until {
            return false;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    true
}

#[cfg(test)]
pub(crate) static STARTUP_TOKEN_USERS: Mutex<()> = Mutex::new(());

pub(crate) fn activation_token() -> Option<String> {
    crate::activation::take()
}

pub(crate) fn clipboard_types() -> Option<crate::clipboard::Payload> {
    if !LOOP_RUNNING.load(Ordering::Acquire) {
        return None;
    }
    let (tx, rx) = crossbeam_channel::bounded(1);
    READS.lock().unwrap_or_else(|e| e.into_inner()).push(tx);
    wake_loop();
    rx.recv_timeout(READ_WAIT).ok().flatten()
}

pub(crate) fn startup_token(token: String) {
    *STARTUP.lock().unwrap_or_else(|e| e.into_inner()) = Some(token).filter(|t| !t.is_empty());
}

pub(crate) fn take_startup_token() -> Option<String> {
    STARTUP.lock().unwrap_or_else(|e| e.into_inner()).take()
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

pub(crate) fn loop_failure(e: winit::error::EventLoopError) -> PlatformError {
    match e {
        winit::error::EventLoopError::ExitFailure(code) => PlatformError::new(i64::from(code), "the connection to the display server was lost; the error logged just before this line says why"),
        other => PlatformError::new(-1, format!("event loop: {other}")),
    }
}

pub(crate) fn devices_changed() {
    DEVICES_CHANGED.store(true, Ordering::Release);
    wake_loop();
}

fn wake_loop() {
    if let Some(proxy) = PROXY.get() {
        proxy.wake_up();
    }
}

static ACCENT: Mutex<Option<flit_core::theme::Accent>> = Mutex::new(None);
static TEXT_SCALE_MILLI: AtomicU32 = AtomicU32::new(1000);
static LOOK_DIRTY: AtomicBool = AtomicBool::new(false);
static ICONS_DIRTY: AtomicBool = AtomicBool::new(false);
static WINDOW_BUTTONS: Mutex<Option<WindowButtons>> = Mutex::new(None);
static BUTTONS_DIRTY: AtomicBool = AtomicBool::new(false);

fn window_buttons() -> WindowButtons {
    WINDOW_BUTTONS.lock().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_default()
}

fn set_window_buttons(buttons: WindowButtons) {
    let mut current = WINDOW_BUTTONS.lock().unwrap_or_else(|e| e.into_inner());
    if current.as_ref() != Some(&buttons) {
        *current = Some(buttons);
        BUTTONS_DIRTY.store(true, Ordering::Release);
        wake_loop();
    }
}

fn system_accent() -> Option<flit_core::theme::Accent> {
    *ACCENT.lock().unwrap_or_else(|e| e.into_inner())
}

pub(crate) fn text_scale(wayland: bool, milli: u32) -> f32 {
    if wayland { milli as f32 / 1000.0 } else { 1.0 }
}

fn apply_look(change: crate::atspi::LookChange) {
    match change {
        crate::atspi::LookChange::Accent(a) => *ACCENT.lock().unwrap_or_else(|e| e.into_inner()) = a,
        crate::atspi::LookChange::TextScale(f) => TEXT_SCALE_MILLI.store((f * 1000.0).round() as u32, Ordering::Release),
        crate::atspi::LookChange::IconTheme(name) => {
            if crate::icons::switch_theme(&name) {
                ICONS_DIRTY.store(true, Ordering::Release);
            }
        }
        crate::atspi::LookChange::WindowButtons(layout) => {
            if crate::preferences::Desktop::current() == crate::preferences::Desktop::Gnome {
                set_window_buttons(crate::preferences::gnome_buttons(&layout));
            }
            return;
        }
    }
    LOOK_DIRTY.store(true, Ordering::Release);
    wake_loop();
}

fn watch_desktop() {
    crate::shell::dark_preference(
        |dark| {
            PREFERRED_DARK.store(if dark { DARK_YES } else { DARK_NO }, Ordering::Release);
            THEME_CHANGED.store(true, Ordering::Release);
            wake_loop();
        },
        |ms| {
            if !input::kde_desktop() && u32::try_from(ms).is_ok_and(input::probed_double_click) {
                MOUSE_DIRTY.store(true, Ordering::Release);
                wake_loop();
            }
        },
        apply_look,
    );
    let _ = std::thread::Builder::new().name("flit-prefs".into()).spawn(|| {
        if crate::preferences::Desktop::current() == crate::preferences::Desktop::Gnome {
            if let Some(layout) = crate::shell::probe_window_buttons() {
                set_window_buttons(crate::preferences::gnome_buttons(&layout));
            }
        }
        if let Some(accent) = crate::shell::probe_accent() {
            apply_look(crate::atspi::LookChange::Accent(Some(accent)));
        }
        if let Some(factor) = crate::shell::probe_text_scale() {
            apply_look(crate::atspi::LookChange::TextScale(factor));
        }
        if let Some(on) = crate::shell::probe_animations() {
            ANIMATIONS.store(if on { ANIM_YES } else { ANIM_NO }, Ordering::Release);
            ANIMATIONS_DIRTY.store(true, Ordering::Release);
            wake_loop();
        }
        if input::kde_desktop() {
            return;
        }
        if crate::shell::probe_double_click().is_some_and(input::probed_double_click) {
            MOUSE_DIRTY.store(true, Ordering::Release);
            wake_loop();
        }
    });
    let shutdown = crate::instance::on_shutdown_signal(|signal| {
        tracing::info!("signal {signal}, saving the session");
        SHUTDOWN.store(true, Ordering::Release);
        wake_loop();
    });
    if let Err(e) = shutdown {
        tracing::warn!("cannot handle shutdown signals: {e}");
    }
    crate::atspi::start(
        || {
            A11Y_REFRESH.store(true, Ordering::Release);
            wake_loop();
        },
        |id, event| {
            QUEUED.lock().unwrap_or_else(|e| e.into_inner()).push((id, event));
            wake_loop();
        },
    );
}

fn publish_a11y<H: WindowHandler<LinuxWindows>>(handler: &mut H, w: &LinuxWindow) {
    if crate::atspi::listening() {
        if let Some(tree) = handler.accessibility(w) {
            crate::atspi::publish(w.id, w.to_screen(0.0, 0.0), tree);
        }
    }
}

enum Command {
    Open(WindowOptions),
    Close(WindowId),
    Paste(WindowId),
    Copy(String),
    CopyFiles(Vec<FsPath>, bool),
    CopyImage(Bitmap),
    SetPrimary(String),
    PastePrimary,
    Drag(winit::window::WindowId, Vec<FsPath>, f32),
    Activate(WindowId, String),
}

type Commands = Rc<RefCell<Vec<Command>>>;

#[derive(Clone, Copy)]
struct TabTarget {
    id: WindowId,
    origin: (f64, f64),
    size: (u32, u32),
    minimized: bool,
}

fn tab_target(windows: impl DoubleEndedIterator<Item = TabTarget>, source: WindowId, point: (f64, f64)) -> Option<(WindowId, f32, f32)> {
    if !point.0.is_finite() || !point.1.is_finite() {
        return None;
    }
    windows.rev().find_map(|w| {
        let (x, y) = (point.0 - w.origin.0, point.1 - w.origin.1);
        (w.id != source && !w.minimized && x >= 0.0 && y >= 0.0 && x < f64::from(w.size.0) && y < f64::from(w.size.1)).then_some((w.id, x as f32, y as f32))
    })
}

struct MinimizedState(AtomicU64);

impl MinimizedState {
    fn new(hidden: bool) -> Self {
        Self(AtomicU64::new(u64::from(hidden)))
    }

    fn get(&self) -> bool {
        self.snapshot() & 1 != 0
    }

    fn snapshot(&self) -> u64 {
        self.0.load(Ordering::Acquire)
    }

    fn set(&self, hidden: bool) {
        let mut seen = self.snapshot();
        loop {
            let next = (seen.wrapping_add(2) & !1) | u64::from(hidden);
            match self.0.compare_exchange_weak(seen, next, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => return,
                Err(current) => seen = current,
            }
        }
    }

    fn complete(&self, seen: u64, hidden: bool) {
        let _ = self.0.compare_exchange(seen, (seen & !1) | u64::from(hidden), Ordering::AcqRel, Ordering::Acquire);
    }
}
struct TrackedWindow {
    target: TabTarget,
    raw: Weak<dyn winit::window::Window>,
    minimized: Arc<MinimizedState>,
}

type MinimizedCheck = (Arc<dyn winit::window::Window>, Arc<MinimizedState>, u64);

struct TabWindows {
    windows: Vec<TrackedWindow>,
    checked: Option<Instant>,
    check: mpsc::SyncSender<Vec<MinimizedCheck>>,
}

fn tab_windows_due(checked: Option<Instant>, now: Instant) -> bool {
    checked.is_none_or(|at| now.saturating_duration_since(at) >= TAB_WINDOWS_WAIT)
}

impl TabWindows {
    fn new() -> Self {
        let (check, pending) = mpsc::sync_channel::<Vec<MinimizedCheck>>(1);
        background("flit-tab-windows", move || {
            while let Ok(windows) = pending.recv() {
                for (raw, minimized, seen) in windows {
                    if let Some(hidden) = raw.is_minimized() {
                        minimized.complete(seen, hidden);
                    }
                }
            }
        });
        Self { windows: Vec::new(), checked: None, check }
    }

    fn refresh(&mut self, now: Instant) {
        if !tab_windows_due(self.checked, now) {
            return;
        }
        self.checked = Some(now);
        let windows = self.windows.iter().filter_map(|w| Some((w.raw.upgrade()?, w.minimized.clone(), w.minimized.snapshot()))).collect();
        let _ = self.check.try_send(windows);
    }
}

pub struct LinuxWindow {
    id: WindowId,
    raw: Arc<dyn winit::window::Window>,
    commands: Commands,
    targets: Rc<RefCell<TabWindows>>,
    size: Cell<(u32, u32)>,
    normal_size: Cell<(u32, u32)>,
    scale: Cell<f32>,
    cursor: Cell<CursorIcon>,
    edge: Cell<Option<ResizeDirection>>,
    timer: Cell<Option<Instant>>,
    ime_area: Cell<Rect>,
    compact_from: Cell<Option<(u32, u32)>>,
    media: RefCell<Option<ShownMedia>>,
    player: RefCell<Option<crate::player::Player>>,
    volume: Cell<Option<f32>>,
}

#[derive(Clone)]
struct ShownMedia {
    gen: u64,
    video: bool,
    path: FsPath,
}

impl LinuxWindow {
    fn new(id: WindowId, raw: Arc<dyn winit::window::Window>, commands: Commands, targets: Rc<RefCell<TabWindows>>) -> Self {
        let size = raw.surface_size();
        let window = Self {
            id,
            scale: Cell::new(raw.scale_factor() as f32),
            raw,
            commands,
            targets,
            size: Cell::new((size.width, size.height)),
            normal_size: Cell::new((size.width, size.height)),
            cursor: Cell::new(CursorIcon::Default),
            edge: Cell::new(None),
            timer: Cell::new(None),
            ime_area: Cell::new(Rect::default()),
            compact_from: Cell::new(None),
            media: RefCell::new(None),
            player: RefCell::new(None),
            volume: Cell::new(None),
        };
        window.cache_target(None);
        window
    }

    fn cache_target(&self, position: Option<PhysicalPosition<i32>>) {
        if self.is_wayland() {
            return;
        }
        let Some(at) = position.or_else(|| self.raw.outer_position().ok()) else { return };
        let inset = self.raw.surface_position();
        let target = TabTarget { id: self.id, origin: (f64::from(at.x) + f64::from(inset.x), f64::from(at.y) + f64::from(inset.y)), size: self.size.get(), minimized: self.raw.is_minimized().unwrap_or(false) };
        let mut targets = self.targets.borrow_mut();
        if let Some(w) = targets.windows.iter_mut().find(|w| w.target.id == self.id) {
            w.target = target;
            w.minimized.set(target.minimized);
        } else {
            targets.windows.push(TrackedWindow { target, raw: Arc::downgrade(&self.raw), minimized: Arc::new(MinimizedState::new(target.minimized)) });
        }
    }

    fn focused(&self) {
        let mut targets = self.targets.borrow_mut();
        if let Some(i) = targets.windows.iter().position(|w| w.target.id == self.id) {
            let window = targets.windows.remove(i);
            targets.windows.push(window);
        }
    }

    fn stop_player(&self) {
        if let Some(player) = self.player.borrow_mut().take() {
            player.stop();
        }
    }

    fn tell_player(&self, line: &str, signal: Option<libc::c_int>) -> bool {
        let player = self.player.borrow();
        let Some(p) = player.as_ref().filter(|p| p.alive()) else { return false };
        p.send(line) || signal.is_some_and(|s| p.signal(s))
    }

    fn play(&self) {
        if self.tell_player(&crate::player::pause(false), Some(libc::SIGCONT)) {
            return;
        }
        self.stop_player();
        let Some(m) = self.media.borrow().clone() else { return };
        let Some(file) = flit_platform_linux::host(&m.path) else { return };
        let socket = std::env::var_os("XDG_RUNTIME_DIR").map(|d| crate::player::socket_path(std::path::Path::new(&d), m.gen));
        match crate::shell::play_media(m.video, file, socket.as_deref(), self.volume.get()) {
            Ok((child, ipc)) => {
                let id = self.id;
                let report = move |e: flit_core::preview::HostEvent| {
                    QUEUED.lock().unwrap_or_else(|e| e.into_inner()).push((id, Event::Hosted(e)));
                    wake_loop();
                };
                self.player.replace(Some(crate::player::Player::start(child, socket.filter(|_| ipc), m.gen, report)));
            }
            Err(error) => {
                let failed = flit_core::preview::HostEvent::Failed { gen: m.gen, message: flit_core::i18n::localize(&error.message).into_owned() };
                QUEUED.lock().unwrap_or_else(|e| e.into_inner()).push((self.id, Event::Hosted(failed)));
                wake_loop();
            }
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

    fn restore_cursor(&self) {
        self.edge.set(None);
        self.raw.set_cursor(input::cursor(self.cursor.get()).into());
    }

    fn leave_compact(&self) {
        let Some((w, h)) = self.compact_from.take() else { return };
        self.raw.set_window_level(WindowLevel::Normal);
        let _ = self.raw.request_surface_size(PhysicalSize::new(w, h).into());
    }
}

impl Drop for LinuxWindow {
    fn drop(&mut self) {
        self.targets.borrow_mut().windows.retain(|w| w.target.id != self.id);
        self.stop_player();
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
        if let Some(w) = self.targets.borrow().windows.iter().find(|w| w.target.id == self.id) {
            w.minimized.set(true);
        }
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
        let token = take_startup_token().or_else(|| self.is_wayland().then(crate::activation::take_ready).flatten());
        match token {
            Some(t) => self.push(Command::Activate(self.id, t)),
            None if self.is_wayland() && !self.raw.has_focus() => self.raw.request_user_attention(Some(UserAttentionType::Informational)),
            None => self.raw.focus_window(),
        }
    }

    fn request_attention(&self) {
        if !self.raw.has_focus() {
            self.raw.request_user_attention(Some(UserAttentionType::Informational));
        }
    }

    fn set_taskbar_progress(&self, progress: Option<(u32, bool)>) {
        let percent = launcher_step(progress.map(|(v, _)| v));
        let sent = percent.map_or(LAUNCHER_HIDDEN, |v| v + 1);
        if LAUNCHER_SENT.swap(sent, Ordering::AcqRel) != sent {
            crate::shell::launcher_progress(percent);
        }
    }

    fn notify(&self, text: &str) {
        crate::alert::notification(text);
    }

    fn host(&self, cmd: HostCmd) {
        match cmd {
            HostCmd::Show { gen, what: Hosted::Media { video }, path, .. } => {
                self.stop_player();
                self.media.replace(Some(ShownMedia { gen, video, path }));
            }
            HostCmd::Show { .. } | HostCmd::Close => {
                self.stop_player();
                self.media.replace(None);
            }
            HostCmd::Place(_) => {}
            HostCmd::Media(MediaCmd::Seek(at)) => {
                self.tell_player(&crate::player::seek(at), None);
            }
            HostCmd::Media(MediaCmd::Volume(v)) => {
                self.volume.set(Some(v));
                self.tell_player(&crate::player::volume(v), None);
            }
            HostCmd::Media(MediaCmd::Play) => self.play(),
            HostCmd::Media(MediaCmd::Pause) => {
                self.tell_player(&crate::player::pause(true), Some(libc::SIGSTOP));
            }
        }
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

    fn set_primary_text(&self, text: &str) {
        self.push(Command::SetPrimary(text.to_owned()));
    }

    fn primary_text(&self) -> Option<String> {
        self.push(Command::PastePrimary);
        None
    }

    fn set_clipboard_image(&self, image: &Bitmap) {
        self.push(Command::CopyImage(image.clone()));
    }

    fn set_ime_area(&self, area: Rect) {
        if self.ime_area.replace(area) == area {
            return;
        }
        let data = ImeRequestData::default().with_cursor_area(PhysicalPosition::new(area.x, area.y).into(), PhysicalSize::new(area.w, area.h).into());
        let _ = self.raw.request_ime_update(ImeRequest::Update(data));
    }

    fn is_dark_mode(&self) -> bool {
        preferred_dark().unwrap_or(false)
    }

    fn set_clipboard_files(&self, paths: &[FsPath], cut: bool) {
        crate::shell::remember_files(paths, cut);
        self.push(Command::CopyFiles(paths.to_vec(), cut));
    }

    fn shell_menu(&self, menu: ShellMenu) {
        let (id, start, x, y, kind) = (self.id, Instant::now(), menu.x, menu.y, menu.kind);
        let show = move |(items, launches): OpenWith| {
            tracing::debug!(elapsed = ?start.elapsed(), apps = launches.len(), "menu ready");
            if !items.is_empty() {
                QUEUED.lock().unwrap_or_else(|e| e.into_inner()).push((id, Event::ShowMenu { items, x, y, launches, kind, shell: Vec::new(), pending: false }));
                wake_loop();
            }
        };
        let ready = PREFETCHED.lock().unwrap_or_else(|e| e.into_inner()).take().filter(|(files, _)| *files == menu.items);
        background("flit-menu", move || {
            let opened = match ready {
                Some((_, apps)) => crate::apps::with_apps(menu.menu, apps),
                None => crate::apps::menu_with_apps(menu.menu, &menu.items),
            };
            show(crate::scripts::with_scripts(opened, &menu.dir, &menu.items))
        });
    }

    fn shell_prefetch(&self, menu: Option<ShellMenu>) {
        let Some(items) = menu.map(|m| m.items).filter(|items| !items.is_empty()) else { return };
        if PREFETCHED.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(|(done, _)| *done == items) {
            return;
        }
        background("flit-menu-prefetch", move || {
            let c = crate::apps::catalog();
            let apps = crate::apps::open_with(&items, &c.entries, &c.assoc);
            *PREFETCHED.lock().unwrap_or_else(|e| e.into_inner()) = Some((items, apps));
        });
    }

    fn shell_warm(&self, _menus: Vec<ShellMenu>) {
        background("flit-menu-warm", || drop(crate::apps::catalog()));
    }

    fn properties(&self, _paths: Vec<FsPath>) {}

    fn start_drag(&self, paths: Vec<FsPath>) {
        self.push(Command::Drag(self.raw.id(), paths, self.scale.get()));
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

    fn window_at(&self, x: f32, y: f32) -> Option<(WindowId, f32, f32)> {
        if self.is_wayland() {
            return None;
        }
        let mut targets = self.targets.borrow_mut();
        targets.refresh(Instant::now());
        let source = targets.windows.iter().find(|w| w.target.id == self.id)?.target;
        let point = (source.origin.0 + f64::from(x), source.origin.1 + f64::from(y));
        tab_target(targets.windows.iter().map(|w| TabTarget { minimized: w.minimized.get(), ..w.target }), self.id, point)
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
    _preferences: Option<crate::preferences::Watch>,
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
        let preferences = if crate::preferences::Desktop::current() == crate::preferences::Desktop::Kde {
            crate::xdg::config_home().and_then(|config| crate::preferences::Watch::start(config, Duration::from_millis(250), |change| match change {
                crate::preferences::Change::Buttons(buttons) => set_window_buttons(buttons),
                crate::preferences::Change::Globals(text) => {
                    if let Some(dark) = crate::shell::kde_dark(&text) {
                        PREFERRED_DARK.store(if dark { DARK_YES } else { DARK_NO }, Ordering::Release);
                        THEME_CHANGED.store(true, Ordering::Release);
                    }
                    apply_look(crate::atspi::LookChange::Accent(crate::shell::kde_accent(&text)));
                }
            }).ok())
        } else { None };
        Ok(Self { event_loop, pending: Arc::default(), _preferences: preferences })
    }

    fn waker(&self) -> LinuxWaker {
        LinuxWaker { proxy: self.event_loop.create_proxy(), pending: self.pending.clone() }
    }

    fn run<H: WindowHandler<Self>>(mut self, first: WindowOptions, handler: H) -> Result<(), PlatformError> {
        let commands: Commands = Rc::new(RefCell::new(vec![Command::Open(first)]));
        let mut app = Loop { handler, nodes: Vec::new(), commands, targets: Rc::new(RefCell::new(TabWindows::new())), pending: self.pending.clone(), next_id: 1, mods: ModifiersState::empty(), started: false, error: None, incoming: None, wl: None, verify: None, reading: None, clip_text: None, watching: false, outgoing: None };
        LOOP_RUNNING.store(true, Ordering::Release);
        let ran = self.event_loop.run_app_on_demand(&mut app);
        LOOP_RUNNING.store(false, Ordering::Release);
        ran.map_err(loop_failure)?;
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
    targets: Rc<RefCell<TabWindows>>,
    pending: Arc<AtomicBool>,
    next_id: WindowId,
    mods: ModifiersState,
    started: bool,
    error: Option<PlatformError>,
    incoming: Option<Incoming>,
    wl: Option<crate::wl::Wl>,
    verify: Option<(Instant, Unverified)>,
    reading: Option<Reading>,
    clip_text: Option<bool>,
    watching: bool,
    outgoing: Option<DataTransferId>,
}

struct Reading {
    want: HashMap<AsyncRequestSerial, String>,
    got: crate::clipboard::Payload,
    waiters: Vec<crossbeam_channel::Sender<Option<crate::clipboard::Payload>>>,
    started: Instant,
}

impl Reading {
    fn finish(self) {
        for w in self.waiters {
            let _ = w.send(Some(self.got.clone()));
        }
    }
}

fn type_name(ty: &dyn TransferType) -> Option<String> {
    if let Some(m) = ty.cast_ref::<winit::platform::wayland::MimeType>() {
        return Some(m.to_string());
    }
    let atom = ty.cast_ref::<winit::platform::x11::SelectionType>()?.atom();
    crate::clipboard::x11_names().iter().find(|(a, _)| *a == atom).map(|(_, n)| (*n).to_owned())
}

enum Unverified {
    Files(Vec<FsPath>, bool),
    Image(Vec<u8>),
}

impl Unverified {
    fn marker(&self) -> &'static str {
        match self {
            Unverified::Files(..) => crate::clipboard::MARKER,
            Unverified::Image(_) => crate::clipboard::IMAGE_MARKER,
        }
    }
}

fn copy_image_through_helper(png: Vec<u8>) {
    background("flit-copy", move || {
        if !crate::shell::set_clipboard_image(&png) {
            tracing::info!("cannot copy the image: no wl-copy or xclip");
        }
    });
}

fn clipboard_has(el: &dyn ActiveEventLoop, name: &str) -> bool {
    let Ok(Some(id)) = el.clipboard() else { return false };
    el.data_transfer(id).is_ok_and(|d| d.available_types().into_iter().any(|t| type_name(t).as_deref() == Some(name)))
}

fn wayland_display(el: &dyn ActiveEventLoop) -> Option<*mut std::ffi::c_void> {
    match el.display_handle().ok()?.as_raw() {
        RawDisplayHandle::Wayland(d) => Some(d.display.as_ptr()),
        _ => None,
    }
}

fn ask_token(w: &LinuxWindow) {
    if w.is_wayland() && w.raw.request_activation_token().is_ok() {
        crate::activation::asked();
    }
}

fn platform_attributes(el: &dyn ActiveEventLoop) -> Box<dyn PlatformWindowAttributes> {
    let token = take_startup_token().map(ActivationToken::from_raw);
    if el.is_wayland() {
        let attrs = WindowAttributesWayland::default().with_name(APP_ID, APP_ID);
        Box::new(match token {
            Some(t) => attrs.with_activation_token(t),
            None => attrs,
        })
    } else {
        let attrs = WindowAttributesX11::default().with_name(APP_ID, APP_ID);
        Box::new(match token {
            Some(t) => attrs.with_activation_token(t),
            None => attrs,
        })
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

fn set_primary(el: &dyn ActiveEventLoop, text: String) {
    let Some(ext) = el.primary_selection_ext() else { return };
    let data = DataTransferSendBuilder::new(text).with_type(TypeHint::Plaintext, |text: &String, _| Some(text.clone())).build();
    if let Err(e) = ext.set_primary_selection(data) {
        tracing::debug!("cannot set the primary selection: {e}");
    }
}

fn paste_primary(el: &dyn ActiveEventLoop) {
    let Some(ext) = el.primary_selection_ext() else { return };
    match ext.primary_selection() {
        Ok(Some(id)) => {
            if let Err(e) = el.fetch_data_transfer(id, &TypeHint::Plaintext) {
                tracing::debug!("the primary selection has no text: {e}");
            }
        }
        Ok(None) => {}
        Err(e) => tracing::debug!("cannot read the primary selection: {e}"),
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

fn drag_out(el: &dyn ActiveEventLoop, source: winit::window::WindowId, paths: Vec<FsPath>, scale: f32) -> Option<DataTransferId> {
    if let Ok(RawDisplayHandle::Xlib(x)) = el.display_handle().map(|h| h.as_raw()) {
        crate::xdnd::release_pointer_of(x.display.map_or(std::ptr::null_mut(), |d| d.as_ptr()));
        background("flit-drag", move || match crate::xdnd::drag(paths, scale) {
            Ok(outcome) => tracing::info!("drag out of Flit: {outcome:?}"),
            Err(e) => tracing::info!("cannot drag files out of Flit here: {e}"),
        });
        return None;
    }
    let list = crate::shell::uri_list(&paths);
    let data = DataTransferSendBuilder::new(list).with_type(TypeHint::UriList, |list: &String, _| Some(list.clone())).build();
    let picture = crate::xdnd::drag_picture_soon(paths, scale);
    let hot = (picture.width / 4) as i32;
    let rgba = flit_core::preview::straight_rgba(&picture.rgba);
    let icon = winit::icon::RgbaIcon::new(rgba, picture.width, picture.height).ok().map(|icon| winit::event_loop::DragIcon { icon: icon.into(), offset_x: hot, offset_y: hot });
    el.start_drag(source, data, &[DndAction::Copy, DndAction::Move], icon).map_err(|e| tracing::info!("cannot drag files out of Flit here: {e}")).ok()
}

fn dragged_kinds(el: &dyn ActiveEventLoop, id: DataTransferId) -> Vec<TypeHint> {
    let offered: Vec<TypeHint> = el.data_transfer(id).map(|t| t.available_types().into_iter().filter_map(|ty| ty.hint()).collect()).unwrap_or_default();
    crate::dropped::wanted(&offered)
}

struct Incoming {
    id: DataTransferId,
    at: (f32, f32),
    paths: Option<Vec<FsPath>>,
    proposed: Option<DndAction>,
    dropped: Option<Option<DndAction>>,
    preferred: Option<DndAction>,
    ask: bool,
    content: Option<crate::dropped::Content>,
    fetching: Vec<TypeHint>,
}

enum Arrival {
    Enter,
    Over,
    Drop,
}

const OFFERED_TO_FLIT: u32 = drop_effect::COPY | drop_effect::MOVE | drop_effect::LINK;

fn winit_copy_files(el: &dyn ActiveEventLoop, paths: Vec<FsPath>, cut: bool) {
    let lossy = |b: Vec<u8>| String::from_utf8_lossy(&b).into_owned();
    let texts = (lossy(crate::clipboard::uri_list(&paths)), lossy(crate::clipboard::plain(&paths)));
    let data = DataTransferSendBuilder::new(texts)
        .with_type(TypeHint::UriList, |t: &(String, String), _| Some(t.0.clone()))
        .with_type(TypeHint::Plaintext, |t: &(String, String), _| Some(t.1.clone()))
        .build();
    if let Err(e) = el.set_clipboard(data) {
        tracing::debug!("winit cannot set the file clipboard ({e}), asking a helper program");
        background("flit-copy", move || {
            if !crate::shell::set_clipboard_files(&paths, cut) {
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
                    Command::CopyFiles(paths, cut) => self.copy_files(el, paths, cut),
                    Command::CopyImage(image) => self.copy_image(el, image),
                    Command::SetPrimary(text) => set_primary(el, text),
                    Command::PastePrimary => paste_primary(el),
                    Command::Drag(source, paths, scale) => self.outgoing = drag_out(el, source, paths, scale),
                    Command::Activate(id, token) => self.activate(id, token),
                }
            }
        }
        if self.nodes.is_empty() {
            el.exit();
        }
    }

    fn copy_files(&mut self, el: &dyn ActiveEventLoop, paths: Vec<FsPath>, cut: bool) {
        let mut payload = crate::clipboard::payload(&paths, cut);
        if el.is_wayland() {
            payload.push((crate::clipboard::MARKER.to_owned(), Vec::new()));
            if self.wl.as_mut().is_some_and(|wl| wl.set_selection(Arc::new(payload))) {
                self.verify = Some((Instant::now() + VERIFY_WAIT, Unverified::Files(paths, cut)));
                return;
            }
        } else {
            match crate::clipboard::own_x11(payload) {
                Ok(()) => return,
                Err(e) => tracing::debug!("cannot own the X11 clipboard ({e}), offering files through winit"),
            }
        }
        winit_copy_files(el, paths, cut);
    }

    fn copy_image(&mut self, el: &dyn ActiveEventLoop, image: Bitmap) {
        let wayland = el.is_wayland();
        background("flit-copy", move || {
            let Some(png) = crate::clipboard::png(&image) else { return };
            if wayland {
                *ENCODED_IMAGE.lock().unwrap_or_else(|e| e.into_inner()) = Some(png);
                return wake_loop();
            }
            match crate::clipboard::own_x11(vec![(crate::clipboard::PNG.to_owned(), png.clone())]) {
                Ok(()) => {}
                Err(e) => {
                    tracing::debug!("cannot own the X11 clipboard ({e}), asking a helper program");
                    copy_image_through_helper(png);
                }
            }
        });
    }

    fn offer_encoded_image(&mut self) {
        let Some(png) = ENCODED_IMAGE.lock().unwrap_or_else(|e| e.into_inner()).take() else { return };
        let payload = vec![(crate::clipboard::PNG.to_owned(), png.clone()), (crate::clipboard::IMAGE_MARKER.to_owned(), Vec::new())];
        if self.wl.as_mut().is_some_and(|wl| wl.set_selection(Arc::new(payload))) {
            self.verify = Some((Instant::now() + VERIFY_WAIT, Unverified::Image(png)));
        } else {
            copy_image_through_helper(png);
        }
    }

    fn verify_copy(&mut self, el: &dyn ActiveEventLoop) {
        let Some((until, what)) = &self.verify else { return };
        if clipboard_has(el, what.marker()) {
            self.verify = None;
        } else if Instant::now() >= *until {
            match self.verify.take().map(|v| v.1) {
                Some(Unverified::Files(paths, cut)) => {
                    tracing::info!("the compositor did not take Flit's file clipboard, offering it through winit");
                    winit_copy_files(el, paths, cut);
                }
                Some(Unverified::Image(png)) => {
                    tracing::info!("the compositor did not take Flit's image, asking a helper program");
                    copy_image_through_helper(png);
                }
                None => {}
            }
        }
    }

    fn activate(&mut self, id: WindowId, token: String) {
        let Some(node) = self.nodes.iter().find(|n| n.window.id == id) else { return };
        let raw = &node.window.raw;
        let done = match raw.window_handle().map(|h| h.as_raw()) {
            Ok(RawWindowHandle::Wayland(h)) => self.wl.as_ref().is_some_and(|wl| wl.activate(h.surface.as_ptr(), &token)),
            Ok(RawWindowHandle::Xlib(h)) => crate::activation::x11_activate(h.window as u32, &token).map_err(|e| tracing::debug!("X11 activation: {e}")).is_ok(),
            Ok(RawWindowHandle::Xcb(h)) => crate::activation::x11_activate(h.window.get(), &token).map_err(|e| tracing::debug!("X11 activation: {e}")).is_ok(),
            _ => false,
        };
        if !done {
            raw.focus_window();
        }
    }

    fn start_reads(&mut self, el: &dyn ActiveEventLoop) {
        let waiters = std::mem::take(&mut *READS.lock().unwrap_or_else(|e| e.into_inner()));
        if waiters.is_empty() {
            return;
        }
        if let Some(r) = self.reading.as_mut().filter(|r| r.started.elapsed() < READ_WAIT) {
            return r.waiters.extend(waiters);
        }
        let mut r = Reading { want: HashMap::new(), got: Vec::new(), waiters, started: Instant::now() };
        if let Ok(Some(id)) = el.clipboard() {
            if let Ok(transfer) = el.data_transfer(id) {
                for ty in transfer.available_types() {
                    let Some(name) = type_name(ty).filter(|n| crate::clipboard::FILE_TYPES.contains(&n.as_str()) && !r.want.values().any(|w| w == n)) else { continue };
                    match el.fetch_data_transfer(id, ty) {
                        Ok(serial) => {
                            r.want.insert(serial, name);
                        }
                        Err(e) => tracing::debug!("cannot read {name} from the clipboard: {e}"),
                    }
                }
            }
        }
        if r.want.is_empty() {
            for w in r.waiters {
                let _ = w.send(None);
            }
            return;
        }
        self.reading = Some(r);
    }

    fn read_arrived(&mut self, event: &WindowEvent) -> bool {
        let WindowEvent::DataTransferReceived { serial, value, .. } = event else { return false };
        let Some(r) = self.reading.as_mut() else { return false };
        let Some(name) = r.want.remove(serial) else { return false };
        r.got.push((name, value.try_as_bytes().unwrap_or_default()));
        if r.want.is_empty() {
            if let Some(r) = self.reading.take() {
                r.finish();
            }
        }
        true
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
        self.nodes.push(Node { window: LinuxWindow::new(id, raw, self.commands.clone(), self.targets.clone()), clicks: Clicks::new(input::double_click()) });
        let node = self.nodes.last().unwrap();
        self.handler.created(&node.window);
        self.handler.event(&node.window, Event::WindowButtons(window_buttons()));
        self.handler.event(&node.window, Event::Animations(animations()));
        self.handler.event(&node.window, Event::Accent(system_accent()));
        self.handler.event(&node.window, Event::TextScale(text_scale(node.window.is_wayland(), TEXT_SCALE_MILLI.load(Ordering::Acquire))));
        if let Some(text) = self.clip_text {
            self.handler.event(&node.window, Event::ClipboardText(text));
        }
    }

    fn clipboard_text(&mut self, text: Option<bool>) {
        let Some(text) = text.filter(|&t| self.clip_text != Some(t)) else { return };
        self.clip_text = Some(text);
        for node in &self.nodes {
            self.handler.event(&node.window, Event::ClipboardText(text));
        }
    }

    fn watch_clipboard(&mut self, el: &dyn ActiveEventLoop) {
        if self.watching || el.is_wayland() {
            return;
        }
        self.watching = true;
        background("flit-clipboard-watch", || {
            let watched = crate::clipboard::watch_x11_text("CLIPBOARD", |text| {
                CLIPBOARD_TEXT.store(if text { TEXT_YES } else { TEXT_NO }, Ordering::Release);
                wake_loop();
            });
            if let Err(e) = watched {
                tracing::debug!("cannot watch the X11 clipboard for text: {e}");
            }
        });
    }

    fn close(&mut self, id: WindowId) {
        let Some(i) = self.nodes.iter().position(|n| n.window.id == id) else { return };
        let node = self.nodes.remove(i);
        crate::atspi::closed(id);
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
            (HitArea::Caption, MouseButton::Left) if clicks == 2 => handler.event(w, Event::MouseDown { x, y, button, clicks }),
            (HitArea::Caption, MouseButton::Left) => {
                let _ = w.raw.drag_window();
            }
            (HitArea::Caption, MouseButton::Right) => w.raw.show_window_menu(position.into()),
            _ => handler.event(w, Event::MouseDown { x, y, button, clicks }),
        }
    }

    fn drop_keys(&self, el: &dyn ActiveEventLoop) -> Option<Mods> {
        if !el.is_wayland() {
            return Some(crate::xdnd::pointer_mods().unwrap_or_else(|| input::mods(self.mods)));
        }
        self.outgoing.is_some().then(|| input::mods(self.mods))
    }

    fn arrive(&mut self, el: &dyn ActiveEventLoop, i: usize, arrival: Arrival, proposed: Option<DndAction>) {
        let keys = self.drop_keys(el);
        let (handler, w) = (&mut self.handler, &self.nodes[i].window);
        let Some(d) = self.incoming.as_mut() else { return };
        let Some(paths) = d.paths.clone() else { return };
        let content = d.content.is_some();
        let (mods, ask) = crate::xdnd::incoming_choice(content, keys, proposed, d.preferred);
        let (x, y) = (d.at.0, d.at.1);
        let allowed = if content { drop_effect::COPY } else { OFFERED_TO_FLIT };
        let reenter = !matches!(arrival, Arrival::Enter) && ask != d.ask;
        if reenter {
            handler.drag(w, DragEvent::Leave);
        }
        d.ask = ask;
        let enter = DragEvent::Enter { paths, x, y, mods, allowed, right: ask };
        let event = match arrival {
            Arrival::Enter => enter,
            Arrival::Over | Arrival::Drop => {
                if reenter {
                    handler.drag(w, enter);
                }
                if matches!(arrival, Arrival::Drop) {
                    match d.content.take() {
                        Some(content) => DragEvent::DropData { x, y, data: Arc::new(content.file(S::UiDroppedText.t(), S::UiDroppedImage.t())) },
                        None => DragEvent::Drop { x, y, mods, allowed },
                    }
                } else {
                    DragEvent::Over { x, y, mods, allowed }
                }
            }
        };
        let reply = handler.drag(w, event);
        let effect = reply.effect;
        if crate::xdnd::chose_freely(keys, proposed, d.preferred) {
            d.preferred = crate::xdnd::wire_action(effect);
        }
        let id = d.id;
        if matches!(arrival, Arrival::Drop) {
            self.incoming = None;
        } else {
            let _ = el.set_valid_dnd_actions(id, &crate::xdnd::incoming_actions(content, effect, d.preferred));
        }
    }

    fn fetch_next(&mut self, el: &dyn ActiveEventLoop, id: DataTransferId) {
        while let Some(kind) = self.incoming.as_ref().filter(|d| d.id == id).and_then(|d| d.fetching.first().copied()) {
            match el.fetch_data_transfer(id, &kind) {
                Ok(_) => return,
                Err(e) => tracing::debug!("cannot read {kind:?} from the dragged data: {e}"),
            }
            if let Some(d) = self.incoming.as_mut() {
                d.fetching.remove(0);
            }
        }
        if self.incoming.as_ref().is_some_and(|d| d.id == id && d.paths.is_none()) {
            self.incoming = None;
            let _ = el.set_valid_dnd_actions(id, &[]);
        }
    }

    fn drag_in(&mut self, el: &dyn ActiveEventLoop, i: usize, event: WindowEvent) -> Option<WindowEvent> {
        match event {
            WindowEvent::DragEntered { id, position } => {
                let at = position.map_or((0.0, 0.0), |p| (p.x as f32, p.y as f32));
                let offered = crate::xdnd::offered_actions(drop_effect::COPY, None);
                let fetching = dragged_kinds(el, id);
                self.incoming = Some(Incoming { id, at, paths: None, proposed: None, dropped: None, preferred: offered.first().copied(), ask: false, content: None, fetching });
                let _ = el.set_valid_dnd_actions(id, &offered);
                self.fetch_next(el, id);
            }
            WindowEvent::DataTransferReceived { id, value, .. } if self.incoming.as_ref().is_some_and(|d| d.id == id && d.paths.is_none()) => {
                let bytes = value.try_as_bytes().unwrap_or_default();
                let d = self.incoming.as_mut()?;
                let kind = (!d.fetching.is_empty()).then(|| d.fetching.remove(0));
                match crate::dropped::interpret(kind, &bytes) {
                    Some(crate::dropped::Dragged::Files(paths)) => d.paths = Some(paths),
                    Some(crate::dropped::Dragged::Content(content)) => {
                        d.content = Some(content);
                        d.paths = Some(Vec::new());
                    }
                    None => {
                        self.fetch_next(el, id);
                        return None;
                    }
                }
                let (proposed, dropped) = (d.proposed, d.dropped);
                self.arrive(el, i, Arrival::Enter, proposed);
                if let Some(action) = dropped {
                    self.arrive(el, i, Arrival::Drop, action);
                }
            }
            WindowEvent::DragPosition { id, position, proposed_action } => {
                let d = self.incoming.as_mut().filter(|d| d.id == id)?;
                d.at = (position.x as f32, position.y as f32);
                d.proposed = proposed_action;
                if d.paths.is_some() {
                    self.arrive(el, i, Arrival::Over, proposed_action);
                }
            }
            WindowEvent::DragDropped { id, proposed_action } => {
                let d = self.incoming.as_mut().filter(|d| d.id == id)?;
                if proposed_action == Some(DndAction::Ask) {
                    let _ = el.set_valid_dnd_actions(id, &[DndAction::Copy]);
                }
                if d.paths.is_none() {
                    d.dropped = Some(proposed_action);
                    return None;
                }
                self.arrive(el, i, Arrival::Drop, proposed_action);
            }
            WindowEvent::DragLeft { id } => {
                if self.incoming.take().is_some_and(|d| d.id == id && d.paths.is_some()) {
                    self.handler.drag(&self.nodes[i].window, DragEvent::Leave);
                }
            }
            WindowEvent::OutgoingDragDropped { id, action } => {
                tracing::info!("drag out of Flit dropped with {action:?}");
                self.outgoing.take_if(|o| *o == id);
            }
            WindowEvent::OutgoingDragCanceled { id } => {
                tracing::info!("drag out of Flit canceled");
                self.outgoing.take_if(|o| *o == id);
            }
            other => return Some(other),
        }
        None
    }

    fn dispatch(&mut self, i: usize, event: WindowEvent) {
        let (handler, node) = (&mut self.handler, &mut self.nodes[i]);
        let w = &node.window;
        match event {
            WindowEvent::RedrawRequested => {
                handler.redraw(w);
                publish_a11y(handler, w);
            }
            WindowEvent::SurfaceResized(size) => {
                w.resized(size);
                w.cache_target(None);
                handler.event(w, Event::Resized { width: size.width, height: size.height });
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                w.scale.set(scale_factor as f32);
                handler.event(w, Event::ScaleChanged(scale_factor as f32));
            }
            WindowEvent::Moved(position) => w.cache_target(Some(position)),
            WindowEvent::Occluded(_) => w.cache_target(None),
            WindowEvent::CloseRequested => handler.event(w, Event::CloseRequested),
            WindowEvent::Focused(focused) => {
                w.cache_target(None);
                if focused {
                    w.focused();
                }
                node.clicks.reset();
                self.mods = ModifiersState::empty();
                handler.event(w, Event::Focus(focused));
            }
            WindowEvent::ModifiersChanged(m) => {
                if let Some(ev) = input::modifiers_changed(std::mem::replace(&mut self.mods, m.state()), m.state()) {
                    handler.event(w, ev);
                }
            }
            WindowEvent::KeyboardInput { event, is_synthetic: false, .. } => {
                let pressed = event.state == ElementState::Pressed;
                if pressed && !event.repeat {
                    ask_token(w);
                }
                handler.event(w, Event::Key { key: input::key(event.physical_key, &event.key_without_modifiers), mods: input::mods(self.mods), pressed });
                for c in event.text.iter().filter(|_| pressed && !self.mods.control_key() && !self.mods.meta_key()).flat_map(|t| t.chars()) {
                    handler.event(w, Event::Char(c));
                }
            }
            WindowEvent::Ime(Ime::Preedit(text, cursor)) => handler.event(w, Event::Preedit(flit_core::input::Preedit::new(text, cursor.map(|(start, _)| start)))),
            WindowEvent::Ime(Ime::Commit(text)) => {
                for c in text.chars() {
                    handler.event(w, Event::Char(c));
                }
            }
            WindowEvent::PointerMoved { position, primary, .. } if primary => {
                let (x, y) = (position.x as f32, position.y as f32);
                w.track_edge(x, y);
                handler.event(w, Event::MouseMove { x, y });
            }
            WindowEvent::PointerEntered { .. } => w.restore_cursor(),
            WindowEvent::PointerLeft { .. } => {
                w.restore_cursor();
                handler.event(w, Event::MouseLeave);
            }
            WindowEvent::ActivationTokenDone { token, .. } => crate::activation::answered(token.into_raw()),
            WindowEvent::PointerButton { state, position, button, primary, .. } => {
                if state == ElementState::Pressed {
                    ask_token(w);
                }
                let touch = matches!(button, winit::event::ButtonSource::Touch { .. });
                let mapped = if touch { primary.then_some(MouseButton::Left) } else { button.mouse_button().and_then(input::mouse_button) };
                if let Some(button) = mapped {
                    if state == ElementState::Pressed {
                        handler.event(w, Event::Pointer { touch });
                    }
                    self.button(i, state == ElementState::Pressed, position, button);
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let (dx, dy) = input::wheel(delta, w.scale.get());
                handler.event(w, Event::Wheel { dx, dy });
            }
            WindowEvent::PinchGesture { delta, .. } if delta.is_finite() && delta != 0.0 => handler.event(w, Event::Pinch(delta as f32)),
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
        crate::activation::set_wayland(el.is_wayland());
        if self.wl.is_none() {
            self.wl = wayland_display(el).and_then(crate::wl::Wl::connect);
        }
        self.watch_clipboard(el);
        self.started = true;
        self.settle(el);
    }

    fn proxy_wake_up(&mut self, el: &dyn ActiveEventLoop) {
        self.pending.store(false, Ordering::Release);
        self.offer_encoded_image();
        for node in &self.nodes {
            self.handler.woken(&node.window);
        }
        if THEME_CHANGED.swap(false, Ordering::AcqRel) {
            let dark = preferred_dark().unwrap_or(false);
            for node in &self.nodes {
                self.handler.event(&node.window, Event::ThemeChanged { dark });
            }
        }
        if ANIMATIONS_DIRTY.swap(false, Ordering::AcqRel) {
            for node in &self.nodes {
                self.handler.event(&node.window, Event::Animations(animations()));
            }
        }
        if ICONS_DIRTY.swap(false, Ordering::AcqRel) {
            for node in &self.nodes {
                self.handler.event(&node.window, Event::IconTheme);
            }
        }
        if LOOK_DIRTY.swap(false, Ordering::AcqRel) {
            let (accent, milli) = (system_accent(), TEXT_SCALE_MILLI.load(Ordering::Acquire));
            for node in &self.nodes {
                self.handler.event(&node.window, Event::Accent(accent));
                self.handler.event(&node.window, Event::TextScale(text_scale(node.window.is_wayland(), milli)));
            }
        }
        if MOUSE_DIRTY.swap(false, Ordering::AcqRel) {
            let (double_click, wheel_lines) = (input::double_click(), input::wheel_lines());
            for node in &mut self.nodes {
                node.clicks.set_interval(double_click);
                self.handler.event(&node.window, Event::MouseSettings { double_click, wheel_lines });
            }
        }
        if BUTTONS_DIRTY.swap(false, Ordering::AcqRel) {
            let buttons = window_buttons();
            for node in &self.nodes {
                self.handler.event(&node.window, Event::WindowButtons(buttons.clone()));
            }
        }
        let queued = std::mem::take(&mut *QUEUED.lock().unwrap_or_else(|e| e.into_inner()));
        for (id, ev) in queued {
            if let Some(node) = self.nodes.iter().find(|n| n.window.id == id) {
                self.handler.event(&node.window, ev);
            }
        }
        if DEVICES_CHANGED.swap(false, Ordering::AcqRel) {
            for node in &self.nodes {
                self.handler.event(&node.window, Event::DevicesChanged);
            }
        }
        if A11Y_REFRESH.swap(false, Ordering::AcqRel) {
            for node in &self.nodes {
                publish_a11y(&mut self.handler, &node.window);
            }
        }
        match CLIPBOARD_TEXT.swap(TEXT_UNSEEN, Ordering::AcqRel) {
            TEXT_UNSEEN => {}
            seen => self.clipboard_text(Some(seen == TEXT_YES)),
        }
        self.start_reads(el);
        if SHUTDOWN.swap(false, Ordering::AcqRel) {
            for node in &self.nodes {
                self.handler.event(&node.window, Event::SessionEnding);
                node.window.close();
            }
        }
        self.settle(el);
    }

    fn window_event(&mut self, el: &dyn ActiveEventLoop, id: winit::window::WindowId, event: WindowEvent) {
        if self.read_arrived(&event) {
            return self.settle(el);
        }
        if let Some(i) = self.nodes.iter().position(|n| n.window.raw.id() == id) {
            if let Some(event) = self.drag_in(el, i, event) {
                self.dispatch(i, event);
            }
        }
        self.settle(el);
    }

    fn about_to_wait(&mut self, el: &dyn ActiveEventLoop) {
        TURNS.fetch_add(1, Ordering::AcqRel);
        let text = self.wl.as_mut().and_then(|wl| {
            wl.pump();
            wl.clipboard_text_changed()
        });
        self.clipboard_text(text);
        self.verify_copy(el);
        self.fire_timers();
        self.settle(el);
        let verify = self.verify.as_ref().map(|v| v.0);
        let next = self.nodes.iter().filter_map(|n| n.window.timer.get()).chain(verify).min();
        el.set_control_flow(next.map_or(ControlFlow::Wait, ControlFlow::WaitUntil));
    }
}

#[cfg(test)]
#[path = "../test/window.rs"]
mod tests;
