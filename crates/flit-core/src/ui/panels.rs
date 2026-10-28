use super::dock::Dock;
use super::filter::FilterRects;
use super::home::Home;
use super::menu::Menu;
use super::sidebar::PlaceCmd;
use super::widgets::{Button, Paint};
use super::{row_hit, App, Drag, DragEvent, Effect, Focus, Hit, Layout, Tab, WindowCmd, CAPTION_W, G_CLOSE, G_MAX, G_MIN, G_RESTORE, MAX_OPEN, TAB_H};
use crate::i18n::S;
use crate::input::{CursorIcon, Event};
use crate::location::Location;
use crate::provider::{ProviderEvent, TabId};
use crate::render::Rect;
use crate::sort::ViewSpec;
use crate::text::{Gfx, TextBackend};
use crate::theme::{Area, Palette, Theme};
use bumpalo::Bump;
use serde::{Deserialize, Serialize};
use std::time::Instant;

const MIN: f32 = 75.0;
const GRIP: f32 = 3.0;
const WHEEL_STEP: f32 = 24.0;
const BUTTON_W: f32 = 36.0;
const G_MENU: &str = "\u{E700}";
const G_QUICK: &str = "\u{E8A0}";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Shape {
    Leaf { tabs: u32, cur: u32 },
    Group { down: bool, kids: Vec<(Shape, f32)> },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PanelsSession {
    pub root: Shape,
    pub active: u32,
}

type Kids = Vec<(Node, f32)>;

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Node {
    Leaf(u32),
    Group { down: bool, kids: Kids },
}

#[derive(Default)]
pub(super) struct Panel {
    pub id: u32,
    pub tabs: Vec<Tab>,
    pub cur: usize,
    pub layout: Layout,
    filter: FilterRects,
    home: Home,
    pub rect: Rect,
    rows: Option<(u64, u32, u32)>,
}

#[derive(Clone, Debug)]
pub(super) struct Sep {
    pub rect: Rect,
    pub down: bool,
    path: Vec<usize>,
    at: usize,
    room: f32,
}

pub(super) struct Panels {
    pub list: Vec<Panel>,
    pub root: Node,
    pub here: u32,
    pub active: u32,
    next: u32,
    pub seps: Vec<Sep>,
    pub caption: [Rect; 3],
    pub menu: Rect,
    pub quick: Rect,
    pub left: f32,
    inset: f32,
    hover: Option<(u32, Hit)>,
}

impl Default for Panels {
    fn default() -> Self {
        Self { list: vec![Panel::default()], root: Node::Leaf(0), here: 0, active: 0, next: 1, seps: Vec::new(), caption: Default::default(), menu: Rect::default(), quick: Rect::default(), left: 0.0, inset: 0.0, hover: None }
    }
}

fn leaves(n: &Node, out: &mut Vec<u32>) {
    match n {
        Node::Leaf(id) => out.push(*id),
        Node::Group { kids, .. } => kids.iter().for_each(|(k, _)| leaves(k, out)),
    }
}

fn path_of(n: &Node, id: u32, path: &mut Vec<usize>) -> bool {
    match n {
        Node::Leaf(l) => *l == id,
        Node::Group { kids, .. } => kids.iter().enumerate().any(|(k, (kid, _))| {
            path.push(k);
            path_of(kid, id, path) || {
                path.pop();
                false
            }
        }),
    }
}

fn node_mut<'a>(n: &'a mut Node, path: &[usize]) -> &'a mut Node {
    match (path.split_first(), n) {
        (Some((&k, rest)), Node::Group { kids, .. }) => node_mut(&mut kids[k].0, rest),
        (_, n) => n,
    }
}

fn normalize(n: &mut Node) {
    let Node::Group { down, kids } = n else { return };
    kids.iter_mut().for_each(|(k, _)| normalize(k));
    let mut flat = Vec::with_capacity(kids.len());
    for (k, w) in std::mem::take(kids) {
        match k {
            Node::Group { down: d, kids: inner } if d == *down => {
                let total: f32 = inner.iter().map(|k| k.1).sum::<f32>().max(f32::EPSILON);
                flat.extend(inner.into_iter().map(|(k, x)| (k, w * x / total)));
            }
            k => flat.push((k, w)),
        }
    }
    *kids = flat;
    if kids.len() == 1 {
        *n = kids.pop().unwrap().0;
    }
}

fn insert(root: &mut Node, next_to: u32, id: u32, down: bool, before: bool) {
    let mut path = Vec::new();
    if !path_of(root, next_to, &mut path) {
        return;
    }
    if let Some((&k, parent)) = path.split_last() {
        if let Node::Group { down: d, kids } = node_mut(root, parent) {
            if *d == down {
                let half = kids[k].1 / 2.0;
                kids[k].1 = half;
                kids.insert(if before { k } else { k + 1 }, (Node::Leaf(id), half));
                return;
            }
        }
    }
    let leaf = node_mut(root, &path);
    let mut kids = vec![(Node::Leaf(next_to), 1.0), (Node::Leaf(id), 1.0)];
    if before {
        kids.reverse();
    }
    *leaf = Node::Group { down, kids };
}

