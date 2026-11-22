use super::edit::EditAction;
use super::field::{Field, FieldKey};
use super::{centered, App, Effect, Focus, Hit, Tab, G_CLOSE};
use crate::input::{Key, Mods};
use crate::render::Rect;
use crate::text::{Gfx, TextBackend};
use crate::theme::Theme;

use crate::filter::{token_ranges, ATTRIBUTE_KEYS, KIND_VALUES};
use crate::query_attrs::Clock;
use std::ops::Range;

const MAX_SAVED: usize = 64;
const MAX_FILTER_LEN: usize = 4096;

fn filter_completions(text: &str, caret: usize, tags: &[crate::tags::Tag]) -> (Range<usize>, Vec<String>) {
    let range = token_ranges(text).into_iter().find(|r| r.start <= caret && caret <= r.end).unwrap_or(caret..caret);
    let raw = &text[range.start..caret];
    let (sign, token) = raw.strip_prefix('!').map_or(("", raw), |t| ("!", t));
    let (key, value) = token.split_once(':').unwrap_or((token, ""));
    let prefix = value.trim_start_matches('"').to_lowercase();
    let suggestions = if token.contains(':') {
        let values: &[&str] = match key.to_ascii_lowercase().as_str() {
            "ext" => &["jpg", "png", "pdf", "txt", "zip"],
            "kind" => KIND_VALUES,
            "size" => &[">10M", "<1M", "1M..100M"],
            "modified" | "created" => &["today", "yesterday", "week", "month", "year"],
            "hidden" => &["true", "false"],
            _ => &[],
        };
        if key.eq_ignore_ascii_case("tag") {
            tags.iter().filter(|tag| tag.name.to_lowercase().starts_with(&prefix)).map(|tag| {
                format!("{sign}{key}:\"{}\"", tag.name)
            }).take(9).collect()
        } else {
            values.iter().filter(|v| v.starts_with(&prefix)).map(|v| format!("{sign}{key}:{v}")).collect()
        }
    } else {
        let key = key.to_ascii_lowercase();
        let mut matches = ATTRIBUTE_KEYS.iter().filter(|k| key.len() >= 2 && k.starts_with(&key));
        match (matches.next(), matches.next()) {
            (Some(k), None) => vec![format!("{sign}{k}:")],
            _ => Vec::new(),
        }
    };
    (range, suggestions)
}

const BOX_W: f32 = 245.0;
const TYPING_W: f32 = 320.0;
pub(super) const MIN_W: f32 = 90.0;
const G_FILTER: &str = "\u{E721}";

#[derive(Default)]
pub(super) struct FilterRects {
    pub bar: Rect,
    pub field: Rect,
    inner: Rect,
    pub close: Rect,
}

impl Tab {
    pub(super) fn reset_filter(&mut self) {
        self.filter = None;
        if let Some(v) = self.view.as_mut().filter(|v| v.is_filtered()) {
            v.set_filter("");
        }
    }

    pub(super) fn refilter_view(&mut self, clock: Clock, tags: &[crate::tags::Tag]) {
        if let (Some(v), Some(f)) = (&mut self.view, &self.filter) {
            v.apply_filter(crate::filter::NameFilter::with_context(f.text(), clock, tags));
        }
    }
}

impl App {
    pub(super) fn refilter_tab(&mut self, k: usize) {
        let clock = Clock::system(self.utc_offset_min);
        let tags = self.tags().clone();
        self.tabs.items[k].refilter_view(clock, &tags.defs);
    }

    pub(super) fn save_current_filter(&mut self) {
        let Some(text) = self.tab().filter.as_ref().map(Field::text).filter(|t| !t.trim().is_empty() && t.len() <= MAX_FILTER_LEN) else { return };
        let text = text.to_owned();
        self.settings.saved_filters.retain(|s| *s != text);
        self.settings.saved_filters.insert(0, text);
        self.settings.saved_filters.truncate(MAX_SAVED);
        self.dirty = true;
    }

    pub(super) fn choose_saved_filter(&mut self, text: &str) {
        self.open_filter();
        if let Some(filter) = &mut self.tab_mut().filter {
            filter.edit.set(text);
            self.filter_changed();
        }
    }

    pub(super) fn open_saved_filters(&mut self) {
        let items = self.settings.saved_filters.iter().map(|s| (s.clone(), String::new())).collect();
        self.open_pick(super::palette::Palette::new(super::palette::Pick::SavedFilters, items, ""));
    }

    pub(super) fn remove_saved_filter(&mut self, text: &str) {
        self.settings.saved_filters.retain(|s| s != text);
    }

    pub(super) fn set_saved_filters(&mut self, mut items: Vec<String>) {
        let mut seen = crate::hash::FxHashSet::default();
        items.retain(|s| !s.trim().is_empty() && s.len() <= MAX_FILTER_LEN && seen.insert(s.clone()));
        items.truncate(MAX_SAVED);
        self.settings.saved_filters = items;
    }

    fn complete_filter(&mut self) -> bool {
        let Some(f) = self.tab().filter.as_ref() else { return false };
        let (range, suggestions) = filter_completions(f.text(), f.edit.caret, self.tag_defs());
        let Some(mut hint) = suggestions.into_iter().find(|s| *s != f.text()[range.clone()]) else { return false };
        if !hint.ends_with(':') {
            hint.push(' ');
        }
        if let Some(f) = self.tab_mut().filter.as_mut() {
            f.edit.move_to(range.start, false);
            f.edit.move_to(range.end, true);
            f.insert(&hint);
        }
        self.filter_changed();
        true
    }
    pub(super) fn open_filter(&mut self) {
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
        self.refilter_tab(self.tabs.cur);
        let t = self.tab_mut();
        t.scroll = 0.0;
        t.scroll_target = 0.0;
        self.dirty = true;
    }

