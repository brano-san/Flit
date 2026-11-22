use super::edit::{EditAction, Grab, LineEdit};
use super::sidebar::folder_icon;
use super::tooltip::TipText;
use super::{centered, icon_key, App, Effect, Hit, IconRequest, G_BACK, G_CHEVRON, G_FILE, G_FOLDER, G_SORT_DOWN, G_SORT_UP, ICON};
use crate::fmt as format;
use crate::hash::FxHashMap;
use crate::i18n::{self, P, S};
use crate::{tr, trn, trn_into, tr_into};
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
use std::time::{Duration, Instant};

pub const PAGE: u64 = 64;
pub const MAX_HISTORY: usize = 32;
const KEEP_ROWS: usize = 4096;
const KEEP_AROUND: u64 = 2048;
const MAX_RECENT: usize = 8;
const G_SEARCH: &str = "\u{E721}";
const G_HISTORY: &str = "\u{E81C}";
const G_FACETS: &str = "\u{E71C}";

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
    Query { gen: u64, text: String, conditions: Vec<String>, sort: SearchSort, scope: SearchScope, paths: crate::query_attrs::SearchPaths },
    Rows { gen: u64, from: u64, to: u64 },
    Folders { gen: u64 },
    Facets { gen: u64, open: bool },
    Close,
    Indexing(bool),
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
    Off,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchSource {
    Walk,
    Shell,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SearchEvent {
    Done { gen: u64, total: u64, version: u64 },
    Found { gen: u64, total: u64, folders: u64, done: bool, source: SearchSource },
    Rows { gen: u64, from: u64, rows: Vec<SearchRow> },
    Groups { gen: u64, starts: Vec<u64> },
    Folders { gen: u64, counts: crate::query_attrs::FolderCounts },
    Facets { gen: u64, counts: crate::query_attrs::Facets },
    FacetTags { gen: u64, counts: Vec<(String, u64)> },
    Status { backend: SearchBackend, volumes: Vec<IndexVolume>, note: Option<String> },
    Error { message: String },
    QueryError { gen: u64, message: String },
}

const COLUMNS: [(S, SearchKey); 4] = [(S::DetailsColumnName, SearchKey::Name), (S::SearchColumnFolder, SearchKey::Folder), (S::DetailsColumnSize, SearchKey::Size), (S::DetailsColumnModified, SearchKey::Modified)];

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum SHit {
    #[default]
    None,
    Back,
    Field,
    Scope,
    Recent(usize),
    Header(usize),
    ColSep(usize),
    Group(usize),
    Row(u64),
    Empty,
    Track,
    Thumb,
    HTrack,
    HThumb,
    Folders,
    WithinResults,
    ResultFolder(usize),
    FolderBackground,
    SimilarChip(usize, bool),
    Facets,
    Facet(usize),
    Date(usize),
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
    HThumb(f32),
    Select(Grab),
    ColSep { col: usize, x: f32, width: f32 },
}

#[derive(Default)]
pub(super) struct SearchView {
    pub(super) active: bool,
    pub(super) edit: LineEdit,
    edit_scroll: f32,
    pending_click: Option<(f32, Grab)>,
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
    widths: Option<[f32; 4]>,
    fit: Option<usize>,
    elided: Vec<u64>,
    scroll_x: f32,
    folders_open: bool,
    folders_summary: crate::query_attrs::FolderCounts,
    folders_scroll: usize,
    result_folder: Option<crate::FsPath>,
    within_results: Option<String>,
    within_native: bool,
    similar: bool,
    similar_chips: Vec<(std::ops::Range<usize>, Rect)>,
    facets_open: bool,
    requery_on_open: bool,
    summary_stamp: Option<(u64, Option<u64>)>,
    facets: crate::query_attrs::Facets,
    facet_tags: Vec<(String, u64)>,
    facet_filter: [Option<String>; 4],
    date_filter: Option<(i64, i64)>,
}

struct SLayout {
    field: Rect,
    header: Rect,
    cols: [Rect; 4],
    list: Rect,
    track: Rect,
    thumb: Rect,
    htrack: Rect,
    hthumb: Rect,
    max_x: f32,
    row_h: f32,
    folders: Rect,
    tools: [Rect; 3],
    chips: Rect,
    facets: Rect,
    timeline: Rect,
    grid: bool,
    tiles: u64,
}

impl SLayout {
    fn cell(&self, line: u64, scroll: f64) -> Rect {
        let width = self.list.w / self.tiles as f32;
        let x = self.list.x + (line % self.tiles) as f32 * width;
        let y = (self.list.y as f64 + (line / self.tiles) as f64 * f64::from(self.row_h) - scroll).round() as f32;
        Rect::new(x, y, width, self.row_h)
    }
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

fn day_query(day: i64) -> String {
    let (y, m, d) = crate::fmt::civil_from_days(day);
    format!("{y:04}-{m:02}-{d:02}")
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

    fn has_facet_filters(&self) -> bool {
        self.date_filter.is_some() || self.facet_filter.iter().any(Option::is_some)
    }

    fn clear_facet_filters(&mut self) -> bool {
        let active = self.has_facet_filters();
        self.facet_filter = Default::default();
        self.date_filter = None;
        active
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
        self.open_scoped(self.folder_search_within());
    }

    fn folder_search_within(&self) -> Option<Within> {
        let loc = self.current_location().clone();
        let usable = matches!(loc.path().kind(), PathKind::Drive | PathKind::Unc | PathKind::Verbatim | PathKind::Unix);
        self.library_within().or_else(|| usable.then_some(Within::Folder(loc)))
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
        let requery = s.gen == 0 || s.within != within || s.requery_on_open;
        s.within = within;
        let reopening = !s.active;
        if reopening {
            s.active = true;
            s.drag = SDrag::None;
            s.recent_open = true;
            s.recent_sel = None;
            self.marquee = None;
        }
        self.effects.push(Effect::Title(self.search_title()));
        if reopening && !requery && self.search.done {
            if self.search.facets_open {
                self.effects.push(Effect::Search(SearchRequest::Facets { gen: self.search.gen, open: true }));
            }
            if self.search.folders_open {
                self.effects.push(Effect::Search(SearchRequest::Folders { gen: self.search.gen }));
            }
        }
        if requery {
            self.search_query(Instant::now());
        }
        self.search.edit.select_all();
        self.dirty = true;
    }

    pub(super) fn search_title(&self) -> String {
        match &self.search.within {
            Some(w) => tr!(S::SearchTitleIn, name = w.name()),
            None => S::SearchTitle.t().into(),
        }
    }

    pub fn close_search(&mut self) {
        if self.search.active {
            self.remember_query();
            if self.search.facets_open {
                self.effects.push(Effect::Search(SearchRequest::Facets { gen: self.search.gen, open: false }));
            }
            self.search.requery_on_open = self.search.clear_facet_filters();
            self.search.active = false;
            self.search.edit.primary_id = 0;
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

    pub fn search_finished(&self) -> Option<u64> {
        (self.search.done && !self.search.walking).then_some(self.search.total)
    }

    pub(super) fn set_indexing(&mut self, on: bool) {
        self.indexing = on;
        self.effects.push(Effect::Search(SearchRequest::Indexing(on)));
    }

    pub fn search_backend(&self) -> SearchBackend {
        self.search.backend
    }

    pub fn index_ready(&self) -> Option<u64> {
        let v = &self.search.volumes;
        let ready = self.search.backend == SearchBackend::Off || (!v.is_empty() && v.iter().all(|v| !matches!(v.state, IndexState::Scanning(_))));
        ready.then(|| v.iter().map(|v| v.files).sum())
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
        let stamp = match &ev {
            SearchEvent::Done { gen, total, version } if *gen == s.gen => Some((*total, Some(*version))),
            SearchEvent::Found { gen, total, done: true, .. } if *gen == s.gen => Some((*total, None)),
            _ => None,
        };
        let summary_due = stamp.is_some_and(|stamp| s.summary_stamp.replace(stamp) != Some(stamp));
        match ev {
            SearchEvent::Done { gen, total, .. } if gen == s.gen => {
                if !summary_due {
                    return;
                }
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
            SearchEvent::Facets { gen, counts } if gen == s.gen && s.facets_open => s.facets = counts,
            SearchEvent::FacetTags { gen, counts } if gen == s.gen && s.facets_open => s.facet_tags = counts,
            SearchEvent::Folders { gen, counts } if gen == s.gen && s.folders_open => {
                s.folders_summary = counts;
                s.folders_scroll = s.folders_scroll.min(s.folders_summary.len().saturating_sub(1));
            }
            SearchEvent::Groups { gen, starts } if gen == s.gen => {
                let ordered = starts.first() == Some(&0) && starts.windows(2).all(|w| w[0] < w[1]) && starts.last().is_some_and(|&l| l < s.total);
                s.groups = if ordered { starts } else { Vec::new() };
            }
            SearchEvent::Status { backend, volumes, note } => {
                if s.backend != backend {
                    s.summary_stamp = None;
                }
                s.backend = backend;
                s.volumes = volumes;
                s.note = note;
            }
            SearchEvent::Error { message } => s.error = Some(message),
            SearchEvent::QueryError { gen, message } if gen == s.gen => s.error = Some(message),
            _ => return,
        }
        if self.search.active && self.search.folders_open && summary_due {
            self.effects.push(Effect::Search(SearchRequest::Folders { gen: self.search.gen }));
        }
        if self.search.active && self.search.facets_open && summary_due {
            self.effects.push(Effect::Search(SearchRequest::Facets { gen: self.search.gen, open: true }));
        }
        self.dirty |= self.search.active;
    }

    pub fn open_saved_search(&mut self, text: &str) -> bool {
        if !self.search.history.iter().any(|query| query == text) {
            return false;
        }
        self.search_for(text);
        true
    }

    pub(super) fn search_for(&mut self, text: &str) {
        self.search_for_in(text, self.library_within());
    }

    fn search_for_in(&mut self, text: &str, within: Option<Within>) {
        self.search.clear_facet_filters();
        self.search.similar = false;
        self.search.within_results = None;
        self.search.within_native = false;
        self.search.result_folder = None;
        self.search.edit.set(text);
        let fresh = self.search.gen == 0 || self.search.within != within || self.search.requery_on_open;
        self.open_scoped(within);
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

    pub(super) fn similar_item(&self) -> Option<(Vec<u8>, Meta, u32)> {
        let view = self.tab().view.as_ref()?;
        if view.selected_count() != 1 {
            return None;
        }
        let i = view.selected().next()?;
        let entry = &view.listing().entries[i as usize];
        (!entry.is_dir()).then(|| (view.listing().name(i).to_vec(), entry.meta(), entry.tags))
    }

    pub(super) fn find_similar(&mut self) {
        let Some((name, meta, tags)) = self.similar_item() else { return };
        self.search_similar(&name, meta, tags);
    }

    fn search_similar(&mut self, name: &[u8], meta: Meta, tags: u32) {
        let mut parts = vec![String::from("file:")];
        let ext = &name[name.len() - crate::name::ext_len(name)..];
        if !ext.is_empty() {
            let ext = name::display(ext);
            if !ext.contains(['"', ';']) {
                parts[0] = format!("ext:\"{ext}\"");
            }
        }
        if meta.modified > 0 {
            let day = crate::fmt::local_days(meta.modified, self.utc_offset_min);
            parts.push(format!("dm:{}", day_query(day)));
        }
        let margin = (meta.size / 5).max(1);
        parts.push(format!("size:{}..{}", meta.size.saturating_sub(margin), meta.size.saturating_add(margin)));
        for k in crate::tags::bits(tags).take(8) {
            if let Some(tag) = self.tag_defs().get(k).filter(|t| !t.name.contains('"')) {
                parts.push(format!("tag:\"{}\"", tag.name));
            }
        }
        let within = if self.indexing { self.library_within() } else { self.folder_search_within() };
        self.search_for_in(&parts.join(" "), within);
        self.search.similar = true;
        self.search.recent_open = false;
        self.search.edit.move_to(self.search.edit.text.len(), false);
    }

    pub(super) fn toggle_facets(&mut self) {
        if !self.search.active || (!self.search.facets_open && self.search.source == Some(SearchSource::Shell)) {
            return;
        }
        self.search.facets_open = !self.search.facets_open;
        if !self.search.facets_open || self.search.done {
            self.effects.push(Effect::Search(SearchRequest::Facets { gen: self.search.gen, open: self.search.facets_open }));
        }
        if !self.search.facets_open && self.search.clear_facet_filters() {
            self.search_query(Instant::now());
        }
        self.dirty = true;
    }

    pub(super) fn choose_search_facet(&mut self, category: usize, expression: Option<String>, now: Instant) {
        let Some(selected) = self.search.facet_filter.get_mut(category) else { return };
        if category == 1 && expression.is_none() {
            self.search.date_filter = None;
        }
        *selected = if *selected == expression { None } else { expression };
        self.search_query(now);
    }

    fn facet_menu(&mut self, category: usize) {
        use super::menu::Cmd;
        let s = &self.search;
        let entries: Vec<(String, String, u64)> = match category {
            0 => s.facets.extensions.iter().map(|(ext, n)| {
                let (title, expression) = match ext.as_str() {
                    "/" => (S::SearchExploreFolders.t().into(), "folder:".to_owned()),
                    "" => (S::SearchExploreNoExtension.t().into(), "ext:".to_owned()),
                    ext => (ext.to_owned(), format!("ext:\"{ext}\"")),
                };
                (title, expression, *n)
            }).collect(),
            1 => s.facets.years.iter().map(|(y, n)| (y.to_string(), format!("dm:{y:04}"), *n)).collect(),
            2 => crate::query_attrs::SIZE_STARTS.iter().enumerate().map(|(i, start)| {
                let end = crate::query_attrs::SIZE_STARTS.get(i + 1).copied();
                let mut title = String::new();
                format::size_in(&mut title, *start, self.style.decimal);
                if let Some(end) = end {
                    title.push_str(" – ");
                    format::size_in(&mut title, end, self.style.decimal);
                } else {
                    title.push('+');
                }
                let expr = end.map_or_else(|| format!("size:>={start}"), |end| format!("size:{start}..{}", end - 1));
                (title, expr, s.facets.sizes[i])
            }).collect(),
            3 => s.facet_tags.iter().filter(|(tag, _)| !tag.contains('"')).map(|(tag, n)| (tag.clone(), format!("tag:\"{tag}\""), *n)).collect(),
            _ => return,
        };
        let selected = s.facet_filter[category].clone();
        let no_selection = selected.is_none() && (category != 1 || s.date_filter.is_none());
        let r = self.search_layout().facets;
        let menu = self.collect(|_, m| {
            m.entry(Cmd::SearchFacet { category, expression: None }, S::SearchExploreAll.t(), String::new(), super::Mark::None, false, no_selection);
            for (label, expression, count) in entries {
                let checked = selected.as_ref() == Some(&expression);
                m.entry(Cmd::SearchFacet { category, expression: Some(expression) }, &label, thousands(count), super::Mark::None, checked, count == 0 && !checked);
            }
        });
        self.flit_menu(r.x + r.w * category as f32 / 4.0, r.bottom(), menu);
    }

    fn select_dates(&mut self, first: usize, last: usize) {
        let dates = &self.search.facets.dates;
        if let Some((start, end)) = dates.get(first.min(last)).zip(dates.get(first.max(last))) {
            let selected = (start.from, end.to);
            self.search.date_filter = (self.search.date_filter != Some(selected)).then_some(selected);
            self.search_query(Instant::now());
        }
    }

    pub(super) fn toggle_search_folders(&mut self) {
        if self.search.active {
            self.search.folders_open = !self.search.folders_open;
            if self.search.folders_open && self.search.done {
                self.effects.push(Effect::Search(SearchRequest::Folders { gen: self.search.gen }));
            }
            if !self.search.folders_open && self.search.result_folder.take().is_some() {
                self.search_query(Instant::now());
            }
            self.dirty = true;
        }
    }

    pub(super) fn toggle_within_results(&mut self, now: Instant) {
        if !self.search.active {
            return;
        }
        if let Some(base) = self.search.within_results.take() {
            self.search.edit.set(&base);
            self.search.within_native = false;
        } else if !self.search.edit.text.trim().is_empty() && !(self.caps.unix_paths && self.search.source == Some(SearchSource::Shell)) {
            self.search.within_native = !self.caps.unix_paths && !matches!(self.search.within, Some(Within::Library { .. }))
                && (self.search.source == Some(SearchSource::Shell) || self.search.edit.text.trim_start().starts_with('$'));
            self.search.within_results = Some(self.search.edit.text.clone());
            self.search.edit.set("");
        } else {
            return;
        }
        self.search_query(now);
        self.search.recent_open = false;
    }

    fn choose_result_folder(&mut self, k: usize, now: Instant) {
        let Some(path) = self.search.folders_summary.get(k).and_then(|(p, _)| crate::FsPath::parse_exact(p)) else { return };
        self.search.result_folder = if self.search.result_folder.as_ref().is_some_and(|p| p.same(&path)) { None } else { Some(path) };
        self.search_query(now);
    }

    fn search_scope(&self) -> SearchScope {
        let Some(Within::Folder(folder)) = self.search.within.clone() else { return SearchScope::All };
        let archived = crate::archive::split(folder.path()).is_some();
        let ready = self.indexing && !archived && drive_id(&folder).is_some_and(|d| self.search.volumes.iter().any(|v| v.id.eq_ignore_ascii_case(&d) && v.state == IndexState::Ready));
        if ready {
            SearchScope::Indexed(folder)
        } else {
            SearchScope::Walk(folder)
        }
    }

    fn search_query(&mut self, now: Instant) {
        let scope = self.search_scope();
        let s = &mut self.search;
        s.requery_on_open = false;
        s.summary_stamp = None;
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
        let extra = s.edit.text.trim();
        let mut conditions = Vec::new();
        let text = match &s.within_results {
            Some(base) if s.within_native && !extra.is_empty() => {
                let base = base.trim_start();
                let base = base.strip_prefix('$').unwrap_or(base).trim();
                let extra = extra.strip_prefix('$').unwrap_or(extra).trim();
                format!("$ ({base}) AND ({extra})")
            }
            Some(base) if !extra.is_empty() => {
                conditions.push(base.clone());
                s.edit.text.clone()
            }
            Some(base) => base.clone(),
            None => s.edit.text.clone(),
        };
        if let Some(Within::Library { filter, .. }) = &s.within {
            conditions.push(filter.clone());
        }
        if !s.within_native && !text.trim_start().starts_with('$') {
            conditions.extend(s.facet_filter.iter().flatten().cloned());
            if let Some((a, b)) = s.date_filter {
                conditions.push(format!("dm:{}..{}", day_query(a), day_query(b)));
            }
        }
        let paths = crate::query_attrs::SearchPaths {
            exclude: self.search_exclusions.iter().filter_map(Location::to_windows).collect(),
            parent: s.result_folder.clone(),
            under: None,
        };
        self.effects.push(Effect::Search(SearchRequest::Query { gen: s.gen, text, conditions, sort: s.sort, scope, paths }));
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
        let w = (l.address.right() - l.nav[1].x).max(self.s(40.0));
        let button = self.s(30.0).min(w / 6.0);
        let field = Rect::new(l.nav[1].x, l.address.y, w - button * 3.0, l.address.h);
        let tools = std::array::from_fn(|i| Rect::new(field.right() + button * i as f32, field.y, button, field.h));
        let mut list = l.list;
        let fw = if self.search.folders_open { self.s(255.0).min(list.w * 0.38) } else { 0.0 };
        let folder_y = l.header.y;
        let chip_h = if self.search.similar && !self.search.edit.text.trim().is_empty() { self.s(29.0) } else { 0.0 };
        let chips = Rect::new(list.x, folder_y, list.w, chip_h);
        let facet_h = if self.search.facets_open { self.s(29.0) } else { 0.0 };
        let facets = Rect::new(list.x, chips.bottom(), list.w, facet_h);
        let timeline = Rect::new(list.x, facets.bottom(), list.w, facet_h);
        let top = chip_h + 2.0 * facet_h;
        let folders = Rect::new(list.x, folder_y + top, fw, (list.bottom() - folder_y - top).max(0.0));
        list.y += top;
        list.h = (list.h - top).max(0.0);
        let track = Rect::new(l.track.x, l.track.y + top, l.track.w, (l.track.h - top).max(0.0));
        list.x += fw;
        list.w = (list.w - fw).max(0.0);
        let header = Rect::new(list.x, l.header.y + top, (l.header.w - fw).max(0.0), l.header.h);
        let size_w = self.s(90.0).min(list.w / 4.0);
        let date_w = self.s(140.0).min(list.w / 4.0);
        let name_w = (list.w * 0.32).max(self.s(140.0)).min(list.w - size_w - date_w);
        let folder_w = (list.w - name_w - size_w - date_w).max(0.0);
        let grid = self.opts.view_rules.search_grid && !self.search.group_folder;
        let mut cols = [Rect::default(); 4];
        let defaults = [name_w, folder_w, size_w, date_w];
        let widths = if grid { defaults } else { self.search.widths.map(|widths| widths.map(|w| w * self.scale)).unwrap_or(defaults) };
        let content_w = widths.iter().sum::<f32>();
        let max_x = (content_w - list.w).max(0.0);
        let mut x = list.x - self.search.scroll_x.clamp(0.0, max_x);
        let htrack = if max_x > 0.0 { Rect::new(list.x, list.bottom() - self.s(14.0), list.w, self.s(14.0)) } else { Rect::default() };
        let hthumb = if max_x > 0.0 {
            list.h = (list.h - htrack.h).max(0.0);
            let width = (htrack.w * htrack.w / content_w).max(self.s(24.0)).min(htrack.w);
            Rect::new(htrack.x + (htrack.w - width) * self.search.scroll_x.clamp(0.0, max_x) / max_x, htrack.y + self.s(4.0), width, self.s(6.0))
        } else { Rect::default() };
        for (c, w) in cols.iter_mut().zip(widths) {
            *c = Rect::new(x, header.y, w, header.h);
            x += w;
        }
        let tiles = if grid { ((list.w / self.s(135.0).max(1.0)) as u64).max(1) } else { 1 };
        let rh = if grid { self.s(94.0) } else { l.row_h }.max(1.0);
        let content = self.search.lines().div_ceil(tiles) as f64 * rh as f64;
        let lh = list.h as f64;
        let thumb = if content > lh {
            let th = (track.h as f64 * lh / content).max(self.s(24.0) as f64).min(track.h as f64) as f32;
            let frac = (self.search.scroll / (content - lh)).clamp(0.0, 1.0) as f32;
            Rect::new(track.x + self.s(3.0), track.y + (track.h - th) * frac, track.w - self.s(6.0), th)
        } else {
            Rect::default()
        };
        SLayout { field, header, cols, list, track, thumb, htrack, hthumb, max_x, row_h: rh, folders, tools, chips, facets, timeline, grid, tiles }
    }

    pub(super) fn search_pans_at(&self, x: f32, y: f32) -> bool {
        matches!(self.search_hit(x, y), SHit::Row(_) | SHit::Group(_) | SHit::Empty)
    }

    pub(super) fn touch_pan_search(&mut self, dx: f32, dy: f32) -> (bool, bool) {
        let (max_x, max) = (self.search_layout().max_x, self.search_max_scroll());
        let s = &mut self.search;
        let x = (s.scroll_x - dx).clamp(0.0, max_x);
        let y = (s.scroll - f64::from(dy)).clamp(0.0, max);
        let moved = (x != s.scroll_x, y != s.scroll);
        (s.scroll_x, s.scroll) = (x, y);
        moved
    }

    pub(super) fn search_let_go(&mut self) {
        self.search.pressed = SHit::None;
        self.search.drag = SDrag::None;
    }

    fn search_max_scroll(&self) -> f64 {
        let sl = self.search_layout();
        (self.search.lines().div_ceil(sl.tiles) as f64 * f64::from(sl.row_h) - f64::from(sl.list.h)).max(0.0)
    }

    fn search_range(&self) -> (u64, u64) {
        let sl = self.search_layout();
        let rh = f64::from(sl.row_h);
        let s = &self.search;
        let first = (s.scroll / rh).floor() as u64;
        let last = ((s.scroll + f64::from(sl.list.h)) / rh).ceil() as u64;
        (first.saturating_mul(sl.tiles).min(s.lines()), last.saturating_mul(sl.tiles).min(s.lines()))
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

    pub(super) fn search_field_at(&self, x: f32, y: f32) -> bool {
        self.search.active && self.search_hit(x, y) == SHit::Field
    }

    fn search_hit(&self, x: f32, y: f32) -> SHit {
        let sl = self.search_layout();
        if let Some(k) = self.search.recent_rects.iter().position(|r| r.contains(x, y)) {
            return SHit::Recent(k);
        }
        if sl.chips.h > 0.0 && sl.chips.contains(x, y) {
            if let Some((i, (_, r))) = self.search.similar_chips.iter().enumerate().find(|(_, (_, r))| r.contains(x, y)) {
                return SHit::SimilarChip(i, x >= r.right() - self.s(20.0));
            }
        }
        if self.search.facets_open {
            if sl.facets.contains(x, y) {
                return SHit::Facet((((x - sl.facets.x) / sl.facets.w.max(1.0)) * 4.0) as usize);
            }
            let n = self.search.facets.dates.len();
            if n > 0 && sl.timeline.contains(x, y) {
                let i = (((x - sl.timeline.x) / sl.timeline.w.max(1.0)) * n as f32) as usize;
                return SHit::Date(i.min(n - 1));
            }
        }
        if sl.tools[2].contains(x, y) {
            return SHit::Facets;
        }
        if sl.tools[0].contains(x, y) {
            return SHit::Folders;
        }
        if sl.tools[1].contains(x, y) {
            return SHit::WithinResults;
        }
        if sl.folders.w > 0.0 && sl.folders.contains(x, y) {
            let at = y - sl.folders.y - self.layout.header.h;
            if at >= 0.0 {
                let i = self.search.folders_scroll + (at / sl.row_h).floor() as usize;
                if i < self.search.folders_summary.len() {
                    return SHit::ResultFolder(i);
                }
            }
            return SHit::FolderBackground;
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
            if !sl.grid {
                if let Some(c) = (0..4).find(|&c| (x - sl.cols[c].right()).abs() <= self.s(4.0)) {
                    return SHit::ColSep(c);
                }
            }
            return sl.cols.iter().position(|c| c.contains(x, y)).map_or(SHit::None, SHit::Header);
        }
        if sl.track.contains(x, y) {
            let t = sl.thumb;
            return if t.h > 0.0 && y >= t.y && y < t.bottom() { SHit::Thumb } else { SHit::Track };
        }
        if sl.htrack.contains(x, y) {
            return if sl.hthumb.contains(x, y) { SHit::HThumb } else { SHit::HTrack };
        }
        if sl.list.contains(x, y) {
            let band = ((self.search.scroll + (y - sl.list.y) as f64) / sl.row_h as f64).floor() as u64;
            let col = (((x - sl.list.x) / sl.list.w * sl.tiles as f32) as u64).min(sl.tiles - 1);
            let line = band.saturating_mul(sl.tiles).saturating_add(col);
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
            Event::MouseMove { x, y } => self.search_mouse_move(*x, *y, now),
            Event::Wheel { dx, dy } => {
                if matches!(self.search.hover, SHit::ResultFolder(_) | SHit::FolderBackground) {
                    let sl = self.search_layout();
                    let shown = ((sl.folders.h - self.layout.header.h).max(0.0) / sl.row_h) as usize;
                    let max = self.search.folders_summary.len().saturating_sub(shown.max(1));
                    self.search.folders_scroll = self.search.folders_scroll.saturating_add_signed((-dy * 3.0).round() as isize).min(max);
                    self.dirty = true;
                    return true;
                }
                if *dx != 0.0 || self.mods.shift {
                    let delta = if *dx != 0.0 { *dx } else { -*dy };
                    self.search.scroll_x = (self.search.scroll_x + delta * self.s(48.0)).clamp(0.0, self.search_layout().max_x);
                    self.dirty = true;
                    return true;
                }
                let sl = self.search_layout();
                let step = f64::from(self.wheel_px(sl.row_h, sl.list.h));
                let max = self.search_max_scroll();
                self.search.scroll = (self.search.scroll - *dy as f64 * step).clamp(0.0, max);
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

    pub(super) fn search_key(&mut self, key: Key, m: Mods, now: Instant) -> bool {
        self.dirty = true;
        if self.recent_key(key, m, now) {
            return true;
        }
        let sl = self.search_layout();
        let page = ((sl.list.h / sl.row_h).floor() as i64).max(1) * sl.tiles as i64;
        let mut moved = None;
        if self.keymap.runs(key, m, "search.open") {
            if self.search.within.is_some() {
                self.open_scoped(None);
            }
            self.search.edit.select_all();
            return true;
        }
        if self.keymap.runs(key, m, "search.folder") {
            if self.search.within.is_none() && self.search.home.is_some() {
                self.toggle_scope();
            }
            self.search.edit.select_all();
            return true;
        }
        match (key, m.ctrl, m.alt) {
            (Key::Tab, false, false) => {
                self.toggle_scope();
                return true;
            }
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
            (Key::Char('C'), true, false) if m.shift => {
                if let Some(p) = self.search.focus.and_then(|f| self.search_row_location(f)) {
                    let text = self.as_path_text(&p);
                    self.effects.push(Effect::SetClipboard(text));
                }
                return true;
            }
            (Key::Char('T' | 'W' | '1'..='9') | Key::Tab | Key::F(4), true, false) => {
                self.close_search();
                return false;
            }
            (Key::Left | Key::Right, false, false) if sl.grid && self.search.focus.is_some() && !m.shift => {
                moved = Some(if key == Key::Left { -1 } else { 1 });
            }
            (Key::Up, _, false) => moved = Some(if self.search.focus.is_some() { -(sl.tiles as i64) } else { -1 }),
            (Key::Down, _, false) => moved = Some(if self.search.focus.is_some() { sl.tiles as i64 } else { 1 }),
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
        let sl = self.search_layout();
        let (rh, lh) = (f64::from(sl.row_h), f64::from(sl.list.h));
        let line = self.search.line_of_row(row) / sl.tiles;
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

    pub(super) fn search_target(&self) -> Option<(Location, bool)> {
        let row = self.search.focus?;
        Some((self.search_row_location(row)?, self.search.row(row)?.meta.attrs & attr::DIR != 0))
    }

    pub(super) fn search_shell_target(&self) -> Option<super::ShellMenu> {
        let row = self.search.rows.get(&self.search.focus?)?;
        let dir = crate::FsPath::parse_exact(&row.dir)?;
        let path = dir.join(&row.name);
        if crate::archive::split(&path).is_some() || crate::recycle::in_bin(&path) {
            return None;
        }
        let kind = if row.meta.attrs & attr::DIR != 0 { super::MenuKind::Folder } else { super::MenuKind::File };
        Some(super::ShellMenu { dir, items: vec![path], x: 0.0, y: 0.0, extended: false, menu: Vec::new(), inline: self.shell_items_inline, shell: true, kind })
    }

    pub(super) fn set_search_exclusions(&mut self, paths: Vec<Location>) {
        let mut unique: Vec<Location> = Vec::new();
        for location in paths.into_iter().take(256) {
            let Some(path) = location.to_windows() else { continue };
            if matches!(path.kind(), PathKind::Drive | PathKind::Unix | PathKind::Unc | PathKind::Verbatim)
                && !unique.iter().filter_map(Location::to_windows).any(|p| p.same(&path))
            {
                unique.push(location);
            }
        }
        if self.search_exclusions != unique {
            self.search_exclusions = unique;
            if self.search.active {
                self.search_query(Instant::now());
            }
        }
    }

    pub(super) fn search_command(&mut self, action: &str, location: Location, folder: bool, now: Instant) {
        match action {
            "open" if !folder => self.effects.push(Effect::Provider { provider: location.provider(), req: ProviderRequest::Open { tab: self.tab().id, path: location.path().clone() } }),
            "open" | "reveal" | "reveal_tab" => {
                let (folder, select) = if action == "open" { (location, None) } else {
                    let Some(parent) = location.parent() else { return };
                    (parent, Some(location.file_name().to_vec()))
                };
                self.remember_query();
                self.close_search();
                self.dispatch(vec![Route { folder, select }], if action == "reveal_tab" { OpenTarget::NewTab } else { OpenTarget::CurrentTab }, now);
            }
            "exclude" => {
                let mut paths = self.search_exclusions.clone();
                paths.insert(0, location);
                self.set_search_exclusions(paths);
            }
            "path" => {
                let text = self.as_path_text(&location);
                self.effects.push(Effect::SetClipboard(text));
            }
            "quoted" => self.effects.push(Effect::SetClipboard(format!("\"{}\"", location.display()))),
            "copy" => {
                if let Some(path) = location.to_windows() {
                    self.effects.push(Effect::SetClipboardFiles { paths: vec![path], cut: false });
                }
            }
            _ => {}
        }
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
        if self.in_title(y) {
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
            SHit::ColSep(col) if button == MouseButton::Left && clicks >= 2 => {
                self.search.drag = SDrag::None;
                self.search.fit = Some(col);
            }
            SHit::ColSep(col) if button == MouseButton::Left => {
                let widths = self.search_layout().cols.map(|c| c.w / self.scale);
                self.search.widths = Some(widths);
                self.search.drag = SDrag::ColSep { col, x, width: widths[col] };
                self.set_cursor(CursorIcon::ResizeHorizontal);
            }
            SHit::Field if button == MouseButton::Right => {
                let menu = self.collect(|a, m| super::field::text_entries(m, &a.search.edit, false, a.text_on_clipboard()));
                self.flit_menu(x, y, menu);
            }
            SHit::Field => {
                if self.search_layout().grid {
                    self.search.focus = None;
                }
                match Grab::of(self.text_clicks((x, y), clicks, now), self.mods.shift) {
                    Some(how) => {
                        self.search.pending_click = Some((x, how));
                        self.search.drag = SDrag::Select(how.dragged());
                    }
                    None => self.search.edit.select_all(),
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
            SHit::HThumb => self.search.drag = SDrag::HThumb(x - self.search_layout().hthumb.x),
            SHit::HTrack => {
                let sl = self.search_layout();
                self.search.scroll_x = (self.search.scroll_x + if x < sl.hthumb.x { -sl.list.w } else { sl.list.w }).clamp(0.0, sl.max_x);
            }
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
        if self.in_title(y) && self.search.drag == SDrag::None {
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
        if button != MouseButton::Left || matches!(was, SDrag::Thumb(_) | SDrag::HThumb(_) | SDrag::ColSep { .. }) {
            return true;
        }
        if let (SHit::Date(a), SHit::Date(b)) = (pressed, h) {
            self.select_dates(a, b);
            return true;
        }
        if h != pressed {
            return true;
        }
        match h {
            SHit::Back => self.close_search(),
            SHit::Scope => self.toggle_scope(),
            SHit::Folders => self.toggle_search_folders(),
            SHit::Facets => self.toggle_facets(),
            SHit::Facet(k) => self.facet_menu(k),
            SHit::WithinResults => self.toggle_within_results(Instant::now()),
            SHit::ResultFolder(k) => self.choose_result_folder(k, Instant::now()),
            SHit::SimilarChip(i, remove) => {
                if let Some((range, _)) = self.search.similar_chips.get(i).cloned() {
                    self.search.edit.move_to(range.start, false);
                    self.search.edit.move_to(range.end, true);
                    if remove {
                        self.search.edit.key(Key::Backspace, Mods::default());
                        self.search_query(Instant::now());
                    }
                }
            }
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

    fn search_mouse_move(&mut self, x: f32, y: f32, now: Instant) -> bool {
        match self.search.drag {
            SDrag::HThumb(grab) => {
                let sl = self.search_layout();
                self.search.scroll_x = ((x - grab - sl.htrack.x) / (sl.htrack.w - sl.hthumb.w).max(1.0)).clamp(0.0, 1.0) * sl.max_x;
                self.dirty = true;
                return true;
            }
            SDrag::ColSep { col, x: start, width } => {
                if let Some(widths) = &mut self.search.widths {
                    widths[col] = (width + (x - start) / self.scale).clamp(48.0, 2400.0);
                }
                self.dirty = true;
                return true;
            }
            SDrag::Thumb(grab) => {
                let sl = self.search_layout();
                let range = (sl.track.h - sl.thumb.h).max(1.0);
                let frac = ((y - grab - sl.track.y) / range).clamp(0.0, 1.0) as f64;
                self.search.scroll = frac * self.search_max_scroll();
                self.dirty = true;
                return true;
            }
            SDrag::Select(how) => {
                self.search.pending_click = Some((x, how));
                self.dirty = true;
                return true;
            }
            SDrag::None => {}
        }
        if self.in_title(y) {
            if self.search.hover != SHit::None {
                self.search.hover = SHit::None;
                self.dirty = true;
            }
            return false;
        }
        let h = self.search_hit(x, y);
        self.set_cursor(match h { SHit::Field => CursorIcon::Text, SHit::ColSep(_) => CursorIcon::ResizeHorizontal, _ => CursorIcon::Default });
        if h != self.search.hover {
            self.search.hover = h;
            self.tip_hover(self.search_tip_hit(), now);
            self.dirty = true;
        }
        true
    }

    pub(super) fn search_tip_hit(&self) -> Hit {
        match self.search.hover {
            SHit::Row(row) if self.search.active && self.search.drag == SDrag::None => Hit::SearchRow(row),
            _ => Hit::None,
        }
    }

    pub(super) fn search_tip_anchor(&self, row: u64) -> Option<Rect> {
        if !self.search.active {
            return None;
        }
        let sl = self.search_layout();
        let cell = sl.cell(self.search.line_of_row(row), self.search.scroll);
        let y = cell.y;
        let name = if sl.grid { cell } else { sl.cols[0] };
        let (left, right) = (name.x.max(sl.list.x), name.right().min(sl.list.right()));
        let (top, bottom) = (y.max(sl.list.y), (y + sl.row_h).min(sl.list.bottom()));
        (bottom > top && right > left).then(|| Rect::new(left, top, right - left, bottom - top))
    }

    pub(super) fn search_tip(&self, row: u64) -> Option<TipText> {
        if !self.search.active {
            return None;
        }
        let r = self.search.row(row)?;
        let tip = |line: String, details: Vec<String>| Some(TipText { line, command: None, keys: Vec::new(), details });
        if self.search.elided.contains(&row) {
            return tip(name::display(&r.name).into_owned(), Vec::new());
        }
        let arena = Bump::new();
        let labelled = |title: S, value: &str| (!value.is_empty()).then(|| format!("{}: {value}", title.t()));
        let is_dir = r.meta.attrs & attr::DIR != 0;
        let ext = if is_dir { &[][..] } else { &r.name[r.name.len() - name::ext_len(&r.name)..] };
        let line = labelled(S::DetailsColumnType, self.type_of(&arena, r.meta.attrs, ext))?;
        let mut details: Vec<String> = labelled(S::SearchColumnFolder, &name::display(&r.dir)).into_iter().collect();
        if !is_dir {
            let (mut size, mut date) = (String::new(), String::new());
            format::size_in(&mut size, r.meta.size, self.style.decimal);
            format::datetime_in(&mut date, r.meta.modified, self.utc_offset_min, &self.style);
            details.extend(labelled(S::DetailsColumnSize, &size));
            details.extend(labelled(S::DetailsColumnModified, &date));
        }
        tip(line, details)
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

    fn fit_search_column<B: TextBackend>(&mut self, g: &mut Gfx<B>, arena: &Bump, col: usize) {
        let (font, hfont) = (self.ui_font(13.0), self.ui_font(12.0));
        let pad = self.s(10.0);
        let indent = if self.search.grouped() { self.s(16.0) } else { 0.0 };
        let lead = if col == 0 { indent + self.s(ICON) + self.s(8.0) } else { 0.0 };
        let mut w = g.measure(COLUMNS[col].0.t(), hfont) + self.s(20.0);
        let (first, last) = self.search_range();
        for line in first..last {
            let SLine::Row(row) = self.search.line_at(line) else { continue };
            let Some(r) = self.search.row(row) else { continue };
            let mut text = bumpalo::collections::String::new_in(arena);
            match col {
                0 => text.push_str(&name::display(&r.name)),
                1 => text.push_str(&name::display(&r.dir)),
                2 if r.meta.attrs & attr::DIR == 0 => format::size_in(&mut text, r.meta.size, self.style.decimal),
                2 => {}
                _ => format::datetime_in(&mut text, r.meta.modified, self.utc_offset_min, &self.style),
            }
            w = w.max(lead + g.measure(&text, font));
        }
        let mut widths = self.search.widths.unwrap_or_else(|| self.search_layout().cols.map(|c| c.w / self.scale));
        widths[col] = ((w + 2.0 * pad) / self.scale).clamp(48.0, 2400.0).ceil();
        self.search.widths = Some(widths);
    }

    fn draw_result_folders<B: TextBackend>(&self, g: &mut Gfx<B>, t: &Theme, sl: &SLayout) {
        let r = sl.folders;
        if r.w <= 0.0 {
            return;
        }
        let font = self.ui_font(12.0);
        let m = g.metrics(font);
        let pad = self.s(8.0);
        let title_y = r.y + (self.layout.header.h + m.ascent - m.descent) / 2.0;
        g.dl.set_clip(Some(r));
        g.dl.rect(r, t.chrome);
        g.text(r.x + pad, title_y, S::SearchExploreFolders.t(), font, t.text, (r.w - 2.0 * pad).max(0.0));
        let scroll = self.search.folders_scroll;
        let shown = ((r.h - self.layout.header.h).max(0.0) / sl.row_h).ceil() as usize;
        for (i, (path, count)) in self.search.folders_summary.iter().enumerate().skip(scroll).take(shown) {
            let y = r.y + self.layout.header.h + (i - scroll) as f32 * sl.row_h;
            let row = Rect::new(r.x + self.s(2.0), y, (r.w - self.s(4.0)).max(0.0), sl.row_h);
            let selected = self.search.result_folder.as_ref().is_some_and(|p| p.as_bytes() == path);
            if selected || self.search.hover == SHit::ResultFolder(i) {
                g.dl.round_rect(row, if selected { t.selection_inactive } else { t.button_hover }, self.s(4.0));
            }
            let y = row.y + (row.h + m.ascent - m.descent) / 2.0;
            let count = thousands(*count);
            let cw = g.measure(&count, font);
            g.text(r.right() - pad - cw, y, &count, font, t.text_dim, cw);
            let width = (r.w - cw - 3.0 * pad).max(0.0);
            let mut label = name::display(path);
            if g.measure(&label, font) > width {
                let sep = if self.caps.unix_paths { '/' } else { '\\' };
                let mut suffixes = label.trim_end_matches(sep).rmatch_indices(sep).map(|(start, _)| &label[start..]);
                if let Some(mut tail) = suffixes.next() {
                    let available = width - g.measure("…", font);
                    for suffix in suffixes.take_while(|suffix| g.measure(suffix, font) <= available) {
                        tail = suffix;
                    }
                    label = format!("…{tail}").into();
                }
            }
            g.text(r.x + pad, y, &label, font, t.text, width);
        }
        g.dl.set_clip(None);
    }

    fn draw_facets<B: TextBackend>(&self, g: &mut Gfx<B>, t: &Theme, sl: &SLayout) {
        if sl.facets.h <= 0.0 {
            return;
        }
        let font = self.ui_font(12.0);
        let m = g.metrics(font);
        let pad = self.s(6.0);
        let row = sl.facets;
        let timeline = sl.timeline;
        let captions = [S::DetailsColumnType, S::SearchExploreYear, S::DetailsColumnSize, S::DetailsColumnTag];
        g.dl.rect(row, t.chrome);
        g.dl.rect(timeline, t.chrome);
        for (i, label) in captions.into_iter().enumerate() {
            let r = Rect::new(row.x + i as f32 * row.w / 4.0, row.y, row.w / 4.0, row.h);
            let active = self.search.facet_filter[i].as_deref();
            if active.is_some() || self.search.hover == SHit::Facet(i) {
                g.dl.round_rect(r.inset(self.s(1.0), self.s(2.0)), if active.is_some() { t.selection_inactive } else { t.button_hover }, self.s(4.0));
            }
            let y = r.y + (r.h + m.ascent - m.descent) / 2.0;
            let text = active.map_or_else(|| label.t().to_owned(), |a| format!("{} · {a}", label.t()));
            g.text(r.x + pad, y, &text, font, t.text, (r.w - 2.0 * pad).max(0.0));
        }
        let dates = &self.search.facets.dates;
        if dates.is_empty() {
            return;
        }
        let total = dates.iter().map(|d| d.count).max().unwrap_or(1).max(1) as f32;
        let w = timeline.w / dates.len() as f32;
        for (i, b) in dates.iter().enumerate() {
            let h = (timeline.h - self.s(4.0)) * (b.count as f32 / total);
            let x = timeline.x + i as f32 * w;
            let dragging = matches!((self.search.pressed, self.search.hover), (SHit::Date(a), SHit::Date(z)) if (a.min(z)..=a.max(z)).contains(&i));
            let chosen = self.search.date_filter.is_some_and(|(a, z)| b.from <= z && b.to >= a);
            if dragging || chosen || self.search.hover == SHit::Date(i) {
                g.dl.rect(Rect::new(x, timeline.y, w, timeline.h), t.selection_inactive);
            }
            if h > 0.0 {
                g.dl.rect(Rect::new(x, timeline.bottom() - h, (w - self.s(2.0)).max(1.0), h), t.accent.alpha(0xB0));
            }
        }
        let range = if let SHit::Date(i) = self.search.hover {
            dates.get(i).map(|b| (b.from, b.to))
        } else {
            self.search.date_filter
        };
        if let Some((a, z)) = range {
            let text = if a == z { day_query(a) } else { format!("{} – {}", day_query(a), day_query(z)) };
            let w = g.measure(&text, font) + 2.0 * pad;
            let r = Rect::new(timeline.right() - w, timeline.y, w, timeline.h);
            g.dl.round_rect(r, t.chrome, self.s(4.0));
            g.text(r.x + pad, r.y + (r.h + m.ascent - m.descent) / 2.0, &text, font, t.text, w);
        }
    }

    fn draw_similar_chips<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme, r: Rect) {
        self.search.similar_chips.clear();
        if r.h <= 0.0 {
            return;
        }
        let font = self.ui_font(12.0);
        let metrics = g.metrics(font);
        let (pad, close) = (self.s(7.0), self.s(20.0));
        let mut x = r.x + pad;
        g.dl.set_clip(Some(r));
        g.dl.rect(r, t.chrome);
        for range in crate::filter::token_ranges(&self.search.edit.text) {
            let label = &self.search.edit.text[range.clone()];
            let remaining = r.right() - x - pad;
            if remaining < self.s(50.0) {
                break;
            }
            let w = (g.measure(label, font) + 2.0 * pad + close).min(remaining);
            let chip = Rect::new(x, r.y + self.s(3.0), w, (r.h - self.s(6.0)).max(0.0));
            let i = self.search.similar_chips.len();
            let hot = matches!(self.search.hover, SHit::SimilarChip(k, _) if k == i);
            g.dl.round_rect(chip, if hot { t.button_hover } else { t.selection_inactive }, self.s(4.0));
            let y = chip.y + (chip.h + metrics.ascent - metrics.descent) / 2.0;
            g.text(chip.x + pad, y, label, font, t.text, (w - 2.0 * pad - close).max(0.0));
            centered(g, Rect::new(chip.right() - close, chip.y, close, chip.h), super::G_CLOSE, self.icon_font(8.0), t.text_dim);
            self.search.similar_chips.push((range, chip));
            x = chip.right() + pad;
        }
        g.dl.set_clip(None);
    }

    pub(super) fn draw_search<B: TextBackend>(&mut self, g: &mut Gfx<B>, arena: &Bump, t: &Theme) {
        if let Some(col) = self.search.fit.take() {
            self.fit_search_column(g, arena, col);
        }
        self.search.scroll_x = self.search.scroll_x.clamp(0.0, self.search_layout().max_x);
        self.search.scroll = self.search.scroll.clamp(0.0, self.search_max_scroll());
        self.request_visible_pages();
        let sl = self.search_layout();
        self.draw_similar_chips(g, t, sl.chips);
        self.draw_facets(g, t, &sl);
        let font = self.ui_font(13.0);
        let m = g.metrics(font);
        let rh = sl.row_h;
        let text_dy = ((rh + m.ascent - m.descent) / 2.0).round();
        let pad = self.s(10.0);
        let icon_px = self.s(if sl.grid { 42.0 } else { ICON });
        let focused = self.active;
        let (first, last) = self.search_range();
        g.dl.set_clip(Some(sl.list));
        let mut requests: Vec<IconRequest> = Vec::new();
        let mut elided: Vec<u64> = Vec::new();
        let mut type_requests: Vec<Vec<u8>> = Vec::new();
        let s = &self.search;
        self.draw_result_folders(g, t, &sl);
        g.dl.set_clip(Some(sl.list));
        let headers = s.grouped();
        if s.done && s.total == 0 && !s.walking {
            let msg = if s.error.is_some() { S::SearchInvalid.t() } else { S::SearchNoMatch.t() };
            let w = g.measure(msg, font);
            g.text(sl.list.x + (sl.list.w - w) / 2.0, sl.list.y + self.s(40.0), msg, font, t.text_dim, f32::INFINITY);
        }
        for line in first..last {
            let cell = sl.cell(line, s.scroll);
            let y = cell.y;
            let row = match s.line_at(line) {
                SLine::Row(r) => r,
                SLine::Header(k) => {
                    self.draw_search_group(g, arena, t, k, Rect::new(sl.list.x, y, sl.list.w, rh), &mut requests);
                    continue;
                }
            };
            let rr = Rect::new(cell.x + self.s(4.0), y + 1.0, cell.w - self.s(8.0), rh - 2.0);
            if s.focus == Some(row) {
                g.dl.round_rect(rr, if focused { t.selection } else { t.selection_inactive }, self.s(4.0));
            } else if s.hover == SHit::Row(row) && s.drag == SDrag::None {
                g.dl.round_rect(rr, t.hover, self.s(4.0));
            }
            let Some(r) = s.row(row) else { continue };
            let is_dir = r.meta.attrs & attr::DIR != 0;
            let ext_at = if is_dir { r.name.len() } else { r.name.len() - name::ext_len(&r.name) };
            let ext = &r.name[ext_at..];
            let folded = ext.to_ascii_lowercase();
            if !sl.grid && !is_dir && !self.type_names.contains_key(&folded) && !type_requests.contains(&folded) {
                type_requests.push(folded);
            }
            let entry = Entry { size: r.meta.size, modified: r.meta.modified, created: r.meta.created, accessed: 0, name_off: 0, attrs: r.meta.attrs, name_len: 0, ext_len: 0, tags: 0 };
            let (key, kind) = icon_key(&r.name, ext, &entry, icon_px as u32, &r.dir, self.icon_epoch);
            self.file_icons.see(key, kind);
            let indent = if headers { self.s(16.0) } else { 0.0 };
            let ir = if sl.grid {
                Rect::new((cell.x + (cell.w - icon_px) / 2.0).round(), y + self.s(14.0), icon_px, icon_px)
            } else {
                Rect::new(sl.cols[0].x + pad + indent, y + ((rh - icon_px) / 2.0).round(), icon_px, icon_px)
            };
            if !g.image(key, ir) {
                if !self.folder_stand_in(g, kind, icon_px as u32, ir, 255, &mut requests) {
                    let (glyph, color) = if is_dir { (G_FOLDER, t.accent.alpha(0xB0)) } else { (G_FILE, t.text_dim.alpha(0x90)) };
                    centered(g, ir, glyph, FontStyle::new(FontFamily::Icons, icon_px * 0.85, false), color);
                }
                let wanted = !self.icons.contains_key(&key) && !requests.iter().any(|q| q.key == key);
                if let Some(path) = wanted.then(|| Location::parse_exact(&r.dir)).flatten().and_then(|d| d.join(&r.name).to_windows()) {
                    requests.push(IconRequest { key, kind, path, size: icon_px as u32, gen: 0, row: 0 });
                }
            }
            self.draw_shell_overlay(g, kind, &entry, &r.dir, &r.name, ir, 255);
            self.draw_file_emblems(g, entry.attrs, ir, 255, &mut requests);
            let color = if r.meta.attrs & attr::HIDDEN != 0 { t.text_dim } else { t.text };
            if sl.grid {
                let shown = name::display(&r.name);
                let room = (cell.w - pad * 2.0).max(0.0);
                let width = g.measure(&shown, font);
                if width > room {
                    elided.push(row);
                }
                g.text(cell.x + (cell.w - width.min(room)) / 2.0, ir.bottom() + self.s(6.0) + m.ascent, &shown, font, color, room);
                continue;
            }
            let ty = y + text_dy;
            let name_x = ir.right() + self.s(8.0);
            let (shown, room) = (name::display(&r.name), sl.cols[0].right() - name_x - pad);
            if g.measure(&shown, font) > room {
                elided.push(row);
            }
            g.text(name_x, ty, &shown, font, color, room);
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
        self.search.elided = elided;
        self.request_icons(requests);
        for ext in type_requests {
            self.type_names.insert(ext.clone(), None);
            self.effects.push(Effect::TypeName(ext));
        }
        let s = &self.search;
        if sl.thumb.h > 0.0 {
            let hot = matches!(s.hover, SHit::Thumb | SHit::Track) || matches!(s.drag, SDrag::Thumb(_));
            g.dl.round_rect(sl.thumb, if hot { t.scrollbar_hover } else { t.scrollbar }, sl.thumb.w / 2.0);
        }
        if sl.max_x > 0.0 {
            g.dl.rect(sl.htrack, t.bg);
            g.dl.round_rect(sl.hthumb, if matches!(s.hover, SHit::HThumb) || matches!(s.drag, SDrag::HThumb(_)) { t.scrollbar_hover } else { t.scrollbar }, sl.hthumb.h / 2.0);
        }
        let head_from = g.dl.instances.len();
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
            let w = g.text(r.x + pad, baseline, title.t(), hfont, t.text_dim, r.w - 2.0 * pad);
            if s.sort.key == *key {
                let glyph = if s.sort.descending { G_SORT_DOWN } else { G_SORT_UP };
                g.text(r.x + pad + w + self.s(6.0), baseline, glyph, self.icon_font(8.0), t.text_dim, f32::INFINITY);
            }
            g.dl.set_clip(None);
        }
        g.dl.confine(head_from, sl.header);
    }

    fn draw_search_group<B: TextBackend>(&self, g: &mut Gfx<B>, arena: &Bump, t: &Theme, k: usize, r: Rect, requests: &mut Vec<IconRequest>) {
        let s = &self.search;
        let font = FontStyle::new(FontFamily::Ui, 12.5 * self.scale, true);
        let m = g.metrics(font);
        let baseline = (r.y + (r.h + m.ascent - m.descent) / 2.0).round();
        if s.hover == SHit::Group(k) {
            g.dl.round_rect(r.inset(self.s(4.0), 1.0), t.hover, self.s(4.0));
        }
        let icon = Rect::new(r.x + self.s(10.0), r.y, self.s(16.0), r.h);
        let folder = s.row(s.groups[k]).and_then(|row| Location::parse_exact(&row.dir));
        let system = folder.and_then(|loc| folder_icon(&loc, &self.nav.sidebar, icon.w as u32));
        if !self.system_icon(g, system, icon, icon.w, 0xFF, requests) {
            centered(g, icon, G_FOLDER, self.icon_font(11.0), t.accent.alpha(0xC0));
        }
        let dir = s.row(s.groups[k]).map(|row| name::display(&row.dir).into_owned()).unwrap_or_else(|| "…".into());
        let x = icon.right() + self.s(8.0);
        let mut count = bumpalo::collections::String::new_in(arena);
        let n = s.group_len(k);
        trn_into!(&mut count, P::DetailsCountItems, n);
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
        let tools = self.search_layout().tools;
        for (i, (hit, icon, selected)) in [(SHit::Folders, "\u{E8B7}", self.search.folders_open), (SHit::WithinResults, G_SEARCH, self.search.within_results.is_some()), (SHit::Facets, G_FACETS, self.search.facets_open)].into_iter().enumerate() {
            let r = tools[i];
            if selected || self.search.hover == hit {
                g.dl.round_rect(r.inset(self.s(1.0), self.s(1.0)), if selected { t.selection_inactive } else { t.button_hover }, self.s(4.0));
            }
            let color = if hit == SHit::Facets && self.search.facets_open && self.search.has_facet_filters() { t.accent } else { t.text };
            centered(g, r, icon, self.icon_font(12.0), color);
        }

        g.dl.rect(status, t.chrome);
        let font = self.ui_font(12.0);
        let m = g.metrics(font);
        let baseline = (status.y + (status.h + m.ascent - m.descent) / 2.0).round();
        let pad = self.s(12.0);
        let s = &self.search;
        let mut left = bumpalo::collections::String::new_in(arena);
        let mut color = t.text_dim;
        if let Some(e) = &s.error {
            left.push_str(&crate::i18n::localize(e));
            color = t.error;
        } else if s.walking {
            if s.folders > 0 {
                trn_into!(&mut left, P::SearchWalkingIn, s.folders, results = trn!(P::SearchResults, s.total));
            } else {
                trn_into!(&mut left, P::SearchWalking, s.total);
            }
        } else if s.done {
            let results = trn!(P::SearchResults, s.total);
            match s.elapsed {
                Some(e) => {
                    let mut ms = String::new();
                    i18n::decimal(&mut ms, e.as_secs_f64() * 1e3, 1);
                    tr_into!(&mut left, S::SearchDoneIn, results = results, ms = ms);
                }
                None => left.push_str(&results),
            }
            if s.folders > 0 {
                left.push_str(" · ");
                trn_into!(&mut left, P::SearchFolders, s.folders);
            }
        } else {
            left.push_str(S::SearchSearching.t());
        }
        if let Some(label) = match s.hover {
            SHit::Folders => Some(S::SearchExploreFolders),
            SHit::WithinResults => Some(S::SearchExploreWithin),
            SHit::Facets => Some(S::SearchExploreFacets),
            _ => None,
        } {
            left.clear();
            left.push_str(label.t());
            color = t.text_dim;
        }
        g.text(status.x + pad, baseline, &left, font, color, status.w / 2.0 - pad);
        let mut right = bumpalo::collections::String::new_in(arena);
        let tagged = crate::tags::has_tag_term(&s.edit.text);
        right.push_str(match (s.source, s.backend) {
            (Some(SearchSource::Walk), _) => S::SearchSourceWalk,
            (Some(SearchSource::Shell), _) if self.caps.unix_paths => S::SearchSourceContent,
            (Some(SearchSource::Shell), _) => S::SearchSourceShell,
            _ if tagged => S::SearchSourceTagged,
            (None, SearchBackend::Connecting) => S::SearchSourceConnecting,
            (None, SearchBackend::Service) => S::SearchSourceService,
            (None, SearchBackend::Fallback) if self.caps.unix_paths => S::SearchSourceLocal,
            (None, SearchBackend::Fallback) => S::SearchSourceFallback,
            (None, SearchBackend::Off) => S::SearchSourceOff,
        }.t());
        for v in s.volumes.iter().filter(|_| !tagged && s.source.is_none()) {
            let label = if v.id.is_empty() { S::SearchIndexDefault.t() } else { &v.id };
            right.push_str(" · ");
            match &v.state {
                IndexState::Ready => trn_into!(&mut right, P::SearchIndexReady, v.files, label = label),
                IndexState::Scanning(p) => trn_into!(&mut right, P::SearchIndexScanning, v.files, label = label, percent = format!("{:.0}", p * 100.0)),
                IndexState::Error(e) => tr_into!(&mut right, S::SearchIndexError, label = label, error = e),
            }
        }
        let rcolor = if (matches!(s.backend, SearchBackend::Fallback) && !self.caps.unix_paths || matches!(s.backend, SearchBackend::Off)) && !tagged && s.source.is_none() { t.error } else { t.text_dim };
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
            Some(w) => tr!(S::SearchIn, name = w.name()),
            None if self.search.home.is_some() => S::SearchAllDrives.t().to_owned(),
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
        if let Some((x, how)) = s.pending_click.take() {
            let pos = g.hit(&s.edit.text, font, x - inner.x + s.edit_scroll);
            s.edit.grab(pos, how);
        }
        let caret_h = (r.h * 0.6).round();
        let look = super::field::EditLook { font, baseline, line_y: r.y + (r.h - caret_h) / 2.0, line_h: caret_h, stroke: 1.0f32.max(self.scale.round()), focused: true };
        let preedit = Some(&self.preedit).filter(|p| !p.is_empty());
        if s.edit.text.is_empty() && preedit.is_none() {
            s.edit_scroll = 0.0;
            g.text(inner.x, baseline, if s.within_results.is_some() { S::SearchExploreWithin } else if self.caps.unix_paths { S::SearchPlaceholderUnix } else { S::SearchPlaceholder }.t(), font, t.text_dim.alpha(0xA0), inner.w);
            g.dl.rect(Rect::new(inner.x, look.line_y, look.stroke, caret_h), t.text);
        } else {
            let line = super::field::EditLine { text: &s.edit.text, caret: s.edit.caret, selection: s.edit.selection() };
            super::field::draw_line(g, t, inner, &mut s.edit_scroll, line, preedit, look);
        }
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
        super::widgets::Paint::new(g, t, self.scale).frame(panel);
        let small = self.ui_font(12.0);
        let head = Rect::new(x + s6, y + s6, w - 2.0 * s6, row_h);
        g.text(head.x + self.s(10.0), (head.y + (head.h + m.ascent - m.descent) / 2.0).round(), S::SearchRecent.t(), small, t.text_dim, head.w);
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
