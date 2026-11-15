use super::{App, Drag, Effect, Hit, Route, Tab, G_DRIVE, G_FOLDER};
use crate::input::{Key, MouseButton};
use crate::location::Location;
use crate::path::PathKind;
use crate::render::{DrawList, Instance, Rect};
use crate::i18n::{P, S};
use crate::text::{Gfx, TextBackend};
use crate::theme::Theme;
use super::widgets::Paint;
use crate::sort::ViewSpec;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::time::{Duration, Instant};

pub const G_TAB_ACTIONS: &str = "\u{E70D}";
const G_COMPUTER: &str = "\u{E7F4}";
const G_LINUX: &str = "\u{E756}";
const G_BIN: &str = "\u{E74D}";
const DRAG_START: f32 = 6.0;
pub(super) const HOLD_DELAY: Duration = Duration::from_millis(350);
pub(super) const HOLD_REPEAT: Duration = Duration::from_millis(90);

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum Startup {
    NewTab,
    #[default]
    Continue,
    Folders(Vec<Location>),
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WindowSession {
    pub tabs: Vec<Location>,
    pub cur: usize,
    #[serde(skip)]
    pub trails: Vec<Location>,
    #[serde(skip)]
    pub panels: Option<super::PanelsSession>,
}

pub struct MovedTab(Box<Tab>);

impl MovedTab {
    pub fn location(&self) -> &Location {
        self.0.location()
    }
}

impl fmt::Debug for MovedTab {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MovedTab({})", self.location().display())
    }
}

#[derive(Default)]
pub(super) struct Strip {
    pub rev: u64,
    pub pointer: Option<(f32, f32)>,
    wheel: f32,
    newest: Option<crate::provider::TabId>,
    born: Vec<(crate::provider::TabId, Instant)>,
    tick: u64,
    lanes: Vec<Lane>,
    targets: Vec<(Part, Rect)>,
    hold: Option<Hold>,
    incoming: Option<(u32, usize)>,
    out: bool,
    grab: f32,
    floating: Option<Rect>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Held {
    Press,
    Tab,
    Files,
}

#[derive(Clone, Copy, Debug)]
struct Hold {
    held: Held,
    right: bool,
    at: (f32, f32),
    next: Instant,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Part {
    Tab(crate::provider::TabId),
    Plus,
    Arrow(bool),
}

#[derive(Default)]
struct Lane {
    panel: u32,
    tick: u64,
    first: Option<Part>,
    drawn: Vec<(Part, Rect, std::ops::Range<usize>)>,
    targets: Vec<Part>,
    shots: Vec<Instance>,
    slides: Vec<Slide>,
    ghosts: Vec<(Rect, Instant, Vec<Instance>)>,
}

struct Slide {
    part: Part,
    from: Rect,
    to: Option<Rect>,
    since: Instant,
}

fn mix(a: Rect, b: Rect, k: f32) -> Rect {
    if k >= 1.0 {
        return b;
    }
    let at = |p: f32, q: f32| (p + (q - p) * k).round();
    let (x, right) = (at(a.x, b.x), at(a.right(), b.right()));
    Rect::new(x, b.y, (right - x).max(0.0), b.h)
}

pub(super) const SLIDE_IN: std::time::Duration = std::time::Duration::from_millis(140);

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct StripScroll {
    first: usize,
    fit: usize,
    shown: Option<crate::provider::TabId>,
    follow: bool,
}

impl StripScroll {
    pub fn place(&mut self, (at, id): (usize, crate::provider::TabId), n: usize, fit: usize) -> usize {
        if self.follow || self.shown != Some(id) || self.fit != fit {
            self.first = self.first.min(at).max((at + 1).saturating_sub(fit));
        }
        self.first = self.first.min(n.saturating_sub(fit));
        (self.fit, self.shown) = (fit, Some(id));
        self.follow = (self.first..self.first + fit).contains(&at);
        self.first
    }

    pub fn hidden(&self, right: bool, n: usize) -> usize {
        if right { n.saturating_sub(self.first + self.fit) } else { self.first.min(n) }
    }

