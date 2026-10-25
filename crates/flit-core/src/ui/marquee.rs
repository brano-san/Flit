use super::{App, Drag};
use crate::provider::TabId;
use crate::render::Rect;
use crate::text::{Gfx, TextBackend};
use crate::theme::Theme;

const START_DISTANCE: f32 = 4.0;
const AUTOSCROLL_RATE: f32 = 12.0;

pub(super) struct Marquee {
    tab: TabId,
    start: (f32, f32),
    last: (f32, f32),
    toggle: bool,
    base: Vec<u32>,
    active: bool,
}

impl App {
    pub(super) fn marquee_column(&self, x: f32) -> bool {
        x >= self.layout.cols[1].x
    }

    pub(super) fn marquee_base(&self) -> Vec<u32> {
        match &self.tab().view {
            Some(v) if self.mods.ctrl => v.selected().collect(),
            _ => Vec::new(),
        }
    }

    pub(super) fn arm_marquee(&mut self, x: f32, y: f32, toggle: bool, base: Vec<u32>) {
        let t = self.tab();
        let start = (x, y - self.layout.list.y + t.scroll);
        self.marquee = Some(Marquee { tab: t.id, start, last: (x, y), toggle, base, active: false });
    }

    pub(super) fn marquee_move(&mut self, x: f32, y: f32) -> bool {
        let threshold = self.s(START_DISTANCE);
        let list_y = self.layout.list.y;
        let scroll = self.tab().scroll;
        let Some(m) = &mut self.marquee else { return false };
        m.last = (x, y);
        if !m.active {
            let (sx, sy) = (m.start.0, m.start.1 + list_y - scroll);
            if (x - sx).abs().max((y - sy).abs()) < threshold {
                return true;
            }
            m.active = true;
            self.drag = Drag::Marquee;
        }
        self.update_marquee();
        self.dirty = true;
        true
    }

    pub(super) fn end_marquee(&mut self) {
        self.marquee = None;
    }

    fn update_marquee(&mut self) {
        let (rh, list) = (self.layout.row_h, self.layout.list);
        let App { marquee, tabs, cur, .. } = self;
        let Some(m) = marquee.as_ref() else { return };
        let t = &mut tabs[*cur];
        if t.id != m.tab {
            *marquee = None;
            return;
        }
        let Some(v) = t.view.as_mut() else { return };
        let end = m.last.1 - list.y + t.scroll;
        let (top, bottom) = (m.start.1.min(end), m.start.1.max(end));
        v.clear_selection();
        for &i in &m.base {
            v.set_selected(i, true);
        }
        let n = v.rows().len();
        if n == 0 || bottom < 0.0 || top >= n as f32 * rh {
            return;
        }
        let rh = rh.max(1.0);
        let (first, last) = ((top.max(0.0) / rh) as usize, ((bottom / rh) as usize).min(n - 1));
        for row in first..=last {
            let i = v.rows()[row];
            let on = !(m.toggle && v.is_selected(i));
            v.set_selected(i, on);
        }
        let row_at = |y: f32| ((y.max(0.0) / rh) as usize).min(n - 1);
        v.anchor = Some(v.rows()[row_at(m.start.1)]);
        v.focus = Some(v.rows()[row_at(end)]);
    }

    pub(super) fn marquee_autoscroll(&mut self, dt: f32) -> bool {
        let Some(y) = self.marquee.as_ref().filter(|m| m.active).map(|m| m.last.1) else { return false };
        let (rh, list) = (self.layout.row_h, self.layout.list);
        let over = if y < list.y {
            y - list.y
        } else if y > list.bottom() {
            y - list.bottom()
        } else {
            return false;
        };
        let t = self.tab_mut();
        let max = t.max_scroll(rh, list.h);
        if (over < 0.0 && t.scroll <= 0.0) || (over > 0.0 && t.scroll >= max) {
            return false;
        }
        t.scroll = (t.scroll + over * AUTOSCROLL_RATE * dt).clamp(0.0, max);
        t.scroll_target = t.scroll;
        self.update_marquee();
        true
    }

    pub(super) fn draw_marquee<B: TextBackend>(&self, g: &mut Gfx<B>, th: &Theme) {
        let Some(m) = self.marquee.as_ref().filter(|m| m.active) else { return };
        let list = self.layout.list;
        let (x0, y0) = (m.start.0, list.y + m.start.1 - self.tab().scroll);
        let (x1, y1) = (m.last.0.clamp(list.x, list.right()), m.last.1.clamp(list.y, list.bottom()));
        let r = Rect::new(x0.min(x1), y0.min(y1), (x1 - x0).abs(), (y1 - y0).abs());
        g.dl.set_clip(Some(list));
        g.dl.rect(r, th.selection);
        g.dl.outline(r, th.accent, 0.0, self.s(1.0).max(1.0));
        g.dl.set_clip(None);
    }
}

#[cfg(test)]
#[path = "../../test/ui/marquee.rs"]
mod tests;
