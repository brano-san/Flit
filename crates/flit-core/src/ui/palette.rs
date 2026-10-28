use super::actions::ACTIONS;
use super::describe::{description, description_en};
use super::edit::EditAction;
use super::field::{Field, FieldKey};
use super::fuzzy::fuzzy;
use super::settings_page::SettingPick;
use super::tags::TagPick;
use super::toolbar::ToolPick;
use super::widgets::{baseline, Button, Paint};
use super::{App, Effect, Glide, Hit, G_CLOSE, G_SORT_DOWN, G_SORT_UP, TAB_H, TOOL_H};
use crate::i18n::S;
use crate::input::{Key, Mods};
use crate::path::FsPath;
use crate::render::Rect;
use crate::text::{Gfx, TextBackend};
use crate::theme::{ink_on, Theme};
use std::cmp::Reverse;
use std::ops::Range;
use std::time::Instant;

const MIN_W: f32 = 360.0;
const ROW_H: f32 = 30.0;
const FIELD_H: f32 = 32.0;
const TOP_GAP: f32 = 45.0;
const EDGE: f32 = 12.0;
const HINT_H: f32 = 36.0;
const SCROLL_W: f32 = 14.0;
const DESC_PENALTY: i32 = 100_000;
const G_SEARCH: &str = "\u{E721}";

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Pick {
    Commands,
    NewItem,
    Shortcut(FsPath),
    Stream(FsPath),
    FolderIcon(FsPath),
    Encoding(FsPath),
    NewLibrary,
    Library(FsPath),
    LibraryAdd(FsPath),
    Tags(TagPick),
    Setting(SettingPick),
    Toolbar(ToolPick),
    Server,
}

impl Pick {
    fn placeholder(&self) -> &'static str {
        match self {
            Pick::Encoding(_) => S::PalettePlaceholderEncoding.t(),
            Pick::Commands => S::PalettePlaceholderCommands.t(),
            Pick::NewItem => S::PalettePlaceholderNewItem.t(),
            Pick::Shortcut(_) => S::PalettePlaceholderShortcut.t(),
            Pick::Stream(_) => S::PalettePlaceholderStream.t(),
            Pick::FolderIcon(_) => S::PalettePlaceholderFolderIcon.t(),
            Pick::NewLibrary => S::PalettePlaceholderNewLibrary.t(),
            Pick::Library(_) => S::PalettePlaceholderLibrary.t(),
            Pick::LibraryAdd(_) => S::PalettePlaceholderLibraryAdd.t(),
            Pick::Tags(t) => t.placeholder(),
            Pick::Setting(p) => p.placeholder(),
            Pick::Toolbar(t) => t.placeholder(),
            Pick::Server => S::SidebarDriveServerPrompt.t(),
        }
    }

    fn empty(&self) -> &'static str {
        match self {
            Pick::Commands => S::PaletteEmptyCommands.t(),
            Pick::NewItem => S::PaletteEmptyNewItem.t(),
            Pick::Encoding(_) => S::PaletteEmptyEncoding.t(),
            Pick::Shortcut(_) => S::PaletteEmptyShortcut.t(),
            Pick::Stream(_) => S::PaletteEmptyStream.t(),
            Pick::FolderIcon(_) => S::PaletteEmptyFolderIcon.t(),
            Pick::NewLibrary => S::PaletteEmptyNewLibrary.t(),
            Pick::Library(_) => S::PaletteEmptyLibrary.t(),
            Pick::LibraryAdd(_) => S::PaletteEmptyLibraryAdd.t(),
            Pick::Tags(t) => t.empty(),
            Pick::Setting(p) => p.empty(),
            Pick::Toolbar(_) => S::PaletteEmptyToolbar.t(),
            Pick::Server => S::SidebarDriveServerEnter.t(),
        }
    }

    fn is_input(&self) -> bool {
        matches!(self, Pick::Shortcut(_) | Pick::Stream(_) | Pick::FolderIcon(_) | Pick::NewLibrary | Pick::LibraryAdd(_) | Pick::Server) || matches!(self, Pick::Tags(t) if t.is_input()) || matches!(self, Pick::Setting(p) if p.is_input())
    }
}