    fn scroll(&mut self, right: bool, n: usize) {
        self.first = if right { (self.first + 1).min(n.saturating_sub(self.fit)) } else { self.first.saturating_sub(1) };
        self.follow = false;
    }

    fn edge(&self, right: bool, n: usize) -> usize {
        if right { (self.first + self.fit).min(n).saturating_sub(1) } else { self.first.min(n.saturating_sub(1)) }
    }
}

impl Strip {
    pub fn arrivals(&mut self, ids: &[crate::provider::TabId], animate: bool, now: Instant) -> bool {
        if let (true, Some(newest)) = (animate, self.newest) {
            self.born.extend(ids.iter().filter(|&&id| id > newest).map(|&id| (id, now)));
        }
        self.newest = ids.iter().copied().chain(self.newest).max();
        self.born.retain(|b| animate && now.saturating_duration_since(b.1) < SLIDE_IN);
        self.born.iter().any(|b| ids.contains(&b.0))
    }

    pub fn arrival(&self, id: crate::provider::TabId, now: Instant) -> f32 {
        self.born.iter().find(|b| b.0 == id).map_or(1.0, |b| super::widgets::eased(b.1, now, SLIDE_IN))
    }

    pub fn begin(&mut self, panels: impl Iterator<Item = u32> + Clone, animate: bool) {
        self.tick += 1;
        self.lanes.retain(|l| animate && panels.clone().any(|p| p == l.panel));
    }

    pub fn drop_ghosts(&mut self) {
        self.lanes.iter_mut().for_each(|l| l.ghosts.clear());
    }

    fn settle(&mut self, panel: u32, now: Instant) -> usize {
        let k = self.lanes.iter().position(|l| l.panel == panel).unwrap_or_else(|| {
            self.lanes.push(Lane { panel, ..Lane::default() });
            self.lanes.len() - 1
        });
        let (lane, targets) = (&mut self.lanes[k], &self.targets);
        let was = lane.drawn.iter().map(|d| (d.0, d.1)).filter(|d| matches!(d.0, Part::Tab(_)));
        let will = targets.iter().copied().filter(|d| matches!(d.0, Part::Tab(_)));
        let first = will.clone().find(|t| t.1.w > 0.0).map(|t| t.0);
        let scrolled = std::mem::replace(&mut lane.first, first) != first;
        let changed = scrolled || !lane.targets.iter().copied().eq(will.map(|d| d.0));
        lane.targets = targets.iter().filter_map(|(p, _)| matches!(p, Part::Tab(_)).then_some(*p)).collect();
        if lane.tick + 1 >= self.tick && changed {
            for (part, r, range) in &lane.drawn {
                if matches!(part, Part::Tab(_)) && r.w > 0.0 && !range.is_empty() && !targets.iter().any(|t| t.0 == *part) {
                    lane.ghosts.push((*r, now, lane.shots[range.clone()].to_vec()));
                }
            }
            let shift = targets.iter().find_map(|&(p, to)| was.clone().find(|w| w.0 == p && w.1.w > 0.0 && to.w > 0.0).map(|w| w.1.x - to.x));
            for &(part, target) in targets {
                let Some(&(_, from, _)) = lane.drawn.iter().find(|d| d.0 == part) else { continue };
                lane.slides.retain(|s| s.part != part);
                let (from, to) = match (from.w > 0.0, target.w > 0.0, shift) {
                    (false, false, _) => continue,
                    (false, true, Some(d)) => (Rect::new(target.x + d, target.y, target.w, target.h), None),
                    (true, false, Some(d)) => (from, Some(Rect::new(from.x - d, from.y, from.w, from.h))),
                    _ => (from, None),
                };
                if to.is_some() || from != target {
                    lane.slides.push(Slide { part, from, to, since: now });
                }
            }
        }
        lane.tick = self.tick;
        lane.slides.retain(|s| now.saturating_duration_since(s.since) < SLIDE_IN);
        lane.ghosts.retain(|g| now.saturating_duration_since(g.1) < SLIDE_IN);
        lane.drawn.clear();
        lane.shots.clear();
        k
    }

