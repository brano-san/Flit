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
use glow::HasContext;
use glutin::config::ConfigTemplateBuilder;
use glutin::context::{ContextApi, ContextAttributesBuilder, NotCurrentGlContext, PossiblyCurrentContext, PossiblyCurrentGlContext, Version};
use glutin::display::{Display, DisplayApiPreference, GlDisplay};
use glutin::surface::{GlSurface, Surface, SurfaceAttributesBuilder, SwapInterval, WindowSurface};
use std::num::NonZeroU32;
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
    type Renderer = StandInRenderer;
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

pub struct StandInDevice;

pub struct StandInRenderer {
    window: Arc<dyn winit::window::Window>,
    surface: Surface<WindowSurface>,
    context: PossiblyCurrentContext,
    gl: glow::Context,
}

fn gl_error(e: impl std::fmt::Display) -> PlatformError {
    PlatformError::new(-1, format!("OpenGL ES: {e}"))
}

fn extent(v: u32) -> NonZeroU32 {
    NonZeroU32::new(v).unwrap_or(NonZeroU32::MIN)
}

thread_local! {
    static GL_DISPLAY: std::cell::OnceCell<Display> = const { std::cell::OnceCell::new() };
}

impl Renderer for StandInRenderer {
    type Window = LinuxWindow;
    type Device = StandInDevice;

    fn device(software: bool) -> Result<StandInDevice, PlatformError> {
        match software {
            true => Err(PlatformError::unsupported("software OpenGL ES")),
            false => Ok(StandInDevice),
        }
    }

    fn new(_: &StandInDevice, window: &LinuxWindow, _: u32) -> Result<Self, PlatformError> {
        let raw = window.winit().clone();
        let display = GL_DISPLAY.with(|d| match d.get() {
            Some(display) => Ok(display.clone()),
            None => {
                let owned = window::display().ok_or_else(|| gl_error("no display connection"))?;
                let handle = owned.display_handle().map_err(gl_error)?.as_raw();
                let display = unsafe { Display::new(handle, DisplayApiPreference::Egl) }.map_err(gl_error)?;
                Ok::<_, PlatformError>(d.get_or_init(|| display).clone())
            }
        })?;
        let config = unsafe { display.find_configs(ConfigTemplateBuilder::new().with_alpha_size(8).build()) }.map_err(gl_error)?.next().ok_or_else(|| gl_error("no matching config"))?;
        let (w, h) = window.size();
        let surface_attrs = SurfaceAttributesBuilder::<WindowSurface>::new().build(raw.window_handle().map_err(gl_error)?.as_raw(), extent(w), extent(h));
        let surface = unsafe { display.create_window_surface(&config, &surface_attrs) }.map_err(gl_error)?;
        let context_attrs = ContextAttributesBuilder::new().with_context_api(ContextApi::Gles(Some(Version::new(3, 0)))).build(None);
        let context = unsafe { display.create_context(&config, &context_attrs) }.map_err(gl_error)?.make_current(&surface).map_err(gl_error)?;
        let interval = if window.is_wayland() { SwapInterval::DontWait } else { SwapInterval::Wait(NonZeroU32::MIN) };
        if let Err(e) = surface.set_swap_interval(&context, interval) {
            tracing::warn!("swap interval: {e}");
        }
        let gl = unsafe { glow::Context::from_loader_function_cstr(|s| display.get_proc_address(s)) };
        Ok(Self { window: raw, surface, context, gl })
    }

    fn resize(&mut self, width: u32, height: u32) {
        self.surface.resize(&self.context, extent(width), extent(height));
    }

    fn upload(&mut self, _: Slot, _: &[u8]) {}

    fn render(&mut self, list: &DrawList) -> Result<(), RenderError> {
        self.context.make_current(&self.surface).map_err(|e| RenderError::Other(gl_error(e)))?;
        let [r, g, b, a] = list.clear.0.to_be_bytes().map(|c| c as f32 / 255.0);
        unsafe {
            self.gl.clear_color(r, g, b, a);
            self.gl.clear(glow::COLOR_BUFFER_BIT);
        }
        self.window.pre_present_notify();
        self.surface.swap_buffers(&self.context).map_err(|e| RenderError::Other(gl_error(e)))
    }
}