pub(super) struct Palette {
    pub pick: Pick,
    pub titles: Vec<String>,
    field: Field,
    pub matches: Vec<usize>,
    pub marks: Vec<Vec<Range<usize>>>,
    pub keys: Vec<String>,
    alts: Vec<&'static str>,
    descs: Vec<String>,
    ids: Vec<usize>,
    pub sel: usize,
    first: usize,
    shift: f32,
    pub(super) scroll: Glide,
    list: Rect,
    pub rect: Rect,
    field_rect: Rect,
    close: Rect,
    arrows: [Rect; 2],
    track: Rect,
    hints: Rect,
    pub page: usize,
    pub rows: Vec<Rect>,
    opened: Option<Instant>,
}

fn described(query: &str, desc: &str) -> Option<i32> {
    let limit = query.chars().filter(|c| !c.is_whitespace()).count().div_ceil(3).max(1);
    fuzzy(query, desc).filter(|m| m.ranges.len() <= limit).map(|m| m.score - DESC_PENALTY)
}

fn rank<'a>(query: &str, items: impl Iterator<Item = (&'a str, &'a str, &'a str)>) -> Vec<(usize, Vec<Range<usize>>)> {
    let mut scored: Vec<(i32, usize, Vec<Range<usize>>)> = items
        .enumerate()
        .filter_map(|(i, (t, alt, d))| {
            let own = fuzzy(query, t).map(|m| (m.score, i, m.ranges));
            let other = fuzzy(query, alt).map(|m| (m.score, i, Vec::new()));
            match (own, other) {
                (Some(a), Some(b)) => Some(if b.0 > a.0 { b } else { a }),
                (a, b) => a.or(b).or_else(|| described(query, d).map(|s| (s, i, Vec::new()))),
            }
        })
        .collect();
    scored.sort_by_key(|&(s, i, _)| (Reverse(s), i));
    scored.into_iter().map(|(_, i, r)| (i, r)).collect()
}

fn searched_description(id: &str) -> String {
    let (local, en) = (description(id), description_en(id));
    if local == en { en.to_owned() } else { format!("{local} {en}") }
}

pub fn matches(query: &str) -> Vec<usize> {
    let descs: Vec<String> = ACTIONS.iter().map(|a| searched_description(a.id)).collect();
    rank(query, ACTIONS.iter().zip(&descs).map(|(a, d)| (a.title.t(), a.title.en(), d.as_str()))).into_iter().map(|(i, _)| i).collect()
}

impl Palette {
    pub(super) fn new(pick: Pick, items: Vec<(String, String)>, text: &str) -> Self {
        let (titles, keys): (Vec<String>, Vec<String>) = items.into_iter().unzip();
        let ids = (0..titles.len()).collect();
        let mut p = Self {
            pick,
            titles,
            field: Field::default(),
            matches: Vec::new(),
            marks: Vec::new(),
            keys,
            alts: Vec::new(),
            descs: Vec::new(),
            ids,
            sel: 0,
            first: 0,
            shift: 0.0,
            scroll: Glide::default(),
            list: Rect::default(),
            rect: Rect::default(),
            field_rect: Rect::default(),
            close: Rect::default(),
            arrows: [Rect::default(); 2],
            track: Rect::default(),
            hints: Rect::default(),
            page: 1,
            rows: Vec::new(),
            opened: None,
        };
        p.field.edit.set(text);
        p.field.edit.select_all();
        p.refresh();
        p
    }

    fn commands(app: &App) -> Self {
        let ids: Vec<usize> = (0..ACTIONS.len()).filter(|&i| app.available(ACTIONS[i].id)).collect();
        let mut p = Self::new(Pick::Commands, ids.iter().map(|&i| (ACTIONS[i].title.t().to_owned(), app.keymap.text(i))).collect(), "");
        p.alts = ids.iter().map(|&i| ACTIONS[i].title.en()).collect();
        p.descs = ids.iter().map(|&i| searched_description(ACTIONS[i].id)).collect();
        p.ids = ids;
        p.refresh();
        p
    }

    fn refresh(&mut self) {
        let alts = self.alts.iter().copied().chain(std::iter::repeat(""));
        let descs = self.descs.iter().map(String::as_str).chain(std::iter::repeat(""));
        let ranked = if self.pick.is_input() {
            Vec::new()
        } else if matches!(self.pick, Pick::Setting(SettingPick::Capture { .. })) {
            (0..self.titles.len()).map(|i| (i, Vec::new())).collect()
        } else {
            rank(self.field.text(), self.titles.iter().map(String::as_str).zip(alts).zip(descs).map(|((t, a), d)| (t, a, d)))
        };
        (self.matches, self.marks) = ranked.into_iter().unzip();
        self.sel = 0;
        self.scroll.snap(0.0);
    }

