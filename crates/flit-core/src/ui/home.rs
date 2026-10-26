use super::sidebar::{KnownFolder, PlaceCmd, G_HOME};
use super::tags::G_TAG;
use super::{centered, App, Effect, Hit, Route, G_DRIVE, G_FILE, G_FOLDER};
use crate::fmt as format;
use crate::i18n::S;
use crate::location::Location;
use crate::open::OpenTarget;
use crate::path::FsPath;
use crate::render::{Color, Rect};
use crate::text::{FontFamily, FontStyle, Gfx, TextBackend};
use crate::theme::Theme;
use std::time::Instant;

const PAD: f32 = 24.0;
const TITLE_H: f32 = 36.0;
const GAP: f32 = 8.0;
const FOLDER_CARD: (f32, f32) = (200.0, 52.0);
const DRIVE_CARD: (f32, f32) = (260.0, 68.0);
const RECENT_H: f32 = 30.0;
const NOTE_H: f32 = 28.0;
const G_WARNING: &str = "\u{E7BA}";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Widget {
    QuickAccess,
    Drives,
    Tags,
    Recent,
}

impl Widget {
    pub const ALL: [Widget; 4] = [Widget::QuickAccess, Widget::Drives, Widget::Tags, Widget::Recent];

    pub fn title(self) -> &'static str {
        match self {
            Widget::QuickAccess => S::HomeWidgetQuickAccess.t(),
            Widget::Drives => S::HomeWidgetDrives.t(),
            Widget::Tags => S::HomeWidgetTags.t(),
            Widget::Recent => S::HomeWidgetRecent.t(),
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Widget::QuickAccess => "quick_access",
            Widget::Drives => "drives",
            Widget::Tags => "tags",
            Widget::Recent => "recent",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Recent {
    pub name: String,
    pub target: FsPath,
    pub dir: bool,
    pub at: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Card {
    Folder { label: String, loc: Location },
    Drive(usize),
    Tag(usize),
    Recent(usize),
}

#[derive(Default)]
pub(super) struct Home {
    pub cards: Vec<(Rect, Card)>,
    pub titles: Vec<(Rect, Widget)>,
    pub notes: Vec<(Rect, S, bool)>,
    pub area: Rect,
    scroll: f32,
    content_h: f32,
}

impl App {
    pub(super) fn on_home(&self) -> bool {
        self.tab().location().is_home_page()
    }

    pub(super) fn go_home_page(&mut self, new_tab: bool, now: Instant) {
        let target = if new_tab { OpenTarget::NewTab } else { OpenTarget::CurrentTab };
        self.dispatch(vec![Route { folder: Location::home_page(), select: None }], target, now);
    }

    pub(super) fn place_loaded(&mut self, k: usize) {
        if self.tabs[k].location().is_home_page() {
            self.effects.push(Effect::RefreshPlaces);
        }
        self.library_crumbs(k);
    }

    pub(super) fn widget_shown(&self, w: Widget) -> bool {
        !self.nav.sidebar.prefs.widgets_off.iter().any(|s| s == w.id())
    }

    pub(super) fn toggle_widget(&mut self, w: Widget) {
        let off = &mut self.nav.sidebar.prefs.widgets_off;
        match off.iter().position(|s| s == w.id()) {
            Some(i) => {
                off.remove(i);
            }
            None => off.push(w.id().to_owned()),
        }
        self.layout();
        self.dirty = true;
    }

    pub(super) fn recent_items(&self) -> impl Iterator<Item = (usize, &Recent)> {
        self.nav.sidebar.recent()
    }

    pub(super) fn forget_recent(&mut self, path: FsPath) {
        let Some(at) = self.nav.sidebar.places.recent.iter().filter(|r| r.target == path).map(|r| r.at).max() else { return };
        let p = &mut self.nav.sidebar.prefs;
        p.recent_hidden.retain(|(q, _)| *q != path);
        p.recent_hidden.push((path, at));
        self.nav_prefs_changed();
    }

    pub(super) fn clear_recent(&mut self) {
        let newest = self.recent_items().map(|(_, r)| r.at).max();
        let p = &mut self.nav.sidebar.prefs;
        if let Some(at) = newest {
            p.recent_cleared = p.recent_cleared.max(at);
            p.recent_hidden.clear();
        }
        self.nav_prefs_changed();
    }

    fn folder_cards(&self) -> Vec<Card> {
        let sb = &self.nav.sidebar;
        let mut cards: Vec<Card> = sb.items.iter().filter(|it| sb.is_pinned(&it.loc)).map(|it| Card::Folder { label: it.label.clone(), loc: it.loc.clone() }).collect();
        for (k, loc) in &sb.places.folders {
            if *k != KnownFolder::Home && !sb.is_pinned(loc) {
                cards.push(Card::Folder { label: k.label().to_owned(), loc: loc.clone() });
            }
        }
        cards
    }

    pub(super) fn layout_home(&mut self) {
        let l = &self.layout;
        let area = Rect::new(l.header.x, l.header.y, l.header.w, (l.bottom - l.header.y).max(0.0));
        let mut folders = if self.on_home() && self.widget_shown(Widget::QuickAccess) { self.folder_cards() } else { Vec::new() };
        let recent: Vec<usize> = self.recent_items().map(|(i, _)| i).collect();
        let sc = self.scale;
        let s = |v: f32| (v * sc).round();
        let shown: Vec<Widget> = Widget::ALL.into_iter().filter(|&w| self.widget_shown(w)).collect();
        let (drives, tags, recent_off, on_home) = (self.nav.sidebar.places.drives.len(), self.nav.sidebar.tags.defs.len(), self.nav.sidebar.places.recent_off, self.on_home());
        let h = &mut self.nav.home;
        h.area = area;
        h.cards.clear();
        h.titles.clear();
        h.notes.clear();
        if !on_home {
            return;
        }
        let (x0, w) = (area.x + s(PAD), (area.w - 2.0 * s(PAD)).max(s(80.0)));
        let mut y = area.y + s(12.0);
        let grid = |cards: &mut Vec<(Rect, Card)>, y: &mut f32, items: Vec<Card>, (cw, ch): (f32, f32)| {
            let (cw, ch, gap) = (s(cw), s(ch), s(GAP));
            let cols = (((w + gap) / (cw + gap)).floor() as usize).max(1);
            let rows = items.len().div_ceil(cols);
            for (k, c) in items.into_iter().enumerate() {
                cards.push((Rect::new(x0 + (k % cols) as f32 * (cw + gap), *y + (k / cols) as f32 * (ch + gap), cw, ch), c));
            }
            *y += rows as f32 * (ch + gap);
        };
        for widget in shown {
            h.titles.push((Rect::new(x0, y, w, s(TITLE_H)), widget));
            y += s(TITLE_H);
            match widget {
                Widget::QuickAccess => grid(&mut h.cards, &mut y, std::mem::take(&mut folders), FOLDER_CARD),
                Widget::Drives => grid(&mut h.cards, &mut y, (0..drives).map(Card::Drive).collect(), DRIVE_CARD),
                Widget::Tags if tags == 0 => {
                    h.notes.push((Rect::new(x0, y, w, s(NOTE_H)), S::HomeNoTags, false));
                    y += s(NOTE_H);
                }
                Widget::Tags => grid(&mut h.cards, &mut y, (0..tags).map(Card::Tag).collect(), FOLDER_CARD),
                Widget::Recent => {
                    if recent_off {
                        h.notes.push((Rect::new(x0, y, w, s(NOTE_H)), S::HomeRecentOff, true));
                        y += s(NOTE_H);
                    } else if recent.is_empty() {
                        h.notes.push((Rect::new(x0, y, w, s(NOTE_H)), S::HomeNoRecent, false));
                        y += s(NOTE_H);
                    }
                    for &i in &recent {
                        h.cards.push((Rect::new(x0, y, w, s(RECENT_H)), Card::Recent(i)));
                        y += s(RECENT_H);
                    }
                }
            }
            y += s(16.0);
        }
        h.content_h = y - area.y;
        h.scroll = h.scroll.clamp(0.0, (h.content_h - area.h).max(0.0));
        let dy = -h.scroll;
        for r in h.cards.iter_mut().map(|(r, _)| r).chain(h.titles.iter_mut().map(|(r, _)| r)).chain(h.notes.iter_mut().map(|(r, _, _)| r)) {
            r.y += dy;
        }
    }

    pub(super) fn home_hit(&self, x: f32, y: f32) -> Option<Hit> {
        let h = &self.nav.home;
        if !self.on_home() || !h.area.contains(x, y) {
            return None;
        }
        Some(h.cards.iter().position(|(r, _)| r.contains(x, y)).map_or(Hit::HomeEmpty, Hit::HomeItem))
    }

    pub(super) fn home_wheel(&mut self, dy: f32) {
        let h = &mut self.nav.home;
        h.scroll = (h.scroll - dy * 3.0 * (RECENT_H * self.scale).round()).clamp(0.0, (h.content_h - h.area.h).max(0.0));
        self.layout();
        self.dirty = true;
    }

    fn card_location(&self, i: usize) -> Option<Location> {
        let p = &self.nav.sidebar.places;
        Some(match &self.nav.home.cards.get(i)?.1 {
            Card::Folder { loc, .. } => loc.clone(),
            Card::Drive(d) => Location::Local(p.drives.get(*d)?.path.clone()),
            Card::Tag(_) => return None,
            Card::Recent(r) => Location::from_local(p.recent.get(*r)?.target.clone()),
        })
    }

    fn card_tag(&self, i: usize) -> Option<usize> {
        match self.nav.home.cards.get(i)?.1 {
            Card::Tag(k) => Some(k),
            _ => None,
        }
    }

    pub(super) fn open_home_item(&mut self, i: usize, new_tab: bool, now: Instant) {
        if let Some(k) = self.card_tag(i) {
            return self.show_tag(k);
        }
        let Some(loc) = self.card_location(i) else { return };
        let file = matches!(self.nav.home.cards[i].1, Card::Recent(r) if !self.nav.sidebar.places.recent[r].dir);
        match (file, new_tab) {
            (true, _) => self.place_command(PlaceCmd::OpenFile(loc.path().clone()), now),
            (false, true) => self.open_in_tabs(vec![Route { folder: loc, select: None }], now),
            (false, false) => self.dispatch(vec![Route { folder: loc, select: None }], OpenTarget::CurrentTab, now),
        }
    }

    pub(super) fn home_menu(&mut self, x: f32, y: f32, h: Hit) {
        let Hit::HomeItem(i) = h else { return self.home_page_menu(x, y) };
        if let Some(k) = self.card_tag(i) {
            let menu = self.build_tag_menu(Some(k));
            return self.flit_menu(x, y, menu);
        }
        let Some(loc) = self.card_location(i) else { return };
        let Card::Recent(r) = self.nav.home.cards[i].1 else { return self.place_menu(x, y, loc, |_, _| {}) };
        self.recent_menu(x, y, r);
    }

    pub(super) fn recent_menu(&mut self, x: f32, y: f32, r: usize) {
        let Some(loc) = self.nav.sidebar.places.recent.get(r).map(|e| Location::from_local(e.target.clone())) else { return };
        if self.nav.sidebar.places.recent[r].dir {
            let path = loc.path().clone();
            return self.place_menu(x, y, loc, |_, m| {
                m.separator();
                m.place(S::HomeRemoveFromList.t(), PlaceCmd::Forget(path), false);
            });
        }
        let path = loc.path().clone();
        let menu = self.collect(|_, m| {
            m.place(S::MenuOpen.t(), PlaceCmd::OpenFile(path.clone()), false);
            m.place(S::MenuOpenFileLocation.t(), PlaceCmd::Reveal(path.clone()), false);
            m.place(S::MenuCopyPath.t(), PlaceCmd::CopyPath(loc.clone()), false);
            m.separator();
            m.place(S::HomeRemoveFromList.t(), PlaceCmd::Forget(path.clone()), false);
            m.place(S::HomeClearList.t(), PlaceCmd::ClearRecent, false);
            m.separator();
            m.place(S::MenuProperties.t(), PlaceCmd::Properties(path.clone()), false);
        });
        self.show_place_menu(x, y, Some(path), menu);
    }

    fn home_page_menu(&mut self, x: f32, y: f32) {
        let menu = self.collect(|a, m| {
            a.here_pane_items(m);
            for w in Widget::ALL {
                m.place(w.title(), PlaceCmd::Widget(w), a.widget_shown(w));
            }
            if a.widget_shown(Widget::Recent) && a.recent_items().next().is_some() {
                m.separator();
                m.place(S::HomeClearRecent.t(), PlaceCmd::ClearRecent, false);
            }
        });
        self.flit_menu(x, y, menu);
    }

    pub(super) fn draw_home<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme) {
        let h = &self.nav.home;
        let area = h.area;
        g.dl.rect(area, t.bg);
        g.dl.set_clip(Some(area));
        let title = FontStyle::new(FontFamily::Ui, 15.0 * self.scale, true);
        let (font, small) = (self.ui_font(13.0), self.ui_font(12.0));
        let (icons, big_icons) = (self.icon_font(14.0), self.icon_font(22.0));
        let (mt, m, ms) = (g.metrics(title), g.metrics(font), g.metrics(small));
        let (s4, s6, s8, s12, s40) = (self.s(4.0), self.s(6.0), self.s(8.0), self.s(12.0), self.s(40.0));
        let mid = |r: Rect, fm: crate::text::FontMetrics| (r.y + (r.h + fm.ascent - fm.descent) / 2.0).round();
        for &(r, w) in &h.titles {
            g.text(r.x, mid(r, mt), w.title(), title, t.text, r.w);
        }
        for &(r, note, warn) in &h.notes {
            if warn {
                centered(g, Rect::new(r.x, r.y, self.s(20.0), r.h), G_WARNING, icons, t.error);
            }
            let x = if warn { r.x + self.s(26.0) } else { r.x };
            g.text(x, mid(r, ms), note.t(), small, t.text_dim, r.right() - x);
        }
        let places = &self.nav.sidebar.places;
        let mut text = String::new();
        for (i, (r, card)) in h.cards.iter().enumerate() {
            if r.bottom() < area.y || r.y > area.bottom() {
                continue;
            }
            let hovered = self.hover == Hit::HomeItem(i);
            match card {
                Card::Folder { label, loc } => {
                    g.dl.round_rect(*r, if hovered { t.button_hover } else { t.chrome }, s6);
                    let glyph = if loc.is_home_page() { G_HOME } else { G_FOLDER };
                    centered(g, Rect::new(r.x + s4, r.y, s40, r.h), glyph, big_icons, t.accent);
                    g.text(r.x + s40 + s8, mid(*r, m), label, font, t.text, r.w - s40 - s12);
                }
                Card::Drive(d) => {
                    let Some(d) = places.drives.get(*d) else { continue };
                    g.dl.round_rect(*r, if hovered { t.button_hover } else { t.chrome }, s6);
                    centered(g, Rect::new(r.x + s4, r.y, s40, r.h), G_DRIVE, big_icons, t.text_dim);
                    let tx = r.x + s40 + s8;
                    let tw = r.right() - s12 - tx;
                    g.text(tx, r.y + s8 + m.ascent, &d.title(), font, t.text, tw);
                    if d.total > 0 {
                        let used = d.total.saturating_sub(d.free) as f32 / d.total as f32;
                        let bar = Rect::new(tx, r.y + (r.h / 2.0).round() + s4 - self.s(2.0), tw.max(0.0), self.s(6.0));
                        g.dl.round_rect(bar, t.separator, bar.h / 2.0);
                        let fill = if used >= 0.9 { t.error } else { t.accent };
                        g.dl.round_rect(Rect::new(bar.x, bar.y, (bar.w * used.clamp(0.0, 1.0)).max(bar.h), bar.h), fill, bar.h / 2.0);
                        let (mut free, mut total) = (String::new(), String::new());
                        format::size_in(&mut free, d.free, self.style.decimal);
                        format::size_in(&mut total, d.total, self.style.decimal);
                        text.clear();
                        crate::tr_into!(&mut text, S::HomeDriveFree, free = free, total = total);
                        g.text(tx, r.bottom() - s6 - ms.descent, &text, small, t.text_dim, tw);
                    }
                }
                Card::Tag(k) => {
                    let Some(tag) = self.nav.sidebar.tags.defs.get(*k) else { continue };
                    g.dl.round_rect(*r, if hovered { t.button_hover } else { t.chrome }, s6);
                    centered(g, Rect::new(r.x + s4, r.y, s40, r.h), G_TAG, big_icons, Color::rgb(tag.color));
                    g.text(r.x + s40 + s8, mid(*r, m), &tag.name, font, t.text, r.w - s40 - s12);
                }
                Card::Recent(k) => {
                    let Some(e) = places.recent.get(*k) else { continue };
                    if hovered {
                        g.dl.round_rect(*r, t.button_hover, s4);
                    }
                    let (glyph, color) = if e.dir { (G_FOLDER, t.accent) } else { (G_FILE, t.text_dim) };
                    centered(g, Rect::new(r.x, r.y, self.s(28.0), r.h), glyph, icons, color);
                    text.clear();
                    format::datetime_in(&mut text, e.at, self.utc_offset_min, &self.style);
                    let dw = g.measure(&text, small);
                    g.text(r.right() - s8 - dw, mid(*r, ms), &text, small, t.text_dim, f32::INFINITY);
                    let x = r.x + self.s(36.0);
                    let avail = r.right() - s8 - dw - s12 - x;
                    let nw = g.text(x, mid(*r, m), &e.name, font, t.text, avail * 0.6);
                    let folder = e.target.parent().map(|p| p.display().into_owned()).unwrap_or_default();
                    g.text(x + nw + s12, mid(*r, ms), &folder, small, t.text_dim, avail - nw - s12);
                }
            }
        }
        g.dl.set_clip(None);
    }
}

#[cfg(test)]
#[path = "../../test/ui/home.rs"]
mod tests;
