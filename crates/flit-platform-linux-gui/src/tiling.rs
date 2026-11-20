use std::collections::{HashMap, HashSet};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::sync::mpsc;
use flit_platform::WindowId;
use x11rb::connection::Connection;
use x11rb::properties::WmSizeHints;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{AtomEnum, ChangeWindowAttributesAux, ConnectionExt, EventMask, GetPropertyReply, Gravity, PropertyNotifyEvent};
use x11rb::xcb_ffi::XCBConnection;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sides {
    pub top: bool,
    pub right: bool,
    pub bottom: bool,
    pub left: bool,
}

impl Sides {
    pub const ALL: Sides = Sides { top: true, right: true, bottom: true, left: true };
    pub const NONE: Sides = Sides { top: false, right: false, bottom: false, left: false };

    pub fn and(self, other: Sides) -> Sides {
        Sides { top: self.top && other.top, right: self.right && other.right, bottom: self.bottom && other.bottom, left: self.left && other.left }
    }
}

impl Default for Sides {
    fn default() -> Self {
        Self::ALL
    }
}

const TOP_TILED: u32 = 1 << 0;
const TOP_RESIZABLE: u32 = 1 << 1;
const RIGHT_TILED: u32 = 1 << 2;
const RIGHT_RESIZABLE: u32 = 1 << 3;
const BOTTOM_TILED: u32 = 1 << 4;
const BOTTOM_RESIZABLE: u32 = 1 << 5;
const LEFT_TILED: u32 = 1 << 6;
const LEFT_RESIZABLE: u32 = 1 << 7;

