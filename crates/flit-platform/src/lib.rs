mod indexer;
mod local;

pub use indexer::{root_of, start_indexer};
pub use local::{via_unc, LocalProvider};

use crossbeam_channel::Sender;
use flit_core::atlas::Slot;
use flit_core::input::{CursorIcon, Event, HitArea};
use flit_core::render::{DrawList, Rect};
use flit_core::ui::{Bitmap, IconRequest};
use flit_core::{FsError, FsPath, ListingBuilder, Location, Open, ProviderEvent, ProviderRequest, TabId};
use flit_search::index::{Checkpoint, IndexSink};
use flit_search::proto::VolumeInfo;
use std::fmt;
use std::sync::Arc;

pub use flit_core::text::TextBackend as Text;

pub type Wake = Arc<dyn Fn() + Send + Sync>;

pub type OpenSink = Arc<std::sync::Mutex<Option<Sender<Open>>>>;

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
}

pub trait WindowHandler<W: WindowSystem> {
    fn created(&mut self, window: &W::Window);
    fn event(&mut self, window: &W::Window, event: Event);
    fn hit_test(&mut self, window: &W::Window, x: f32, y: f32) -> HitArea;
    fn redraw(&mut self, window: &W::Window);
    fn woken(&mut self, window: &W::Window);
    fn destroyed(&mut self, window: &W::Window);
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
    fn render(&mut self, list: &DrawList) -> Result<(), RenderError>;
}

pub trait Provider: Send + Sync + 'static {
    fn submit(&self, req: ProviderRequest);
}

pub trait Fs: Send + Sync + 'static {
    fn read_dir(&self, dir: &FsPath, out: &mut ListingBuilder<'_>) -> Result<(), FsError>;
    fn seek_penalty(&self, path: &FsPath) -> Option<bool>;
}

pub trait Watcher: Sized + Send + 'static {
    fn new(events: Sender<ProviderEvent>, wake: Wake) -> Result<Self, PlatformError>;
    fn watch(&mut self, tab: TabId, gen: u64, dir: Option<&FsPath>);
}

pub trait Shell: Sized {
    fn new() -> Result<Self, PlatformError>;
    fn icon(&mut self, req: &IconRequest) -> Option<Bitmap>;
    fn type_name(&mut self, ext: &[u8]) -> Option<String>;
    fn open(&mut self, path: &FsPath) -> Result<(), PlatformError>;
}

pub trait Trash {
    fn trash(&mut self, paths: &[FsPath]) -> Result<(), PlatformError>;
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
    type Shell: Shell + Trash;
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
}
