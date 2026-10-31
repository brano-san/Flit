use super::actions::{Group, ACTIONS};
use super::edit::EditAction;
use super::field::{Field, FieldKey};
use super::fuzzy::fuzzy;
use super::goto::Go;
use super::settings_page::SettingPick;
use super::tags::TagPick;
use super::toolbar::ToolPick;
use super::widgets::{baseline, Button, Paint, G_NEXT, G_OPEN};
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
const DESC_ROW_H: f32 = 46.0;
const FIELD_H: f32 = 32.0;
const TOP_GAP: f32 = 8.0;
const EDGE: f32 = 12.0;
const HINT_H: f32 = 36.0;
const SCROLL_W: f32 = 14.0;
const CRUMB_W: f32 = 320.0;
const TABS_W: f32 = 380.0;
const GO_W: f32 = 720.0;
const DESC_PENALTY: i32 = 100_000;
const G_SEARCH: &str = "\u{E721}";
const G_DESCRIBE: &str = "\u{E8A5}";
const G_SHORTCUTS: &str = "\u{E8AC}";
const CHIP: f32 = 24.0;

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Line {
    Header(Group),
    Item(usize, Vec<Range<usize>>, Vec<Range<usize>>),
}

#[derive(Default)]
pub(super) struct Commands {
    pub lines: Vec<Line>,
    pub collapsed: Vec<Group>,
    pub descriptions: bool,
    groups: Vec<Group>,
    shown: Vec<&'static str>,
    caps: Vec<Vec<String>>,
    pub live: Vec<bool>,
}

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
    GoTo,
    Crumb(usize),
    Tabs,
    SaveLayout,
    LoadLayout,
    Keys(usize),
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
            Pick::GoTo | Pick::Crumb(_) => S::AddressGotoPlaceholder.t(),
            Pick::Tabs => S::MenusSearchTabsShort.t(),
            Pick::SaveLayout => S::MenusSaveLayoutPlaceholder.t(),
            Pick::LoadLayout => S::MenusLoadLayoutPlaceholder.t(),
            Pick::Keys(_) => S::PalettePlaceholderCommands.t(),
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
            Pick::GoTo | Pick::Crumb(_) => S::AddressGotoEmpty.t(),
            Pick::Tabs => S::MenusNoTabs.t(),
            Pick::SaveLayout => S::MenusSaveLayoutHint.t(),
            Pick::LoadLayout => S::MenusNoLayouts.t(),
            Pick::Keys(_) => S::PaletteEmptyCommands.t(),
        }
    }

    fn edits_keys(&self) -> bool {
        matches!(self, Pick::Keys(_) | Pick::Setting(SettingPick::Capture { .. } | SettingPick::ResetKeys))
    }

    fn is_input(&self) -> bool {
        matches!(self, Pick::Shortcut(_) | Pick::Stream(_) | Pick::FolderIcon(_) | Pick::NewLibrary | Pick::LibraryAdd(_) | Pick::Server | Pick::SaveLayout) || matches!(self, Pick::Tags(t) if t.is_input()) || matches!(self, Pick::Setting(p) if p.is_input())
    }
}

