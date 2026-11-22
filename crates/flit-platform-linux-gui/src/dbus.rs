use flit_core::open::OpenTarget;
use flit_core::Open;
use flit_platform::{OpenSink, Wake};
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::ptr::NonNull;

#[path = "tray.rs"]
pub(crate) mod tray;

pub(crate) struct Bus {
    pub(crate) api: &'static Api,
    pub(crate) conn: *mut c_void,
}

impl Bus {
    pub(crate) fn session() -> Option<Self> {
        let api = load()?;
        let address = CString::new(session_address()?).ok()?;
        unsafe {
            let mut err = Error::new();
            (api.error_init)(&mut err);
            let conn = (api.open_private)(address.as_ptr(), &mut err);
            if conn.is_null() {
                (api.error_free)(&mut err);
                return None;
            }
            let bus = Self { api, conn };
            let registered = (bus.api.register)(conn, &mut err) != 0;
            (bus.api.error_free)(&mut err);
            registered.then_some(bus)
        }
    }

    pub(crate) fn call(&self, dest: &CStr, path: &CStr, interface: &CStr, member: &CStr, args: &[crate::atspi::V]) -> Option<Vec<crate::atspi::Arg>> {
        unsafe {
            let api = self.api;
            let msg = (api.new_method_call)(dest.as_ptr(), path.as_ptr(), interface.as_ptr(), member.as_ptr());
            if msg.is_null() {
                return None;
            }
            self.append(msg, args);
            let mut err = Error::new();
            (api.error_init)(&mut err);
            let result = (api.call)(self.conn, msg, 1000, &mut err);
            (api.error_free)(&mut err);
            (api.unref)(msg);
            if result.is_null() {
                return None;
            }
            let values = crate::atspi::args(api, result);
            (api.unref)(result);
            Some(values)
        }
    }

    unsafe fn append(&self, msg: *mut c_void, args: &[crate::atspi::V]) {
        let mut iter = Iter::new();
        (self.api.iter_init_append)(msg, &mut iter);
        for arg in args {
            crate::atspi::put(self.api, &mut iter, "", arg);
        }
    }

    pub(crate) unsafe fn send(&self, msg: *mut c_void, args: &[crate::atspi::V]) {
        if !msg.is_null() {
            self.append(msg, args);
            (self.api.send)(self.conn, msg, std::ptr::null_mut());
            (self.api.unref)(msg);
        }
    }

    pub(crate) unsafe fn respond(&self, request: *mut c_void, args: &[crate::atspi::V]) {
        self.send((self.api.new_return)(request), args);
    }

    pub(crate) unsafe fn signal(&self, path: &CStr, interface: &CStr, member: &CStr, args: &[crate::atspi::V]) {
        self.send((self.api.new_signal)(path.as_ptr(), interface.as_ptr(), member.as_ptr()), args);
    }

    pub(crate) fn own(&self, name: &CStr) -> bool {
        unsafe {
            let mut err = Error::new();
            (self.api.error_init)(&mut err);
            let result = (self.api.request_name)(self.conn, name.as_ptr(), NAME_DO_NOT_QUEUE, &mut err);
            (self.api.error_free)(&mut err);
            result == PRIMARY_OWNER
        }
    }

    pub(crate) fn subscribe(&self, rule: &CStr) {
        unsafe {
            let mut err = Error::new();
            (self.api.error_init)(&mut err);
            (self.api.add_match)(self.conn, rule.as_ptr(), &mut err);
            (self.api.error_free)(&mut err);
        }
    }
}

impl Drop for Bus {
    fn drop(&mut self) {
        unsafe {
            (self.api.close)(self.conn);
            (self.api.release)(self.conn);
        }
    }
}

