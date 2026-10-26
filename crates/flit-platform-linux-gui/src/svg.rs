use flit_core::ui::Bitmap;
use std::ffi::{c_int, c_void, CStr};
use std::sync::OnceLock;

#[repr(C)]
struct Viewport {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

struct Rsvg {
    new_from_data: unsafe extern "C" fn(*const u8, usize, *mut *mut c_void) -> *mut c_void,
    render_document: unsafe extern "C" fn(*mut c_void, *mut c_void, *const Viewport, *mut *mut c_void) -> c_int,
    unref: unsafe extern "C" fn(*mut c_void),
    surface_create: unsafe extern "C" fn(c_int, c_int, c_int) -> *mut c_void,
    surface_flush: unsafe extern "C" fn(*mut c_void),
    surface_data: unsafe extern "C" fn(*mut c_void) -> *mut u8,
    surface_stride: unsafe extern "C" fn(*mut c_void) -> c_int,
    surface_destroy: unsafe extern "C" fn(*mut c_void),
    cairo_create: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    cairo_destroy: unsafe extern "C" fn(*mut c_void),
}

unsafe impl Send for Rsvg {}
unsafe impl Sync for Rsvg {}

unsafe fn cast<T: Copy>(p: *mut c_void) -> T {
    std::mem::transmute_copy(&p)
}

fn load() -> Option<Rsvg> {
    let lib = unsafe { libc::dlopen(c"librsvg-2.so.2".as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
    if lib.is_null() {
        return None;
    }
    let sym = |name: &CStr| {
        let p = unsafe { libc::dlsym(lib, name.as_ptr()) };
        (!p.is_null()).then_some(p)
    };
    unsafe {
        Some(Rsvg {
            new_from_data: cast(sym(c"rsvg_handle_new_from_data")?),
            render_document: cast(sym(c"rsvg_handle_render_document")?),
            unref: cast(sym(c"g_object_unref")?),
            surface_create: cast(sym(c"cairo_image_surface_create")?),
            surface_flush: cast(sym(c"cairo_surface_flush")?),
            surface_data: cast(sym(c"cairo_image_surface_get_data")?),
            surface_stride: cast(sym(c"cairo_image_surface_get_stride")?),
            surface_destroy: cast(sym(c"cairo_surface_destroy")?),
            cairo_create: cast(sym(c"cairo_create")?),
            cairo_destroy: cast(sym(c"cairo_destroy")?),
        })
    }
}

pub fn available() -> bool {
    api().is_some()
}

fn api() -> Option<&'static Rsvg> {
    static API: OnceLock<Option<Rsvg>> = OnceLock::new();
    API.get_or_init(load).as_ref()
}

pub fn render(data: &[u8], size: u32) -> Option<Bitmap> {
    let r = api()?;
    let side = size.clamp(1, 4096) as c_int;
    unsafe {
        let handle = (r.new_from_data)(data.as_ptr(), data.len(), std::ptr::null_mut());
        if handle.is_null() {
            return None;
        }
        let surface = (r.surface_create)(0, side, side);
        let cr = (r.cairo_create)(surface);
        let viewport = Viewport { x: 0.0, y: 0.0, width: side as f64, height: side as f64 };
        let ok = (r.render_document)(handle, cr, &viewport, std::ptr::null_mut()) != 0;
        (r.cairo_destroy)(cr);
        (r.unref)(handle);
        (r.surface_flush)(surface);
        let (data, stride) = ((r.surface_data)(surface), (r.surface_stride)(surface) as usize);
        let out = (ok && !data.is_null()).then(|| {
            let mut rgba = Vec::with_capacity(side as usize * side as usize * 4);
            for y in 0..side as usize {
                let row = std::slice::from_raw_parts(data.add(y * stride), side as usize * 4);
                rgba.extend(row.chunks_exact(4).flat_map(|p| [p[2], p[1], p[0], p[3]]));
            }
            Bitmap { width: side as u32, height: side as u32, rgba }
        });
        (r.surface_destroy)(surface);
        out
    }
}
