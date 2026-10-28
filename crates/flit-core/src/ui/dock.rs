use crate::render::Rect;

const BUTTON_W: f32 = 44.0;
const BUTTON_H: f32 = 32.0;
const GAP: f32 = 8.0;
const EDGE: f32 = 56.0;
const COLUMN: f32 = 1.0 / 3.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Dock {
    Into(u32),
    Split { panel: u32, down: bool, before: bool },
    Window { before: bool },
    Between { left: u32 },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Scheme {
    Whole,
    Half { down: bool, before: bool },
    Third(usize),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Zone {
    pub button: Rect,
    pub dock: Dock,
    pub area: Rect,
    pub scheme: Scheme,
}

fn overlaps(a: Rect, b: Rect) -> bool {
    a.x < b.right() && b.x < a.right() && a.y < b.bottom() && b.y < a.bottom()
}

fn half(r: Rect, down: bool, before: bool) -> Rect {
    match (down, before) {
        (false, true) => Rect::new(r.x, r.y, (r.w / 2.0).round(), r.h),
        (false, false) => Rect::new(r.x + (r.w / 2.0).round(), r.y, r.w - (r.w / 2.0).round(), r.h),
        (true, true) => Rect::new(r.x, r.y, r.w, (r.h / 2.0).round()),
        (true, false) => Rect::new(r.x, r.y + (r.h / 2.0).round(), r.w, r.h - (r.h / 2.0).round()),
    }
}

fn split(panel: u32, r: Rect, down: bool, before: bool) -> (Dock, Rect, Scheme) {
    (Dock::Split { panel, down, before }, half(r, down, before), Scheme::Half { down, before })
}

pub fn zones(panels: &[(u32, Rect)], window: Rect, (x, y): (f32, f32), sc: f32) -> Vec<Zone> {
    let Some(&(id, r)) = panels.iter().find(|(_, r)| r.contains(x, y)) else { return Vec::new() };
    let (bw, bh, gap, edge) = (BUTTON_W * sc, BUTTON_H * sc, GAP * sc, EDGE * sc);
    let wide = r.w >= r.h;
    let (cx, cy) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
    let center = [split(id, r, !wide, true), (Dock::Into(id), r, Scheme::Whole), split(id, r, !wide, false)];
    let mut out: Vec<Zone> = center
        .into_iter()
        .enumerate()
        .map(|(k, (dock, area, scheme))| {
            let off = (k as f32 - 1.0) * if wide { bw + gap } else { bh + gap };
            let button = if wide { Rect::new(cx - bw / 2.0 + off, cy - bh / 2.0, bw, bh) } else { Rect::new(cx - bw / 2.0, cy - bh / 2.0 + off, bw, bh) };
            Zone { button: button.snap(), dock, area, scheme }
        })
        .collect();
    let mut near = |dock: Dock, area: Rect, scheme: Scheme, bx: f32, by: f32| {
        let button = Rect::new((bx - bw / 2.0).clamp(window.x, (window.right() - bw).max(window.x)), (by - bh / 2.0).clamp(window.y, (window.bottom() - bh).max(window.y)), bw, bh).snap();
        if out.iter().all(|z| !overlaps(z.button, button)) {
            out.push(Zone { button, dock, area, scheme });
        }
    };
    let column = (window.w * COLUMN).round();
    if x - window.x < edge {
        near(Dock::Window { before: true }, Rect::new(window.x, window.y, column, window.h), Scheme::Third(0), window.x + gap + bw / 2.0, y);
    } else if window.right() - x < edge {
        near(Dock::Window { before: false }, Rect::new(window.right() - column, window.y, column, window.h), Scheme::Third(2), window.right() - gap - bw / 2.0, y);
    }
    let junction = panels.iter().find(|(other, o)| *other != id && (o.x - r.right()).abs() <= 2.0 * sc && o.y < r.bottom() && r.y < o.bottom());
    let junction = junction.filter(|(_, o)| o.x - x < edge).map(|&(_, o)| (id, r, o)).or_else(|| {
        panels.iter().find(|(other, o)| *other != id && (r.x - o.right()).abs() <= 2.0 * sc && o.y < r.bottom() && r.y < o.bottom() && x - r.x < edge).map(|&(left, o)| (left, o, r))
    });
    if let Some((left, a, b)) = junction {
        let band = ((a.w + b.w) * COLUMN / 2.0).round();
        let top = a.y.max(b.y);
        let area = Rect::new(b.x - band, top, 2.0 * band, a.bottom().min(b.bottom()) - top);
        near(Dock::Between { left }, area, Scheme::Third(1), b.x, y);
    }
    if y - r.y < edge {
        let (d, a, s) = split(id, r, true, true);
        near(d, a, s, x, r.y + gap + bh / 2.0);
    } else if r.bottom() - y < edge {
        let (d, a, s) = split(id, r, true, false);
        near(d, a, s, x, r.bottom() - gap - bh / 2.0);
    }
    out
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Docking {
    pub zones: Vec<Zone>,
    pub hot: Option<usize>,
    pub at: (f32, f32),
}

pub fn zone_at(zones: &[Zone], x: f32, y: f32) -> Option<usize> {
    zones.iter().position(|z| z.button.contains(x, y))
}

pub fn scheme_rects(button: Rect, scheme: Scheme, sc: f32) -> Vec<(Rect, bool)> {
    let r = button.inset(6.0 * sc, 6.0 * sc);
    let gap = (2.0 * sc).round().max(1.0);
    match scheme {
        Scheme::Whole => vec![(r, true)],
        Scheme::Half { down, before } => {
            let a = half(r, down, true);
            let b = half(r, down, false);
            let (a, b) = if down { (Rect::new(a.x, a.y, a.w, a.h - gap / 2.0), Rect::new(b.x, b.y + gap / 2.0, b.w, b.h - gap / 2.0)) } else { (Rect::new(a.x, a.y, a.w - gap / 2.0, a.h), Rect::new(b.x + gap / 2.0, b.y, b.w - gap / 2.0, b.h)) };
            vec![(a, before), (b, !before)]
        }
        Scheme::Third(k) => {
            let w = (r.w - 2.0 * gap) / 3.0;
            (0..3).map(|i| (Rect::new(r.x + i as f32 * (w + gap), r.y, w, r.h), i == k)).collect()
        }
    }
}

#[cfg(test)]
#[path = "../../test/ui/dock.rs"]
mod tests;