pub const NAME: &CStr = c"org.freedesktop.FileManager1";
const INTROSPECTION: &CStr = c"<node><interface name=\"org.freedesktop.FileManager1\">\
<method name=\"ShowFolders\"><arg type=\"as\" direction=\"in\"/><arg type=\"s\" direction=\"in\"/></method>\
<method name=\"ShowItems\"><arg type=\"as\" direction=\"in\"/><arg type=\"s\" direction=\"in\"/></method>\
<method name=\"ShowItemProperties\"><arg type=\"as\" direction=\"in\"/><arg type=\"s\" direction=\"in\"/></method>\
</interface></node>";
const TYPE_METHOD_CALL: c_int = 1;
const TYPE_ARRAY: c_int = b'a' as c_int;
const TYPE_STRING: c_int = b's' as c_int;
const NAME_DO_NOT_QUEUE: u32 = 4;
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
        unsafe { text(self.message) }
    }

    pub(crate) fn name(&self) -> String {
        unsafe { text(self.name) }
    }
}

#[repr(C, align(8))]
pub(crate) struct Iter([u8; 128]);

impl Iter {
    pub(crate) fn new() -> Self {
        Iter([0; 128])
    }
}

type AddWatch = unsafe extern "C" fn(*mut c_void, *mut c_void) -> u32;
type WatchFunction = unsafe extern "C" fn(*mut c_void, *mut c_void);
type FreeData = unsafe extern "C" fn(*mut c_void);

crate::symbols::library_api! {
    pub(crate) struct Api from c"libdbus-1.so.3" {
        threads_init = c"dbus_threads_init_default": fn() -> u32;
        pub(crate) error_init = c"dbus_error_init": fn(*mut Error);
        pub(crate) error_free = c"dbus_error_free": fn(*mut Error);
        pub(crate) open_private = c"dbus_connection_open_private": fn(*const c_char, *mut Error) -> *mut c_void;
        pub(crate) register = c"dbus_bus_register": fn(*mut c_void, *mut Error) -> u32;
        pub(crate) request_name = c"dbus_bus_request_name": fn(*mut c_void, *const c_char, u32, *mut Error) -> c_int;
        pub(crate) read_write = c"dbus_connection_read_write": fn(*mut c_void, c_int) -> u32;
        pub(crate) pop = c"dbus_connection_pop_message": fn(*mut c_void) -> *mut c_void;
        pub(crate) msg_type = c"dbus_message_get_type": fn(*mut c_void) -> c_int;
        pub(crate) member = c"dbus_message_get_member": fn(*mut c_void) -> *const c_char;
        pub(crate) sender = c"dbus_message_get_sender": fn(*mut c_void) -> *const c_char;
        pub(crate) interface = c"dbus_message_get_interface": fn(*mut c_void) -> *const c_char;
        pub(crate) iter_init = c"dbus_message_iter_init": fn(*mut c_void, *mut Iter) -> u32;
        pub(crate) iter_type = c"dbus_message_iter_get_arg_type": fn(*mut Iter) -> c_int;
        pub(crate) iter_recurse = c"dbus_message_iter_recurse": fn(*mut Iter, *mut Iter);
        pub(crate) iter_basic = c"dbus_message_iter_get_basic": fn(*mut Iter, *mut c_void);
        pub(crate) iter_next = c"dbus_message_iter_next": fn(*mut Iter) -> u32;
        pub(crate) iter_init_append = c"dbus_message_iter_init_append": fn(*mut c_void, *mut Iter);
        pub(crate) iter_append_basic = c"dbus_message_iter_append_basic": fn(*mut Iter, c_int, *const c_void) -> u32;
        pub(crate) new_return = c"dbus_message_new_method_return": fn(*mut c_void) -> *mut c_void;
        pub(crate) send = c"dbus_connection_send": fn(*mut c_void, *mut c_void, *mut u32) -> u32;
        pub(crate) flush = c"dbus_connection_flush": fn(*mut c_void);
        pub(crate) unref = c"dbus_message_unref": fn(*mut c_void);
        pub(crate) new_method_call = c"dbus_message_new_method_call": fn(*const c_char, *const c_char, *const c_char, *const c_char) -> *mut c_void;
        pub(crate) new_signal = c"dbus_message_new_signal": fn(*const c_char, *const c_char, *const c_char) -> *mut c_void;
        pub(crate) new_error = c"dbus_message_new_error": fn(*mut c_void, *const c_char, *const c_char) -> *mut c_void;
        pub(crate) call = c"dbus_connection_send_with_reply_and_block": fn(*mut c_void, *mut c_void, c_int, *mut Error) -> *mut c_void;
        pub(crate) open_container = c"dbus_message_iter_open_container": fn(*mut Iter, c_int, *const c_char, *mut Iter) -> u32;
        pub(crate) close_container = c"dbus_message_iter_close_container": fn(*mut Iter, *mut Iter) -> u32;
        pub(crate) add_match = c"dbus_bus_add_match": fn(*mut c_void, *const c_char, *mut Error);
        pub(crate) path = c"dbus_message_get_path": fn(*mut c_void) -> *const c_char;
        pub(crate) unique_name = c"dbus_bus_get_unique_name": fn(*mut c_void) -> *const c_char;
        pub(crate) unix_fd = c"dbus_connection_get_unix_fd": fn(*mut c_void, *mut c_int) -> u32;
        pub(crate) dispatch_status = c"dbus_connection_get_dispatch_status": fn(*mut c_void) -> c_int;
        pub(crate) is_connected = c"dbus_connection_get_is_connected": fn(*mut c_void) -> u32;
        pub(crate) set_watches = c"dbus_connection_set_watch_functions": fn(*mut c_void, Option<AddWatch>, Option<WatchFunction>, Option<WatchFunction>, *mut c_void, Option<FreeData>) -> u32;
        pub(crate) watch_fd = c"dbus_watch_get_unix_fd": fn(*mut c_void) -> c_int;
        pub(crate) watch_flags = c"dbus_watch_get_flags": fn(*mut c_void) -> u32;
        pub(crate) watch_enabled = c"dbus_watch_get_enabled": fn(*mut c_void) -> u32;
        pub(crate) watch_handle = c"dbus_watch_handle": fn(*mut c_void, u32) -> u32;
        #[cfg(test)]
        pub(crate) signature = c"dbus_message_get_signature": fn(*mut c_void) -> *const c_char;
        pub(crate) close = c"dbus_connection_close": fn(*mut c_void);
        pub(crate) release = c"dbus_connection_unref": fn(*mut c_void);
        pub(crate) reply_serial = c"dbus_message_get_reply_serial": fn(*mut c_void) -> u32;
        pub(crate) error_name = c"dbus_message_get_error_name": fn(*mut c_void) -> *const c_char;
    }
}