    pub fn busy(&self, k: Option<usize>) -> bool {
        k.and_then(|k| self.lanes.get(k)).is_some_and(|l| !l.slides.is_empty() || !l.ghosts.is_empty())
    }

    pub fn at(&self, k: Option<usize>, part: Part, to: Rect, now: Instant) -> Rect {
        let Some(lane) = k.and_then(|k| self.lanes.get(k)) else { return to };
        lane.slides.iter().find(|s| s.part == part).map_or(to, |s| mix(s.from, s.to.unwrap_or(to), super::widgets::eased(s.since, now, SLIDE_IN)))
    }

    pub fn ghosts(&self, k: Option<usize>, dl: &mut DrawList, now: Instant) {
        let Some(lane) = k.and_then(|k| self.lanes.get(k)) else { return };
        for (r, since, shot) in &lane.ghosts {
            let from = dl.instances.len();
            dl.instances.extend_from_slice(shot);
            dl.grow(from, 1.0 - super::widgets::eased(*since, now, SLIDE_IN), (r.x, r.y + r.h / 2.0), 0.6);
        }
    }

    pub fn record(&mut self, k: Option<usize>, part: Part, r: Rect, shot: &[Instance]) {
        let Some(lane) = k.and_then(|k| self.lanes.get_mut(k)) else { return };
        let from = lane.shots.len();
        lane.shots.extend_from_slice(shot);
        lane.drawn.push((part, r, from..lane.shots.len()));
    }

    #[cfg(test)]
    pub fn drawn(&self, panel: u32, part: Part) -> Option<Rect> {
        self.lanes.iter().filter(|l| l.panel == panel).flat_map(|l| &l.drawn).find(|d| d.0 == part).map(|d| d.1)
    }

    #[cfg(test)]
    pub fn ghost_count(&self, panel: u32) -> usize {
        self.lanes.iter().filter(|l| l.panel == panel).map(|l| l.ghosts.len()).sum()
    }
}

pub fn tab_glyph(loc: &Location) -> &'static str {
    match loc.path().kind() {
        PathKind::Computer => G_COMPUTER,
        PathKind::RecycleBin => G_BIN,
        PathKind::Home => super::sidebar::G_HOME,
        PathKind::Unix if loc.is_root() => G_LINUX,
        _ if loc.is_root() => G_DRIVE,
        _ => G_FOLDER,
    }
}

pub(super) fn full_name(t: &Tab) -> String {
    match t.location().path().kind() {
        PathKind::Computer | PathKind::RecycleBin | PathKind::Home => t.title(),
        _ => t.location().display().into_owned(),
    }
}

impl App {
    pub fn window_title(&self) -> String {
        format!("{} — Flit", full_name(self.tab()))
    }

    pub fn session_rev(&self) -> u64 {
        self.strip.rev
    }

    pub fn restore_tabs(&mut self, tabs: &[Location], cur: usize) {
        if self.tabs.len() != 1 || tabs.is_empty() {
            return;
        }
        let cur = cur.min(tabs.len() - 1);
        let mut live = self.tabs.pop();
        let (view, show_hidden) = live.as_ref().map(|t| (t.folder_view(), t.view_spec.show_hidden)).unwrap_or_default();
        for (i, loc) in tabs.iter().enumerate() {
            match live.take_if(|_| i == cur) {
                Some(t) => self.tabs.push(t),
                None => {
                    let mut t = Tab::new(self.next_tab, loc.clone(), ViewSpec { show_hidden, ..ViewSpec::default() });
                    self.next_tab += 1;
                    t.set_folder_view(view);
                    t.crumbs = loc.crumbs();
                    self.tabs.push(t);
                }
            }
        }
        self.cur = cur;
        self.strip.rev += 1;
        self.effects.push(Effect::Title(self.window_title()));
        self.dirty = true;
    }

    pub fn restore_trails(&mut self, trails: &[Location]) {
        if trails.len() != self.tabs.len() {
            return;
        }
        for (t, trail) in self.tabs.iter_mut().zip(trails) {
            t.trail = trail.clone();
            t.follow_trail();
        }
        self.dirty = true;
    }

