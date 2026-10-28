use super::App;
use crate::render::{Color, Rect};
use crate::text::{FontStyle, Gfx, TextBackend};
use crate::theme::{ink_on, Theme};
use std::ops::Range;
use std::time::{Duration, Instant};

const POP: Duration = Duration::from_millis(120);

pub(super) const RADIUS: f32 = 6.0;
const BUTTON_RADIUS: f32 = 4.0;
const CAP_PAD: f32 = 6.0;
const CAP_GAP: f32 = 4.0;
const HINT_GAP: f32 = 18.0;
const DISABLED: u8 = 0x70;
const STEP_W: f32 = 34.0;
pub(super) const G_CHECK_MARK: &str = "\u{E73E}";
pub(super) const G_CROSS: &str = "\u{E8BB}";
pub(super) const G_NEXT: &str = "\u{E76C}";
pub(super) const G_OPEN: &str = "\u{E70D}";
pub(super) const G_MINUS: &str = "\u{E738}";
pub(super) const G_PLUS: &str = "\u{E710}";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Button {
    Idle,
    Hover,
    Open,
    Off,
}

impl Button {
    pub fn of(hover: bool, open: bool, enabled: bool) -> Self {
        match (enabled, open, hover) {
            (false, _, _) => Button::Off,
            (true, true, _) => Button::Open,
            (true, false, true) => Button::Hover,
            _ => Button::Idle,
        }
    }
}

pub(super) struct Paint<'a, B: TextBackend> {
    pub g: &'a mut Gfx<B>,
    pub t: &'a Theme,
    pub s: f32,
}

pub(super) fn baseline<B: TextBackend>(g: &mut Gfx<B>, r: Rect, font: FontStyle) -> f32 {
    let m = g.metrics(font);
    (r.y + (r.h + m.ascent - m.descent) / 2.0).round()
}

impl<'a, B: TextBackend> Paint<'a, B> {
    pub fn new(g: &'a mut Gfx<B>, t: &'a Theme, s: f32) -> Self {
        Self { g, t, s }
    }

    fn px(&self, v: f32) -> f32 {
        (v * self.s).round()
    }

    pub fn centered(&mut self, r: Rect, text: &str, font: FontStyle, color: Color) {
        let w = self.g.measure(text, font);
        let y = baseline(self.g, r, font);
        self.g.text((r.x + (r.w - w) / 2.0).round(), y, text, font, color, f32::INFINITY);
    }

    pub fn frame(&mut self, r: Rect) {
        let radius = self.px(RADIUS);
        self.g.dl.shadow(Rect::new(r.x, r.y + self.px(2.0), r.w, r.h), Color::rgba(0x0000_0050), radius, self.px(10.0));
        self.g.dl.round_rect(r, self.t.popup, radius);
        self.g.dl.outline(r, self.t.popup_border, radius, 1.0);
    }

    pub fn dashed(&mut self, r: Rect, color: Color) {
        let (dash, gap, inset) = (self.px(3.0).max(2.0), self.px(3.0).max(2.0), self.px(BUTTON_RADIUS));
        let mut x = r.x + inset;
        while x < r.right() - inset {
            let len = dash.min(r.right() - inset - x);
            self.g.dl.rect(Rect::new(x, r.y, len, 1.0), color);
            self.g.dl.rect(Rect::new(x, r.bottom() - 1.0, len, 1.0), color);
            x += dash + gap;
        }
        let mut y = r.y + inset;
        while y < r.bottom() - inset {
            let len = dash.min(r.bottom() - inset - y);
            self.g.dl.rect(Rect::new(r.x, y, 1.0, len), color);
            self.g.dl.rect(Rect::new(r.right() - 1.0, y, 1.0, len), color);
            y += dash + gap;
        }
    }

    pub fn separator(&mut self, x: f32, y: f32, w: f32) {
        self.g.dl.rect(Rect::new(x, y.round(), w, 1.0), self.t.separator);
    }

    pub fn fill_of(&self, state: Button) -> Option<Color> {
        match state {
            Button::Hover => Some(self.t.teal),
            Button::Open => Some(self.t.accent_fill),
            Button::Idle | Button::Off => None,
        }
    }

    pub fn ink_of(&self, state: Button, idle: Color) -> Color {
        match self.fill_of(state) {
            Some(fill) => ink_on(fill),
            None if state == Button::Off => self.t.text_dim.alpha(DISABLED),
            None => idle,
        }
    }

    pub fn icon_button(&mut self, r: Rect, glyph: &str, font: FontStyle, state: Button, idle: Color) {
        if let Some(fill) = self.fill_of(state) {
            self.g.dl.round_rect(r, fill, self.px(BUTTON_RADIUS));
        }
        let ink = self.ink_of(state, idle);
        self.centered(r, glyph, font, ink);
    }

