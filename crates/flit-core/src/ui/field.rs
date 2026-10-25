use super::edit::{EditAction, LineEdit};
use crate::input::{Key, Mods};
use crate::render::Rect;
use crate::text::{FontStyle, Gfx, TextBackend};
use crate::theme::Theme;

#[derive(Default, Debug)]
pub(super) struct Field {
    pub edit: LineEdit,
    scroll: f32,
    click: Option<f32>,
}

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
    pub fn draw<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme, inner: Rect, font: FontStyle, focused: bool, placeholder: &str, scale: f32) {
        let m = g.metrics(font);
        let baseline = (inner.y + (inner.h + m.ascent - m.descent) / 2.0).round();
        let line_h = (m.ascent + m.descent + 4.0 * scale).min(inner.h);
        let line_y = (inner.y + (inner.h - line_h) / 2.0).round();
        g.dl.set_clip(Some(inner));
        if let Some(x) = self.click.take() {
            let pos = g.hit(&self.edit.text, font, x - inner.x + self.scroll);
            self.edit.move_to(pos, false);
        }
        let caret_x = g.caret_x(&self.edit.text, font, self.edit.caret);
        if caret_x - self.scroll > inner.w {
            self.scroll = caret_x - inner.w;
        } else if caret_x < self.scroll {
            self.scroll = caret_x;
        }
        let x0 = inner.x - self.scroll;
        if self.edit.text.is_empty() {
            g.text(inner.x, baseline, placeholder, font, t.text_dim, inner.w);
        } else {
            let sel = self.edit.selection();
            if focused && !sel.is_empty() {
                let a = g.caret_x(&self.edit.text, font, sel.start);
                let b = g.caret_x(&self.edit.text, font, sel.end);
                g.dl.rect(Rect::new(x0 + a, line_y, b - a, line_h), t.text_selection);
            }
            g.text(x0, baseline, &self.edit.text, font, t.text, f32::INFINITY);
        }
        if focused {
            g.dl.rect(Rect::new(x0 + caret_x, line_y, scale.round().max(1.0), line_h), t.text);
        }
        g.dl.set_clip(None);
    }
}
