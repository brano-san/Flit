use super::archive::Kind;
use super::form::{Control, Form};
use super::{widgets, App, Glide, Hit};
use crate::input::{Key, Mods};
use crate::render::Rect;
use crate::text::{FontFamily, FontStyle, Gfx, TextBackend};
use crate::theme::Theme;
use std::time::Instant;

#[derive(Default)]
pub(super) struct Document {
    pub scroll: Glide,
    pub viewport: Rect,
    pub height: f32,
    pub grab: Option<f32>,
    pub copied: Option<(String, Instant)>,
    width: f32,
    scale: f32,
    lines: Vec<Line>,
    rows: Vec<(usize, f32, f32)>,
    pointer: (f32, f32),
    button_widths: Vec<f32>,
    font: String,
}

fn dialog_scale(w: f32, h: f32, scale: f32) -> f32 {
    scale.min((w / 360.0).max(0.1)).min((h / 320.0).max(0.1))
}

struct Line {
    text: String,
    top: f32,
    heading: bool,
    indent: bool,
    bullet: bool,
}

impl Document {
    pub fn max(&self) -> f32 {
        (self.height - self.viewport.h).max(0.0)
    }

    fn track(&self) -> Rect {
        Rect::new(self.viewport.right() - 10.0 * self.scale, self.viewport.y, 10.0 * self.scale, self.viewport.h)
    }

    fn thumb(&self) -> Rect {
        let track = self.track();
        let h = (track.h * track.h / self.height.max(1.0)).max(24.0 * self.scale).min(track.h);
        Rect::new(track.x + 2.0 * self.scale, track.y + (track.h - h) * self.scroll.pos / self.max().max(1.0), 6.0 * self.scale, h)
    }
}

impl Form {
    pub(super) fn layout_update(&mut self, w: f32, h: f32, sc: f32) {
        let sc = dialog_scale(w, h, sc);
        let s = |v: f32| (v * sc).round();
        let fw = s(760.0).min((w - s(24.0)).max(1.0));
        let room = (fw - s(40.0)).max(1.0);
        let widths: Vec<f32> = if self.document.button_widths.len() == self.buttons.len() {
            self.document.button_widths.clone()
        } else {
            self.buttons.iter().map(|b| s(b.chars().map(|c| if c >= '\u{2E80}' { 13.0 } else { 7.5 }).sum::<f32>() + 32.0).max(s(112.0))).collect()
        };
        let stacked = widths.iter().sum::<f32>() + s(8.0) * widths.len().saturating_sub(1) as f32 > room;
        let button_rows = if stacked && widths.len() > 1 { 2.0 } else { 1.0 };
        let fh = s(640.0).min((h - s(24.0)).max(1.0));
        let x = ((w - fw) / 2.0).round();
        let y = ((h - fh) / 2.0).round();
        self.rect = Rect::new(x, y, fw, fh);
        self.rows = (0..self.controls.len()).map(|k| if k < 2 { Rect::new(x + s(20.0), y + s(52.0 + k as f32 * 28.0), room, s(28.0)) } else { Rect::default() }).collect();
        let top = self.rect.bottom() - s(20.0) - s(40.0) * button_rows + s(8.0);
        self.button_rects.clear();
        if stacked && widths.len() > 1 {
            self.button_rects.push(Rect::new(x + s(20.0), top, room, s(32.0)));
            let n = widths.len() - 1;
            let bw = (room - s(8.0) * (n - 1) as f32) / n as f32;
            for k in 0..n {
                self.button_rects.push(Rect::new(x + s(20.0) + k as f32 * (bw + s(8.0)), top + s(40.0), bw, s(32.0)));
            }
        } else {
            let mut right = self.rect.right() - s(20.0);
            for width in widths.iter().rev() {
                let width = width.min(room);
                right -= width;
                self.button_rects.push(Rect::new(right, top, width, s(32.0)));
                right -= s(8.0);
            }
            self.button_rects.reverse();
        }
        self.document.viewport = Rect::new(x + s(20.0), y + s(120.0), room, (top - s(64.0) - y - s(120.0)).max(0.0));
        self.document.scroll.clamp(self.document.max());
    }
}

fn wrapped<B: TextBackend>(g: &mut Gfx<B>, text: &str, font: FontStyle, width: f32) -> Vec<String> {
    let mut out = Vec::new();
    for line in super::settings_page::wrap(g, text, font, width) {
        if g.measure(line, font) <= width {
            out.push(line.to_owned());
            continue;
        }
        let mut start = 0;
        let mut end = 0;
        for (at, ch) in line.char_indices() {
            if at > start && g.measure(&line[start..at + ch.len_utf8()], font) > width {
                out.push(line[start..at].to_owned());
                start = at;
            }
            end = at + ch.len_utf8();
        }
        if end > start {
            out.push(line[start..end].to_owned());
        }
    }
    out
}