pub(super) struct Palette {
    pub pick: Pick,
    pub titles: Vec<String>,
    pub(super) field: Field,
    pub matches: Vec<usize>,
    pub marks: Vec<Vec<Range<usize>>>,
    pub keys: Vec<String>,
    alts: Vec<&'static str>,
    descs: Vec<String>,
    pub ids: Vec<usize>,
    pub sel: usize,
    pub first: usize,
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
    pub commands: Option<Commands>,
    describe: Rect,
    pub go: Option<Go>,
    pub placeholder: Option<String>,
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

fn searched_description(i: usize) -> String {
    let (local, en) = (ACTIONS[i].desc.t(), ACTIONS[i].desc.en());
    if local == en { en.to_owned() } else { format!("{local} {en}") }
}

fn desc_marks(query: &str, desc: &str) -> Vec<Range<usize>> {
    let limit = query.chars().filter(|c| !c.is_whitespace()).count().div_ceil(3).max(1);
    fuzzy(query, desc).filter(|m| m.ranges.len() <= limit).map(|m| m.ranges).unwrap_or_default()
}

fn keys_chip(row: Rect, scale: f32) -> Rect {
    let s = |v: f32| (v * scale).round();
    Rect::new(row.right() - s(8.0) - s(CHIP), row.y + ((row.h - s(CHIP)) / 2.0).round(), s(CHIP), s(CHIP))
}

pub(super) fn popup_top(scale: f32, h: f32, fixed: f32) -> f32 {
    let s = |v: f32| (v * scale).round();
    (s(TAB_H) + s(TOOL_H) + s(TOP_GAP)).min((h - s(EDGE) - fixed).max(s(EDGE)))
}

pub fn matches(query: &str) -> Vec<usize> {
    let descs: Vec<String> = (0..ACTIONS.len()).map(searched_description).collect();
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
            commands: None,
            describe: Rect::default(),
            go: None,
            placeholder: None,
        };
        p.field.edit.set(text);
        p.field.edit.select_all();
        p.refresh();
        p
    }

    fn commands(app: &App) -> Self {
        let mut ids: Vec<usize> = (0..ACTIONS.len()).filter(|&i| app.supports(ACTIONS[i].id)).collect();
        ids.sort_by_key(|&i| Group::ALL.iter().position(|&g| g == ACTIONS[i].group));
        let mut p = Self::new(Pick::Commands, ids.iter().map(|&i| (ACTIONS[i].title.t().to_owned(), app.keymap.text(i))).collect(), "");
        p.alts = ids.iter().map(|&i| ACTIONS[i].title.en()).collect();
        p.descs = ids.iter().map(|&i| searched_description(i)).collect();
        p.commands = Some(Commands {
            descriptions: app.prefs.command_descriptions,
            groups: ids.iter().map(|&i| ACTIONS[i].group).collect(),
            shown: ids.iter().map(|&i| ACTIONS[i].desc.t()).collect(),
            caps: ids.iter().map(|&i| app.keymap.shown(i)).collect(),
            live: ids.iter().map(|&i| app.available(ACTIONS[i].id)).collect(),
            ..Default::default()
        });
        p.ids = ids;
        p.refresh();
        p
    }

    pub(super) fn set_descs(&mut self, descs: Vec<String>) {
        self.descs = descs;
        self.refresh();
    }

    fn unit(&self) -> f32 {
        if self.commands.as_ref().is_some_and(|c| c.descriptions) || self.pick == Pick::Tabs { DESC_ROW_H } else { ROW_H }
    }

    fn command_lines(&mut self) {
        let query = self.field.text().to_owned();
        let Some(c) = &mut self.commands else { return };
        c.lines.clear();
        if query.trim().is_empty() {
            for g in Group::ALL {
                let items: Vec<usize> = (0..c.groups.len()).filter(|&k| c.groups[k] == g).collect();
                if items.is_empty() {
                    continue;
                }
                c.lines.push(Line::Header(g));
                if !c.collapsed.contains(&g) {
                    c.lines.extend(items.into_iter().map(|k| Line::Item(k, Vec::new(), Vec::new())));
                }
            }
        } else {
            let alts = self.alts.iter().copied();
            let ranked = rank(&query, self.titles.iter().map(String::as_str).zip(alts).zip(&self.descs).map(|((t, a), d)| (t, a, d.as_str())));
            c.lines.extend(ranked.into_iter().map(|(k, marks)| Line::Item(k, marks, desc_marks(&query, c.shown[k]))));
        }
        let n = c.lines.len();
        self.matches = (0..n).collect();
        self.marks = vec![Vec::new(); n];
        self.sel = 0;
        self.scroll.snap(0.0);
    }

    fn fold(&mut self, g: Group) {
        let Some(c) = &mut self.commands else { return };
        match c.collapsed.iter().position(|&x| x == g) {
            Some(k) => {
                c.collapsed.remove(k);
            }
            None => c.collapsed.push(g),
        }
        let pos = self.scroll.pos;
        self.command_lines();
        let at = self.commands.as_ref().and_then(|c| c.lines.iter().position(|l| *l == Line::Header(g))).unwrap_or(0);
        self.scroll.snap(pos.min(self.max_scroll()));
        self.select(at as isize);
    }

    fn refresh(&mut self) {
        if self.go.is_some() {
            return;
        }
        if self.commands.is_some() {
            return self.command_lines();
        }
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
        self.matches.len().saturating_sub(self.page) as f32 * self.unit()
    }

    pub(super) fn select(&mut self, to: isize) {
        let n = self.matches.len() as isize;
        if n == 0 {
            return;
        }
        self.sel = to.rem_euclid(n) as usize;
        let unit = self.unit();
        let top = self.sel as f32 * unit;
        let view = self.page as f32 * unit;
        if top < self.scroll.target {
            self.scroll.target = top;
        } else if top + unit > self.scroll.target + view {
            self.scroll.target = top + unit - view;
        }
    }

    pub(super) fn select_row(&mut self, k: usize) {
        self.select(k.min(self.matches.len().saturating_sub(1)) as isize);
    }

    pub(super) fn row_action(&self, i: usize) -> Option<usize> {
        let c = self.commands.as_ref()?;
        match c.lines.get(*self.matches.get(i)?)? {
            Line::Item(k, ..) => self.ids.get(*k).copied(),
            Line::Header(_) => None,
        }
    }

    fn rebinds(&self, i: usize) -> Option<usize> {
        self.row_action(i).filter(|&a| ACTIONS[a].rebindable())
    }

    fn reload_keys(&mut self, keymap: &super::keymap::Keymap) {
        self.keys = self.ids.iter().map(|&i| keymap.text(i)).collect();
        if let Some(c) = &mut self.commands {
            c.caps = self.ids.iter().map(|&i| keymap.shown(i)).collect();
        }
    }

}

