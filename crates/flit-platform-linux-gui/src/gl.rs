use flit_core::atlas::Slot;
use flit_core::render::{DrawList, Instance};
use flit_core::thumbs::THUMB_ATLAS;
use flit_core::ui::Bitmap;
use flit_platform::{PlatformError, RenderError};
use glow::HasContext;
use glutin::api::egl::config::Config;
use glutin::api::egl::context::PossiblyCurrentContext;
use glutin::api::egl::device::Device;
use glutin::api::egl::display::Display;
use glutin::api::egl::surface::Surface;
use glutin::config::{Api, ColorBufferType, ConfigSurfaceTypes, ConfigTemplateBuilder, GlConfig};
use glutin::context::{ContextApi, ContextAttributesBuilder, Robustness, Version};
use glutin::error::ErrorKind;
use glutin::prelude::*;
use glutin::surface::{SurfaceAttributesBuilder, SwapInterval, WindowSurface};
use raw_window_handle::{RawDisplayHandle, RawWindowHandle};
use std::cell::Cell;
use std::ffi::CStr;
use std::num::NonZeroU32;
use std::rc::Rc;
use std::thread::ThreadId;

mod shader {
    include!(concat!(env!("OUT_DIR"), "/ui_glsl.rs"));
}

#[repr(C)]
struct Globals {
    viewport: [f32; 2],
    text_contrast: f32,
    pad: f32,
}

type ResetStatus = unsafe extern "system" fn() -> u32;

struct Shared {
    gl: glow::Context,
    context: PossiblyCurrentContext,
    display: Display,
    config: Config,
    program: glow::Program,
    empty: glow::Texture,
    reset_status: Option<ResetStatus>,
    render_thread: Cell<Option<ThreadId>>,
}

pub struct GlDevice {
    shared: Rc<Shared>,
    pub software: bool,
}

unsafe impl Send for GlDevice {}

fn err(e: impl std::fmt::Display) -> PlatformError {
    PlatformError::new(-1, e.to_string())
}

fn classify(e: glutin::error::Error) -> RenderError {
    if e.error_kind() == ErrorKind::ContextLost {
        RenderError::DeviceLost
    } else {
        RenderError::Other(err(e))
    }
}

fn size(v: u32) -> NonZeroU32 {
    NonZeroU32::new(v.max(1)).unwrap()
}

unsafe fn program(gl: &glow::Context) -> Result<glow::Program, String> {
    let program = gl.create_program()?;
    let mut shaders = Vec::new();
    for (kind, src) in [(glow::VERTEX_SHADER, shader::VERTEX), (glow::FRAGMENT_SHADER, shader::FRAGMENT)] {
        let s = gl.create_shader(kind)?;
        gl.shader_source(s, src);
        gl.compile_shader(s);
        if !gl.get_shader_compile_status(s) {
            return Err(gl.get_shader_info_log(s));
        }
        gl.attach_shader(program, s);
        shaders.push(s);
    }
    gl.link_program(program);
    for s in shaders {
        gl.detach_shader(program, s);
        gl.delete_shader(s);
    }
    if !gl.get_program_link_status(program) {
        return Err(gl.get_program_info_log(program));
    }
    gl.use_program(Some(program));
    for name in shader::UNIFORM_BLOCKS {
        let index = gl.get_uniform_block_index(program, name).ok_or_else(|| format!("uniform block {name}"))?;
        gl.uniform_block_binding(program, index, 0);
    }
    for &(name, unit) in shader::SAMPLERS {
        gl.uniform_1_i32(gl.get_uniform_location(program, name).as_ref(), unit as i32);
    }
    Ok(program)
}

unsafe fn replace_image(gl: &glow::Context, slot: &mut Option<glow::Texture>, image: Option<&Bitmap>) {
    if let Some(old) = slot.take() {
        gl.delete_texture(old);
    }
    *slot = image.filter(|b| b.width > 0 && b.height > 0).and_then(|b| {
        let tex = texture(gl, b.width, b.height).map_err(|e| tracing::warn!("image texture: {e}")).ok()?;
        update(gl, tex, Slot { x: 0, y: 0, w: b.width, h: b.height }, &b.rgba);
        Some(tex)
    });
}

