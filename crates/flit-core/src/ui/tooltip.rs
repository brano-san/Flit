use super::actions::{index, ACTIONS};
use super::ops::TIMER_SLACK;
use super::options::InspectorAt;
use super::preview_pane::{PaneHit, PaneTool, Scale};
use super::toolbar::ToolHit;
use super::widgets::{Paint, RADIUS};
use super::{App, Column, Drag, Hit, ViewMode};
use crate::i18n::S;
use crate::render::Rect;
use crate::text::{Gfx, TextBackend};
use crate::theme::Theme;
use std::time::{Duration, Instant};

pub(super) const DELAY: Duration = Duration::from_millis(600);
const GAP: f32 = 6.0;
const PAD_X: f32 = 12.0;
const PAD_Y: f32 = 8.0;
const LINE_GAP: f32 = 6.0;
const MAX_W: f32 = 520.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Tip {
    pub hit: Hit,
    pub at: Instant,
    pub shown: Option<Instant>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TipText {
    pub line: String,
    pub command: Option<String>,
    pub keys: Vec<String>,
}

fn view_tip(mode: ViewMode) -> S {
    match mode {
        ViewMode::None => S::TooltipViewNone,
        ViewMode::Folder => S::TooltipViewFolder,
        ViewMode::Subfolders => S::TooltipViewSubfolders,
    }
}

fn text(line: S, command: &str) -> (String, Option<&str>) {
    (line.t().to_owned(), Some(command).filter(|c| !c.is_empty()))
}

impl App {
    pub(super) fn tip_hover(&mut self, h: Hit, now: Instant) {
        let quiet = self.tooltips && self.drag == Drag::None && self.pressed == Hit::None && self.own_menu.is_none() && self.confirm.is_none();
        let wanted = quiet && self.tip_anchor(h).is_some() && self.tip_text(h).is_some();
        match (wanted, self.tip) {
            (true, Some(t)) if t.hit == h => {}
            (true, prev) => {
                let chained = prev.is_some_and(|t| t.shown.is_some());
                let at = if chained { now } else { now + DELAY };
                self.tip = Some(Tip { hit: h, at, shown: chained.then_some(now) });
                if chained {
                    self.dirty = true;
                } else {
                    self.arm_timer(now, at);
                }
            }
            (false, Some(_)) => self.tip_hide(),
            (false, None) => {}
        }
    }

    pub(super) fn tip_timer(&mut self, now: Instant) {
        let Some(t) = self.tip.filter(|t| t.shown.is_none()) else { return };
        if now + TIMER_SLACK < t.at {
            return self.arm_timer(now, t.at);
        }
        if self.tip_anchor(t.hit).is_none() || self.hover != t.hit {
            self.tip = None;
            return;
        }
        self.tip = Some(Tip { shown: Some(now), ..t });
        self.dirty = true;
    }

    pub(super) fn tip_hide(&mut self) {
        if let Some(t) = self.tip.take() {
            self.dirty |= t.shown.is_some();
        }
    }

    pub(super) fn set_tooltips(&mut self, on: bool) {
        self.tooltips = on;
        if !on {
            self.tip_hide();
        }
    }

    pub fn tooltip(&self) -> Option<TipText> {
        self.tip.filter(|t| t.shown.is_some() && self.tooltips).and_then(|t| self.tip_text(t.hit))
    }

    fn tip_anchor(&self, h: Hit) -> Option<Rect> {
        let l = &self.layout;
        let p = &self.panels;
        let r = match h {
            Hit::Menu => p.menu,
            Hit::QuickToggle => p.quick,
            Hit::Caption(i) => *p.caption.get(i)?,
            Hit::Tab(i) => l.tabs.get(i)?.0,
            Hit::TabClose(i) => l.tabs.get(i)?.1,
            Hit::NewTab => l.new_tab,
            Hit::TabActions => l.actions,
            Hit::Nav(i) => *l.nav.get(i)?,
            Hit::ViewMode => l.view,
            Hit::Bookmark => l.bookmark,
            Hit::PanelMenu => l.more,
            Hit::CrumbScroll(right) => l.crumb_arrows[usize::from(right)],
            Hit::Crumb(i) => *l.crumbs.get(i)?,
            Hit::QuickClear => self.nav.sidebar.clear,
            Hit::GoButton(_) => self.palette_button(h)?,
            Hit::OpsButton => l.ops_button,
            Hit::Encoding => l.encoding,
            Hit::Branch => l.branch,
            Hit::Bin(k) => *l.bin.get(k)?,
            Hit::SidePin(i) => self.nav.sidebar.rects.get(i).map(|r| self.nav.sidebar.pin_rect(*r, self.scale))?,
            Hit::SideSettings => self.nav.sidebar.settings,
            Hit::Eject => self.eject_rect()?,
            Hit::FilterClose => self.nav.filter.close,
            Hit::PaletteClose | Hit::PaletteDescribe | Hit::PaletteScroll(_) => self.palette_button(h)?,
            Hit::Tool(ToolHit::Button(_) | ToolHit::More) => self.tool_rect(h)?,
            Hit::Preview(ph) => self.preview_rect(ph)?,
            _ => return None,
        };
        (r.w > 0.0 && r.h > 0.0).then_some(r)
    }

    pub(super) fn tip_text(&self, h: Hit) -> Option<TipText> {
        let quick = if self.nav.sidebar.state.sidebar { S::TooltipQuickShown } else { S::TooltipQuickHidden };
        let (line, command) = match h {
            Hit::Menu => text(S::TooltipMenu, "menu.main"),
            Hit::QuickToggle => text(quick, "quick_access.toggle"),
            Hit::Caption(0) => text(S::TooltipMinimize, "window.minimize"),
            Hit::Caption(1) => text(if self.maximized { S::TooltipRestore } else { S::TooltipMaximize }, ""),
            Hit::Caption(_) => text(S::TooltipCloseWindow, "window.close"),
            Hit::Tab(i) => (super::tabbar::full_name(self.tabs.get(i)?), None),
            Hit::TabClose(_) => text(S::TooltipCloseTab, "tab.close"),
            Hit::NewTab => text(S::TooltipNewTab, "tab.new"),
            Hit::TabActions => text(S::TooltipTabActions, "tab.search"),
            Hit::Nav(0) => text(S::TooltipBack, "nav.back"),
            Hit::Nav(1) => text(S::TooltipForward, "nav.forward"),
            Hit::Nav(2) => text(S::TooltipUp, "nav.up"),
            Hit::Nav(_) => text(S::TooltipVisited, ""),
            Hit::ViewMode => text(view_tip(self.view_mode()), "folder.cycle_view"),
            Hit::Bookmark => text(if self.nav.sidebar.is_pinned(self.current_location()) { S::TooltipBookmarked } else { S::TooltipNotBookmarked }, "folder.bookmark"),
            Hit::PanelMenu => text(S::TooltipPanelMenu, ""),
            Hit::CrumbScroll(right) => text(if right { S::TooltipCrumbRight } else { S::TooltipCrumbLeft }, ""),
            Hit::Crumb(i) if i < self.tab().crumbs.len() => text(S::TooltipCrumb, "folder.dropdown"),
            Hit::QuickClear => text(S::TooltipQuickClear, ""),
            Hit::GoButton(0) => text(S::TooltipBack, "goto.back"),
            Hit::GoButton(1) => text(S::TooltipForward, "goto.forward"),
            Hit::GoButton(_) => text(S::TooltipUp, "goto.up"),
            Hit::OpsButton => text(S::TooltipOps, "ops.show"),
            Hit::Encoding => text(S::TooltipEncoding, "archive.encoding"),
            Hit::Branch => text(S::TooltipBranch, "git.branches"),
            Hit::Bin(0) => text(S::TooltipToolBinRestore, "bin.restore"),
            Hit::Bin(1) => text(S::TooltipToolBinRestoreAll, "bin.restore_all"),
            Hit::Bin(_) => text(S::TooltipToolBinEmpty, "bin.empty"),
            Hit::SidePin(_) => text(S::TooltipRemoveBookmark, ""),
            Hit::SideSettings => text(S::TooltipSettings, "options.open"),
            Hit::Eject => text(S::TooltipEject, ""),
            Hit::FilterClose => text(S::TooltipRevertFilter, ""),
            Hit::FilterField => text(S::TooltipFilterBox, "view.filter"),
            Hit::Bottom(b) => text(self.bottom_tip(b), b.command()),
            Hit::Header(c) => match self.layout.columns.get(c)? {
                Column::Type if self.tab().hidden_types.is_empty() => text(S::TooltipTypes, "folder.types"),
                Column::Type => text(S::TooltipTypesActive, "folder.types"),
                Column::Name if self.prefs.checkboxes => text(S::TooltipSelectAll, "select.all"),
                _ => return None,
            },
            Hit::PaletteClose => text(S::TooltipClosePalette, ""),
            Hit::PaletteDescribe => text(if self.prefs.command_descriptions { S::TooltipPaletteDescriptionsOn } else { S::TooltipPaletteDescriptionsOff }, ""),
            Hit::PaletteScroll(down) => text(if down { S::TooltipScrollDown } else { S::TooltipScrollUp }, ""),
            Hit::Tool(ToolHit::More) => text(S::TooltipMore, ""),
            Hit::Preview(ph) => match ph {
                PaneHit::Close => text(S::TooltipInspectorClose, "inspector.toggle"),
                PaneHit::At(InspectorAt::Auto) => text(S::TooltipInspectorAuto, "inspector.layout"),
                PaneHit::At(InspectorAt::Right) => text(S::TooltipInspectorRight, "inspector.layout"),
                PaneHit::At(InspectorAt::Below) => text(S::TooltipInspectorBelow, "inspector.layout"),
                PaneHit::Tool(PaneTool::Wrap) => text(S::TooltipInspectorWrap, "inspector.wrap"),
                PaneHit::Tool(PaneTool::Lines) => text(S::TooltipInspectorLines, "inspector.line_numbers"),
                PaneHit::Tool(PaneTool::Spaces) => text(S::TooltipInspectorSpaces, "inspector.whitespace"),
                PaneHit::Tool(PaneTool::Scale(Scale::Fit)) => text(S::TooltipInspectorFit, "inspector.scale"),
                PaneHit::Tool(PaneTool::Scale(Scale::Original)) => text(S::TooltipInspectorOriginal, "inspector.scale"),
                PaneHit::Tool(PaneTool::Scale(Scale::Custom)) => text(S::TooltipInspectorCustom, "inspector.scale"),
                PaneHit::Tool(PaneTool::Grid) => text(S::TooltipInspectorGrid, "inspector.grid"),
                PaneHit::Title => text(S::TooltipInspectorTitle, ""),
                PaneHit::CardDetails => text(S::TooltipInspectorDetails, "inspector.details"),
                PaneHit::CardClose => text(S::TooltipInspectorCardClose, ""),
                PaneHit::Size if self.shows_image() => text(S::TooltipInspectorZoom, "inspector.zoom_in"),
                PaneHit::Size => text(S::TooltipInspectorSize, "inspector.zoom_in"),
                PaneHit::Filter => text(S::TooltipFilterBox, "inspector.filter"),
                _ => return None,
            },
            Hit::Tool(ToolHit::Button(i)) => {
                let (id, line) = self.tool_tip(i)?;
                (line.t().to_owned(), Some(id).filter(|id| index(id).is_some()))
            }
            _ => return None,
        };
        let (command, keys) = match command.and_then(index) {
            Some(i) => {
                let keys = self.keymap.shown(i);
                (Some(ACTIONS[i].title.t().to_owned()), if keys.is_empty() { vec![S::WidgetsBlank.t().to_owned()] } else { keys })
            }
            None => (None, Vec::new()),
        };
        Some(TipText { line, command, keys })
    }

    pub(super) fn draw_tooltip<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme) {
        let Some(tip) = self.tip.filter(|t| t.shown.is_some()) else { return };
        let (Some(text), Some(anchor)) = (self.tooltip(), self.tip_anchor(tip.hit)) else { return };
        let (font, small, sc) = (self.ui_font(12.5), self.ui_font(12.0), self.scale);
        let s = |v: f32| (v * sc).round();
        let (w, h) = self.size;
        let from = g.dl.instances.len();
        let mut p = Paint::new(g, t, sc);
        let m = p.g.metrics(font);
        let line_h = (m.ascent + m.descent).ceil();
        let lead = S::WidgetsCmd.t();
        let cmd = text.command.as_deref().unwrap_or_default();
        let (lead_w, cmd_w) = (p.g.measure(lead, small), p.g.measure(cmd, small));
        let caps_w = p.keycaps_w(&text.keys, small);
        let second_w = if text.command.is_some() { lead_w + s(6.0) + cmd_w + s(8.0) + caps_w } else { 0.0 };
        let cap_h = line_h + s(6.0);
        let max_w = s(MAX_W).min(w - 2.0 * s(GAP));
        let first_w = p.g.measure(&text.line, font).min(max_w - 2.0 * s(PAD_X));
        let bw = (first_w.max(second_w) + 2.0 * s(PAD_X)).ceil().min(max_w);
        let bh = 2.0 * s(PAD_Y) + line_h + if text.command.is_some() { s(LINE_GAP) + cap_h } else { 0.0 };
        let below = anchor.bottom() + s(GAP);
        let y = if below + bh <= h - s(GAP) { below } else { (anchor.y - s(GAP) - bh).max(s(GAP)) };
        let x = (anchor.x + anchor.w / 2.0 - bw / 2.0).round().min(w - bw - s(GAP)).max(s(GAP));
        let r = Rect::new(x, y.round(), bw, bh);
        p.g.dl.round_rect(r, t.popup, s(RADIUS));
        p.g.dl.outline(r, t.popup_border, s(RADIUS), 1.0);
        let top = r.y + s(PAD_Y);
        p.g.text((r.x + (bw - first_w) / 2.0).round(), (top + m.ascent).round(), &text.line, font, t.text, max_w - 2.0 * s(PAD_X));
        if text.command.is_some() {
            let cy = top + line_h + s(LINE_GAP) + cap_h / 2.0;
            let ms = p.g.metrics(small);
            let base = (cy + (ms.ascent - ms.descent) / 2.0).round();
            let mut cx = (r.x + (bw - second_w) / 2.0).round();
            cx += p.g.text(cx, base, lead, small, t.text_dim.alpha(0xA0), f32::INFINITY) + s(6.0);
            cx += p.g.text(cx, base, cmd, small, t.text_dim, f32::INFINITY) + s(8.0);
            p.keycaps(cx, cy, &text.keys, small);
        }
        let since = tip.shown.unwrap_or(tip.at);
        self.pop(g, from, since, (r.x + r.w / 2.0, if r.y > anchor.y { r.y } else { r.bottom() }));
    }
}

#[cfg(test)]
#[path = "../../test/ui/tooltip.rs"]
mod tests;
