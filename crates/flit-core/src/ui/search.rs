use super::edit::{EditAction, LineEdit};
use super::{centered, icon_key, App, Effect, Hit, G_BACK, G_FILE, G_FOLDER, G_SORT_DOWN, G_SORT_UP, ICON};
use crate::fmt as format;
use crate::hash::FxHashMap;
use crate::input::{CursorIcon, Event, Key, Mods, MouseButton};
use crate::listing::{attr, Entry, Meta};
use crate::location::Location;
use crate::name;
use crate::open::{OpenTarget, Route};
use crate::provider::ProviderRequest;
use crate::render::Rect;
use crate::text::{FontFamily, FontStyle, Gfx, TextBackend};
use crate::theme::Theme;
use bumpalo::Bump;
use std::collections::HashSet;
use std::fmt::Write as _;
use std::time::{Duration, Instant};

pub const PAGE: u64 = 64;
const KEEP_ROWS: usize = 4096;
const KEEP_AROUND: u64 = 2048;
const G_SEARCH: &str = "\u{E721}";
const PLACEHOLDER: &str = "Search all files: words, *.ext, ext:, size:, dm:, path:, regex:";

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

#[derive(Clone, Debug, PartialEq)]
pub enum SearchRequest {
    Query { gen: u64, text: String, sort: SearchSort },
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

#[derive(Clone, Debug, PartialEq)]
pub enum SearchEvent {
    Done { gen: u64, total: u64 },
    Rows { gen: u64, from: u64, rows: Vec<SearchRow> },
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
    Header(usize),
    Row(u64),
    Empty,
    Track,
    Thumb,
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
    total: u64,
    done: bool,
    rows: FxHashMap<u64, SearchRow>,
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
    scope: Option<(String, String)>,
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

impl App {
    pub fn open_search(&mut self) {
        let lib = self.current_library().and_then(|p| self.nav.sidebar.library(&p).cloned());
        let scope = lib.and_then(|l| Some((l.name, crate::library::search_scope(&l.folders)?)));
        let s = &mut self.search;
        if !s.active {
            s.active = true;
            s.drag = SDrag::None;
            self.editing = false;
            self.marquee = None;
            let title = scope.as_ref().map_or_else(|| "Search — Flit".to_owned(), |(name, _)| format!("Search in {name} — Flit"));
            self.effects.push(Effect::Title(title));
            let rescope = s.scope != scope;
            s.scope = scope;
            if s.gen == 0 || rescope {
                self.search_query(Instant::now());
            }
        }
        self.search.edit.select_all();
        self.dirty = true;
    }

