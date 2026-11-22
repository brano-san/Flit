use crate::atspi::{args, put, Arg, V};
use crate::dbus::{self, Api, Iter};
use flit_core::i18n::S;
use flit_core::provider::{LoginAsk, MountAsk, MountReply};
use flit_platform::MountPrompt;
use std::ffi::{c_char, c_int, c_void, CString};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

const DAEMON: &str = "org.gtk.vfs.Daemon";
const TRACKER_PATH: &str = "/org/gtk/vfs/mounttracker";
const TRACKER: &str = "org.gtk.vfs.MountTracker";
const OPERATION: &str = "org.gtk.vfs.MountOperation";
const TYPE_METHOD_CALL: c_int = 1;
const TYPE_RETURN: c_int = 2;
const TYPE_ERROR: c_int = 3;
const POLL_MS: c_int = 50;
const UNKNOWN_METHOD: &str = "org.freedesktop.DBus.Error.UnknownMethod";
const INVALID_ARGS: &str = "org.freedesktop.DBus.Error.InvalidArgs";
const NO_GVFS: [&str; 5] = ["org.freedesktop.DBus.Error.ServiceUnknown", "org.freedesktop.DBus.Error.NameHasNoOwner", "org.freedesktop.DBus.Error.UnknownMethod", "org.freedesktop.DBus.Error.UnknownObject", "org.freedesktop.DBus.Error.UnknownInterface"];
const ALREADY_MOUNTED: &str = "_g_2dio_2derror_2dquark.Code17";
const INFO_WAIT: Duration = Duration::from_secs(5);
const SMB_PORT: u16 = 445;
const SSH_PORT: u16 = 22;
const HTTP_PORT: u16 = 80;
const HTTPS_PORT: u16 = 443;
const FTP_PORT: u16 = 21;

#[derive(Debug, PartialEq, Eq)]
pub struct Spec {
    pub prefix: Vec<u8>,
    pub items: Vec<(&'static str, Vec<u8>)>,
}

struct Parts {
    scheme: String,
    user: Option<Vec<u8>>,
    host: Vec<u8>,
    port: Option<u16>,
    path: Vec<u8>,
}

fn decode(s: &str) -> Vec<u8> {
    flit_platform_linux::percent_decode(s.as_bytes())
}

fn canonical(path: &[u8]) -> Vec<u8> {
    let mut parts: Vec<&[u8]> = Vec::new();
    for part in path.split(|&b| b == b'/') {
        match part {
            b"" | b"." => {}
            b".." => {
                parts.pop();
            }
            p => parts.push(p),
        }
    }
    let mut out = Vec::new();
    for p in &parts {
        out.push(b'/');
        out.extend_from_slice(p);
    }
    if out.is_empty() {
        out.push(b'/');
    }
    out
}

fn parts(uri: &str) -> Option<Parts> {
    let (scheme, rest) = uri.trim().split_once("://")?;
    if rest.contains(['?', '#']) || rest.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return None;
    }
    let (authority, path) = rest.find('/').map_or((rest, "/"), |i| (&rest[..i], &rest[i..]));
    let (user, hostport) = match authority.rsplit_once('@') {
        Some((u, h)) => (Some(decode(u)).filter(|u| !u.is_empty()), h),
        None => (None, authority),
    };
    let (host, port) = match hostport.strip_prefix('[') {
        Some(inner) => {
            let (h, after) = inner.split_once(']')?;
            (format!("[{h}]"), if after.is_empty() { None } else { Some(after.strip_prefix(':')?) })
        }
        None => match hostport.rsplit_once(':') {
            Some((h, p)) => (h.to_owned(), Some(p)),
            None => (hostport.to_owned(), None),
        },
    };
    let port = match port {
        Some(p) => Some(p.parse::<u16>().ok()?),
        None => None,
    };
    let host = decode(&host);
    if host.is_empty() || host == b"[]" {
        return None;
    }
    Some(Parts { scheme: scheme.to_ascii_lowercase(), user, host, port, path: decode(path) })
}

fn lower(b: &[u8]) -> Vec<u8> {
    match std::str::from_utf8(b) {
        Ok(s) => s.to_lowercase().into_bytes(),
        Err(_) => b.to_ascii_lowercase(),
    }
}

fn unbracket(host: &[u8]) -> Vec<u8> {
    match host.strip_prefix(b"[").and_then(|h| h.strip_suffix(b"]")) {
        Some(inner) => inner.to_vec(),
        None => host.to_vec(),
    }
}