impl App {
    pub(super) fn open_palette(&mut self) {
        self.nav.back = None;
        self.open_pick(Palette::commands(self));
    }

    pub(super) fn palette_shortcuts(&mut self, i: usize) {
        if let Some(a) = self.nav.palette.as_ref().and_then(|p| p.rebinds(i)) {
            if let Some(p) = &mut self.nav.palette {
                p.select_row(i);
            }
            self.edit_shortcuts(a);
        }
    }

    fn back_to_commands(&mut self) {
        if self.nav.palette.is_some() {
            return;
        }
        let Some(mut p) = self.nav.back.take() else { return };
        p.reload_keys(&self.keymap);
        self.nav.palette = Some(*p);
        self.layout();
        self.dirty = true;
    }

    pub(super) fn open_pick(&mut self, p: Palette) {
        let same = self.nav.palette.as_ref().is_some_and(|open| open.pick == p.pick);
        self.nav.palette = None;
        if !same {
            self.nav.palette = Some(p);
        }
        self.layout();
        self.dirty = true;
    }

    pub(super) fn toggle_descriptions(&mut self) {
        self.prefs.command_descriptions = !self.prefs.command_descriptions;
        if let Some(p) = &mut self.nav.palette {
            if let Some(c) = &mut p.commands {
                c.descriptions = self.prefs.command_descriptions;
                let sel = p.sel as isize;
                p.scroll.snap(0.0);
                p.select(sel);
                p.scroll.snap(p.scroll.target);
            }
        }
        self.layout();
        self.dirty = true;
    }

    pub(super) fn close_palette(&mut self) {
        let editing = self.nav.palette.take().is_some_and(|p| p.pick.edits_keys());
        match editing {
            true => self.back_to_commands(),
            false => self.nav.back = None,
        }
        self.dirty = true;
    }

    pub(super) fn run_palette_item(&mut self, index: usize, now: Instant) {
        if self.go_open() {
            return self.go_activate(index, now);
        }
        let Some(p) = self.nav.palette.take() else { return };
        self.dirty = true;
        if p.pick.is_input() {
            return self.submit_pick(p.pick, p.field.text().trim());
        }
        let Some(&id) = p.matches.get(index) else {
            self.nav.palette = Some(p);
            return;
        };
        if let Some(c) = &p.commands {
            match c.lines.get(id) {
                Some(Line::Header(g)) => {
                    let g = *g;
                    let mut p = p;
                    p.fold(g);
                    self.nav.palette = Some(p);
                    self.layout();
                }
                Some(Line::Item(k, ..)) if c.live[*k] => {
                    if let Some(run) = ACTIONS[p.ids[*k]].run {
                        run(self, now);
                    }
                }
                _ => self.nav.palette = Some(p),
            }
            return;
        }
        match p.pick {
            Pick::Encoding(archive) => self.set_encoding(&archive, id, now),
            Pick::Library(lib) => self.library_row(lib, &p.titles[id], false),
            Pick::Tags(t) => self.tag_choose(t, id, now),
            Pick::Setting(p) => {
                self.setting_choose(p, id);
                self.back_to_commands();
            }
            Pick::Keys(a) => {
                self.shortcut_choose(a, id);
                self.back_to_commands();
            }
            Pick::Toolbar(t) => self.tool_choose(t, id),
            Pick::Tabs => self.choose_tab_row(id, now),
            Pick::LoadLayout => self.load_layout(&p.titles[id]),
            _ => self.new_item(id),
        }
    }

