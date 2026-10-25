#![cfg(target_os = "linux")]

mod fonts;
mod fs;
mod gl;
mod input;
mod instance;
mod places;
mod shell;
mod text;
mod window;
mod xdg;

pub use window::{LinuxWaker, LinuxWindow, LinuxWindows};

use flit_core::atlas::Slot;
use flit_core::render::DrawList;
use flit_core::text::{FontMetrics, FontStyle, GlyphBitmap, GlyphKey, ShapedGlyph, ShapedText, TextBackend};
use flit_core::{FsPath, Location};
use flit_platform::{Platform, PlatformError, RenderError, Renderer, Window};
use flit_platform_linux::{DbusInstance, FreedesktopShell, Inotify, LinuxFs, Systemd, WalkIndex};
use flit_core::ui::Bitmap;
use gl::{GlDevice, GlRenderer};
use std::path::PathBuf;
use std::sync::Arc;
use winit::raw_window_handle::{HasDisplayHandle, HasWindowHandle};

pub struct Linux;

pub(crate) fn dark_mode_from_env() -> bool {
    std::env::var("GTK_THEME").is_ok_and(|t| t.to_ascii_lowercase().contains("dark"))
}

fn xdg_dir(var: &str, fallback: &str) -> Option<PathBuf> {
    let base = std::env::var_os(var).map(PathBuf::from).filter(|p| p.is_absolute());
    base.or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(fallback))).map(|d| d.join("flit"))
}

impl Platform for Linux {
    type WindowSystem = LinuxWindows;
    type Renderer = LinuxRenderer;
    type Text = StandInText;
    type Fs = LinuxFs;
    type Watcher = Inotify;
    type Shell = FreedesktopShell;
    type Instance = DbusInstance;
    type IndexSource = WalkIndex;
    type Service = Systemd;
    type FallbackIndex = WalkIndex;

    fn text() -> Result<StandInText, PlatformError> {
        Ok(StandInText)
    }

    fn fs() -> LinuxFs {
        LinuxFs
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
        dark_mode_from_env()
    }

    fn caps() -> flit_core::caps::Caps {
        flit_core::caps::Caps { unix_paths: true }
    }

    fn state_dir() -> Option<PathBuf> {
        xdg_dir("XDG_STATE_HOME", ".local/state")
    }

    fn cache_dir() -> Option<PathBuf> {
        xdg_dir("XDG_CACHE_HOME", ".cache")
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

    fn drives() -> Vec<flit_core::ui::Drive> {
        Vec::new()
    }

    fn wsl_distros() -> Vec<String> {
        Vec::new()
    }
}

pub struct StandInText;

impl TextBackend for StandInText {
    fn shape(&mut self, text: &str, style: FontStyle, out: &mut ShapedText) {
        let advance = style.px() * 0.55;
        out.glyphs.clear();
        out.glyphs.extend(text.char_indices().enumerate().map(|(i, (at, _))| ShapedGlyph { font: 0, id: 0, x: i as f32 * advance, y: 0.0, cluster: at as u32 }));
        out.width = out.glyphs.len() as f32 * advance;
    }

    fn rasterize(&mut self, _: GlyphKey, _: &mut GlyphBitmap) -> bool {
        false
    }

    fn metrics(&mut self, style: FontStyle) -> FontMetrics {
        FontMetrics { ascent: style.px() * 0.8, descent: style.px() * 0.2 }
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

    fn render(&mut self, list: &DrawList) -> Result<(), RenderError> {
        self.window.pre_present_notify();
        self.gl.render(list)
    }
}
