use super::edit::EditAction;
use super::field::{Field, FieldKey};
use super::fuzzy::fuzzy;
use super::menu::{Mark, MenuItem};
use super::popup::MenuOutcome;
use super::widgets::{baseline, Button, Paint};
use crate::i18n::S;
use crate::input::{Key, Mods, Preedit};
use crate::render::Rect;
use crate::text::{FontFamily, FontStyle, Gfx, TextBackend};
use crate::theme::{ink_on, Theme};
use std::cmp::Reverse;
use std::ops::Range;

const ROW_H: f32 = 28.0;
const SEPARATOR_H: f32 = 9.0;
const FIELD_H: f32 = 30.0;
const PAD: f32 = 4.0;
const ICON_W: f32 = 28.0;
const INDENT: f32 = 20.0;
const SIDE_W: f32 = 28.0;
const STAR_W: f32 = 24.0;
const KEYS_GAP: f32 = 32.0;
const MIN_W: f32 = 280.0;
const MAX_W: f32 = 560.0;
const SCROLL_W: f32 = 12.0;
const FONT_PX: f32 = 13.0;
const ICON_PX: f32 = 11.0;
const ARM_DISTANCE: f32 = 4.0;
const OFF: u8 = 0x70;
pub const PATH_SEP: &str = " \u{203A} ";
const G_SEARCH: &str = "\u{E721}";
const G_CLOSE: &str = "\u{E8BB}";
const G_CLOSED: &str = "\u{E76C}";
const G_OPENED: &str = "\u{E70D}";
const G_PIN: &str = "\u{E718}";
const G_UP: &str = "\u{E70E}";
const G_DOWN: &str = "\u{E70D}";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum MenuKind {
    #[default]
    None,
    File,
    Folder,
    Background,
}

