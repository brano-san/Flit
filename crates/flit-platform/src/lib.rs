mod digest;
mod indexer;
mod local;
pub mod walk;
mod wsl;

pub use digest::hash_file;
pub use indexer::{root_of, start_indexer};
pub use local::{measure, read_deep, via_unc, LocalProvider, CANCELLED};
pub use wsl::{AgentImage, AgentImages, AgentLauncher, AgentProcess, Clipboard, Timing, WslProvider, UNMAPPABLE};

use crossbeam_channel::Sender;
use flit_core::atlas::Slot;
use flit_core::desktop_ini::DesktopIni;
use flit_core::input::{CursorIcon, Event, HitArea};
use flit_core::preview::{HostCmd, PropGroup};
use flit_core::render::{DrawList, Rect};
use flit_core::ui::{Bitmap, DragEvent, IconRequest, NewTemplate, ShellMenu};
use flit_core::digest::HashKind;
use flit_core::provider::{Access, Accounts, Conflict, Failure, FileOp, Launch, OpCtl, OpProgress, Recover, Resolve, Stage};
use flit_core::listing::DirStats;
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
    pub untrashable: Vec<FsPath>,
}

impl PlatformError {
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self { code, message: message.into(), untrashable: Vec::new() }
    }
    pub fn untrashable(mut self, items: Vec<FsPath>) -> Self {
        self.untrashable = items;
        self
    }
    pub fn unsupported(what: &str) -> Self {
        Self::new(-1, format!("{what}: not supported on this platform yet"))
    }
}

impl fmt::Display for PlatformError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match cfg!(windows) {
            true => write!(f, "{} (0x{:08X})", self.message, self.code as u32),
            false => f.write_str(&self.message),
        }
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

pub type SelectionSource = std::sync::Arc<dyn Fn() -> (Vec<FsPath>, Option<FsPath>) + Send + Sync>;

pub trait Window {
    fn id(&self) -> WindowId;
    fn size(&self) -> (u32, u32);
    fn scale(&self) -> f32;
    fn request_redraw(&self);
    fn set_title(&self, title: &str);
    fn set_folder(&self, _folder: Option<&FsPath>) {}
    fn set_selection(&self, _selection: &dyn Fn() -> SelectionSource) -> bool {
        true
    }
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
    fn set_clipboard_image(&self, _image: &Bitmap) {}
    fn set_primary_text(&self, _text: &str) {}
    fn primary_text(&self) -> Option<String> {
        None
    }
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
    fn request_attention(&self) {}
    fn set_taskbar_progress(&self, _progress: Option<(u32, bool)>) {}
    fn notify(&self, _text: &str) {}
    fn set_dark(&self, _dark: bool) {}
    fn set_compact(&self, _on: Option<bool>) {}
    fn host(&self, _cmd: HostCmd) {}
    fn share(&self, _paths: Vec<FsPath>) {}
    fn set_volume_label(&self, _drive: FsPath, _label: String) {}
    fn shell_pick(&self, _cmd: Option<u32>) {}
    fn shell_prefetch(&self, _menu: Option<ShellMenu>) {}
    fn shell_warm(&self, _menus: Vec<ShellMenu>) {}
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DropReply {
    pub effect: u32,
    pub caption: Option<(String, String)>,
    pub folder: Option<FsPath>,
    pub onto: Option<FsPath>,
}

pub trait WindowHandler<W: WindowSystem> {
    fn created(&mut self, window: &W::Window);
    fn event(&mut self, window: &W::Window, event: Event);
    fn hit_test(&mut self, window: &W::Window, x: f32, y: f32) -> HitArea;
    fn redraw(&mut self, window: &W::Window);
    fn woken(&mut self, window: &W::Window);
    fn destroyed(&mut self, window: &W::Window);
    fn drag(&mut self, window: &W::Window, event: DragEvent) -> DropReply;
    fn accessibility(&mut self, _window: &W::Window) -> Option<flit_core::a11y::Tree> {
        None
    }
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
    fn upload_backdrop(&mut self, _image: Option<&Bitmap>) {}
    fn render(&mut self, list: &DrawList) -> Result<(), RenderError>;
}

pub trait Provider: Send + Sync + 'static {
    fn submit(&self, req: ProviderRequest);
}