    pub fn tab_incoming(&mut self, at: Option<(f32, f32)>) -> bool {
        let spot = at.map(|(x, y)| {
            let id = self.panel_at(x, y).unwrap_or(self.panels.active);
            (id, self.visit(id, |a| a.layout.tabs.iter().filter(|(r, _)| x > r.x + r.w / 2.0).count()))
        });
        if spot == self.strip.incoming {
            return false;
        }
        self.strip.incoming = spot;
        self.dirty = true;
        true
    }

    pub(super) fn insert_mark(&self) -> Option<Rect> {
        let slot = if self.dragged_tab().is_some() { self.cur } else { self.strip.incoming.filter(|s| s.0 == self.panels.here)?.1 };
        let tabs = &self.layout.tabs;
        let x = tabs.get(slot).map(|t| t.0.x).or_else(|| tabs.last().map(|t| t.0.right()))?;
        let r = tabs.first()?.0;
        let w = self.s(2.0).max(2.0);
        Some(Rect::new((x - w / 2.0).round(), r.y + self.s(2.0), w, (r.h - self.s(4.0)).max(0.0)))
    }

    fn tab_out(&mut self, at: Option<(f32, f32)>) {
        if at.is_some() || self.strip.out {
            self.effects.push(Effect::TabOver(at));
        }
        self.strip.out = at.is_some();
    }

    pub fn insert_tab(&mut self, moved: MovedTab, at: Option<(f32, f32)>, now: Instant) {
        self.strip.incoming = None;
        if let Some(id) = at.and_then(|(x, y)| self.panel_at(x, y)) {
            self.activate_panel(id);
        }
        let mut t = *moved.0;
        t.id = self.next_tab;
        self.next_tab += 1;
        self.tabs.push(t);
        self.cur = self.tabs.len() - 1;
        if let Some((x, _)) = at {
            self.move_tab(self.tab_slot(x));
        }
        self.reload(self.cur, now);
        self.after_tab_change();
    }

    pub fn adopt_tab(&mut self, moved: MovedTab, now: Instant) {
        if self.tabs.len() == 1 {
            let mut old = self.tabs.remove(0);
            old.shutdown(&mut self.effects);
        }
        self.insert_tab(moved, None, now);
    }

    pub(super) fn wake_tab(&mut self) {
        self.strip.rev += 1;
        self.tip_hide();
        if self.tab().gen == 0 {
            self.load_tab(self.cur, None, Instant::now());
        }
        self.wake_panels();
    }

    pub(super) fn strip_lane(&mut self, now: Instant) -> Option<usize> {
        if !self.animations {
            return None;
        }
        let l = &self.layout;
        let targets = &mut self.strip.targets;
        targets.clear();
        targets.extend(self.tabs.iter().zip(&l.tabs).map(|(t, &(r, _))| (Part::Tab(t.id), r)));
        targets.push((Part::Plus, l.new_tab));
        if l.tab_arrows[0].w > 0.0 {
            targets.extend([(Part::Arrow(false), l.tab_arrows[0]), (Part::Arrow(true), l.tab_arrows[1])]);
        }
        let k = self.strip.settle(self.panels.here, now);
        let part = Part::Tab(self.tabs[self.cur].id);
        if matches!(self.drag, Drag::Tab { .. }) {
            self.strip.lanes[k].slides.retain(|s| s.part != part);
        } else if let Some(from) = self.strip.floating.take_if(|_| self.panels.here == self.panels.active) {
            let lane = &mut self.strip.lanes[k];
            lane.slides.retain(|s| s.part != part);
            lane.slides.push(Slide { part, from, to: None, since: now });
        }
        self.motion |= self.strip.busy(Some(k));
        Some(k)
    }

    pub(super) fn dragged_tab(&self) -> Option<Rect> {
        (self.panels.here == self.panels.active && matches!(self.drag, Drag::Tab { moved: true, torn: false, .. })).then_some(self.strip.floating).flatten()
    }

