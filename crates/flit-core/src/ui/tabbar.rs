use super::{App, Drag, Effect, Hit, Route, Tab, G_DRIVE, G_FOLDER, MAX_ROUTES};
use crate::input::{Key, MouseButton};
use crate::location::Location;
use crate::path::PathKind;
use crate::render::Rect;
use crate::i18n::S;
use crate::text::{Gfx, TextBackend};
use crate::theme::Theme;
use super::widgets::Paint;
use crate::sort::ViewSpec;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::time::Instant;

pub const G_TAB_ACTIONS: &str = "\u{E70D}";
const G_COMPUTER: &str = "\u{E7F4}";
const G_LINUX: &str = "\u{E756}";
const G_BIN: &str = "\u{E74D}";
const DRAG_START: f32 = 6.0;
const TEAR_MARGIN: f32 = 24.0;

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
        let cur = cur.min(tabs.len().min(MAX_ROUTES) - 1);
        let mut live = self.tabs.pop();
        let (view, show_hidden) = live.as_ref().map(|t| (t.folder_view(), t.view_spec.show_hidden)).unwrap_or_default();
        for (i, loc) in tabs.iter().take(MAX_ROUTES).enumerate() {
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

    pub fn insert_tab(&mut self, moved: MovedTab, at: Option<(f32, f32)>, now: Instant) {
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
            self.drag = Drag::Tab { origin: self.cur, x, y, moved: false, torn: false };
        }
    }

    pub(super) fn drag_tab(&mut self, x: f32, y: f32) {
        let Drag::Tab { origin, x: x0, y: y0, moved, .. } = self.drag else { return };
        if !moved && (x - x0).abs().max((y - y0).abs()) < self.s(DRAG_START) {
            return;
        }
        let (m, title) = (self.s(TEAR_MARGIN), self.layout.title);
        let torn = x < 0.0 || y < 0.0 || x >= self.size.0 || y >= self.size.1;
        let on_strip = y >= title.y - m && y <= title.bottom() + m && x >= title.x && x < title.right();
        if torn || on_strip {
            self.docking = None;
            if on_strip {
                self.move_tab(self.tab_slot(x));
            }
        } else {
            let panels: Vec<(u32, Rect)> = self.panels.list.iter().map(|p| (p.id, p.rect)).collect();
            let area = Rect::new(self.panels.left, 0.0, (self.size.0 - self.panels.left).max(0.0), self.size.1);
            let zones = super::dock::zones(&panels, area, (x, y), self.scale);
            let hot = super::dock::zone_at(&zones, x, y);
            self.docking = Some(super::dock::Docking { zones, hot, at: (x, y) });
        }
        self.drag = Drag::Tab { origin, x: x0, y: y0, moved: true, torn };
        self.dirty = true;
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
        let Drag::Tab { origin, .. } = self.drag else { return false };
        if key != Key::Escape {
            return false;
        }
        self.move_tab(origin.min(self.tabs.len() - 1));
        self.docking = None;
        self.drag = Drag::None;
        self.pressed = Hit::None;
        self.dirty = true;
        true
    }

    pub(super) fn tear_off(&mut self, x: f32, y: f32) {
        self.detach_tab(Some((x, y)));
    }

    fn detach_tab(&mut self, at: Option<(f32, f32)>) {
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
                self.remember_closed(t.location().clone());
            }
        }
        self.after_tab_change();
    }

    pub(super) fn close_all_tabs(&mut self) {
        self.close_panel();
    }

    pub(super) fn strip_wheel(&mut self, dy: f32) -> bool {
        let Some((x, y)) = self.strip.pointer else { return false };
        if !self.layout.title.contains(x, y) || self.caption_hit(x, y) != Hit::None {
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
        if button != MouseButton::Right {
            return;
        }
        match self.hit(x, y) {
            Hit::Tab(i) | Hit::TabClose(i) => {
                self.select_tab(i);
                self.open_tab_menu();
            }
            Hit::Nav(i @ 0..=1) => self.history_menu(i),
            _ => {}
        }
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
