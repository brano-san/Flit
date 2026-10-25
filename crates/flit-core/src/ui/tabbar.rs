use super::{App, Drag, Effect, Hit, Route, Tab, WindowCmd, G_DRIVE, G_FOLDER, MAX_ROUTES};
use crate::input::{Key, MouseButton};
use crate::location::Location;
use crate::path::PathKind;
use crate::render::Rect;
use crate::sort::ViewSpec;
use crate::text::{Gfx, TextBackend};
use crate::theme::Theme;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::time::{Duration, Instant};

pub const G_TAB_ACTIONS: &str = "\u{E70D}";
const G_COMPUTER: &str = "\u{E7F4}";
const G_LINUX: &str = "\u{E756}";
const G_BIN: &str = "\u{E74D}";
const DRAG_START: f32 = 6.0;
const TEAR_MARGIN: f32 = 24.0;
const TIP_DELAY: Duration = Duration::from_millis(600);

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
    pub splits: Vec<Option<super::SplitSession>>,
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
    tip: Option<(usize, Option<Instant>)>,
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

fn full_name(t: &Tab) -> String {
    match t.location().path().kind() {
        PathKind::Computer | PathKind::RecycleBin | PathKind::Home => t.title(),
        _ => t.location().display().into_owned(),
    }
}

impl App {
    pub fn window_title(&self) -> String {
        format!("{} — Flit", full_name(self.tab()))
    }

    pub fn session(&self) -> WindowSession {
        WindowSession { tabs: self.tabs.iter().map(|t| t.location().clone()).collect(), cur: self.cur, splits: self.split_sessions() }
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
        self.adopt_split(now);
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
        self.strip.tip = None;
        if self.tab().gen == 0 {
            self.load_tab(self.cur, None, Instant::now());
        }
        self.wake_other();
    }

    fn tab_slot(&self, x: f32) -> usize {
        let before = self.layout.tabs.iter().enumerate().filter(|&(j, (r, _))| j != self.cur && x > r.x + r.w / 2.0).count();
        before.min(self.tabs.len() - 1)
    }

    fn move_tab(&mut self, to: usize) {
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
        let torn = y < title.y - m || y > title.bottom() + m || x < -m || x > self.size.0 + m;
        if !torn {
            self.move_tab(self.tab_slot(x));
        }
        self.drag = Drag::Tab { origin, x: x0, y: y0, moved: true, torn };
        self.dirty = true;
    }

    pub(super) fn strip_key(&mut self, key: Key) -> bool {
        let Drag::Tab { origin, .. } = self.drag else { return false };
        if key != Key::Escape {
            return false;
        }
        self.move_tab(origin.min(self.tabs.len() - 1));
        self.drag = Drag::None;
        self.pressed = Hit::None;
        self.dirty = true;
        true
    }

    pub(super) fn tear_off(&mut self, x: f32, y: f32) {
        self.detach_tab(Some((x, y)));
    }

    fn detach_tab(&mut self, at: Option<(f32, f32)>) {
        let last = self.tabs.len() == 1;
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
        } else {
            self.cur = self.cur.min(self.tabs.len() - 1);
            self.after_tab_change();
        }
        self.effects.push(Effect::MoveTab { tab: MovedTab(Box::new(t)), at, last });
    }

    pub(super) fn move_tab_to_window(&mut self) {
        if self.tabs.len() == 1 {
            return self.status("This is the only tab in the window");
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
        self.effects.push(Effect::Window(WindowCmd::Close));
    }

    pub(super) fn strip_wheel(&mut self, dy: f32) -> bool {
        let Some((x, y)) = self.strip.pointer else { return false };
        let l = &self.layout;
        if !l.title.contains(x, y) || l.caption.iter().any(|r| r.contains(x, y)) {
            return false;
        }
        self.strip.wheel += if self.tab_wheel_reversed { -dy } else { dy };
        let steps = self.strip.wheel.trunc();
        self.strip.wheel -= steps;
        let to = (self.cur as isize - steps as isize).clamp(0, self.tabs.len() as isize - 1);
        self.select_tab(to as usize);
        true
    }

    pub(super) fn strip_pointer(&mut self, x: f32, y: f32, now: Instant) {
        self.strip.pointer = Some((x, y));
        let over = match self.hit(x, y) {
            Hit::Tab(i) if self.drag == Drag::None => Some(i),
            _ => None,
        };
        match (over, self.strip.tip) {
            (Some(i), Some((k, _))) if i == k => {}
            (Some(i), _) => {
                self.strip.tip = Some((i, Some(now + TIP_DELAY)));
                self.arm_timer(now, now + TIP_DELAY);
            }
            (None, Some((_, shown))) => {
                self.strip.tip = None;
                self.dirty |= shown.is_none();
            }
            (None, None) => {}
        }
    }

    pub(super) fn tip_timer(&mut self, now: Instant) {
        let Some((i, Some(at))) = self.strip.tip else { return };
        if now + super::ops::TIMER_SLACK < at {
            return self.arm_timer(now, at);
        }
        self.strip.tip = Some((i, None));
        self.dirty = true;
    }

    pub(super) fn draw_tab_tip<B: TextBackend>(&mut self, g: &mut Gfx<B>, th: &Theme) {
        let Some((i, None)) = self.strip.tip else { return };
        let (Some(t), Some(&(r, _))) = (self.tabs.get(i), self.layout.tabs.get(i)) else { return };
        let text = full_name(t);
        let font = self.ui_font(12.0);
        let m = g.metrics(font);
        let (pad, h) = (self.s(8.0), self.s(26.0));
        let w = (g.measure(&text, font) + 2.0 * pad).min(self.size.0 - self.s(16.0));
        let x = r.x.min(self.size.0 - w - self.s(8.0)).max(self.s(8.0));
        let b = Rect::new(x, r.bottom() + self.s(4.0), w, h);
        g.dl.round_rect(b, th.bg, self.s(4.0));
        g.dl.outline(b, th.input_border, self.s(4.0), 1.0);
        g.text(b.x + pad, (b.y + (h + m.ascent - m.descent) / 2.0).round(), &text, font, th.text, w - 2.0 * pad);
    }

    pub(super) fn strip_mouse_down(&mut self, h: Hit, button: MouseButton, clicks: u8, now: Instant) -> bool {
        let plain = !self.mods.ctrl && !self.mods.shift && !self.mods.alt;
        match (button, h) {
            (MouseButton::Middle, Hit::Row(row)) if !self.autoscrolling() => self.middle_open(row, now),
            (MouseButton::Left, Hit::ListEmpty) if clicks == 2 && plain && self.double_click_up => {
                self.go_up(now);
                true
            }
            (MouseButton::Left, Hit::Suggestion(k)) => {
                self.pick_suggestion(k);
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
            Hit::Tab(_) | Hit::TabClose(_) => {
                let menu = self.build_tab_menu();
                self.flit_menu(x, y, menu);
            }
            Hit::Nav(i @ 0..=1) => self.history_menu(i),
            _ => {}
        }
    }

    pub(super) fn tab_actions_menu(&mut self) {
        let r = self.layout.actions;
        let menu = self.build_tab_actions_menu();
        self.flit_menu(r.x, r.bottom(), menu);
    }

    pub(super) fn open_many(&mut self, now: Instant) {
        let t = self.tab();
        let Some(v) = &t.view else { return };
        let browse = self.archive_view().is_none();
        let (dirs, files): (Vec<u32>, Vec<u32>) = v.selected().partition(|&i| v.listing().entries[i as usize].is_dir() || (browse && crate::archive::browsable(v.listing().name(i)).is_some()));
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
