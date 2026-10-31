use super::address::{place_key, Listed, SEPS};
use super::fuzzy::fuzzy;
use super::palette::{Palette, Pick};
use super::sidebar::{folder_icon, place_icon_request, Item, Section, SystemIcon, G_COLLAPSED, G_EXPANDED};
use super::widgets::{baseline, Button, Paint};
use super::{App, Effect, Hit, IconKind, IconRequest, Route, G_BACK, G_FILE, G_FOLDER, G_FORWARD, G_UP, PER_FILE_ICON_EXT};
use crate::hash;
use crate::i18n::{P, S};
use crate::input::{Key, Mods};
use crate::location::Location;
use crate::name;
use crate::open::OpenTarget;
use crate::path::PathKind;
use crate::provider::ProviderRequest;
use crate::render::Rect;
use crate::text::{FontStyle, Gfx, TextBackend};
use crate::theme::{ink_on, Theme};
use std::ops::Range;
use std::time::Instant;

const QUICK: [Section; 4] = [Section::Recents, Section::Bookmarks, Section::Storage, Section::Places];
const G_TYPED: &str = "\u{E81C}";
const G_MARK: &str = "\u{E72B}";
const ICON: f32 = 16.0;
const BAR_W: f32 = 60.0;

#[derive(Clone, Debug)]
pub(super) enum Kind {
    Place(Item),
    Entry { loc: Location, dir: bool },
    Typed(String),
    Suggested(String),
}

#[derive(Clone, Debug)]
pub(super) struct Row {
    pub label: String,
    pub path: String,
    pub kind: Kind,
}

impl Row {
    fn loc(&self) -> Option<Location> {
        match &self.kind {
            Kind::Place(it) => Some(it.loc.clone()),
            Kind::Entry { loc, .. } => Some(loc.clone()),
            Kind::Typed(t) | Kind::Suggested(t) => Location::parse(t),
        }
    }