    fn max_scroll(&self) -> f32 {
        self.matches.len().saturating_sub(self.page) as f32 * ROW_H
    }

    fn select(&mut self, to: isize) {
        let n = self.matches.len() as isize;
        if n == 0 {
            return;
        }
        self.sel = to.rem_euclid(n) as usize;
        let top = self.sel as f32 * ROW_H;
        let view = self.page as f32 * ROW_H;
        if top < self.scroll.target {
            self.scroll.target = top;
        } else if top + ROW_H > self.scroll.target + view {
            self.scroll.target = top + ROW_H - view;
        }
    }

    pub(super) fn select_row(&mut self, k: usize) {
        self.select(k.min(self.matches.len().saturating_sub(1)) as isize);
    }

}

impl App {
    pub(super) fn open_palette(&mut self) {
        self.open_pick(Palette::commands(self));
    }

    pub(super) fn open_pick(&mut self, p: Palette) {
        let same = self.nav.palette.as_ref().is_some_and(|open| open.pick == p.pick);
        self.nav.palette = None;
        if !same {
            if self.editing {
                self.cancel_edit();
            }
            self.nav.palette = Some(p);
        }
        self.layout();
        self.dirty = true;
    }

    pub(super) fn close_palette(&mut self) {
        self.nav.palette = None;
        self.dirty = true;
    }

    pub(super) fn run_palette_item(&mut self, index: usize, now: Instant) {
        let Some(p) = self.nav.palette.take() else { return };
        self.dirty = true;
        if p.pick.is_input() {
            return self.submit_pick(p.pick, p.field.text().trim());
        }
        let Some(&id) = p.matches.get(index) else {
            self.nav.palette = Some(p);
            return;
        };
        match p.pick {
            Pick::Commands => (ACTIONS[p.ids[id]].run)(self, now),
            Pick::Encoding(archive) => self.set_encoding(&archive, id, now),
            Pick::Library(lib) => self.library_row(lib, &p.titles[id], false),
            Pick::Tags(t) => self.tag_choose(t, id, now),
            Pick::Setting(p) => self.setting_choose(p, id),
            Pick::Toolbar(t) => self.tool_choose(t, id),
            _ => self.new_item(id),
        }
    }

    fn remove_palette_item(&mut self) -> bool {
        let Some(p) = self.nav.palette.take_if(|p| matches!(p.pick, Pick::Library(_))) else { return false };
        if let (Pick::Library(lib), Some(&id)) = (p.pick, p.matches.get(p.sel)) {
            self.library_row(lib, &p.titles[id], true);
        }
        true
    }

    pub(super) fn palette_key(&mut self, key: Key, m: Mods, now: Instant) -> bool {
        if self.capture_key(key, m, now) {
            return true;
        }
        if key == Key::Delete && self.remove_palette_item() {
            self.layout();
            return true;
        }
        let closes = self.keymap.runs(key, m, "palette.show");
        let Some(p) = &mut self.nav.palette else { return false };
        let page = p.page as isize;
        match key {
            Key::Escape => self.close_palette(),
            Key::Enter => {
                let sel = p.sel;
                self.run_palette_item(sel, now);
            }
            Key::Up => p.select(p.sel as isize - 1),
            Key::Down => p.select(p.sel as isize + 1),
            Key::PageUp => p.select((p.sel as isize - page).max(0)),
            Key::PageDown => p.select((p.sel as isize + page).min(p.matches.len() as isize - 1)),
            _ => match p.field.key(key, m) {
                FieldKey::Edited { changed, action } => {
                    if changed {
                        p.refresh();
                    }
                    match action {
                        EditAction::Copy(s) => self.effects.push(Effect::SetClipboard(s)),
                        EditAction::Paste => self.effects.push(Effect::Paste),
                        _ => {}
                    }
                }
                FieldKey::Ignored if closes => self.close_palette(),
                FieldKey::Ignored => {}
            },
        }
        self.layout();
        true
    }

    pub(super) fn palette_insert(&mut self, s: &str) -> bool {
        let Some(p) = &mut self.nav.palette else { return false };
        if matches!(p.pick, Pick::Setting(SettingPick::Capture { .. })) {
            return true;
        }
        p.field.insert(s);
        p.refresh();
        self.layout();
        true
    }

    pub(super) fn palette_field(&self) -> Option<Rect> {
        self.nav.palette.as_ref().map(|p| p.field_rect)
    }