    pub(super) fn strip_clip(&self, tabs: bool) -> Rect {
        let l = &self.layout;
        let caption = self.panels.caption.iter().filter(|c| c.w > 0.0 && c.y < l.title.bottom() && c.bottom() > l.title.y && c.x > l.title.x).map(|c| c.x);
        let edge = caption.chain((l.actions.w > 0.0).then_some(l.actions.x)).fold(l.title.right(), f32::min);
        let [left, right] = l.tab_arrows;
        let (x, edge) = if tabs && left.w > 0.0 { (left.right(), edge.min(right.x)) } else { (l.title.x, edge) };
        Rect::new(x, l.title.y, (edge - x).max(0.0), l.title.h)
    }

    pub(super) fn scroll_tabs(&mut self, right: bool) {
        let n = self.tabs.len();
        self.layout.strip.scroll(right, n);
        self.layout();
        self.dirty = true;
    }

    fn arrow_at(&self, x: f32, y: f32) -> Option<bool> {
        self.layout.tab_arrows.iter().position(|r| r.w > 0.0 && r.contains(x, y)).map(|k| k == 1)
    }

    fn hold_live(&self, h: &Hold) -> bool {
        match h.held {
            Held::Press => self.pressed == Hit::TabScroll(h.right) && self.drag == Drag::None,
            Held::Tab => matches!(self.drag, Drag::Tab { .. }),
            Held::Files => self.ops.dropping.is_some(),
        }
    }

    pub(super) fn press_arrow(&mut self, right: bool, at: (f32, f32), held: Held, now: Instant) {
        self.step_arrow(right);
        self.strip.hold = Some(Hold { held, right, at, next: now + HOLD_DELAY });
        self.arm_timer(now, now + HOLD_DELAY);
    }

    fn step_arrow(&mut self, right: bool) {
        self.scroll_tabs(right);
        if matches!(self.drag, Drag::Tab { .. }) {
            let edge = self.layout.strip.edge(right, self.tabs.len());
            self.move_tab(edge);
            self.layout();
        }
    }

    pub(super) fn hover_arrow(&mut self, x: f32, y: f32, held: Held, now: Instant) -> bool {
        let Some(right) = self.arrow_at(x, y) else {
            self.strip.hold = None;
            return false;
        };
        let live = self.strip.hold.filter(|h| h.held == held && h.right == right && self.hold_live(h));
        match live {
            Some(h) => self.strip.hold = Some(Hold { at: (x, y), ..h }),
            None => self.press_arrow(right, (x, y), held, now),
        }
        true
    }

    pub(super) fn strip_drag(&mut self, ev: &super::DragEvent, now: Instant) {
        match ev {
            super::DragEvent::Enter { x, y, .. } | super::DragEvent::Over { x, y, .. } => {
                self.hover_arrow(*x, *y, Held::Files, now);
            }
            super::DragEvent::Leave | super::DragEvent::Drop { .. } => self.strip.hold = None,
        }
    }

    pub(super) fn strip_timer(&mut self, now: Instant) {
        let Some(h) = self.strip.hold.filter(|h| self.hold_live(h)) else {
            self.strip.hold = None;
            return;
        };
        if now + super::ops::TIMER_SLACK < h.next {
            return self.arm_timer(now, h.next);
        }
        let next = now + HOLD_REPEAT;
        let over = self.arrow_at(h.at.0, h.at.1) == Some(h.right);
        if !over && h.held != Held::Press {
            self.strip.hold = None;
            return;
        }
        if over {
            self.step_arrow(h.right);
        }
        self.strip.hold = Some(Hold { next, ..h });
        if !over || self.layout.strip.hidden(h.right, self.tabs.len()) > 0 {
            self.arm_timer(now, next);
        }
    }

    pub(super) fn tab_arrow_text(&self, right: bool) -> (&'static str, Option<String>) {
        let hidden = self.layout.strip.hidden(right, self.tabs.len());
        let name = if right { S::TooltipTabsRight } else { S::TooltipTabsLeft };
        (name.t(), (hidden > 0).then(|| crate::trn!(P::TooltipTabsHidden, hidden)))
    }

    fn tab_slot(&self, x: f32) -> usize {
        let before = self.layout.tabs.iter().enumerate().filter(|&(j, (r, _))| j != self.cur && x > r.x + r.w / 2.0).count();
        before.min(self.tabs.len() - 1)
    }