    fn folder(&self) -> bool {
        match &self.kind {
            Kind::Place(it) => it.glyph != G_FILE,
            Kind::Entry { dir, .. } => *dir,
            Kind::Typed(t) | Kind::Suggested(t) => t.ends_with(SEPS),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Line {
    Header(Section),
    Row(usize, Vec<Range<usize>>),
}

#[derive(Default)]
pub(super) struct Go {
    pub crumb: Option<usize>,
    pub dir: Option<Location>,
    pub mark: Option<Vec<u8>>,
    pub rows: Vec<Row>,
    pub lines: Vec<Line>,
    pub collapsed: Vec<Section>,
    pub picked: bool,
    pub count: usize,
    pub buttons: [Rect; 3],
    seen: Option<String>,
}

pub(super) fn with_sep(loc: &Location) -> String {
    if matches!(loc.path().kind(), PathKind::Computer | PathKind::Home) {
        return String::new();
    }
    let mut s = loc.display().into_owned();
    if !s.ends_with(SEPS) {
        s.push(if s.contains('\\') || !s.starts_with('/') { '\\' } else { '/' });
    }
    s
}

fn ranked(query: &str, rows: &[Row], from: usize) -> Vec<Line> {
    if query.is_empty() {
        return (from..rows.len()).map(|k| Line::Row(k, Vec::new())).collect();
    }
    let mut scored: Vec<(i32, usize, Vec<Range<usize>>)> = (from..rows.len()).filter_map(|k| fuzzy(query, &rows[k].label).map(|m| (m.score, k, m.ranges))).collect();
    scored.sort_by_key(|&(s, k, _)| (std::cmp::Reverse(s), k));
    scored.into_iter().map(|(_, k, r)| Line::Row(k, r)).collect()
}

fn place_row(it: Item) -> Row {
    Row { label: it.label.clone(), path: it.path.clone(), kind: Kind::Place(it) }
}

fn entry_icon(loc: &Location, dir: bool, sb: &super::sidebar::Sidebar, px: u32) -> Option<SystemIcon> {
    if dir {
        return folder_icon(loc, sb, px);
    }
    let path = loc.to_windows()?;
    let name = loc.file_name();
    let ext = name.iter().rposition(|&b| b == b'.').map_or(&b""[..], |i| &name[i + 1..]).to_ascii_lowercase();
    if PER_FILE_ICON_EXT.contains(&ext.as_slice()) {
        return Some((hash::hash(&("place", path.as_bytes(), px)), IconKind::File, path));
    }
    Some((hash::hash(&("ext", ext.as_slice(), px)), IconKind::Type, path))
}

impl App {
    pub(super) fn open_goto(&mut self) {
        self.open_go(String::new(), None);
    }

    pub(super) fn open_address(&mut self) {
        let text = with_sep(self.current_location());
        self.open_go(text, None);
    }

    pub(super) fn open_crumb(&mut self, i: usize) {
        if self.tab().crumb(i).is_none() {
            return;
        }
        self.open_go(String::new(), Some(i));
    }

    pub(super) fn toggle_crumb(&mut self, i: usize) {
        if self.nav.palette.as_ref().is_some_and(|p| p.pick == Pick::Crumb(i)) {
            self.close_palette();
        } else {
            self.open_crumb(i);
        }
    }

    pub(super) fn open_folder_dropdown(&mut self) {
        let last = self.tab().crumbs.len().saturating_sub(1);
        self.open_crumb(last);
    }

    fn open_go(&mut self, text: String, crumb: Option<usize>) {
        let pick = if let Some(i) = crumb { Pick::Crumb(i) } else { Pick::GoTo };
        let mut p = Palette::new(pick, Vec::new(), &text);
        p.go = Some(Go { crumb, ..Default::default() });
        self.nav.palette = None;
        self.focus = super::Focus::List;
        self.nav.palette = Some(p);
        self.layout();
        self.go_refresh();
        self.layout();
        self.dirty = true;
    }

    pub(super) fn go_open(&self) -> bool {
        self.nav.palette.as_ref().is_some_and(|p| p.go.is_some())
    }

    fn go_lines(&mut self, text: &str, collapsed: &[Section]) -> (Vec<Row>, Vec<Line>, Option<Location>) {
        let mut rows: Vec<Row> = Vec::new();
        let mut lines: Vec<Line> = Vec::new();
        let sb = &self.nav.sidebar;
        if text.is_empty() {
            for s in sb.prefs.order().into_iter().filter(|s| QUICK.contains(s)) {
                let items = sb.section_items(s);
                if items.is_empty() {
                    continue;
                }
                lines.push(Line::Header(s));
                for it in items {
                    if !collapsed.contains(&s) {
                        lines.push(Line::Row(rows.len(), Vec::new()));
                    }
                    rows.push(place_row(it));
                }
            }
            return (rows, lines, None);
        }
        let mut listed = None;
        let mut complete = false;
        if let Some(c) = text.rfind(SEPS) {
            let (head, rest) = (&text[..=c], &text[c + 1..]);
            if let Some(dir) = self.resolve_typed(head) {
                if let Listed::Found(entries) = self.listing(&dir) {
                    rows.extend(entries.into_iter().map(|(n, d)| Row { label: name::display(&n).into_owned(), path: String::new(), kind: Kind::Entry { loc: dir.join(&n), dir: d } }));
                    lines = ranked(rest, &rows, 0);
                    complete = rest.is_empty();
                }
                listed = Some(dir);
            }
        } else {
            let sb = &self.nav.sidebar;
            let mut seen: Vec<Location> = Vec::new();
            for s in sb.prefs.order().into_iter().filter(|&s| s != Section::Tags) {
                for it in sb.section_items(s) {
                    if !seen.contains(&it.loc) {
                        seen.push(it.loc.clone());
                        rows.push(place_row(it));
                    }
                }
            }
            lines = ranked(text, &rows, 0);
        }
        let suggested = if complete { Vec::new() } else { self.suggest(text) };
        if suggested.is_empty() {
            return (rows, lines, listed);
        }
        let known: Vec<String> = lines.iter().filter_map(|l| if let Line::Row(k, _) = l { rows[*k].loc().map(|l| place_key(&l.display())) } else { None }).collect();
        for (t, typed) in suggested {
            if known.contains(&place_key(&t)) {
                continue;
            }
            lines.push(Line::Row(rows.len(), Vec::new()));
            rows.push(Row { label: t.clone(), path: String::new(), kind: if typed { Kind::Typed(t) } else { Kind::Suggested(t) } });
        }
        (rows, lines, listed)
    }

    fn crumb_lines(&mut self, i: usize, text: &str) -> (Vec<Row>, Vec<Line>, Option<Location>, Option<Vec<u8>>) {
        let t = self.tab();
        let (Some(loc), mark) = (t.crumb(i).map(|c| c.1.clone()), t.crumb(i + 1).map(|c| c.1.file_name().to_vec())) else { return Default::default() };
        let rows: Vec<Row> = match self.listing(&loc) {
            Listed::Found(entries) => entries.into_iter().map(|(n, d)| Row { label: name::display(&n).into_owned(), path: String::new(), kind: Kind::Entry { loc: loc.join(&n), dir: d } }).collect(),
            _ => Vec::new(),
        };
        let lines = ranked(text.trim(), &rows, 0);
        (rows, lines, Some(loc), mark)
    }

    pub(super) fn go_refresh(&mut self) {
        let Some((text, crumb, collapsed)) = self.nav.palette.as_ref().and_then(|p| Some((p.field.text().to_owned(), p.go.as_ref()?.crumb, p.go.as_ref()?.collapsed.clone()))) else { return };
        let (rows, lines, dir, mark) = match crumb {
            Some(i) => self.crumb_lines(i, &text),
            None => {
                let (r, l, d) = self.go_lines(&text, &collapsed);
                (r, l, d, None)
            }
        };
        let Some(p) = self.nav.palette.as_mut() else { return };
        let fresh = p.go.as_ref().is_none_or(|g| g.seen.as_deref() != Some(text.as_str()) || (g.rows.is_empty() && !rows.is_empty()));
        let first = lines.iter().position(|l| matches!(l, Line::Row(..)));
        let marked = mark.as_ref().and_then(|m| lines.iter().position(|l| matches!(l, Line::Row(k, _) if matches!(&rows[*k].kind, Kind::Entry { loc, .. } if loc.file_name() == m.as_slice()))));
        let count = rows.len();
        p.matches = (0..lines.len()).collect();
        p.marks = vec![Vec::new(); lines.len()];
        let Some(g) = p.go.as_mut() else { return };
        (g.rows, g.lines, g.dir, g.mark, g.count) = (rows, lines, dir, mark, count);
        if fresh {
            g.seen = Some(text.clone());
            g.picked = (crumb.is_none() && text.is_empty() && first.is_some()) || marked.is_some();
            p.sel = marked.or(first).unwrap_or(0);
            p.scroll.snap(0.0);
            let sel = p.sel as isize;
            p.select(sel);
            p.scroll.snap(p.scroll.target);
        } else {
            p.sel = p.sel.min(p.matches.len().saturating_sub(1));
        }
        self.dirty = true;
    }

    fn picked_row(&self) -> Option<Row> {
        let p = self.nav.palette.as_ref()?;
        let g = p.go.as_ref().filter(|g| g.picked)?;
        match g.lines.get(p.sel)? {
            Line::Row(k, _) => g.rows.get(*k).cloned(),
            Line::Header(_) => None,
        }
    }

    fn set_go_text(&mut self, text: &str) {
        if let Some(p) = self.nav.palette.as_mut() {
            p.field.edit.set(text);
        }
        self.go_refresh();
        self.layout();
    }

    pub(super) fn go_activate(&mut self, line: usize, now: Instant) {
        let Some(p) = self.nav.palette.as_mut() else { return };
        let Some(g) = p.go.as_mut() else { return };
        if let Some(Line::Header(s)) = g.lines.get(line) {
            let s = *s;
            match g.collapsed.iter().position(|&x| x == s) {
                Some(k) => {
                    g.collapsed.remove(k);
                }
                None => g.collapsed.push(s),
            }
            g.seen = None;
            self.go_refresh();
            if let Some(p) = self.nav.palette.as_mut() {
                let at = p.go.as_ref().and_then(|g| g.lines.iter().position(|l| *l == Line::Header(s))).unwrap_or(0);
                p.select(at as isize);
            }
            return self.layout();
        }
        p.sel = line;
        g.picked = true;
        let Some(row) = self.picked_row() else { return };
        self.close_palette();
        match row.kind {
            Kind::Place(it) if it.glyph == G_FILE => self.place_command(super::sidebar::PlaceCmd::OpenFile(it.loc.path().clone()), now),
            Kind::Place(it) => self.dispatch(vec![Route { folder: it.loc, select: None }], OpenTarget::CurrentTab, now),
            Kind::Entry { loc, dir: true } => self.navigate(loc, now),
            Kind::Entry { loc, dir: false } => {
                let tab = self.tab().id;
                self.effects.push(Effect::Provider { provider: loc.provider(), req: ProviderRequest::Open { tab, path: loc.path().clone() } });
            }
            Kind::Typed(t) | Kind::Suggested(t) => self.submit_address(&t, now),
        }
    }

    fn go_enter(&mut self, now: Instant) {
        let Some(p) = self.nav.palette.as_ref() else { return };
        let picked = p.go.as_ref().is_some_and(|g| g.picked);
        let (sel, text, crumb) = (p.sel, p.field.text().to_owned(), p.go.as_ref().and_then(|g| g.crumb));
        if picked {
            return self.go_activate(sel, now);
        }
        if crumb.is_none() {
            self.close_palette();
            self.submit_address(&text, now);
        }
    }

    fn go_expand(&mut self) {
        let Some(row) = self.picked_row().filter(Row::folder) else { return };
        let text = match &row.kind {
            Kind::Typed(t) | Kind::Suggested(t) => t.clone(),
            _ => row.loc().map(|l| with_sep(&l)).unwrap_or_default(),
        };
        if let Some(p) = self.nav.palette.as_mut() {
            if let Some(g) = p.go.as_mut() {
                g.crumb = None;
            }
            p.pick = Pick::GoTo;
        }
        self.set_go_text(&text);
    }

    fn go_select_in_panel(&mut self, now: Instant) {
        let Some(loc) = self.picked_row().and_then(|r| r.loc()) else { return };
        let Some(folder) = loc.parent() else { return };
        self.close_palette();
        self.dispatch(vec![Route { folder, select: Some(loc.file_name().to_vec()) }], OpenTarget::CurrentTab, now);
    }

    pub(super) fn go_history_step(&mut self, n: isize, now: Instant) {
        self.jump_history(n, now);
        let text = with_sep(self.current_location());
        self.set_go_text(&text);
    }

    pub(super) fn go_up_step(&mut self) {
        let base = self.nav.palette.as_ref().and_then(|p| p.go.as_ref()?.dir.clone()).unwrap_or_else(|| self.current_location().clone());
        if let Some(parent) = base.parent() {
            self.set_go_text(&with_sep(&parent));
        }
    }

    fn crumb_step(&mut self, by: isize) {
        let Some(i) = self.nav.palette.as_ref().and_then(|p| p.go.as_ref()?.crumb) else { return };
        if let Some(to) = i.checked_add_signed(by).filter(|&k| self.tab().crumb(k).is_some()) {
            self.open_crumb(to);
        }
    }

    pub(super) fn go_key(&mut self, key: Key, m: Mods, now: Instant) -> bool {
        let Some(crumb) = self.nav.palette.as_ref().and_then(|p| p.go.as_ref()).map(|g| g.crumb) else { return false };
        let plain = !m.ctrl && !m.alt && !m.shift;
        match (key, crumb) {
            (Key::Left, Some(_)) if m.alt && !m.ctrl => self.crumb_step(-1),
            (Key::Right, Some(_)) if m.alt && !m.ctrl => self.crumb_step(1),
            (Key::Left, None) if m.alt && !m.ctrl => self.go_history_step(-1, now),
            (Key::Right, None) if m.alt && !m.ctrl => self.go_history_step(1, now),
            (Key::Up, None) if m.alt && !m.ctrl => self.go_up_step(),
            (Key::BrowserBack, None) => self.go_history_step(-1, now),
            (Key::BrowserForward, None) => self.go_history_step(1, now),
            (Key::Tab, _) if plain => self.go_expand(),
            (Key::Enter, _) if m.ctrl && !m.alt => self.go_select_in_panel(now),
            (Key::Enter, _) if !m.alt => self.go_enter(now),
            (Key::Up | Key::Down, _) if plain => {
                let Some(p) = self.nav.palette.as_mut() else { return true };
                let Some(g) = p.go.as_mut() else { return true };
                if !g.picked {
                    g.picked = true;
                    let n = p.matches.len();
                    p.sel = if key == Key::Down { 0 } else { n.saturating_sub(1) };
                    let sel = p.sel as isize;
                    p.select(sel);
                } else {
                    let to = p.sel as isize + if key == Key::Down { 1 } else { -1 };
                    p.select(to);
                }
            }
            _ => return false,
        }
        self.layout();
        self.dirty = true;
        true
    }

    pub(super) fn go_button(&mut self, k: usize, now: Instant) {
        match k {
            0 => self.go_history_step(-1, now),
            1 => self.go_history_step(1, now),
            _ => self.go_up_step(),
        }
    }

    pub(super) fn go_placeholder(&self, p: &Palette) -> String {
        match p.go.as_ref() {
            Some(g) if g.crumb.is_some() => crate::trn!(P::AddressFilterFiles, g.count),
            _ => S::AddressGotoPlaceholder.t().to_owned(),
        }
    }

    pub(super) fn go_field_icon(&self, p: &Palette) -> Option<Option<SystemIcon>> {
        let go = p.go.as_ref().filter(|g| g.crumb.is_none())?;
        let dir = go.dir.as_ref().unwrap_or_else(|| self.current_location());
        Some(folder_icon(dir, &self.nav.sidebar, (ICON * self.scale).round() as u32))
    }

    pub(super) fn go_buttons_enabled(&self) -> [bool; 3] {
        let h = &self.tab().history;
        let base = self.nav.palette.as_ref().and_then(|p| p.go.as_ref()?.dir.clone()).unwrap_or_else(|| self.current_location().clone());
        [h.can_back(), h.can_forward(), base.parent().is_some()]
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw_go_rows<B: TextBackend>(&self, g: &mut Gfx<B>, t: &Theme, p: &Palette, font: FontStyle, small: FontStyle, icons: FontStyle, requests: &mut Vec<IconRequest>) {
        let Some(go) = p.go.as_ref() else { return };
        let sc = self.scale;
        let s = |v: f32| (v * sc).round();
        let (s4, s6, s12) = (s(4.0), s(6.0), s(12.0));
        let px = s(ICON);
        let sb = &self.nav.sidebar;
        let chevron = self.icon_font(8.0);
        for (k, &row) in p.rows.iter().enumerate() {
            let i = p.first + k;
            let Some(line) = p.matches.get(i).and_then(|&m| go.lines.get(m)) else { break };
            let selected = go.picked && i == p.sel;
            let hover = self.hover == Hit::PaletteRow(i);
            match line {
                Line::Header(sec) => {
                    let hot = selected || hover;
                    let head = Rect::new(row.x, row.y + s(2.0), row.w, row.h - s(4.0));
                    if hot {
                        g.dl.round_rect(head, t.accent_fill, s4);
                    }
                    let ink = if hot { ink_on(t.accent_fill) } else { t.text };
                    let mut w = Paint::new(g, t, sc);
                    w.centered(Rect::new(head.x + s6, head.y, s(16.0) + s6, head.h), sec.glyph(), icons, ink);
                    let y = baseline(w.g, head, font);
                    w.g.text(head.x + s(16.0) + 2.0 * s12, y, sec.title(), font, ink, head.w);
                    let glyph = if go.collapsed.contains(sec) { G_COLLAPSED } else { G_EXPANDED };
                    w.centered(Rect::new(head.right() - s12 - s(16.0), head.y, s(16.0), head.h), glyph, icons, ink);
                }
                Line::Row(n, marks) => {
                    let r = &go.rows[*n];
                    let marked = matches!(&r.kind, Kind::Entry { loc, .. } if go.mark.as_deref() == Some(loc.file_name()));
                    if selected || (marked && !go.picked) {
                        g.dl.round_rect(row, t.accent_fill, s4);
                    } else if hover {
                        g.dl.round_rect(row, t.hover, s4);
                    }
                    let lit = selected || (marked && !go.picked);
                    let (ink, dim, mark) = if lit { (ink_on(t.accent_fill), ink_on(t.accent_fill).alpha(0xC0), ink_on(t.accent_fill)) } else { (t.text, t.text_dim, t.accent) };
                    let icon = Rect::new(row.x + s(10.0), row.y, px, row.h);
                    let (system, glyph) = match &r.kind {
                        Kind::Place(it) => (place_icon_request(it, &sb.places, px as u32), it.glyph),
                        Kind::Entry { loc, dir } => (entry_icon(loc, *dir, sb, px as u32), if *dir { G_FOLDER } else { G_FILE }),
                        Kind::Typed(t) => (Location::parse(t).filter(|_| t.ends_with(SEPS)).and_then(|l| folder_icon(&l, sb, px as u32)), G_TYPED),
                        Kind::Suggested(t) => (Location::parse(t).and_then(|l| folder_icon(&l, sb, px as u32)), G_FOLDER),
                    };
                    if !self.system_icon(g, system, icon, px, 0xFF, requests) {
                        Paint::new(g, t, sc).centered(icon, glyph, icons, if lit { ink } else { t.accent });
                    }
                    let x = icon.right() + s(8.0);
                    let usage = if let Kind::Place(it) = &r.kind { it.usage } else { None };
                    let right = row.right() - s12 - if usage.is_some() { s(BAR_W) + s12 } else { 0.0 } - if marked { s(16.0) + s6 } else { 0.0 };
                    let y = baseline(g, row, font);
                    let w = Paint::new(g, t, sc).marked((x, y), &r.label, marks, font, (ink, mark), right - x);
                    if !r.path.is_empty() && x + w + s6 < right {
                        let cx = x + w + s6;
                        let cw = g.text(cx, y - s(1.0), G_COLLAPSED, chevron, dim, right - cx);
                        g.text(cx + cw + s6, y, &r.path, small, dim, right - cx - cw - s6);
                    }
                    if let Some(used) = usage {
                        let bar = Rect::new(row.right() - s12 - s(BAR_W), row.y + (row.h - s(4.0)) / 2.0, s(BAR_W), s(4.0));
                        g.dl.round_rect(bar, t.separator, bar.h / 2.0);
                        g.dl.round_rect(Rect::new(bar.x, bar.y, (bar.w * used.clamp(0.0, 1.0)).max(bar.h), bar.h), if used >= 0.9 { t.error } else { t.accent }, bar.h / 2.0);
                    }
                    if marked {
                        Paint::new(g, t, sc).centered(Rect::new(row.right() - s12 - s(16.0), row.y, s(16.0), row.h), G_MARK, icons, ink);
                    }
                }
            }
        }
    }

    pub(super) fn draw_go_buttons<B: TextBackend>(&self, g: &mut Gfx<B>, t: &Theme, p: &Palette, icons: FontStyle) {
        let Some(go) = p.go.as_ref().filter(|g| g.crumb.is_none()) else { return };
        let enabled = self.go_buttons_enabled();
        let mut w = Paint::new(g, t, self.scale);
        for (k, glyph) in [G_BACK, G_FORWARD, G_UP].into_iter().enumerate() {
            w.icon_button(go.buttons[k], glyph, icons, Button::of(self.hover == Hit::GoButton(k), false, enabled[k]), t.text_dim);
        }
        let last = go.buttons[2];
        w.g.dl.rect(Rect::new(last.right() + self.s(4.0), last.y + self.s(6.0), 1.0, last.h - self.s(12.0)), t.separator);
    }
}

#[cfg(test)]
#[path = "../../test/ui/goto.rs"]
mod tests;
