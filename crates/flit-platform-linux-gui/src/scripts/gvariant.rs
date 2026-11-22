use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::ptr::{null, null_mut, NonNull};
use std::sync::OnceLock;

type Parse = unsafe extern "C" fn(*const c_char, *const c_char, *const c_char, *mut *const c_char, *mut *mut c_void) -> *mut c_void;
type Compare = unsafe extern "C" fn(*const c_void, *const c_void) -> c_int;

struct Api {
    parse: Parse,
    kind: unsafe extern "C" fn(*mut c_void) -> *const c_char,
    equal: Compare,
    compare: Compare,
    double: unsafe extern "C" fn(*mut c_void) -> f64,
    unref: unsafe extern "C" fn(*mut c_void),
}

fn api() -> Option<&'static Api> {
    static API: OnceLock<Option<Api>> = OnceLock::new();
    API.get_or_init(|| unsafe {
        let lib = NonNull::new(libc::dlopen(c"libglib-2.0.so.0".as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL))?;
        let symbol = |name: &CStr| NonNull::new(libc::dlsym(lib.as_ptr(), name.as_ptr()));
        let load = || {
            Some(Api {
                parse: std::mem::transmute_copy(&symbol(c"g_variant_parse")?),
                kind: std::mem::transmute_copy(&symbol(c"g_variant_get_type_string")?),
                equal: std::mem::transmute_copy(&symbol(c"g_variant_equal")?),
                compare: std::mem::transmute_copy(&symbol(c"g_variant_compare")?),
                double: std::mem::transmute_copy(&symbol(c"g_variant_get_double")?),
                unref: std::mem::transmute_copy(&symbol(c"g_variant_unref")?),
            })
        };
        let api = load();
        if api.is_none() {
            libc::dlclose(lib.as_ptr());
        }
        api
    })
    .as_ref()
}

struct Value<'a> {
    ptr: NonNull<c_void>,
    api: &'a Api,
}

impl<'a> Value<'a> {
    fn parse(api: &'a Api, kind: Option<&CStr>, text: &str) -> Option<Self> {
        let text = CString::new(text).ok()?;
        let raw = unsafe { (api.parse)(kind.map_or(null(), CStr::as_ptr), text.as_ptr(), null(), null_mut(), null_mut()) };
        Some(Self { ptr: NonNull::new(raw)?, api })
    }

    fn kind(&self) -> &CStr {
        unsafe { CStr::from_ptr((self.api.kind)(self.ptr.as_ptr())) }
    }
}

impl Drop for Value<'_> {
    fn drop(&mut self) {
        unsafe { (self.api.unref)(self.ptr.as_ptr()) }
    }
}

pub(super) fn matches(actual: &str, kind: &str, op: &str, value: &str) -> bool {
    let Some(api) = api() else { return false };
    let Some(actual) = Value::parse(api, None, actual) else { return false };
    if actual.kind().to_bytes() != kind.as_bytes() {
        return false;
    }
    let Some(value) = Value::parse(api, Some(actual.kind()), value) else { return false };
    let (left, right) = (actual.ptr.as_ptr(), value.ptr.as_ptr());
    unsafe {
        if kind == "d" {
            let (a, b) = ((api.double)(left), (api.double)(right));
            return match op {
                "eq" => a == b,
                "ne" => a != b,
                "lt" => a < b,
                "gt" => a > b,
                _ => false,
            };
        }
        match op {
            "eq" => (api.equal)(left, right) != 0,
            "ne" => (api.equal)(left, right) == 0,
            "lt" | "gt" if matches!(kind, "y" | "n" | "q" | "i" | "u" | "x" | "t" | "h") => {
                let order = (api.compare)(left, right);
                (op == "lt" && order < 0) || (op == "gt" && order > 0)
            }
            _ => false,
        }
    }
}
