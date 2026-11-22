#![cfg(target_os = "linux")]

mod activation;
mod alert;
mod apps;
mod awake;
mod clipboard;
mod content;
mod atspi;
mod dbus;
pub use dbus::tray::host_available as tray_host_available;
mod dropped;
pub mod elevated;
mod folder_icons;
mod fonts;
mod gvfs;
pub mod fs;
mod gl;
mod icons;
mod input;
mod locale;
mod mime;
mod mpv;
mod network;
mod poll;
mod preferences;
mod player;
mod props;
pub use flit_platform_linux::portable_metadata;
mod svg;
mod symbols;
pub mod instance;
pub mod places;
pub mod shell;
mod tools;
mod scripts;
mod text;
mod touch;
mod thumb;
mod thumbnailers;
mod tiling;
mod turbo;
mod udisks;
mod update;
mod window;
pub mod file_chooser;
mod wallpaper;
mod wl;
mod xdnd;
mod xds;
pub mod xdg;

#[cfg(test)]
#[path = "../test/temp.rs"]
mod temp;

#[cfg(test)]
#[path = "../test/frame.rs"]
mod frame;

pub use fonts::prefetch as prefetch_fonts;
pub use gl::{GlDevice, GlRenderer};
pub use thumb::decode_webp;
pub use window::{LinuxWaker, LinuxWindow, LinuxWindows};

use flit_core::atlas::Slot;
use flit_core::render::DrawList;

use flit_core::ui::Bitmap;
use flit_core::{FsPath, Location};
use flit_platform::{DevicePlatform, FilePlatform, NetworkPlatform, Platform, PlatformError, ProfilePlatform, RenderError, Renderer, SearchPlatform, ShellPlatform, SystemPlatform, WindowFrame, WindowPlatform};
use flit_platform_linux::{Systemd, WalkIndex};
use std::path::PathBuf;
use std::sync::Arc;
use winit::raw_window_handle::{HasDisplayHandle, HasWindowHandle};

pub struct Linux;

impl Linux {
    pub fn capture_text() -> text::LinuxText {
        text::LinuxText::bundled()
    }
}

impl WindowPlatform for Linux {
    type WindowSystem = LinuxWindows;
    type Renderer = LinuxRenderer;
    type Text = text::LinuxText;

    fn text() -> Result<text::LinuxText, PlatformError> {
        Ok(text::LinuxText::new())
    }

    fn dark_mode() -> bool {
        window::preferred_dark().unwrap_or(false)
    }

    fn double_click_time() -> std::time::Duration {
        input::double_click()
    }

    fn wheel_lines() -> u32 {
        input::wheel_lines()
    }

    fn fatal_error(message: &str) {
        eprintln!("flit: {message}");
        alert::show(message);
    }

    fn refresh_period() -> Option<std::time::Duration> {
        None
    }
}

impl FilePlatform for Linux {
    type Fs = fs::LinuxGuiFs;
    type Watcher = fs::LinuxGuiWatcher;

    fn raise_watch_limit() -> flit_core::ui::WatchLimit {
        fs::raise_watch_limit()
    }

    fn fs() -> fs::LinuxGuiFs {
        fs::LinuxGuiFs
    }

    fn caps() -> flit_core::caps::Caps {
        flit_core::caps::Caps { unix_paths: true }
    }

    fn recycle_bin() -> bool {
        true
    }

    fn file_key(path: &FsPath) -> Option<flit_core::tags::FileKey> {
        flit_platform_linux::file_key(path)
    }

    fn resolve_files(files: &[(flit_core::tags::FileKey, &FsPath)]) -> Vec<flit_platform::Resolved> {
        flit_platform_linux::resolve_files(files)
    }

    fn sync_tag_names(path: &FsPath, drop: &[String], keep: &[String]) {
        flit_platform_linux::sync_tag_names(path, drop, keep)
    }
}

impl ShellPlatform for Linux {
    type Shell = shell::LinuxShell;

    fn recent(limit: usize) -> (Vec<flit_core::ui::Recent>, bool) {
        places::recent(limit)
    }

    fn bookmarks() -> Vec<flit_core::Location> {
        places::bookmarks()
    }

    fn bookmarks_changed(added: &[flit_core::Location], removed: &[flit_core::Location]) {
        places::sync_bookmarks(added, removed)
    }

    fn home() -> FsPath {
        xdg::home().and_then(|h| xdg::fs_path(&h)).unwrap_or_else(FsPath::unix_root)
    }

    fn known_folder(name: &str) -> Option<Location> {
        xdg::known_folder(name)
    }

    fn templates() -> Vec<flit_core::ui::NewTemplate> {
        xdg::templates()
    }
}

impl DevicePlatform for Linux {
    fn drives() -> Vec<flit_core::ui::Drive> {
        places::drives()
    }

    fn wsl_distros() -> Vec<String> {
        Vec::new()
    }

    fn watch_wsl(changed: Box<dyn Fn() + Send>) {
        places::watch_mounts(Box::new(move || {
            changed();
            window::devices_changed();
        }));
        udisks::watch(window::devices_changed);
    }
}