pub(crate) fn load() -> Option<&'static Api> {
    static API: std::sync::OnceLock<Option<Api>> = std::sync::OnceLock::new();
    API.get_or_init(|| {
        let api = Api::load()?;
        (unsafe { (api.threads_init)() } != 0).then_some(api)
    }).as_ref()
}

pub fn request(method: &str, uris: &[Vec<u8>]) -> Option<Open> {
    let paths: Vec<Vec<u8>> = uris.iter().filter_map(|u| crate::instance::decode_uri(u).or_else(|| std::str::from_utf8(u).ok().filter(|s| flit_core::ui::server_uri(s).is_some()).map(|_| u.clone()))).collect();
    if paths.is_empty() {
        return None;
    }
    let open = Open { target: OpenTarget::NewTab, ..Default::default() };
    match method {
        "ShowFolders" => Some(Open { paths, ..open }),
        "ShowItems" => Some(Open { select: paths, ..open }),
        "ShowItemProperties" => Some(Open { select: paths, properties: true, ..open }),
        _ => None,
    }
}

pub(crate) unsafe fn text(p: *const c_char) -> String {
    if p.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
}

unsafe fn string(api: &Api, it: &mut Iter) -> Option<Vec<u8>> {
    if (api.iter_type)(it) != TYPE_STRING {
        return None;
    }
    let mut s: *const c_char = std::ptr::null();
    (api.iter_basic)(it, (&mut s as *mut *const c_char).cast());
    (!s.is_null()).then(|| CStr::from_ptr(s).to_bytes().to_vec())
}

unsafe fn uris_and_startup_id(api: &Api, msg: *mut c_void) -> (Vec<Vec<u8>>, String) {
    let (mut top, mut inner) = (Iter::new(), Iter::new());
    if (api.iter_init)(msg, &mut top) == 0 || (api.iter_type)(&mut top) != TYPE_ARRAY {
        return (Vec::new(), String::new());
    }
    (api.iter_recurse)(&mut top, &mut inner);
    let mut out = Vec::new();
    while let Some(s) = string(api, &mut inner) {
        out.push(s);
        if (api.iter_next)(&mut inner) == 0 {
            break;
        }
    }
    let startup_id = if (api.iter_next)(&mut top) != 0 { string(api, &mut top) } else { None };
    (out, startup_id.map(|s| String::from_utf8_lossy(&s).into_owned()).unwrap_or_default())
}

