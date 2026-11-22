use super::{App, Effect, Hit, WindowCmd};
use crate::input::{Event, PINCH_PER_STEP};
use crate::open::{OpenTarget, Route};
use std::time::{Duration, Instant};

pub(super) const WHEEL_LINES: u32 = 3;
pub(super) const WHEEL_PAGE: u32 = u32::MAX;
const STEP_IDLE: Duration = Duration::from_millis(500);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Steps {
    Zoom,
    Inspector,
    Font,
    Spacing,
    Jobs,
}

#[derive(Default)]
pub(super) struct WheelSteps {
    kind: Option<Steps>,
    sum: f32,
    at: Option<Instant>,
}

impl WheelSteps {
    pub(super) fn add(&mut self, kind: Steps, d: f32, now: Instant) -> i32 {
        let continues = self.kind == Some(kind) && self.sum * d >= 0.0 && self.at.is_some_and(|t| now.saturating_duration_since(t) <= STEP_IDLE);
        if !continues {
            self.sum = 0.0;
        }
        (self.kind, self.at) = (Some(kind), Some(now));
        self.sum += d;
        let whole = self.sum.trunc();
        self.sum -= whole;
        whole as i32
    }
}

pub(super) fn upright(dx: f32, dy: f32) -> bool {
    dy != 0.0 && dy.abs() >= dx.abs()
}

pub(super) fn sideways(dx: f32, dy: f32, shift: bool) -> f32 {
    match (dx != 0.0, shift) {
        (true, _) => dx,
        (false, true) => -dy,
        (false, false) => 0.0,
    }
}

impl App {
    pub(super) fn wheel_px(&self, line: f32, page: f32) -> f32 {
        if self.wheel_lines == WHEEL_PAGE { page } else { self.wheel_lines as f32 * line }
    }

    pub(super) fn wheel(&mut self, ev: &Event, now: Instant) {
        let flip = if self.opts.reverse_scroll { -1.0 } else { 1.0 };
        let (dx, dy, pinch) = match *ev {
            Event::Wheel { dx, dy } => (dx * flip, dy * flip, false),
            Event::Pinch(scale) if scale.is_finite() => (0.0, scale / PINCH_PER_STEP, true),
            _ => return,
        };
        let m = self.mods;
        if !pinch && m.alt && !m.ctrl && !self.settings_open() {
            let (kind, up, down) = if m.shift { (Steps::Spacing, "spacing.increase", "spacing.decrease") } else { (Steps::Font, "font.increase", "font.decrease") };
            if upright(dx, dy) {
                let n = self.wheel_steps.add(kind, dy, now);
                for _ in 0..n.unsigned_abs() {
                    self.run_action(if n > 0 { up } else { down }, now);
                }
            }
            return;
        }
        let zoom = pinch || m.ctrl;
        let scroll = if pinch { 0.0 } else { dy };
        if self.update_wheel(scroll) || self.settings_wheel(scroll) || self.panel_wheel(ev, scroll, now) || self.jobs_wheel(scroll, now) || self.nav_wheel(scroll) || self.columns_wheel(dx, dy) || self.pane_wheel(scroll) {
            return;
        }
        if self.preview_wheel(dx, dy, zoom, now) || self.strip_wheel(scroll) || self.bottom_wheel(dx, dy, now) {
            return;
        }
        if zoom {
            if upright(dx, dy) {
                self.zoom_steps(dy, now);
            }
            return;
        }
        if self.table_wheel(dx, dy) {
            return;
        }
        let (step, max) = (self.wheel_step(), self.max_scroll());
        let along = if self.layout.flow.across { dy - dx } else { dy };
        let t = self.tab_mut();
        t.scroll_target = (t.scroll_target - along * step).clamp(0.0, max);
        self.dirty = true;
    }

    fn zoom_steps(&mut self, dy: f32, now: Instant) {
        let n = self.wheel_steps.add(Steps::Zoom, dy, now);
        for _ in 0..n.unsigned_abs() {
            self.resize_items(n.signum());
        }
    }

    pub(super) fn middle_down(&mut self, h: Hit, x: f32, y: f32, now: Instant) {
        match h {
            Hit::Crumb(i) => self.open_crumb_in_tab(i, now),
            Hit::Nav(i @ 0..=2) => self.open_nav_in_tab(i, now),
            Hit::None if self.layout.title.contains(x, y) => self.open_new_tab(now),
            _ => {}
        }
    }

    pub(super) fn strip_double_click(&mut self, x: f32, y: f32, now: Instant) -> bool {
        if self.layout.title.contains(x, y) {
            self.open_new_tab(now);
            return true;
        }
        if self.in_title(y) && !self.system_title {
            self.effects.push(Effect::Window(WindowCmd::ToggleMaximize));
            return true;
        }
        false
    }

    pub(super) fn open_crumb_in_tab(&mut self, i: usize, now: Instant) {
        if let Some((_, folder)) = self.tab().crumb(i).cloned() {
            self.open_in_tabs(vec![Route { folder, select: None }], now);
        }
    }

    fn open_nav_in_tab(&mut self, i: usize, now: Instant) {
        let t = self.tab();
        let (route, history) = if i < 2 {
            let mut h = t.history.clone();
            if !h.jump(if i == 0 { -1 } else { 1 }, None) {
                return;
            }
            (Route { folder: h.current().clone(), select: None }, Some(h))
        } else {
            let here = t.location();
            let Some(folder) = here.parent() else { return };
            (Route { folder, select: self.select_previous.then(|| here.file_name().to_vec()) }, None)
        };
        let before = self.tabs.len();
        self.open_in_tabs(vec![route], now);
        if let (Some(mut h), true) = (history, self.tabs.len() > before) {
            let k = self.tabs.len() - 1;
            h.replace(self.tabs[k].location().clone());
            self.tabs[k].history = h;
        }
    }

    pub(super) fn modified_double_click(&mut self, i: u32, now: Instant) -> bool {
        let m = self.mods;
        let t = self.tab();
        let Some(v) = &t.view else { return false };
        let folder = (v.listing().entries.get(i as usize).is_some_and(|e| e.is_dir()) && !t.view_loc.is_recycle_bin()).then(|| t.entry_location(i)).flatten();
        let target = match (m.ctrl, m.shift, m.alt, folder) {
            (false, false, true, _) => None,
            (true, false, false, Some(folder)) => Some(folder),
            _ => return false,
        };
        if let Some(v) = &mut self.tabs[self.cur].view {
            v.clear_selection();
            v.set_selected(i, true);
            (v.focus, v.anchor) = (Some(i), Some(i));
        }
        match target {
            Some(folder) => self.dispatch_entries(vec![Route { folder, select: None }], OpenTarget::NewWindow, now),
            None => {
                self.run_action("item.inspect", now);
            }
        }
        true
    }
}

#[cfg(test)]
#[path = "../../test/ui/pointer.rs"]
mod tests;