pub fn spec(uri: &str) -> Option<Spec> {
    let p = parts(uri)?;
    let mut items: Vec<(&'static str, Vec<u8>)> = Vec::new();
    let port = |items: &mut Vec<(&'static str, Vec<u8>)>, default: u16| {
        if let Some(n) = p.port.filter(|&n| n != default) {
            items.push(("port", n.to_string().into_bytes()));
        }
    };
    let prefix = match p.scheme.as_str() {
        "smb" => {
            let path = canonical(&p.path);
            let mut segments = path.split(|&b| b == b'/').filter(|s| !s.is_empty());
            items.push(("server", lower(&p.host)));
            let prefix = match segments.next() {
                None => {
                    items.push(("type", b"smb-server".to_vec()));
                    b"/".to_vec()
                }
                Some(share) if share.starts_with(b"._") => return None,
                Some(share) => {
                    items.push(("type", b"smb-share".to_vec()));
                    items.push(("share", lower(share)));
                    canonical(&segments.collect::<Vec<_>>().join(&b'/'))
                }
            };
            port(&mut items, SMB_PORT);
            if let Some(user) = &p.user {
                let (domain, name) = match user.iter().position(|&b| b == b';') {
                    Some(k) => (&user[..k], &user[k + 1..]),
                    None => (&user[..0], &user[..]),
                };
                if !domain.is_empty() {
                    items.push(("domain", domain.to_vec()));
                }
                if !name.is_empty() {
                    items.push(("user", name.to_vec()));
                }
            }
            prefix
        }
        "sftp" | "ssh" => {
            items.push(("type", b"sftp".to_vec()));
            items.push(("host", unbracket(&lower(&p.host))));
            if let Some(user) = &p.user {
                items.push(("user", user.clone()));
            }
            port(&mut items, SSH_PORT);
            canonical(&p.path)
        }
        "dav" | "davs" => {
            let ssl = p.scheme == "davs";
            items.push(("type", b"dav".to_vec()));
            items.push(("ssl", if ssl { b"true".to_vec() } else { b"false".to_vec() }));
            items.push(("host", p.host.clone()));
            if let Some(user) = &p.user {
                items.push(("user", user.clone()));
            }
            port(&mut items, if ssl { HTTPS_PORT } else { HTTP_PORT });
            canonical(&p.path)
        }
        "dav+sd" | "davs+sd" | "ftp" | "nfs" => {
            items.push(("type", p.scheme.as_bytes().to_vec()));
            let ftp = p.scheme == "ftp";
            items.push(("host", if ftp { unbracket(&p.host.to_ascii_lowercase()) } else { p.host.clone() }));
            if let Some(user) = &p.user {
                items.push(("user", user.clone()));
            }
            port(&mut items, if ftp { FTP_PORT } else { 0 });
            canonical(&p.path)
        }
        _ => return None,
    };
    items.sort();
    Some(Spec { prefix, items })
}

pub fn server(uri: &str) -> String {
    parts(uri).map_or_else(|| uri.to_owned(), |p| String::from_utf8_lossy(&p.host).into_owned())
}

#[derive(Clone, Copy, Debug)]
pub struct Budget {
    left: Duration,
    since: Option<Instant>,
}

impl Budget {
    pub fn start(total: Duration, now: Instant) -> Self {
        Budget { left: total, since: Some(now) }
    }

    pub fn remaining(&self, now: Instant) -> Duration {
        match self.since {
            Some(s) => self.left.saturating_sub(now.saturating_duration_since(s)),
            None => self.left,
        }
    }

    pub fn pause(&mut self, now: Instant) {
        if self.since.is_some() {
            self.left = self.remaining(now);
            self.since = None;
        }
    }

    pub fn resume(&mut self, now: Instant) {
        if self.since.is_none() {
            self.since = Some(now);
        }
    }

    pub fn expired(&self, now: Instant) -> bool {
        self.since.is_some() && self.remaining(now).is_zero()
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Mounted(Duration),
    Cancelled,
    Unavailable,
}

pub(crate) unsafe fn append_text(api: &Api, it: &mut Iter, t: u8, s: &[u8]) -> bool {
    let mut buf: Vec<u8> = s.iter().copied().filter(|&b| b != 0).collect();
    buf.push(0);
    let p = buf.as_ptr() as *const c_char;
    let ok = (api.iter_append_basic)(it, t as c_int, (&p as *const *const c_char).cast()) != 0;
    for b in buf.iter_mut() {
        std::ptr::write_volatile(b, 0);
    }
    std::sync::atomic::compiler_fence(Ordering::SeqCst);
    ok
}

pub(crate) unsafe fn append_flag(api: &Api, it: &mut Iter, t: u8, v: u32) -> bool {
    (api.iter_append_basic)(it, t as c_int, (&v as *const u32).cast()) != 0
}

pub(crate) fn spec_values(spec: &Spec) -> V {
    let z = |b: &[u8]| {
        let mut v: Vec<u8> = b.iter().copied().filter(|&c| c != 0).collect();
        v.push(0);
        v
    };
    V::Tuple(vec![V::Bytes(z(&spec.prefix)), V::Props(spec.items.iter().map(|(k, v)| ((*k).to_owned(), V::Bytes(z(v)))).collect())])
}

pub(crate) fn mount_location_message(api: &Api, spec: &Spec, me: &str, path: &str) -> *mut c_void {
    let cs = |s: &str| CString::new(s).unwrap_or_default();
    unsafe {
        let msg = (api.new_method_call)(cs(DAEMON).as_ptr(), cs(TRACKER_PATH).as_ptr(), cs(TRACKER).as_ptr(), c"MountLocation".as_ptr());
        if !msg.is_null() {
            let mut it = Iter::new();
            (api.iter_init_append)(msg, &mut it);
            put(api, &mut it, "", &spec_values(spec));
            put(api, &mut it, "", &V::Tuple(vec![V::S(me.to_owned()), V::O(path.to_owned())]));
        }
        msg
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Asked {
    Login,
    Question,
}

pub(crate) fn reply_message(api: &Api, call: *mut c_void, asked: Asked, reply: Option<&MountReply>) -> *mut c_void {
    unsafe {
        let r = (api.new_return)(call);
        if r.is_null() {
            return r;
        }
        let mut it = Iter::new();
        (api.iter_init_append)(r, &mut it);
        let aborted = matches!(reply, None | Some(MountReply::Cancel));
        let mut ok = append_flag(api, &mut it, b'b', 1) && append_flag(api, &mut it, b'b', u32::from(aborted));
        ok &= match (asked, reply) {
            (Asked::Login, Some(MountReply::Login { user, domain, password, anonymous, remember })) => {
                append_text(api, &mut it, b's', password.as_str().as_bytes())
                    && append_text(api, &mut it, b's', user.as_bytes())
                    && append_text(api, &mut it, b's', domain.as_bytes())
                    && append_flag(api, &mut it, b'b', u32::from(*anonymous))
                    && append_flag(api, &mut it, b'u', u32::from(*remember))
            }
            (Asked::Login, _) => append_text(api, &mut it, b's', b"") && append_text(api, &mut it, b's', b"") && append_text(api, &mut it, b's', b"") && append_flag(api, &mut it, b'b', 0) && append_flag(api, &mut it, b'u', 0),
            (Asked::Question, Some(MountReply::Choice(k))) => append_flag(api, &mut it, b'u', u32::try_from(*k).unwrap_or(u32::MAX)),
            (Asked::Question, _) => append_flag(api, &mut it, b'u', 0),
        };
        if !ok {
            (api.unref)(r);
            return std::ptr::null_mut();
        }
        r
    }
}

fn word(a: Option<&Arg>) -> Option<String> {
    match a {
        Some(Arg::S(s)) => Some(s.clone()),
        _ => None,
    }
}

fn words(a: Option<&Arg>) -> Option<Vec<String>> {
    match a {
        Some(Arg::L(items)) => items.iter().map(|i| word(Some(i))).collect(),
        _ => None,
    }
}

pub(crate) fn ask_of(member: &str, server: &str, a: &[Arg]) -> Option<(Asked, MountAsk)> {
    match member {
        "AskPassword" => {
            let flags = match a.get(3) {
                Some(Arg::I(f)) => u32::try_from(*f).ok()?,
                _ => return None,
            };
            let ask = LoginAsk { server: server.to_owned(), message: word(a.first())?, user: word(a.get(1))?, domain: word(a.get(2))?, flags };
            Some((Asked::Login, MountAsk::Login(ask)))
        }
        "AskQuestion" | "ShowProcesses" => Some((Asked::Question, MountAsk::Question { server: server.to_owned(), message: word(a.first())?, choices: words(a.get(1))? })),
        _ => None,
    }
}

struct Conn<'a> {
    api: &'a Api,
    raw: *mut c_void,
}

impl<'a> Conn<'a> {
    fn open(api: &'a Api, address: &str) -> Option<Self> {
        let address = CString::new(address).ok()?;
        let mut err = dbus::Error::new();
        unsafe {
            (api.error_init)(&mut err);
            let raw = (api.open_private)(address.as_ptr(), &mut err);
            if raw.is_null() {
                (api.error_free)(&mut err);
                return None;
            }
            let conn = Conn { api, raw };
            let registered = (api.register)(raw, &mut err) != 0;
            (api.error_free)(&mut err);
            registered.then_some(conn)
        }
    }

    fn send(&self, msg: *mut c_void) -> Option<u32> {
        if msg.is_null() {
            return None;
        }
        let mut serial = 0u32;
        let sent = unsafe { (self.api.send)(self.raw, msg, &mut serial) != 0 };
        unsafe { (self.api.unref)(msg) };
        sent.then_some(serial)
    }

    fn error(&self, call: *mut c_void, name: &str, text: &str) {
        let cs = |s: &str| CString::new(s).unwrap_or_default();
        let r = unsafe { (self.api.new_error)(call, cs(name).as_ptr(), cs(text).as_ptr()) };
        self.send(r);
    }

    fn empty(&self, call: *mut c_void) {
        let r = unsafe { (self.api.new_return)(call) };
        self.send(r);
    }
}

impl Drop for Conn<'_> {
    fn drop(&mut self) {
        unsafe {
            (self.api.flush)(self.raw);
            (self.api.close)(self.raw);
            (self.api.release)(self.raw);
        }
    }
}

struct Held {
    call: *mut c_void,
    asked: Asked,
    id: u64,
    rx: Receiver<MountReply>,
}

struct Session<'p> {
    path: String,
    server: String,
    prompt: &'p MountPrompt,
    held: Option<Held>,
    asked: bool,
    cancelled: bool,
    abandoned: bool,
}

impl Session<'_> {
    fn answer(&mut self, conn: &Conn, reply: Option<MountReply>) {
        let Some(h) = self.held.take() else { return };
        let r = reply_message(conn.api, h.call, h.asked, reply.as_ref());
        conn.send(r);
        unsafe { (conn.api.unref)(h.call) };
    }

    fn withdraw(&mut self, conn: &Conn) {
        if let Some(id) = self.held.as_ref().map(|h| h.id) {
            self.answer(conn, None);
            self.prompt.withdraw(id);
        }
    }

    fn call(&mut self, conn: &Conn, msg: *mut c_void, budget: &mut Budget) {
        let api = conn.api;
        let (path, iface, member) = unsafe { (dbus::text((api.path)(msg)), dbus::text((api.interface)(msg)), dbus::text((api.member)(msg))) };
        if path != self.path || !(iface.is_empty() || iface == OPERATION) {
            conn.error(msg, UNKNOWN_METHOD, &member);
            unsafe { (api.unref)(msg) };
            return;
        }
        match member.as_str() {
            "ShowUnmountProgress" => conn.empty(msg),
            "Aborted" => {
                conn.empty(msg);
                self.withdraw(conn);
                budget.resume(Instant::now());
            }
            "AskPassword" | "AskQuestion" | "ShowProcesses" => {
                let a = unsafe { args(api, msg) };
                match ask_of(&member, &self.server, &a) {
                    None => conn.error(msg, INVALID_ARGS, &member),
                    Some((asked, _)) if self.held.is_some() => {
                        conn.send(reply_message(api, msg, asked, None));
                    }
                    Some((asked, ask)) => {
                        let (id, rx) = self.prompt.ask(ask);
                        self.held = Some(Held { call: msg, asked, id, rx });
                        self.asked = true;
                        budget.pause(Instant::now());
                        return;
                    }
                }
            }
            _ => conn.error(msg, UNKNOWN_METHOD, &member),
        }
        unsafe { (api.unref)(msg) };
    }

    fn poll(&mut self, conn: &Conn, budget: &mut Budget) {
        let Some(h) = &self.held else { return };
        match h.rx.try_recv() {
            Ok(reply) => {
                self.cancelled |= matches!(reply, MountReply::Cancel);
                self.answer(conn, Some(reply));
                budget.resume(Instant::now());
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.abandoned = true;
                self.answer(conn, None);
                budget.resume(Instant::now());
            }
        }
    }

    fn finished(&self, api: &Api, msg: *mut c_void, budget: &Budget) -> std::io::Result<Outcome> {
        let now = Instant::now();
        if unsafe { (api.msg_type)(msg) } == TYPE_RETURN {
            return Ok(Outcome::Mounted(budget.remaining(now)));
        }
        let name = unsafe { dbus::text((api.error_name)(msg)) };
        let text = word(unsafe { args(api, msg) }.first()).unwrap_or_default();
        if NO_GVFS.contains(&name.as_str()) && !self.asked {
            return Ok(Outcome::Unavailable);
        }
        if self.cancelled {
            return Ok(Outcome::Cancelled);
        }
        if name.ends_with(ALREADY_MOUNTED) {
            return Ok(Outcome::Mounted(budget.remaining(now)));
        }
        if self.abandoned {
            return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, S::ErrorsFsNeedsLogin.en()));
        }
        Err(std::io::Error::other(if text.is_empty() { name } else { text }))
    }
}