impl App {
    fn update_document(&mut self) -> Option<&mut Document> {
        if !self.jobs.asks.is_empty() {
            return None;
        }
        self.jobs.form.as_mut().filter(|f| matches!(f.kind, Kind::Update(_))).map(|f| &mut f.document)
    }

    pub(super) fn update_wheel(&mut self, dy: f32) -> bool {
        let step = self.s(48.0);
        let Some(d) = self.update_document() else { return false };
        d.scroll.by(-dy * step, d.max());
        self.dirty = true;
        true
    }

    pub(super) fn update_scroll_key(&mut self, key: Key, mods: Mods) -> bool {
        if mods.ctrl || mods.alt {
            return false;
        }
        let Some(d) = self.update_document() else { return false };
        match key {
            Key::PageDown => d.scroll.by(d.viewport.h * 0.9, d.max()),
            Key::PageUp => d.scroll.by(-d.viewport.h * 0.9, d.max()),
            Key::Home => d.scroll.snap(0.0),
            Key::End => d.scroll.snap(d.max()),
            _ => return false,
        }
        self.dirty = true;
        true
    }

    pub(super) fn update_scroll_down(&mut self, x: f32, y: f32) -> bool {
        let Some(d) = self.update_document() else { return false };
        if d.max() <= 0.0 || !d.track().contains(x, y) {
            return false;
        }
        let thumb = d.thumb();
        if thumb.contains(x, y) {
            d.grab = Some(y - thumb.y);
        } else {
            d.scroll.by(if y < thumb.y { -d.viewport.h } else { d.viewport.h }, d.max());
        }
        self.dirty = true;
        true
    }

    pub(super) fn update_scroll_move(&mut self, x: f32, y: f32) -> bool {
        let Some(d) = self.update_document() else { return false };
        d.pointer = (x, y);
        let Some(grab) = d.grab else { return false };
        let track = d.track();
        let fraction = ((y - grab - track.y) / (track.h - d.thumb().h).max(1.0)).clamp(0.0, 1.0);
        d.scroll.snap(fraction * d.max());
        self.dirty = true;
        true
    }

