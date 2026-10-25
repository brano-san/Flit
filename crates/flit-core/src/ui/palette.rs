use super::actions::{keys_text, ACTIONS};
use super::describe::description;
use super::edit::EditAction;
use super::field::{Field, FieldKey};
use super::tags::TagPick;
use super::toolbar::ToolPick;
use super::{App, Effect, Hit};
use crate::input::{Key, Mods};
use crate::path::FsPath;
use crate::render::{Color, Rect};
use crate::text::{Gfx, TextBackend};
use crate::theme::Theme;
use std::cmp::Reverse;
use std::time::Instant;

const WIDTH: f32 = 560.0;
const ROW_H: f32 = 30.0;
const FIELD_H: f32 = 32.0;
const MAX_ROWS: usize = 12;

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
    Toolbar(ToolPick),
}

impl Pick {
    fn placeholder(&self) -> &'static str {
        match self {
            Pick::Encoding(_) => "Type to filter the encodings of names in the archive",
            Pick::Commands => "Type a command",
            Pick::NewItem => "Type to filter the kinds of new items",
            Pick::Shortcut(_) => "Full path of the shortcut target",
            Pick::Stream(_) => "Name of the new alternate data stream",
            Pick::FolderIcon(_) => "Full path of an icon, program or library, then optionally a comma and the icon index",
            Pick::NewLibrary => "Name of the new library",
            Pick::Library(_) => "Enter makes a folder the default save location, Delete removes it from the library",
            Pick::LibraryAdd(_) => "Full path of the folder to include in the library",
            Pick::Tags(t) => t.placeholder(),
            Pick::Toolbar(t) => t.placeholder(),
        }
    }

    fn empty(&self) -> &'static str {
        match self {
            Pick::Commands => "No matching commands",
            Pick::NewItem => "No matching kinds of items",
            Pick::Encoding(_) => "No matching encodings",
            Pick::Shortcut(_) => "Enter creates the shortcut in this folder",
            Pick::Stream(_) => "Enter creates the stream",
            Pick::FolderIcon(_) => "Enter writes the icon into desktop.ini of the folder",
            Pick::NewLibrary => "Enter creates the library in the Libraries folder",
            Pick::Library(_) => "No matching folders",
            Pick::LibraryAdd(_) => "Enter adds the folder to the library",
            Pick::Tags(t) => t.empty(),
            Pick::Toolbar(_) => "No matching buttons",
        }
    }

    fn is_input(&self) -> bool {
        matches!(self, Pick::Shortcut(_) | Pick::Stream(_) | Pick::FolderIcon(_) | Pick::NewLibrary | Pick::LibraryAdd(_)) || matches!(self, Pick::Tags(t) if t.is_input())
    }
}

pub(super) struct Palette {
    pub pick: Pick,
    pub titles: Vec<String>,
    field: Field,
    pub matches: Vec<usize>,
    pub keys: Vec<String>,
    descs: Vec<&'static str>,
    ids: Vec<usize>,
    pub sel: usize,
    first: usize,
    rect: Rect,
    field_rect: Rect,
    pub rows: Vec<Rect>,
}

pub fn score(query: &str, title: &str) -> Option<u32> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Some(0);
    }
    let t = title.to_lowercase();
    if let Some(p) = t.find(&q) {
        let word = p == 0 || t.as_bytes()[p - 1] == b' ';
        return Some(3000 - p.min(999) as u32 + if word { 1000 } else { 0 });
    }
    let chars: Vec<char> = t.chars().collect();
    let word_start = |i: usize| i == 0 || chars[i - 1] == ' ';
    let (mut score, mut next) = (1000u32, 0usize);
    for qc in q.chars().filter(|c| !c.is_whitespace()) {
        let found = (next..chars.len()).find(|&i| chars[i] == qc && ((i == next && next > 0) || word_start(i)))?;
        score = (score + if found == next { 10 } else { 0 }).saturating_sub((found - next) as u32);
        next = found + 1;
    }
    Some(score.min(1999))
}

