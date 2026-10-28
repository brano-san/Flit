use super::layouts::{layout_of_zoom, step_zoom};
use super::{centered, App, Hit, BIN_BUTTONS, G_BRANCH};
use crate::fmt as format;
use crate::i18n::{P, S};
use crate::render::Rect;
use crate::sort::Folders;
use crate::text::{Gfx, TextBackend};
use crate::theme::Theme;
use bumpalo::Bump;

const ELAPSED_MIN_MS: u128 = 500;
const BUTTON_W: f32 = 26.0;
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
                let r = self.layout.bar.iter().find(|x| x.0 == BottomHit::Layout).map_or(Rect::default(), |x| x.1);
                let menu = self.build_layout_menu();
                self.flit_menu(r.x, r.y, menu);
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
        if on {
            g.dl.round_rect(r, t.accent.alpha(0x40), s3);
        } else if self.hover == Hit::Bottom(b) {
            g.dl.round_rect(r, t.button_hover, s3);
        }
        centered(g, r, glyph, self.icon_font(12.0), if on { t.text } else { t.text_dim });
        self.layout.bar.push((b, r));
    }

    fn counter<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme, right: f32, b: BottomHit, label: &str) -> f32 {
        let r = self.layout.status;
        let spec = self.tab().view_spec;
        let (glyph, hidden) = if b == BottomHit::Folders { (G_FOLDER, spec.hide_dirs) } else { (G_FILE, spec.hide_files) };
        let font = self.ui_font(12.0);
        let (pad, icon) = (self.s(6.0), self.s(16.0));
        let w = g.measure(label, font) + icon + 3.0 * pad;
        let br = Rect::new(right - w, r.y + self.s(2.0), w, r.h - self.s(4.0));
        if self.hover == Hit::Bottom(b) {
            g.dl.round_rect(br, t.button_hover, self.s(3.0));
        }
        let ir = Rect::new(br.x + pad, br.y, icon, br.h);
        centered(g, ir, glyph, self.icon_font(12.0), t.text_dim);
        if hidden {
            centered(g, ir, G_HIDDEN, self.icon_font(14.0), t.error);
        }
        let m = g.metrics(font);
        let baseline = (r.y + (r.h + m.ascent - m.descent) / 2.0).round();
        g.text(ir.right() + pad, baseline, label, font, if hidden { t.text_dim.alpha(0x90) } else { t.text_dim }, f32::INFINITY);
        self.layout.bar.push((b, br));
        br.x - self.s(2.0)
    }

    pub(super) fn draw_status<B: TextBackend>(&mut self, g: &mut Gfx<B>, arena: &Bump, t: &Theme) {
        let r = self.layout.status;
        self.layout.bar.clear();
        g.dl.rect(r, t.chrome);
        g.dl.rect(Rect::new(r.x, r.y, r.w, 1.0), t.separator);
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
            g.dl.round_rect(zr, if self.hover == Hit::Bottom(BottomHit::Zoom) { t.button_hover } else { t.input_bg }, self.s(3.0));
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
            right = self.counter(g, t, right, BottomHit::Files, arena.alloc_str(&files));
            right = self.counter(g, t, right, BottomHit::Folders, arena.alloc_str(&dirs));
        }
        if let Some(done) = self.tab().stats.done.filter(|d| d.as_millis() > ELAPSED_MIN_MS) {
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
        }

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
            if self.hover == if in_bin { Hit::Bin(k) } else { Hit::Encoding } {
                g.dl.round_rect(b, t.button_hover, self.s(3.0));
            }
            g.text(b.x + self.s(8.0), baseline, label, font, t.text_dim, f32::INFINITY);
            right -= self.s(4.0);
        }
        self.layout.encoding = if in_bin { Rect::default() } else { buttons[0] };
        self.layout.bin = if in_bin { buttons } else { Default::default() };
        self.layout.branch = Rect::default();
        if let Some(branch) = self.git_branch().filter(|b| !b.is_empty() && !in_bin) {
            let label = arena.alloc_str(&format!("{G_BRANCH} {branch}"));
            let w = g.measure(label, font) + self.s(16.0);
            right -= w;
            let b = Rect::new(right, r.y + self.s(2.0), w, r.h - self.s(4.0));
            if self.hover == Hit::Branch {
                g.dl.round_rect(b, t.button_hover, self.s(3.0));
            }
            g.text(b.x + self.s(8.0), baseline, label, font, t.text_dim, f32::INFINITY);
            self.layout.branch = b;
        }

        self.fit_filter(right - 2.0 * bw - pad - self.s(6.0));
        self.draw_filter(g, t);
        let mut x = self.nav.filter.field.right() + self.s(4.0);
        let (subfolders, retain) = (self.tab().subfolders, self.prefs.retain_filter);
        self.bar_button(g, t, br(x), BottomHit::Subfolders, G_SUBFOLDERS, subfolders);
        x += bw + self.s(2.0);
        self.bar_button(g, t, br(x), BottomHit::Retain, G_RETAIN, retain);
        x += bw + pad;

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
        g.text(x, baseline, &s, font, color, (right - x - pad).max(0.0));
    }
}

fn current_view(a: &App) -> Option<&crate::view::DirView> {
    let tab = a.tab();
    tab.view.as_ref().filter(|v| v.snap().gen == tab.gen && !tab.location().is_home_page())
}

#[cfg(test)]
#[path = "../../test/ui/bottom.rs"]
mod tests;