    fn remove_palette_item(&mut self) -> bool {
        if let Some(p) = self.nav.palette.as_ref().filter(|p| matches!(p.pick, Pick::Keys(_))) {
            if let (Pick::Keys(a), Some(&row)) = (&p.pick, p.matches.get(p.sel)) {
                let a = *a;
                self.shortcut_remove(a, row);
            }
            return true;
        }
        if let Some(p) = self.nav.palette.take_if(|p| p.pick == Pick::LoadLayout && p.field.text().is_empty()) {
            if let Some(&id) = p.matches.get(p.sel) {
                let name = p.titles[id].clone();
                self.delete_layout(&name);
            }
            if !self.opts.layouts.is_empty() {
                self.ask_load_layout();
            }
            return true;
        }
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
        if key == Key::F(2) && m == Mods::default() {
            if let Some(sel) = self.nav.palette.as_ref().filter(|p| p.commands.is_some()).map(|p| p.sel) {
                self.palette_shortcuts(sel);
                return true;
            }
        }
        if self.go_key(key, m, now) {
            return true;
        }
        let closes = self.keymap.runs(key, m, "palette.show");
        let mut changed_go = false;
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
                        changed_go = p.go.is_some();
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
        if changed_go {
            self.go_refresh();
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
        if p.go.is_some() {
            self.go_refresh();
        }
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
            let (max, unit) = (p.max_scroll(), p.unit());
            p.scroll.by(-dy * 3.0 * unit, max);
            self.hover = Hit::PaletteBox;
            self.dirty = true;
        }
    }

    pub(super) fn palette_step(&mut self, down: bool) {
        if let Some(p) = &mut self.nav.palette {
            let (max, unit) = (p.max_scroll(), p.unit());
            p.scroll.by(if down { unit } else { -unit }, max);
            self.dirty = true;
        }
    }