    pub(super) fn move_tab(&mut self, to: usize) {
        if to != self.cur {
            let t = self.tabs.remove(self.cur);
            self.tabs.insert(to, t);
            self.cur = to;
            self.strip.rev += 1;
            self.dirty = true;
        }
    }

    pub(super) fn press_tab(&mut self, i: usize, x: f32, y: f32, button: MouseButton) {
        self.select_tab(i);
        if button == MouseButton::Left {
            self.strip.grab = self.layout.tabs.get(self.cur).map_or(0.0, |t| x - t.0.x);
            self.strip.floating = None;
            self.drag = Drag::Tab { origin: self.cur, x, y, moved: false, torn: false };
        }
    }

    pub(super) fn drag_tab(&mut self, x: f32, y: f32) {
        let Drag::Tab { origin, x: x0, y: y0, moved, .. } = self.drag else { return };
        if !moved && (x - x0).abs().max((y - y0).abs()) < self.s(DRAG_START) {
            return;
        }
        let torn = x < 0.0 || y < 0.0 || x >= self.size.0 || y >= self.size.1;
        let on_strip = self.layout.title.contains(x, y);
        let arrow = on_strip && self.hover_arrow(x, y, Held::Tab, self.clock());
        self.strip.floating = if on_strip && !torn {
            self.layout.tabs.get(self.cur).map(|t| {
                let clip = self.strip_clip(true);
                Rect::new((x - self.strip.grab).clamp(clip.x, (clip.right() - t.0.w).max(clip.x)), t.0.y, t.0.w, t.0.h)
            })
        } else { None };
        self.tab_out(torn.then_some((x, y)));
        if !on_strip {
            self.strip.hold = None;
        }
        if torn || on_strip {
            self.docking = None;
            if on_strip && !arrow {
                self.move_tab(self.tab_slot(x));
            }
        } else {
            let panels: Vec<(u32, Rect)> = self.panels.list.iter().map(|p| (p.id, p.rect)).collect();
            let area = Rect::new(self.panels.left, 0.0, (self.size.0 - self.panels.left).max(0.0), self.size.1);
            let copy = self.mods.ctrl;
            let zones = super::dock::zones(&panels, &self.junctions(), area, (x, y), self.scale, |d| self.dock_moves(d, copy));
            let hot = super::dock::zone_at(&zones, x, y);
            self.docking = Some(super::dock::Docking { zones, hot, at: (x, y) });
        }
        self.drag = Drag::Tab { origin, x: x0, y: y0, moved: true, torn };
        self.dirty = true;
    }

    pub(super) fn redock(&mut self) {
        if let Some((x, y)) = self.docking.as_ref().map(|d| d.at) {
            self.drag_tab(x, y);
        }
    }

