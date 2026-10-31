use super::edit::{EditAction, LineEdit};
use crate::input::{Key, Mods, Preedit};
use std::ops::Range;
use crate::render::Rect;
use crate::text::{FontStyle, Gfx, TextBackend};
use crate::theme::Theme;

#[derive(Default, Debug)]
pub(super) struct Field {
    pub edit: LineEdit,
    pub secret: bool,
    scroll: f32,
    click: Option<f32>,
}

const MASK: char = '\u{2022}';

pub(super) enum FieldKey {
    Ignored,
    Edited { changed: bool, action: EditAction },
}

impl Field {
    pub fn text(&self) -> &str {
        &self.edit.text
    }

    pub fn click(&mut self, x: f32, clicks: u8) {
        if clicks >= 2 {
            self.edit.select_all();
            self.click = None;
        } else {
            self.click = Some(x);
        }
    }

    pub fn key(&mut self, key: Key, m: Mods) -> FieldKey {
        let editing = matches!(key, Key::Left | Key::Right | Key::Home | Key::End | Key::Backspace | Key::Delete)
            || (m.ctrl && !m.alt && matches!(key, Key::Char('A' | 'C' | 'X' | 'V')));
        if !editing {
            return FieldKey::Ignored;
        }
        let before = self.edit.text.len();
        let action = self.edit.key(key, m);
        let changed = action == EditAction::Changed || self.edit.text.len() != before;
        FieldKey::Edited { changed, action }
    }

    pub fn insert(&mut self, s: &str) {
        self.edit.insert(s);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme, inner: Rect, font: FontStyle, focused: bool, placeholder: &str, scale: f32, preedit: Option<&Preedit>) {
        let m = g.metrics(font);
        let baseline = (inner.y + (inner.h + m.ascent - m.descent) / 2.0).round();
        let line_h = (m.ascent + m.descent + 4.0 * scale).min(inner.h);
        let line_y = (inner.y + (inner.h - line_h) / 2.0).round();
        g.dl.set_clip(Some(inner));
        let shown: String = if self.secret { self.edit.text.chars().map(|_| MASK).collect() } else { self.edit.text.clone() };
        if let Some(x) = self.click.take() {
            let pos = g.hit(&shown, font, x - inner.x + self.scroll);
            let text = &self.edit.text;
            let pos = if self.secret { text.char_indices().nth(pos / MASK.len_utf8()).map_or(text.len(), |(i, _)| i) } else { pos };
            self.edit.move_to(pos, false);
        }
        let text = &self.edit.text;
        let to_shown = |b: usize| if self.secret { text[..b].chars().count() * MASK.len_utf8() } else { b };
        let sel = self.edit.selection();
        let line = EditLine { text: &shown, caret: to_shown(self.edit.caret), selection: to_shown(sel.start)..to_shown(sel.end) };
        let composing = preedit.filter(|p| focused && !self.secret && !p.is_empty());
        if self.edit.text.is_empty() && composing.is_none() {
            self.scroll = 0.0;
            if focused {
                g.dl.rect(Rect::new(inner.x, line_y, scale.round().max(1.0), line_h), t.text);
            } else {
                g.text(inner.x, baseline, placeholder, font, t.text_dim, inner.w);
            }
        } else {
            let look = EditLook { font, baseline, line_y, line_h, stroke: scale.round().max(1.0), focused };
            draw_line(g, t, inner, &mut self.scroll, line, composing, look);
        }
        g.dl.set_clip(None);
    }
}

pub(super) struct EditLine<'a> {
    pub text: &'a str,
    pub caret: usize,
    pub selection: Range<usize>,
}

#[derive(Clone, Copy)]
pub(super) struct EditLook {
    pub font: FontStyle,
    pub baseline: f32,
    pub line_y: f32,
    pub line_h: f32,
    pub stroke: f32,
    pub focused: bool,
}

pub(super) fn compose(line: &EditLine<'_>, preedit: &Preedit) -> (String, usize, Range<usize>) {
    let at = line.caret.min(line.text.len());
    let mut text = String::with_capacity(line.text.len() + preedit.text.len());
    text.push_str(&line.text[..at]);
    text.push_str(&preedit.text);
    text.push_str(&line.text[at..]);
    (text, at + preedit.caret(), at..at + preedit.text.len())
}

pub(super) fn draw_line<B: TextBackend>(g: &mut Gfx<B>, t: &Theme, inner: Rect, scroll: &mut f32, line: EditLine<'_>, preedit: Option<&Preedit>, look: EditLook) {
    let font = look.font;
    let (owned, caret, selection, underline) = match preedit {
        Some(p) => {
            let (text, caret, underline) = compose(&line, p);
            (Some(text), caret, 0..0, Some(underline))
        }
        None => (None, line.caret, line.selection.clone(), None),
    };
    let text = owned.as_deref().unwrap_or(line.text);
    let caret_x = g.caret_x(text, font, caret);
    if caret_x - *scroll > inner.w {
        *scroll = caret_x - inner.w;
    } else if caret_x < *scroll {
        *scroll = caret_x;
    }
    let x0 = inner.x - *scroll;
    if look.focused && !selection.is_empty() {
        let a = g.caret_x(text, font, selection.start);
        let b = g.caret_x(text, font, selection.end);
        g.dl.rect(Rect::new(x0 + a, look.line_y, b - a, look.line_h), t.text_selection);
    }
    g.text(x0, look.baseline, text, font, t.text, f32::INFINITY);
    if let Some(u) = underline {
        let a = g.caret_x(text, font, u.start);
        let b = g.caret_x(text, font, u.end);
        g.dl.rect(Rect::new(x0 + a, look.baseline + 2.0 * look.stroke, b - a, look.stroke), t.text);
    }
    if look.focused {
        g.dl.rect(Rect::new(x0 + caret_x, look.line_y, look.stroke, look.line_h), t.text);
    }
}