pub trait Fs: Send + Sync + 'static {
    fn read_dir(&self, dir: &FsPath, out: &mut ListingBuilder<'_>) -> Result<(), FsError>;
    fn seek_penalty(&self, path: &FsPath) -> Option<bool>;
    fn remote(&self, _path: &FsPath) -> bool {
        false
    }
    fn desktop_ini(&self, _dir: &FsPath) -> Option<DesktopIni> {
        None
    }
    fn localized_name(&self, _dir: &FsPath) -> Option<String> {
        None
    }
    fn dir_sizes(&self, dir: &FsPath, items: &[(u32, Vec<u8>)], keep_going: &(dyn Fn() -> bool + Sync), report: &(dyn Fn(u32, DirStats) + Sync)) {
        local::dir_sizes(self, dir, items, keep_going, report)
    }
    fn read_deep(&self, root: &FsPath, out: &mut ListingBuilder<'_>, keep_going: &dyn Fn() -> bool) {
        local::read_deep(self, root, out, keep_going)
    }
    fn hash(&self, file: &FsPath, kinds: &[HashKind], job: &Job) -> Result<Vec<String>, PlatformError> {
        hash_file(file, kinds, job)
    }
    fn cluster_size(&self, _path: &FsPath) -> u64 {
        0
    }
    fn measure(&self, items: &[(FsPath, Option<u64>)], keep_going: &dyn Fn() -> bool, report: &mut dyn FnMut(&flit_core::provider::Totals)) -> flit_core::provider::Totals {
        local::measure(self, items, keep_going, report)
    }
    fn file_system(&self, _path: &FsPath) -> Option<String> {
        None
    }
    fn volume_size(&self, _path: &FsPath) -> u64 {
        0
    }
    fn volume_key(&self, path: &FsPath) -> Vec<u8> {
        path.root_bytes().to_ascii_lowercase()
    }
}

static REMEMBER_RECENT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

pub fn set_remember_recent(on: bool) {
    REMEMBER_RECENT.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub fn remember_recent() -> bool {
    REMEMBER_RECENT.load(std::sync::atomic::Ordering::Relaxed)
}

#[cfg(windows)]
pub fn std_path(p: &FsPath) -> std::path::PathBuf {
    use std::os::windows::ffi::OsStringExt;
    let mut w = p.to_wide(true);
    w.pop();
    std::path::PathBuf::from(std::ffi::OsString::from_wide(&w))
}

#[cfg(unix)]
pub fn std_path(p: &FsPath) -> std::path::PathBuf {
    use std::os::unix::ffi::OsStrExt;
    std::path::PathBuf::from(std::ffi::OsStr::from_bytes(p.as_bytes()))
}

pub trait Watcher: Sized + Send + 'static {
    fn new(events: Sender<ProviderEvent>, wake: Wake) -> Result<Self, PlatformError>;
    fn watch(&mut self, tab: TabId, gen: u64, dir: Option<&FsPath>);
}