    pub(super) fn layout_palette(&mut self) {
        let (w, h, sc) = (self.size.0, self.size.1, self.scale);
        let s = |v: f32| (v * sc).round();
        let left = self.panels.left;
        let anchor = match self.nav.palette.as_ref().map(|p| &p.pick) {
            Some(Pick::Crumb(i)) => self.crumb_rect(*i),
            Some(Pick::Tabs) => Some(self.tab_list_anchor()),
            _ => None,
        };
        let tabs = self.nav.palette.as_ref().is_some_and(|p| p.pick == Pick::Tabs);
        let Some(p) = &mut self.nav.palette else { return };
        let row_h = p.unit();
        let area = (w - left).max(s(200.0));
        let unit = s(row_h);
        let hint_h = if anchor.is_some() && !tabs { 0.0 } else { s(HINT_H) };
        let fixed = s(8.0) + s(FIELD_H) + s(6.0) + hint_h;
        let (x, y, pw) = match anchor {
            Some(a) => {
                let pw = a.w.max(s(if tabs { TABS_W } else { CRUMB_W })).min(w - 2.0 * s(EDGE));
                let ax = if tabs { a.right() - pw } else { a.x };
                (ax.min(w - s(EDGE) - pw).max(s(EDGE)), (a.bottom() + s(4.0)).min((h - s(EDGE) - fixed - unit).max(s(EDGE))), pw)
            }
            None => {
                let pw = (w / 2.0).round().max(s(if p.go.is_some() { GO_W } else { MIN_W })).min(area - 2.0 * s(EDGE)).max(s(120.0));
                ((left + (area - pw) / 2.0).round().min(w - pw).max(0.0), popup_top(sc, h, fixed + unit), pw)
            }
        };
        let extra = if p.commands.is_some() { s(FIELD_H) + s(4.0) } else { 0.0 };
        let lead = if matches!(&p.go, Some(g) if g.crumb.is_none()) { 3.0 * s(FIELD_H) + s(9.0) } else { 0.0 };
        if let Some(g) = &mut p.go {
            g.buttons = std::array::from_fn(|k| if lead > 0.0 { Rect::new(x + pw - s(8.0) - s(FIELD_H) - lead + k as f32 * s(FIELD_H), y + s(8.0), s(FIELD_H), s(FIELD_H)) } else { Rect::default() });
        }
        p.field_rect = Rect::new(x + s(8.0), y + s(8.0), pw - s(16.0) - s(FIELD_H) - s(4.0) - extra - lead, s(FIELD_H));
        p.describe = if extra > 0.0 { Rect::new(p.field_rect.right() + s(4.0), p.field_rect.y, s(FIELD_H), s(FIELD_H)) } else { Rect::default() };
        p.close = Rect::new(p.field_rect.right() + s(4.0) + extra + lead, p.field_rect.y, s(FIELD_H), s(FIELD_H));
        let top = p.field_rect.bottom() + s(6.0);
        let room = (h - s(EDGE) - hint_h - top).max(unit);
        p.page = ((room / unit).floor() as usize).max(1);
        let max = p.max_scroll();
        p.scroll.clamp(max);
        let n = p.matches.len();
        let shown = n.clamp(1, p.page);
        let list = Rect::new(x, top, pw, shown as f32 * unit);
        let scrolls = n > p.page;
        let bar = if scrolls { s(SCROLL_W) + s(4.0) } else { 0.0 };
        p.rows.clear();
        let at = p.scroll.pos / row_h;
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
        p.hints = Rect::new(x, list.bottom(), pw, hint_h.max(s(6.0)));
        p.rect = Rect::new(x, y, pw, p.hints.bottom() - y);
    }

