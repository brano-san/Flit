use std::ffi::c_void;
use std::time::{Duration, Instant};
use x11rb::connection::Connection;
type R<T> = Result<T, Box<dyn std::error::Error>>;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ClientMessageEvent, ConnectionExt, CreateWindowAux, EventMask, GrabMode, GrabStatus, PropMode, SelectionNotifyEvent, SelectionRequestEvent, Timestamp, Window, WindowClass,
    SELECTION_NOTIFY_EVENT,
};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::{CURRENT_TIME, NONE};

const VERSION: u32 = 5;
const GRAB_TRIES: u32 = 50;
const FINISH_WAIT: Duration = Duration::from_secs(5);

x11rb::atom_manager! {
    Atoms: AtomsCookie {
        XdndAware,
        XdndSelection,
        XdndEnter,
        XdndPosition,
        XdndStatus,
        XdndLeave,
        XdndDrop,
        XdndFinished,
        XdndActionCopy,
        XdndTypeList,
        TARGETS,
        UriList: b"text/uri-list",
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Dropped(Window),
    Refused,
    Nowhere,
}

pub fn release_pointer_of(display: *mut c_void) {
    type Ungrab = unsafe extern "C" fn(*mut c_void, u64) -> i32;
    type Flush = unsafe extern "C" fn(*mut c_void) -> i32;
    let lib = unsafe { libc::dlopen(c"libX11.so.6".as_ptr(), libc::RTLD_LAZY | libc::RTLD_NOLOAD) };
    if lib.is_null() || display.is_null() {
        return;
    }
    let (ungrab, flush) = unsafe { (libc::dlsym(lib, c"XUngrabPointer".as_ptr()), libc::dlsym(lib, c"XFlush".as_ptr())) };
    if !ungrab.is_null() && !flush.is_null() {
        unsafe {
            std::mem::transmute::<*mut c_void, Ungrab>(ungrab)(display, 0);
            std::mem::transmute::<*mut c_void, Flush>(flush)(display);
        }
    }
    unsafe { libc::dlclose(lib) };
}

struct Source {
    conn: RustConnection,
    root: Window,
    window: Window,
    atoms: Atoms,
    data: Vec<u8>,
}

struct Target {
    window: Window,
    version: u32,
    waiting: bool,
    pending: Option<(i16, i16, Timestamp)>,
    accepted: bool,
}

impl Source {
    fn open(data: Vec<u8>) -> R<Self> {
        let (conn, screen) = RustConnection::connect(None)?;
        let root = conn.setup().roots[screen].root;
        let atoms = Atoms::new(&conn)?.reply()?;
        let window = conn.generate_id()?;
        conn.create_window(0, window, root, -10, -10, 1, 1, 0, WindowClass::INPUT_ONLY, 0, &CreateWindowAux::new().override_redirect(1))?;
        conn.set_selection_owner(window, atoms.XdndSelection, CURRENT_TIME)?;
        Ok(Self { conn, root, window, atoms, data })
    }

    fn grab(&self) -> R<bool> {
        let mask = EventMask::BUTTON_RELEASE | EventMask::POINTER_MOTION | EventMask::BUTTON_MOTION;
        for _ in 0..GRAB_TRIES {
            if self.conn.grab_pointer(false, self.root, mask, GrabMode::ASYNC, GrabMode::ASYNC, NONE, NONE, CURRENT_TIME)?.reply()?.status == GrabStatus::SUCCESS {
                return Ok(true);
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        Ok(false)
    }

    fn aware(&self, w: Window) -> Option<u32> {
        let reply = self.conn.get_property(false, w, self.atoms.XdndAware, AtomEnum::ATOM, 0, 1).ok()?.reply().ok()?;
        reply.value32()?.next()
    }

    fn target_at(&self) -> Option<(Window, u32)> {
        let mut w = self.root;
        loop {
            if w != self.root {
                if let Some(v) = self.aware(w) {
                    return Some((w, v.min(VERSION)));
                }
            }
            let child = self.conn.query_pointer(w).ok()?.reply().ok()?.child;
            if child == NONE {
                return None;
            }
            w = child;
        }
    }

    fn send(&self, to: Window, kind: Atom, data: [u32; 5]) -> R<()> {
        self.conn.send_event(false, to, EventMask::NO_EVENT, ClientMessageEvent::new(32, to, kind, data))?;
        self.conn.flush()?;
        Ok(())
    }

    fn enter(&self, t: &Target) -> R<()> {
        self.send(t.window, self.atoms.XdndEnter, [self.window, t.version << 24, self.atoms.UriList, 0, 0])
    }

    fn leave(&self, t: &Target) -> R<()> {
        self.send(t.window, self.atoms.XdndLeave, [self.window, 0, 0, 0, 0])
    }

    fn position(&self, t: &mut Target, (x, y, time): (i16, i16, Timestamp)) -> R<()> {
        if t.waiting {
            t.pending = Some((x, y, time));
            return Ok(());
        }
        t.waiting = true;
        t.pending = None;
        let at = ((x as u16 as u32) << 16) | y as u16 as u32;
        self.send(t.window, self.atoms.XdndPosition, [self.window, 0, at, time, self.atoms.XdndActionCopy])
    }

    fn answer(&self, req: &SelectionRequestEvent) -> R<()> {
        let property = if req.property == NONE { req.target } else { req.property };
        let stored = if req.selection != self.atoms.XdndSelection {
            false
        } else if req.target == self.atoms.UriList {
            self.conn.change_property(PropMode::REPLACE, req.requestor, property, self.atoms.UriList, 8, self.data.len() as u32, &self.data)?;
            true
        } else if req.target == self.atoms.TARGETS {
            let targets = [self.atoms.TARGETS, self.atoms.UriList];
            let bytes: Vec<u8> = targets.iter().flat_map(|a| a.to_ne_bytes()).collect();
            self.conn.change_property(PropMode::REPLACE, req.requestor, property, AtomEnum::ATOM, 32, targets.len() as u32, &bytes)?;
            true
        } else {
            false
        };
        let notify = SelectionNotifyEvent {
            response_type: SELECTION_NOTIFY_EVENT,
            sequence: 0,
            time: req.time,
            requestor: req.requestor,
            selection: req.selection,
            target: req.target,
            property: if stored { property } else { NONE },
        };
        self.conn.send_event(false, req.requestor, EventMask::NO_EVENT, notify)?;
        self.conn.flush()?;
        Ok(())
    }

    fn run(&self) -> R<Outcome> {
        let mut target: Option<Target> = None;
        loop {
            let event = self.conn.wait_for_event()?;
            match event {
                Event::MotionNotify(m) => {
                    let found = self.target_at();
                    if target.as_ref().map(|t| t.window) != found.map(|f| f.0) {
                        if let Some(old) = target.take() {
                            self.leave(&old)?;
                        }
                        if let Some((window, version)) = found {
                            let t = Target { window, version, waiting: false, pending: None, accepted: false };
                            self.enter(&t)?;
                            target = Some(t);
                        }
                    }
                    if let Some(t) = target.as_mut() {
                        self.position(t, (m.root_x, m.root_y, m.time))?;
                    }
                }
                Event::ClientMessage(c) if c.type_ == self.atoms.XdndStatus => {
                    let d = c.data.as_data32();
                    if let Some(t) = target.as_mut().filter(|t| t.window == d[0]) {
                        t.waiting = false;
                        t.accepted = d[1] & 1 != 0;
                        if let Some(p) = t.pending.take() {
                            self.position(t, p)?;
                        }
                    }
                }
                Event::SelectionRequest(req) => self.answer(&req)?,
                Event::ButtonRelease(b) => {
                    let Some(t) = target else { return Ok(Outcome::Nowhere) };
                    if !t.accepted {
                        self.leave(&t)?;
                        return Ok(Outcome::Refused);
                    }
                    self.conn.ungrab_pointer(CURRENT_TIME)?;
                    self.send(t.window, self.atoms.XdndDrop, [self.window, 0, b.time, 0, 0])?;
                    return self.finish(t.window);
                }
                _ => {}
            }
        }
    }

    fn finish(&self, target: Window) -> R<Outcome> {
        let until = Instant::now() + FINISH_WAIT;
        while Instant::now() < until {
            match self.conn.poll_for_event()? {
                Some(Event::SelectionRequest(req)) => self.answer(&req)?,
                Some(Event::ClientMessage(c)) if c.type_ == self.atoms.XdndFinished => return Ok(Outcome::Dropped(target)),
                Some(_) => {}
                None => std::thread::sleep(Duration::from_millis(5)),
            }
        }
        Ok(Outcome::Dropped(target))
    }
}

impl Drop for Source {
    fn drop(&mut self) {
        let _ = self.conn.ungrab_pointer(CURRENT_TIME);
        let _ = self.conn.destroy_window(self.window);
        let _ = self.conn.flush();
    }
}

pub fn drag(uri_list: String) -> Result<Outcome, String> {
    let source = Source::open(uri_list.into_bytes()).map_err(|e| e.to_string())?;
    if !source.grab().map_err(|e| e.to_string())? {
        return Err("the pointer is grabbed by another client".into());
    }
    source.run().map_err(|e| e.to_string())
}

#[cfg(test)]
#[path = "../test/xdnd.rs"]
mod tests;