fn described(query: &str, desc: &str) -> Option<u32> {
    let d = desc.to_lowercase();
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    (!words.is_empty() && words.iter().all(|w| d.contains(w.as_str()))).then_some(500)
}

fn rank<'a>(query: &str, items: impl Iterator<Item = (&'a str, &'a str)>) -> Vec<usize> {
    let mut scored: Vec<(u32, usize)> = items.enumerate().filter_map(|(i, (t, d))| score(query, t).or_else(|| described(query, d)).map(|s| (s, i))).collect();
    scored.sort_by_key(|&(s, i)| (Reverse(s), i));
    scored.into_iter().map(|(_, i)| i).collect()
}

pub fn matches(query: &str) -> Vec<usize> {
    rank(query, ACTIONS.iter().map(|a| (a.title, description(a.id))))
}

impl Palette {
    pub(super) fn new(pick: Pick, items: Vec<(String, String)>, text: &str) -> Self {
        let (titles, keys): (Vec<String>, Vec<String>) = items.into_iter().unzip();
        let ids = (0..titles.len()).collect();
        let mut p = Self { pick, titles, field: Field::default(), matches: Vec::new(), keys, descs: Vec::new(), ids, sel: 0, first: 0, rect: Rect::default(), field_rect: Rect::default(), rows: Vec::new() };
        p.field.edit.set(text);
        p.field.edit.select_all();
        p.refresh();
        p
    }

    fn commands(app: &App) -> Self {
        let ids: Vec<usize> = (0..ACTIONS.len()).filter(|&i| app.available(ACTIONS[i].id)).collect();
        let mut p = Self::new(Pick::Commands, ids.iter().map(|&i| (ACTIONS[i].title.to_owned(), keys_text(&ACTIONS[i]))).collect(), "");
        p.descs = ids.iter().map(|&i| description(ACTIONS[i].id)).collect();
        p.ids = ids;
        p.refresh();
        p
    }

    fn refresh(&mut self) {
        let descs = self.descs.iter().copied().chain(std::iter::repeat(""));
        self.matches = if self.pick.is_input() { Vec::new() } else { rank(self.field.text(), self.titles.iter().map(String::as_str).zip(descs)) };
        self.sel = 0;
        self.first = 0;
    }

    fn select(&mut self, to: isize) {
        let n = self.matches.len() as isize;
        if n == 0 {
            return;
        }
        self.sel = to.rem_euclid(n) as usize;
        if self.sel < self.first {
            self.first = self.sel;
        } else if self.sel >= self.first + MAX_ROWS {
            self.first = self.sel + 1 - MAX_ROWS;
        }
    }

    pub(super) fn select_row(&mut self, k: usize) {
        self.select(k.min(self.matches.len().saturating_sub(1)) as isize);
    }

    fn scroll(&mut self, rows: isize) {
        let max = self.matches.len().saturating_sub(MAX_ROWS) as isize;
        self.first = (self.first as isize + rows).clamp(0, max) as usize;
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
        if key == Key::Delete && self.remove_palette_item() {
            self.layout();
            return true;
        }
        let Some(p) = &mut self.nav.palette else { return false };
        let page = MAX_ROWS as isize;
        match key {
            Key::Escape => self.close_palette(),
            Key::Char('P') if m.ctrl && m.shift => self.close_palette(),
            Key::Enter => {
                let sel = p.sel;
                self.run_palette_item(sel, now);
            }
            Key::Up => p.select(p.sel as isize - 1),
            Key::Down => p.select(p.sel as isize + 1),
            Key::PageUp => p.select((p.sel as isize - page).max(0)),
            Key::PageDown => p.select((p.sel as isize + page).min(p.matches.len() as isize - 1)),
            _ => {
                if let FieldKey::Edited { changed, action } = p.field.key(key, m) {
                    if changed {
                        p.refresh();
                    }
                    match action {
                        EditAction::Copy(s) => self.effects.push(Effect::SetClipboard(s)),
                        EditAction::Paste => self.effects.push(Effect::Paste),
                        _ => {}
                    }
                }
            }
        }
        self.layout();
        true
    }