    pub fn button(&mut self, r: Rect, label: &str, font: FontStyle, hot: Option<Color>) {
        let radius = self.px(BUTTON_RADIUS);
        self.g.dl.round_rect(r, hot.unwrap_or(self.t.button_pressed), radius);
        self.centered(r, label, font, hot.map_or(self.t.text, ink_on));
    }

    pub fn keycap_w(&mut self, text: &str, font: FontStyle) -> f32 {
        (self.g.measure(text, font) + 2.0 * self.px(CAP_PAD)).ceil()
    }

    pub fn keycap_lit(&mut self, x: f32, cy: f32, text: &str, font: FontStyle, lit: bool) -> f32 {
        let m = self.g.metrics(font);
        let w = self.keycap_w(text, font);
        let h = (m.ascent + m.descent + self.px(6.0)).round();
        let r = Rect::new(x.round(), (cy - h / 2.0).round(), w, h);
        let radius = self.px(BUTTON_RADIUS);
        let (fill, ink) = if lit { (self.t.button_hover, self.t.text) } else { (self.t.button_hover.alpha(0x50), self.t.text_dim) };
        self.g.dl.round_rect(r, fill, radius);
        self.g.dl.outline(r, self.t.popup_border, radius, 1.0);
        self.centered(r, text, font, ink);
        w
    }

    pub fn keycaps_w(&mut self, keys: &[String], font: FontStyle) -> f32 {
        let gap = self.px(CAP_GAP);
        keys.iter().map(|k| self.keycap_w(k, font) + gap).sum::<f32>() - if keys.is_empty() { 0.0 } else { gap }
    }

    pub fn keycaps(&mut self, x: f32, cy: f32, keys: &[String], font: FontStyle) -> f32 {
        self.keycaps_lit(x, cy, keys, font, true)
    }

    pub fn keycaps_lit(&mut self, mut x: f32, cy: f32, keys: &[String], font: FontStyle, lit: bool) -> f32 {
        let (start, gap) = (x, self.px(CAP_GAP));
        for k in keys {
            x += self.keycap_lit(x, cy, k, font, lit) + gap;
        }
        x - start - if keys.is_empty() { 0.0 } else { gap }
    }

    pub fn hints(&mut self, center: f32, cy: f32, items: &[(&[&str], &str)], font: FontStyle) {
        let caps: Vec<Vec<String>> = items.iter().map(|(keys, _)| keys.iter().map(|k| (*k).to_owned()).collect()).collect();
        let (gap, hint_gap) = (self.px(CAP_GAP), self.px(HINT_GAP));
        let widths: Vec<f32> = caps.iter().zip(items).map(|(k, (_, label))| self.keycaps_w(k, font) + gap + self.g.measure(label, font)).collect();
        let total = widths.iter().sum::<f32>() + hint_gap * (items.len().saturating_sub(1)) as f32;
        let mut x = (center - total / 2.0).round();
        let m = self.g.metrics(font);
        let y = (cy + (m.ascent - m.descent) / 2.0).round();
        for ((keys, (_, label)), w) in caps.iter().zip(items).zip(widths) {
            let kw = self.keycaps(x, cy, keys, font);
            self.g.text(x + kw + gap, y, label, font, self.t.text_dim, f32::INFINITY);
            x += w + hint_gap;
        }
    }

    pub fn checkbox(&mut self, r: Rect, on: bool) {
        let radius = self.px(3.0);
        if on {
            self.g.dl.round_rect(r, self.t.accent_fill, radius);
            self.g.dl.check(r, ink_on(self.t.accent_fill));
        } else {
            self.g.dl.outline(r, self.t.text_dim, radius, 1.0);
        }
    }

    pub fn radio(&mut self, r: Rect, on: bool, arrow: Option<(bool, FontStyle)>) {
        if !on {
            self.g.dl.ring(r, self.t.text_dim, 1.0);
            return;
        }
        self.g.dl.circle(r, self.t.accent_fill);
        let ink = ink_on(self.t.accent_fill);
        match arrow {
            Some((up, font)) => self.centered(r, if up { "\u{2191}" } else { "\u{2193}" }, font, ink),
            None => self.g.dl.circle(r.inset(r.w * 0.32, r.h * 0.32), ink),
        }
    }

    pub fn switch(&mut self, r: Rect, on: bool, hot: bool, icons: FontStyle) {
        let pad = self.px(3.0);
        let knob = r.h - 2.0 * pad;
        let track = if on { self.t.accent_fill } else { self.t.text_dim.alpha(0x60) };
        self.g.dl.round_rect(r, if hot { crate::theme::mix(track, ink_on(track), 0.12) } else { track }, r.h / 2.0);
        let kx = if on { r.right() - pad - knob } else { r.x + pad };
        let k = Rect::new(kx, r.y + pad, knob, knob);
        self.g.dl.circle(k, Color::rgb(0xFFFFFF));
        let mark = if on { (G_CHECK_MARK, self.t.accent_fill) } else { (G_CROSS, self.t.text_dim) };
        self.centered(k, mark.0, icons, mark.1);
    }