impl ProfilePlatform for Linux {
    fn state_dir() -> Option<PathBuf> {
        xdg::state_dir()
    }

    fn cache_dir() -> Option<PathBuf> {
        xdg::cache_dir()
    }

    fn config_dir() -> Option<PathBuf> {
        xdg::config_dir()
    }
}

impl SystemPlatform for Linux {
    type Instance = instance::SocketInstance;

    fn keep_awake(reason: &str) -> Option<flit_platform::Awake> {
        awake::Awake::hold(reason).map(|a| Box::new(a) as flit_platform::Awake)
    }

    fn utc_offset_minutes() -> i32 {
        let now = unsafe { libc::time(std::ptr::null_mut()) };
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        match unsafe { libc::localtime_r(&now, &mut tm) }.is_null() {
            true => 0,
            false => (tm.tm_gmtoff / 60) as i32,
        }
    }

    fn elevated() -> bool {
        unsafe { libc::geteuid() == 0 }
    }

    fn missing_tools() -> Vec<flit_core::ui::MissingTool> {
        tools::missing()
    }

    fn ui_language() -> String {
        ["LC_ALL", "LC_MESSAGES", "LANG"].iter().filter_map(|k| std::env::var(k).ok()).find(|v| !v.is_empty()).unwrap_or_default()
    }

    fn date_patterns() -> (String, String) {
        locale::date_patterns().unwrap_or_else(|| ("yyyy-MM-dd".into(), "HH:mm".into()))
    }

    fn admin_launcher() -> Option<Box<dyn flit_platform::AgentLauncher>> {
        Some(Box::new(elevated::RootLauncher::pkexec()))
    }

    fn integrate(on: bool) -> Result<(), PlatformError> {
        xdg::desktop_entry(on).and_then(|_| xdg::default_manager(on)).and_then(|_| xdg::bus_activation(on)).map_err(handle_error)
    }

    fn wait_exit(pid: u32, limit: std::time::Duration) {
        update::wait_exit(pid, limit)
    }
}

impl SearchPlatform for Linux {
    type IndexSource = WalkIndex;
    type Service = Systemd;
    type FallbackIndex = WalkIndex;

    fn shell_search(folder: Option<&FsPath>, query: &str, keep_going: &dyn Fn() -> bool, found: &mut dyn FnMut(FsPath, flit_core::Meta)) -> Result<(), PlatformError> {
        content::search(folder, query, keep_going, found)
    }
}

impl NetworkPlatform for Linux {
    fn network_warning() -> Option<flit_core::provider::NetworkWarning> {
        network::discovery_warning(network::avahi_running(), tools::gvfs_network_present(), &tools::os_release())
    }

    fn download(url: &str, out: &mut dyn std::io::Write, progress: &mut dyn FnMut(u64, Option<u64>) -> bool) -> Result<(), PlatformError> {
        update::download(url, out, progress)
    }
}

impl Platform for Linux {}
pub struct LinuxRenderer {
    gl: GlRenderer,
    window: Arc<dyn winit::window::Window>,
}

fn handle_error(e: impl std::fmt::Display) -> PlatformError {
    PlatformError::new(-1, format!("window handle: {e}"))
}

impl Renderer for LinuxRenderer {
    type Window = LinuxWindow;
    type Device = GlDevice;

    fn device(software: bool) -> Result<GlDevice, PlatformError> {
        if software {
            return Err(PlatformError::new(-1, "OpenGL ES has no separate software device, Mesa picks the driver for the display"));
        }
        let display = window::display().ok_or_else(|| PlatformError::new(-1, "no display connection"))?;
        GlDevice::new(display.display_handle().map_err(handle_error)?.as_raw())
    }

    fn new(device: &GlDevice, window: &LinuxWindow, atlas_size: u32) -> Result<Self, PlatformError> {
        let raw = window.winit().clone();
        let gl = GlRenderer::new(device, raw.window_handle().map_err(handle_error)?.as_raw(), window.size(), atlas_size)?;
        if let Err(e) = gl.set_swap_interval(if window.is_wayland() { 0 } else { 1 }) {
            tracing::warn!("swap interval: {e:?}");
        }
        Ok(Self { gl, window: raw })
    }

    fn resize(&mut self, width: u32, height: u32) {
        self.gl.resize(width, height);
    }

    fn upload(&mut self, slot: Slot, rgba: &[u8]) {
        self.gl.upload(slot, rgba);
    }

    fn upload_thumb(&mut self, slot: Slot, rgba: &[u8]) {
        self.gl.upload_thumb(slot, rgba);
    }

    fn upload_preview(&mut self, image: Option<&Bitmap>) {
        self.gl.upload_preview(image);
    }

    fn upload_backdrop(&mut self, image: Option<&Bitmap>) {
        self.gl.upload_backdrop(image);
    }

    fn render(&mut self, list: &DrawList) -> Result<(), RenderError> {
        self.window.pre_present_notify();
        self.gl.render(list)
    }
}
