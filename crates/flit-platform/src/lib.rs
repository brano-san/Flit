mod indexer;
mod local;
mod wsl;

pub use indexer::{root_of, start_indexer};
pub use local::{via_unc, LocalProvider, CANCELLED};
pub use wsl::{AgentImage, AgentImages, AgentLauncher, AgentProcess, Clipboard, Timing, WslProvider, UNMAPPABLE};

use crossbeam_channel::Sender;
use flit_core::atlas::Slot;
use flit_core::desktop_ini::DesktopIni;
use flit_core::input::{CursorIcon, Event, HitArea};
use flit_core::preview::{HostCmd, PropGroup};
use flit_core::render::{DrawList, Rect};
use flit_core::ui::{Bitmap, DragEvent, IconRequest, NewTemplate, ShellMenu};
use flit_core::digest::HashKind;
use flit_core::provider::{Conflict, FileOp, Launch, OpCtl, OpProgress, Resolve, Stage};
use flit_core::tags::FileKey;
use flit_core::{FsError, FsPath, ListingBuilder, Location, Meta, Open, ProviderEvent, ProviderRequest, TabId};
use flit_search::index::{Checkpoint, IndexSink};
use flit_search::proto::VolumeInfo;
use std::fmt;
use std::sync::Arc;

pub use flit_core::text::TextBackend as Text;

pub type Wake = Arc<dyn Fn() + Send + Sync>;

pub type OpenSink = Arc<std::sync::Mutex<Option<Sender<Open>>>>;

#[derive(Clone, Debug, PartialEq)]
pub enum Resolved {
    Found(FsPath, Meta),
    Gone,
    Unavailable,
}

#[derive(Debug, Clone)]
pub struct PlatformError {
    pub code: i64,
    pub message: String,
}

impl PlatformError {
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self { code, message: message.into() }
    }
    pub fn unsupported(what: &str) -> Self {
        Self::new(-1, format!("{what}: not supported on this platform yet"))
    }
}

impl fmt::Display for PlatformError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (0x{:08X})", self.message, self.code as u32)
    }
}

impl std::error::Error for PlatformError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placement {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub maximized: bool,
}

#[derive(Clone, Debug)]
pub struct WindowOptions {
    pub title: String,
    pub placement: Option<Placement>,
    pub dark: bool,
}

pub type WindowId = u64;

pub trait Waker: Clone + Send + Sync + 'static {
    fn wake(&self);
}

pub trait Window {
    fn id(&self) -> WindowId;
    fn size(&self) -> (u32, u32);
    fn scale(&self) -> f32;
    fn request_redraw(&self);
    fn set_title(&self, title: &str);
    fn set_cursor(&self, cursor: CursorIcon);
    fn minimize(&self);
    fn toggle_maximize(&self);
    fn is_maximized(&self) -> bool;
    fn close(&self);
    fn activate(&self);
    fn open_window(&self, opts: WindowOptions);
    fn placement(&self) -> Placement;
    fn clipboard_text(&self) -> Option<String>;
    fn set_clipboard_text(&self, text: &str);
    fn set_ime_area(&self, area: Rect);
    fn is_dark_mode(&self) -> bool;
    fn set_clipboard_files(&self, paths: &[FsPath], cut: bool);
    fn shell_menu(&self, menu: ShellMenu);
    fn properties(&self, paths: Vec<FsPath>);
    fn start_drag(&self, paths: Vec<FsPath>);
    fn set_timer(&self, ms: u32);
    fn to_screen(&self, x: f32, y: f32) -> (i32, i32) {
        (x as i32, y as i32)
    }
    fn window_at(&self, _x: f32, _y: f32) -> Option<(WindowId, f32, f32)> {
        None
    }
    fn toggle_fullscreen(&self) {}
    fn set_dark(&self, _dark: bool) {}
    fn set_compact(&self, _on: Option<bool>) {}
    fn host(&self, _cmd: HostCmd) {}
    fn share(&self, _paths: Vec<FsPath>) {}
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DropReply {
    pub effect: u32,
    pub caption: Option<(String, String)>,
}

pub trait WindowHandler<W: WindowSystem> {
    fn created(&mut self, window: &W::Window);
    fn event(&mut self, window: &W::Window, event: Event);
    fn hit_test(&mut self, window: &W::Window, x: f32, y: f32) -> HitArea;
    fn redraw(&mut self, window: &W::Window);
    fn woken(&mut self, window: &W::Window);
    fn destroyed(&mut self, window: &W::Window);
    fn drag(&mut self, window: &W::Window, event: DragEvent) -> DropReply;
}

pub trait WindowSystem: Sized + 'static {
    type Window: Window;
    type Waker: Waker;
    fn new() -> Result<Self, PlatformError>;
    fn waker(&self) -> Self::Waker;
    fn run<H: WindowHandler<Self>>(self, first: WindowOptions, handler: H) -> Result<(), PlatformError>;
}