    pub fn close_search(&mut self) {
        if self.search.active {
            self.search.active = false;
            self.search.drag = SDrag::None;
            self.effects.push(Effect::Title(format!("{} — Flit", self.tab().title())));
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

    pub fn on_search_event(&mut self, ev: SearchEvent, now: Instant) {
        let s = &mut self.search;
        match ev {
            SearchEvent::Done { gen, total } if gen == s.gen => {
                s.total = total;
                s.done = true;
                s.rows.clear();
                s.requested.clear();
                s.focus = s.focus.filter(|&f| f < total);
                if let Some(t) = s.started.take() {
                    s.elapsed = Some(now.saturating_duration_since(t));
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

    fn search_query(&mut self, now: Instant) {
        let s = &mut self.search;
        s.gen += 1;
        s.total = 0;
        s.done = false;
        s.rows.clear();
        s.requested.clear();
        s.focus = None;
        s.scroll = 0.0;
        s.error = None;
        s.started = Some(now);
        s.elapsed = None;
        let text = match &s.scope {
            Some((_, scope)) => format!("{scope} {}", s.edit.text),
            None => s.edit.text.clone(),
        };
        self.effects.push(Effect::Search(SearchRequest::Query { gen: s.gen, text, sort: s.sort }));
        self.dirty = true;
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
        let content = self.search.total as f64 * rh as f64;
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
        (self.search.total as f64 * l.row_h.max(1.0) as f64 - l.list.h as f64).max(0.0)
    }

    fn search_range(&self) -> (u64, u64) {
        let l = &self.layout;
        let rh = l.row_h.max(1.0) as f64;
        let s = &self.search;
        let first = (s.scroll / rh).floor() as u64;
        let last = ((s.scroll + l.list.h as f64) / rh).ceil() as u64;
        (first.min(s.total), last.min(s.total))
    }

    fn search_hit(&self, x: f32, y: f32) -> SHit {
        let sl = self.search_layout();
        if self.layout.nav[0].contains(x, y) {
            return SHit::Back;
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
            let row = ((self.search.scroll + (y - sl.list.y) as f64) / sl.row_h as f64).floor() as u64;
            return if row < self.search.total { SHit::Row(row) } else { SHit::Empty };
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

    fn search_key(&mut self, key: Key, m: Mods, now: Instant) -> bool {
        self.dirty = true;
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
            (Key::Char('E'), true, false) => {
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
        let (top, bottom) = (row as f64 * rh, (row + 1) as f64 * rh);
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
        let r = self.search.rows.get(&row)?;
        if r.dir.is_empty() {
            return None;
        }
        Some(Location::parse_exact(&r.dir)?.join(&r.name))
    }

    fn open_search_row(&mut self, row: u64, now: Instant) {
        let Some(loc) = self.search_row_location(row) else { return };
        if self.search.rows[&row].meta.attrs & attr::DIR != 0 {
            self.close_search();
            self.dispatch(vec![Route { folder: loc, select: None }], OpenTarget::CurrentTab, now);
        } else {
            let tab = self.tab().id;
            self.effects.push(Effect::Provider { provider: loc.provider(), req: ProviderRequest::Open { tab, path: loc.path().clone() } });
        }
    }

    fn reveal_search_row(&mut self, row: u64, now: Instant) {
        let Some(loc) = self.search_row_location(row) else { return };
        let Some(folder) = loc.parent() else { return };
        let select = Some(loc.file_name().to_vec());
        self.close_search();
        self.dispatch(vec![Route { folder, select }], OpenTarget::CurrentTab, now);
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
        match h {
            SHit::Field => {
                if clicks >= 2 {
                    self.search.edit.select_all();
                } else {
                    self.search.pending_click = Some((x, self.mods.shift));
                    self.search.drag = SDrag::Select;
                }
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
        if button == MouseButton::Right && matches!(h, SHit::Header(_) | SHit::Row(_) | SHit::Empty) {
            let menu = self.build_search_menu();
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
        let (first, last) = self.search_range();
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
        if s.done && s.total == 0 {
            let msg = if s.error.is_some() { "The query is not valid." } else { "No files match." };
            let w = g.measure(msg, font);
            g.text(sl.list.x + (sl.list.w - w) / 2.0, sl.list.y + self.s(40.0), msg, font, t.text_dim, f32::INFINITY);
        }
        for row in first..last {
            let y = (sl.list.y as f64 + row as f64 * rh as f64 - s.scroll).round() as f32;
            let rr = Rect::new(sl.list.x + self.s(4.0), y + 1.0, sl.list.w - self.s(8.0), rh - 2.0);
            if s.focus == Some(row) {
                g.dl.round_rect(rr, if focused { t.selection } else { t.selection_inactive }, self.s(4.0));
            } else if s.hover == SHit::Row(row) && s.drag == SDrag::None {
                g.dl.round_rect(rr, t.hover, self.s(4.0));
            }
            let Some(r) = s.rows.get(&row) else { continue };
            let is_dir = r.meta.attrs & attr::DIR != 0;
            let ext_at = if is_dir { r.name.len() } else { r.name.len() - name::ext_len(&r.name) };
            let ext = &r.name[ext_at..];
            let entry = Entry { size: 0, modified: 0, created: 0, name_off: 0, attrs: r.meta.attrs, name_len: 0, ext_len: 0 };
            let (key, _) = icon_key(&r.name, ext, &entry, icon_px as u32, &r.dir, self.icon_epoch);
            let ir = Rect::new(sl.cols[0].x + pad, y + ((rh - icon_px) / 2.0).round(), icon_px, icon_px);
            if !g.image(key, ir) {
                let (glyph, color) = if is_dir { (G_FOLDER, t.accent.alpha(0xB0)) } else { (G_FILE, t.text_dim.alpha(0x90)) };
                centered(g, ir, glyph, FontStyle::new(FontFamily::Icons, icon_px * 0.85, false), color);
            }
            let ty = y + text_dy;
            let name_x = ir.right() + self.s(8.0);
            let color = if r.meta.attrs & attr::HIDDEN != 0 { t.text_dim } else { t.text };
            g.text(name_x, ty, &name::display(&r.name), font, color, sl.cols[0].right() - name_x - pad);
            let run_start = row == 0 || s.rows.get(&(row - 1)).is_none_or(|p| p.dir != r.dir);
            if !s.group_folder || run_start {
                let folder_color = if s.group_folder { t.text } else { t.text_dim };
                g.text(sl.cols[1].x + pad, ty, &name::display(&r.dir), font, folder_color, sl.cols[1].w - 2.0 * pad);
            }
            if s.group_folder && run_start && row > first {
                g.dl.rect(Rect::new(sl.list.x + self.s(4.0), y, sl.list.w - self.s(8.0), 1.0), t.separator);
            }
            if !is_dir {
                let mut sz = bumpalo::collections::String::new_in(arena);
                format::size(&mut sz, r.meta.size);
                let w = g.measure(&sz, font);
                g.text((sl.cols[2].right() - pad - w).max(sl.cols[2].x + pad), ty, &sz, font, t.text_dim, sl.cols[2].w - 2.0 * pad);
            }
            let mut d = bumpalo::collections::String::new_in(arena);
            format::datetime(&mut d, r.meta.modified, self.utc_offset_min);
            g.text(sl.cols[3].x + pad, ty, &d, font, t.text_dim, sl.cols[3].w - 2.0 * pad);
        }
        g.dl.set_clip(None);
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
        } else if s.done {
            let _ = write!(left, "{} {}", thousands(s.total), if s.total == 1 { "result" } else { "results" });
            if let Some(e) = s.elapsed {
                let _ = write!(left, " in {:.1} ms", e.as_secs_f64() * 1e3);
            }
        } else {
            left.push_str("Searching…");
        }
        g.text(status.x + pad, baseline, &left, font, color, status.w / 2.0 - pad);
        let mut right = bumpalo::collections::String::new_in(arena);
        right.push_str(match s.backend {
            SearchBackend::Connecting => "Connecting to the index",
            SearchBackend::Service => "Index service",
            SearchBackend::Fallback => "Local index (flit-indexd unavailable)",
        });
        for v in &s.volumes {
            let label = if v.id.is_empty() { "index" } else { &v.id };
            let _ = match &v.state {
                IndexState::Ready => write!(right, " · {label} {} files", thousands(v.files)),
                IndexState::Scanning(p) => write!(right, " · {label} indexing {:.0}% ({} files)", p * 100.0, thousands(v.files)),
                IndexState::Error(e) => write!(right, " · {label} error: {e}"),
            };
        }
        let rcolor = if s.backend == SearchBackend::Fallback { t.error } else { t.text_dim };
        let rw = g.measure(&right, font).min(status.w / 2.0 - pad);
        g.text(status.right() - pad - rw, baseline, &right, font, rcolor, status.w / 2.0 - pad);
    }

    fn draw_search_field<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme) {
        let r = self.search_layout().field;
        let radius = self.s(4.0);
        g.dl.round_rect(r, t.input_bg, radius);
        g.dl.outline(r, t.accent, radius, self.s(1.0));
        let icon = Rect::new(r.x + self.s(4.0), r.y, self.s(24.0), r.h);
        centered(g, icon, G_SEARCH, self.icon_font(11.0), t.text_dim);
        let font = self.ui_font(13.0);
        let m = g.metrics(font);
        let baseline = (r.y + (r.h + m.ascent - m.descent) / 2.0).round();
        let inner = Rect::new(icon.right() + self.s(4.0), r.y, r.right() - icon.right() - self.s(12.0), r.h);
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

    pub(super) fn search_ime_area(&self) -> Option<Rect> {
        self.search.active.then(|| self.search_layout().field)
    }
}

#[cfg(test)]
#[path = "../../test/ui/search.rs"]
mod tests;
