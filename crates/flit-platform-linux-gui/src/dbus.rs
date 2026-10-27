use flit_core::open::OpenTarget;
use flit_core::Open;
use flit_platform::{OpenSink, Wake};
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::sync::{Condvar, Mutex, MutexGuard};

pub const NAME: &CStr = c"org.freedesktop.FileManager1";
const INTROSPECTION: &CStr = c"<node><interface name=\"org.freedesktop.FileManager1\">\
<method name=\"ShowFolders\"><arg type=\"as\" direction=\"in\"/><arg type=\"s\" direction=\"in\"/></method>\
<method name=\"ShowItems\"><arg type=\"as\" direction=\"in\"/><arg type=\"s\" direction=\"in\"/></method>\
<method name=\"ShowItemProperties\"><arg type=\"as\" direction=\"in\"/><arg type=\"s\" direction=\"in\"/></method>\
</interface></node>";
const TYPE_METHOD_CALL: c_int = 1;
const TYPE_ARRAY: c_int = b'a' as c_int;
const TYPE_STRING: c_int = b's' as c_int;
const NAME_REPLACE_EXISTING: u32 = 2;
const NAME_DO_NOT_QUEUE: u32 = 4;
const POLL_MS: c_int = 250;
const PRIMARY_OWNER: c_int = 1;

#[repr(C)]
pub(crate) struct Error {
    name: *const c_char,
    message: *const c_char,
    flags: u32,
    padding: *mut c_void,
}

impl Error {
    pub(crate) fn new() -> Self {
        Error { name: std::ptr::null(), message: std::ptr::null(), flags: 0, padding: std::ptr::null_mut() }
    }

    pub(crate) fn message(&self) -> String {
        text(self.message)
    }
}

#[repr(C, align(8))]
pub(crate) struct Iter([u8; 128]);

impl Iter {
    pub(crate) fn new() -> Self {
        Iter([0; 128])
    }
}

pub(crate) struct Api {
    pub(crate) error_init: unsafe extern "C" fn(*mut Error),
    pub(crate) error_free: unsafe extern "C" fn(*mut Error),
    pub(crate) open_private: unsafe extern "C" fn(*const c_char, *mut Error) -> *mut c_void,
    pub(crate) register: unsafe extern "C" fn(*mut c_void, *mut Error) -> u32,
    request_name: unsafe extern "C" fn(*mut c_void, *const c_char, u32, *mut Error) -> c_int,
    pub(crate) read_write: unsafe extern "C" fn(*mut c_void, c_int) -> u32,
    pub(crate) pop: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    pub(crate) msg_type: unsafe extern "C" fn(*mut c_void) -> c_int,
    pub(crate) member: unsafe extern "C" fn(*mut c_void) -> *const c_char,
    pub(crate) interface: unsafe extern "C" fn(*mut c_void) -> *const c_char,
    pub(crate) iter_init: unsafe extern "C" fn(*mut c_void, *mut Iter) -> u32,
    pub(crate) iter_type: unsafe extern "C" fn(*mut Iter) -> c_int,
    pub(crate) iter_recurse: unsafe extern "C" fn(*mut Iter, *mut Iter),
    pub(crate) iter_basic: unsafe extern "C" fn(*mut Iter, *mut c_void),
    pub(crate) iter_next: unsafe extern "C" fn(*mut Iter) -> u32,
    pub(crate) iter_init_append: unsafe extern "C" fn(*mut c_void, *mut Iter),
    pub(crate) iter_append_basic: unsafe extern "C" fn(*mut Iter, c_int, *const c_void) -> u32,
    pub(crate) new_return: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    pub(crate) send: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut u32) -> u32,
    pub(crate) flush: unsafe extern "C" fn(*mut c_void),
    pub(crate) unref: unsafe extern "C" fn(*mut c_void),
    pub(crate) new_method_call: unsafe extern "C" fn(*const c_char, *const c_char, *const c_char, *const c_char) -> *mut c_void,
    pub(crate) new_signal: unsafe extern "C" fn(*const c_char, *const c_char, *const c_char) -> *mut c_void,
    pub(crate) new_error: unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char) -> *mut c_void,
    pub(crate) call: unsafe extern "C" fn(*mut c_void, *mut c_void, c_int, *mut Error) -> *mut c_void,
    pub(crate) open_container: unsafe extern "C" fn(*mut Iter, c_int, *const c_char, *mut Iter) -> u32,
    pub(crate) close_container: unsafe extern "C" fn(*mut Iter, *mut Iter) -> u32,
    pub(crate) add_match: unsafe extern "C" fn(*mut c_void, *const c_char, *mut Error),
    pub(crate) path: unsafe extern "C" fn(*mut c_void) -> *const c_char,
    pub(crate) unique_name: unsafe extern "C" fn(*mut c_void) -> *const c_char,
    pub(crate) unix_fd: unsafe extern "C" fn(*mut c_void, *mut c_int) -> u32,
    pub(crate) dispatch_status: unsafe extern "C" fn(*mut c_void) -> c_int,
    pub(crate) close: unsafe extern "C" fn(*mut c_void),
    pub(crate) release: unsafe extern "C" fn(*mut c_void),
}