    pub(super) fn filter_key(&mut self, key: Key, m: Mods) -> bool {
        if key == Key::Tab && m == Mods::default() && self.complete_filter() {
            return true;
        }
        let Some(f) = self.tabs.items[self.tabs.cur].filter.as_mut() else {
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
        let Some(f) = self.tabs.items[self.tabs.cur].filter.as_mut() else { return false };
        f.insert(s);
        self.filter_changed();
        true
    }

    pub(super) fn filter_click(&mut self, x: f32, clicks: u8, extend: bool) {
        if self.tab().filter.is_none() {
            self.open_filter();
        }
        if let Some(f) = self.tabs.items[self.tabs.cur].filter.as_mut() {
            f.click(x, clicks, extend);
            self.focus = Focus::Filter;
        }
    }

    pub(super) fn layout_filter(&mut self) {
        let bar = self.status_face();
        let typing = self.focus == Focus::Filter || self.tab().filter.as_ref().is_some_and(|f| !f.text().is_empty());
        let sc = self.scale;
        let s = |v: f32| (v * sc).round();
        let r = &mut self.nav.filter;
        if bar.h <= 0.0 {
            *r = FilterRects::default();
            return;
        }
        r.bar = bar;
        let w = s(if typing { TYPING_W } else { BOX_W }).min(bar.w / 3.0).max(0.0);
        r.field = Rect::new(bar.x + s(6.0), bar.y + s(3.0), w, (bar.h - s(6.0)).max(0.0));
        let side = (r.field.h - s(4.0)).max(0.0);
        r.close = if typing { Rect::new(r.field.right() - side - s(2.0), r.field.y + s(2.0), side, side) } else { Rect::default() };
        let edge = if typing { r.close.x } else { r.field.right() };
        r.inner = Rect::new(r.field.x + s(24.0), r.field.y, (edge - s(4.0) - r.field.x - s(24.0)).max(0.0), r.field.h);
    }

    pub(super) fn fit_filter(&mut self, right: f32) {
        let min = self.s(MIN_W);
        let r = &mut self.nav.filter;
        let excess = r.field.right() - right.max(r.field.x + min);
        if excess > 0.0 {
            r.field.w -= excess;
            r.inner.w = (r.inner.w - excess).max(0.0);
            r.close.x -= excess;
        }
    }

    pub(super) fn filter_hit(&self, x: f32, y: f32) -> Option<Hit> {
        let r = &self.nav.filter;
        if r.close.contains(x, y) {
            return Some(Hit::FilterClose);
        }
        r.field.contains(x, y).then_some(Hit::FilterField)
    }

    pub(super) fn draw_filter<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme) {
        let r = &self.nav.filter;
        if r.field.w <= 0.0 {
            return;
        }
        let (field, inner, close) = (r.field, r.inner, r.close);
        let focused = self.active && self.focus == Focus::Filter;
        let (s4, scale) = (self.s(4.0), self.scale);
        let font = self.ui_font(12.0);
        let lit = if focused { 0.0 } else { self.lit(Hit::FilterField).max(self.lit(Hit::FilterClose)) };
        if lit > 0.0 {
            g.dl.round_rect(field, super::widgets::faded(t.button_hover, lit), s4);
        }
        if focused {
            g.dl.round_rect(field, t.input_bg, s4);
            g.dl.outline(field, t.input_border, s4, 1.0);
        }
        centered(g, Rect::new(field.x + s4, field.y, self.s(18.0), field.h), G_FILTER, self.icon_font(11.0), t.text_dim);
        if close.w > 0.0 {
            let lit = self.lit(Hit::FilterClose);
            if lit > 0.0 {
                g.dl.round_rect(close, super::widgets::faded(t.button_hover, lit), s4);
            }
            centered(g, close, G_CLOSE, self.icon_font(8.0), t.text_dim);
        }
        let placeholder = self.filter_placeholder();
        if let Some(f) = self.tab().filter.as_ref().filter(|_| focused && self.nav.palette.is_none()) {
            let (_, suggestions) = filter_completions(f.text(), f.edit.caret, self.tag_defs());
            if !suggestions.is_empty() {
                let text = crate::tr!(crate::i18n::S::FilterCompletions, choices = suggestions.join("  "));
                let room = (self.size.0 - field.x - self.s(8.0)).max(0.0);
                let width = (g.measure(&text, font) + self.s(16.0)).min(room);
                let r = Rect::new(field.x, field.y - self.s(27.0), width, self.s(24.0));
                g.dl.round_rect(r, t.input_bg, s4);
                g.dl.outline(r, t.input_border, s4, 1.0);
                let m = g.metrics(font);
                let y = (r.y + (r.h + m.ascent - m.descent) / 2.0).round();
                g.text(r.x + self.s(8.0), y, &text, font, t.text, (r.w - self.s(16.0)).max(0.0));
            }
        }
        match self.tabs.items[self.tabs.cur].filter.as_mut() {
            Some(f) => f.draw(g, t, inner, font, focused, &placeholder, scale, Some(&self.preedit)),
            None => {
                let m = g.metrics(font);
                let y = (inner.y + (inner.h + m.ascent - m.descent) / 2.0).round();
                g.text(inner.x, y, &placeholder, font, t.text_dim.alpha(0xA0), inner.w);
            }
        }
    }
}
