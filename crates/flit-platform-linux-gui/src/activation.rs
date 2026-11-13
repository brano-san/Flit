use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{AtomEnum, ClientMessageEvent, ConnectionExt, CreateWindowAux, EventMask, PropMode, Window, WindowClass};
use x11rb::protocol::Event;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::xcb_ffi::XCBConnection;

pub const VARS: [&str; 2] = ["XDG_ACTIVATION_TOKEN", "DESKTOP_STARTUP_ID"];
const WAIT: Duration = Duration::from_millis(300);

static WAYLAND: AtomicBool = AtomicBool::new(false);
static PENDING: Mutex<Pending> = Mutex::new(Pending { latest: None, asked: 0, answered: 0 });
static READY: Condvar = Condvar::new();
static MADE: AtomicU32 = AtomicU32::new(0);

struct Pending {
    latest: Option<String>,
    asked: u64,
    answered: u64,
}

pub fn set_wayland(on: bool) {
    WAYLAND.store(on, Ordering::Release);
}

pub fn pass(cmd: &mut Command, token: Option<&str>) {
    for var in VARS {
        match token {
            Some(t) => cmd.env(var, t),
            None => cmd.env_remove(var),
        };
    }
}

pub fn from_env(read: impl Fn(&str) -> Option<String>, wayland: bool) -> Option<String> {
    let order = if wayland { VARS } else { [VARS[1], VARS[0]] };
    order.iter().find_map(|v| read(v).filter(|t| !t.trim().is_empty()))
}

pub fn launcher_token() -> Option<String> {
    static TOKEN: OnceLock<Option<String>> = OnceLock::new();
    TOKEN
        .get_or_init(|| {
            let token = from_env(|v| std::env::var(v).ok(), std::env::var_os("WAYLAND_DISPLAY").is_some());
            for var in VARS {
                std::env::remove_var(var);
            }
            token
        })
        .clone()
}

pub fn asked() {
    PENDING.lock().unwrap_or_else(|e| e.into_inner()).asked += 1;
}

pub fn answered(token: String) {
    let mut p = PENDING.lock().unwrap_or_else(|e| e.into_inner());
    p.answered = (p.answered + 1).min(p.asked);
    p.latest = Some(token).filter(|t| !t.is_empty());
    READY.notify_all();
}

pub fn take_waiting(wait: Duration) -> Option<String> {
    let until = Instant::now() + wait;
    let mut p = PENDING.lock().unwrap_or_else(|e| e.into_inner());
    while p.answered < p.asked {
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            p.answered = p.asked;
            break;
        }
        p = READY.wait_timeout(p, left).unwrap_or_else(|e| e.into_inner()).0;
    }
    p.latest.take()
}

pub fn take_ready() -> Option<String> {
    PENDING.lock().unwrap_or_else(|e| e.into_inner()).latest.take()
}

pub fn take() -> Option<String> {
    match WAYLAND.load(Ordering::Acquire) {
        true => take_waiting(WAIT),
        false => x11_token().map_err(|e| tracing::debug!("no X11 startup id: {e}")).ok(),
    }
}

type R<T> = Result<T, Box<dyn std::error::Error>>;

x11rb::atom_manager! {
    Atoms: AtomsCookie {
        _NET_STARTUP_ID,
        _NET_ACTIVE_WINDOW,
        _NET_STARTUP_INFO_BEGIN,
        _NET_STARTUP_INFO,
        UTF8_STRING,
        FLIT_TIME,
    }
}

fn server_time(conn: &XCBConnection, window: Window, atoms: &Atoms) -> R<u32> {
    conn.change_property8(PropMode::APPEND, window, atoms.FLIT_TIME, AtomEnum::STRING, &[])?;
    conn.flush()?;
    loop {
        if let Event::PropertyNotify(e) = conn.wait_for_event()? {
            if e.window == window {
                return Ok(e.time);
            }
        }
    }
}

fn helper_window(conn: &XCBConnection, screen: usize) -> R<Window> {
    let root = conn.setup().roots[screen].root;
    let window = conn.generate_id()?;
    conn.create_window(0, window, root, -10, -10, 1, 1, 0, WindowClass::INPUT_ONLY, 0, &CreateWindowAux::new().override_redirect(1).event_mask(EventMask::PROPERTY_CHANGE))?;
    Ok(window)
}

pub fn startup_id(host: &str, pid: u32, serial: u32, time: u32) -> String {
    format!("{host}-flit-{pid}-{serial}_TIME{time}")
}

pub fn startup_time(token: &str) -> Option<u32> {
    token.rsplit_once("_TIME")?.1.parse().ok()
}

fn x11_token() -> R<String> {
    let (conn, screen) = XCBConnection::connect(None)?;
    let atoms = Atoms::new(&conn)?.reply()?;
    let window = helper_window(&conn, screen)?;
    let time = server_time(&conn, window, &atoms)?;
    let mut host = [0u8; 256];
    unsafe { libc::gethostname(host.as_mut_ptr().cast(), host.len()) };
    let host = String::from_utf8_lossy(&host[..host.iter().position(|&b| b == 0).unwrap_or(0)]).into_owned();
    Ok(startup_id(&host, std::process::id(), MADE.fetch_add(1, Ordering::Relaxed), time))
}

pub fn remove_message(token: &str) -> Vec<u8> {
    let mut out = b"remove: ID=\"".to_vec();
    for &b in token.as_bytes() {
        if b == b'"' || b == b'\\' {
            out.push(b'\\');
        }
        out.push(b);
    }
    out.extend_from_slice(b"\"\0");
    out
}

pub fn x11_activate(target: u32, token: &str) -> R<()> {
    let (conn, screen) = XCBConnection::connect(None)?;
    let root = conn.setup().roots[screen].root;
    let atoms = Atoms::new(&conn)?.reply()?;
    let window = helper_window(&conn, screen)?;
    conn.change_property8(PropMode::REPLACE, target, atoms._NET_STARTUP_ID, atoms.UTF8_STRING, token.as_bytes())?;
    let mut kind = atoms._NET_STARTUP_INFO_BEGIN;
    for chunk in remove_message(token).chunks(20) {
        let mut data = [0u8; 20];
        data[..chunk.len()].copy_from_slice(chunk);
        conn.send_event(false, root, EventMask::PROPERTY_CHANGE, ClientMessageEvent::new(8, window, kind, data))?;
        kind = atoms._NET_STARTUP_INFO;
    }
    let time = startup_time(token).unwrap_or(x11rb::CURRENT_TIME);
    let activate = ClientMessageEvent::new(32, target, atoms._NET_ACTIVE_WINDOW, [1, time, 0, 0, 0]);
    conn.send_event(false, root, EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY, activate)?;
    conn.destroy_window(window)?;
    conn.flush()?;
    Ok(())
}

#[cfg(test)]
#[path = "../test/activation.rs"]
mod tests;