    pub(super) fn palette_click(&mut self, x: f32, clicks: u8) {
        if let Some(p) = &mut self.nav.palette {
            p.field.click(x, clicks);
        }
    }

    pub(super) fn palette_wheel(&mut self, dy: f32) {
        if let Some(p) = &mut self.nav.palette {
            let max = p.max_scroll();
            p.scroll.by(-dy * 3.0 * ROW_H, max);
            self.hover = Hit::PaletteBox;
            self.dirty = true;
        }
    }

    pub(super) fn palette_step(&mut self, down: bool) {
        if let Some(p) = &mut self.nav.palette {
            let max = p.max_scroll();
            p.scroll.by(if down { ROW_H } else { -ROW_H }, max);
            self.dirty = true;
        }
    }

    pub(super) fn layout_palette(&mut self) {
        let (w, h, sc) = (self.size.0, self.size.1, self.scale);
        let s = |v: f32| (v * sc).round();
        let left = self.panels.left;
        let Some(p) = &mut self.nav.palette else { return };
        let area = (w - left).max(s(200.0));
        let pw = (w / 2.0).round().max(s(MIN_W)).min(area - 2.0 * s(EDGE)).max(s(120.0));
        let x = (left + (area - pw) / 2.0).round().min(w - pw).max(0.0);
        let unit = s(ROW_H);
        let fixed = s(8.0) + s(FIELD_H) + s(6.0) + s(HINT_H);
        let y = (s(TAB_H) + s(TOOL_H) + s(TOP_GAP)).min((h - s(EDGE) - fixed - unit).max(s(EDGE)));
        p.field_rect = Rect::new(x + s(8.0), y + s(8.0), pw - s(16.0) - s(FIELD_H) - s(4.0), s(FIELD_H));
        p.close = Rect::new(p.field_rect.right() + s(4.0), p.field_rect.y, s(FIELD_H), s(FIELD_H));
        let top = p.field_rect.bottom() + s(6.0);
        let room = (h - s(EDGE) - s(HINT_H) - top).max(unit);
        p.page = ((room / unit).floor() as usize).max(1);
        let n = p.matches.len();
        let shown = n.clamp(1, p.page);
        let list = Rect::new(x, top, pw, shown as f32 * unit);
        let scrolls = n > p.page;
        let bar = if scrolls { s(SCROLL_W) + s(4.0) } else { 0.0 };
        p.rows.clear();
        let at = p.scroll.pos / ROW_H;
        p.first = at.floor() as usize;
        p.shift = ((at - at.floor()) * unit).round();
        for k in p.first..n.min(p.first + p.page + 1) {
            p.rows.push(Rect::new(x + s(6.0), top + (k - p.first) as f32 * unit - p.shift, pw - s(12.0) - bar, unit));
        }
        p.list = list;
        let sw = s(SCROLL_W);
        let tx = list.right() - s(4.0) - sw;
        (p.arrows, p.track) = if scrolls {
            ([Rect::new(tx, list.y, sw, sw), Rect::new(tx, list.bottom() - sw, sw, sw)], Rect::new(tx, list.y + sw, sw, (list.h - 2.0 * sw).max(0.0)))
        } else {
            ([Rect::default(); 2], Rect::default())
        };
        p.hints = Rect::new(x, list.bottom(), pw, s(HINT_H));
        p.rect = Rect::new(x, y, pw, p.hints.bottom() - y);
    }

    pub(super) fn palette_button(&self, h: Hit) -> Option<Rect> {
        let p = self.nav.palette.as_ref()?;
        match h {
            Hit::PaletteClose => Some(p.close),
            Hit::PaletteScroll(down) => Some(p.arrows[usize::from(down)]),
            _ => None,
        }
    }

    pub(super) fn palette_hit(&self, x: f32, y: f32) -> Option<Hit> {
        let p = self.nav.palette.as_ref()?;
        if self.in_title(y) {
            return None;
        }
        if !p.rect.contains(x, y) {
            return Some(Hit::PaletteOutside);
        }
        if p.field_rect.contains(x, y) {
            return Some(Hit::PaletteField);
        }
        if p.close.contains(x, y) {
            return Some(Hit::PaletteClose);
        }
        if let Some(k) = p.arrows.iter().position(|r| r.contains(x, y)) {
            return Some(Hit::PaletteScroll(k == 1));
        }
        if p.track.contains(x, y) {
            return Some(Hit::PaletteBox);
        }
        Some(p.rows.iter().position(|r| r.contains(x, y) && p.list.contains(x, y)).map_or(Hit::PaletteBox, |i| Hit::PaletteRow(p.first + i)))
    }

