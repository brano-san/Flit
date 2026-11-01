use super::layouts::{layout_of_zoom, step_zoom};
use super::{centered, App, Hit, BIN_BUTTONS, G_BRANCH};
use crate::fmt as format;
use crate::i18n::{P, S};
use crate::render::Rect;
use crate::sort::Folders;
use crate::text::{Gfx, TextBackend};
use crate::theme::Theme;
use bumpalo::Bump;

pub(super) const ELAPSED_MIN_MS: u128 = 500;
const ARRIVE_ELAPSED: u8 = 0;
const ARRIVE_BUTTONS: u8 = 1;
const ARRIVE_BRANCH: u8 = 2;
const ARRIVE_MESSAGE: u8 = 3;
const BUTTON_W: f32 = 26.0;
const ROLL_LIFT: f32 = 8.0;
const ROLL: std::time::Duration = std::time::Duration::from_millis(120);

#[derive(Default)]
pub(super) struct Roll {
    tab: Option<u64>,
    shown: String,
    old: Option<(String, std::time::Instant)>,
}

impl Roll {
    pub fn show(&mut self, tab: u64, label: &str, animate: bool, now: std::time::Instant) -> Option<(String, f32)> {
        if self.tab != Some(tab) {
            *self = Roll { tab: Some(tab), shown: label.to_owned(), old: None };
        } else if self.shown != label {
            let old = std::mem::replace(&mut self.shown, label.to_owned());
            self.old = animate.then_some((old, now));
        }
        if !animate || self.old.as_ref().is_some_and(|o| now.saturating_duration_since(o.1) >= ROLL) {
            self.old = None;
        }
        self.old.as_ref().map(|(o, at)| (o.clone(), super::widgets::eased(*at, now, ROLL)))
    }
}
const ZOOM_W: f32 = 46.0;
const G_CLOCK: &str = "\u{E81C}";
const G_FOLDER: &str = "\u{E8B7}";
const G_FILE: &str = "\u{E7C3}";
const G_ORDER: &str = "\u{E8CB}";
const G_SUBFOLDERS: &str = "\u{E8F4}";
const G_RETAIN: &str = "\u{E718}";
const G_HIDDEN: &str = "\u{E8BB}";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum BottomHit {
    Subfolders,
    Retain,
    Folders,
    Files,
    Order,
    Layout,
    Zoom,
    Elapsed,
}

impl BottomHit {
    pub(super) fn command(self) -> &'static str {
        match self {
            BottomHit::Subfolders => "subfolders.toggle",
            BottomHit::Folders => "view.toggle_folders",
            BottomHit::Files => "view.toggle_files",
            BottomHit::Order => "view.sort.cycle_folders",
            BottomHit::Retain | BottomHit::Layout | BottomHit::Zoom | BottomHit::Elapsed => "",
        }
    }
}

impl App {
    pub(super) fn bottom_hit(&self, x: f32, y: f32) -> Option<Hit> {
        self.layout.bar.iter().find(|(_, r)| r.contains(x, y)).map(|&(b, _)| Hit::Bottom(b))
    }

    pub(super) fn bottom_click(&mut self, b: BottomHit) {
        match b {
            BottomHit::Subfolders => self.toggle_subfolders(),
            BottomHit::Retain => {
                self.prefs.retain_filter = !self.prefs.retain_filter;
                for t in &mut self.tabs {
                    t.retain_filter = self.prefs.retain_filter;
                }
            }
            BottomHit::Folders => self.toggle_kind(true),
            BottomHit::Files => self.toggle_kind(false),
            BottomHit::Order => {
                self.run_action("view.sort.cycle_folders", std::time::Instant::now());
            }
            BottomHit::Elapsed => {}
            BottomHit::Layout | BottomHit::Zoom => {
                let r = self.layout.bar.iter().find(|x| x.0 == b).map_or(Rect::default(), |x| x.1);
                let menu = self.build_layout_menu();
                self.button_menu(Hit::Bottom(b), r.x, r.y, menu);
                if b == BottomHit::Zoom {
                    self.menu_right = Some(r.right());
                }
            }
        }
        self.dirty = true;
    }

