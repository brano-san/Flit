use super::{App, Drag, LayoutMode};
use crate::provider::TabId;
use crate::render::Rect;
use crate::view::Line;
use crate::text::{Gfx, TextBackend};
use crate::theme::Theme;

const START_DISTANCE: f32 = 4.0;
const AUTOSCROLL_RATE: f32 = 12.0;

fn overlaps(a: Rect, b: Rect) -> bool {
    a.x <= b.right() && b.x <= a.right() && a.y <= b.bottom() && b.y <= a.bottom()
}

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
        self.layout.mode == LayoutMode::Details && self.layout.cols.first().is_some_and(|c| x >= c.right())
    }

    pub(super) fn marquee_base(&self) -> Vec<u32> {
        match &self.tab().view {
            Some(v) if self.mods.ctrl => v.selected().collect(),
            _ => Vec::new(),
        }
    }

    pub(super) fn arm_marquee(&mut self, x: f32, y: f32, toggle: bool, base: Vec<u32>) {
        let start = self.to_content(x, y);
        self.marquee = Some(Marquee { tab: self.tab().id, start, last: (x, y), toggle, base, active: false });
    }

    pub(super) fn marquee_move(&mut self, x: f32, y: f32) -> bool {
        let threshold = self.s(START_DISTANCE);
        let Some(start) = self.marquee.as_ref().map(|m| self.to_screen(m.start.0, m.start.1)) else { return false };
        let Some(m) = &mut self.marquee else { return false };
        m.last = (x, y);
        if !m.active {
            let (sx, sy) = start;
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
        if self.marquee.as_ref().is_some_and(|m| m.tab != self.tab().id) {
            self.marquee = None;
            return;
        }
        let Some(m) = self.marquee.as_ref() else { return };
        let (f, across) = (self.flow(), self.layout.flow.across);
        let end = self.to_content(m.last.0, m.last.1);
        let band = Rect::new(m.start.0.min(end.0), m.start.1.min(end.1), (m.start.0 - end.0).abs(), (m.start.1 - end.1).abs());
        let (a, b) = if across { (band.x, band.right()) } else { (band.y, band.bottom()) };
        let hits: Vec<usize> = f.lines_in(a, b).filter(|&n| overlaps(f.cell(n), band)).collect();
        let (start, stop) = (self.nearest_line(&f, m.start.0, m.start.1), self.nearest_line(&f, end.0, end.1));
        let App { marquee, tabs, cur, .. } = self;
        let Some(m) = marquee.as_ref() else { return };
        let Some(v) = tabs[*cur].view.as_mut() else { return };
        v.clear_selection();
        for &i in &m.base {
            v.set_selected(i, true);
        }
        let n = v.rows().len();
        if n == 0 {
            return;
        }
        for line in hits {
            if let Some(Line::Row(row)) = v.line(line) {
                let i = v.rows()[row];
                let on = !(m.toggle && v.is_selected(i));
                v.set_selected(i, on);
            }
        }
        let row_at = |line: Option<usize>| v.rows()[line.map_or(0, |l| v.row_at_line(l)).min(n - 1)];
        let (anchor, focus) = (row_at(start), row_at(stop));
        v.anchor = Some(anchor);
        v.focus = Some(focus);
    }

    pub(super) fn marquee_autoscroll(&mut self, dt: f32) -> bool {
        let Some((x, y)) = self.marquee.as_ref().filter(|m| m.active).map(|m| m.last) else { return false };
        let list = self.layout.list;
        let (at, lo, hi) = if self.layout.flow.across { (x, list.x, list.right()) } else { (y, list.y, list.bottom()) };
        let over = if at < lo {
            at - lo
        } else if at > hi {
            at - hi
        } else {
            return false;
        };
        let max = self.max_scroll();
        let t = self.tab_mut();
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
        let (x0, y0) = self.to_screen(m.start.0, m.start.1);
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