struct BusError<'a>(&'a Api, Error);

impl<'a> BusError<'a> {
    fn new(api: &'a Api) -> Self {
        let mut error = Self(api, Error::new());
        unsafe { (api.error_init)(&mut error.1) };
        error
    }
}

impl Drop for BusError<'_> {
    fn drop(&mut self) {
        unsafe { (self.0.error_free)(&mut self.1) };
    }
}

struct Connection<'a> {
    api: &'a Api,
    raw: NonNull<c_void>,
}

impl<'a> Connection<'a> {
    fn open(api: &'a Api, address: &CStr, name: Option<&CStr>) -> Result<Self, String> {
        let mut err = BusError::new(api);
        let conn = unsafe {
            let raw = (api.open_private)(address.as_ptr(), &mut err.1);
            let raw = NonNull::new(raw).ok_or_else(|| err.1.message())?;
            let conn = Self { api, raw };
            if (api.register)(raw.as_ptr(), &mut err.1) == 0
                || name.is_some_and(|name| (api.request_name)(raw.as_ptr(), name.as_ptr(), NAME_DO_NOT_QUEUE, &mut err.1) != PRIMARY_OWNER)
            {
                return Err(err.1.message());
            }
            conn
        };
        Ok(conn)
    }

    fn pop(&self) -> Option<Message<'a>> {
        let raw = NonNull::new(unsafe { (self.api.pop)(self.raw.as_ptr()) })?;
        Some(Message { api: self.api, raw })
    }

    fn wait(&self, timeout: c_int) -> bool {
        unsafe { (self.api.read_write)(self.raw.as_ptr(), timeout) != 0 }
    }

    fn send(&self, msg: &Message<'_>) -> bool {
        unsafe { (self.api.send)(self.raw.as_ptr(), msg.raw.as_ptr(), std::ptr::null_mut()) != 0 }
    }

    fn flush(&self) {
        unsafe { (self.api.flush)(self.raw.as_ptr()) };
    }
}

impl Drop for Connection<'_> {
    fn drop(&mut self) {
        unsafe {
            (self.api.close)(self.raw.as_ptr());
            (self.api.release)(self.raw.as_ptr());
        }
    }
}

struct Message<'a> {
    api: &'a Api,
    raw: NonNull<c_void>,
}

impl Message<'_> {
    fn method(&self) -> Option<(String, String)> {
        let raw = self.raw.as_ptr();
        unsafe {
            ((self.api.msg_type)(raw) == TYPE_METHOD_CALL).then(|| (text((self.api.member)(raw)), text((self.api.interface)(raw))))
        }
    }

    fn args(&self) -> (Vec<Vec<u8>>, String) {
        unsafe { uris_and_startup_id(self.api, self.raw.as_ptr()) }
    }

    fn method_return(&self, xml: Option<&CStr>) -> Option<Self> {
        unsafe {
            let raw = NonNull::new((self.api.new_return)(self.raw.as_ptr()))?;
            if let Some(xml) = xml {
                let mut it = Iter::new();
                (self.api.iter_init_append)(raw.as_ptr(), &mut it);
                let p = xml.as_ptr();
                (self.api.iter_append_basic)(&mut it, TYPE_STRING, (&p as *const *const c_char).cast());
            }
            Some(Self { api: self.api, raw })
        }
    }

    fn reply(&self, conn: &Connection<'_>, xml: Option<&CStr>) {
        if let Some(r) = self.method_return(xml) {
            conn.send(&r);
            conn.flush();
        }
    }
}

impl Drop for Message<'_> {
    fn drop(&mut self) {
        unsafe { (self.api.unref)(self.raw.as_ptr()) };
    }
}
pub fn session_address() -> Option<String> {
    ["DBUS_SESSION_BUS_ADDRESS", "DBUS_STARTER_ADDRESS"]
        .iter()
        .find_map(|k| std::env::var(k).ok().filter(|a| !a.is_empty()))
        .or_else(|| Some(format!("unix:path={}/bus", crate::xdg::runtime_dir()?.display())))
}