unsafe fn texture(gl: &glow::Context, w: u32, h: u32) -> Result<glow::Texture, PlatformError> {
    let t = gl.create_texture().map_err(err)?;
    gl.bind_texture(glow::TEXTURE_2D, Some(t));
    gl.tex_storage_2d(glow::TEXTURE_2D, 1, glow::RGBA8, w as i32, h as i32);
    Ok(t)
}

unsafe fn cleared_texture(gl: &glow::Context, side: u32) -> Result<glow::Texture, PlatformError> {
    let t = texture(gl, side, side)?;
    let fbo = gl.create_framebuffer().map_err(err)?;
    gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
    gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(t), 0);
    gl.clear_color(0.0, 0.0, 0.0, 0.0);
    gl.clear(glow::COLOR_BUFFER_BIT);
    gl.bind_framebuffer(glow::FRAMEBUFFER, None);
    gl.delete_framebuffer(fbo);
    Ok(t)
}

unsafe fn update(gl: &glow::Context, tex: glow::Texture, slot: Slot, rgba: &[u8]) {
    gl.bind_texture(glow::TEXTURE_2D, Some(tex));
    gl.tex_sub_image_2d(
        glow::TEXTURE_2D,
        0,
        slot.x as i32,
        slot.y as i32,
        slot.w as i32,
        slot.h as i32,
        glow::RGBA,
        glow::UNSIGNED_BYTE,
        glow::PixelUnpackData::Slice(Some(&rgba[..(slot.w * slot.h * 4) as usize])),
    );
}

impl GlDevice {
    pub fn new(display: RawDisplayHandle) -> Result<Self, PlatformError> {
        let display = unsafe { Display::new(display) }.map_err(err)?;
        Self::with_display(display, ConfigSurfaceTypes::WINDOW)
    }

    pub fn headless() -> Result<Self, PlatformError> {
        let devices: Vec<Device> = Device::query_devices().map_err(err)?.collect();
        let device = devices
            .iter()
            .find(|d| d.extensions().contains("EGL_MESA_device_software"))
            .or(devices.first())
            .ok_or_else(|| PlatformError::new(-1, "no EGL device"))?;
        let display = unsafe { Display::with_device(device, None) }.map_err(err)?;
        Self::with_display(display, ConfigSurfaceTypes::empty())
    }

    pub fn config(&self) -> &Config {
        &self.shared.config
    }

