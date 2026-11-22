mod interaction;
mod results_view;

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
    pub(super) exclusions: Vec<Location>,
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
            exclude: s.exclusions.iter().filter_map(Location::to_windows).collect(),
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
        if self.search.exclusions != unique {
            self.search.exclusions = unique;
            if self.search.active {
                self.search_query(Instant::now());
            }
        }
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
}

#[cfg(test)]
#[path = "../../test/ui/search.rs"]
mod tests;