fn add_column(root: &mut Node, id: u32, before: bool) {
    let old = std::mem::replace(root, Node::Leaf(id));
    let total = match &old {
        Node::Group { down: false, kids } => kids.iter().map(|k| k.1).sum::<f32>(),
        _ => 1.0,
    };
    let mut kids = vec![(old, total), (Node::Leaf(id), total / 2.0)];
    if before {
        kids.reverse();
    }
    *root = Node::Group { down: false, kids };
    normalize(root);
}

fn remove(root: &mut Node, id: u32) {
    let mut path = Vec::new();
    if !path_of(root, id, &mut path) {
        return;
    }
    if let Some((&k, parent)) = path.split_last() {
        if let Node::Group { kids, .. } = node_mut(root, parent) {
            kids.remove(k);
        }
    }
    normalize(root);
}

fn even(n: &mut Node, deep: bool) {
    if let Node::Group { kids, .. } = n {
        for (k, w) in kids.iter_mut() {
            *w = 1.0;
            if deep {
                even(k, true);
            }
        }
    }
}

fn min_len(n: &Node, down: bool, m: f32, gap: f32) -> f32 {
    match n {
        Node::Leaf(_) => m,
        Node::Group { down: d, kids } if *d == down => kids.iter().map(|(k, _)| min_len(k, down, m, gap)).sum::<f32>() + gap * (kids.len() - 1) as f32,
        Node::Group { kids, .. } => kids.iter().map(|(k, _)| min_len(k, down, m, gap)).fold(0.0, f32::max),
    }
}

fn place(n: &Node, r: Rect, gap: f32, path: &mut Vec<usize>, rects: &mut Vec<(u32, Rect)>, seps: &mut Vec<Sep>) {
    let Node::Group { down, kids } = n else {
        if let Node::Leaf(id) = n {
            rects.push((*id, r));
        }
        return;
    };
    let len = if *down { r.h } else { r.w };
    let room = (len - gap * (kids.len() - 1) as f32).max(0.0);
    let total: f32 = kids.iter().map(|k| k.1).sum::<f32>().max(f32::EPSILON);
    let (mut at, mut acc) = (0.0, 0.0);
    for (k, (kid, w)) in kids.iter().enumerate() {
        acc += w;
        let end = if k + 1 == kids.len() { room } else { (room * acc / total).round() };
        let off = at + gap * k as f32;
        let kr = if *down { Rect::new(r.x, r.y + off, r.w, end - at) } else { Rect::new(r.x + off, r.y, end - at, r.h) };
        path.push(k);
        place(kid, kr, gap, path, rects, seps);
        path.pop();
        if k + 1 < kids.len() {
            let rect = if *down { Rect::new(r.x, kr.bottom(), r.w, gap) } else { Rect::new(kr.right(), r.y, gap, r.h) };
            seps.push(Sep { rect, down: *down, path: path.clone(), at: k, room });
        }
        at = end;
    }
}

fn node_of(s: &Shape, next: &mut u32) -> Node {
    match s {
        Shape::Leaf { .. } => {
            *next += 1;
            Node::Leaf(*next - 1)
        }
        Shape::Group { down, kids } => Node::Group { down: *down, kids: kids.iter().map(|(k, w)| (node_of(k, next), if w.is_finite() && *w > 0.0 { *w } else { 1.0 })).collect() },
    }
}

fn sizes(s: &Shape, out: &mut Vec<(usize, usize)>) {
    match s {
        Shape::Leaf { tabs, cur } => out.push((*tabs as usize, *cur as usize)),
        Shape::Group { kids, .. } => kids.iter().for_each(|(k, _)| sizes(k, out)),
    }
}

fn event_tab(ev: &ProviderEvent) -> Option<TabId> {
    match ev {
        ProviderEvent::Changes { tab, .. } | ProviderEvent::Rescan { tab, .. } | ProviderEvent::DirSize { tab, .. } | ProviderEvent::OpDone { tab, .. } | ProviderEvent::Navigate { tab, .. } | ProviderEvent::Reveal { tab, .. } | ProviderEvent::Status { tab, .. } => Some(*tab),
        _ => None,
    }
}

impl App {
    pub fn panel_count(&self) -> usize {
        self.panels.list.len()
    }

    pub fn parked_tabs(&self) -> impl Iterator<Item = &Tab> {
        self.panels.list.iter().filter(|p| p.id != self.panels.here).filter_map(|p| p.tabs.get(p.cur))
    }

