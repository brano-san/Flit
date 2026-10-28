use super::toolbar::ToolHit;
use super::{App, Focus, Hit};
use crate::input::{Key, Mods};
use crate::render::Rect;
use crate::text::{Gfx, TextBackend};
use crate::theme::Theme;
use std::time::Instant;

pub(super) const ORDER: [Focus; 7] = [Focus::TabBar, Focus::Nav, Focus::Address, Focus::Toolbar, Focus::Sidebar, Focus::List, Focus::Preview];

impl App {
    pub(super) fn focus_targets(&self, region: Focus) -> Vec<(Hit, Rect)> {
        let l = &self.layout;
        match region {
            Focus::TabBar => l.tabs.iter().enumerate().map(|(i, &(r, _))| (Hit::Tab(i), r)).chain([(Hit::NewTab, l.new_tab)]).collect(),
            Focus::Nav => (0..4).map(|i| (Hit::Nav(i), l.nav[i])).chain([(Hit::ViewMode, l.view), (Hit::Bookmark, l.bookmark), (Hit::PanelMenu, l.more)]).collect(),
            Focus::Address => vec![(Hit::Address, l.address)],
            Focus::Toolbar => self.tool_buttons().into_iter().map(|(h, r, ..)| (Hit::Tool(h), r)).collect(),
            Focus::Sidebar => self.nav.sidebar.rects.iter().enumerate().map(|(i, &r)| (Hit::Side(i), r)).collect(),
            Focus::List => vec![(Hit::ListEmpty, l.list)],
            Focus::Preview => vec![(Hit::None, self.preview_area())],
            Focus::Filter => vec![(Hit::FilterField, self.nav.filter.field)],
        }
    }

    fn reachable(&self, region: Focus) -> bool {
        match region {
            Focus::Address => false,
            Focus::List => true,
            _ => self.focus_targets(region).iter().any(|(_, r)| r.w > 0.0 && r.h > 0.0),
        }
    }

    pub(super) fn cycle_focus(&mut self, back: bool) {
        let at = ORDER.iter().position(|&f| f == self.focus).unwrap_or(5);
        let n = ORDER.len();
        let next = (1..=n).map(|k| ORDER[if back { (at + n * 2 - k) % n } else { (at + k) % n }]).find(|&f| self.reachable(f)).unwrap_or(Focus::List);
        self.enter_focus(next);
    }

    pub(super) fn enter_focus(&mut self, region: Focus) {
        self.focus = region;
        self.kb = match region {
            Focus::TabBar => self.cur,
            Focus::Sidebar => {
                let here = self.current_location().clone();
                let sb = &mut self.nav.sidebar;
                if let Some(i) = sb.items.iter().position(|it| it.loc == here) {
                    sb.cursor = i;
                }
                sb.cursor
            }
            _ => 0,
        };
        if region == Focus::Address {
            self.open_address();
        }
        self.dirty = true;
    }

    pub(super) fn focus_key(&mut self, key: Key, m: Mods, now: Instant) -> bool {
        if key == Key::Tab && !m.ctrl && !m.alt && self.nav.palette.is_none() && !self.cycles_panels() {
            self.cycle_focus(m.shift);
            return true;
        }
        if !matches!(self.focus, Focus::TabBar | Focus::Nav | Focus::Toolbar | Focus::Preview) || m.ctrl || m.alt {
            return false;
        }
        if self.focus == Focus::Preview {
            return match key {
                Key::Up | Key::Down | Key::PageUp | Key::PageDown => {
                    let step = if matches!(key, Key::PageUp | Key::PageDown) { 5.0 } else { 1.0 };
                    self.scroll_preview(if matches!(key, Key::Up | Key::PageUp) { step } else { -step })
                }
                Key::Escape => {
                    self.focus = Focus::List;
                    true
                }
                _ => false,
            };
        }
        let n = self.focus_targets(self.focus).len();
        if n == 0 {
            return false;
        }
        match key {
            Key::Left | Key::Up => self.kb = (self.kb + n - 1) % n,
            Key::Right | Key::Down => self.kb = (self.kb + 1) % n,
            Key::Home => self.kb = 0,
            Key::End => self.kb = n - 1,
            Key::Escape => self.focus = Focus::List,
            Key::Enter | Key::Space => {
                if let Some(&(h, _)) = self.focus_targets(self.focus).get(self.kb.min(n - 1)) {
                    self.activate(h, now);
                }
            }
            _ => return false,
        }
        self.dirty = true;
        true
    }

    pub(super) fn activate(&mut self, h: Hit, now: Instant) {
        match h {
            Hit::Tab(i) => self.select_tab(i),
            Hit::Row(row) => {
                let i = self.tab().view.as_ref().and_then(|v| v.rows().get(row).copied());
                if let Some(i) = i {
                    self.open_entry(i, now);
                }
            }
            Hit::Address => self.enter_focus(Focus::Address),
            Hit::Tool(ToolHit::Button(_) | ToolHit::More) => {
                if let Hit::Tool(t) = h {
                    self.toolbar_click(t, now);
                }
            }
            _ if self.jobs_click(h, now) => {}
            _ => {
                let r = self.focus_targets(self.focus).into_iter().find(|&(k, _)| k == h).map_or(Rect::default(), |(_, r)| r);
                self.click(h, r.x, r.bottom(), now);
            }
        }
        self.dirty = true;
    }

    pub(super) fn kb_rect(&self) -> Option<Rect> {
        if !self.active || !matches!(self.focus, Focus::TabBar | Focus::Nav | Focus::Toolbar | Focus::Preview) {
            return None;
        }
        self.focus_targets(self.focus).get(self.kb).map(|&(_, r)| r).filter(|r| r.w > 0.0)
    }

    pub(super) fn draw_kb_focus<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme) {
        if let Some(r) = self.kb_rect() {
            let w = self.s(2.0).max(2.0);
            g.dl.outline(r.inset(w / 2.0, w / 2.0), t.focus_ring, self.s(4.0), w);
        }
    }
}

#[cfg(test)]
#[path = "../../test/ui/focus.rs"]
mod tests;