    pub(super) fn draw_palette<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme) {
        let (font, small, icons) = (self.ui_font(13.0), self.ui_font(12.0), self.icon_font(11.0));
        let (s3, s4, s6, s12, s16, sc) = (self.s(3.0), self.s(4.0), self.s(6.0), self.s(12.0), self.s(16.0), self.scale);
        let (close_font, arrow_font) = (self.icon_font(9.0), self.icon_font(8.0));
        let (active, hover, now) = (self.active, self.hover, self.last_frame.unwrap_or_else(Instant::now));
        let from = g.dl.instances.len();
        let Some(p) = &mut self.nav.palette else { return };
        let opened = *p.opened.get_or_insert(now);
        let r = p.rect;
        let mut w = Paint::new(g, t, sc);
        w.frame(r);
        let fr = p.field_rect;
        w.field(fr, true);
        w.centered(Rect::new(fr.x, fr.y, fr.h, fr.h), G_SEARCH, icons, t.text_dim);
        let inner = Rect::new(fr.x + fr.h, fr.y, fr.w - fr.h - s12, fr.h);
        p.field.draw(w.g, t, inner, font, active, p.pick.placeholder(), sc, Some(&self.preedit));
        w.icon_button(p.close, G_CLOSE, close_font, Button::of(hover == Hit::PaletteClose, false, true), t.text_dim);
        w.g.dl.set_clip(Some(p.list));
        for (k, &row) in p.rows.iter().enumerate() {
            let i = p.first + k;
            let selected = i == p.sel;
            if selected {
                w.g.dl.round_rect(row, t.accent_fill, s4);
            } else if hover == Hit::PaletteRow(i) {
                w.g.dl.round_rect(row, t.hover, s4);
            }
            let (ink, dim, mark) = if selected { (ink_on(t.accent_fill), ink_on(t.accent_fill), ink_on(t.accent_fill)) } else { (t.text, t.text_dim, t.accent) };
            let y = baseline(w.g, row, font);
            let keys = &p.keys[p.matches[i]];
            let kw = if keys.is_empty() { 0.0 } else { w.g.measure(keys, small) };
            w.g.text(row.right() - s12 - kw, y, keys, small, dim, f32::INFINITY);
            w.marked((row.x + s12, y), &p.titles[p.matches[i]], &p.marks[i], font, (ink, mark), row.w - 3.0 * s12 - kw);
        }
        w.g.dl.set_clip(None);
        if p.matches.is_empty() {
            let row = Rect::new(r.x + s12, p.list.y, r.w - 2.0 * s12, p.list.h);
            let y = baseline(w.g, row, font);
            w.g.text(row.x + s6, y, p.pick.empty(), font, t.text_dim, row.w);
        }
        if p.track.h > 0.0 {
            let tr = p.track;
            let total = p.matches.len().max(1) as f32;
            let th = (tr.h * p.page as f32 / total).max(s16).min(tr.h);
            let frac = if p.max_scroll() > 0.0 { p.scroll.pos / p.max_scroll() } else { 0.0 };
            w.g.dl.round_rect(Rect::new(tr.x + s3, tr.y + (tr.h - th) * frac, tr.w - s6, th), if matches!(hover, Hit::PaletteBox) { t.scrollbar_hover } else { t.scrollbar }, s3);
            for (k, glyph) in [G_SORT_UP, G_SORT_DOWN].into_iter().enumerate() {
                w.icon_button(p.arrows[k], glyph, arrow_font, Button::of(hover == Hit::PaletteScroll(k == 1), false, true), t.text_dim);
            }
        }
        let hints = p.hints;
        w.separator(hints.x + s12, hints.y, hints.w - 2.0 * s12);
        let keys = [S::WidgetsKeyUp.t(), S::WidgetsKeyDown.t()];
        w.hints(hints.x + hints.w / 2.0, hints.y + hints.h / 2.0, &[(&keys, S::WidgetsHintNavigate.t()), (&[S::WidgetsKeyEnter.t()], S::WidgetsHintUse.t()), (&[S::WidgetsKeyEscape.t()], S::WidgetsHintDismiss.t())], small);
        self.pop(g, from, opened, (r.x + r.w / 2.0, r.y));
    }
}

#[cfg(test)]
#[path = "../../test/ui/palette.rs"]
mod tests;
