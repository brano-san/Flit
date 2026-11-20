use std::sync::mpsc;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt};
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
    Sides { top: window.y > area.y, left: window.x > area.x, bottom: window.y + window.height < area.y + area.height, right: window.x + window.width < area.x + area.width }
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
    }
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

pub fn latest<K: PartialEq, V>(batch: impl IntoIterator<Item = (K, V)>) -> Vec<(K, V)> {
    let mut out: Vec<(K, V)> = Vec::new();
    for (k, v) in batch {
        match out.iter_mut().find(|(seen, _)| *seen == k) {
            Some(slot) => slot.1 = v,
            None => out.push((k, v)),
        }
    }
    out
}

pub fn x11_watcher<K: PartialEq + Copy + Send + 'static>(report: impl Fn(K, Sides) + Send + 'static) -> Option<mpsc::Sender<(K, u32)>> {
    let (send, receive) = mpsc::channel::<(K, u32)>();
    let spawned = std::thread::Builder::new().name("flit-x11-edges".into()).spawn(move || {
        let x11 = match X11::connect() {
            Ok(x11) => x11,
            Err(e) => return tracing::debug!("cannot watch window tiling on X11: {e}"),
        };
        while let Ok(first) = receive.recv() {
            for (key, window) in latest(std::iter::once(first).chain(receive.try_iter())) {
                match x11.sides(window) {
                    Ok(sides) => report(key, sides),
                    Err(e) => tracing::debug!("cannot read the X11 tiling state: {e}"),
                }
            }
        }
    });
    match spawned {
        Ok(_) => Some(send),
        Err(e) => {
            tracing::warn!("cannot start flit-x11-edges: {e}");
            None
        }
    }
}

#[cfg(test)]
#[path = "../test/tiling.rs"]
mod tests;