    pub(super) fn bottom_wheel(&mut self, dy: f32) -> bool {
        if !matches!(self.hover, Hit::Bottom(BottomHit::Zoom | BottomHit::Layout)) {
            return false;
        }
        let z = if self.tab().mode.zoomed() { self.tab().zoom } else { super::layouts::ZOOM_DETAILS };
        self.set_zoom(step_zoom(z, dy > 0.0));
        true
    }

    pub(super) fn bottom_tip(&self, b: BottomHit) -> S {
        let spec = self.tab().view_spec;
        match b {
            BottomHit::Subfolders if self.tab().subfolders => S::TooltipSubfoldersOn,
            BottomHit::Subfolders => S::TooltipSubfoldersOff,
            BottomHit::Retain if self.prefs.retain_filter => S::TooltipRetainFilter,
            BottomHit::Retain => S::TooltipResetFilter,
            BottomHit::Folders if spec.hide_dirs => S::TooltipFoldersHidden,
            BottomHit::Folders => S::TooltipFoldersShown,
            BottomHit::Files if spec.hide_files => S::TooltipFilesHidden,
            BottomHit::Files => S::TooltipFilesShown,
            BottomHit::Order => match spec.sort.folders {
                Folders::First => S::TooltipOrderFirst,
                Folders::Last => S::TooltipOrderLast,
                Folders::Mixed => S::TooltipOrderMixed,
            },
            BottomHit::Layout => S::TooltipLayoutPicker,
            BottomHit::Zoom => S::TooltipZoom,
            BottomHit::Elapsed if self.tab().stats.op.is_some() => S::TooltipElapsedOp,
            BottomHit::Elapsed => S::TooltipElapsed,
        }
    }

    pub(super) fn filter_placeholder(&self) -> String {
        let n = self.tab().view.as_ref().map_or(0, |v| {
            let c = v.all_counts();
            if self.tab().subfolders { c.dirs + c.files } else { v.snap().order.len() }
        });
        crate::tr!(S::FilterFiles, n = n)
    }