#[derive(Debug)]
pub enum RenderError {
    DeviceLost,
    Other(PlatformError),
}

pub trait Renderer: Sized {
    type Window;
    type Device: Send + 'static;
    fn device(software: bool) -> Result<Self::Device, PlatformError>;
    fn new(device: &Self::Device, window: &Self::Window, atlas_size: u32) -> Result<Self, PlatformError>;
    fn resize(&mut self, width: u32, height: u32);
    fn upload(&mut self, slot: Slot, rgba: &[u8]);
    fn upload_thumb(&mut self, _slot: Slot, _rgba: &[u8]) {}
    fn upload_preview(&mut self, _image: Option<&Bitmap>) {}
    fn render(&mut self, list: &DrawList) -> Result<(), RenderError>;
}

pub trait Provider: Send + Sync + 'static {
    fn submit(&self, req: ProviderRequest);
}

pub trait Fs: Send + Sync + 'static {
    fn read_dir(&self, dir: &FsPath, out: &mut ListingBuilder<'_>) -> Result<(), FsError>;
    fn seek_penalty(&self, path: &FsPath) -> Option<bool>;
    fn desktop_ini(&self, _dir: &FsPath) -> Option<DesktopIni> {
        None
    }
    fn dir_sizes(&self, dir: &FsPath, items: &[(u32, Vec<u8>)], keep_going: &(dyn Fn() -> bool + Sync), report: &(dyn Fn(u32, u64) + Sync)) {
        local::dir_sizes(self, dir, items, keep_going, report)
    }
    fn hash(&self, _file: &FsPath, _kinds: &[HashKind], _job: &Job) -> Result<Vec<String>, PlatformError> {
        Err(PlatformError::unsupported("file hashes"))
    }
}

pub trait Watcher: Sized + Send + 'static {
    fn new(events: Sender<ProviderEvent>, wake: Wake) -> Result<Self, PlatformError>;
    fn watch(&mut self, tab: TabId, gen: u64, dir: Option<&FsPath>);
}

pub trait Shell: Sized {
    fn new() -> Result<Self, PlatformError>;
    fn icon(&mut self, req: &IconRequest) -> Option<Bitmap>;
    fn thumbnail(&mut self, _req: &IconRequest) -> Option<Bitmap> {
        None
    }
    fn type_name(&mut self, ext: &[u8]) -> Option<String>;
    fn open(&mut self, path: &FsPath) -> Result<(), PlatformError>;
    fn launch(&mut self, _launch: &Launch) -> Result<(), PlatformError> {
        Err(PlatformError::unsupported("launching programs"))
    }
    fn shortcut_folder(&mut self, _link: &FsPath) -> Option<FsPath> {
        None
    }
    fn link_target(&mut self, _link: &FsPath) -> Option<FsPath> {
        None
    }
    fn preview_image(&mut self, _path: &FsPath, _max: u32) -> Option<Bitmap> {
        None
    }
    fn poster(&mut self, _path: &FsPath, _size: u32) -> Option<Bitmap> {
        None
    }
    fn properties(&mut self, _path: &FsPath) -> Vec<PropGroup> {
        Vec::new()
    }
    fn preview_handler(&mut self, _ext: &[u8]) -> Option<String> {
        None
    }
    fn quick_view(&mut self, _path: &FsPath) -> bool {
        false
    }
}

#[derive(Debug, Default)]
pub struct OpOutcome {
    pub items: Vec<(FsPath, FsPath)>,
    pub error: Option<PlatformError>,
}

const PROGRESS_EVERY: std::time::Duration = std::time::Duration::from_millis(33);

type Sink<T> = Box<dyn Fn(T) + Send + Sync>;

#[derive(Default)]
struct Shown {
    progress: OpProgress,
    sent: Option<(std::time::Instant, Stage)>,
}

struct JobState {
    ctl: OpCtl,
    progress: Sink<OpProgress>,
    conflict: Sink<Conflict>,
    now: std::sync::Mutex<Shown>,
}

#[derive(Clone)]
pub struct Job(Arc<JobState>);

impl Job {
    pub fn new(ctl: OpCtl, progress: Sink<OpProgress>, conflict: Sink<Conflict>) -> Self {
        Self(Arc::new(JobState { ctl, progress, conflict, now: Default::default() }))
    }

    pub fn detached(policy: Resolve) -> Self {
        Self::new(OpCtl::new(Some(policy)), Box::new(|_| {}), Box::new(|_| {}))
    }

    pub fn ctl(&self) -> &OpCtl {
        &self.0.ctl
    }

    pub fn cancelled(&self) -> bool {
        self.0.ctl.is_cancelled()
    }

    pub fn progress(&self) -> OpProgress {
        self.0.now.lock().unwrap_or_else(|e| e.into_inner()).progress
    }