    pub(super) fn draw_tab_dock<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme) {
        let Some(d) = self.docking.as_ref() else { return };
        let sc = self.scale;
        let s = |v: f32| (v * sc).round();
        let mut p = Paint::new(g, t, sc);
        if let Some(z) = d.hot.and_then(|k| d.zones.get(k)) {
            p.g.dl.rect(z.area, t.teal.alpha(super::ops::DROP_FILL));
            p.g.dl.outline(z.area, t.teal, 0.0, s(1.0).max(1.0));
        }
        for (k, z) in d.zones.iter().enumerate() {
            let hot = d.hot == Some(k);
            p.g.dl.round_rect(z.button, if hot { t.teal.alpha(0x40) } else { t.popup }, s(4.0));
            p.dashed(z.button, if hot { t.teal } else { t.text_dim });
            for (r, on) in super::dock::scheme_rects(z.button, z.scheme, sc) {
                if on {
                    p.g.dl.round_rect(r, if hot { t.teal } else { t.accent }, s(2.0));
                } else {
                    p.g.dl.outline(r, t.text_dim, s(2.0), 1.0);
                }
            }
        }
        let tab = &self.tabs[self.cur];
        let (font, small) = (self.ui_font(12.5), self.ui_font(11.5));
        let label = tab.title();
        let caption = if self.mods.ctrl { S::TabbarDragCopy.t() } else { S::TabbarDragMove.t() };
        let w = (p.g.measure(&label, font) + s(70.0)).max(p.g.measure(caption, small) + s(24.0)).min(s(320.0));
        let (x, y) = d.at;
        let card = Rect::new((x + s(14.0)).min(self.size.0 - w).max(0.0), (y + s(14.0)).min(self.size.1 - s(58.0)).max(0.0), w, s(58.0));
        p.frame(card);
        let head = Rect::new(card.x, card.y + s(4.0), card.w, s(26.0));
        p.g.dl.round_rect(head.inset(s(4.0), 0.0), t.accent, s(4.0));
        let ink = crate::theme::ink_on(t.accent);
        let icon = Rect::new(head.x + s(10.0), head.y, s(16.0), head.h);
        p.centered(icon, tab_glyph(tab.location()), self.icon_font(12.0), ink);
        let m = p.g.metrics(font);
        let baseline = (head.y + (head.h + m.ascent - m.descent) / 2.0).round();
        p.g.text(icon.right() + s(6.0), baseline, &label, font, ink, head.right() - icon.right() - s(36.0));
        p.centered(Rect::new(head.right() - s(28.0), head.y, s(20.0), head.h), super::G_CLOSE, self.icon_font(8.0), ink);
        let m = p.g.metrics(small);
        let line = Rect::new(card.x, head.bottom() + s(2.0), card.w, card.bottom() - head.bottom() - s(4.0));
        let baseline = (line.y + (line.h + m.ascent - m.descent) / 2.0).round();
        p.g.text(card.x + s(12.0), baseline, caption, small, t.text_dim, card.w - s(24.0));
    }

    pub(super) fn strip_key(&mut self, key: Key) -> bool {
        if !matches!(self.drag, Drag::Tab { .. }) {
            return false;
        }
        if key == Key::Escape {
            self.cancel_tab_drag();
        }
        true
    }

    pub(super) fn cancel_tab_drag(&mut self) {
        let Drag::Tab { origin, .. } = self.drag else { return };
        self.tab_out(None);
        self.strip.hold = None;
        self.move_tab(origin.min(self.tabs.len() - 1));
        self.docking = None;
        self.drag = Drag::None;
        self.pressed = Hit::None;
        self.dirty = true;
    }

    pub(super) fn tear_off(&mut self, x: f32, y: f32) {
        self.detach_tab(Some((x, y)));
    }

    fn detach_tab(&mut self, at: Option<(f32, f32)>) {
        self.strip.floating = None;
        self.strip.out = false;
        let alone = self.tabs.len() == 1;
        let last = alone && self.panel_count() == 1;
        let mut t = if last {
            let loc = self.tab().location().clone();
            let mut stand_in = Tab::new(self.next_tab, loc.clone(), self.tab().view_spec);
            self.next_tab += 1;
            stand_in.crumbs = loc.crumbs();
            std::mem::replace(&mut self.tabs[0], stand_in)
        } else {
            self.tabs.remove(self.cur)
        };
        t.shutdown(&mut self.effects);
        if last {
            self.dirty = true;
        } else if alone {
            self.close_panel();
        } else {
            self.cur = self.cur.min(self.tabs.len() - 1);
            self.after_tab_change();
        }
        self.effects.push(Effect::MoveTab { tab: MovedTab(Box::new(t)), at, last });
    }

    pub(super) fn move_tab_to_window(&mut self) {
        if self.tabs.len() == 1 && self.panel_count() == 1 {
            return self.status(crate::i18n::S::TabbarOnlyTab.t());
        }
        self.detach_tab(None);
    }

    pub(super) fn close_tabs(&mut self, keep: impl Fn(usize, usize) -> bool) {
        let cur = self.cur;
        for (i, mut t) in std::mem::take(&mut self.tabs).into_iter().enumerate() {
            if keep(i, cur) {
                if i == cur {
                    self.cur = self.tabs.len();
                }
                self.tabs.push(t);
            } else {
                t.shutdown(&mut self.effects);
                self.remember_closed(t);
            }
        }
        self.after_tab_change();
    }