impl MenuKind {
    pub fn pins(self) -> Option<usize> {
        match self {
            MenuKind::None => None,
            MenuKind::File => Some(0),
            MenuKind::Folder => Some(1),
            MenuKind::Background => Some(2),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum What {
    Command { id: usize, checked: bool, stay: bool, mark: Mark, icon: &'static str, disabled: bool },
    Submenu { end: usize },
    Header,
    Separator,
}

#[derive(Clone, Debug, PartialEq)]
struct Node {
    label: String,
    path: String,
    keys: String,
    depth: usize,
    what: What,
}

#[derive(Clone, Debug, PartialEq)]
enum Row {
    Node { node: usize, marks: Vec<Range<usize>>, pinned: bool, path: bool },
    Separator,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CtxHit {
    Field,
    Close,
    Row(usize),
    Star(usize),
    Scroll(bool),
    Box,
}

pub struct ContextMenu {
    nodes: Vec<Node>,
    open: Vec<usize>,
    pins: Vec<String>,
    pub kind: MenuKind,
    pub query: Field,
    rows: Vec<Row>,
    pub(super) rects: Vec<Rect>,
    hot: Option<usize>,
    hover: Option<CtxHit>,
    pressed: Option<CtxHit>,
    scroll: f32,
    content_h: f32,
    pub rect: Rect,
    field: Rect,
    close: Rect,
    list: Rect,
    arrows: [Rect; 2],
    bounds: Rect,
    scale: f32,
    start: (f32, f32),
    armed: bool,
    width: f32,
}

fn flatten(items: &[MenuItem], depth: usize, parent: &str, next: &mut usize, out: &mut Vec<Node>) {
    for item in items {
        let label = match item {
            MenuItem::Command { label, .. } | MenuItem::Submenu { label, .. } | MenuItem::Header(label) => label.clone(),
            MenuItem::Separator => String::new(),
        };
        let path = if parent.is_empty() || label.is_empty() { label.clone() } else { format!("{parent}{PATH_SEP}{label}") };
        match item {
            MenuItem::Command { keys, checked, stay, mark, icon, disabled, .. } => {
                out.push(Node { label, path, keys: keys.clone(), depth, what: What::Command { id: *next, checked: *checked, stay: *stay, mark: *mark, icon, disabled: *disabled } });
                *next += 1;
            }
            MenuItem::Submenu { items, .. } => {
                let at = out.len();
                out.push(Node { label, path: path.clone(), keys: String::new(), depth, what: What::Submenu { end: 0 } });
                flatten(items, depth + 1, &path, next, out);
                out[at].what = What::Submenu { end: out.len() };
            }
            MenuItem::Header(_) => out.push(Node { label, path, keys: String::new(), depth, what: What::Header }),
            MenuItem::Separator => out.push(Node { label, path, keys: String::new(), depth, what: What::Separator }),
        }
    }
}

fn push_separator(rows: &mut Vec<Row>) {
    if rows.last().is_some_and(|r| *r != Row::Separator) {
        rows.push(Row::Separator);
    }
}

impl ContextMenu {
    pub fn new(items: &[MenuItem], kind: MenuKind, pins: Vec<String>, at: (f32, f32), bounds: Rect, scale: f32) -> Self {
        let mut nodes = Vec::new();
        flatten(items, 0, "", &mut 0, &mut nodes);
        let mut m = Self {
            nodes,
            open: Vec::new(),
            pins,
            kind,
            query: Field::default(),
            rows: Vec::new(),
            rects: Vec::new(),
            hot: None,
            hover: None,
            pressed: None,
            scroll: 0.0,
            content_h: 0.0,
            rect: Rect::default(),
            field: Rect::default(),
            close: Rect::default(),
            list: Rect::default(),
            arrows: [Rect::default(); 2],
            bounds,
            scale,
            start: at,
            armed: false,
            width: 0.0,
        };
        m.rebuild();
        m
    }

    #[cfg(test)]
    pub fn labels(&self) -> Vec<String> {
        self.rows
            .iter()
            .map(|r| match r {
                Row::Separator => "-".to_owned(),
                Row::Node { node, pinned, path, .. } => {
                    let n = &self.nodes[*node];
                    let text = if *path { &n.path } else { &n.label };
                    let indent = if *path { String::new() } else { "  ".repeat(n.depth) };
                    let tail = match &n.what {
                        What::Submenu { .. } if self.open.contains(node) => " v",
                        What::Submenu { .. } => " >",
                        _ => "",
                    };
                    format!("{indent}{}{text}{tail}", if *pinned { "* " } else { "" })
                }
            })
            .collect()
    }

    #[cfg(test)]
    pub fn pins(&self) -> &[String] {
        &self.pins
    }

    pub fn set_pins(&mut self, pins: Vec<String>) {
        self.pins = pins;
        self.rebuild();
    }

    #[cfg(test)]
    pub fn is_open(&self, label: &str) -> bool {
        self.open.iter().any(|&i| self.nodes[i].path == label)
    }

    fn pinned_node(&self, key: &str) -> Option<usize> {
        self.nodes.iter().position(|n| n.path == key && matches!(n.what, What::Command { .. }))
    }

    fn walk(&self, from: usize, end: usize, pinned: &[usize], rows: &mut Vec<Row>) {
        let mut i = from;
        while i < end {
            let n = &self.nodes[i];
            match n.what {
                What::Separator => {
                    push_separator(rows);
                    i += 1;
                }
                What::Submenu { end: sub } => {
                    rows.push(Row::Node { node: i, marks: Vec::new(), pinned: false, path: false });
                    if self.open.contains(&i) {
                        push_separator(rows);
                        self.walk(i + 1, sub, pinned, rows);
                        push_separator(rows);
                    }
                    i = sub;
                }
                _ => {
                    if !pinned.contains(&i) {
                        rows.push(Row::Node { node: i, marks: Vec::new(), pinned: false, path: false });
                    }
                    i += 1;
                }
            }
        }
    }

    fn rebuild(&mut self) {
        let hot_node = self.hot.and_then(|r| match self.rows.get(r) {
            Some(Row::Node { node, .. }) => Some(*node),
            _ => None,
        });
        let query = self.query.text().trim().to_owned();
        let pinned: Vec<usize> = self.pins.iter().filter_map(|k| self.pinned_node(k)).collect();
        let mut rows = Vec::new();
        if query.is_empty() {
            rows.extend(pinned.iter().map(|&i| Row::Node { node: i, marks: Vec::new(), pinned: true, path: self.nodes[i].depth > 0 }));
            push_separator(&mut rows);
            self.walk(0, self.nodes.len(), &pinned, &mut rows);
        } else {
            let mut scored: Vec<(i32, usize, Vec<Range<usize>>)> = self
                .nodes
                .iter()
                .enumerate()
                .filter(|(_, n)| matches!(n.what, What::Command { .. }))
                .filter_map(|(i, n)| fuzzy(&query, &n.path).map(|m| (m.score, i, m.ranges)))
                .collect();
            scored.sort_by_key(|&(s, i, _)| (Reverse(s), i));
            rows.extend(scored.into_iter().map(|(_, i, marks)| Row::Node { node: i, marks, pinned: pinned.contains(&i), path: true }));
        }
        while rows.last() == Some(&Row::Separator) {
            rows.pop();
        }
        if rows.first() == Some(&Row::Separator) {
            rows.remove(0);
        }
        self.rows = rows;
        self.hot = hot_node.and_then(|n| self.rows.iter().position(|r| matches!(r, Row::Node { node, .. } if *node == n)));
        if !query.is_empty() && self.hot.is_none() {
            self.hot = self.selectable().first().copied();
        }
        self.layout();
    }

    fn row_h(&self, r: &Row) -> f32 {
        (if *r == Row::Separator { SEPARATOR_H } else { ROW_H }) * self.scale
    }

    pub fn measure<B: TextBackend>(&mut self, g: &mut Gfx<B>) {
        let s = self.scale;
        let font = font(s);
        let mut w = MIN_W * s;
        for n in &self.nodes {
            if matches!(n.what, What::Separator) {
                continue;
            }
            let keys = if n.keys.is_empty() { 0.0 } else { KEYS_GAP * s + g.measure(&n.keys, font) };
            let own = (ICON_W + INDENT * n.depth as f32) * s + g.measure(&n.label, font);
            let path = ICON_W * s + g.measure(&n.path, font);
            w = w.max(own.max(path) + keys + (STAR_W + SIDE_W + 2.0 * PAD + SCROLL_W) * s);
        }
        self.width = w.min(MAX_W * s).min(self.bounds.w).ceil();
        self.layout();
    }

    fn layout(&mut self) {
        let s = self.scale;
        let b = self.bounds;
        let w = if self.width > 0.0 { self.width } else { (MIN_W * s).min(b.w) };
        self.content_h = self.rows.iter().map(|r| self.row_h(r)).sum::<f32>().max(ROW_H * s);
        let head = FIELD_H * s + 2.0 * PAD * s;
        let total = (head + self.content_h + PAD * s).min(b.h);
        let (x, y) = self.start;
        let x = if x + w <= b.right() { x } else { (x - w).max(b.x) };
        let y = if y + total <= b.bottom() { y } else if y - total >= b.y { y - total } else { b.bottom() - total };
        self.rect = Rect::new(x.min(b.right() - w).max(b.x), y.max(b.y), w, total);
        let r = self.rect;
        self.field = Rect::new(r.x + PAD * s, r.y + PAD * s, r.w - 2.0 * PAD * s, FIELD_H * s);
        self.close = Rect::new(self.field.right() - FIELD_H * s, self.field.y, FIELD_H * s, FIELD_H * s);
        self.list = Rect::new(r.x + PAD * s, self.field.bottom() + PAD * s, r.w - 2.0 * PAD * s, r.bottom() - self.field.bottom() - 2.0 * PAD * s);
        let max = (self.content_h - self.list.h).max(0.0);
        self.scroll = self.scroll.clamp(0.0, max);
        let scrolls = max > 0.0;
        let sw = SCROLL_W * s;
        self.arrows = if scrolls { [Rect::new(self.list.right() - sw, self.list.y, sw, sw), Rect::new(self.list.right() - sw, self.list.bottom() - sw, sw, sw)] } else { [Rect::default(); 2] };
        let rw = self.list.w - if scrolls { sw + PAD * s } else { 0.0 };
        let mut y = self.list.y - self.scroll;
        self.rects = self
            .rows
            .iter()
            .map(|row| {
                let h = self.row_h(row);
                y += h;
                Rect::new(self.list.x, y - h, rw, h)
            })
            .collect();
    }

    fn selectable(&self) -> Vec<usize> {
        self.rows
            .iter()
            .enumerate()
            .filter(|(_, r)| match r {
                Row::Node { node, .. } => matches!(self.nodes[*node].what, What::Command { disabled: false, .. } | What::Submenu { .. }),
                Row::Separator => false,
            })
            .map(|(i, _)| i)
            .collect()
    }

    pub(super) fn star_rect(&self, row: Rect) -> Rect {
        Rect::new(row.right() - SIDE_W * self.scale - STAR_W * self.scale, row.y, STAR_W * self.scale, row.h)
    }

    fn pinnable(&self, row: usize) -> bool {
        matches!(self.rows.get(row), Some(Row::Node { node, .. }) if matches!(self.nodes[*node].what, What::Command { .. })) && self.kind.pins().is_some()
    }

    pub fn hit(&self, x: f32, y: f32) -> Option<CtxHit> {
        if !self.rect.contains(x, y) {
            return None;
        }
        if self.close.contains(x, y) {
            return Some(CtxHit::Close);
        }
        if self.field.contains(x, y) {
            return Some(CtxHit::Field);
        }
        if let Some(k) = self.arrows.iter().position(|r| r.contains(x, y)) {
            return Some(CtxHit::Scroll(k == 1));
        }
        if !self.list.contains(x, y) {
            return Some(CtxHit::Box);
        }
        let Some(k) = self.rects.iter().position(|r| r.contains(x, y)) else { return Some(CtxHit::Box) };
        if self.pinnable(k) && self.star_rect(self.rects[k]).contains(x, y) {
            return Some(CtxHit::Star(k));
        }
        Some(CtxHit::Row(k))
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        self.rect.contains(x, y)
    }

    pub fn mouse_move(&mut self, x: f32, y: f32) -> bool {
        if !self.armed && (x - self.start.0).hypot(y - self.start.1) > ARM_DISTANCE * self.scale {
            self.armed = true;
        }
        let before = (self.hover, self.hot);
        self.hover = self.hit(x, y);
        if let Some(CtxHit::Row(k) | CtxHit::Star(k)) = self.hover {
            if self.selectable().contains(&k) {
                self.hot = Some(k);
            }
        }
        before != (self.hover, self.hot)
    }

    pub fn mouse_down(&mut self, x: f32, y: f32, clicks: u8) -> MenuOutcome {
        self.armed = true;
        self.pressed = self.hit(x, y);
        match self.pressed {
            None => MenuOutcome::Closed,
            Some(CtxHit::Field) => {
                let inner = self.field_inner();
                self.query.click(x.max(inner.x), clicks);
                MenuOutcome::Open
            }
            Some(CtxHit::Scroll(down)) => {
                self.scroll_by(if down { ROW_H } else { -ROW_H } * self.scale);
                MenuOutcome::Open
            }
            _ => MenuOutcome::Open,
        }
    }

    pub fn mouse_up(&mut self, x: f32, y: f32) -> MenuOutcome {
        let pressed = self.pressed.take();
        if !self.armed {
            return MenuOutcome::Open;
        }
        let up = self.hit(x, y);
        if pressed.is_some() && pressed != up {
            return MenuOutcome::Open;
        }
        match up {
            Some(CtxHit::Close) => self.dismiss(),
            Some(CtxHit::Star(k)) => self.pin_outcome(k),
            Some(CtxHit::Row(k)) => self.activate(k),
            _ => MenuOutcome::Open,
        }
    }

    fn dismiss(&mut self) -> MenuOutcome {
        if self.query.text().is_empty() {
            return MenuOutcome::Closed;
        }
        self.query.edit.set("");
        self.rebuild();
        MenuOutcome::Open
    }

    fn pin_outcome(&self, k: usize) -> MenuOutcome {
        let Some(Row::Node { node, pinned, .. }) = self.rows.get(k) else { return MenuOutcome::Open };
        MenuOutcome::Pin(self.nodes[*node].path.clone(), !pinned, self.star_rect(self.rects[k]))
    }

    fn activate(&mut self, k: usize) -> MenuOutcome {
        let Some(Row::Node { node, .. }) = self.rows.get(k) else { return MenuOutcome::Open };
        let node = *node;
        match &mut self.nodes[node].what {
            What::Command { disabled: true, .. } => MenuOutcome::Open,
            What::Command { stay: true, checked, id, .. } => {
                *checked = !*checked;
                MenuOutcome::Toggled(*id)
            }
            What::Command { id, .. } => MenuOutcome::Command(*id),
            What::Submenu { .. } => {
                self.toggle(node);
                MenuOutcome::Open
            }
            _ => MenuOutcome::Open,
        }
    }

    fn toggle(&mut self, node: usize) {
        match self.open.iter().position(|&i| i == node) {
            Some(k) => {
                self.open.remove(k);
            }
            None => self.open.push(node),
        }
        self.rebuild();
        self.reveal();
    }

    pub fn wheel(&mut self, dy: f32) {
        self.scroll_by(-dy * 3.0 * ROW_H * self.scale);
    }

    fn scroll_by(&mut self, d: f32) {
        self.scroll += d;
        self.layout();
    }

    fn reveal(&mut self) {
        let Some(r) = self.hot.and_then(|k| self.rects.get(k).copied()) else { return };
        if r.y < self.list.y {
            self.scroll_by(r.y - self.list.y);
        } else if r.bottom() > self.list.bottom() {
            self.scroll_by(r.bottom() - self.list.bottom());
        }
    }

    fn step(&mut self, d: isize) {
        let rows = self.selectable();
        if rows.is_empty() {
            return;
        }
        let n = rows.len() as isize;
        let at = self.hot.and_then(|h| rows.iter().position(|&r| r == h)).map(|i| i as isize);
        let next = match (d, at) {
            (isize::MIN, _) => 0,
            (isize::MAX, _) => n - 1,
            (d, None) if d > 0 => 0,
            (_, None) => n - 1,
            (d, Some(i)) => (i + d).rem_euclid(n),
        };
        self.hot = Some(rows[next as usize]);
        self.reveal();
    }

    pub fn key(&mut self, key: Key, m: Mods) -> (MenuOutcome, Option<EditAction>) {
        let hot_node = self.hot.and_then(|k| match self.rows.get(k) {
            Some(Row::Node { node, .. }) => Some(*node),
            _ => None,
        });
        let outcome = match key {
            Key::Escape => self.dismiss(),
            Key::Down => {
                self.step(1);
                MenuOutcome::Open
            }
            Key::Up => {
                self.step(-1);
                MenuOutcome::Open
            }
            Key::PageDown | Key::End if !m.shift && self.query.text().is_empty() => {
                self.step(isize::MAX);
                MenuOutcome::Open
            }
            Key::PageUp | Key::Home if !m.shift && self.query.text().is_empty() => {
                self.step(isize::MIN);
                MenuOutcome::Open
            }
            Key::Enter => self.hot.map_or(MenuOutcome::Open, |k| self.activate(k)),
            Key::Right if self.query.text().is_empty() => {
                if let Some(n) = hot_node.filter(|n| matches!(self.nodes[*n].what, What::Submenu { .. }) && !self.open.contains(n)) {
                    self.toggle(n);
                }
                MenuOutcome::Open
            }
            Key::Left if self.query.text().is_empty() => {
                if let Some(n) = hot_node {
                    let target = if self.open.contains(&n) { Some(n) } else { self.parent(n) };
                    if let Some(p) = target {
                        self.toggle(p);
                        self.hot = self.rows.iter().position(|r| matches!(r, Row::Node { node, .. } if *node == p));
                    }
                }
                MenuOutcome::Open
            }
            _ => match self.query.key(key, m) {
                FieldKey::Edited { changed, action } => {
                    if changed {
                        self.rebuild();
                    }
                    return (MenuOutcome::Open, Some(action));
                }
                FieldKey::Ignored => MenuOutcome::Open,
            },
        };
        (outcome, None)
    }

    fn parent(&self, n: usize) -> Option<usize> {
        (0..n).rev().find(|&i| matches!(self.nodes[i].what, What::Submenu { end } if end > n))
    }

    pub fn insert(&mut self, s: &str) {
        if s.chars().all(char::is_control) {
            return;
        }
        self.query.insert(s);
        self.rebuild();
    }

    fn field_inner(&self) -> Rect {
        let f = self.field;
        Rect::new(f.x + f.h, f.y, (f.w - 2.0 * f.h).max(0.0), f.h)
    }

    pub fn ime_area(&self) -> Rect {
        self.field_inner()
    }

    pub fn draw<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme, active: bool, preedit: &Preedit) {
        let s = self.scale;
        let (font, icons) = (font(s), FontStyle::new(FontFamily::Icons, ICON_PX * s, false));
        let mut p = Paint::new(g, t, s);
        p.frame(self.rect);
        p.field(self.field, true);
        p.centered(Rect::new(self.field.x, self.field.y, self.field.h, self.field.h), G_SEARCH, icons, t.text_dim);
        let inner = self.field_inner();
        self.query.draw(p.g, t, inner, font, active, S::MenusSearchCommand.t(), s, Some(preedit));
        p.icon_button(self.close, G_CLOSE, FontStyle::new(FontFamily::Icons, 9.0 * s, false), Button::of(self.hover == Some(CtxHit::Close), false, true), t.text_dim);
        p.g.dl.set_clip(Some(self.list));
        for (k, row) in self.rows.iter().enumerate() {
            let r = self.rects[k];
            if r.bottom() < self.list.y || r.y > self.list.bottom() {
                continue;
            }
            let Row::Node { node, marks, pinned, path } = row else {
                p.separator(r.x + PAD * s, r.y + r.h / 2.0, r.w - 2.0 * PAD * s);
                continue;
            };
            let n = &self.nodes[*node];
            let hot = self.hot == Some(k);
            if hot {
                p.g.dl.round_rect(r, t.accent_fill, 4.0 * s);
            }
            let off = matches!(n.what, What::Command { disabled: true, .. } | What::Header);
            let (ink, dim, mark) = match (hot, off) {
                (true, _) => (ink_on(t.accent_fill), ink_on(t.accent_fill), ink_on(t.accent_fill)),
                (false, true) => (t.text_dim.alpha(OFF), t.text_dim.alpha(OFF), t.accent.alpha(OFF)),
                (false, false) => (t.text, t.text_dim, t.accent),
            };
            let depth = if *path { 0.0 } else { n.depth as f32 };
            let x0 = r.x + depth * INDENT * s;
            let side = Rect::new(r.right() - SIDE_W * s, r.y, SIDE_W * s, r.h);
            let star = self.star_rect(r);
            let text_y = baseline(p.g, r, font);
            let keys_w = if n.keys.is_empty() { 0.0 } else { p.g.measure(&n.keys, font) };
            let star_on = *pinned || (hot && self.pinnable(k) && self.hover.is_some());
            let keys_right = if star_on { star.x } else { side.x };
            let label_x = x0 + ICON_W * s;
            let room = keys_right - label_x - if keys_w > 0.0 { keys_w + KEYS_GAP * s } else { 0.0 };
            let text = if *path { &n.path } else { &n.label };
            p.marked((label_x, text_y), text, marks, font, (ink, mark), room);
            if let What::Command { icon, .. } = &n.what {
                if !icon.is_empty() {
                    p.centered(Rect::new(x0, r.y, ICON_W * s, r.h), icon, icons, if off && !hot { dim } else { ink });
                }
            }
            if keys_w > 0.0 {
                p.g.text(keys_right - keys_w - PAD * s, text_y, &n.keys, font, dim, f32::INFINITY);
            }
            if star_on {
                let ink = if *pinned && !hot { t.accent } else { dim };
                p.centered(star, G_PIN, icons, ink);
            }
            match &n.what {
                What::Command { checked, mark: m, .. } => {
                    let bx = Rect::new(side.x + (side.w - 14.0 * s) / 2.0, r.y + (r.h - 14.0 * s) / 2.0, 14.0 * s, 14.0 * s).snap();
                    match (m, checked) {
                        (Mark::Radio, on) => p.radio(bx, *on, None),
                        (Mark::Sort(desc), on) => p.radio(bx, *on, on.then_some((!*desc, FontStyle::new(FontFamily::Ui, 10.0 * s, false)))),
                        (Mark::Check, on) | (Mark::None, on @ true) => p.checkbox(bx, *on),
                        (Mark::None, false) => {}
                    }
                }
                What::Submenu { .. } => p.centered(side, if self.open.contains(node) { G_OPENED } else { G_CLOSED }, icons, ink),
                What::Header | What::Separator => {}
            }
        }
        if self.rows.is_empty() {
            let r = Rect::new(self.list.x, self.list.y, self.list.w, ROW_H * s);
            let y = baseline(p.g, r, font);
            p.g.text(r.x + ICON_W * s, y, S::MenusNoMatch.t(), font, t.text_dim, r.w);
        }
        p.g.dl.set_clip(None);
        if self.arrows[0].w > 0.0 {
            for (k, glyph) in [G_UP, G_DOWN].into_iter().enumerate() {
                p.icon_button(self.arrows[k], glyph, FontStyle::new(FontFamily::Icons, 8.0 * s, false), Button::of(self.hover == Some(CtxHit::Scroll(k == 1)), false, true), t.text_dim);
            }
        }
    }
}

fn font(scale: f32) -> FontStyle {
    FontStyle::new(FontFamily::Ui, FONT_PX * scale, false)
}

#[cfg(test)]
#[path = "../../test/ui/context.rs"]
mod tests;
