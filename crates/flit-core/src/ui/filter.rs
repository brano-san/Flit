use super::edit::EditAction;
use super::field::{Field, FieldKey};
use super::{centered, App, Effect, Focus, Hit, Tab, G_CLOSE};
use crate::input::{Key, Mods};
use crate::render::Rect;
use crate::text::{Gfx, TextBackend};
use crate::theme::Theme;

const BAR_H: f32 = 38.0;
const BOX_W: f32 = 320.0;
const G_FILTER: &str = "\u{E71C}";

#[derive(Default)]
pub(super) struct FilterRects {
    pub bar: Rect,
    pub field: Rect,
    inner: Rect,
    close: Rect,
}

impl Tab {
    pub(super) fn reset_filter(&mut self) {
        self.filter = None;
        if let Some(v) = self.view.as_mut().filter(|v| v.is_filtered()) {
            v.set_filter("");
        }
    }

    pub(super) fn refilter_view(&mut self) {
        if let (Some(v), Some(f)) = (&mut self.view, &self.filter) {
            v.set_filter(f.text());
        }
    }
}

impl App {
    pub(super) fn filter_bar_h(&self) -> f32 {
        if self.tab().filter.is_some() {
            self.s(BAR_H)
        } else {
            0.0
        }
    }

    pub(super) fn open_filter(&mut self) {
        if self.editing {
            self.cancel_edit();
        }
        self.tab_mut().filter.get_or_insert_with(Field::default).edit.select_all();
        self.focus = Focus::Filter;
        self.layout();
        self.dirty = true;
    }

    pub(super) fn close_filter(&mut self) {
        self.tab_mut().reset_filter();
        self.reveal_focus(true);
        if self.focus == Focus::Filter {
            self.focus = Focus::List;
        }
        self.layout();
        self.dirty = true;
    }

    fn filter_changed(&mut self) {
        let t = self.tab_mut();
        t.refilter_view();
        t.scroll = 0.0;
        t.scroll_target = 0.0;
        self.dirty = true;
    }

    pub(super) fn filter_key(&mut self, key: Key, m: Mods) -> bool {
        let Some(f) = self.tabs[self.cur].filter.as_mut() else {
            self.focus = Focus::List;
            return false;
        };
        match key {
            Key::Escape => self.close_filter(),
            Key::Enter => {
                self.focus = Focus::List;
                let t = self.tab_mut();
                if t.view.as_ref().is_some_and(|v| v.focus.is_none()) {
                    t.move_focus(1, Mods::default());
                    self.reveal_focus(false);
                }
            }
            Key::Up | Key::Down | Key::PageUp | Key::PageDown => {
                self.focus = Focus::List;
                return false;
            }
            _ => match f.key(key, m) {
                FieldKey::Edited { changed, action } => {
                    match action {
                        EditAction::Copy(s) => self.effects.push(Effect::SetClipboard(s)),
                        EditAction::Paste => self.effects.push(Effect::Paste),
                        _ => {}
                    }
                    if changed {
                        self.filter_changed();
                    }
                }
                FieldKey::Ignored => return !m.ctrl && !m.alt && !matches!(key, Key::F(_) | Key::Tab),
            },
        }
        self.dirty = true;
        true
    }

    pub(super) fn filter_insert(&mut self, s: &str) -> bool {
        let Some(f) = self.tabs[self.cur].filter.as_mut() else { return false };
        f.insert(s);
        self.filter_changed();
        true
    }

    pub(super) fn filter_click(&mut self, x: f32, clicks: u8) {
        if let Some(f) = self.tabs[self.cur].filter.as_mut() {
            f.click(x, clicks);
            self.focus = Focus::Filter;
        }
    }

    pub(super) fn layout_filter(&mut self, x: f32, top: f32) {
        let (w, h) = (self.size.0 - x, self.filter_bar_h());
        let sc = self.scale;
        let s = |v: f32| (v * sc).round();
        let r = &mut self.nav.filter;
        if h <= 0.0 {
            *r = FilterRects::default();
            return;
        }
        r.bar = Rect::new(x, top, w, h);
        r.field = Rect::new(x + s(10.0), top + s(5.0), s(BOX_W).min(w - s(20.0)).max(0.0), h - s(10.0));
        r.close = Rect::new(r.field.right() - s(26.0), r.field.y + ((r.field.h - s(22.0)) / 2.0).round(), s(22.0), s(22.0));
        r.inner = Rect::new(r.field.x + s(30.0), r.field.y, (r.close.x - s(4.0) - r.field.x - s(30.0)).max(0.0), r.field.h);
    }

    pub(super) fn filter_hit(&self, x: f32, y: f32) -> Option<Hit> {
        let r = &self.nav.filter;
        if !r.bar.contains(x, y) {
            return None;
        }
        if r.close.contains(x, y) {
            return Some(Hit::FilterClose);
        }
        Some(if r.field.contains(x, y) { Hit::FilterField } else { Hit::None })
    }

    pub(super) fn draw_filter<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme) {
        let r = &self.nav.filter;
        if r.bar.h <= 0.0 {
            return;
        }
        let (bar, field, inner, close) = (r.bar, r.field, r.inner, r.close);
        let focused = self.active && self.focus == Focus::Filter;
        let (s4, scale) = (self.s(4.0), self.scale);
        let font = self.ui_font(13.0);
        g.dl.rect(bar, t.bg);
        g.dl.round_rect(field, t.input_bg, s4);
        g.dl.outline(field, if focused { t.accent } else { t.input_border }, s4, scale.round().max(1.0));
        centered(g, Rect::new(field.x + s4, field.y, self.s(24.0), field.h), G_FILTER, self.icon_font(12.0), t.text_dim);
        if self.hover == Hit::FilterClose {
            g.dl.round_rect(close, t.button_hover, s4);
        }
        centered(g, close, G_CLOSE, self.icon_font(8.0), t.text_dim);
        if let Some(f) = self.tabs[self.cur].filter.as_mut() {
            f.draw(g, t, inner, font, focused, "Filter by name", scale);
        }
    }
}
