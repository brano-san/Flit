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
        Ok(StandInText::default())
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

const STAND_IN_FONTS: [&str; 3] = ["/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf", "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf"];

fn stand_in_fonts() -> &'static [Option<Vec<u8>>] {
    static FONTS: std::sync::OnceLock<Vec<Option<Vec<u8>>>> = std::sync::OnceLock::new();
    FONTS.get_or_init(|| STAND_IN_FONTS.iter().map(|p| std::fs::read(p).ok()).collect())
}

#[derive(Default)]
pub struct StandInText {
    scaler: swash::scale::ScaleContext,
}

impl StandInText {
    fn font(index: u16) -> Option<swash::FontRef<'static>> {
        stand_in_fonts().get(index as usize)?.as_deref().and_then(|data| swash::FontRef::from_index(data, 0))
    }

    fn face(style: FontStyle) -> u16 {
        match (style.family, style.bold) {
            (flit_core::text::FontFamily::Mono, _) => 2,
            (_, true) => 1,
            _ => 0,
        }
    }
}

impl TextBackend for StandInText {
    fn shape(&mut self, text: &str, style: FontStyle, out: &mut ShapedText) {
        let face = Self::face(style);
        out.glyphs.clear();
        let mut x = 0.0;
        if let Some(font) = Self::font(face) {
            let (charmap, metrics) = (font.charmap(), font.glyph_metrics(&[]).scale(style.px()));
            for (at, c) in text.char_indices() {
                let id = charmap.map(c);
                out.glyphs.push(ShapedGlyph { font: face, id, x, y: 0.0, cluster: at as u32 });
                x += metrics.advance_width(id);
            }
        }
        out.width = x;
    }

    fn rasterize(&mut self, key: GlyphKey, out: &mut GlyphBitmap) -> bool {
        use swash::scale::{Render, Source};
        let Some(font) = Self::font(key.font) else { return false };
        let mut scaler = self.scaler.builder(font).size(key.size_q as f32 / 4.0).hint(false).build();
        let offset = swash::zeno::Vector::new(key.subpx as f32 / 4.0, 0.0);
        let Some(image) = Render::new(&[Source::Outline]).format(swash::zeno::Format::Alpha).offset(offset).render(&mut scaler, key.id) else { return false };
        out.left = image.placement.left;
        out.top = image.placement.top;
        out.width = image.placement.width;
        out.height = image.placement.height;
        out.data.extend_from_slice(&image.data);
        true
    }

    fn metrics(&mut self, style: FontStyle) -> FontMetrics {
        match Self::font(Self::face(style)) {
            Some(font) => {
                let m = font.metrics(&[]).scale(style.px());
                FontMetrics { ascent: m.ascent, descent: m.descent }
            }
            None => FontMetrics { ascent: style.px() * 0.8, descent: style.px() * 0.2 },
        }
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
