use std::ffi::{c_void, CString};
use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=shaders/ui.hlsl");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let src = fs::read_to_string("shaders/ui.hlsl").unwrap();
    for (entry, target, file) in [("vs_main", "vs_5_0", "ui_vs.cso"), ("ps_main", "ps_5_0", "ui_ps.cso")] {
        let code = compile(&src, entry, target).unwrap_or_else(|e| panic!("{entry}: {e}"));
        fs::write(out.join(file), code).unwrap();
    }
}

#[cfg(windows)]
fn compile(src: &str, entry: &str, target: &str) -> Result<Vec<u8>, String> {
    #[repr(C)]
    struct BlobVtbl {
        _qi: usize,
        _add_ref: usize,
        release: unsafe extern "system" fn(*mut c_void) -> u32,
        ptr: unsafe extern "system" fn(*mut c_void) -> *const u8,
        size: unsafe extern "system" fn(*mut c_void) -> usize,
    }
    #[link(name = "d3dcompiler_47", kind = "raw-dylib")]
    extern "system" {
        fn D3DCompile(
            src: *const c_void,
            len: usize,
            name: *const i8,
            defines: *const c_void,
            include: *const c_void,
            entry: *const i8,
            target: *const i8,
            flags1: u32,
            flags2: u32,
            code: *mut *mut c_void,
            errors: *mut *mut c_void,
        ) -> i32;
    }
    unsafe fn take(blob: *mut c_void) -> Vec<u8> {
        let vt = &**(blob as *const *const BlobVtbl);
        let v = std::slice::from_raw_parts((vt.ptr)(blob), (vt.size)(blob)).to_vec();
        (vt.release)(blob);
        v
    }
    const OPTIMIZATION_LEVEL3: u32 = 1 << 15;
    let (name, entry, target) = (CString::new("ui.hlsl").unwrap(), CString::new(entry).unwrap(), CString::new(target).unwrap());
    let (mut code, mut errors) = (std::ptr::null_mut(), std::ptr::null_mut());
    unsafe {
        let hr = D3DCompile(
            src.as_ptr().cast(),
            src.len(),
            name.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            entry.as_ptr(),
            target.as_ptr(),
            OPTIMIZATION_LEVEL3,
            0,
            &mut code,
            &mut errors,
        );
        let log = if errors.is_null() { Vec::new() } else { take(errors) };
        if hr < 0 {
            return Err(String::from_utf8_lossy(&log).into_owned());
        }
        Ok(take(code))
    }
}

#[cfg(not(windows))]
fn compile(_: &str, _: &str, _: &str) -> Result<Vec<u8>, String> {
    Err("cross-compiling to Windows needs a Windows host for D3DCompile".into())
}