pub fn gtk_edge_constraints(bits: u32) -> Sides {
    let free = |tiled: u32, resizable: u32| bits & tiled == 0 || bits & resizable != 0;
    Sides { top: free(TOP_TILED, TOP_RESIZABLE), right: free(RIGHT_TILED, RIGHT_RESIZABLE), bottom: free(BOTTOM_TILED, BOTTOM_RESIZABLE), left: free(LEFT_TILED, LEFT_RESIZABLE) }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StateAtoms {
    pub vertical: u32,
    pub horizontal: u32,
    pub fullscreen: u32,
}

pub fn net_wm_state(states: &[u32], atoms: StateAtoms) -> Sides {
    if states.contains(&atoms.fullscreen) {
        return Sides::NONE;
    }
    let (vertical, horizontal) = (states.contains(&atoms.vertical), states.contains(&atoms.horizontal));
    Sides { top: !vertical, bottom: !vertical, left: !horizontal, right: !horizontal }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Area {
    pub x: i64,
    pub y: i64,
    pub width: i64,
    pub height: i64,
}

pub fn workarea(values: &[u32], desktop: Option<u32>) -> Option<Area> {
    let pick = |k: usize| values.get(k * 4..k * 4 + 4).map(|v| Area { x: i64::from(v[0]), y: i64::from(v[1]), width: i64::from(v[2]), height: i64::from(v[3]) });
    desktop.and_then(|d| pick(d as usize)).or_else(|| pick(0)).filter(|a| a.width > 0 && a.height > 0)
}

pub fn against_workarea(window: Area, area: Area) -> Sides {
    let vertical = window.y <= area.y && window.y + window.height >= area.y + area.height;
    let horizontal = window.x <= area.x && window.x + window.width >= area.x + area.width;
    Sides { top: !vertical, bottom: !vertical, left: !horizontal, right: !horizontal }
}

pub fn x11_sides(gtk: Option<u32>, state: Sides, geometry: Option<(Area, Area)>) -> Sides {
    let placed = match (gtk, geometry) {
        (Some(bits), _) => gtk_edge_constraints(bits),
        (None, Some((window, area))) => against_workarea(window, area),
        (None, None) => Sides::ALL,
    };
    placed.and(state)
}

x11rb::atom_manager! {
    Atoms: AtomsCookie {
        _NET_WM_STATE,
        _NET_WM_STATE_MAXIMIZED_VERT,
        _NET_WM_STATE_MAXIMIZED_HORZ,
        _NET_WM_STATE_FULLSCREEN,
        _GTK_EDGE_CONSTRAINTS,
        _NET_WORKAREA,
        _NET_CURRENT_DESKTOP,
        _NET_SUPPORTING_WM_CHECK,
        _NET_WM_NAME,
        UTF8_STRING,
    }
}

const FRAMELESS_WM_PREFIXES: &[&str] = &["i3", "dwm", "bspwm", "awesome", "xmonad", "herbstluftwm", "qtile", "LG3D", "sway"];

fn wm_draws_frames(name: &str, sway_session: bool) -> bool {
    let starts_with = |prefix: &str| name.get(..prefix.len()).is_some_and(|start| start.eq_ignore_ascii_case(prefix));
    !(FRAMELESS_WM_PREFIXES.iter().any(|prefix| starts_with(prefix)) || (sway_session && starts_with("wlroots wm")))
}

fn wm_check_window(reply: &GetPropertyReply) -> Option<u32> {
    if reply.type_ != u32::from(AtomEnum::WINDOW) || reply.value_len != 1 || reply.bytes_after != 0 {
        return None;
    }
    reply.value32()?.next().filter(|&window| window != 0)
}

fn supporting_wm(root: u32, property: impl Fn(u32) -> Option<GetPropertyReply>) -> Option<u32> {
    let candidate = wm_check_window(&property(root)?)?;
    (candidate != root && wm_check_window(&property(candidate)?) == Some(candidate)).then_some(candidate)
}

type R<T> = Result<T, Box<dyn std::error::Error>>;

struct X11 {
    conn: XCBConnection,
    root: u32,
    atoms: Atoms,
}

impl X11 {
    fn connect() -> R<Self> {
        let (conn, screen) = XCBConnection::connect(None)?;
        let root = conn.setup().roots[screen].root;
        let atoms = Atoms::new(&conn)?.reply()?;
        Ok(Self { conn, root, atoms })
    }

    fn cardinals(&self, window: u32, property: u32, kind: impl Into<u32>) -> R<Vec<u32>> {
        let reply = self.conn.get_property(false, window, property, kind, 0, 1024)?.reply()?;
        Ok(reply.value32().map(Iterator::collect).unwrap_or_default())
    }

    fn server_frames(&self) -> bool {
        let Some(wm) = supporting_wm(self.root, |window| {
            self.conn.get_property(false, window, self.atoms._NET_SUPPORTING_WM_CHECK, AtomEnum::WINDOW, 0, 1).ok()?.reply().ok()
        }) else {
            tracing::debug!("no compatible X11 window manager found");
            return false;
        };
        let name = self.conn.get_property(false, wm, self.atoms._NET_WM_NAME, AtomEnum::ANY, 0, 256).ok()
            .and_then(|cookie| cookie.reply().ok())
            .filter(|reply| reply.format == 8 && [self.atoms.UTF8_STRING, AtomEnum::STRING.into()].contains(&reply.type_))
            .map(|reply| String::from_utf8_lossy(&reply.value).into_owned())
            .unwrap_or_default();
        let available = wm_draws_frames(&name, std::env::var_os("SWAYSOCK").is_some());
        tracing::debug!(wm = %name, server_frames = available, "found an X11 window manager");
        available
    }

    fn watch_properties(&self, window: u32) -> R<()> {
        self.conn.change_window_attributes(window, &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE))?.check()?;
        Ok(())
    }

    fn prepare_placement(&self, window: u32) -> R<()> {
        let mut hints = WmSizeHints::get_normal_hints(&self.conn, window)?.reply()?.unwrap_or_default();
        hints.win_gravity = Some(Gravity::STATIC);
        hints.set_normal_hints(&self.conn, window)?.check()?;
        Ok(())
    }

    fn sides(&self, window: u32) -> R<Sides> {
        let a = &self.atoms;
        let states = self.cardinals(window, a._NET_WM_STATE, AtomEnum::ATOM)?;
        let state = net_wm_state(&states, StateAtoms { vertical: a._NET_WM_STATE_MAXIMIZED_VERT, horizontal: a._NET_WM_STATE_MAXIMIZED_HORZ, fullscreen: a._NET_WM_STATE_FULLSCREEN });
        let gtk = self.cardinals(window, a._GTK_EDGE_CONSTRAINTS, AtomEnum::CARDINAL)?.first().copied();
        let geometry = match gtk {
            Some(_) => None,
            None => {
                let size = self.conn.get_geometry(window)?.reply()?;
                let at = self.conn.translate_coordinates(window, self.root, 0, 0)?.reply()?;
                let desktop = self.cardinals(self.root, a._NET_CURRENT_DESKTOP, AtomEnum::CARDINAL)?.first().copied();
                let area = workarea(&self.cardinals(self.root, a._NET_WORKAREA, AtomEnum::CARDINAL)?, desktop);
                area.map(|area| (Area { x: i64::from(at.dst_x), y: i64::from(at.dst_y), width: i64::from(size.width), height: i64::from(size.height) }, area))
            }
        };
        Ok(x11_sides(gtk, state, geometry))
    }
}

#[derive(Default)]
struct WatchedWindows {
    ids: HashMap<u32, WindowId>,
    dirty: HashSet<u32>,
}

impl WatchedWindows {
    fn property_changed(&mut self, event: PropertyNotifyEvent, root: u32, window_properties: [u32; 2], root_properties: [u32; 2]) {
        if event.window == root && root_properties.contains(&event.atom) {
            self.dirty.extend(self.ids.keys());
        } else if window_properties.contains(&event.atom) && self.ids.contains_key(&event.window) {
            self.dirty.insert(event.window);
        }
    }

    fn refresh(&mut self, read: impl Fn(u32) -> Option<Sides>, mut report: impl FnMut(WindowId, Sides)) {
        for window in self.dirty.drain() {
            let Some(&id) = self.ids.get(&window) else { continue };
            if let Some(sides) = read(window) {
                report(id, sides);
            }
        }
    }
}

pub enum Command {
    PreparePlacement(WindowId, u32),
    Refresh(WindowId, u32),
    Forget(u32),
}

pub enum Update {
    ServerFrames(bool),
    PlacementReady(WindowId),
    Edges(WindowId, Sides),
    Stopped,
}

pub struct Watcher {
    commands: mpsc::Sender<Command>,
    signal: UnixStream,
    pub updates: mpsc::Receiver<Update>,
}

impl Watcher {
    pub fn start(wake: impl Fn() + Send + 'static) -> io::Result<Self> {
        let (commands, receive) = mpsc::channel();
        let (send, updates) = mpsc::channel();
        let (signal, ready) = UnixStream::pair()?;
        signal.set_nonblocking(true)?;
        std::thread::Builder::new().name("flit-x11-edges".into()).spawn(move || {
            let report = |update| {
                if send.send(update).is_ok() {
                    wake();
                }
            };
            let result = X11::connect().and_then(|x11| {
                report(Update::ServerFrames(x11.server_frames()));
                x11.watch(ready, &receive, report)
            });
            if let Err(e) = result {
                tracing::debug!("cannot watch window tiling on X11: {e}");
                report(Update::Stopped);
            }
        })?;
        Ok(Self { commands, signal, updates })
    }

    pub fn send(&self, command: Command) -> bool {
        if self.commands.send(command).is_err() {
            return false;
        }
        loop {
            match (&self.signal).write(&[1]) {
                Ok(_) => return true,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return e.kind() == io::ErrorKind::WouldBlock,
            }
        }
    }
}

impl X11 {
    fn watch(&self, mut ready: UnixStream, commands: &mpsc::Receiver<Command>, report: impl Fn(Update)) -> R<()> {
        self.watch_properties(self.root)?;
        let mut watched = WatchedWindows::default();
        let a = &self.atoms;
        let mut fds = [
            libc::pollfd { fd: ready.as_raw_fd(), events: libc::POLLIN, revents: 0 },
            libc::pollfd { fd: self.conn.as_raw_fd(), events: libc::POLLIN, revents: 0 },
        ];
        loop {
            for command in commands.try_iter() {
                match command {
                    Command::PreparePlacement(id, window) => {
                        if let Err(e) = self.prepare_placement(window) {
                            tracing::debug!("cannot prepare X11 window placement: {e}");
                        }
                        report(Update::PlacementReady(id));
                    }
                    Command::Refresh(id, window) => {
                        if let std::collections::hash_map::Entry::Vacant(entry) = watched.ids.entry(window) {
                            if let Err(e) = self.watch_properties(window) {
                                tracing::debug!("cannot watch X11 window properties: {e}");
                                continue;
                            }
                            entry.insert(id);
                        }
                        watched.dirty.insert(window);
                    }
                    Command::Forget(window) => {
                        watched.ids.remove(&window);
                        watched.dirty.remove(&window);
                    }
                }
            }
            while let Some(event) = self.conn.poll_for_event()? {
                if let Event::PropertyNotify(event) = event {
                    watched.property_changed(event, self.root, [a._NET_WM_STATE, a._GTK_EDGE_CONSTRAINTS], [a._NET_WORKAREA, a._NET_CURRENT_DESKTOP]);
                }
            }
            if !watched.dirty.is_empty() {
                watched.refresh(|window| match self.sides(window) {
                    Ok(sides) => Some(sides),
                    Err(e) => {
                        tracing::debug!("cannot read the X11 tiling state: {e}");
                        None
                    }
                }, |id, sides| report(Update::Edges(id, sides)));
                continue;
            }
            if unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, -1) } < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(error.into());
            }
            if fds[0].revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 || (fds[0].revents & libc::POLLIN != 0 && ready.read(&mut [0; 256])? == 0) {
                return Ok(());
            }
            if fds[1].revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
                return Err(io::Error::from(io::ErrorKind::ConnectionAborted).into());
            }
        }
    }
}

#[cfg(test)]
#[path = "../test/tiling.rs"]
mod tests;