    pub fn slider(&mut self, track: Rect, frac: f32, hot: bool) {
        let frac = frac.clamp(0.0, 1.0);
        let vertical = track.h > track.w;
        let thick = track.w.min(track.h);
        self.g.dl.circle(track, self.t.text_dim.alpha(0x50));
        let (fill, center) = if vertical {
            let y = track.bottom() - track.h * frac;
            (Rect::new(track.x, y, track.w, track.bottom() - y), (track.x + track.w / 2.0, y))
        } else {
            let x = track.x + track.w * frac;
            (Rect::new(track.x, track.y, x - track.x, track.h), (x, track.y + track.h / 2.0))
        };
        self.g.dl.round_rect(fill, self.t.accent_fill, thick / 2.0);
        let knob = self.px(if hot { 16.0 } else { 14.0 });
        let k = Rect::new(center.0 - knob / 2.0, center.1 - knob / 2.0, knob, knob);
        self.g.dl.circle(k, Color::rgb(0xFFFFFF));
        self.g.dl.ring(k, self.t.accent_fill, self.px(2.0));
    }

    pub fn field(&mut self, r: Rect, focused: bool) {
        let radius = self.px(RADIUS);
        self.g.dl.round_rect(r, self.t.input_bg, radius);
        self.g.dl.outline(r, if focused { self.t.accent } else { self.t.input_border }, radius, if focused { self.s.round().max(1.0) } else { 1.0 });
    }

    pub fn stepper(&mut self, r: Rect, value: &str, font: FontStyle, icons: FontStyle, hot: Option<bool>) -> (Rect, Rect) {
        let radius = self.px(BUTTON_RADIUS);
        self.g.dl.round_rect(r, self.t.button_pressed, radius);
        let side = self.px(STEP_W).min(r.w / 3.0);
        let (minus, plus) = (Rect::new(r.x, r.y, side, r.h), Rect::new(r.right() - side, r.y, side, r.h));
        for (b, glyph, up) in [(minus, G_MINUS, false), (plus, G_PLUS, true)] {
            let state = if hot == Some(up) { Button::Hover } else { Button::Idle };
            self.icon_button(b, glyph, icons, state, self.t.text);
        }
        let line = self.t.separator;
        self.g.dl.rect(Rect::new(minus.right(), r.y + self.px(4.0), 1.0, r.h - self.px(8.0)), line);
        self.g.dl.rect(Rect::new(plus.x, r.y + self.px(4.0), 1.0, r.h - self.px(8.0)), line);
        self.centered(Rect::new(minus.right(), r.y, plus.x - minus.right(), r.h), value, font, self.t.text);
        (minus, plus)
    }

    pub fn picker(&mut self, r: Rect, value: &str, font: FontStyle, icons: FontStyle, open: bool, hot: bool) {
        let radius = self.px(BUTTON_RADIUS);
        let fill = if open { self.t.accent_fill } else if hot { self.t.teal } else { self.t.button_pressed };
        self.g.dl.round_rect(r, fill, radius);
        let ink = if open || hot { ink_on(fill) } else { self.t.text };
        let arrow = self.px(22.0);
        self.centered(Rect::new(r.x + arrow, r.y, r.w - 2.0 * arrow, r.h), value, font, ink);
        self.centered(Rect::new(r.right() - arrow, r.y, arrow, r.h), if open { G_OPEN } else { G_NEXT }, icons, ink);
    }

    pub fn marked(&mut self, (x, y): (f32, f32), text: &str, ranges: &[Range<usize>], font: FontStyle, (color, mark): (Color, Color), max_w: f32) -> f32 {
        if ranges.is_empty() || self.g.measure(text, font) > max_w {
            return self.g.text(x, y, text, font, color, max_w);
        }
        let mut at = 0;
        let mut cx = x;
        for r in ranges.iter().filter(|r| r.end <= text.len() && text.is_char_boundary(r.start) && text.is_char_boundary(r.end)) {
            if r.start > at {
                cx += self.g.text(cx, y, &text[at..r.start], font, color, f32::INFINITY);
            }
            cx += self.g.text(cx, y, &text[r.start..r.end], font, mark, f32::INFINITY);
            at = r.end;
        }
        if at < text.len() {
            cx += self.g.text(cx, y, &text[at..], font, color, f32::INFINITY);
        }
        cx - x
    }
}

impl App {
    pub(super) fn pop<B: TextBackend>(&mut self, g: &mut Gfx<B>, from: usize, since: Instant, center: (f32, f32)) {
        if !self.animations {
            return;
        }
        let now = self.last_frame.unwrap_or(since);
        let t = now.saturating_duration_since(since).as_secs_f32() / POP.as_secs_f32();
        if t < 1.0 {
            g.dl.pop_in(from, t, center);
            self.motion = true;
        }
    }
}

#[cfg(test)]
#[path = "../../test/ui/widgets.rs"]
mod tests;