    pub fn parked_rows(&self) -> impl Iterator<Item = (u64, u32, u32)> + '_ {
        self.panels.list.iter().filter_map(|p| p.rows)
    }

    fn slot(&self, id: u32) -> Option<usize> {
        self.panels.list.iter().position(|p| p.id == id)
    }

    fn swap_slot(&mut self, i: usize) {
        let p = &mut self.panels.list[i];
        std::mem::swap(&mut self.tabs, &mut p.tabs);
        std::mem::swap(&mut self.cur, &mut p.cur);
        std::mem::swap(&mut self.layout, &mut p.layout);
        std::mem::swap(&mut self.nav.filter, &mut p.filter);
        std::mem::swap(&mut self.nav.home, &mut p.home);
    }

    fn enter(&mut self, id: u32) {
        let (Some(a), Some(b)) = (self.slot(self.panels.here), self.slot(id)) else { return };
        if a != b {
            self.swap_slot(a);
            self.swap_slot(b);
            self.panels.here = id;
        }
    }

    pub(super) fn visit<R>(&mut self, id: u32, f: impl FnOnce(&mut App) -> R) -> R {
        let back = self.panels.here;
        if id == back {
            return f(self);
        }
        let (mark, saved) = (self.effects.len(), (self.focus, self.hover, self.pressed));
        self.enter(id);
        let r = f(self);
        self.enter(back);
        (self.focus, self.hover, self.pressed) = saved;
        let tail: Vec<Effect> = self.effects.drain(mark.min(self.effects.len())..).collect();
        let retitle = tail.iter().any(|e| matches!(e, Effect::Title(_)));
        self.effects.extend(tail.into_iter().filter(|e| !matches!(e, Effect::Title(_))));
        if retitle && self.panels.here == self.panels.active {
            self.effects.push(Effect::Title(self.window_title()));
        }
        r
    }

    pub(super) fn parked_ids(&self) -> Vec<u32> {
        self.panels.list.iter().map(|p| p.id).filter(|&id| id != self.panels.here).collect()
    }

    pub(super) fn order(&self) -> Vec<u32> {
        let mut out = Vec::new();
        leaves(&self.panels.root, &mut out);
        out
    }

    pub(super) fn each_pane(&mut self, f: impl Fn(&mut App, usize)) {
        for k in 0..self.tabs.len() {
            f(self, k);
        }
        for id in self.parked_ids() {
            self.visit(id, |a| (0..a.tabs.len()).for_each(|k| f(a, k)));
        }
    }

    pub(super) fn parked_of(&self, tab: TabId) -> Option<u32> {
        self.panels.list.iter().find(|p| p.id != self.panels.here && p.tabs.iter().any(|t| t.id == tab)).map(|p| p.id)
    }

    pub(super) fn parked_of_event(&self, ev: &ProviderEvent) -> Option<u32> {
        event_tab(ev).and_then(|id| self.parked_of(id))
    }

    pub(super) fn poll_parked(&mut self) -> bool {
        self.parked_ids().into_iter().fold(false, |v, id| self.visit(id, |a| a.poll_here()) | v)
    }

    pub(super) fn wake_panels(&mut self) {
        if self.panels.here != self.panels.active {
            return;
        }
        let asleep: Vec<u32> = self.panels.list.iter().filter(|p| p.id != self.panels.here && p.tabs.get(p.cur).is_some_and(|t| t.gen == 0)).map(|p| p.id).collect();
        for id in asleep {
            self.visit(id, |a| a.load_tab(a.cur, None, Instant::now()));
        }
    }

    pub(super) fn step_parked(&mut self, dt: f32) -> bool {
        if self.search.active {
            return false;
        }
        self.parked_ids().into_iter().fold(false, |v, id| self.visit(id, |a| a.scroll_step(dt)) | v)
    }

    pub(super) fn fit_parked<B: TextBackend>(&mut self, g: &mut Gfx<B>, arena: &Bump) {
        if !self.search.active {
            for id in self.parked_ids() {
                self.visit(id, |a| a.fit_columns(g, arena));
            }
        }
    }

    pub(super) fn activate_panel(&mut self, id: u32) {
        if id == self.panels.active || self.panels.here != self.panels.active || self.slot(id).is_none() {
            return;
        }
        self.enter(id);
        self.panels.active = id;
        self.panel_changed();
    }

    fn panel_changed(&mut self) {
        self.end_marquee();
        self.end_autoscroll();
        self.hover = Hit::None;
        self.panels.hover = None;
        self.after_tab_change();
        self.layout();
    }

    pub(super) fn layout_panels(&mut self) {
        let (w, h) = self.size;
        let top = self.s(TAB_H);
        let side = self.sidebar_width();
        let (cw, bw, pad) = (self.s(CAPTION_W), self.s(BUTTON_W), self.s(4.0));
        let p = &mut self.panels;
        for (i, r) in p.caption.iter_mut().enumerate() {
            *r = Rect::new(w - (3 - i) as f32 * cw, 0.0, cw, top);
        }
        let above = side >= 2.0 * bw + 3.0 * pad;
        p.menu = Rect::new(pad, pad, bw, top - 2.0 * pad);
        p.quick = Rect::new(if above { side - bw - pad } else { p.menu.right() + pad / 2.0 }, pad, bw, top - 2.0 * pad);
        p.left = side;
        p.inset = if above { 0.0 } else { (p.quick.right() + pad - side).max(0.0) };
        self.layout_nav(top, h);
        let area = Rect::new(side, 0.0, (w - side).max(0.0), h);
        let (mut rects, mut seps) = (Vec::new(), Vec::new());
        if self.search.active {
            rects.push((self.panels.active, area));
        } else {
            place(&self.panels.root, area, self.s(1.0).max(1.0), &mut Vec::new(), &mut rects, &mut seps);
        }
        self.panels.seps = seps;
        for p in &mut self.panels.list {
            p.rect = rects.iter().find(|r| r.0 == p.id).map_or(Rect::default(), |r| r.1);
        }
        for id in self.parked_ids() {
            if let Some(r) = rects.iter().find(|r| r.0 == id).map(|r| r.1) {
                self.visit(id, |a| a.layout_panel(r));
            }
        }
        let r = self.panel_rect();
        self.layout_panel(r);
    }

    pub(super) fn panel_rect(&self) -> Rect {
        self.slot(self.panels.here).map_or(Rect::default(), |i| self.panels.list[i].rect)
    }

    pub(super) fn panel_edges(&self, r: Rect) -> (bool, f32) {
        let top = r.y <= 0.5;
        (top && r.right() >= self.size.0 - 0.5, if top && r.x <= self.panels.left + 0.5 { self.panels.inset } else { 0.0 })
    }

    pub(super) fn chrome_hit(&self, x: f32, y: f32) -> Option<Hit> {
        let p = &self.panels;
        if let Some(i) = p.caption.iter().position(|r| r.contains(x, y)) {
            return Some(Hit::Caption(i));
        }
        if p.menu.contains(x, y) {
            return Some(Hit::Menu);
        }
        if p.quick.contains(x, y) {
            return Some(Hit::QuickToggle);
        }
        None
    }

    pub(super) fn caption_hit(&self, x: f32, y: f32) -> Hit {
        self.panels.caption.iter().position(|r| r.contains(x, y)).map_or(Hit::None, Hit::Caption)
    }

    pub(super) fn in_title(&self, y: f32) -> bool {
        (0.0..self.s(TAB_H)).contains(&y)
    }

    pub(super) fn sep_hit(&self, x: f32, y: f32) -> Option<Hit> {
        let grip = self.s(GRIP);
        self.panels.seps.iter().position(|s| s.rect.inset(-grip, -grip).contains(x, y)).map(Hit::PanelSep)
    }

    pub(super) fn sep_cursor(&self, k: usize) -> CursorIcon {
        if self.panels.seps.get(k).is_some_and(|s| s.down) { CursorIcon::ResizeVertical } else { CursorIcon::ResizeHorizontal }
    }

    pub(super) fn parked_row_hit(&self, x: f32, y: f32) -> Option<(u32, Hit)> {
        self.panels.list.iter().filter(|p| p.id != self.panels.here && p.rect.contains(x, y) && p.layout.title.contains(x, y)).map(|p| (p.id, row_hit(&p.layout, x, y))).find(|h| h.1 != Hit::None)
    }

    pub(super) fn parked_hover(&mut self, x: f32, y: f32) {
        let h = if self.drag == Drag::None { self.parked_row_hit(x, y) } else { None };
        if h != self.panels.hover {
            self.panels.hover = h;
            self.dirty = true;
        }
    }

    fn panel_at(&self, x: f32, y: f32) -> Option<u32> {
        self.panels.list.iter().find(|p| p.rect.contains(x, y)).map(|p| p.id)
    }

    pub(super) fn panel_press(&mut self, x: f32, y: f32) {
        if self.is_renaming() || self.hit(x, y) != Hit::None {
            return;
        }
        if let Some(id) = self.panel_at(x, y).filter(|&id| id != self.panels.active) {
            self.activate_panel(id);
        }
    }

    pub(super) fn drag_panel(&mut self, ev: &DragEvent) {
        if let DragEvent::Enter { x, y, .. } | DragEvent::Over { x, y, .. } | DragEvent::Drop { x, y, .. } = ev {
            self.panel_press(*x, *y);
        }
    }

    pub(super) fn panel_wheel(&mut self, ev: &Event, dy: f32, now: Instant) -> bool {
        let Some((x, y)) = self.strip.pointer else { return false };
        if self.nav.palette.is_some() || self.jobs.modal() {
            return false;
        }
        let Some(id) = self.panel_at(x, y) else { return false };
        if self.mods.shift && !self.mods.ctrl && !self.mods.alt && self.panels.list.len() > 1 {
            return self.wheel_sep(id, dy);
        }
        if id == self.panels.active || self.hit(x, y) != Hit::None {
            return false;
        }
        let hover = self.hover;
        self.visit(id, |a| {
            a.hover = a.hit(x, y);
            a.on_event(ev, now);
        });
        self.hover = hover;
        true
    }

    fn wheel_sep(&mut self, id: u32, dy: f32) -> bool {
        let mut path = Vec::new();
        if !path_of(&self.panels.root, id, &mut path) {
            return false;
        }
        let Some((&k, parent)) = path.split_last() else { return false };
        let n = match node_mut(&mut self.panels.root, parent) {
            Node::Group { kids, .. } => kids.len(),
            Node::Leaf(_) => return false,
        };
        let at = if k + 1 < n { k } else { k.saturating_sub(1) };
        if let Some(s) = self.panels.seps.iter().position(|s| s.path == parent && s.at == at) {
            let first = self.sep_first(s);
            self.set_sep(s, first - dy * self.s(WHEEL_STEP));
        }
        true
    }

    fn sep_weights(&mut self, k: usize) -> Option<(&mut Kids, usize, f32, bool)> {
        let s = self.panels.seps.get(k)?.clone();
        match node_mut(&mut self.panels.root, &s.path) {
            Node::Group { kids, .. } if s.at + 1 < kids.len() => Some((kids, s.at, s.room, s.down)),
            _ => None,
        }
    }

    pub(super) fn sep_first(&mut self, k: usize) -> f32 {
        let Some((kids, at, room, _)) = self.sep_weights(k) else { return 0.0 };
        let total: f32 = kids.iter().map(|k| k.1).sum::<f32>().max(f32::EPSILON);
        room * kids[at].1 / total
    }

    pub(super) fn set_sep(&mut self, k: usize, first: f32) {
        let (m, gap) = (self.s(MIN), self.s(1.0).max(1.0));
        let Some((kids, at, room, down)) = self.sep_weights(k) else { return };
        let total: f32 = kids.iter().map(|k| k.1).sum::<f32>().max(f32::EPSILON);
        let pair_w = kids[at].1 + kids[at + 1].1;
        let pair = room * pair_w / total;
        let (lo, hi) = (min_len(&kids[at].0, down, m, gap), pair - min_len(&kids[at + 1].0, down, m, gap));
        if lo > hi || pair <= 0.0 {
            return;
        }
        kids[at].1 = pair_w * first.clamp(lo, hi) / pair;
        kids[at + 1].1 = pair_w - kids[at].1;
        self.strip.rev += 1;
        self.layout();
        self.dirty = true;
    }

    pub(super) fn press_sep(&mut self, k: usize, x: f32, y: f32, clicks: u8) {
        let first = self.sep_first(k);
        if clicks >= 2 {
            let pair = self.sep_weights(k).map_or(0.0, |(kids, at, room, _)| room * (kids[at].1 + kids[at + 1].1) / kids.iter().map(|k| k.1).sum::<f32>().max(f32::EPSILON));
            return self.set_sep(k, pair / 2.0);
        }
        let down = self.panels.seps.get(k).is_some_and(|s| s.down);
        self.drag = Drag::PanelSep { sep: k, start: if down { y } else { x }, first };
    }

    pub(super) fn drag_sep(&mut self, k: usize, x: f32, y: f32, start: f32, first: f32) {
        let down = self.panels.seps.get(k).is_some_and(|s| s.down);
        self.set_sep(k, first + if down { y } else { x } - start);
    }

    fn new_panel(&mut self, down: bool, mut folder: Location, now: Instant) {
        if self.search.active {
            self.close_search();
        }
        folder.canonicalize(&self.nav.sidebar.places.wsl);
        let t = self.tab();
        let mut tab = Tab::new(self.next_tab, folder, ViewSpec { show_hidden: t.view_spec.show_hidden, ..ViewSpec::default() });
        tab.set_folder_view(t.folder_view());
        self.next_tab += 1;
        let id = self.panels.next;
        self.panels.next += 1;
        self.panels.list.push(Panel { id, tabs: vec![tab], ..Default::default() });
        insert(&mut self.panels.root, self.panels.active, id, down, false);
        self.strip.rev += 1;
        self.activate_panel(id);
        if self.tab().gen == 0 {
            self.load_tab(self.cur, None, now);
        }
    }

    pub(super) fn dock_tab(&mut self, dock: Dock, copy: bool, now: Instant) {
        let src = self.panels.here;
        let alone = self.tabs.len() == 1;
        let onto_self = matches!(dock, Dock::Into(p) | Dock::Split { panel: p, .. } if p == src);
        if src != self.panels.active || (!copy && (dock == Dock::Into(src) || (alone && onto_self))) {
            return;
        }
        let tab = if copy {
            let t = self.tab();
            let loc = t.location().clone();
            let mut tab = Tab::new(self.next_tab, loc.clone(), t.view_spec);
            tab.set_folder_view(t.folder_view());
            tab.crumbs = loc.crumbs();
            self.next_tab += 1;
            tab
        } else {
            let tab = self.tabs.remove(self.cur);
            if !alone {
                self.cur = self.cur.min(self.tabs.len() - 1);
                self.after_tab_change();
            }
            tab
        };
        let id = match dock {
            Dock::Into(p) => p,
            _ => {
                let id = self.panels.next;
                self.panels.next += 1;
                self.panels.list.push(Panel { id, ..Default::default() });
                match dock {
                    Dock::Split { panel, down, before } => insert(&mut self.panels.root, panel, id, down, before),
                    Dock::Window { before } => add_column(&mut self.panels.root, id, before),
                    Dock::Between { left } => insert(&mut self.panels.root, left, id, false, false),
                    Dock::Into(_) => {}
                }
                id
            }
        };
        if id == src {
            self.tabs.push(tab);
            self.cur = self.tabs.len() - 1;
        } else if let Some(i) = self.slot(id) {
            let p = &mut self.panels.list[i];
            p.tabs.push(tab);
            p.cur = p.tabs.len() - 1;
            self.enter(id);
            self.panels.active = id;
        }
        if !copy && alone {
            self.panels.list.retain(|p| p.id != src);
            remove(&mut self.panels.root, src);
        }
        self.strip.rev += 1;
        self.panel_changed();
        if self.tab().gen == 0 {
            self.load_tab(self.cur, None, now);
        }
    }

    fn split_here(&mut self, down: bool, now: Instant) {
        let folder = self.current_location().clone();
        self.new_panel(down, folder, now);
    }

    fn open_in_split(&mut self, down: bool, now: Instant) {
        let mut folders = self.selected_folders();
        if folders.is_empty() {
            folders.push(self.current_location().clone());
        }
        for f in folders.into_iter().take(MAX_OPEN) {
            self.new_panel(down, f, now);
        }
    }

    pub(super) fn open_in_panel(&mut self, loc: Location, down: bool, now: Instant) {
        self.new_panel(down, loc, now);
    }

    pub(super) fn close_panel(&mut self) {
        if self.panels.list.len() == 1 {
            return self.effects.push(Effect::Window(WindowCmd::Close));
        }
        let gone = self.panels.active;
        let order = self.order();
        let pos = order.iter().position(|&id| id == gone).unwrap_or(0);
        let next = order[if pos > 0 { pos - 1 } else { 1 }];
        for mut t in std::mem::take(&mut self.tabs) {
            t.shutdown(&mut self.effects);
            self.remember_closed(t.location().clone());
        }
        self.enter(next);
        self.panels.active = next;
        self.panels.list.retain(|p| p.id != gone);
        remove(&mut self.panels.root, gone);
        self.strip.rev += 1;
        self.panel_changed();
    }

    fn cycle_panel(&mut self, back: bool) {
        let order = self.order();
        let n = order.len();
        let at = order.iter().position(|&id| id == self.panels.active).unwrap_or(0);
        self.activate_panel(order[if back { (at + n - 1) % n } else { (at + 1) % n }]);
    }

    fn select_toward(&mut self, down: bool, forward: bool) {
        let me = self.panel_rect();
        let span = |a: f32, b: f32, c: f32, d: f32| b.min(d) - a.max(c);
        let best = self
            .panels
            .list
            .iter()
            .filter(|p| p.id != self.panels.active && p.rect.w > 0.0)
            .filter_map(|p| {
                let r = p.rect;
                let (gap, overlap) = match (down, forward) {
                    (false, false) => (me.x - r.right(), span(me.y, me.bottom(), r.y, r.bottom())),
                    (false, true) => (r.x - me.right(), span(me.y, me.bottom(), r.y, r.bottom())),
                    (true, false) => (me.y - r.bottom(), span(me.x, me.right(), r.x, r.right())),
                    (true, true) => (r.y - me.bottom(), span(me.x, me.right(), r.x, r.right())),
                };
                (gap >= -2.0 && overlap > 0.0).then_some((gap, -overlap, p.id))
            })
            .min_by(|a, b| (a.0, a.1).partial_cmp(&(b.0, b.1)).unwrap_or(std::cmp::Ordering::Equal));
        if let Some((_, _, id)) = best {
            self.activate_panel(id);
        }
    }

    fn even_panels(&mut self, all: bool) {
        if all {
            even(&mut self.panels.root, true);
        } else {
            let mut path = Vec::new();
            if path_of(&self.panels.root, self.panels.active, &mut path) {
                path.pop();
                even(node_mut(&mut self.panels.root, &path), false);
            }
        }
        self.strip.rev += 1;
        self.layout();
        self.dirty = true;
    }

    pub(super) fn run_panel(&mut self, id: &str, now: Instant) {
        match id {
            "panel.split_right" => self.split_here(false, now),
            "panel.split_bottom" => self.split_here(true, now),
            "panel.open_right" => self.open_in_split(false, now),
            "panel.open_bottom" => self.open_in_split(true, now),
            "panel.next" if !self.is_renaming() => self.cycle_panel(false),
            "panel.previous" if !self.is_renaming() => self.cycle_panel(true),
            "panel.left" => self.select_toward(false, false),
            "panel.right" => self.select_toward(false, true),
            "panel.above" => self.select_toward(true, false),
            "panel.below" => self.select_toward(true, true),
            "panel.even_siblings" => self.even_panels(false),
            "panel.even_all" => self.even_panels(true),
            _ => {}
        }
    }

    pub(super) fn panel_available(&self, id: &str) -> bool {
        matches!(id, "panel.split_right" | "panel.split_bottom" | "panel.open_right" | "panel.open_bottom") || self.panels.list.len() > 1
    }

    pub(super) fn cycles_panels(&self) -> bool {
        self.focus == Focus::List && self.panels.list.len() > 1 && !self.is_renaming()
    }

    fn panel_tabs(&self, id: u32) -> (&[Tab], usize) {
        match self.slot(id) {
            Some(i) if id != self.panels.here => (&self.panels.list[i].tabs, self.panels.list[i].cur),
            _ => (&self.tabs, self.cur),
        }
    }

    fn shape(&self, n: &Node) -> Shape {
        match n {
            Node::Leaf(id) => {
                let (tabs, cur) = self.panel_tabs(*id);
                Shape::Leaf { tabs: tabs.len() as u32, cur: cur as u32 }
            }
            Node::Group { down, kids } => Shape::Group { down: *down, kids: kids.iter().map(|(k, w)| (self.shape(k), *w)).collect() },
        }
    }

    pub fn session(&self) -> super::WindowSession {
        let (mut tabs, mut trails, mut cur) = (Vec::new(), Vec::new(), 0);
        let order = self.order();
        for &id in &order {
            let (list, c) = self.panel_tabs(id);
            if id == self.panels.active {
                cur = tabs.len() + c;
            }
            tabs.extend(list.iter().map(|t| t.location().clone()));
            trails.extend(list.iter().map(|t| t.trail.clone()));
        }
        let active = order.iter().position(|&id| id == self.panels.active).unwrap_or(0) as u32;
        let panels = (order.len() > 1).then(|| PanelsSession { root: self.shape(&self.panels.root), active });
        super::WindowSession { tabs, cur, trails, panels }
    }

    pub(super) fn reset_panels(&mut self, layout: &super::options::SavedLayout, now: Instant) {
        for mut t in std::mem::take(&mut self.tabs) {
            t.shutdown(&mut self.effects);
        }
        for p in &mut self.panels.list {
            for mut t in std::mem::take(&mut p.tabs) {
                t.shutdown(&mut self.effects);
            }
        }
        let id = self.panels.next;
        self.panels.list = vec![Panel { id, ..Default::default() }];
        self.panels.root = Node::Leaf(id);
        (self.panels.here, self.panels.active, self.panels.next) = (id, id, id + 1);
        self.panels.seps.clear();
        let (view, hidden) = (self.prefs.defaults, self.show_hidden);
        for loc in &layout.tabs {
            self.add_tab(super::Route { folder: loc.clone(), select: None }, view, hidden, now);
        }
        self.cur = layout.cur.min(self.tabs.len().saturating_sub(1));
        self.after_tab_change();
    }

    pub fn restore_panels(&mut self, s: &PanelsSession) {
        let mut counts = Vec::new();
        sizes(&s.root, &mut counts);
        if self.panels.list.len() != 1 || counts.len() < 2 || counts.iter().any(|c| c.0 == 0) || counts.iter().map(|c| c.0).sum::<usize>() != self.tabs.len() {
            return;
        }
        let base = self.panels.next;
        let mut rest = std::mem::take(&mut self.tabs).into_iter();
        self.panels.list = counts.iter().enumerate().map(|(k, &(n, cur))| Panel { id: base + k as u32, tabs: rest.by_ref().take(n).collect(), cur: cur.min(n - 1), ..Default::default() }).collect();
        let mut next = base;
        self.panels.root = node_of(&s.root, &mut next);
        normalize(&mut self.panels.root);
        self.panels.next = next;
        let active = base + (s.active as usize).min(counts.len() - 1) as u32;
        (self.panels.here, self.panels.active) = (active, active);
        if let Some(i) = self.slot(active) {
            self.swap_slot(i);
        }
        self.strip.rev += 1;
        self.after_tab_change();
        self.layout();
    }

    pub(super) fn item_pane_items(&self, m: &mut Menu) {
        m.action("panel.open_right", S::ActionsPanelOpenRight.t());
        m.action("panel.open_bottom", S::ActionsPanelOpenBottom.t());
    }

    pub(super) fn here_pane_items(&self, m: &mut Menu) {
        m.action("panel.split_right", S::ActionsPanelSplitRight.t());
        m.action("panel.split_bottom", S::ActionsPanelSplitBottom.t());
        m.separator();
    }

    pub(super) fn place_pane_items(&self, m: &mut Menu, loc: &Location) {
        m.place(S::ActionsPanelOpenRight.t(), PlaceCmd::Pane(loc.clone(), false), false);
        m.place(S::ActionsPanelOpenBottom.t(), PlaceCmd::Pane(loc.clone(), true), false);
    }

    pub(super) fn draw_panels<B: TextBackend>(&mut self, g: &mut Gfx<B>, arena: &Bump, palette: &Palette) {
        let files = *palette.area(Area::Files);
        let saved = (self.hover, self.pressed, self.drag, self.active, self.focus, self.ops.dropping.take());
        (self.drag, self.active, self.focus) = (Drag::None, false, Focus::List);
        for id in self.parked_ids() {
            let hover = self.panels.hover.filter(|h| h.0 == id).map_or(Hit::None, |h| h.1);
            let rows = self.visit(id, |a| {
                (a.hover, a.pressed) = (hover, Hit::None);
                a.draw_body(g, arena, palette);
                let (r, top) = (a.panel_rect(), a.layout.toolbar.y);
                g.dl.rect(Rect::new(r.x, top, r.w, (r.bottom() - top).max(0.0)), files.bg.alpha(a.dim_alpha()));
                a.visible_rows()
            });
            if let Some(i) = self.slot(id) {
                self.panels.list[i].rows = Some(rows);
            }
        }
        (self.hover, self.pressed, self.drag, self.active, self.focus, self.ops.dropping) = saved;
        self.draw_body(g, arena, palette);
        if let Some(i) = self.slot(self.panels.here) {
            self.panels.list[i].rows = None;
        }
        let th = &palette.base;
        for (k, s) in self.panels.seps.iter().enumerate() {
            let hot = self.hover == Hit::PanelSep(k) || matches!(self.drag, Drag::PanelSep { sep, .. } if sep == k);
            let w = if hot { self.s(2.0).max(1.0) } else { s.rect.w.min(s.rect.h).max(1.0) };
            let line = if s.down { Rect::new(s.rect.x, (s.rect.y + (s.rect.h - w) / 2.0).round(), s.rect.w, w) } else { Rect::new((s.rect.x + (s.rect.w - w) / 2.0).round(), s.rect.y, w, s.rect.h) };
            g.dl.rect(line, if hot { th.accent } else { th.separator });
        }
    }

    pub(super) fn clear_parked_rows(&mut self) {
        self.panels.list.iter_mut().for_each(|p| p.rows = None);
    }

    pub(super) fn draw_chrome<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme) {
        let p = &self.panels;
        let (side, top) = (p.left, self.s(TAB_H));
        if side > 0.0 {
            g.dl.rect(Rect::new(0.0, 0.0, side, top), t.chrome);
        }
        let icons = self.icon_font(12.0);
        let palette_open = self.menu_open_from(Hit::Menu);
        let mut w = Paint::new(g, t, self.scale);
        for (r, glyph, hit, open) in [(p.menu, G_MENU, Hit::Menu, palette_open), (p.quick, G_QUICK, Hit::QuickToggle, false)] {
            w.icon_button(r, glyph, icons, Button::of(self.hover == hit, open, true), t.text_dim);
        }
        let caption_icons = self.icon_font(10.0);
        let max_glyph = if self.maximized { G_RESTORE } else { G_MAX };
        for (i, glyph) in [G_MIN, max_glyph, G_CLOSE].into_iter().enumerate() {
            let r = self.panels.caption[i];
            let idle = if self.active { t.text } else { t.text_dim };
            let hot = self.hover == Hit::Caption(i);
            let fill = match (hot, i) {
                (true, 2) => Some(t.close_hover),
                (true, _) => Some(t.teal),
                _ => None,
            };
            if let Some(c) = fill {
                w.g.dl.rect(r, c);
            }
            w.centered(r, glyph, caption_icons, fill.map_or(idle, crate::theme::ink_on));
        }
    }
}

#[cfg(test)]
#[path = "../../test/ui/panels.rs"]
mod tests;