    fn with_display(display: Display, surfaces: ConfigSurfaceTypes) -> Result<Self, PlatformError> {
        let _span = tracing::info_span!("gl_device").entered();
        let template = ConfigTemplateBuilder::new()
            .with_alpha_size(0)
            .with_depth_size(0)
            .with_stencil_size(0)
            .with_api(Api::GLES3)
            .with_surface_type(surfaces)
            .build();
        let rgb8 = Some(ColorBufferType::Rgb { r_size: 8, g_size: 8, b_size: 8 });
        let config = unsafe { display.find_configs(template) }
            .map_err(err)?
            .filter(|c| c.color_buffer_type() == rgb8 && c.num_samples() <= 1)
            .min_by_key(|c| (c.alpha_size(), c.depth_size(), c.stencil_size()))
            .ok_or_else(|| PlatformError::new(-1, "no 8-bit OpenGL ES 3 config"))?;
        let attrs = |robustness| {
            ContextAttributesBuilder::new()
                .with_context_api(ContextApi::Gles(Some(Version::new(3, 0))))
                .with_robustness(robustness)
                .build(None)
        };
        let (context, robust) = match unsafe { display.create_context(&config, &attrs(Robustness::RobustLoseContextOnReset)) } {
            Ok(c) => (c, true),
            Err(_) => (unsafe { display.create_context(&config, &attrs(Robustness::NotRobust)) }.map_err(err)?, false),
        };
        let context = context.make_current_surfaceless().map_err(err)?;
        unsafe {
            let gl = glow::Context::from_loader_function_cstr(|s| display.get_proc_address(s));
            let renderer = gl.get_parameter_string(glow::RENDERER);
            let software = ["llvmpipe", "softpipe", "SwiftShader"].iter().any(|s| renderer.contains(s));
            let reset_status = (robust
                && ["GL_KHR_robustness", "GL_EXT_robustness"].iter().any(|e| gl.supported_extensions().contains(*e)))
            .then(|| {
                [c"glGetGraphicsResetStatusKHR", c"glGetGraphicsResetStatusEXT", c"glGetGraphicsResetStatus"]
                    .into_iter()
                    .map(|name: &CStr| display.get_proc_address(name))
                    .find(|p| !p.is_null())
                    .map(|p| std::mem::transmute::<*const std::ffi::c_void, ResetStatus>(p))
            })
            .flatten();
            let program = program(&gl).map_err(|e| PlatformError::new(-1, format!("ui shader: {e}")))?;
            let sampler = gl.create_sampler().map_err(err)?;
            for (name, value) in [
                (glow::TEXTURE_MIN_FILTER, glow::LINEAR),
                (glow::TEXTURE_MAG_FILTER, glow::LINEAR),
                (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
                (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
            ] {
                gl.sampler_parameter_i32(sampler, name, value as i32);
            }
            for unit in 0..3 {
                gl.bind_sampler(unit, Some(sampler));
            }
            let empty = cleared_texture(&gl, 1)?;
            gl.disable(glow::DITHER);
            gl.enable(glow::BLEND);
            gl.blend_func(glow::ONE, glow::ONE_MINUS_SRC_ALPHA);
            context.make_not_current_in_place().map_err(err)?;
            tracing::info!(renderer, robust, "gl device ready");
            let shared = Shared {
                gl,
                context,
                display,
                config,
                program,
                empty,
                reset_status,
                render_thread: Cell::new(None),
            };
            Ok(Self { shared: Rc::new(shared), software })
        }
    }

    fn bind_to_current_thread(&self) -> Result<(), PlatformError> {
        let me = std::thread::current().id();
        let owner = *self.shared.render_thread.get().get_or_insert(me);
        self.shared.render_thread.set(Some(owner));
        if owner != me {
            return Err(PlatformError::new(-1, "the graphics device already renders on another thread"));
        }
        Ok(())
    }
}

impl Shared {
    fn has_context(&self) -> bool {
        self.context.is_current() || self.context.make_current_surfaceless().is_ok()
    }
}

impl Drop for Shared {
    fn drop(&mut self) {
        if self.context.is_current() {
            let _ = self.context.make_not_current_in_place();
        }
    }
}

enum Target {
    Window(Surface<WindowSurface>),
    #[cfg_attr(not(test), allow(dead_code))]
    Offscreen(glow::Framebuffer, glow::Texture),
}

pub struct GlRenderer {
    shared: Rc<Shared>,
    target: Target,
    vao: glow::VertexArray,
    instances: glow::Buffer,
    globals: glow::Buffer,
    atlas: glow::Texture,
    thumbs: Option<glow::Texture>,
    preview: Option<glow::Texture>,
    backdrop: Option<glow::Texture>,
    size: (u32, u32),
}

impl GlRenderer {
    pub fn new(dev: &GlDevice, window: RawWindowHandle, (w, h): (u32, u32), atlas_size: u32) -> Result<Self, PlatformError> {
        let _span = tracing::info_span!("renderer_new", software = dev.software).entered();
        dev.bind_to_current_thread()?;
        let s = &dev.shared;
        let attrs = SurfaceAttributesBuilder::<WindowSurface>::new().with_srgb(Some(false)).build(window, size(w), size(h));
        let surface = unsafe { s.display.create_window_surface(&s.config, &attrs) }.map_err(err)?;
        s.context.make_current(&surface).map_err(err)?;
        Self::with_target(dev, Target::Window(surface), (w, h), atlas_size)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn offscreen(dev: &GlDevice, (w, h): (u32, u32), atlas_size: u32) -> Result<Self, PlatformError> {
        dev.bind_to_current_thread()?;
        let s = &dev.shared;
        s.context.make_current_surfaceless().map_err(err)?;
        unsafe {
            let fbo = s.gl.create_framebuffer().map_err(err)?;
            let tex = texture(&s.gl, w.max(1), h.max(1))?;
            s.gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
            s.gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(tex), 0);
            Self::with_target(dev, Target::Offscreen(fbo, tex), (w, h), atlas_size)
        }
    }

    fn with_target(dev: &GlDevice, target: Target, (w, h): (u32, u32), atlas_size: u32) -> Result<Self, PlatformError> {
        let gl = &dev.shared.gl;
        unsafe {
            let instances = gl.create_buffer().map_err(err)?;
            let globals = gl.create_buffer().map_err(err)?;
            let vao = gl.create_vertex_array().map_err(err)?;
            gl.bind_vertex_array(Some(vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(instances));
            let stride = size_of::<Instance>() as i32;
            for (loc, n, ty, offset) in [
                (0, 4, glow::FLOAT, 0),
                (1, 4, glow::FLOAT, 16),
                (2, 4, glow::FLOAT, 32),
                (3, 4, glow::UNSIGNED_BYTE, 48),
                (4, 2, glow::FLOAT, 52),
            ] {
                gl.vertex_attrib_pointer_f32(loc, n, ty, ty == glow::UNSIGNED_BYTE, stride, offset);
            }
            gl.vertex_attrib_pointer_i32(5, 1, glow::UNSIGNED_INT, stride, 60);
            for loc in 0..6 {
                gl.enable_vertex_attrib_array(loc);
                gl.vertex_attrib_divisor(loc, 1);
            }
            gl.bind_vertex_array(None);
            let atlas = cleared_texture(gl, atlas_size)?;
            tracing::info!(software = dev.software, "renderer ready");
            Ok(Self {
                shared: dev.shared.clone(),
                target,
                vao,
                instances,
                globals,
                atlas,
                thumbs: None,
                preview: None,
                backdrop: None,
                size: (w.max(1), h.max(1)),
            })
        }
    }

    fn make_current(&self) -> Result<(), RenderError> {
        let s = &self.shared;
        match &self.target {
            Target::Window(surface) => s.context.make_current(surface),
            Target::Offscreen(..) => s.context.make_current_surfaceless(),
        }
        .map_err(classify)
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        let (width, height) = (width.max(1), height.max(1));
        if (width, height) == self.size {
            return;
        }
        self.size = (width, height);
        let s = &self.shared;
        match &mut self.target {
            Target::Window(surface) => surface.resize(&s.context, size(width), size(height)),
            Target::Offscreen(fbo, tex) => unsafe {
                if !s.has_context() {
                    return;
                }
                s.gl.delete_texture(*tex);
                match texture(&s.gl, width, height) {
                    Ok(t) => *tex = t,
                    Err(e) => return tracing::warn!("offscreen texture: {e}"),
                }
                s.gl.bind_framebuffer(glow::FRAMEBUFFER, Some(*fbo));
                s.gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(*tex), 0);
            },
        }
    }

    pub fn set_swap_interval(&self, interval: u32) -> Result<(), RenderError> {
        let Target::Window(surface) = &self.target else { return Ok(()) };
        self.make_current()?;
        let interval = NonZeroU32::new(interval).map_or(SwapInterval::DontWait, SwapInterval::Wait);
        surface.set_swap_interval(&self.shared.context, interval).map_err(classify)
    }

    pub fn upload(&mut self, slot: Slot, rgba: &[u8]) {
        if self.shared.has_context() {
            unsafe { update(&self.shared.gl, self.atlas, slot, rgba) };
        }
    }

    pub fn upload_thumb(&mut self, slot: Slot, rgba: &[u8]) {
        if !self.shared.has_context() {
            return;
        }
        let gl = &self.shared.gl;
        if self.thumbs.is_none() {
            match unsafe { cleared_texture(gl, THUMB_ATLAS) } {
                Ok(t) => self.thumbs = Some(t),
                Err(e) => return tracing::warn!("thumbnail texture: {e}"),
            }
        }
        if let Some(tex) = self.thumbs {
            unsafe { update(gl, tex, slot, rgba) };
        }
    }

    pub fn upload_preview(&mut self, image: Option<&Bitmap>) {
        if self.shared.has_context() {
            unsafe { replace_image(&self.shared.gl, &mut self.preview, image) };
        }
    }

    pub fn upload_backdrop(&mut self, image: Option<&Bitmap>) {
        if self.shared.has_context() {
            unsafe { replace_image(&self.shared.gl, &mut self.backdrop, image) };
        }
    }

    pub fn render(&mut self, list: &DrawList) -> Result<(), RenderError> {
        let _span = tracing::trace_span!("render", n = list.instances.len()).entered();
        self.make_current()?;
        let s = &*self.shared;
        let gl = &s.gl;
        let (w, h) = self.size;
        unsafe {
            let fbo = match &self.target {
                Target::Window(_) => None,
                Target::Offscreen(fbo, _) => Some(*fbo),
            };
            gl.bind_framebuffer(glow::FRAMEBUFFER, fbo);
            gl.viewport(0, 0, w as i32, h as i32);
            let [r, g, b, a] = list.clear.to_f32();
            gl.clear_color(r, g, b, a);
            gl.clear(glow::COLOR_BUFFER_BIT);
            gl.use_program(Some(s.program));
            let globals = Globals { viewport: [w as f32, h as f32], text_contrast: 0.5, pad: 0.0 };
            gl.bind_buffer(glow::UNIFORM_BUFFER, Some(self.globals));
            gl.buffer_data_u8_slice(glow::UNIFORM_BUFFER, bytes(std::slice::from_ref(&globals)), glow::STREAM_DRAW);
            gl.bind_buffer_base(glow::UNIFORM_BUFFER, 0, Some(self.globals));
            for (unit, tex) in [Some(self.atlas), self.thumbs, self.preview, self.backdrop].into_iter().enumerate() {
                gl.active_texture(glow::TEXTURE0 + unit as u32);
                gl.bind_texture(glow::TEXTURE_2D, Some(tex.unwrap_or(s.empty)));
            }
            let n = list.instances.len();
            if n > 0 {
                gl.bind_vertex_array(Some(self.vao));
                gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.instances));
                gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes(&list.instances), glow::STREAM_DRAW);
                gl.draw_arrays_instanced(glow::TRIANGLE_STRIP, 0, 4, n as i32);
                gl.bind_vertex_array(None);
            }
            if s.reset_status.is_some_and(|status| status() != glow::NO_ERROR) {
                return Err(RenderError::DeviceLost);
            }
        }
        match &self.target {
            Target::Window(surface) => surface.swap_buffers(&s.context).map_err(classify),
            Target::Offscreen(..) => Ok(()),
        }
    }
}

fn bytes<T>(data: &[T]) -> &[u8] {
    unsafe { std::slice::from_raw_parts(data.as_ptr().cast(), size_of_val(data)) }
}

impl Drop for GlRenderer {
    fn drop(&mut self) {
        let s = &self.shared;
        if !s.has_context() {
            return;
        }
        let gl = &s.gl;
        unsafe {
            gl.delete_vertex_array(self.vao);
            gl.delete_buffer(self.instances);
            gl.delete_buffer(self.globals);
            for tex in [Some(self.atlas), self.thumbs, self.preview, self.backdrop].into_iter().flatten() {
                gl.delete_texture(tex);
            }
            if let Target::Offscreen(fbo, tex) = self.target {
                gl.delete_framebuffer(fbo);
                gl.delete_texture(tex);
            }
        }
    }
}

#[cfg(test)]
#[path = "../test/gl.rs"]
pub(crate) mod tests;