unsafe fn cast<T: Copy>(p: *mut c_void) -> T {
    std::mem::transmute_copy(&p)
}

pub(crate) fn load() -> Option<Api> {
    let lib = unsafe { libc::dlopen(c"libdbus-1.so.3".as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
    if lib.is_null() {
        return None;
    }
    let sym = |name: &CStr| {
        let p = unsafe { libc::dlsym(lib, name.as_ptr()) };
        (!p.is_null()).then_some(p)
    };
    unsafe {
        Some(Api {
            error_init: cast(sym(c"dbus_error_init")?),
            error_free: cast(sym(c"dbus_error_free")?),
            open_private: cast(sym(c"dbus_connection_open_private")?),
            register: cast(sym(c"dbus_bus_register")?),
            request_name: cast(sym(c"dbus_bus_request_name")?),
            read_write: cast(sym(c"dbus_connection_read_write")?),
            pop: cast(sym(c"dbus_connection_pop_message")?),
            msg_type: cast(sym(c"dbus_message_get_type")?),
            member: cast(sym(c"dbus_message_get_member")?),
            interface: cast(sym(c"dbus_message_get_interface")?),
            iter_init: cast(sym(c"dbus_message_iter_init")?),
            iter_type: cast(sym(c"dbus_message_iter_get_arg_type")?),
            iter_recurse: cast(sym(c"dbus_message_iter_recurse")?),
            iter_basic: cast(sym(c"dbus_message_iter_get_basic")?),
            iter_next: cast(sym(c"dbus_message_iter_next")?),
            iter_init_append: cast(sym(c"dbus_message_iter_init_append")?),
            iter_append_basic: cast(sym(c"dbus_message_iter_append_basic")?),
            new_return: cast(sym(c"dbus_message_new_method_return")?),
            send: cast(sym(c"dbus_connection_send")?),
            flush: cast(sym(c"dbus_connection_flush")?),
            unref: cast(sym(c"dbus_message_unref")?),
            new_method_call: cast(sym(c"dbus_message_new_method_call")?),
            new_signal: cast(sym(c"dbus_message_new_signal")?),
            new_error: cast(sym(c"dbus_message_new_error")?),
            call: cast(sym(c"dbus_connection_send_with_reply_and_block")?),
            open_container: cast(sym(c"dbus_message_iter_open_container")?),
            close_container: cast(sym(c"dbus_message_iter_close_container")?),
            add_match: cast(sym(c"dbus_bus_add_match")?),
            path: cast(sym(c"dbus_message_get_path")?),
            unique_name: cast(sym(c"dbus_bus_get_unique_name")?),
            unix_fd: cast(sym(c"dbus_connection_get_unix_fd")?),
            dispatch_status: cast(sym(c"dbus_connection_get_dispatch_status")?),
            close: cast(sym(c"dbus_connection_close")?),
            release: cast(sym(c"dbus_connection_unref")?),
        })
    }
}

pub fn request(method: &str, uris: &[Vec<u8>]) -> Option<Open> {
    let paths: Vec<Vec<u8>> = uris.iter().filter_map(|u| crate::instance::decode_uri(u)).collect();
    if paths.is_empty() {
        return None;
    }
    let open = Open { target: OpenTarget::NewTab, ..Default::default() };
    match method {
        "ShowFolders" => Some(Open { paths, ..open }),
        "ShowItems" | "ShowItemProperties" => Some(Open { select: paths, ..open }),
        _ => None,
    }
}

pub(crate) fn text(p: *const c_char) -> String {
    if p.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
}

unsafe fn uris(api: &Api, msg: *mut c_void) -> Vec<Vec<u8>> {
    let (mut top, mut inner) = (Iter::new(), Iter::new());
    if (api.iter_init)(msg, &mut top) == 0 || (api.iter_type)(&mut top) != TYPE_ARRAY {
        return Vec::new();
    }
    (api.iter_recurse)(&mut top, &mut inner);
    let mut out = Vec::new();
    while (api.iter_type)(&mut inner) == TYPE_STRING {
        let mut s: *const c_char = std::ptr::null();
        (api.iter_basic)(&mut inner, (&mut s as *mut *const c_char).cast());
        if !s.is_null() {
            out.push(CStr::from_ptr(s).to_bytes().to_vec());
        }
        if (api.iter_next)(&mut inner) == 0 {
            break;
        }
    }
    out
}

unsafe fn reply(api: &Api, conn: *mut c_void, msg: *mut c_void, xml: Option<&CStr>) {
    let r = (api.new_return)(msg);
    if r.is_null() {
        return;
    }
    if let Some(xml) = xml {
        let mut it = Iter::new();
        (api.iter_init_append)(r, &mut it);
        let p = xml.as_ptr();
        (api.iter_append_basic)(&mut it, TYPE_STRING, (&p as *const *const c_char).cast());
    }
    (api.send)(conn, r, std::ptr::null_mut());
    (api.flush)(conn);
    (api.unref)(r);
}

pub fn session_address() -> Option<String> {
    ["DBUS_SESSION_BUS_ADDRESS", "DBUS_STARTER_ADDRESS"]
        .iter()
        .find_map(|k| std::env::var(k).ok().filter(|a| !a.is_empty()))
        .or_else(|| Some(format!("unix:path={}/bus", crate::xdg::runtime_dir()?.display())))
}

pub fn serve(address: &str, opens: OpenSink, wake: Wake, ready: impl FnOnce(bool), keep: impl Fn() -> bool) {
    let Some(api) = load() else { return ready(false) };
    let Ok(addr) = CString::new(address) else { return ready(false) };
    unsafe {
        let mut err = std::mem::zeroed::<Error>();
        (api.error_init)(&mut err);
        let conn = (api.open_private)(addr.as_ptr(), &mut err);
        let owned = !conn.is_null() && (api.register)(conn, &mut err) != 0 && (api.request_name)(conn, NAME.as_ptr(), NAME_DO_NOT_QUEUE | NAME_REPLACE_EXISTING, &mut err) == PRIMARY_OWNER;
        if !owned {
            tracing::info!("not serving {}: {}", NAME.to_string_lossy(), text(err.message));
            (api.error_free)(&mut err);
            if !conn.is_null() {
                (api.close)(conn);
                (api.release)(conn);
            }
            return ready(false);
        }
        ready(true);
        while keep() {
            loop {
                let msg = (api.pop)(conn);
                if msg.is_null() {
                    break;
                }
                if (api.msg_type)(msg) == TYPE_METHOD_CALL {
                    let (member, iface) = (text((api.member)(msg)), text((api.interface)(msg)));
                    if member == "Introspect" {
                        reply(&api, conn, msg, Some(INTROSPECTION));
                    } else if iface.is_empty() || iface == "org.freedesktop.FileManager1" {
                        if let Some(open) = request(&member, &uris(&api, msg)) {
                            let sent = opens.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(|tx| tx.send(open).is_ok());
                            if sent {
                                wake();
                            }
                        }
                        reply(&api, conn, msg, None);
                    }
                }
                (api.unref)(msg);
            }
            if (api.read_write)(conn, POLL_MS) == 0 {
                break;
            }
        }
        (api.close)(conn);
        (api.release)(conn);
    }
}

static WANTED: Mutex<(bool, u64)> = Mutex::new((false, 0));
static CHANGED: Condvar = Condvar::new();

fn wanted() -> MutexGuard<'static, (bool, u64)> {
    WANTED.lock().unwrap_or_else(|e| e.into_inner())
}

fn wait_until(ok: impl Fn(&(bool, u64)) -> bool) -> u64 {
    let mut w = wanted();
    while !ok(&w) {
        w = CHANGED.wait(w).unwrap_or_else(|e| e.into_inner());
    }
    w.1
}

pub fn want(on: bool) {
    let mut w = wanted();
    if w.0 != on {
        *w = (on, w.1 + 1);
        CHANGED.notify_all();
    }
}

pub fn run(address: &str, opens: OpenSink, wake: Wake) {
    loop {
        let turn = wait_until(|w| w.0);
        serve(address, opens.clone(), wake.clone(), |_| {}, || *wanted() == (true, turn));
        wait_until(|w| w.1 != turn);
    }
}

pub fn start(opens: OpenSink, wake: Wake) {
    let Some(address) = session_address() else { return };
    if std::env::args_os().any(|a| a == "--dbus-activated") {
        want(true);
    }
    let _ = std::thread::Builder::new().name("flit-dbus".into()).spawn(move || run(&address, opens, wake));
}

#[cfg(test)]
#[path = "../test/dbus.rs"]
mod tests;
