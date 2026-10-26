use flit_core::ui::Bitmap;
use std::ffi::{c_int, c_ulong, c_void};
use std::sync::OnceLock;

const PIXEL_RGBA: c_int = 7;
const FAST_UPSAMPLE: c_int = 256;
const FAST_DCT: c_int = 2048;

#[repr(C)]
#[derive(Clone, Copy)]
struct Factor {
    num: c_int,
    denom: c_int,
}

struct Api {
    init: unsafe extern "C" fn() -> *mut c_void,
    header: unsafe extern "C" fn(*mut c_void, *const u8, c_ulong, *mut c_int, *mut c_int, *mut c_int, *mut c_int) -> c_int,
    decompress: unsafe extern "C" fn(*mut c_void, *const u8, c_ulong, *mut u8, c_int, c_int, c_int, c_int, c_int) -> c_int,
    destroy: unsafe extern "C" fn(*mut c_void) -> c_int,
    factors: Vec<Factor>,
}

unsafe impl Send for Api {}
unsafe impl Sync for Api {}

unsafe fn cast<T: Copy>(p: *mut c_void) -> T {
    std::mem::transmute_copy(&p)
}

fn load() -> Option<Api> {
    let lib = unsafe { libc::dlopen(c"libturbojpeg.so.0".as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
    if lib.is_null() {
        return None;
    }
    let sym = |name: &std::ffi::CStr| {
        let p = unsafe { libc::dlsym(lib, name.as_ptr()) };
        (!p.is_null()).then_some(p)
    };
    unsafe {
        let factors: unsafe extern "C" fn(*mut c_int) -> *const Factor = cast(sym(c"tjGetScalingFactors")?);
        let mut n = 0;
        let list = factors(&mut n);
        if list.is_null() || n <= 0 {
            return None;
        }
        Some(Api {
            init: cast(sym(c"tjInitDecompress")?),
            header: cast(sym(c"tjDecompressHeader3")?),
            decompress: cast(sym(c"tjDecompress2")?),
            destroy: cast(sym(c"tjDestroy")?),
            factors: std::slice::from_raw_parts(list, n as usize).to_vec(),
        })
    }
}

fn api() -> Option<&'static Api> {
    static API: OnceLock<Option<Api>> = OnceLock::new();
    API.get_or_init(load).as_ref()
}

fn scaled(dim: c_int, f: Factor) -> c_int {
    (dim * f.num + f.denom - 1) / f.denom
}

pub fn scale_for(width: c_int, height: c_int, size: u32, factors: &[(c_int, c_int)]) -> (c_int, c_int) {
    let target = size.max(1) as c_int;
    factors
        .iter()
        .map(|&(num, denom)| Factor { num, denom })
        .filter(|&f| scaled(width.max(height), f) >= target.min(width.max(height)))
        .min_by_key(|&f| scaled(width, f) as i64 * scaled(height, f) as i64)
        .map_or((width, height), |f| (scaled(width, f), scaled(height, f)))
}

pub fn decode(data: &[u8], size: u32) -> Option<Bitmap> {
    let api = api()?;
    unsafe {
        let handle = (api.init)();
        if handle.is_null() {
            return None;
        }
        let (mut w, mut h, mut sub, mut space) = (0, 0, 0, 0);
        let mut out = None;
        if (api.header)(handle, data.as_ptr(), data.len() as c_ulong, &mut w, &mut h, &mut sub, &mut space) == 0 && w > 0 && h > 0 {
            let factors: Vec<(c_int, c_int)> = api.factors.iter().map(|f| (f.num, f.denom)).collect();
            let (sw, sh) = scale_for(w, h, size, &factors);
            let mut rgba = vec![0u8; sw as usize * sh as usize * 4];
            if (api.decompress)(handle, data.as_ptr(), data.len() as c_ulong, rgba.as_mut_ptr(), sw, sw * 4, sh, PIXEL_RGBA, FAST_UPSAMPLE | FAST_DCT) == 0 {
                out = Some(Bitmap { width: sw as u32, height: sh as u32, rgba });
            }
        }
        (api.destroy)(handle);
        out
    }
}

#[cfg(test)]
#[path = "../test/turbo.rs"]
mod tests;