    fn bar_button<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme, r: Rect, b: BottomHit, glyph: &str, on: bool) {
        let s3 = self.s(3.0);
        let lit = self.lit(Hit::Bottom(b));
        if lit > 0.0 {
            g.dl.round_rect(r, super::widgets::faded(t.button_hover, lit), s3);
        }
        centered(g, r, if on { super::widgets::filled(glyph) } else { glyph }, self.icon_font(12.0), if on { t.text } else { t.text_dim });
        self.layout.bar.push((b, r));
    }

    fn counter<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme, right: f32, b: BottomHit, label: &str, settled: bool) -> f32 {
        let r = self.layout.status;
        let now = self.last_frame.unwrap_or_else(std::time::Instant::now);
        let (tab, animate) = (self.tab().id, self.animations && settled);
        let roll = &mut self.rolls[usize::from(b == BottomHit::Folders)];
        let rolling = roll.show(tab, label, animate, now);
        self.motion |= rolling.is_some();
        let spec = self.tab().view_spec;
        let (glyph, hidden) = if b == BottomHit::Folders { (G_FOLDER, spec.hide_dirs) } else { (G_FILE, spec.hide_files) };
        let font = self.ui_font(12.0);
        let (pad, icon) = (self.s(6.0), self.s(16.0));
        let w = g.measure(label, font) + icon + 3.0 * pad;
        let br = Rect::new(right - w, r.y + self.s(2.0), w, r.h - self.s(4.0));
        let lit = self.lit(Hit::Bottom(b));
        if lit > 0.0 {
            g.dl.round_rect(br, super::widgets::faded(t.button_hover, lit), self.s(3.0));
        }
        let ir = Rect::new(br.x + pad, br.y, icon, br.h);
        centered(g, ir, glyph, self.icon_font(12.0), t.text_dim);
        if hidden {
            centered(g, ir, G_HIDDEN, self.icon_font(7.0), t.text_dim);
        }
        let m = g.metrics(font);
        let baseline = (r.y + (r.h + m.ascent - m.descent) / 2.0).round();
        let ink = if hidden { t.text_dim.alpha(0x90) } else { t.text_dim };
        match rolling {
            Some((old, e)) => {
                let lift = self.s(ROLL_LIFT);
                g.dl.set_clip(Some(br));
                g.text(ir.right() + pad, (baseline - e * lift).round(), &old, font, super::widgets::faded(ink, 1.0 - e), f32::INFINITY);
                g.text(ir.right() + pad, (baseline + (1.0 - e) * lift).round(), label, font, super::widgets::faded(ink, e), f32::INFINITY);
                g.dl.set_clip(None);
            }
            None => {
                g.text(ir.right() + pad, baseline, label, font, ink, f32::INFINITY);
            }
        }
        self.layout.bar.push((b, br));
        br.x - self.s(2.0)
    }

    pub(super) fn draw_status<B: TextBackend>(&mut self, g: &mut Gfx<B>, arena: &Bump, t: &Theme) {
        let r = self.layout.status;
        self.layout.bar.clear();
        let right_from = g.dl.instances.len();
        let font = self.ui_font(12.0);
        let m = g.metrics(font);
        let baseline = (r.y + (r.h + m.ascent - m.descent) / 2.0).round();
        let pad = self.s(10.0);
        let bw = self.s(BUTTON_W);
        let br = |x: f32| Rect::new(x, r.y + (r.h - bw).max(0.0) / 2.0, bw, bw.min(r.h));
        let mut right = r.right() - self.s(6.0);
        let (mode, zoom) = (self.tab().mode, self.tab().zoom);
        let zr = Rect::new(right - self.s(ZOOM_W), r.y + self.s(2.0), self.s(ZOOM_W), r.h - self.s(4.0));
        if mode.zoomed() {
            let fill = Rect::new(zr.x, zr.y, zr.w * zoom.min(100) as f32 / 100.0, zr.h);
            g.dl.round_rect(zr, crate::theme::mix(t.input_bg, t.button_hover, self.lit(Hit::Bottom(BottomHit::Zoom)).max(f32::from(u8::from(self.menu_open_from(Hit::Bottom(BottomHit::Zoom)))))), self.s(3.0));
            if fill.w > 0.0 {
                g.dl.round_rect(fill, t.accent.alpha(0x60), self.s(3.0));
            }
            let label = arena.alloc_str(&format!("{zoom}%"));
            let w = g.measure(label, font);
            g.text(zr.x + (zr.w - w) / 2.0, baseline, label, font, t.text, f32::INFINITY);
            self.layout.bar.push((BottomHit::Zoom, zr));
            right = zr.x - self.s(4.0);
        }
        let look = if mode.zoomed() { layout_of_zoom(zoom) } else { mode };
        self.bar_button(g, t, br(right - bw), BottomHit::Layout, look.icon(), false);
        right -= bw + self.s(2.0);
        let order = self.tab().view_spec.sort.folders != Folders::First;
        self.bar_button(g, t, br(right - bw), BottomHit::Order, G_ORDER, order);
        right -= bw + self.s(6.0);

        let tab = self.tab();
        let current = tab.view.as_ref().filter(|v| v.snap().gen == tab.gen && !tab.location().is_home_page());
        if let Some(v) = current {
            let (spec, all, shown) = (v.snap().view, v.all_counts(), v.shown_counts());
            let label = |n: usize, total: usize, hidden: bool| match hidden {
                true => format!("0/{total}"),
                false if n == total => total.to_string(),
                false => format!("{n}/{total}"),
            };
            let (files, dirs) = (label(shown.files, all.files, spec.hide_files), label(shown.dirs, all.dirs, spec.hide_dirs));
            let settled = v.snap().done && tab.loading.is_none();
            right = self.counter(g, t, right, BottomHit::Files, arena.alloc_str(&files), settled);
            right = self.counter(g, t, right, BottomHit::Folders, arena.alloc_str(&dirs), settled);
        }
        let mut arrived: Vec<(u8, usize, usize)> = Vec::new();
        let stats = self.tab().stats;
        let at = g.dl.instances.len();
        if let Some(done) = stats.op.or(stats.done).filter(|d| d.as_millis() > ELAPSED_MIN_MS) {
            let mut n = bumpalo::collections::String::new_in(arena);
            crate::i18n::decimal(&mut n, done.as_secs_f64(), 1);
            let label = arena.alloc_str(&crate::tr!(S::FilterSeconds, n = n));
            let w = g.measure(label, font);
            let icon = self.s(16.0);
            right -= w + icon + self.s(10.0);
            centered(g, Rect::new(right, r.y, icon, r.h), G_CLOCK, self.icon_font(12.0), t.text_dim);
            g.text(right + icon + self.s(4.0), baseline, label, font, t.text_dim, f32::INFINITY);
            self.layout.bar.push((BottomHit::Elapsed, Rect::new(right, r.y, w + icon + self.s(4.0), r.h)));
            right -= self.s(6.0);
            arrived.push((ARRIVE_ELAPSED, at, g.dl.instances.len()));
        }
        let at = g.dl.instances.len();

        let in_bin = self.tab().view_loc.is_recycle_bin();
        let encoding = self.encoding_label();
        let encoding = [encoding.as_str()];
        let bin = BIN_BUTTONS.map(S::t);
        let labels: &[&str] = if in_bin { &bin } else if encoding[0].is_empty() { &[] } else { &encoding };
        let mut buttons = [Rect::default(); 3];
        for (k, label) in labels.iter().enumerate().rev() {
            let w = g.measure(label, font) + self.s(16.0);
            right -= w;
            let b = Rect::new(right, r.y + self.s(2.0), w, r.h - self.s(4.0));
            buttons[k] = b;
            let lit = self.lit(if in_bin { Hit::Bin(k) } else { Hit::Encoding });
            if lit > 0.0 {
                g.dl.round_rect(b, super::widgets::faded(t.button_hover, lit), self.s(3.0));
            }
            g.text(b.x + self.s(8.0), baseline, label, font, t.text_dim, f32::INFINITY);
            right -= self.s(4.0);
        }
        if !labels.is_empty() {
            arrived.push((ARRIVE_BUTTONS, at, g.dl.instances.len()));
        }
        let at = g.dl.instances.len();
        self.layout.encoding = if in_bin { Rect::default() } else { buttons[0] };
        self.layout.bin = if in_bin { buttons } else { Default::default() };
        self.layout.branch = Rect::default();
        if let Some(branch) = self.git_branch().filter(|b| !b.is_empty() && !in_bin) {
            let label = arena.alloc_str(&format!("{G_BRANCH} {branch}"));
            let w = g.measure(label, font) + self.s(16.0);
            right -= w;
            let b = Rect::new(right, r.y + self.s(2.0), w, r.h - self.s(4.0));
            let lit = self.lit(Hit::Branch);
            if lit > 0.0 {
                g.dl.round_rect(b, super::widgets::faded(t.button_hover, lit), self.s(3.0));
            }
            g.text(b.x + self.s(8.0), baseline, label, font, t.text_dim, f32::INFINITY);
            self.layout.branch = b;
            arrived.push((ARRIVE_BRANCH, at, g.dl.instances.len()));
        }

        self.fit_filter(right - 2.0 * bw - pad - self.s(6.0));
        let left_from = g.dl.instances.len();
        self.draw_filter(g, t);
        let mut x = self.nav.filter.field.right() + self.s(4.0);
        let (subfolders, retain) = (self.tab().subfolders, self.prefs.retain_filter);
        self.bar_button(g, t, br(x), BottomHit::Subfolders, G_SUBFOLDERS, subfolders);
        x += bw + self.s(2.0);
        self.bar_button(g, t, br(x), BottomHit::Retain, G_RETAIN, retain);
        x += bw + self.s(4.0);
        let left_end = x;
        x += pad;

        let tab = self.tab();
        let mut s = bumpalo::collections::String::new_in(arena);
        let mut color = t.text_dim;
        if let Some(err) = &tab.error {
            s.push_str(err);
            color = t.error;
        } else if let Some(v) = current_view(self) {
            let k = v.selected_count();
            if k > 0 {
                crate::trn_into!(&mut s, P::UiStatusSelected, k);
                let size = v.selected_size();
                if size > 0 {
                    s.push_str("  ");
                    format::size_in(&mut s, size, self.style.decimal);
                }
            }
            if let Some(total) = self.unpacked_total() {
                if !s.is_empty() {
                    s.push_str("    ");
                }
                let mut size = String::new();
                format::size_in(&mut size, total, self.style.decimal);
                crate::tr_into!(&mut s, S::UiStatusUnpacked, size = size);
            }
            if !v.snap().done {
                if !s.is_empty() {
                    s.push_str("    ");
                }
                s.push_str(S::UiStatusLoading.t());
            }
        } else if let Some(l) = &tab.loading {
            s.push_str(l.note.as_deref().unwrap_or(S::UiStatusLoading.t()));
        }
        let room = (right - x - pad).max(0.0);
        let text_w = g.measure(&s, font).min(room);
        let text_x = right - text_w;
        if text_w > 0.0 {
            let at = g.dl.instances.len();
            g.text(text_x, baseline, &s, font, color, text_w);
            arrived.push((ARRIVE_MESSAGE, at, g.dl.instances.len()));
        }
        let now = self.last_frame.unwrap_or_else(std::time::Instant::now);
        let keys: Vec<u8> = arrived.iter().map(|a| a.0).collect();
        if self.bar_arrivals.settle(self.tab().id, &keys, self.animations, now) {
            self.motion = true;
        }
        for &(key, from, to) in &arrived {
            let shown = self.bar_arrivals.shown(key, now);
            if shown < 1.0 {
                let edge = g.dl.instances[from..to].iter().map(|i| i.rect[0] + i.rect[2]).fold(0.0, f32::max);
                g.dl.grow_range(from..to, shown, (edge, r.y + r.h / 2.0), 0.9);
                g.dl.shift_range(from..to, ((1.0 - shown) * self.s(12.0)).round(), 0.0);
            }
        }
        let right_start = if text_w > 0.0 { text_x - pad } else { right - self.s(2.0) };
        let radius = self.s(6.0);
        let (left, right) = (Rect::new(r.x, r.y, (left_end - r.x).max(0.0), r.h), Rect::new(right_start, r.y, (r.right() - right_start).max(0.0), r.h));
        let piece = |dl: &mut crate::render::DrawList, p: Rect, open_left: bool| {
            let shape = Rect::new(if open_left { p.x - radius } else { p.x }, p.y, p.w + radius, p.h + radius);
            dl.set_clip(Some(r));
            dl.round_rect(shape, t.chrome, radius);
            dl.outline(shape, t.separator, radius, 1.0);
            dl.set_clip(None);
        };
        g.dl.beneath(left_from, |dl| piece(dl, left, true));
        g.dl.beneath(right_from, |dl| piece(dl, right, false));
        self.layout.bar_pieces = [left, right];
    }
}

fn current_view(a: &App) -> Option<&crate::view::DirView> {
    let tab = a.tab();
    tab.view.as_ref().filter(|v| v.snap().gen == tab.gen && !tab.location().is_home_page())
}

#[cfg(test)]
#[path = "../../test/ui/bottom.rs"]
mod tests;
