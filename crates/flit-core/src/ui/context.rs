use super::edit::EditAction;
use super::field::{is_menu_key, Field, FieldKey, TextCmd};
use super::fuzzy::fuzzy;
use super::menu::{Mark, MenuImage, MenuItem};
use super::popup::MenuOutcome;
use super::widgets::{baseline, Button, Edge, Paint, Unfold};
use crate::i18n::S;
use crate::input::{Key, Mods, Preedit};
use crate::render::Rect;
use crate::text::{FontFamily, FontStyle, Gfx, TextBackend};
use crate::theme::{ink_on, Theme};
use std::cmp::Reverse;
use std::ops::Range;
use std::time::Instant;

const ROW_H: f32 = 24.0;
const SEPARATOR_H: f32 = 7.0;
const FIELD_H: f32 = 28.0;
const PAD: f32 = 4.0;
const ICON_W: f32 = 26.0;
const INDENT: f32 = ICON_W;
const SIDE_W: f32 = 22.0;
const STAR_W: f32 = 22.0;
const KEYS_GAP: f32 = 24.0;
const MIN_W: f32 = 232.0;
const IMAGE_PX: f32 = 16.0;
const MAX_W: f32 = 480.0;
const SCROLL_W: f32 = 10.0;
const FONT_PX: f32 = 13.0;
const ICON_PX: f32 = 11.0;
const FIELD_MENU_W: f32 = 180.0;
const FIELD_MENU: [(TextCmd, S); 4] = [(TextCmd::Cut, S::MenuCut), (TextCmd::Copy, S::MenuCopy), (TextCmd::Paste, S::MenuPaste), (TextCmd::SelectAll, S::MenuFieldSelectAll)];

