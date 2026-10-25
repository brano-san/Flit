use super::edit::{EditAction, LineEdit};
use super::{centered, icon_key, App, Effect, Hit, G_BACK, G_CHEVRON, G_FILE, G_FOLDER, G_SORT_DOWN, G_SORT_UP, ICON};
use crate::fmt as format;
use crate::hash::FxHashMap;
use crate::input::{CursorIcon, Event, Key, Mods, MouseButton};
use crate::listing::{attr, Entry, Meta};
use crate::location::Location;
use crate::name;
use crate::open::{OpenTarget, Route};
use crate::path::PathKind;
use crate::provider::ProviderRequest;
use crate::render::Rect;
use crate::text::{FontFamily, FontStyle, Gfx, TextBackend};
use crate::theme::Theme;
use bumpalo::Bump;
use std::collections::HashSet;
use std::fmt::Write as _;
use std::time::{Duration, Instant};

pub const PAGE: u64 = 64;
pub const MAX_HISTORY: usize = 32;
const KEEP_ROWS: usize = 4096;
const KEEP_AROUND: u64 = 2048;
const MAX_RECENT: usize = 8;
const G_SEARCH: &str = "\u{E721}";
const G_HISTORY: &str = "\u{E81C}";
const PLACEHOLDER: &str = "Search: words, *.ext, ext:, size:, dm:, path:, regex:, tag:, or $ and property:value for Windows Search";

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SearchKey {
    #[default]
    Name,
    Folder,
    Size,
    Modified,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct SearchSort {
    pub key: SearchKey,
    pub descending: bool,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub enum SearchScope {
    #[default]
    All,
    Indexed(Location),
    Walk(Location),
}

impl SearchScope {
    pub fn folder(&self) -> Option<&Location> {
        match self {
            SearchScope::All => None,
            SearchScope::Indexed(l) | SearchScope::Walk(l) => Some(l),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum SearchRequest {
    Query { gen: u64, text: String, sort: SearchSort, scope: SearchScope },
    Rows { gen: u64, from: u64, to: u64 },
    Close,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SearchRow {
    pub name: Vec<u8>,
    pub dir: Vec<u8>,
    pub meta: Meta,
}

#[derive(Clone, Debug, PartialEq)]
pub enum IndexState {
    Scanning(f32),
    Ready,
    Error(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct IndexVolume {
    pub id: String,
    pub files: u64,
    pub state: IndexState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SearchBackend {
    #[default]
    Connecting,
    Service,
    Fallback,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchSource {
    Walk,
    Shell,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SearchEvent {
    Done { gen: u64, total: u64 },
    Found { gen: u64, total: u64, folders: u64, done: bool, source: SearchSource },
    Rows { gen: u64, from: u64, rows: Vec<SearchRow> },
    Groups { gen: u64, starts: Vec<u64> },
    Status { backend: SearchBackend, volumes: Vec<IndexVolume>, note: Option<String> },
    Error { message: String },
}

const COLUMNS: [(&str, SearchKey); 4] = [("Name", SearchKey::Name), ("Folder", SearchKey::Folder), ("Size", SearchKey::Size), ("Date modified", SearchKey::Modified)];

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum SHit {
    #[default]
    None,
    Back,
    Field,
    Scope,
    Recent(usize),
    Header(usize),
    Group(usize),
    Row(u64),
    Empty,
    Track,
    Thumb,
}

#[derive(Clone, Debug, PartialEq)]
enum Within {
    Folder(Location),
    Library { name: String, filter: String },
}

impl Within {
    fn name(&self) -> String {
        match self {
            Within::Folder(f) => name::display(f.file_name()).trim_end_matches('\\').to_owned(),
            Within::Library { name, .. } => name.clone(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SLine {
    Header(usize),
    Row(u64),
}

#[derive(Clone, Copy, PartialEq, Debug, Default)]
enum SDrag {
    #[default]
    None,
    Thumb(f32),
    Select,
}

#[derive(Default)]
pub(super) struct SearchView {
    pub(super) active: bool,
    pub(super) edit: LineEdit,
    edit_scroll: f32,
    pending_click: Option<(f32, bool)>,
    gen: u64,
    sort: SearchSort,
    pub(super) group_folder: bool,
    within: Option<Within>,
    home: Option<Within>,
    total: u64,
    done: bool,
    rows: FxHashMap<u64, SearchRow>,
    stale: FxHashMap<u64, SearchRow>,
    groups: Vec<u64>,
    requested: HashSet<u64>,
    focus: Option<u64>,
    scroll: f64,
    hover: SHit,
    pressed: SHit,
    drag: SDrag,
    backend: SearchBackend,
    volumes: Vec<IndexVolume>,
    note: Option<String>,
    error: Option<String>,
    started: Option<Instant>,
    elapsed: Option<Duration>,
    source: Option<SearchSource>,
    walking: bool,
    folders: u64,
    history: Vec<String>,
    recent_open: bool,
    recent_sel: Option<usize>,
    recent_rects: Vec<Rect>,
    chip: Rect,
}

struct SLayout {
    field: Rect,
    header: Rect,
    cols: [Rect; 4],
    list: Rect,
    track: Rect,
    thumb: Rect,
    row_h: f32,
}

pub fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn drive_id(loc: &Location) -> Option<String> {
    let Location::Local(p) = loc else { return None };
    if p.kind() != PathKind::Drive {
        return None;
    }
    let d = p.display();
    d.get(..2).map(|s| s.to_ascii_uppercase())
}

impl SearchView {
    fn grouped(&self) -> bool {
        self.group_folder && self.sort.key == SearchKey::Folder && !self.groups.is_empty()
    }

    fn lines(&self) -> u64 {
        if self.grouped() {
            self.total + self.groups.len() as u64
        } else {
            self.total
        }
    }

    fn line_at(&self, line: u64) -> SLine {
        if !self.grouped() {
            return SLine::Row(line);
        }
        let (mut lo, mut hi) = (0, self.groups.len());
        while lo < hi {
            let mid = (lo + hi) / 2;
            if self.groups[mid] + mid as u64 <= line {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        let k = lo.saturating_sub(1);
        if self.groups[k] + k as u64 == line {
            SLine::Header(k)
        } else {
            SLine::Row(line - k as u64 - 1)
        }
    }

    fn line_of_row(&self, row: u64) -> u64 {
        if !self.grouped() {
            return row;
        }
        row + self.groups.partition_point(|&g| g <= row) as u64
    }

    fn group_len(&self, k: usize) -> u64 {
        self.groups.get(k + 1).copied().unwrap_or(self.total).saturating_sub(self.groups[k])
    }

    fn row(&self, r: u64) -> Option<&SearchRow> {
        self.rows.get(&r).or_else(|| self.stale.get(&r))
    }

    fn recent(&self) -> Vec<usize> {
        if !self.recent_open || !self.edit.text.is_empty() {
            return Vec::new();
        }
        (0..self.history.len().min(MAX_RECENT)).collect()
    }
}

impl App {
    pub fn open_search(&mut self) {
        let lib = self.library_within();
        self.open_scoped(lib);
    }

    pub fn open_folder_search(&mut self) {
        let loc = self.current_location().clone();
        let usable = matches!(loc.path().kind(), PathKind::Drive | PathKind::Unc | PathKind::Verbatim | PathKind::Unix);
        let within = self.library_within().or_else(|| usable.then_some(Within::Folder(loc)));
        if within.is_some() {
            self.search.home = within.clone();
        }
        self.open_scoped(within);
    }

    fn library_within(&self) -> Option<Within> {
        let lib = self.current_library().and_then(|p| self.nav.sidebar.library(&p).cloned())?;
        let filter = crate::library::search_scope(&lib.folders)?;
        Some(Within::Library { name: lib.name, filter })
    }

    fn open_scoped(&mut self, within: Option<Within>) {
        if within.is_some() {
            self.search.home = within.clone();
        }
        let s = &mut self.search;
        let requery = s.gen == 0 || s.within != within;
        s.within = within;
        if !s.active {
            s.active = true;
            s.drag = SDrag::None;
            s.recent_open = true;
            s.recent_sel = None;
            self.editing = false;
            self.marquee = None;
        }
        self.effects.push(Effect::Title(self.search_title()));
        if requery {
            self.search_query(Instant::now());
        }
        self.search.edit.select_all();
        self.dirty = true;
    }

    fn search_title(&self) -> String {
        match &self.search.within {
            Some(w) => format!("Search in {} — Flit", w.name()),
            None => "Search — Flit".into(),
        }
    }

    pub fn close_search(&mut self) {
        if self.search.active {
            self.remember_query();
            self.search.active = false;
            self.search.drag = SDrag::None;
            self.effects.push(Effect::Title(self.window_title()));
            self.set_cursor(CursorIcon::Default);
            self.dirty = true;
        }
    }

    pub fn search_active(&self) -> bool {
        self.search.active
    }

    pub fn search_text(&self) -> &str {
        &self.search.edit.text
    }

    pub fn search_folder(&self) -> Option<&Location> {
        match &self.search.within {
            Some(Within::Folder(f)) => Some(f),
            _ => None,
        }
    }

    pub fn search_history(&self) -> &[String] {
        &self.search.history
    }

    pub fn set_search_history(&mut self, mut history: Vec<String>) {
        history.retain(|q| !q.trim().is_empty() && q.len() <= 4096);
        history.truncate(MAX_HISTORY);
        self.search.history = history;
    }

    pub(super) fn clear_search_history(&mut self) {
        self.search.history.clear();
        self.search.recent_sel = None;
        self.dirty = true;
    }

    fn remember_query(&mut self) {
        let q = self.search.edit.text.trim().to_owned();
        if q.is_empty() {
            return;
        }
        let h = &mut self.search.history;
        h.retain(|old| !old.eq_ignore_ascii_case(&q));
        h.insert(0, q);
        h.truncate(MAX_HISTORY);
    }

    pub fn on_search_event(&mut self, ev: SearchEvent, now: Instant) {
        let s = &mut self.search;
        match ev {
            SearchEvent::Done { gen, total } if gen == s.gen => {
                s.total = total;
                s.done = true;
                s.walking = false;
                s.rows.clear();
                s.stale.clear();
                s.groups.clear();
                s.requested.clear();
                s.focus = s.focus.filter(|&f| f < total);
                if let Some(t) = s.started.take() {
                    s.elapsed = Some(now.saturating_duration_since(t));
                }
            }
            SearchEvent::Found { gen, total, folders, done, source } if gen == s.gen => {
                s.total = total;
                s.done = true;
                s.walking = !done;
                s.folders = folders;
                s.source = Some(source);
                s.stale = std::mem::take(&mut s.rows);
                s.requested.clear();
                s.focus = s.focus.filter(|&f| f < total);
                if done {
                    s.elapsed = s.started.take().map(|t| now.saturating_duration_since(t));
                }
            }
            SearchEvent::Rows { gen, from, rows } if gen == s.gen && s.done => {
                let n = rows.len() as u64;
                for (k, r) in rows.into_iter().enumerate() {
                    s.rows.insert(from + k as u64, r);
                }
                if n > 0 {
                    for p in from / PAGE..=(from + n - 1) / PAGE {
                        s.requested.remove(&p);
                    }
                }
            }
            SearchEvent::Groups { gen, starts } if gen == s.gen => {
                let ordered = starts.first() == Some(&0) && starts.windows(2).all(|w| w[0] < w[1]) && starts.last().is_some_and(|&l| l < s.total);
                s.groups = if ordered { starts } else { Vec::new() };
            }
            SearchEvent::Status { backend, volumes, note } => {
                s.backend = backend;
                s.volumes = volumes;
                s.note = note;
            }
            SearchEvent::Error { message } => s.error = Some(message),
            _ => return,
        }
        self.dirty |= self.search.active;
    }

    pub(super) fn search_for(&mut self, text: &str) {
        self.search.edit.set(text);
        let fresh = self.search.gen == 0 || self.search.within != self.library_within();
        self.open_search();
        if !fresh {
            self.search_query(Instant::now());
        }
    }

    pub(super) fn search_paste(&mut self, text: &str) {
        self.search.edit.insert(text.lines().next().unwrap_or(""));
        self.search_query(Instant::now());
    }

    pub(super) fn toggle_search_groups(&mut self) {
        let s = &mut self.search;
        s.group_folder = !s.group_folder;
        if s.group_folder {
            s.sort = SearchSort { key: SearchKey::Folder, descending: false };
        }
        if s.gen > 0 {
            self.search_query(Instant::now());
        }
    }

    fn search_scope(&self) -> SearchScope {
        let Some(Within::Folder(folder)) = self.search.within.clone() else { return SearchScope::All };
        let archived = crate::archive::split(folder.path()).is_some();
        let ready = !archived && drive_id(&folder).is_some_and(|d| self.search.volumes.iter().any(|v| v.id.eq_ignore_ascii_case(&d) && v.state == IndexState::Ready));
        if ready {
            SearchScope::Indexed(folder)
        } else {
            SearchScope::Walk(folder)
        }
    }

    fn search_query(&mut self, now: Instant) {
        let scope = self.search_scope();
        let s = &mut self.search;
        s.gen += 1;
        s.total = 0;
        s.done = false;
        s.walking = false;
        s.folders = 0;
        s.source = None;
        s.rows.clear();
        s.stale.clear();
        s.groups.clear();
        s.requested.clear();
        s.focus = None;
        s.scroll = 0.0;
        s.error = None;
        s.started = Some(now);
        s.elapsed = None;
        if s.edit.text.is_empty() {
            s.recent_open = true;
        }
        s.recent_sel = None;
        let text = match &s.within {
            Some(Within::Library { filter, .. }) => format!("{filter} {}", s.edit.text),
            _ => s.edit.text.clone(),
        };
        self.effects.push(Effect::Search(SearchRequest::Query { gen: s.gen, text, sort: s.sort, scope }));
        self.dirty = true;
    }

    fn toggle_scope(&mut self) {
        let within = match self.search.within {
            Some(_) => None,
            None => self.search.home.clone(),
        };
        if within.is_none() && self.search.within.is_none() {
            return;
        }
        self.search.within = within;
        self.effects.push(Effect::Title(self.search_title()));
        self.search_query(Instant::now());
    }

    fn search_layout(&self) -> SLayout {
        let l = &self.layout;
        let field = Rect::new(l.nav[1].x, l.address.y, (l.address.right() - l.nav[1].x).max(self.s(40.0)), l.address.h);
        let list = l.list;
        let size_w = self.s(90.0).min(list.w / 4.0);
        let date_w = self.s(140.0).min(list.w / 4.0);
        let name_w = (list.w * 0.32).max(self.s(140.0)).min(list.w - size_w - date_w);
        let folder_w = (list.w - name_w - size_w - date_w).max(0.0);
        let mut x = list.x;
        let mut cols = [Rect::default(); 4];
        for (c, w) in cols.iter_mut().zip([name_w, folder_w, size_w, date_w]) {
            *c = Rect::new(x, l.header.y, w, l.header.h);
            x += w;
        }
        let rh = l.row_h.max(1.0);
        let content = self.search.lines() as f64 * rh as f64;
        let lh = list.h as f64;
        let thumb = if content > lh {
            let th = (l.track.h as f64 * lh / content).max(self.s(24.0) as f64).min(l.track.h as f64) as f32;
            let frac = (self.search.scroll / (content - lh)).clamp(0.0, 1.0) as f32;
            Rect::new(l.track.x + self.s(3.0), l.track.y + (l.track.h - th) * frac, l.track.w - self.s(6.0), th)
        } else {
            Rect::default()
        };
        SLayout { field, header: l.header, cols, list, track: l.track, thumb, row_h: rh }
    }

    fn search_max_scroll(&self) -> f64 {
        let l = &self.layout;
        (self.search.lines() as f64 * l.row_h.max(1.0) as f64 - l.list.h as f64).max(0.0)
    }

    fn search_range(&self) -> (u64, u64) {
        let l = &self.layout;
        let rh = l.row_h.max(1.0) as f64;
        let s = &self.search;
        let first = (s.scroll / rh).floor() as u64;
        let last = ((s.scroll + l.list.h as f64) / rh).ceil() as u64;
        (first.min(s.lines()), last.min(s.lines()))
    }

    fn search_row_range(&self) -> (u64, u64) {
        let (first, last) = self.search_range();
        let s = &self.search;
        let row_of = |line: u64| match s.line_at(line) {
            SLine::Row(r) => r,
            SLine::Header(k) => s.groups[k],
        };
        if first >= last {
            return (row_of(first.min(s.lines().saturating_sub(1))).min(s.total), row_of(first.min(s.lines().saturating_sub(1))).min(s.total));
        }
        (row_of(first), (row_of(last - 1) + 1).min(s.total))
    }

    fn search_hit(&self, x: f32, y: f32) -> SHit {
        let sl = self.search_layout();
        if let Some(k) = self.search.recent_rects.iter().position(|r| r.contains(x, y)) {
            return SHit::Recent(k);
        }
        if self.layout.nav[0].contains(x, y) {
            return SHit::Back;
        }
        if self.search.chip.contains(x, y) {
            return SHit::Scope;
        }
        if sl.field.contains(x, y) {
            return SHit::Field;
        }
        if sl.header.contains(x, y) {
            return sl.cols.iter().position(|c| c.contains(x, y)).map_or(SHit::None, SHit::Header);
        }
        if sl.track.contains(x, y) {
            let t = sl.thumb;
            return if t.h > 0.0 && y >= t.y && y < t.bottom() { SHit::Thumb } else { SHit::Track };
        }
        if sl.list.contains(x, y) {
            let line = ((self.search.scroll + (y - sl.list.y) as f64) / sl.row_h as f64).floor() as u64;
            if line >= self.search.lines() {
                return SHit::Empty;
            }
            return match self.search.line_at(line) {
                SLine::Row(r) => SHit::Row(r),
                SLine::Header(k) => SHit::Group(k),
            };
        }
        SHit::None
    }

    pub(super) fn search_event(&mut self, ev: &Event, now: Instant) -> bool {
        match ev {
            Event::Key { key, mods, pressed } => {
                self.mods = *mods;
                !*pressed || self.search_key(*key, *mods, now)
            }
            Event::Char(c) => {
                if !c.is_control() {
                    let mut b = [0; 4];
                    self.search.edit.insert(c.encode_utf8(&mut b));
                    self.search_query(now);
                }
                true
            }
            Event::MouseDown { x, y, button, clicks } => self.search_mouse_down(*x, *y, *button, *clicks, now),
            Event::MouseUp { x, y, button } => self.search_mouse_up(*x, *y, *button),
            Event::MouseMove { x, y } => self.search_mouse_move(*x, *y),
            Event::Wheel { dy, .. } => {
                let rh = self.layout.row_h as f64;
                let max = self.search_max_scroll();
                self.search.scroll = (self.search.scroll - *dy as f64 * 3.0 * rh).clamp(0.0, max);
                self.dirty = true;
                true
            }
            Event::DroppedFiles(_) => {
                self.close_search();
                false
            }
            _ => false,
        }
    }

    fn recent_key(&mut self, key: Key, m: Mods, now: Instant) -> bool {
        let n = self.search.recent().len();
        if n == 0 {
            return false;
        }
        let s = &mut self.search;
        match key {
            Key::Down => s.recent_sel = Some(s.recent_sel.map_or(0, |k| (k + 1) % n)),
            Key::Up => s.recent_sel = Some(s.recent_sel.map_or(n - 1, |k| (k + n - 1) % n)),
            Key::Escape => {
                s.recent_open = false;
                s.recent_sel = None;
            }
            Key::Enter if s.recent_sel.is_some() => {
                let k = s.recent_sel.unwrap_or(0);
                self.apply_recent(k, now);
            }
            Key::Delete if m.shift && s.recent_sel.is_some() => {
                let k = s.recent_sel.unwrap_or(0);
                s.history.remove(k);
                let left = s.recent().len();
                s.recent_sel = (left > 0).then(|| k.min(left - 1));
            }
            _ => return false,
        }
        true
    }

    fn apply_recent(&mut self, k: usize, now: Instant) {
        let Some(q) = self.search.history.get(k).cloned() else { return };
        let s = &mut self.search;
        s.edit.set(&q);
        s.edit.select_all();
        s.recent_open = false;
        s.recent_sel = None;
        self.search_query(now);
        self.search.recent_open = false;
    }

    fn search_key(&mut self, key: Key, m: Mods, now: Instant) -> bool {
        self.dirty = true;
        if self.recent_key(key, m, now) {
            return true;
        }
        let page = ((self.layout.list.h / self.layout.row_h.max(1.0)).floor() as i64).max(1);
        let mut moved = None;
        match (key, m.ctrl, m.alt) {
            (Key::Escape, ..) => {
                self.close_search();
                return true;
            }
            (Key::Enter, false, false) => {
                self.open_search_row(self.search.focus.unwrap_or(0), now);
                return true;
            }
            (Key::Enter, true, false) => {
                self.reveal_search_row(self.search.focus.unwrap_or(0), now);
                return true;
            }
            (Key::Char('E'), true, false) if !m.shift => {
                if self.search.within.is_some() {
                    self.open_scoped(None);
                }
                self.search.edit.select_all();
                return true;
            }
            (Key::F(3), false, false) => {
                if self.search.within.is_none() && self.search.home.is_some() {
                    self.toggle_scope();
                }
                self.search.edit.select_all();
                return true;
            }
            (Key::Char('C'), true, false) if m.shift => {
                if let Some(p) = self.search.focus.and_then(|f| self.search_row_location(f)) {
                    self.effects.push(Effect::SetClipboard(p.display().into_owned()));
                }
                return true;
            }
            (Key::Char('T' | 'W' | '1'..='9') | Key::Tab | Key::F(4), true, false) => {
                self.close_search();
                return false;
            }
            (Key::Up, _, false) => moved = Some(-1),
            (Key::Down, _, false) => moved = Some(1),
            (Key::PageUp, _, false) => moved = Some(-page),
            (Key::PageDown, _, false) => moved = Some(page),
            (Key::Home, true, false) => moved = Some(i64::MIN / 2),
            (Key::End, true, false) => moved = Some(i64::MAX / 2),
            _ => {}
        }
        if let Some(d) = moved {
            self.search_move(d);
            return true;
        }
        match self.search.edit.key(key, m) {
            EditAction::Changed => self.search_query(now),
            EditAction::Copy(s) => self.effects.push(Effect::SetClipboard(s)),
            EditAction::Paste => self.effects.push(Effect::Paste),
            EditAction::Submit | EditAction::Cancel | EditAction::None => {}
        }
        true
    }

    fn search_move(&mut self, delta: i64) {
        let total = self.search.total;
        if total == 0 {
            return;
        }
        let base = self.search.focus.map_or(if delta < 0 { total as i64 } else { -1 }, |f| f as i64);
        let row = base.saturating_add(delta).clamp(0, total as i64 - 1) as u64;
        self.search.focus = Some(row);
        self.search_reveal(row);
    }

    fn search_reveal(&mut self, row: u64) {
        let rh = self.layout.row_h as f64;
        let lh = self.layout.list.h as f64;
        let line = self.search.line_of_row(row);
        let (top, bottom) = (line as f64 * rh, (line + 1) as f64 * rh);
        let max = self.search_max_scroll();
        let s = &mut self.search;
        if top < s.scroll {
            s.scroll = top;
        } else if bottom > s.scroll + lh {
            s.scroll = bottom - lh;
        }
        s.scroll = s.scroll.clamp(0.0, max);
    }

    fn search_row_location(&self, row: u64) -> Option<Location> {
        let r = self.search.row(row)?;
        if r.dir.is_empty() {
            return None;
        }
        Some(Location::parse_exact(&r.dir)?.join(&r.name))
    }

    fn open_search_row(&mut self, row: u64, now: Instant) {
        let Some(loc) = self.search_row_location(row) else { return };
        self.remember_query();
        if self.search.row(row).is_some_and(|r| r.meta.attrs & attr::DIR != 0) {
            self.close_search();
            self.dispatch(vec![Route { folder: loc, select: None }], OpenTarget::CurrentTab, now);
        } else {
            let tab = self.tab().id;
            self.effects.push(Effect::Provider { provider: loc.provider(), req: ProviderRequest::Open { tab, path: loc.path().clone() } });
        }
    }

    pub(super) fn reveal_search_focus(&mut self, now: Instant) {
        if let Some(row) = self.search.focus {
            self.reveal_search_row(row, now);
        }
    }

    fn reveal_search_row(&mut self, row: u64, now: Instant) {
        let Some(loc) = self.search_row_location(row) else { return };
        let Some(folder) = loc.parent() else { return };
        let select = Some(loc.file_name().to_vec());
        self.close_search();
        self.dispatch(vec![Route { folder, select }], OpenTarget::CurrentTab, now);
    }

    fn open_group(&mut self, k: usize, now: Instant) {
        let Some(dir) = self.search.groups.get(k).and_then(|&r| self.search.row(r)).map(|r| r.dir.clone()) else { return };
        let Some(folder) = Location::parse_exact(&dir) else { return };
        self.close_search();
        self.dispatch(vec![Route { folder, select: None }], OpenTarget::CurrentTab, now);
    }

    fn search_mouse_down(&mut self, x: f32, y: f32, button: MouseButton, clicks: u8, now: Instant) -> bool {
        if self.sidebar_hit(x, y).is_some() {
            self.close_search();
            return false;
        }
        if self.layout.title.contains(x, y) {
            if matches!(self.hit(x, y), Hit::Tab(_) | Hit::TabClose(_) | Hit::NewTab) {
                self.close_search();
            }
            return false;
        }
        if matches!(button, MouseButton::Back) {
            self.close_search();
            return true;
        }
        self.dirty = true;
        let h = self.search_hit(x, y);
        self.search.pressed = h;
        if !matches!(h, SHit::Recent(_) | SHit::Field) {
            self.search.recent_open = false;
        }
        match h {
            SHit::Field => {
                if clicks >= 2 {
                    self.search.edit.select_all();
                } else {
                    self.search.pending_click = Some((x, self.mods.shift));
                    self.search.drag = SDrag::Select;
                }
                self.search.recent_open = true;
            }
            SHit::Row(row) => {
                self.search.focus = Some(row);
                if clicks >= 2 && button == MouseButton::Left {
                    if self.mods.ctrl {
                        self.reveal_search_row(row, now);
                    } else {
                        self.open_search_row(row, now);
                    }
                }
            }
            SHit::Group(k) if clicks >= 2 && button == MouseButton::Left => self.open_group(k, now),
            SHit::Thumb => self.search.drag = SDrag::Thumb(y - self.search_layout().thumb.y),
            SHit::Track => {
                let dir = if y < self.search_layout().thumb.y { -1.0 } else { 1.0 };
                let max = self.search_max_scroll();
                self.search.scroll = (self.search.scroll + dir * self.layout.list.h as f64).clamp(0.0, max);
            }
            _ => {}
        }
        true
    }

    fn search_mouse_up(&mut self, x: f32, y: f32, button: MouseButton) -> bool {
        if self.layout.title.contains(x, y) && self.search.drag == SDrag::None {
            return false;
        }
        let was = std::mem::take(&mut self.search.drag);
        let pressed = std::mem::take(&mut self.search.pressed);
        self.dirty = true;
        let h = self.search_hit(x, y);
        if button == MouseButton::Right && matches!(h, SHit::Header(_) | SHit::Row(_) | SHit::Group(_) | SHit::Empty) {
            let menu = self.build_search_menu(matches!(h, SHit::Row(_)));
            self.flit_menu(x, y, menu);
            return true;
        }
        if button != MouseButton::Left || matches!(was, SDrag::Thumb(_)) {
            return true;
        }
        if h != pressed {
            return true;
        }
        match h {
            SHit::Back => self.close_search(),
            SHit::Scope => self.toggle_scope(),
            SHit::Recent(k) => self.apply_recent(k, Instant::now()),
            SHit::Header(c) => {
                let key = COLUMNS[c].1;
                self.search.group_folder &= key == SearchKey::Folder;
                let s = &mut self.search.sort;
                *s = SearchSort { key, descending: s.key == key && !s.descending };
                self.search_query(Instant::now());
            }
            _ => {}
        }
        true
    }

    fn search_mouse_move(&mut self, x: f32, y: f32) -> bool {
        match self.search.drag {
            SDrag::Thumb(grab) => {
                let sl = self.search_layout();
                let range = (sl.track.h - sl.thumb.h).max(1.0);
                let frac = ((y - grab - sl.track.y) / range).clamp(0.0, 1.0) as f64;
                self.search.scroll = frac * self.search_max_scroll();
                self.dirty = true;
                return true;
            }
            SDrag::Select => {
                self.search.pending_click = Some((x, true));
                self.dirty = true;
                return true;
            }
            SDrag::None => {}
        }
        if self.layout.title.contains(x, y) {
            if self.search.hover != SHit::None {
                self.search.hover = SHit::None;
                self.dirty = true;
            }
            return false;
        }
        let h = self.search_hit(x, y);
        self.set_cursor(if h == SHit::Field { CursorIcon::Text } else { CursorIcon::Default });
        if h != self.search.hover {
            self.search.hover = h;
            self.dirty = true;
        }
        true
    }

    fn request_visible_pages(&mut self) {
        let s = &self.search;
        if !s.done || s.total == 0 {
            return;
        }
        let (first, last) = self.search_row_range();
        let total = s.total;
        let p0 = first.saturating_sub(PAGE) / PAGE;
        let p1 = (last + PAGE).min(total).saturating_sub(1) / PAGE;
        let loaded = |p: u64| s.rows.contains_key(&(p * PAGE)) && s.rows.contains_key(&(((p + 1) * PAGE).min(total) - 1));
        let missing: Vec<u64> = (p0..=p1).filter(|&p| !loaded(p) && !s.requested.contains(&p)).collect();
        let gen = s.gen;
        let mut runs: Vec<(u64, u64)> = Vec::new();
        for p in missing {
            match runs.last_mut() {
                Some((_, end)) if *end + 1 == p => *end = p,
                _ => runs.push((p, p)),
            }
        }
        for (a, b) in runs {
            self.search.requested.extend(a..=b);
            self.effects.push(Effect::Search(SearchRequest::Rows { gen, from: a * PAGE, to: ((b + 1) * PAGE).min(total) }));
        }
        if self.search.rows.len() > KEEP_ROWS {
            let (lo, hi) = (first.saturating_sub(KEEP_AROUND), last + KEEP_AROUND);
            self.search.rows.retain(|&r, _| r >= lo && r < hi);
        }
    }

    pub(super) fn draw_search<B: TextBackend>(&mut self, g: &mut Gfx<B>, arena: &Bump, t: &Theme) {
        self.search.scroll = self.search.scroll.clamp(0.0, self.search_max_scroll());
        self.request_visible_pages();
        let sl = self.search_layout();
        let font = self.ui_font(13.0);
        let m = g.metrics(font);
        let rh = sl.row_h;
        let text_dy = ((rh + m.ascent - m.descent) / 2.0).round();
        let pad = self.s(10.0);
        let icon_px = self.s(ICON);
        let focused = self.active;
        let (first, last) = self.search_range();
        g.dl.set_clip(Some(sl.list));
        let s = &self.search;
        let headers = s.grouped();
        if s.done && s.total == 0 && !s.walking {
            let msg = if s.error.is_some() { "The query is not valid." } else { "No files match." };
            let w = g.measure(msg, font);
            g.text(sl.list.x + (sl.list.w - w) / 2.0, sl.list.y + self.s(40.0), msg, font, t.text_dim, f32::INFINITY);
        }
        for line in first..last {
            let y = (sl.list.y as f64 + line as f64 * rh as f64 - s.scroll).round() as f32;
            let row = match s.line_at(line) {
                SLine::Row(r) => r,
                SLine::Header(k) => {
                    self.draw_search_group(g, arena, t, k, Rect::new(sl.list.x, y, sl.list.w, rh));
                    continue;
                }
            };
            let rr = Rect::new(sl.list.x + self.s(4.0), y + 1.0, sl.list.w - self.s(8.0), rh - 2.0);
            if s.focus == Some(row) {
                g.dl.round_rect(rr, if focused { t.selection } else { t.selection_inactive }, self.s(4.0));
            } else if s.hover == SHit::Row(row) && s.drag == SDrag::None {
                g.dl.round_rect(rr, t.hover, self.s(4.0));
            }
            let Some(r) = s.row(row) else { continue };
            let is_dir = r.meta.attrs & attr::DIR != 0;
            let ext_at = if is_dir { r.name.len() } else { r.name.len() - name::ext_len(&r.name) };
            let ext = &r.name[ext_at..];
            let entry = Entry { size: 0, modified: 0, created: 0, name_off: 0, attrs: r.meta.attrs, name_len: 0, ext_len: 0, tags: 0 };
            let (key, _) = icon_key(&r.name, ext, &entry, icon_px as u32, &r.dir, self.icon_epoch);
            let indent = if headers { self.s(16.0) } else { 0.0 };
            let ir = Rect::new(sl.cols[0].x + pad + indent, y + ((rh - icon_px) / 2.0).round(), icon_px, icon_px);
            if !g.image(key, ir) {
                let (glyph, color) = if is_dir { (G_FOLDER, t.accent.alpha(0xB0)) } else { (G_FILE, t.text_dim.alpha(0x90)) };
                centered(g, ir, glyph, FontStyle::new(FontFamily::Icons, icon_px * 0.85, false), color);
            }
            let ty = y + text_dy;
            let name_x = ir.right() + self.s(8.0);
            let color = if r.meta.attrs & attr::HIDDEN != 0 { t.text_dim } else { t.text };
            g.text(name_x, ty, &name::display(&r.name), font, color, sl.cols[0].right() - name_x - pad);
            let run_start = row == 0 || s.row(row - 1).is_none_or(|p| p.dir != r.dir);
            if !headers && (!s.group_folder || run_start) {
                let folder_color = if s.group_folder { t.text } else { t.text_dim };
                g.text(sl.cols[1].x + pad, ty, &name::display(&r.dir), font, folder_color, sl.cols[1].w - 2.0 * pad);
            }
            if !headers && s.group_folder && run_start && line > first {
                g.dl.rect(Rect::new(sl.list.x + self.s(4.0), y, sl.list.w - self.s(8.0), 1.0), t.separator);
            }
            if !is_dir {
                let mut sz = bumpalo::collections::String::new_in(arena);
                format::size_in(&mut sz, r.meta.size, self.style.decimal);
                let w = g.measure(&sz, font);
                g.text((sl.cols[2].right() - pad - w).max(sl.cols[2].x + pad), ty, &sz, font, t.text_dim, sl.cols[2].w - 2.0 * pad);
            }
            let mut d = bumpalo::collections::String::new_in(arena);
            format::datetime_in(&mut d, r.meta.modified, self.utc_offset_min, &self.style);
            g.text(sl.cols[3].x + pad, ty, &d, font, t.text_dim, sl.cols[3].w - 2.0 * pad);
        }
        g.dl.set_clip(None);
        let s = &self.search;
        if sl.thumb.h > 0.0 {
            let hot = matches!(s.hover, SHit::Thumb | SHit::Track) || matches!(s.drag, SDrag::Thumb(_));
            g.dl.round_rect(sl.thumb, if hot { t.scrollbar_hover } else { t.scrollbar }, sl.thumb.w / 2.0);
        }
        g.dl.rect(sl.header, t.header);
        g.dl.rect(Rect::new(0.0, sl.header.bottom() - 1.0, sl.header.w, 1.0), t.separator);
        let hfont = self.ui_font(12.0);
        let hm = g.metrics(hfont);
        let baseline = (sl.header.y + (sl.header.h + hm.ascent - hm.descent) / 2.0).round();
        for (i, (title, key)) in COLUMNS.iter().enumerate() {
            let r = sl.cols[i];
            if s.hover == SHit::Header(i) {
                g.dl.rect(r, t.button_hover);
            }
            if i > 0 {
                g.dl.rect(Rect::new(r.x, r.y + self.s(6.0), 1.0, r.h - self.s(12.0)), t.separator);
            }
            g.dl.set_clip(Some(r));
            let w = g.text(r.x + pad, baseline, title, hfont, t.text_dim, r.w - 2.0 * pad);
            if s.sort.key == *key {
                let glyph = if s.sort.descending { G_SORT_DOWN } else { G_SORT_UP };
                g.text(r.x + pad + w + self.s(6.0), baseline, glyph, self.icon_font(8.0), t.text_dim, f32::INFINITY);
            }
            g.dl.set_clip(None);
        }
    }

    fn draw_search_group<B: TextBackend>(&self, g: &mut Gfx<B>, arena: &Bump, t: &Theme, k: usize, r: Rect) {
        let s = &self.search;
        let font = FontStyle::new(FontFamily::Ui, 12.5 * self.scale, true);
        let m = g.metrics(font);
        let baseline = (r.y + (r.h + m.ascent - m.descent) / 2.0).round();
        if s.hover == SHit::Group(k) {
            g.dl.round_rect(r.inset(self.s(4.0), 1.0), t.hover, self.s(4.0));
        }
        let icon = Rect::new(r.x + self.s(10.0), r.y, self.s(16.0), r.h);
        centered(g, icon, G_FOLDER, self.icon_font(11.0), t.accent.alpha(0xC0));
        let dir = s.row(s.groups[k]).map(|row| name::display(&row.dir).into_owned()).unwrap_or_else(|| "…".into());
        let x = icon.right() + self.s(8.0);
        let mut count = bumpalo::collections::String::new_in(arena);
        let n = s.group_len(k);
        let _ = write!(count, "{} {}", thousands(n), if n == 1 { "item" } else { "items" });
        let small = self.ui_font(12.0);
        let cw = g.measure(&count, small);
        let w = g.text(x, baseline, &dir, font, t.text, (r.right() - x - cw - self.s(32.0)).max(0.0));
        g.text(x + w + self.s(12.0), baseline, &count, small, t.text_dim, f32::INFINITY);
        g.dl.rect(Rect::new(x + w + cw + self.s(24.0), (r.y + r.h / 2.0).round(), (r.right() - x - w - cw - self.s(32.0)).max(0.0), 1.0), t.separator);
    }

    pub(super) fn draw_search_bar<B: TextBackend>(&mut self, g: &mut Gfx<B>, arena: &Bump, t: &Theme) {
        let l = &self.layout;
        let (toolbar, back, status) = (l.toolbar, l.nav[0], l.status);
        g.dl.rect(toolbar, t.bg);
        if self.search.hover == SHit::Back {
            g.dl.round_rect(back.inset(self.s(2.0), 0.0), t.button_hover, self.s(4.0));
        }
        centered(g, back, G_BACK, self.icon_font(12.0), t.text);
        self.draw_search_field(g, t);

        g.dl.rect(status, t.chrome);
        let font = self.ui_font(12.0);
        let m = g.metrics(font);
        let baseline = (status.y + (status.h + m.ascent - m.descent) / 2.0).round();
        let pad = self.s(12.0);
        let s = &self.search;
        let mut left = bumpalo::collections::String::new_in(arena);
        let mut color = t.text_dim;
        if let Some(e) = &s.error {
            left.push_str(e);
            color = t.error;
        } else if s.walking {
            let _ = write!(left, "Searching… {} {}", thousands(s.total), if s.total == 1 { "result" } else { "results" });
            if s.folders > 0 {
                let _ = write!(left, " in {} folders", thousands(s.folders));
            }
        } else if s.done {
            let _ = write!(left, "{} {}", thousands(s.total), if s.total == 1 { "result" } else { "results" });
            if let Some(e) = s.elapsed {
                let _ = write!(left, " in {:.1} ms", e.as_secs_f64() * 1e3);
            }
            if s.folders > 0 {
                let _ = write!(left, " · {} folders", thousands(s.folders));
            }
        } else {
            left.push_str("Searching…");
        }
        g.text(status.x + pad, baseline, &left, font, color, status.w / 2.0 - pad);
        let mut right = bumpalo::collections::String::new_in(arena);
        let tagged = crate::tags::has_tag_term(&s.edit.text);
        right.push_str(match (s.source, s.backend) {
            (Some(SearchSource::Walk), _) => "Live search of the folder and its subfolders",
            (Some(SearchSource::Shell), _) => "Windows Search",
            _ if tagged => "Tagged files",
            (None, SearchBackend::Connecting) => "Connecting to the index",
            (None, SearchBackend::Service) => "Index service",
            (None, SearchBackend::Fallback) => "Local index (flit-indexd unavailable)",
        });
        for v in s.volumes.iter().filter(|_| !tagged && s.source.is_none()) {
            let label = if v.id.is_empty() { "index" } else { &v.id };
            let _ = match &v.state {
                IndexState::Ready => write!(right, " · {label} {} files", thousands(v.files)),
                IndexState::Scanning(p) => write!(right, " · {label} indexing {:.0}% ({} files)", p * 100.0, thousands(v.files)),
                IndexState::Error(e) => write!(right, " · {label} error: {e}"),
            };
        }
        let rcolor = if s.backend == SearchBackend::Fallback && !tagged && s.source.is_none() { t.error } else { t.text_dim };
        let rw = g.measure(&right, font).min(status.w / 2.0 - pad);
        g.text(status.right() - pad - rw, baseline, &right, font, rcolor, status.w / 2.0 - pad);
        self.draw_recent(g, t);
    }

    fn draw_search_field<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme) {
        let r = self.search_layout().field;
        let radius = self.s(4.0);
        g.dl.round_rect(r, t.input_bg, radius);
        g.dl.outline(r, t.accent, radius, self.s(1.0));
        let icon = Rect::new(r.x + self.s(4.0), r.y, self.s(24.0), r.h);
        centered(g, icon, G_SEARCH, self.icon_font(11.0), t.text_dim);
        let font = self.ui_font(13.0);
        let small = self.ui_font(12.0);
        let m = g.metrics(font);
        let baseline = (r.y + (r.h + m.ascent - m.descent) / 2.0).round();
        let label = match &self.search.within {
            Some(w) => format!("In {}", w.name()),
            None if self.search.home.is_some() => "All drives".to_owned(),
            None => String::new(),
        };
        let mut x = icon.right() + self.s(4.0);
        self.search.chip = Rect::default();
        if !label.is_empty() {
            let cw = (g.measure(&label, small) + self.s(30.0)).min(r.w / 3.0);
            let chip = Rect::new(x, r.y + self.s(4.0), cw, r.h - self.s(8.0));
            let hot = self.search.hover == SHit::Scope;
            g.dl.round_rect(chip, if hot { t.button_hover } else { t.selection_inactive }, self.s(3.0));
            let w = g.text(chip.x + self.s(8.0), baseline, &label, small, t.text, chip.w - self.s(24.0));
            g.text(chip.x + self.s(12.0) + w, baseline - self.s(1.0), G_CHEVRON, self.icon_font(7.0), t.text_dim, f32::INFINITY);
            self.search.chip = chip;
            x = chip.right() + self.s(8.0);
        }
        let inner = Rect::new(x, r.y, r.right() - x - self.s(12.0), r.h);
        g.dl.set_clip(Some(inner));
        let s = &mut self.search;
        if let Some((x, extend)) = s.pending_click.take() {
            let pos = g.hit(&s.edit.text, font, x - inner.x + s.edit_scroll);
            s.edit.move_to(pos, extend);
        }
        let caret_x = g.caret_x(&s.edit.text, font, s.edit.caret);
        if caret_x - s.edit_scroll > inner.w {
            s.edit_scroll = caret_x - inner.w;
        } else if caret_x < s.edit_scroll {
            s.edit_scroll = caret_x;
        }
        let x0 = inner.x - s.edit_scroll;
        let sel = s.edit.selection();
        if !sel.is_empty() {
            let a = g.caret_x(&s.edit.text, font, sel.start);
            let b = g.caret_x(&s.edit.text, font, sel.end);
            g.dl.rect(Rect::new(x0 + a, r.y + (r.h * 0.2).round(), b - a, (r.h * 0.6).round()), t.text_selection);
        }
        if s.edit.text.is_empty() {
            g.text(inner.x, baseline, PLACEHOLDER, font, t.text_dim.alpha(0xA0), inner.w);
        } else {
            g.text(x0, baseline, &s.edit.text, font, t.text, f32::INFINITY);
        }
        let caret_h = (r.h * 0.6).round();
        g.dl.rect(Rect::new(x0 + caret_x, r.y + (r.h - caret_h) / 2.0, 1.0f32.max(self.scale.round()), caret_h), t.text);
        g.dl.set_clip(None);
    }

    fn draw_recent<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme) {
        let recent = self.search.recent();
        self.search.recent_rects.clear();
        if recent.is_empty() {
            return;
        }
        let field = self.search_layout().field;
        let font = self.ui_font(13.0);
        let m = g.metrics(font);
        let (s4, s6, row_h) = (self.s(4.0), self.s(6.0), self.s(30.0));
        let w = field.w.min(self.s(560.0));
        let x = field.x;
        let y = field.bottom() + s4;
        let h = s6 * 2.0 + row_h * (recent.len() + 1) as f32;
        let panel = Rect::new(x, y, w, h);
        let shadow = self.shadow();
        g.dl.shadow(Rect::new(panel.x, panel.y + s4, panel.w, panel.h), shadow, s6, s6 * 2.0);
        g.dl.round_rect(panel, t.bg, s6);
        g.dl.outline(panel, t.input_border, s6, 1.0);
        let small = self.ui_font(12.0);
        let head = Rect::new(x + s6, y + s6, w - 2.0 * s6, row_h);
        g.text(head.x + self.s(10.0), (head.y + (head.h + m.ascent - m.descent) / 2.0).round(), "Recent searches · Shift+Del removes one", small, t.text_dim, head.w);
        for (n, &k) in recent.iter().enumerate() {
            let r = Rect::new(x + s6, head.bottom() + n as f32 * row_h, w - 2.0 * s6, row_h);
            if self.search.recent_sel == Some(n) {
                g.dl.round_rect(r, t.selection, s4);
            } else if self.search.hover == SHit::Recent(n) {
                g.dl.round_rect(r, t.hover, s4);
            }
            centered(g, Rect::new(r.x + s4, r.y, self.s(24.0), r.h), G_HISTORY, self.icon_font(11.0), t.text_dim);
            let q = &self.search.history[k];
            g.text(r.x + self.s(34.0), (r.y + (r.h + m.ascent - m.descent) / 2.0).round(), q, font, t.text, r.w - self.s(44.0));
            self.search.recent_rects.push(r);
        }
    }

    pub(super) fn search_ime_area(&self) -> Option<Rect> {
        self.search.active.then(|| self.search_layout().field)
    }
}

#[cfg(test)]
#[path = "../../test/ui/search.rs"]
mod tests;