static NEXT: AtomicU64 = AtomicU64::new(1);

pub fn mount(address: &str, server: &str, spec: &Spec, prompt: &MountPrompt, wait: Duration) -> std::io::Result<Outcome> {
    let Some(api) = dbus::load() else { return Ok(Outcome::Unavailable) };
    let Some(conn) = Conn::open(api, address) else { return Ok(Outcome::Unavailable) };
    let me = unsafe { dbus::text((api.unique_name)(conn.raw)) };
    let path = format!("/org/flit/MountOperation/op{}_{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed));
    let Some(serial) = conn.send(mount_location_message(api, spec, &me, &path)) else { return Ok(Outcome::Unavailable) };
    let mut budget = Budget::start(wait, Instant::now());
    let mut session = Session { path, server: server.to_owned(), prompt, held: None, asked: false, cancelled: false, abandoned: false };
    let result = 'run: loop {
        if session.held.is_none() && budget.expired(Instant::now()) {
            break Err(std::io::Error::new(std::io::ErrorKind::TimedOut, S::ErrorsFsNotResponding.en()));
        }
        if unsafe { (api.read_write)(conn.raw, POLL_MS) } == 0 {
            break Err(std::io::Error::new(std::io::ErrorKind::BrokenPipe, S::ErrorsFsNotResponding.en()));
        }
        loop {
            let msg = unsafe { (api.pop)(conn.raw) };
            if msg.is_null() {
                break;
            }
            let kind = unsafe { (api.msg_type)(msg) };
            if (kind == TYPE_RETURN || kind == TYPE_ERROR) && unsafe { (api.reply_serial)(msg) } == serial {
                let done = session.finished(api, msg, &budget);
                unsafe { (api.unref)(msg) };
                break 'run done;
            }
            if kind == TYPE_METHOD_CALL {
                session.call(&conn, msg, &mut budget);
            } else {
                unsafe { (api.unref)(msg) };
            }
        }
        session.poll(&conn, &mut budget);
    };
    session.withdraw(&conn);
    result
}

fn mount_uri(uri: &str, prompt: &MountPrompt) -> std::io::Result<Outcome> {
    match (spec(uri), dbus::session_address()) {
        (Some(spec), Some(address)) => mount(&address, &server(uri), &spec, prompt, crate::places::CONNECT_WAIT),
        _ => Ok(Outcome::Unavailable),
    }
}

pub fn connect(uri: &str, prompt: &MountPrompt) -> std::io::Result<Option<PathBuf>> {
    match mount_uri(uri, prompt)? {
        Outcome::Unavailable => crate::places::connect(uri).map(Some),
        Outcome::Cancelled => Ok(None),
        Outcome::Mounted(left) => crate::places::local_path_within(uri, left.max(INFO_WAIT)).map(Some),
    }
}

pub fn browse(uri: &str, prompt: &MountPrompt) -> std::io::Result<bool> {
    match mount_uri(uri, prompt)? {
        Outcome::Unavailable => crate::places::mount_only(uri).map(|()| true),
        Outcome::Cancelled => Ok(false),
        Outcome::Mounted(_) => Ok(true),
    }
}

#[cfg(test)]
#[path = "../test/gvfs.rs"]
mod tests;