pub(super) struct FieldMenu {
    pub rect: Rect,
    pub rows: Vec<(TextCmd, Rect, bool)>,
    pub hot: Option<usize>,
}
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
    Command { id: usize, checked: bool, stay: bool, mark: Mark, icon: &'static str, image: Option<MenuImage>, disabled: bool },
    Submenu { end: usize },
    Header,
    Separator,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Role {
    Opening,
    ListOpening,
    Properties,
}

pub(super) fn roles(cmds: &[super::menu::Cmd]) -> Vec<Option<Role>> {
    use super::menu::Cmd;
    use super::sidebar::PlaceCmd;
    cmds.iter()
        .map(|c| match c {
            Cmd::Action("item.open" | "item.open_tab" | "item.open_window" | "panel.open_right" | "panel.open_bottom") => Some(Role::ListOpening),
            Cmd::Place(PlaceCmd::Open(..) | PlaceCmd::OpenFile(_)) => Some(Role::Opening),
            Cmd::Action("item.properties") | Cmd::FolderProperties(_) | Cmd::Place(PlaceCmd::Properties(_) | PlaceCmd::FileProperties(_)) => Some(Role::Properties),
            _ => None,
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq)]
struct Node {
    label: String,
    path: String,
    keys: String,
    depth: usize,
    hidden: bool,
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
    ScrollThumb,
    ScrollTrack,
    Box,
}

pub struct ContextMenu {
    nodes: Vec<Node>,
    top: Vec<Option<usize>>,
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
    pub(super) field: Rect,
    close: Rect,
    list: Rect,
    arrows: [Rect; 2],
    track: Rect,
    thumb: Rect,
    grab: Option<f32>,
    bounds: Rect,
    scale: f32,
    start: (f32, f32),
    armed: bool,
    width: f32,
    anchored: bool,
    pub floor_w: f32,
    glow: super::widgets::Glow<usize>,
    star_glow: super::widgets::Glow<usize>,
    glow_at: Option<Instant>,
    shell: Option<(usize, Option<usize>)>,
    unfold: Option<(Instant, Instant)>,
    fresh: Option<(usize, usize, Option<Instant>)>,
    roles: Vec<Option<Role>>,
    pub(super) field_menu: Option<FieldMenu>,
}

fn flatten(items: &[MenuItem], depth: usize, parent: &str, next: &mut usize, out: &mut Vec<Node>) {
    for item in items {
        let label = match item {
            MenuItem::Command { label, .. } | MenuItem::Submenu { label, .. } | MenuItem::Header(label) => label.clone(),
            MenuItem::Separator => String::new(),
        };
        let path = if parent.is_empty() || label.is_empty() { label.clone() } else { format!("{parent}{PATH_SEP}{label}") };
        match item {
            MenuItem::Command { keys, checked, stay, mark, icon, image, disabled, .. } => {
                out.push(Node { label, path, keys: keys.clone(), depth, hidden: false, what: What::Command { id: *next, checked: *checked, stay: *stay, mark: *mark, icon, image: image.clone(), disabled: *disabled } });
                *next += 1;
            }
            MenuItem::Submenu { items, .. } => {
                let at = out.len();
                out.push(Node { label, path: path.clone(), keys: String::new(), depth, hidden: false, what: What::Submenu { end: 0 } });
                flatten(items, depth + 1, &path, next, out);
                out[at].what = What::Submenu { end: out.len() };
            }
            MenuItem::Header(_) => out.push(Node { label, path, keys: String::new(), depth, hidden: false, what: What::Header }),
            MenuItem::Separator => out.push(Node { label, path, keys: String::new(), depth, hidden: false, what: What::Separator }),
        }
    }
}

fn push_separator(rows: &mut Vec<Row>) {
    if rows.last().is_some_and(|r| *r != Row::Separator) {
        rows.push(Row::Separator);
    }
}

impl ContextMenu {
    pub(super) fn set_disabled(&mut self, command: usize, value: bool) {
        for node in &mut self.nodes {
            if let What::Command { id, disabled, .. } = &mut node.what {
                if *id == command {
                    *disabled = value;
                }
            }
        }
    }

    pub fn new(items: &[MenuItem], kind: MenuKind, pins: Vec<String>, at: (f32, f32), bounds: Rect, scale: f32) -> Self {
        let mut nodes = Vec::new();
        flatten(items, 0, "", &mut 0, &mut nodes);
        let mut m = Self {
            nodes,
            top: Vec::new(),
            open: Vec::new(),
            pins,
            kind,
            query: Field::default(),
            field_menu: None,
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
            track: Rect::default(),
            thumb: Rect::default(),
            grab: None,
            bounds,
            scale,
            start: at,
            armed: false,
            width: 0.0,
            anchored: false,
            floor_w: 0.0,
            glow: Default::default(),
            star_glow: Default::default(),
            glow_at: None,
            shell: None,
            unfold: None,
            fresh: None,
            roles: Vec::new(),
        };
        m.arrange(&[], &[]);
        m
    }

    pub fn arrange(&mut self, hidden: &[String], order: &[String]) {
        for n in &mut self.nodes {
            n.hidden = !n.path.is_empty() && hidden.contains(&n.path);
        }
        let shell = if order.is_empty() { self.shell } else { None };
        let list_opening = self.roles.contains(&Some(Role::ListOpening));
        let mut items = Vec::new();
        let (mut group, mut anchor, mut i) = (0, -1, 0);
        while i < self.nodes.len() {
            let n = &self.nodes[i];
            if n.what == What::Separator {
                group += 1;
            } else {
                if let Some(p) = order.iter().position(|o| *o == n.path) {
                    anchor = p as isize;
                }
                if !n.hidden {
                    let (slot, group) = match shell {
                        Some((_, Some(tail))) if i == tail => (3, group),
                        Some((from, _)) if i >= from => (1, 0),
                        Some(_) if group == 0 && list_opening => (2, 0),
                        _ if group == 0 => (0, 0),
                        _ => (2, group),
                    };
                    let (slot, group) = match self.role(i) {
                        Some(Role::Opening | Role::ListOpening) if order.is_empty() && self.kind != MenuKind::None => (0, 0),
                        Some(Role::Properties) if order.is_empty() && self.kind != MenuKind::None => (4, usize::MAX),
                        _ => (slot, group),
                    };
                    items.push((anchor, slot, group, i));
                }
            }
            i = match n.what {
                What::Submenu { end } => end,
                _ => i + 1,
            };
        }
        items.sort_by_key(|&(anchor, slot, ..)| (anchor, slot));
        self.top.clear();
        let mut last = None;
        for (_, _, group, node) in items {
            if last.is_some_and(|g| g != group) {
                self.top.push(None);
            }
            last = Some(group);
            self.top.push(Some(node));
        }
        self.rebuild();
    }

    pub(super) fn set_roles(&mut self, roles: Vec<Option<Role>>) {
        self.roles = roles;
        self.arrange(&[], &[]);
    }

    fn role(&self, i: usize) -> Option<Role> {
        if self.shell.is_some_and(|(from, _)| i >= from) {
            return (self.nodes[i].label == S::MenuOpenWith.t()).then_some(Role::Opening);
        }
        let end = match self.nodes[i].what {
            What::Submenu { end } => end,
            _ => i + 1,
        };
        let id = self.nodes[i..end].iter().find_map(|n| match n.what {
            What::Command { id, .. } => Some(id),
            _ => None,
        })?;
        self.roles.get(id).copied().flatten()
    }

    pub fn lift_shell(&mut self, own: usize, keep_tail: bool) {
        let mut tops = Vec::new();
        let mut i = 0;
        while i < self.nodes.len() {
            tops.push(i);
            i = match self.nodes[i].what {
                What::Submenu { end } => end,
                _ => i + 1,
            };
        }
        let Some(&from) = tops.get(own) else { return };
        let tail = tops.last().copied().filter(|_| keep_tail).filter(|&t| t > from);
        self.shell = Some((from, tail));
        self.arrange(&[], &[]);
    }

    pub fn seen(&self) -> Vec<String> {
        self.nodes.iter().filter(|n| !n.label.is_empty() && matches!(n.what, What::Command { .. } | What::Submenu { .. })).map(|n| n.path.clone()).collect()
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
            if n.what == What::Separator {
                push_separator(rows);
            } else if !n.hidden {
                self.push_node(i, pinned, rows);
            }
            i = match n.what {
                What::Submenu { end: sub } => sub,
                _ => i + 1,
            };
        }
    }

    fn push_node(&self, i: usize, pinned: &[usize], rows: &mut Vec<Row>) {
        match self.nodes[i].what {
            What::Submenu { end } => {
                rows.push(Row::Node { node: i, marks: Vec::new(), pinned: false, path: false });
                if self.open.contains(&i) {
                    push_separator(rows);
                    self.walk(i + 1, end, pinned, rows);
                    push_separator(rows);
                }
            }
            _ if pinned.contains(&i) => {}
            _ => rows.push(Row::Node { node: i, marks: Vec::new(), pinned: false, path: false }),
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
            for top in &self.top {
                match top {
                    Some(i) => self.push_node(*i, &pinned, &mut rows),
                    None => push_separator(&mut rows),
                }
            }
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
        let mut w = (MIN_W * s).max(self.floor_w);
        for n in &self.nodes {
            if matches!(n.what, What::Separator) {
                continue;
            }
            let keys = if n.keys.is_empty() { 0.0 } else { KEYS_GAP * s + g.measure(&n.keys, font) };
            let own = (ICON_W + INDENT * n.depth as f32) * s + g.measure(&n.label, font);
            w = w.max(own + keys + (STAR_W + SIDE_W + 2.0 * PAD + SCROLL_W) * s);
        }
        self.width = w.min(MAX_W * s).min(self.bounds.w).ceil();
        self.layout();
        self.anchored = true;
    }

    pub fn width(&self) -> f32 {
        self.width
    }

    fn layout(&mut self) {
        let s = self.scale;
        let b = self.bounds;
        let w = if self.width > 0.0 { self.width } else { MIN_W * s }.min(b.w).max(0.0);
        let w = if self.anchored { w.min(b.right() - self.rect.x) } else { w };
        self.content_h = self.rows.iter().map(|r| self.row_h(r)).sum::<f32>().max(ROW_H * s);
        let head = FIELD_H * s + 2.0 * PAD * s;
        let anchored = self.anchored;
        let total = (head + self.content_h + PAD * s).min(if anchored { b.bottom() - self.rect.y } else { b.h });
        let (x, y) = self.start;
        let x = if anchored { self.rect.x } else if x + w <= b.right() { x } else { (x - w).max(b.x) };
        let y = if anchored { self.rect.y } else if y + total <= b.bottom() { y } else if y - total >= b.y { y - total } else { b.bottom() - total };
        self.rect = Rect::new(x.min(b.right() - w).max(b.x), y.max(b.y), w, total);
        let r = self.rect;
        self.field = Rect::new(r.x + PAD * s, r.y + PAD * s, r.w - 2.0 * PAD * s, FIELD_H * s);
        self.close = Rect::new(self.field.right() - FIELD_H * s, self.field.y, FIELD_H * s, FIELD_H * s);
        self.list = Rect::new(r.x + PAD * s, self.field.bottom() + PAD * s, (r.w - 2.0 * PAD * s).max(0.0), (r.bottom() - self.field.bottom() - 2.0 * PAD * s).max(0.0));
        let overflow = self.content_h - self.list.h;
        let max = if overflow > 0.5 { overflow } else { 0.0 };
        self.scroll = self.scroll.clamp(0.0, max);
        let scrolls = max > 0.0;
        let sw = SCROLL_W * s;
        self.arrows = if scrolls { [Rect::new(self.list.right() - sw, self.list.y, sw, sw), Rect::new(self.list.right() - sw, self.list.bottom() - sw, sw, sw)] } else { [Rect::default(); 2] };
        self.track = if scrolls { Rect::new(self.list.right() - sw, self.list.y + sw, sw, (self.list.h - 2.0 * sw).max(0.0)) } else { Rect::default() };
        let h = (self.track.h * self.list.h / self.content_h).max(18.0 * s).min(self.track.h);
        self.thumb = Rect::new(self.track.x, self.track.y + if max > 0.0 { (self.track.h - h) * self.scroll / max } else { 0.0 }, self.track.w, h);
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

    fn side_used(&self, k: usize) -> bool {
        match self.rows.get(k) {
            Some(Row::Node { node, .. }) => match &self.nodes[*node].what {
                What::Command { mark, checked, .. } => *mark != Mark::None || *checked,
                What::Submenu { .. } => true,
                What::Header | What::Separator => false,
            },
            _ => false,
        }
    }

    pub(super) fn star_rect(&self, k: usize) -> Rect {
        let row = self.rects.get(k).copied().unwrap_or_default();
        let side = if self.side_used(k) { SIDE_W * self.scale } else { 0.0 };
        Rect::new(row.right() - side - STAR_W * self.scale, row.y, STAR_W * self.scale, row.h)
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
        if self.thumb.contains(x, y) { return Some(CtxHit::ScrollThumb); }
        if self.track.contains(x, y) { return Some(CtxHit::ScrollTrack); }
        if !self.list.contains(x, y) {
            return Some(CtxHit::Box);
        }
        let Some(k) = self.rects.iter().position(|r| r.contains(x, y)) else { return Some(CtxHit::Box) };
        if self.pinnable(k) && self.star_rect(k).contains(x, y) {
            return Some(CtxHit::Star(k));
        }
        Some(CtxHit::Row(k))
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        self.rect.contains(x, y)
    }

    pub fn mouse_move(&mut self, x: f32, y: f32) -> bool {
        if let Some(grab) = self.grab {
            let travel = self.track.h - self.thumb.h;
            if travel > 0.0 {
                self.scroll = ((y - self.track.y - grab) / travel).clamp(0.0, 1.0) * (self.content_h - self.list.h).max(0.0);
                self.layout();
            }
            return true;
        }
        if let Some(menu) = self.field_menu.as_mut() {
            let hot = menu.rows.iter().position(|(_, r, on)| *on && r.contains(x, y));
            return std::mem::replace(&mut menu.hot, hot) != hot;
        }
        if self.pressed == Some(CtxHit::Field) {
            let inner = self.field_inner();
            self.query.drag(x.max(inner.x));
            return true;
        }
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

    pub fn mouse_down(&mut self, x: f32, y: f32, clicks: u8, extend: bool) -> MenuOutcome {
        self.armed = true;
        self.pressed = self.hit(x, y);
        match self.pressed {
            Some(CtxHit::ScrollThumb) => {
                self.grab = Some(y - self.thumb.y);
                MenuOutcome::Open
            }
            Some(CtxHit::ScrollTrack) => {
                self.scroll_by(if y < self.thumb.y { -self.list.h } else { self.list.h });
                MenuOutcome::Open
            }
            None => MenuOutcome::Closed,
            Some(CtxHit::Field) => {
                let inner = self.field_inner();
                self.query.click(x.max(inner.x), clicks, extend);
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
        if self.grab.take().is_some() { return MenuOutcome::Open; }
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
        MenuOutcome::Pin(self.nodes[*node].path.clone(), !pinned, self.star_rect(k))
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
            None => {
                self.open.push(node);
                if let What::Submenu { end } = self.nodes[node].what {
                    self.fresh = Some((node, end, None));
                }
            }
        }
        self.rebuild();
        self.reveal();
    }

    pub fn wheel(&mut self, dy: f32) {
        self.scroll_by(-dy * 3.0 * ROW_H * self.scale);
    }

    pub fn pans_at(&self, x: f32, y: f32) -> bool {
        self.field_menu.is_none() && self.list.contains(x, y) && matches!(self.hit(x, y), Some(CtxHit::Row(_) | CtxHit::Star(_) | CtxHit::Box))
    }

    pub fn pan(&mut self, dy: f32) -> bool {
        let was = self.scroll;
        self.scroll_by(-dy);
        self.scroll != was
    }

    pub fn let_go(&mut self) {
        self.pressed = None;
        self.grab = None;
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

    pub fn open_field_menu(&mut self, x: f32, y: f32) {
        let s = self.scale;
        let (w, rh) = (FIELD_MENU_W * s, ROW_H * s);
        let e = &self.query.edit;
        let (picked, whole) = (!e.selection().is_empty(), e.selection() == (0..e.text.len()));
        let enabled = |c: TextCmd| match c {
            TextCmd::Cut | TextCmd::Copy => picked,
            TextCmd::SelectAll => !whole && !e.text.is_empty(),
            _ => true,
        };
        let h = FIELD_MENU.len() as f32 * rh + 2.0 * PAD * s;
        let b = self.rect;
        let x = x.min(b.right() - w).max(b.x);
        let y = y.min(b.bottom() - h).max(b.y);
        let rows = FIELD_MENU.iter().enumerate().map(|(k, &(c, _))| (c, Rect::new(x + PAD * s, y + PAD * s + k as f32 * rh, w - 2.0 * PAD * s, rh), enabled(c))).collect();
        self.field_menu = Some(FieldMenu { rect: Rect::new(x, y, w, h), rows, hot: None });
        self.pressed = Some(CtxHit::Box);
    }

    pub fn pick_field_menu(&mut self, x: f32, y: f32) -> Option<Option<TextCmd>> {
        let menu = self.field_menu.take()?;
        self.pressed = Some(CtxHit::Box);
        Some(menu.rows.iter().find(|(_, r, on)| *on && r.contains(x, y)).map(|(c, ..)| *c))
    }

    fn field_menu_key(&mut self, key: Key) -> Option<(MenuOutcome, Option<EditAction>)> {
        let menu = self.field_menu.as_mut()?;
        let enabled: Vec<usize> = (0..menu.rows.len()).filter(|&k| menu.rows[k].2).collect();
        match key {
            Key::Up | Key::Down => {
                let at = menu.hot.and_then(|h| enabled.iter().position(|&k| k == h));
                let n = enabled.len();
                menu.hot = match (at, key == Key::Down) {
                    (_, _) if n == 0 => None,
                    (None, true) => enabled.first().copied(),
                    (None, false) => enabled.last().copied(),
                    (Some(i), true) => Some(enabled[(i + 1) % n]),
                    (Some(i), false) => Some(enabled[(i + n - 1) % n]),
                };
                Some((MenuOutcome::Open, None))
            }
            Key::Enter => {
                let cmd = menu.hot.map(|h| menu.rows[h].0);
                self.field_menu = None;
                Some(cmd.map_or((MenuOutcome::Open, None), |c| {
                    let (k, m) = c.chord();
                    self.key(k, m)
                }))
            }
            _ => {
                self.field_menu = None;
                (key == Key::Escape).then_some((MenuOutcome::Open, None))
            }
        }
    }

    pub fn key(&mut self, key: Key, m: Mods) -> (MenuOutcome, Option<EditAction>) {
        if let Some(done) = self.field_menu_key(key) {
            return done;
        }
        if is_menu_key(key, m) {
            let f = self.field;
            self.open_field_menu(f.x + f.h, f.bottom());
            return (MenuOutcome::Open, None);
        }
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

    pub fn settle_unfold(&mut self, since: Instant, now: Instant, animate: bool) -> bool {
        if let Some(f) = self.fresh.as_mut() {
            f.2 = f.2.or(Some(now));
        }
        self.fresh = self.fresh.filter(|f| animate && f.2.is_some_and(|b| Unfold::running(b, now)));
        self.unfold = animate.then_some((since, now));
        animate && (Unfold::running(since, now) || self.fresh.is_some())
    }

    pub fn settle_glow(&mut self, now: Instant, animate: bool) -> bool {
        let hot = self.hot.and_then(|k| match self.rows.get(k) {
            Some(Row::Node { node, .. }) => Some(*node),
            _ => None,
        });
        let star = match self.hover {
            Some(CtxHit::Star(k)) => match self.rows.get(k) {
                Some(Row::Node { node, .. }) => Some(*node),
                _ => None,
            },
            _ => None,
        };
        self.glow_at = Some(now);
        self.glow.aim(hot, animate, now) | self.star_glow.aim(star, animate, now)
    }

    pub fn draw<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme, active: bool, preedit: &Preedit) {
        let s = self.scale;
        let (font, icons) = (font(s), FontStyle::new(FontFamily::Icons, ICON_PX * s, false));
        let unfold = self.unfold.and_then(|(since, now)| Unfold::at(since, now, Edge::nearest(self.rect, self.start.1), s));
        let fresh = self.fresh.and_then(|(node, end, born)| Some((node + 1..end, Unfold::at(born?, self.unfold?.1, Edge::Top, s)?)));
        let (list, shown) = (self.list, unfold.map_or(self.rect, |u| u.shown(self.rect)));
        let seen = move |r: &Rect| r.bottom() >= list.y && r.y <= list.bottom();
        let count = self.rects.iter().filter(|r| seen(r)).count();
        let order = |k: usize| if unfold.is_some_and(|u| u.edge() == Edge::Bottom) { count.saturating_sub(k) } else { k };
        let mut p = Paint::new(g, t, s);
        let at = p.g.dl.instances.len();
        p.frame(self.rect);
        if let Some(u) = unfold {
            u.panel(&mut p.g.dl, at, self.rect);
        }
        let at = p.g.dl.instances.len();
        p.field(self.field, true);
        p.centered(Rect::new(self.field.x, self.field.y, self.field.h, self.field.h), G_SEARCH, icons, t.text_dim);
        let inner = self.field_inner();
        self.query.draw(p.g, t, inner, font, active, S::MenusSearchCommand.t(), s, Some(preedit));
        p.icon_button(self.close, G_CLOSE, FontStyle::new(FontFamily::Icons, 9.0 * s, false), Button::of(self.hover == Some(CtxHit::Close), false, true), t.text_dim);
        if let Some(u) = unfold {
            u.item(&mut p.g.dl, at, order(0), shown);
        }
        p.g.dl.set_clip(Some(list));
        let (mut n, mut kids) = (0, 0);
        for k in (0..self.rows.len()).filter(|&k| seen(&self.rects[k])) {
            let at = p.g.dl.instances.len();
            self.draw_row(&mut p, k, (font, icons));
            n += 1;
            if let (Some((nodes, f)), Row::Node { node, pinned: false, .. }) = (&fresh, &self.rows[k]) {
                if nodes.contains(node) {
                    f.item(&mut p.g.dl, at, kids, list);
                    kids += 1;
                }
            }
            if let Some(u) = unfold {
                u.item(&mut p.g.dl, at, order(n), shown);
            }
        }
        if self.rows.is_empty() {
            let at = p.g.dl.instances.len();
            let r = Rect::new(self.list.x, self.list.y, self.list.w, ROW_H * s);
            let y = baseline(p.g, r, font);
            p.g.text(r.x + ICON_W * s, y, S::MenusNoMatch.t(), font, t.text_dim, r.w);
            if let Some(u) = unfold {
                u.item(&mut p.g.dl, at, order(1), shown);
            }
        }
        p.g.dl.set_clip(None);
        self.draw_field_menu(&mut p, font);
        if self.arrows[0].w > 0.0 {
            let at = p.g.dl.instances.len();
            let thumb = self.thumb.inset(2.0 * s, 0.0);
            p.g.dl.round_rect(thumb, if self.grab.is_some() || self.hover == Some(CtxHit::ScrollThumb) { t.text_dim } else { t.text_dim.alpha(0x70) }, 3.0 * s);
            for (k, glyph) in [G_UP, G_DOWN].into_iter().enumerate() {
                p.icon_button(self.arrows[k], glyph, FontStyle::new(FontFamily::Icons, 8.0 * s, false), Button::of(self.hover == Some(CtxHit::Scroll(k == 1)), false, true), t.text_dim);
            }
            if let Some(u) = unfold {
                u.item(&mut p.g.dl, at, 0, shown);
            }
        }
    }

    fn draw_field_menu<B: TextBackend>(&self, p: &mut Paint<B>, font: FontStyle) {
        let Some(menu) = &self.field_menu else { return };
        let t = p.t;
        p.frame(menu.rect);
        for (k, (cmd, r, on)) in menu.rows.iter().enumerate() {
            if menu.hot == Some(k) {
                p.g.dl.round_rect(*r, t.hover, 4.0 * self.scale);
            }
            let label = FIELD_MENU.iter().find(|(c, _)| c == cmd).map_or("", |(_, s)| s.t());
            let y = baseline(p.g, *r, font);
            p.g.text(r.x + PAD * 2.0 * self.scale, y, label, font, if *on { t.text } else { t.text_dim }, r.w);
        }
    }

    pub(super) fn icon_slot(&self, k: usize) -> Rect {
        let (s, r) = (self.scale, self.rects[k]);
        let depth = match &self.rows[k] {
            Row::Node { node, path: false, .. } => self.nodes[*node].depth as f32,
            _ => 0.0,
        };
        let tuck = if depth > 0.0 { (ICON_W - IMAGE_PX) / 2.0 * s } else { 0.0 };
        Rect::new(r.x + depth * INDENT * s - tuck, r.y, ICON_W * s, r.h)
    }

    fn draw_row<B: TextBackend>(&self, p: &mut Paint<B>, k: usize, (font, icons): (FontStyle, FontStyle)) {
        let (s, t, r) = (self.scale, p.t, self.rects[k]);
        let row = &self.rows[k];
        let Row::Node { node, marks, pinned, path } = row else {
            p.separator(r.x + PAD * s, r.y + r.h / 2.0, r.w - 2.0 * PAD * s);
            return;
        };
        let n = &self.nodes[*node];
        if n.label.is_empty() && n.what == What::Header {
            let bar = Rect::new(r.x + ICON_W * s, r.y + (r.h * 0.36).round(), (r.w * 0.5 - (k % 3) as f32 * 24.0 * s).max(r.w * 0.2), (r.h * 0.28).round());
            p.g.dl.round_rect(bar, t.text_dim.alpha(0x24), 3.0 * s);
            return;
        }
        let lit = match self.glow_at {
            Some(now) => self.glow.alpha(*node, now),
            None => f32::from(u8::from(self.hot == Some(k))),
        };
        let hot = lit >= 0.5;
        if lit > 0.0 {
            p.g.dl.round_rect(r, super::widgets::faded(t.accent_fill, lit), 4.0 * s);
        }
        let off = matches!(n.what, What::Command { disabled: true, .. } | What::Header);
        let (ink, dim, mark) = match (hot, off) {
            (true, _) => (ink_on(t.accent_fill), ink_on(t.accent_fill), ink_on(t.accent_fill)),
            (false, true) => (t.text_dim.alpha(OFF), t.text_dim.alpha(OFF), t.accent.alpha(OFF)),
            (false, false) => (t.text, t.text_dim, t.accent),
        };
        let slot = self.icon_slot(k);
        let x0 = slot.x;
        let side = Rect::new(r.right() - SIDE_W * s, r.y, SIDE_W * s, r.h);
        let star = self.star_rect(k);
        let text_y = baseline(p.g, r, font);
        let keys_w = if n.keys.is_empty() { 0.0 } else { p.g.measure(&n.keys, font) };
        let star_on = *pinned || (hot && self.pinnable(k) && self.hover.is_some());
        let keys_right = if star_on { star.x } else { side.x };
        let label_x = x0 + ICON_W * s;
        let room = keys_right - label_x - if keys_w > 0.0 { keys_w + KEYS_GAP * s } else { 0.0 };
        let text = if *path { &n.path } else { &n.label };
        p.marked((label_x, text_y), text, marks, font, (ink, mark), room);
        if let What::Command { icon, image, .. } = &n.what {
            let shown = image.as_ref().is_some_and(|i| draw_image(p.g, i, slot, s, if off && !hot { OFF } else { 0xFF }));
            if !shown {
                p.centered(slot, if icon.is_empty() { "\u{E8A5}" } else { icon }, icons, if off && !hot { dim } else { ink });
            }
        } else if matches!(n.what, What::Submenu { .. }) {
            p.centered(slot, "\u{E8B7}", icons, ink);
        }
        if keys_w > 0.0 {
            p.g.text(keys_right - keys_w - PAD * s, text_y, &n.keys, font, dim, f32::INFINITY);
        }
        if star_on {
            let lit = match self.glow_at {
                Some(now) => self.star_glow.alpha(*node, now),
                None => f32::from(u8::from(self.hover == Some(CtxHit::Star(k)))),
            };
            if lit > 0.0 {
                let pad = (3.0 * s).round();
                let back = if hot { ink_on(t.accent_fill).alpha(0x38) } else { t.button_hover };
                p.g.dl.round_rect(Rect::new(star.x + 1.0, star.y + pad, star.w - 2.0, star.h - 2.0 * pad), super::widgets::faded(back, lit), 3.0 * s);
            }
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
}

pub(super) fn image_px(scale: f32) -> u32 {
    (IMAGE_PX * scale).round() as u32
}

pub(super) fn draw_image<B: TextBackend>(g: &mut Gfx<B>, image: &MenuImage, slot: Rect, scale: f32, alpha: u8) -> bool {
    let px = image_px(scale);
    let key = image.key(px);
    if let MenuImage::Pixels { bitmap, .. } = image {
        if !g.has_image(key) {
            g.add_image(key, bitmap.width, bitmap.height, bitmap.rgba.clone());
        }
    }
    let Some((w, h)) = g.image_size(key).map(|(w, h)| (w.max(1) as f32, h.max(1) as f32)) else { return false };
    let k = px as f32 / w.max(h);
    let (dw, dh) = ((w * k).round(), (h * k).round());
    g.image_alpha(key, Rect::new((slot.x + (slot.w - dw) / 2.0).round(), (slot.y + (slot.h - dh) / 2.0).round(), dw, dh), alpha)
}

fn font(scale: f32) -> FontStyle {
    FontStyle::new(FontFamily::Ui, FONT_PX * scale, false)
}

#[cfg(test)]
#[path = "../../test/ui/context.rs"]
mod tests;