pub fn serve(address: &str, opens: OpenSink, wake: Wake, ready: impl FnOnce(bool)) {
    let Some(api) = load() else { return ready(false) };
    let Ok(addr) = CString::new(address) else { return ready(false) };
    let conn = match Connection::open(api, &addr, Some(NAME)) {
        Ok(conn) => conn,
        Err(error) => {
            tracing::info!("not serving {}: {error}", NAME.to_string_lossy());
            return ready(false);
        }
    };
    ready(true);
    loop {
        while let Some(msg) = conn.pop() {
            let Some((member, iface)) = msg.method() else { continue };
            if member == "Introspect" {
                msg.reply(&conn, Some(INTROSPECTION));
            } else if iface.is_empty() || iface == "org.freedesktop.FileManager1" {
                let (uris, startup_id) = msg.args();
                if let Some(open) = request(&member, &uris) {
                    crate::window::startup_token(startup_id);
                    let sent = opens.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(|tx| tx.send(open).is_ok());
                    if sent {
                        wake();
                    }
                }
                msg.reply(&conn, None);
            }
        }
        if !conn.wait(-1) {
            break;
        }
    }
}
pub fn start(opens: OpenSink, wake: Wake) {
    let Some(address) = session_address() else { return };
    let _ = std::thread::Builder::new().name("flit-dbus".into()).spawn(move || serve(&address, opens, wake, |_| {}));
}

fn launcher_values(percent: Option<u32>) -> (f64, bool) {
    (percent.map_or(0.0, |v| f64::from(v.min(100)) / 100.0), percent.is_some())
}

fn launcher_updates(rx: crossbeam_channel::Receiver<Option<u32>>, mut send: impl FnMut((f64, bool)) -> bool) {
    while let Ok(percent) = rx.recv() {
        let percent = rx.try_iter().last().unwrap_or(percent);
        if !send(launcher_values(percent)) {
            return;
        }
    }
}

fn launcher_signal(api: &Api, percent: (f64, bool)) -> Option<Message<'_>> {
    use crate::atspi::{put, V};
    let path = CString::new(format!("/com/canonical/unity/launcherentry/{}", std::process::id())).ok()?;
    unsafe {
        let raw = NonNull::new((api.new_signal)(path.as_ptr(), c"com.canonical.Unity.LauncherEntry".as_ptr(), c"Update".as_ptr()))?;
        let msg = Message { api, raw };
        let mut it = Iter::new();
        (api.iter_init_append)(raw.as_ptr(), &mut it);
        put(api, &mut it, "", &V::S("application://flit.desktop".into()));
        put(api, &mut it, "", &V::Props(vec![("progress".into(), V::D(percent.0)), ("progress-visible".into(), V::B(percent.1))]));
        Some(msg)
    }
}

fn serve_launcher(rx: crossbeam_channel::Receiver<Option<u32>>) {
    let Some(address) = session_address().and_then(|a| CString::new(a).ok()) else { return };
    let Some(api) = load() else { return };
    let Ok(conn) = Connection::open(api, &address, None) else { return };
    launcher_updates(rx, |value| {
        let Some(msg) = launcher_signal(api, value) else { return false };
        let sent = conn.send(&msg);
        if sent {
            conn.flush();
        }
        let alive = conn.wait(0);
        while conn.pop().is_some() {}
        sent && alive
    });
}
pub(crate) fn launcher_progress(percent: Option<u32>) {
    static WORKER: std::sync::OnceLock<Option<crossbeam_channel::Sender<Option<u32>>>> = std::sync::OnceLock::new();
    if let Some(tx) = WORKER.get_or_init(|| {
        let (tx, rx) = crossbeam_channel::unbounded();
        std::thread::Builder::new().name("flit-launcher".into()).spawn(move || serve_launcher(rx)).ok().map(|_| tx)
    }) {
        let _ = tx.send(percent);
    }
}

#[cfg(test)]
#[path = "../test/dbus.rs"]
mod tests;