    pub fn update(&self, f: impl FnOnce(&mut OpProgress)) {
        let mut now = self.0.now.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut now.progress);
        let (t, p) = (std::time::Instant::now(), now.progress);
        if now.sent.is_none_or(|(at, stage)| stage != p.stage || t.duration_since(at) >= PROGRESS_EVERY) {
            now.sent = Some((t, p.stage));
            drop(now);
            (self.0.progress)(p);
        }
    }

    pub fn resolve(&self, c: Conflict) -> Option<Resolve> {
        self.0.ctl.resolve(|| (self.0.conflict)(c))
    }
}

pub trait FileOps {
    fn file_op(&mut self, op: &FileOp, job: &Job) -> OpOutcome;
    fn clipboard_files(&mut self) -> Result<(Vec<FsPath>, bool), PlatformError>;
    fn clear_clipboard(&mut self);
}

pub enum Instance<S> {
    Primary(S),
    Forwarded,
    Standalone(PlatformError),
}

pub trait SingleInstance: Sized + Send + 'static {
    fn acquire(open: &Open) -> Instance<Self>;
    fn listen(self, opens: OpenSink, wake: Wake);
}

pub trait IndexSource: Sized + Send {
    fn new() -> Result<Self, PlatformError>;
    fn volumes(&self) -> Result<Vec<VolumeInfo>, PlatformError>;
    fn scan<S: IndexSink>(&mut self, volume: &str, sink: &mut S) -> Result<Checkpoint, PlatformError>;
    fn changes<S: IndexSink>(&mut self, volume: &str, from: Checkpoint, sink: &mut S) -> Result<Option<Checkpoint>, PlatformError>;
}

pub trait Duplex: std::io::Read + std::io::Write + Send + Sized + 'static {
    fn try_clone(&self) -> std::io::Result<Self>;
    fn set_read_timeout(&mut self, timeout: Option<std::time::Duration>);
    fn access(&self) -> std::io::Result<Option<Arc<dyn flit_search::engine::Visibility>>> {
        Ok(None)
    }
}

pub type OnClient<C> = Arc<dyn Fn(C) + Send + Sync>;

pub trait Service {
    type Conn: Duplex;
    fn endpoint() -> String;
    fn run(body: fn() -> Result<(), PlatformError>) -> Result<(), PlatformError>;
    fn connect() -> Result<Self::Conn, PlatformError>;
    fn listen(max_clients: usize, on_client: OnClient<Self::Conn>) -> Result<(), PlatformError>;
}

pub trait Platform: 'static {
    type WindowSystem: WindowSystem;
    type Renderer: Renderer<Window = <Self::WindowSystem as WindowSystem>::Window>;
    type Text: Text;
    type Fs: Fs;
    type Watcher: Watcher;
    type Shell: Shell + FileOps;
    type Instance: SingleInstance;
    type IndexSource: IndexSource;
    type Service: Service;
    type FallbackIndex: IndexSource + 'static;

    fn text() -> Result<Self::Text, PlatformError>;
    fn fs() -> Self::Fs;
    fn utc_offset_minutes() -> i32;
    fn dark_mode() -> bool;
    fn state_dir() -> Option<std::path::PathBuf>;
    fn home() -> FsPath;
    fn known_folder(name: &str) -> Option<Location>;
    fn fatal_error(message: &str);
    fn refresh_period() -> Option<std::time::Duration>;
    fn drives() -> Vec<flit_core::ui::Drive>;
    fn wsl_distros() -> Vec<String>;
    fn templates() -> Vec<NewTemplate> {
        Vec::new()
    }
    fn libraries() -> Vec<flit_core::library::Library> {
        Vec::new()
    }
    fn library(_op: &flit_core::library::LibraryOp) -> Result<(), PlatformError> {
        Err(PlatformError::new(-1, "libraries are not available on this system"))
    }
    fn recent(_limit: usize) -> (Vec<flit_core::ui::Recent>, bool) {
        (Vec::new(), false)
    }
    fn double_click_time() -> std::time::Duration {
        std::time::Duration::from_millis(500)
    }
    fn elevated() -> bool {
        false
    }
    fn date_patterns() -> (String, String) {
        ("yyyy-MM-dd".into(), "HH:mm".into())
    }
    fn os_version() -> String {
        std::env::consts::OS.to_owned()
    }
    fn watch_wsl(_changed: Box<dyn Fn() + Send>) {}
    fn wsl_launcher(_distro: &str, _images: &AgentImages) -> Option<Box<dyn AgentLauncher>> {
        None
    }
    fn file_key(_path: &FsPath) -> Option<FileKey> {
        None
    }
    fn resolve_files(keys: &[FileKey]) -> Vec<Resolved> {
        keys.iter().map(|_| Resolved::Unavailable).collect()
    }
    fn shell_search(_folder: Option<&FsPath>, _query: &str, _keep_going: &dyn Fn() -> bool, _found: &mut dyn FnMut(FsPath, flit_core::Meta)) -> Result<(), PlatformError> {
        Err(PlatformError::new(-1, "Windows Search is not available on this system"))
    }
}