    pub(super) fn palette_insert(&mut self, s: &str) -> bool {
        let Some(p) = &mut self.nav.palette else { return false };
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
            p.scroll(-(dy * 3.0).round() as isize);
            self.layout();
        }
    }

    pub(super) fn layout_palette(&mut self, top: f32) {
        let (w, sc) = (self.size.0, self.scale);
        let s = |v: f32| (v * sc).round();
        let Some(p) = &mut self.nav.palette else { return };
        let pw = s(WIDTH).min(w - s(32.0)).max(s(120.0));
        let x = ((w - pw) / 2.0).round();
        let y = top + s(4.0);
        p.field_rect = Rect::new(x + s(8.0), y + s(8.0), pw - s(16.0), s(FIELD_H));
        p.rows.clear();
        let mut ry = p.field_rect.bottom() + s(6.0);
        for _ in p.first..p.matches.len().min(p.first + MAX_ROWS) {
            p.rows.push(Rect::new(x + s(6.0), ry, pw - s(12.0), s(ROW_H)));
            ry += s(ROW_H);
        }
        if p.matches.is_empty() {
            ry += s(ROW_H);
        }
        p.rect = Rect::new(x, y, pw, ry + s(6.0) - y);
    }

    pub(super) fn palette_hit(&self, x: f32, y: f32) -> Option<Hit> {
        let p = self.nav.palette.as_ref()?;
        if self.layout.title.contains(x, y) {
            return None;
        }
        if !p.rect.contains(x, y) {
            return Some(Hit::PaletteOutside);
        }
        if p.field_rect.contains(x, y) {
            return Some(Hit::PaletteField);
        }
        Some(p.rows.iter().position(|r| r.contains(x, y)).map_or(Hit::PaletteBox, |i| Hit::PaletteRow(p.first + i)))
    }

    pub(super) fn draw_palette<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme) {
        let (font, small) = (self.ui_font(13.0), self.ui_font(12.0));
        let (s4, s6, s12, row_h) = (self.s(4.0), self.s(6.0), self.s(12.0), self.s(ROW_H));
        let (scale, active, hover) = (self.scale, self.active, self.hover);
        let shadow = if self.dark { Color::rgba(0x00000090) } else { Color::rgba(0x00000030) };
        let Some(p) = &mut self.nav.palette else { return };
        let r = p.rect;
        g.dl.round_rect(Rect::new(r.x - s4, r.y, r.w + 2.0 * s4, r.h + s6 + s4), shadow, s6 + s4);
        g.dl.round_rect(r, t.bg, s6);
        g.dl.outline(r, t.input_border, s6, 1.0);
        let fr = p.field_rect;
        g.dl.round_rect(fr, t.input_bg, s4);
        g.dl.outline(fr, t.accent, s4, scale.round().max(1.0));
        let inner = Rect::new(fr.x + s12, fr.y, fr.w - 2.0 * s12, fr.h);
        p.field.draw(g, t, inner, font, active, p.pick.placeholder(), scale);
        let m = g.metrics(font);
        for (k, &row) in p.rows.iter().enumerate() {
            let i = p.first + k;
            if i == p.sel {
                g.dl.round_rect(row, t.selection, s4);
            } else if hover == Hit::PaletteRow(i) {
                g.dl.round_rect(row, t.hover, s4);
            }
            let baseline = (row.y + (row.h + m.ascent - m.descent) / 2.0).round();
            let keys = &p.keys[p.matches[i]];
            let kw = if keys.is_empty() { 0.0 } else { g.measure(keys, small) };
            g.text(row.right() - s12 - kw, baseline, keys, small, t.text_dim, f32::INFINITY);
            g.text(row.x + s12, baseline, &p.titles[p.matches[i]], font, t.text, row.w - 3.0 * s12 - kw);
        }
        if p.matches.is_empty() {
            let baseline = (fr.bottom() + s6 + (row_h + m.ascent - m.descent) / 2.0).round();
            g.text(r.x + s12 + s6, baseline, p.pick.empty(), font, t.text_dim, r.w - 2.0 * s12);
        }
    }
}

#[cfg(test)]
#[path = "../../test/ui/palette.rs"]
mod tests;