    pub(super) fn draw_update_dialog<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme) -> bool {
        let sc = dialog_scale(self.size.0, self.size.1, self.scale);
        let s = |v: f32| (v * sc).round();
        let font = FontStyle::new(FontFamily::Ui, 13.0 * sc, false);
        let heading = FontStyle::new(FontFamily::Ui, 14.0 * sc, true);
        let title_font = FontStyle::new(FontFamily::Ui, 16.0 * sc, true);
        let lit = [self.lit(Hit::Form(0)), self.lit(Hit::Form(1))];
        let Some(f) = &mut self.jobs.form else { return false };
        if !matches!(f.kind, Kind::Update(_)) {
            return false;
        }
        let widths: Vec<f32> = f.buttons.iter().map(|label| (g.measure(label, font) + s(32.0)).ceil().max(s(112.0))).collect();
        if f.document.button_widths != widths {
            f.document.button_widths = widths;
            f.layout_update(self.size.0, self.size.1, self.scale);
        }
        let Kind::Update(update) = &f.kind else { return false };
        let r = f.rect;
        g.dl.round_rect(r, t.bg, s(8.0));
        g.dl.outline(r, t.popup_border, s(8.0), sc.max(1.0));
        let ascent = g.metrics(title_font).ascent;
        g.text(r.x + s(20.0), r.y + s(20.0) + ascent, &f.title, title_font, t.text, r.w - s(40.0));
        for (k, glow) in lit.into_iter().enumerate() {
            let row = f.rows[k];
            if glow > 0.0 {
                g.dl.round_rect(row, widgets::faded(t.hover, glow), s(4.0));
            }
            if f.focus == k {
                g.dl.outline(row, t.input_border, s(4.0), sc.max(1.0));
            }
            if let Control::Value { label, value } = &f.controls[k] {
                let base = row.y + (row.h + g.metrics(font).ascent - g.metrics(font).descent) / 2.0;
                let split = row.x + row.w * 0.65;
                g.text(row.x + s(8.0), base, label, font, t.text_dim, split - row.x - s(16.0));
                g.text(split, base, value, font, if k == 1 { t.accent } else { t.text }, row.right() - split - s(8.0));
            }
        }
        let d = &mut f.document;
        let width = (d.viewport.w - s(24.0)).max(1.0);
        if d.width != width || d.scale != sc || d.font != self.look.font {
            d.width = width;
            d.scale = sc;
            d.font.clone_from(&self.look.font);
            d.lines.clear();
            d.rows.clear();
            let headings: Vec<String> = update.release.notes.lines().filter(|l| l.trim_start().starts_with('#')).map(super::update::plain).collect();
            let mut top = 0.0;
            for (k, c) in f.controls.iter().enumerate().skip(2) {
                let Control::Line(text) = c else { continue };
                let is_heading = headings.contains(text);
                let bullet = text.starts_with("• ");
                if text.is_empty() {
                    top += s(8.0);
                    continue;
                }
                if is_heading && top > 0.0 {
                    top += s(12.0);
                }
                let start = top;
                let face = if is_heading { heading } else { font };
                let content = text.strip_prefix("• ").unwrap_or(text);
                for (i, text) in wrapped(g, content, face, width - if bullet { s(18.0) } else { 0.0 }).into_iter().enumerate() {
                    d.lines.push(Line { text, top, heading: is_heading, indent: bullet, bullet: bullet && i == 0 });
                    top += s(if is_heading { 24.0 } else { 20.0 });
                }
                d.rows.push((k, start, top - start));
                top += s(4.0);
            }
            d.height = top;
        }
        d.scroll.clamp(d.max());
        for &(k, top, height) in &d.rows {
            let y = d.viewport.y + top - d.scroll.pos;
            let start = y.max(d.viewport.y);
            f.rows[k] = Rect::new(d.viewport.x, start, width, ((y + height).min(d.viewport.bottom()) - start).max(0.0));
        }
        g.dl.set_clip(Some(d.viewport));
        for line in &d.lines {
            let top = d.viewport.y + line.top - d.scroll.pos;
            if top + s(24.0) < d.viewport.y || top > d.viewport.bottom() {
                continue;
            }
            let face = if line.heading { heading } else { font };
            let base = top + g.metrics(face).ascent;
            if line.bullet {
                g.text(d.viewport.x, base, "•", font, t.text_dim, s(16.0));
            }
            let indent = if line.indent { s(18.0) } else { 0.0 };
            g.text(d.viewport.x + indent, base, &line.text, face, t.text, width - indent + 1.0);
        }
        if d.max() > 0.0 {
            let thumb = d.thumb();
            g.dl.round_rect(thumb, if d.grab.is_some() || d.track().contains(d.pointer.0, d.pointer.1) { t.scrollbar_hover } else { t.scrollbar }, thumb.w / 2.0);
        }
        g.dl.set_clip(None);
        let footer = d.viewport.bottom() + s(12.0);
        g.dl.rect(Rect::new(d.viewport.x, footer, d.viewport.w, sc.max(1.0)), t.separator);
        if let Some(progress) = f.progress {
            let bar = Rect::new(d.viewport.x, footer + s(4.0), d.viewport.w, s(3.0));
            g.dl.round_rect(bar, t.separator, s(1.5));
            g.dl.round_rect(Rect::new(bar.x, bar.y, bar.w * progress.clamp(0.0, 1.0), bar.h), t.accent, s(1.5));
        }
        let now = self.last_frame.unwrap_or_else(Instant::now);
        let copied = d.copied.as_ref().filter(|(_, at)| now.saturating_duration_since(*at).as_secs_f32() < 3.0);
        let (status, color) = if let Some((text, at)) = copied {
            let alpha = ((3.0 - now.saturating_duration_since(*at).as_secs_f32()).clamp(0.0, 1.0) * 255.0) as u8;
            self.motion = true;
            (text.as_str(), t.accent.alpha(alpha))
        } else {
            (f.note.as_str(), if f.error { t.error } else { t.text_dim })
        };
        let ascent = g.metrics(font).ascent;
        for (i, line) in wrapped(g, status, font, d.viewport.w).into_iter().take(2).enumerate() {
            g.text(d.viewport.x, footer + s(10.0 + i as f32 * 17.0) + ascent, &line, font, color, d.viewport.w + 1.0);
        }
        for (b, (rect, label)) in f.button_rects.iter().zip(&f.buttons).enumerate() {
            let hit = Hit::Form(f.controls.len() + b);
            let hot = self.hover == hit || self.pressed == hit;
            let primary = b == 0 && update.stage != super::update::Stage::Busy;
            g.dl.round_rect(*rect, if primary { if hot { t.accent.alpha(0xD0) } else { t.accent } } else if hot { t.button_pressed } else { t.button_hover }, s(4.0));
            if !primary || f.focus == f.controls.len() + b {
                g.dl.outline(*rect, if f.focus == f.controls.len() + b { t.text_dim } else { t.input_border }, s(4.0), sc.max(1.0));
            }
            super::centered(g, *rect, label, font, if primary { crate::theme::ink_on(t.accent) } else { t.text });
        }
        true
    }
}

#[cfg(test)]
#[path = "../../test/ui/update_dialog.rs"]
mod tests;