    pub(super) fn close_all_tabs(&mut self) {
        self.close_panel();
    }

    pub(super) fn strip_wheel(&mut self, dy: f32) -> bool {
        let Some((x, y)) = self.strip.pointer else { return false };
        if matches!(self.drag, Drag::Tab { .. }) || !self.layout.title.contains(x, y) || self.caption_hit(x, y) != Hit::None {
            return false;
        }
        self.strip.wheel += if self.tab_wheel_reversed { -dy } else { dy };
        let steps = self.strip.wheel.trunc();
        self.strip.wheel -= steps;
        let to = (self.cur as isize - steps as isize).clamp(0, self.tabs.len() as isize - 1);
        self.select_tab(to as usize);
        true
    }

    pub(super) fn strip_pointer(&mut self, x: f32, y: f32) {
        self.strip.pointer = Some((x, y));
        if let Some(h) = self.strip.hold.as_mut().filter(|h| h.held == Held::Press) {
            h.at = (x, y);
        }
    }

    pub(super) fn strip_mouse_down(&mut self, h: Hit, button: MouseButton, clicks: u8, now: Instant) -> bool {
        let plain = !self.mods.ctrl && !self.mods.shift && !self.mods.alt;
        match (button, h) {
            (MouseButton::Middle, Hit::Row(row)) if !self.autoscrolling() => self.middle_open(row, now),
            (MouseButton::Left, Hit::ListEmpty) if clicks == 2 && plain && self.double_click_up => {
                self.go_up(now);
                true
            }
            _ => false,
        }
    }

    fn middle_open(&mut self, row: usize, now: Instant) -> bool {
        let t = self.tab();
        let Some(i) = t.view.as_ref().and_then(|v| v.rows().get(row).copied().filter(|&i| v.listing().entries[i as usize].is_dir())) else { return false };
        if t.view_loc.is_recycle_bin() {
            return false;
        }
        let folder = t.entry_location(i);
        self.open_in_tabs(vec![Route { folder, select: None }], now);
        true
    }

    pub(super) fn strip_mouse_up(&mut self, x: f32, y: f32, button: MouseButton) {
        self.addr.press = None;
        if button == MouseButton::Left && matches!(self.drag, Drag::Tab { .. }) {
            self.strip.hold = None;
        }
        if button != MouseButton::Right {
            return;
        }
        match self.hit(x, y) {
            Hit::Tab(i) | Hit::TabClose(i) if !self.menu_open_from(Hit::Tab(i)) => {
                self.select_tab(i);
                self.open_tab_menu();
            }
            Hit::Nav(i @ 0..=1) => self.history_menu(i),
            Hit::None if self.in_title(y) && !self.layout.new_tab.contains(x, y) => self.open_strip_menu(x, y),
            _ => {}
        }
    }

    pub(super) fn open_strip_menu(&mut self, x: f32, y: f32) {
        let menu = self.collect(|_, m| {
            m.action("tab.new", S::ActionsTabNew.t());
            m.action("tab.reopen", S::MenuReopenClosedTab.t());
        });
        self.flit_menu(x, y, menu);
    }

    pub(super) fn open_tab_menu(&mut self) {
        let Some(&(r, _)) = self.layout.tabs.get(self.cur) else { return };
        let menu = self.build_tab_menu();
        self.button_menu(Hit::Tab(self.cur), r.x, r.bottom(), menu);
    }

    pub(super) fn open_many(&mut self, now: Instant) {
        let t = self.tab();
        let Some(v) = &t.view else { return };
        let (dirs, files): (Vec<u32>, Vec<u32>) = v.selected().partition(|&i| v.listing().entries[i as usize].is_dir() || self.browses_as_folder(v.listing().name(i)));
        let routes: Vec<Route> = dirs.into_iter().map(|i| Route { folder: t.entry_location(i), select: None }).collect();
        for i in files.into_iter().take(super::MAX_OPEN) {
            self.open_entry(i, now);
        }
        self.open_in_tabs(routes, now);
    }
}

#[cfg(test)]
#[path = "../../test/ui/tabbar.rs"]
mod tests;
