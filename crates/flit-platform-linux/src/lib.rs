use crossbeam_channel::Sender;
use flit_core::atlas::Slot;
use flit_core::input::CursorIcon;
use flit_core::render::{DrawList, Rect};
use flit_core::text::{FontMetrics, FontStyle, GlyphBitmap, GlyphKey, ShapedText, TextBackend};
use flit_core::ui::{Bitmap, IconRequest};
use flit_core::{FsError, FsErrorKind, FsPath, ListingBuilder, Location, Open, ProviderEvent, TabId};
use flit_platform::*;
use flit_search::index::{Checkpoint, IndexSink};
use flit_search::proto::VolumeInfo;

pub struct Linux;

impl Platform for Linux {
    type WindowSystem = LinuxWindows;
    type Renderer = GlRenderer;
    type Text = SwashText;
    type Fs = LinuxFs;
    type Watcher = Inotify;
    type Shell = FreedesktopShell;
    type Instance = DbusInstance;
    type IndexSource = WalkIndex;
    type Service = Systemd;

    fn text() -> Result<SwashText, PlatformError> {
        Ok(SwashText)
    }
    fn fs() -> LinuxFs {
        LinuxFs
    }
    fn utc_offset_minutes() -> i32 {
        0
    }
    fn dark_mode() -> bool {
        false
    }
    fn state_dir() -> Option<std::path::PathBuf> {
        std::env::var_os("XDG_STATE_HOME").map(Into::into)
    }
    fn home() -> FsPath {
        std::env::var("HOME").ok().and_then(|h| FsPath::parse(&h)).unwrap_or_else(|| FsPath::parse("/").unwrap())
    }
    fn known_folder(_: &str) -> Option<Location> {
        None
    }

    fn fatal_error(message: &str) {
        eprintln!("flit: {message}");
    }

    fn refresh_period() -> Option<std::time::Duration> {
        None
    }
}

pub struct LinuxWindows;
pub struct LinuxWindow;
#[derive(Clone)]
pub struct LinuxWaker;

impl Waker for LinuxWaker {
    fn wake(&self) {
        todo!("EventLoopProxy::send_event")
    }
}

impl Window for LinuxWindow {
    fn id(&self) -> WindowId {
        todo!()
    }
    fn activate(&self) {
        todo!()
    }
    fn open_window(&self, _: WindowOptions) {
        todo!()
    }
    fn size(&self) -> (u32, u32) {
        todo!()
    }
    fn scale(&self) -> f32 {
        todo!()
    }
    fn request_redraw(&self) {
        todo!()
    }
    fn set_title(&self, _: &str) {
        todo!()
    }
    fn set_cursor(&self, _: CursorIcon) {
        todo!()
    }
    fn minimize(&self) {
        todo!()
    }
    fn toggle_maximize(&self) {
        todo!()
    }
    fn is_maximized(&self) -> bool {
        todo!()
    }
    fn close(&self) {
        todo!()
    }
    fn placement(&self) -> Placement {
        todo!()
    }
    fn clipboard_text(&self) -> Option<String> {
        todo!()
    }
    fn set_clipboard_text(&self, _: &str) {
        todo!()
    }
    fn set_ime_area(&self, _: Rect) {
        todo!()
    }
    fn is_dark_mode(&self) -> bool {
        todo!()
    }
}

impl WindowSystem for LinuxWindows {
    type Window = LinuxWindow;
    type Waker = LinuxWaker;
    fn new() -> Result<Self, PlatformError> {
        Err(PlatformError::unsupported("Linux window system"))
    }
    fn waker(&self) -> LinuxWaker {
        LinuxWaker
    }
    fn run<H: WindowHandler<Self>>(self, _: WindowOptions, _: H) -> Result<(), PlatformError> {
        Err(PlatformError::unsupported("Linux window system"))
    }
}

pub struct GlRenderer;

impl Renderer for GlRenderer {
    type Window = LinuxWindow;
    type Device = ();
    fn device(_: bool) -> Result<(), PlatformError> {
        Err(PlatformError::unsupported("Linux renderer"))
    }
    fn new(_: &(), _: &LinuxWindow, _: u32) -> Result<Self, PlatformError> {
        Err(PlatformError::unsupported("Linux renderer"))
    }
    fn resize(&mut self, _: u32, _: u32) {}
    fn upload(&mut self, _: Slot, _: &[u8]) {}
    fn render(&mut self, _: &DrawList) -> Result<(), RenderError> {
        Ok(())
    }
}

pub struct SwashText;

impl TextBackend for SwashText {
    fn shape(&mut self, _: &str, _: FontStyle, _: &mut ShapedText) {
        todo!()
    }
    fn rasterize(&mut self, _: GlyphKey, _: &mut GlyphBitmap) -> bool {
        todo!()
    }
    fn metrics(&mut self, _: FontStyle) -> FontMetrics {
        todo!()
    }
}

pub struct LinuxFs;

impl Fs for LinuxFs {
    fn read_dir(&self, _: &FsPath, _: &mut ListingBuilder<'_>) -> Result<(), FsError> {
        Err(FsError { kind: FsErrorKind::Unavailable, code: -1 })
    }
    fn seek_penalty(&self, _: &FsPath) -> Option<bool> {
        None
    }
}

pub struct Inotify;

impl Watcher for Inotify {
    fn new(_: Sender<ProviderEvent>, _: Wake) -> Result<Self, PlatformError> {
        Ok(Inotify)
    }
    fn watch(&mut self, _: TabId, _: u64, _: Option<&FsPath>) {}
}

pub struct FreedesktopShell;

impl Shell for FreedesktopShell {
    fn new() -> Result<Self, PlatformError> {
        Ok(FreedesktopShell)
    }
    fn icon(&mut self, _: &IconRequest) -> Option<Bitmap> {
        None
    }
    fn type_name(&mut self, _: &[u8]) -> Option<String> {
        None
    }
    fn open(&mut self, _: &FsPath) -> Result<(), PlatformError> {
        Err(PlatformError::unsupported("xdg-open"))
    }
}

impl Trash for FreedesktopShell {
    fn trash(&mut self, _: &[FsPath]) -> Result<(), PlatformError> {
        Err(PlatformError::unsupported("trash"))
    }
}

pub struct DbusInstance;

impl SingleInstance for DbusInstance {
    fn acquire(_: &Open) -> Instance<Self> {
        Instance::Primary(DbusInstance)
    }
    fn listen(self, _: OpenSink, _: Wake) {}
}

pub struct WalkIndex;

impl IndexSource for WalkIndex {
    fn volumes(&self) -> Result<Vec<VolumeInfo>, PlatformError> {
        Ok(Vec::new())
    }
    fn scan<S: IndexSink>(&mut self, _: &str, _: &mut S) -> Result<Checkpoint, PlatformError> {
        Err(PlatformError::unsupported("index scan"))
    }
    fn changes<S: IndexSink>(&mut self, _: &str, _: Checkpoint, _: &mut S) -> Result<Checkpoint, PlatformError> {
        Err(PlatformError::unsupported("index changes"))
    }
}

pub struct Systemd;

impl Service for Systemd {
    fn endpoint() -> String {
        "/run/flit/indexd.sock".into()
    }
    fn run(body: fn() -> Result<(), PlatformError>) -> Result<(), PlatformError> {
        body()
    }
}
