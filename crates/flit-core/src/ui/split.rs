use crate::i18n::S;
use super::filter::FilterRects;
use super::home::Home;
use super::menu::Menu;
use super::sidebar::PlaceCmd;
use super::{App, Drag, DragEvent, Effect, Focus, Hit, Layout, Route, Tab};
use crate::input::{CursorIcon, Event, Key, Mods};
use crate::location::Location;
use crate::provider::{ProviderEvent, TabId};
use crate::render::Rect;
use crate::sort::ViewSpec;
use crate::text::{Gfx, TextBackend};
use crate::theme::Theme;
use bumpalo::Bump;
use serde::{Deserialize, Serialize};
use std::time::Instant;

const NARROW: f32 = 750.0;
const GAP: f32 = 5.0;
const GRIP: f32 = 3.0;
const MIN_SHARE: f32 = 0.15;
const MARK_H: f32 = 2.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SplitDir {
    #[default]
    Vertical,
    Horizontal,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SplitPrefs {
    pub dual: bool,
    pub dir: SplitDir,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SplitSession {
    pub other: Location,
    pub dir: SplitDir,
    pub ratio: f32,
    pub second: bool,
}

pub(super) struct Split {
    pub other: Tab,
    pub dir: SplitDir,
    pub ratio: f32,
    pub second: bool,
}

#[derive(Default)]
pub(super) struct Alt {
    pub layout: Layout,
    pub filter: FilterRects,
    pub home: Home,
    rows: Option<(u64, u32, u32)>,
}

fn share(r: f32) -> f32 {
    if r.is_finite() {
        r.clamp(MIN_SHARE, 1.0 - MIN_SHARE)
    } else {
        0.5
    }
}

fn event_tab(ev: &ProviderEvent) -> Option<TabId> {
    match ev {
        ProviderEvent::Changes { tab, .. } | ProviderEvent::Rescan { tab, .. } | ProviderEvent::DirSize { tab, .. } | ProviderEvent::OpDone { tab, .. } | ProviderEvent::Navigate { tab, .. } | ProviderEvent::Status { tab, .. } => Some(*tab),
        _ => None,
    }
}

impl App {
    pub fn other_pane(&self) -> Option<&Tab> {
        self.tab().split.as_deref().map(|s| &s.other)
    }

    pub fn other_visible_rows(&self) -> Option<(u64, u32, u32)> {
        self.alt.rows.filter(|_| self.split_shown())
    }

    pub(super) fn wide(&self) -> bool {
        self.size.0 > NARROW * self.scale
    }

    pub(super) fn split_shown(&self) -> bool {
        self.tab().split.is_some() && !self.search.active && self.wide()
    }

    fn flip_at(&mut self, k: usize) {
        let t = &mut self.tabs[k];
        let Some(mut s) = t.split.take() else { return };
        std::mem::swap(t, &mut s.other);
        s.second = !s.second;
        t.split = Some(s);
    }

    fn swap_alt(&mut self) {
        std::mem::swap(&mut self.layout, &mut self.alt.layout);
        std::mem::swap(&mut self.nav.filter, &mut self.alt.filter);
        std::mem::swap(&mut self.nav.home, &mut self.alt.home);
    }

    pub(super) fn on_other_at<R>(&mut self, k: usize, f: impl FnOnce(&mut App) -> R) -> R {
        let (front, mark) = (k == self.cur, self.effects.len());
        self.flip_at(k);
        if front {
            self.swap_alt();
        }
        let id = self.tabs[k].id;
        let r = f(self);
        if let Some(k) = self.tabs.iter().position(|t| t.id == id) {
            self.flip_at(k);
        }
        if front {
            self.swap_alt();
            let tail: Vec<Effect> = self.effects.drain(mark.min(self.effects.len())..).collect();
            let retitle = tail.iter().any(|e| matches!(e, Effect::Title(_)));
            self.effects.extend(tail.into_iter().filter(|e| !matches!(e, Effect::Title(_))));
            if retitle {
                self.effects.push(Effect::Title(self.window_title()));
            }
        }
        r
    }

    pub(super) fn each_pane(&mut self, f: impl Fn(&mut App, usize)) {
        for k in 0..self.tabs.len() {
            f(self, k);
            if self.tabs[k].split.is_some() {
                self.on_other_at(k, |a| f(a, k));
            }
        }
    }

    pub(super) fn inactive_of(&self, id: TabId) -> Option<usize> {
        self.tabs.iter().position(|t| t.split.as_ref().is_some_and(|s| s.other.id == id))
    }

    pub(super) fn inactive_of_event(&self, ev: &ProviderEvent) -> Option<usize> {
        event_tab(ev).and_then(|id| self.inactive_of(id))
    }

    pub(super) fn split_areas(&mut self, area: Rect) -> (Rect, Option<Rect>) {
        self.layout.splitter = Rect::default();
        self.layout.content = area;
        if !self.split_shown() {
            return (area, None);
        }
        let Some((dir, ratio, second)) = self.tab().split.as_deref().map(|s| (s.dir, share(s.ratio), s.second)) else { return (area, None) };
        let gap = self.s(GAP);
        let (first, rest, bar) = match dir {
            SplitDir::Vertical => {
                let w = ((area.w - gap) * ratio).round().max(0.0);
                (Rect::new(area.x, area.y, w, area.h), Rect::new(area.x + w + gap, area.y, (area.w - w - gap).max(0.0), area.h), Rect::new(area.x + w, area.y, gap, area.h))
            }
            SplitDir::Horizontal => {
                let h = ((area.h - gap) * ratio).round().max(0.0);
                (Rect::new(area.x, area.y, area.w, h), Rect::new(area.x, area.y + h + gap, area.w, (area.h - h - gap).max(0.0)), Rect::new(area.x, area.y + h, area.w, gap))
            }
        };
        self.layout.splitter = bar;
        if second {
            (rest, Some(first))
        } else {
            (first, Some(rest))
        }
    }

    pub(super) fn layout_other(&mut self, area: Rect) {
        let k = self.cur;
        self.alt.layout = self.layout.clone();
        self.flip_at(k);
        self.swap_alt();
        self.layout_pane(area);
        self.flip_at(k);
        self.swap_alt();
    }

    pub(super) fn on_splitter(&self, x: f32, y: f32) -> bool {
        let grip = self.s(GRIP);
        self.split_shown() && self.layout.splitter.inset(-grip, -grip).contains(x, y)
    }

    fn dir(&self) -> Option<SplitDir> {
        self.tab().split.as_deref().map(|s| s.dir)
    }

    pub(super) fn split_cursor(&self) -> CursorIcon {
        match self.dir() {
            Some(SplitDir::Horizontal) => CursorIcon::ResizeVertical,
            _ => CursorIcon::ResizeHorizontal,
        }
    }

    pub(super) fn begin_pane_split(&mut self, x: f32, y: f32, clicks: u8) {
        let Some(s) = self.tab_mut().split.as_deref_mut() else { return };
        if clicks >= 2 {
            s.ratio = 0.5;
            self.strip.rev += 1;
            return self.layout();
        }
        let start = if s.dir == SplitDir::Vertical { x } else { y };
        self.drag = Drag::PaneSplit { start, ratio: share(s.ratio) };
    }

    pub(super) fn drag_pane_split(&mut self, x: f32, y: f32, start: f32, ratio: f32) {
        let (c, gap) = (self.layout.content, self.s(GAP));
        let Some(s) = self.tab_mut().split.as_deref_mut() else { return };
        let (at, len) = if s.dir == SplitDir::Vertical { (x, c.w) } else { (y, c.h) };
        s.ratio = share(ratio + (at - start) / (len - gap).max(1.0));
        self.layout();
        self.dirty = true;
    }

    fn over_other(&self, x: f32, y: f32) -> bool {
        self.split_shown() && self.alt.layout.pane.contains(x, y) && self.hit(x, y) == Hit::None
    }

    pub(super) fn pane_press(&mut self, x: f32, y: f32) {
        if !self.is_renaming() && self.over_other(x, y) {
            self.focus_other_pane();
        }
    }

    pub(super) fn drag_pane(&mut self, ev: &DragEvent) {
        if let DragEvent::Enter { x, y, .. } | DragEvent::Over { x, y, .. } | DragEvent::Drop { x, y, .. } = ev {
            self.pane_press(*x, *y);
        }
    }

    pub(super) fn wheel_other(&mut self, ev: &Event, now: Instant) -> bool {
        let Some((x, y)) = self.strip.pointer.filter(|&(x, y)| self.over_other(x, y)) else { return false };
        let (hover, k) = (self.hover, self.cur);
        self.on_other_at(k, |a| {
            a.hover = a.hit(x, y);
            a.on_event(ev, now);
        });
        self.hover = hover;
        true
    }

    pub(super) fn focus_other_pane(&mut self) {
        if self.tab().split.is_none() {
            return;
        }
        self.flip_at(self.cur);
        self.swap_alt();
        self.pane_changed();
    }

    fn pane_changed(&mut self) {
        self.end_marquee();
        self.end_autoscroll();
        self.hover = Hit::None;
        self.after_tab_change();
        self.layout();
    }

    pub(super) fn step_other(&mut self, dt: f32) -> bool {
        if !self.split_shown() {
            return false;
        }
        let k = self.cur;
        self.on_other_at(k, |a| a.scroll_step(dt))
    }

    pub(super) fn fit_other<B: TextBackend>(&mut self, g: &mut Gfx<B>, arena: &Bump) {
        if self.split_shown() {
            let k = self.cur;
            self.on_other_at(k, |a| a.fit_columns(g, arena));
        }
    }

    pub(super) fn draw_other<B: TextBackend>(&mut self, g: &mut Gfx<B>, arena: &Bump, th: &Theme) {
        if !self.split_shown() {
            self.alt.rows = None;
            return;
        }
        let saved = (self.hover, self.pressed, self.drag, self.active, self.focus, self.ops.dropping.take());
        (self.hover, self.pressed, self.drag, self.active, self.focus) = (Hit::None, Hit::None, Drag::None, false, Focus::List);
        let k = self.cur;
        let rows = self.on_other_at(k, |a| {
            if a.on_home() {
                a.draw_home(g, th);
            } else {
                a.draw_list(g, arena, th);
                a.draw_panes(g, th);
                a.draw_header(g, th);
            }
            a.draw_filter(g, th);
            a.visible_rows()
        });
        self.alt.rows = Some(rows);
        (self.hover, self.pressed, self.drag, self.active, self.focus, self.ops.dropping) = saved;
    }

    pub(super) fn draw_split<B: TextBackend>(&mut self, g: &mut Gfx<B>, th: &Theme) {
        if !self.split_shown() {
            return;
        }
        let (bar, pane) = (self.layout.splitter, self.layout.pane);
        let hot = self.hover == Hit::PaneSplit || matches!(self.drag, Drag::PaneSplit { .. });
        let w = if hot { self.s(2.0).max(1.0) } else { 1.0 };
        let line = match self.dir() {
            Some(SplitDir::Horizontal) => Rect::new(bar.x, (bar.y + (bar.h - w) / 2.0).round(), bar.w, w),
            _ => Rect::new((bar.x + (bar.w - w) / 2.0).round(), bar.y, w, bar.h),
        };
        g.dl.rect(bar, th.bg);
        g.dl.rect(line, if hot { th.accent } else { th.separator });
        g.dl.rect(Rect::new(pane.x, pane.y, pane.w, self.s(MARK_H)), if self.active { th.accent } else { th.separator });
    }

    pub(super) fn wake_other(&mut self) {
        let k = self.cur;
        if self.tabs[k].split.as_ref().is_some_and(|s| s.other.gen == 0) {
            self.on_other_at(k, |a| a.load_tab(k, None, Instant::now()));
        }
    }

    pub(super) fn dual_new_tab(&mut self, now: Instant) {
        if self.split_prefs.dual {
            let folder = self.tab().location().clone();
            self.split_open(self.split_prefs.dir, folder, false, now);
        }
    }

    fn pane_tab(&mut self, folder: Location) -> Tab {
        let t = self.tab();
        let mut other = Tab::new(self.next_tab, folder, ViewSpec { show_hidden: t.view_spec.show_hidden, ..ViewSpec::default() });
        other.set_folder_view(t.folder_view());
        self.next_tab += 1;
        other
    }

    fn split_open(&mut self, dir: SplitDir, folder: Location, focus: bool, now: Instant) {
        let k = self.cur;
        let other = self.pane_tab(folder);
        if let Some(mut old) = self.tabs[k].split.replace(Box::new(Split { other, dir, ratio: 0.5, second: false })) {
            old.other.shutdown(&mut self.effects);
        }
        self.on_other_at(k, |a| a.load_tab(k, None, now));
        if focus {
            self.flip_at(k);
            self.swap_alt();
        }
        self.strip.rev += 1;
    }

    pub(super) fn open_in_pane(&mut self, mut folder: Location, dir: Option<SplitDir>, now: Instant) {
        if !self.wide() {
            return self.status(S::SplitTooNarrow.t());
        }
        if self.search.active {
            self.close_search();
        }
        folder.canonicalize(&self.nav.sidebar.places.wsl);
        let k = self.cur;
        match self.tabs[k].split.as_deref_mut() {
            Some(s) => {
                s.dir = dir.unwrap_or(s.dir);
                self.flip_at(k);
                self.swap_alt();
                self.route_here(Route { folder, select: None }, now);
            }
            None => self.split_open(dir.unwrap_or(self.split_prefs.dir), folder, true, now),
        }
        self.pane_changed();
    }

    fn close_pane(&mut self) {
        let k = self.cur;
        let Some(s) = self.tabs[k].split.take() else { return self.status(S::SplitOnePane.t()) };
        let mut gone = std::mem::replace(&mut self.tabs[k], s.other);
        gone.shutdown(&mut self.effects);
        self.swap_alt();
        self.pane_changed();
    }

    fn toggle_dual(&mut self, now: Instant) {
        match self.tab_mut().split.take() {
            Some(mut s) => {
                s.other.shutdown(&mut self.effects);
                self.pane_changed();
            }
            None => self.split_here(self.split_prefs.dir, now),
        }
    }

    fn split_here(&mut self, dir: SplitDir, now: Instant) {
        if self.tab().split.is_some() {
            return self.arrange(dir);
        }
        let folder = self.current_location().clone();
        self.open_in_pane(folder, Some(dir), now);
    }

    fn arrange(&mut self, dir: SplitDir) {
        let Some(s) = self.tab_mut().split.as_deref_mut() else { return self.status(S::SplitOnePane.t()) };
        s.dir = dir;
        self.strip.rev += 1;
        self.layout();
        self.dirty = true;
    }

    fn focus_or_arrow(&mut self, now: Instant) {
        if self.split_shown() {
            return self.focus_other_pane();
        }
        self.arrow_key(Key::Right, Mods { ctrl: true, shift: true, alt: false }, now);
    }

    fn open_selection_in_pane(&mut self, dir: Option<SplitDir>, now: Instant) {
        let folder = self.selected_folders().into_iter().next().unwrap_or_else(|| self.current_location().clone());
        self.open_in_pane(folder, dir, now);
    }

    fn open_here_in_pane(&mut self, now: Instant) {
        let folder = self.current_location().clone();
        self.open_in_pane(folder, None, now);
    }

    fn toggle_dual_default(&mut self) {
        self.split_prefs.dual = !self.split_prefs.dual;
        self.status(if self.split_prefs.dual { S::SplitDualOn.t() } else { S::SplitDualOff.t() });
    }

    fn switch_default_dir(&mut self) {
        let p = &mut self.split_prefs;
        p.dir = if p.dir == SplitDir::Vertical { SplitDir::Horizontal } else { SplitDir::Vertical };
        self.status(if self.split_prefs.dir == SplitDir::Vertical { S::SplitSideBySide.t() } else { S::SplitStacked.t() });
    }

    pub(super) fn run_pane(&mut self, id: &str, now: Instant) {
        match id {
            "pane.toggle" => self.toggle_dual(now),
            "pane.split_vertical" => self.split_here(SplitDir::Vertical, now),
            "pane.split_horizontal" => self.split_here(SplitDir::Horizontal, now),
            "pane.arrange_vertical" => self.arrange(SplitDir::Vertical),
            "pane.arrange_horizontal" => self.arrange(SplitDir::Horizontal),
            "pane.close" => self.close_pane(),
            "pane.focus" => self.focus_or_arrow(now),
            "pane.open_other" => self.open_selection_in_pane(None, now),
            "pane.here_other" => self.open_here_in_pane(now),
            "pane.new_vertical" => self.open_selection_in_pane(Some(SplitDir::Vertical), now),
            "pane.new_horizontal" => self.open_selection_in_pane(Some(SplitDir::Horizontal), now),
            "pane.dual_default" => self.toggle_dual_default(),
            "pane.default_direction" => self.switch_default_dir(),
            _ => {}
        }
    }

    pub(super) fn pane_available(&self, id: &str) -> bool {
        match id {
            "pane.arrange_vertical" | "pane.arrange_horizontal" | "pane.close" | "pane.focus" => self.tab().split.is_some(),
            _ => true,
        }
    }

    pub(super) fn split_sessions(&self) -> Vec<Option<SplitSession>> {
        self.tabs.iter().map(|t| t.split.as_deref().map(|s| SplitSession { other: s.other.location().clone(), dir: s.dir, ratio: s.ratio, second: s.second })).collect()
    }

    pub fn restore_splits(&mut self, splits: &[Option<SplitSession>]) {
        if splits.len() != self.tabs.len() {
            return;
        }
        for (k, saved) in splits.iter().enumerate() {
            if let Some(mut old) = self.tabs[k].split.take() {
                old.other.shutdown(&mut self.effects);
            }
            let Some(s) = saved else { continue };
            let t = &self.tabs[k];
            let mut other = Tab::new(self.next_tab, s.other.clone(), ViewSpec { show_hidden: t.view_spec.show_hidden, ..ViewSpec::default() });
            other.set_folder_view(t.folder_view());
            other.crumbs = s.other.crumbs();
            self.next_tab += 1;
            self.tabs[k].split = Some(Box::new(Split { other, dir: s.dir, ratio: share(s.ratio), second: s.second }));
        }
        self.wake_other();
        self.strip.rev += 1;
        self.layout();
        self.dirty = true;
    }

    pub(super) fn adopt_split(&mut self, now: Instant) {
        let k = self.cur;
        let id = self.next_tab;
        let Some(s) = self.tabs[k].split.as_deref_mut() else { return };
        s.other.id = id;
        self.next_tab += 1;
        self.on_other_at(k, |a| a.reload(k, now));
    }

    pub(super) fn item_pane_items(&self, m: &mut Menu) {
        if self.split_shown() {
            m.action("pane.open_other", S::SplitOpenInOtherPane.t());
        } else if self.wide() {
            m.submenu(S::SplitOpenInNewPane.t(), |m| {
                m.action("pane.new_vertical", S::SplitSplitVertically.t());
                m.action("pane.new_horizontal", S::SplitSplitHorizontally.t());
            });
        }
    }

    pub(super) fn here_pane_items(&self, m: &mut Menu) {
        if self.split_shown() {
            m.action("pane.here_other", S::SplitOpenInOtherPane.t());
            m.action("pane.close", S::SplitClosePane.t());
        } else if self.wide() {
            m.submenu(S::SplitOpenInNewPane.t(), |m| {
                m.action("pane.split_vertical", S::SplitSplitVertically.t());
                m.action("pane.split_horizontal", S::SplitSplitHorizontally.t());
            });
        }
        m.separator();
    }

    pub(super) fn place_pane_items(&self, m: &mut Menu, loc: &Location) {
        if self.split_shown() {
            m.place(S::SplitOpenInOtherPane.t(), PlaceCmd::Pane(loc.clone(), None), false);
        } else if self.wide() {
            m.submenu(S::SplitOpenInNewPane.t(), |m| {
                m.place(S::SplitSplitVertically.t(), PlaceCmd::Pane(loc.clone(), Some(SplitDir::Vertical)), false);
                m.place(S::SplitSplitHorizontally.t(), PlaceCmd::Pane(loc.clone(), Some(SplitDir::Horizontal)), false);
            });
        }
    }

    pub(super) fn tab_pane_items(&self, m: &mut Menu) {
        m.separator();
        match self.dir() {
            Some(dir) => {
                m.check("pane.arrange_vertical", S::SplitArrangeSideBySide.t(), dir == SplitDir::Vertical);
                m.check("pane.arrange_horizontal", S::SplitArrangeStacked.t(), dir == SplitDir::Horizontal);
                m.action("pane.close", S::SplitClosePane.t());
            }
            None => {
                m.action("pane.split_vertical", S::SplitSplitVertically.t());
                m.action("pane.split_horizontal", S::SplitSplitHorizontally.t());
            }
        }
        m.check("pane.dual_default", S::SplitDualDefault.t(), self.split_prefs.dual);
    }
}

#[cfg(test)]
#[path = "../../test/ui/split.rs"]
mod tests;
