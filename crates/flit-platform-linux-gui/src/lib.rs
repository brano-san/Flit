#![cfg(target_os = "linux")]

mod apps;
mod dbus;
mod fonts;
pub mod fs;
mod gl;
mod icons;
mod input;
mod mime;
mod props;
mod svg;
pub mod instance;
pub mod places;
pub mod shell;
mod text;
mod thumb;
mod window;
pub mod xdg;

#[cfg(test)]
#[path = "../test/temp.rs"]
mod temp;

#[cfg(test)]
#[path = "../test/frame.rs"]
mod frame;

pub use fonts::prefetch as prefetch_fonts;
pub use window::{LinuxWaker, LinuxWindow, LinuxWindows};

use flit_core::atlas::Slot;
use flit_core::render::DrawList;

use flit_core::ui::Bitmap;
use flit_core::{FsPath, Location};
use flit_platform::{Platform, PlatformError, RenderError, Renderer, Window};
use flit_platform_linux::{Systemd, WalkIndex};
use gl::{GlDevice, GlRenderer};
use std::path::PathBuf;
use std::sync::Arc;
use winit::raw_window_handle::{HasDisplayHandle, HasWindowHandle};

pub struct Linux;

impl Platform for Linux {
    type WindowSystem = LinuxWindows;
    type Renderer = LinuxRenderer;
    type Text = text::LinuxText;
    type Fs = fs::LinuxGuiFs;
    type Watcher = fs::LinuxGuiWatcher;
    type Shell = shell::LinuxShell;
    type Instance = instance::SocketInstance;
    type IndexSource = WalkIndex;
    type Service = Systemd;
    type FallbackIndex = WalkIndex;

    fn text() -> Result<text::LinuxText, PlatformError> {
        Ok(text::LinuxText::new())
    }

    fn fs() -> fs::LinuxGuiFs {
        fs::LinuxGuiFs
    }

    fn utc_offset_minutes() -> i32 {
        let now = unsafe { libc::time(std::ptr::null_mut()) };
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        match unsafe { libc::localtime_r(&now, &mut tm) }.is_null() {
            true => 0,
            false => (tm.tm_gmtoff / 60) as i32,
        }
    }

    fn dark_mode() -> bool {
        window::preferred_dark().unwrap_or(false)
    }

    fn caps() -> flit_core::caps::Caps {
        flit_core::caps::Caps { unix_paths: true }
    }

    fn recycle_bin() -> bool {
        true
    }

    fn recent(limit: usize) -> (Vec<flit_core::ui::Recent>, bool) {
        places::recent(limit)
    }
    fn bookmarks() -> Vec<flit_core::Location> {
        places::bookmarks()
    }

    fn state_dir() -> Option<PathBuf> {
        xdg::state_dir()
    }

    fn cache_dir() -> Option<PathBuf> {
        xdg::cache_dir()
    }

    fn config_dir() -> Option<PathBuf> {
        xdg::config_dir()
    }

    fn home() -> FsPath {
        xdg::home().and_then(|h| xdg::fs_path(&h)).unwrap_or_else(|| FsPath::parse("/").unwrap())
    }

    fn known_folder(name: &str) -> Option<Location> {
        xdg::known_folder(name)
    }

    fn ui_language() -> String {
        ["LC_ALL", "LC_MESSAGES", "LANG"].iter().filter_map(|k| std::env::var(k).ok()).find(|v| !v.is_empty()).unwrap_or_default()
    }

    fn templates() -> Vec<flit_core::ui::NewTemplate> {
        xdg::templates()
    }

    fn fatal_error(message: &str) {
        eprintln!("flit: {message}");
    }

    fn refresh_period() -> Option<std::time::Duration> {
        None
    }

    fn drives() -> Vec<flit_core::ui::Drive> {
        places::drives()
    }

    fn wsl_distros() -> Vec<String> {
        Vec::new()
    }

    fn watch_wsl(changed: Box<dyn Fn() + Send>) {
        places::watch_mounts(changed)
    }
}
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