    pub(super) fn palette_button(&self, h: Hit) -> Option<Rect> {
        let p = self.nav.palette.as_ref()?;
        match h {
            Hit::PaletteClose => Some(p.close),
            Hit::PaletteDescribe => Some(p.describe).filter(|r| r.w > 0.0),
            Hit::PaletteScroll(down) => Some(p.arrows[usize::from(down)]),
            Hit::PaletteKeys(i) => p.rows.get(i.checked_sub(p.first)?).map(|&r| keys_chip(r, self.scale)),
            Hit::GoButton(k) => p.go.as_ref().map(|g| g.buttons[k]),
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
        if p.describe.contains(x, y) {
            return Some(Hit::PaletteDescribe);
        }
        if let Some(k) = p.go.as_ref().and_then(|g| g.buttons.iter().position(|r| r.contains(x, y))) {
            return Some(Hit::GoButton(k));
        }
        if let Some(k) = p.arrows.iter().position(|r| r.contains(x, y)) {
            return Some(Hit::PaletteScroll(k == 1));
        }
        if p.track.contains(x, y) {
            return Some(Hit::PaletteBox);
        }
        let row = p.rows.iter().position(|r| r.contains(x, y) && p.list.contains(x, y));
        Some(match row {
            Some(i) if p.rebinds(p.first + i).is_some() && keys_chip(p.rows[i], self.scale).contains(x, y) => Hit::PaletteKeys(p.first + i),
            Some(i) => Hit::PaletteRow(p.first + i),
            None => Hit::PaletteBox,
        })
    }

    pub(super) fn draw_palette<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme) {
        let (font, small, icons) = (self.ui_font(13.0), self.ui_font(12.0), self.icon_font(11.0));
        let (s3, s4, s6, s12, s16, sc) = (self.s(3.0), self.s(4.0), self.s(6.0), self.s(12.0), self.s(16.0), self.scale);
        let (close_font, arrow_font) = (self.icon_font(9.0), self.icon_font(8.0));
        let (active, hover, now) = (self.active, self.hover, self.last_frame.unwrap_or_else(Instant::now));
        let from = g.dl.instances.len();
        let placeholder = self.nav.palette.as_ref().and_then(|p| p.go.as_ref().map(|_| self.go_placeholder(p)));
        let Some(mut p) = self.nav.palette.take() else { return };
        let mut requests = Vec::new();
        let opened = *p.opened.get_or_insert(now);
        let r = p.rect;
        let mut w = Paint::new(g, t, sc);
        w.frame(r);
        let fr = p.field_rect;
        w.field(fr, true);
        let lead_rect = Rect::new(fr.x, fr.y, fr.h, fr.h);
        let (lead, lead_ink) = match self.go_field_icon(&p) {
            Some(icon) => (if self.system_icon(w.g, icon, lead_rect, s16, 0xFF, &mut requests) { "" } else { super::G_FOLDER }, t.accent),
            None => (G_SEARCH, t.text_dim),
        };
        w.centered(lead_rect, lead, icons, lead_ink);
        let inner = Rect::new(fr.x + fr.h, fr.y, fr.w - fr.h - s12, fr.h);
        p.field.draw(w.g, t, inner, font, active, placeholder.as_deref().or(p.placeholder.as_deref()).unwrap_or(p.pick.placeholder()), sc, Some(&self.preedit));
        w.icon_button(p.close, G_CLOSE, close_font, Button::of(hover == Hit::PaletteClose, false, true), t.text_dim);
        if let Some(c) = &p.commands {
            w.icon_button(p.describe, G_DESCRIBE, icons, Button::of(hover == Hit::PaletteDescribe, c.descriptions, true), t.text_dim);
        }
        if p.go.is_some() {
            self.draw_go_buttons(w.g, t, &p, icons);
        }
        w.g.dl.set_clip(Some(p.list));
        if p.go.is_some() {
            self.draw_go_rows(w.g, t, &p, font, small, self.icon_font(14.0), &mut requests);
        }
        for (k, &row) in p.rows.iter().enumerate() {
            let i = p.first + k;
            let Some(c) = &p.commands else { break };
            let selected = i == p.sel;
            let hot = selected || hover == Hit::PaletteRow(i);
            match &c.lines[p.matches[i]] {
                Line::Header(g) => {
                    let head = Rect::new(row.x, row.y + (row.h - s4 * 7.0).max(0.0) / 2.0, row.w, row.h.min(s4 * 7.0));
                    if hot {
                        w.g.dl.round_rect(head, t.accent_fill, s4);
                    }
                    let ink = if hot { ink_on(t.accent_fill) } else { t.text };
                    w.centered(Rect::new(head.x + s6, head.y, s16 + s6, head.h), g.icon(), icons, ink);
                    let y = baseline(w.g, head, font);
                    w.g.text(head.x + s16 + 2.0 * s12, y, g.title().t(), font, ink, head.w);
                    let chevron = if c.collapsed.contains(g) { G_NEXT } else { G_OPEN };
                    w.centered(Rect::new(head.right() - s12 - s16, head.y, s16, head.h), chevron, icons, ink);
                }
                Line::Item(n, title_marks, desc_marks) => {
                    let live = c.live[*n];
                    if selected {
                        w.g.dl.round_rect(row, t.accent_fill, s4);
                    } else if hover == Hit::PaletteRow(i) {
                        w.g.dl.round_rect(row, t.hover, s4);
                    }
                    let gray = t.text_dim.alpha(0x90);
                    let (ink, dim, mark) = match (selected, live) {
                        (true, _) => {
                            let on = ink_on(t.accent_fill);
                            let quiet = if title_marks.is_empty() && desc_marks.is_empty() { on } else { on.alpha(0xA0) };
                            (quiet, quiet, on)
                        }
                        (false, true) => (t.text, t.text_dim, t.accent),
                        (false, false) => (gray, gray, t.accent.alpha(0x90)),
                    };
                    let caps_w = w.keycaps_w(&c.caps[*n], small);
                    let rebind = ACTIONS[p.ids[*n]].rebindable();
                    let chip = keys_chip(row, sc);
                    let caps_right = if rebind { chip.x - s6 } else { row.right() - s12 };
                    let room = caps_right - caps_w - s12 - row.x - s12;
                    let line = if c.descriptions { Rect::new(row.x, row.y + s3, row.w, row.h / 2.0 - s3) } else { row };
                    let y = baseline(w.g, line, font);
                    w.marked((row.x + s12, y), &p.titles[*n], title_marks, font, (ink, mark), room);
                    if c.descriptions {
                        let below = Rect::new(row.x, row.y + row.h / 2.0, row.w, row.h / 2.0 - s3);
                        let y = baseline(w.g, below, small);
                        w.marked((row.x + s12, y), c.shown[*n], desc_marks, small, (dim, mark), room);
                    }
                    w.keycaps_lit(caps_right - caps_w, row.y + row.h / 2.0, &c.caps[*n], small, live);
                    if rebind && (selected || hover == Hit::PaletteRow(i) || hover == Hit::PaletteKeys(i)) {
                        w.icon_button(chip, G_SHORTCUTS, icons, Button::of(hover == Hit::PaletteKeys(i), false, true), if selected { ink } else { t.text_dim });
                    }
                }
            }
        }
        for (k, &row) in p.rows.iter().enumerate().filter(|_| p.commands.is_none() && p.go.is_none()) {
            let i = p.first + k;
            let selected = i == p.sel;
            if selected {
                w.g.dl.round_rect(row, t.accent_fill, s4);
            } else if hover == Hit::PaletteRow(i) {
                w.g.dl.round_rect(row, t.hover, s4);
            }
            let (ink, dim, mark) = if selected { (ink_on(t.accent_fill), ink_on(t.accent_fill), ink_on(t.accent_fill)) } else { (t.text, t.text_dim, t.accent) };
            if p.pick == Pick::Tabs {
                let id = p.matches[i];
                let closed = self.tab_row_closed(id);
                let fade = |c: crate::render::Color| if closed && !selected { c.alpha(0x90) } else { c };
                let glyph = self.tab_row_glyph(id);
                w.centered(Rect::new(row.x + s6, row.y, s16 * 2.0, row.h), glyph, self.icon_font(18.0), fade(if selected { ink } else { t.accent }));
                let x0 = row.x + s16 * 2.0 + s12;
                let top = Rect::new(row.x, row.y + s3, row.w, row.h / 2.0 - s3);
                let y = baseline(w.g, top, font);
                w.marked((x0, y), &p.titles[id], &p.marks[i], font, (fade(ink), mark), row.right() - x0 - s12);
                let below = Rect::new(row.x, row.y + row.h / 2.0, row.w, row.h / 2.0 - s3);
                let y = baseline(w.g, below, small);
                w.g.text(x0, y, &p.descs[id], small, fade(dim), row.right() - x0 - s12);
                continue;
            }
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
        let crumb = matches!(p.pick, Pick::Crumb(_));
        if !crumb {
            w.separator(hints.x + s12, hints.y, hints.w - 2.0 * s12);
            let keys = [S::WidgetsKeyUp.t(), S::WidgetsKeyDown.t()];
            if p.go.is_some() {
                w.hints_within(hints, &[(&keys, S::WidgetsHintNavigate.t()), (&[S::WidgetsKeyEnter.t()], S::WidgetsHintUse.t()), (&[S::WidgetsKeyTab.t()], S::WidgetsHintExpand.t()), (&[S::WidgetsKeyCtrlEnter.t()], S::WidgetsHintOpenInPanel.t()), (&[S::WidgetsKeyEscape.t()], S::WidgetsHintDismiss.t())], small);
            } else {
                let (enter, f2, escape) = ([S::WidgetsKeyEnter.t()], [S::WidgetsKeyF2.t()], [S::WidgetsKeyEscape.t()]);
                let base: [(&[&str], &str); 3] = [(&keys, S::WidgetsHintNavigate.t()), (&enter, S::WidgetsHintUse.t()), (&escape, S::WidgetsHintDismiss.t())];
                let rebind: [(&[&str], &str); 4] = [base[0], base[1], (&f2, S::WidgetsHintRebind.t()), base[2]];
                let items: &[(&[&str], &str)] = if p.commands.is_some() { &rebind } else { &base };
                w.hints_within(hints, items, small);
            }
        }
        self.nav.palette = Some(p);
        self.request_icons(requests);
        self.pop(g, from, opened, (r.x + r.w / 2.0, r.y));
    }
}

#[cfg(test)]
#[path = "../../test/ui/palette.rs"]
mod tests;