pub trait Shell: Sized {
    const BATCHES_FOLDER_ICONS: bool = false;
    fn new() -> Result<Self, PlatformError>;
    fn icon(&mut self, req: &IconRequest) -> Option<Bitmap>;
    fn prepare_icons(&mut self, _reqs: &[IconRequest]) {}
    fn thumbnail(&mut self, _req: &IconRequest) -> Option<Bitmap> {
        None
    }
    fn type_name(&mut self, ext: &[u8]) -> Option<String>;
    fn open(&mut self, path: &FsPath) -> Result<(), PlatformError>;
    fn launch(&mut self, _launch: &Launch) -> Result<(), PlatformError> {
        Err(PlatformError::unsupported("launching programs"))
    }
    fn mount(&mut self, _device: &FsPath) -> Result<FsPath, PlatformError> {
        Err(PlatformError::unsupported("mounting volumes"))
    }
    fn connect(&mut self, _uri: &str) -> Result<FsPath, PlatformError> {
        Err(PlatformError::unsupported("connecting to servers"))
    }
    fn unlock(&mut self, _device: &FsPath, _passphrase: &str) -> Result<FsPath, PlatformError> {
        Err(PlatformError::unsupported("unlocking encrypted volumes"))
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
    fn column_props(&mut self, _path: &FsPath) -> flit_core::listing::ColumnValues {
        flit_core::listing::ColumnValues::default()
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
    fn browse(&mut self, _exts: &[&str], _folder: bool, _start: Option<&FsPath>) -> Option<FsPath> {
        None
    }
    fn access(&mut self, _items: &[FsPath]) -> Option<(Vec<Option<Access>>, Accounts)> {
        None
    }
    fn open_ask(&mut self, _path: &FsPath) -> Option<flit_core::provider::OpenAsk> {
        None
    }
    fn needs_exec_bit(&mut self, _path: &FsPath) -> bool {
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
    failure: Option<Sink<Failure>>,
    now: std::sync::Mutex<Shown>,
    performed: std::sync::Mutex<Option<FileOp>>,
}

#[derive(Clone)]
pub struct Job(Arc<JobState>);

impl Job {
    pub fn new(ctl: OpCtl, progress: Sink<OpProgress>, conflict: Sink<Conflict>) -> Self {
        Self(Arc::new(JobState { ctl, progress, conflict, failure: None, now: Default::default(), performed: Default::default() }))
    }

    pub fn asking(ctl: OpCtl, progress: Sink<OpProgress>, conflict: Sink<Conflict>, failure: Sink<Failure>) -> Self {
        Self(Arc::new(JobState { ctl, progress, conflict, failure: Some(failure), now: Default::default(), performed: Default::default() }))
    }

    pub fn performed(&self, op: FileOp) {
        *self.0.performed.lock().unwrap_or_else(|e| e.into_inner()) = Some(op);
    }

    pub fn take_performed(&self) -> Option<FileOp> {
        self.0.performed.lock().unwrap_or_else(|e| e.into_inner()).take()
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
        self.0.ctl.wait_while_paused();
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
        self.0.ctl.resolve(&c, || (self.0.conflict)(c.clone()))
    }

    pub fn room(&self, dest: &FsPath, need: u64, free: impl Fn() -> Option<u64>) -> Result<(), Option<u64>> {
        loop {
            let Some(free) = free().filter(|&free| free < need) else { return Ok(()) };
            if !self.asks_failures() && self.0.ctl.policy().is_some() {
                return Err(Some(free));
            }
            match self.resolve(flit_core::limits::short_of_space(dest, need, free)) {
                Some(flit_core::limits::ROOM_RETRY) => {}
                Some(_) => return Ok(()),
                None => return Err(None),
            }
        }
    }

    pub fn asks_failures(&self) -> bool {
        self.0.failure.is_some()
    }

    pub fn fail(&self, f: Failure) -> Option<Recover> {
        match &self.0.failure {
            Some(sink) => self.0.ctl.recover(f.fatal, || sink(f)),
            None => Some(Recover::Skip),
        }
    }
}

pub type Awake = Box<dyn std::any::Any>;

pub struct KeepAwake<G = Awake> {
    busy: bool,
    guard: Option<G>,
}

impl<G> Default for KeepAwake<G> {
    fn default() -> Self {
        Self { busy: false, guard: None }
    }
}

impl<G> KeepAwake<G> {
    pub fn track(&mut self, busy: bool, acquire: impl FnOnce() -> Option<G>) {
        if busy == self.busy {
            return;
        }
        self.busy = busy;
        self.guard = if busy { acquire() } else { None };
    }

    pub fn held(&self) -> bool {
        self.guard.is_some()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pasted {
    Image(Vec<u8>),
    Text(String),
}

impl Pasted {
    pub fn usable(&self) -> bool {
        match self {
            Pasted::Image(png) => png.starts_with(b"\x89PNG\r\n\x1a\n"),
            Pasted::Text(text) => !text.trim().is_empty(),
        }
    }

    pub fn file(self, dir: &FsPath, taken: impl Fn(&FsPath) -> bool) -> Option<FileOp> {
        if !self.usable() {
            return None;
        }
        let (stem, ext, data) = match self {
            Pasted::Image(png) => (flit_core::i18n::S::OpsExtraPastedImage, ".png", png),
            Pasted::Text(text) => (flit_core::i18n::S::OpsExtraPastedText, ".txt", text.into_bytes()),
        };
        let name = (1u32..).map(|k| if k == 1 { format!("{}{ext}", stem.t()) } else { format!("{} ({k}){ext}", stem.t()) }).find(|n| !taken(&dir.join(n.as_bytes())))?;
        Some(FileOp::NewFile { dir: dir.clone(), name: name.into_bytes(), template: flit_core::provider::Template::Data(data) })
    }
}

pub trait FileOps {
    fn file_op(&mut self, op: &FileOp, job: &Job) -> OpOutcome;
    fn clipboard_files(&mut self) -> Result<(Vec<FsPath>, bool), PlatformError>;
    fn clear_clipboard(&mut self);
    fn link_choices(&mut self, _items: Option<&[FsPath]>, _dest: &FsPath) -> flit_core::provider::LinkChoices {
        Default::default()
    }
    fn clipboard_content(&mut self) -> bool {
        false
    }
    fn clipboard_data(&mut self, _dest: &FsPath) -> Option<FileOp> {
        None
    }
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
    fn snapshot_key(&self, volume: &str) -> String {
        volume.to_owned()
    }
    fn refresh(&mut self) -> bool {
        false
    }
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
    fn cache_dir() -> Option<std::path::PathBuf> {
        Self::state_dir()
    }
    fn config_dir() -> Option<std::path::PathBuf> {
        Self::state_dir()
    }
    fn caps() -> flit_core::caps::Caps {
        flit_core::caps::Caps::default()
    }
    fn recycle_bin() -> bool {
        false
    }
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
        Err(PlatformError::new(-1, flit_core::i18n::S::ErrorsEngineNoLibraries.en()))
    }
    fn recent(_limit: usize) -> (Vec<flit_core::ui::Recent>, bool) {
        (Vec::new(), false)
    }
    fn bookmarks() -> Vec<Location> {
        Vec::new()
    }
    fn double_click_time() -> std::time::Duration {
        std::time::Duration::from_millis(500)
    }
    fn wheel_lines() -> u32 {
        3
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
    fn missing_tools() -> Vec<flit_core::ui::MissingTool> {
        Vec::new()
    }
    fn ui_language() -> String {
        String::new()
    }
    fn integrate(_on: bool) -> Result<(), PlatformError> {
        Err(PlatformError::unsupported("system integration"))
    }
    fn tidy_integration() {}
    fn watch_wsl(_changed: Box<dyn Fn() + Send>) {}
    fn wsl_launcher(_distro: &str, _images: &AgentImages) -> Option<Box<dyn AgentLauncher>> {
        None
    }
    fn file_key(_path: &FsPath) -> Option<FileKey> {
        None
    }
    fn resolve_files(files: &[(FileKey, &FsPath)]) -> Vec<Resolved> {
        files.iter().map(|_| Resolved::Unavailable).collect()
    }
    fn sync_tag_names(_path: &FsPath, _drop: &[String], _keep: &[String]) {}
    fn shell_search(_folder: Option<&FsPath>, _query: &str, _keep_going: &dyn Fn() -> bool, _found: &mut dyn FnMut(FsPath, flit_core::Meta)) -> Result<(), PlatformError> {
        Err(PlatformError::new(-1, flit_core::i18n::S::ErrorsEngineNoWindowsSearch.en()))
    }
    fn download(_url: &str, _out: &mut dyn std::io::Write, _progress: &mut dyn FnMut(u64, Option<u64>) -> bool) -> Result<(), PlatformError> {
        Err(PlatformError::unsupported("downloads"))
    }
    fn wait_exit(_pid: u32, _limit: std::time::Duration) {}
    fn keep_awake(_reason: &str) -> Option<Awake> {
        None
    }
    fn jump_list(_pinned: &[Location], _frequent: &[Location]) {}
    fn never_shown_extensions() -> Vec<String> {
        Vec::new()
    }
    fn run_elevated(_args: &[String]) -> Result<u32, PlatformError> {
        Err(PlatformError::unsupported("running with administrator rights"))
    }

    fn raise_watch_limit() -> flit_core::ui::WatchLimit {
        flit_core::ui::WatchLimit::Failed(PlatformError::unsupported("raising the watch limit").message)
    }
}

pub fn posix_date_patterns(date: &str, time: &str) -> Option<(String, String)> {
    let date = posix_picture(date)?;
    let time = [":ss", ".ss"].iter().fold(posix_picture(time)?, |t, seconds| t.replace(seconds, ""));
    let shows = |p: &str, any: &[char]| p.chars().any(|c| any.contains(&c));
    (shows(&date, &['d']) && shows(&date, &['M']) && shows(&time, &['H', 'h']) && shows(&time, &['m'])).then_some((date, time))
}

fn posix_picture(format: &str) -> Option<String> {
    let mut out = String::new();
    let mut literal = String::new();
    let flush = |out: &mut String, literal: &mut String| {
        if literal.chars().any(|c| c.is_ascii_alphabetic() || c == '\'') {
            out.push('\'');
            out.push_str(&literal.replace('\'', "''"));
            out.push('\'');
        } else {
            out.push_str(literal);
        }
        literal.clear();
    };
    let mut chars = format.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            literal.push(c);
            continue;
        }
        let unpadded = chars.next_if(|&f| matches!(f, '-' | '_' | '0' | '^' | '#')).is_some_and(|f| f == '-');
        chars.next_if(|&m| matches!(m, 'E' | 'O'));
        let spec = chars.next()?;
        let pick = |padded: &'static str, bare: &'static str| if unpadded { bare } else { padded };
        let piece = match spec {
            '%' => {
                literal.push('%');
                continue;
            }
            'n' | 't' => {
                literal.push(' ');
                continue;
            }
            'Y' | 'G' => "yyyy",
            'y' | 'g' => pick("yy", "y"),
            'm' => pick("MM", "M"),
            'd' => pick("dd", "d"),
            'e' => "d",
            'b' | 'h' => "MMM",
            'B' => "MMMM",
            'a' => "ddd",
            'A' => "dddd",
            'H' => pick("HH", "H"),
            'k' => "H",
            'I' => pick("hh", "h"),
            'l' => "h",
            'M' => pick("mm", "m"),
            'S' => pick("ss", "s"),
            'p' | 'P' => "tt",
            'T' => "HH:mm:ss",
            'R' => "HH:mm",
            'r' => "hh:mm:ss tt",
            'D' => "MM/dd/yy",
            'F' => "yyyy-MM-dd",
            _ => return None,
        };
        flush(&mut out, &mut literal);
        out.push_str(piece);
    }
    flush(&mut out, &mut literal);
    Some(out)
}

#[cfg(test)]
#[path = "../test/lib.rs"]
mod tests;
