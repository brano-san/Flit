use super::layouts::{LayoutMode, Sizes};
use super::ops::TIMER_SLACK;
use super::sidebar::PlaceCmd;
use super::{centered, App, Column, Drag, Effect, Hit, MenuItem, ShellMenu, Tab, BIN_COLUMNS, COLUMNS, COMPUTER_COLUMNS, REPO_COLUMNS, G_CHEVRON, G_FOLDER, G_SORT_DOWN};
use super::menu::ColumnCmd;
use crate::fmt as format;
use crate::hash::{FxHashMap, FxHasher};
use crate::i18n::S;
use crate::input::{CursorIcon, MouseButton};
use crate::listing::{attr, FsError, FsErrorKind, Listing};
use crate::location::Location;
use crate::path::PathKind;
use crate::name;
use crate::provider::TabId;
use crate::recycle;
use crate::render::Rect;
use crate::sort::{self, Folders, GroupField, GroupSpec, SortField, SortSpec};
use crate::text::{FontFamily, FontStyle, Gfx, TextBackend};
use crate::theme::Theme;
use crate::view::{DirView, Group, Line};
use bumpalo::Bump;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::fmt::Write as _;
use std::hash::Hasher;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

pub const FOLDER_VIEWS_CAP: usize = 4096;
pub const MIN_COL_W: f32 = 40.0;
pub const MAX_COL_W: f32 = 600.0;
const HOVER_SELECT: Duration = Duration::from_millis(600);
const AUTOSCROLL_DEAD: f32 = 12.0;
const AUTOSCROLL_RATE: f32 = 8.0;
pub(super) const CHECK: f32 = 14.0;
const CHECK_IN: Duration = Duration::from_millis(140);
pub const THUMB_CACHE_MB: (u32, u32, u32) = (100, 500, 5000);
const SIZE_GROUPS: [S; 7] = [
    S::DetailsSizeGroupGigantic,
    S::DetailsSizeGroupHuge,
    S::DetailsSizeGroupLarge,
    S::DetailsSizeGroupMedium,
    S::DetailsSizeGroupSmall,
    S::DetailsSizeGroupTiny,
    S::DetailsSizeGroupEmpty,
];
const G_WARNING: &str = "\u{E7BA}";

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Columns {
    pub hidden: u8,
    pub widths: [f32; 7],
    #[serde(skip)]
    pub git: u8,
    #[serde(skip, default = "git_widths")]
    pub git_widths: [f32; 4],
    #[serde(skip, default = "extra_shown")]
    pub extra: u8,
    #[serde(skip, default = "extra_widths")]
    pub extra_widths: [f32; EXTRA],
    #[serde(skip, default = "column_order")]
    pub order: [u8; ORDERED],
}

pub const GIT_WIDTHS: [f32; 4] = [100.0, 260.0, 140.0, 150.0];
pub const EXTRA: usize = 8;
pub const ORDERED: usize = 16;
const FIRST_EXTRA: u8 = Column::Folders as u8;
pub const EXTRA_WIDTHS: [f32; EXTRA] = [90.0, 150.0, 110.0, 110.0, 110.0, 90.0, 110.0, 90.0];
const DEFAULT_HIDDEN: u8 = 1 << Column::Created as u8;
const DEFAULT_EXTRA: u8 = 1 << (Column::Folders as u8 - FIRST_EXTRA) | 1 << (Column::Free as u8 - FIRST_EXTRA) | 1 << (Column::FileSystem as u8 - FIRST_EXTRA) | 1 << (Column::Percent as u8 - FIRST_EXTRA) | 1 << (Column::Files as u8 - FIRST_EXTRA);
const DEFAULT_WIDTHS: [f32; 7] = [0.0, 150.0, 140.0, 90.0, 220.0, 150.0, 150.0];
pub const DEFAULT_ORDER: [Column; ORDERED] = [
    Column::Name,
    Column::Origin,
    Column::Deleted,
    Column::Type,
    Column::Modified,
    Column::Folders,
    Column::Files,
    Column::Size,
    Column::Volume,
    Column::Created,
    Column::Accessed,
    Column::FolderUsage,
    Column::Tag,
    Column::Free,
    Column::Percent,
    Column::FileSystem,
];
const ALL_COLUMNS: [Column; ORDERED] = [
    Column::Name,
    Column::Modified,
    Column::Type,
    Column::Size,
    Column::Origin,
    Column::Deleted,
    Column::Created,
    Column::Tag,
    Column::Folders,
    Column::Accessed,
    Column::Volume,
    Column::FolderUsage,
    Column::Free,
    Column::FileSystem,
    Column::Percent,
    Column::Files,
];

fn git_widths() -> [f32; 4] {
    GIT_WIDTHS
}

fn extra_shown() -> u8 {
    DEFAULT_EXTRA
}

fn extra_widths() -> [f32; EXTRA] {
    EXTRA_WIDTHS
}

fn column_order() -> [u8; ORDERED] {
    DEFAULT_ORDER.map(|c| c as u8)
}

impl Default for Columns {
    fn default() -> Self {
        Self { hidden: DEFAULT_HIDDEN, widths: DEFAULT_WIDTHS, git: 0, git_widths: GIT_WIDTHS, extra: DEFAULT_EXTRA, extra_widths: EXTRA_WIDTHS, order: column_order() }
    }
}

fn git_slot(c: Column) -> usize {
    c as usize - Column::GitStatus as usize
}

fn extra_slot(c: Column) -> Option<usize> {
    (c as u8 >= FIRST_EXTRA && !c.is_git()).then(|| (c as u8 - FIRST_EXTRA) as usize)
}

impl Columns {
    pub fn shown(&self, c: Column) -> bool {
        if c.is_git() {
            return (self.git >> git_slot(c)) & 1 == 1;
        }
        if let Some(k) = extra_slot(c) {
            return (self.extra >> k) & 1 == 1;
        }
        c == Column::Name || ((self.hidden >> c as u8) & 1 == 1) == (c == Column::Tag)
    }

    pub fn width(&self, c: Column) -> f32 {
        if c.is_git() {
            let w = self.git_widths[git_slot(c)];
            return if (MIN_COL_W..=MAX_COL_W).contains(&w) { w } else { GIT_WIDTHS[git_slot(c)] };
        }
        if let Some(k) = extra_slot(c) {
            return self.extra_widths[k];
        }
        self.widths[c.slot()]
    }

    pub fn set_width(&mut self, c: Column, w: f32) {
        match (c.is_git(), extra_slot(c)) {
            (true, _) => self.git_widths[git_slot(c)] = w,
            (false, Some(k)) => self.extra_widths[k] = w,
            (false, None) => self.widths[c.slot()] = w,
        }
    }

    pub fn toggle(&mut self, c: Column) {
        match (c, extra_slot(c)) {
            (Column::Name, _) => {}
            (c, _) if c.is_git() => self.git ^= 1 << git_slot(c),
            (_, Some(k)) => self.extra ^= 1 << k,
            (c, None) => self.hidden ^= 1 << c as u8,
        }
    }

    pub fn ordered(&self, kinds: &[Column]) -> Vec<Column> {
        let mut out: Vec<Column> = self.order.iter().filter_map(|&id| ALL_COLUMNS.iter().copied().find(|&c| c as u8 == id)).filter(|c| kinds.contains(c)).collect();
        let missing: Vec<Column> = kinds.iter().copied().filter(|c| !out.contains(c)).collect();
        out.extend(missing);
        if let Some(k) = out.iter().position(|&c| c == Column::Name) {
            out[..=k].rotate_right(1);
        }
        out
    }

    pub fn shift(&mut self, c: Column, kinds: &[Column], right: bool) {
        let shown: Vec<Column> = self.ordered(kinds).into_iter().filter(|&x| self.shown(x)).collect();
        let Some(k) = shown.iter().position(|&x| x == c) else { return };
        let other = match right {
            true => shown.get(k + 1),
            false => k.checked_sub(1).and_then(|j| shown.get(j)),
        };
        let Some(&other) = other.filter(|&&o| o != Column::Name && c != Column::Name) else { return };
        let (a, b) = (self.order.iter().position(|&id| id == c as u8), self.order.iter().position(|&id| id == other as u8));
        if let (Some(a), Some(b)) = (a, b) {
            self.order.swap(a, b);
        }
    }

    pub fn is_default(&self) -> bool {
        let d = Self::default();
        self.hidden == d.hidden && self.extra == d.extra && self.order == d.order && self.widths[1..] == d.widths[1..] && self.extra_widths == d.extra_widths
    }

    pub fn restored(self) -> Self {
        Self { git: self.git, git_widths: self.git_widths, ..Self::default() }
    }

    pub fn sanitized(mut self) -> Self {
        let d = Self::default();
        for (w, dw) in self.widths.iter_mut().zip(d.widths).chain(self.extra_widths.iter_mut().zip(d.extra_widths)) {
            if !(w.is_finite() && (MIN_COL_W..=MAX_COL_W).contains(w)) {
                *w = dw;
            }
        }
        self.hidden &= 0xFE;
        let mut seen = self.order;
        seen.sort_unstable();
        if seen != ALL_COLUMNS.map(|c| c as u8) {
            self.order = d.order;
        }
        self
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FolderView {
    pub layout: LayoutMode,
    pub sort: SortSpec,
    pub group: GroupSpec,
    pub columns: Columns,
    #[serde(skip)]
    pub sizes: Sizes,
    #[serde(skip)]
    pub zoom: u8,
    #[serde(skip)]
    pub subfolders: bool,
}

impl FolderView {
    pub fn sanitized(mut self) -> Self {
        self.columns = self.columns.sanitized();
        self.sizes = self.sizes.sanitized();
        self.zoom = super::layouts::zoom_of(self.layout, self.zoom);
        self
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ViewPrefs {
    pub same_for_all: bool,
    pub defaults: FolderView,
    pub auto_fit: bool,
    pub show_dotfiles: bool,
    pub show_system: bool,
    pub show_extensions: bool,
    pub auto_sizes: bool,
    pub status_bar: bool,
    pub split_count: bool,
    pub checkboxes: bool,
    pub hover_select: bool,
    #[serde(skip, default = "yes")]
    pub command_descriptions: bool,
    #[serde(skip, default = "yes")]
    pub thumbnails: bool,
    #[serde(skip, default = "thumb_cache_mb")]
    pub thumb_cache_mb: u32,
    #[serde(skip)]
    pub show_streams: bool,
    #[serde(skip)]
    pub retain_filter: bool,
}

fn yes() -> bool {
    true
}

fn thumb_cache_mb() -> u32 {
    THUMB_CACHE_MB.1
}

impl ViewPrefs {
    pub fn sanitized(mut self) -> Self {
        self.defaults = self.defaults.sanitized();
        self.thumb_cache_mb = self.thumb_cache_mb.clamp(THUMB_CACHE_MB.0, THUMB_CACHE_MB.2);
        self
    }
}

impl Default for ViewPrefs {
    fn default() -> Self {
        Self {
            same_for_all: false,
            defaults: FolderView::default(),
            auto_fit: false,
            show_dotfiles: true,
            show_system: false,
            show_extensions: true,
            auto_sizes: false,
            status_bar: true,
            split_count: false,
            checkboxes: false,
            hover_select: false,
            command_descriptions: true,
            thumbnails: true,
            thumb_cache_mb: THUMB_CACHE_MB.1,
            show_streams: false,
            retain_filter: false,
        }
    }
}

#[derive(Default)]
struct Lru {
    map: FxHashMap<u64, (u64, FolderView)>,
    deep: Vec<u64>,
    clock: u64,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ViewMode {
    None,
    Folder,
    Subfolders,
}

pub(super) fn view_mode_glyph(mode: ViewMode) -> &'static str {
    match mode {
        ViewMode::None => "\u{E8A9}",
        ViewMode::Folder => G_FOLDER,
        ViewMode::Subfolders => "\u{E8F4}",
    }
}

#[derive(Clone, Default)]
pub struct FolderViews(Arc<Mutex<Lru>>);

impl FolderViews {
    fn lock(&self) -> MutexGuard<'_, Lru> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn get(&self, key: u64) -> Option<FolderView> {
        let mut l = self.lock();
        l.clock += 1;
        let now = l.clock;
        l.map.get_mut(&key).map(|e| {
            e.0 = now;
            e.1
        })
    }

    pub fn put(&self, key: u64, v: FolderView) {
        let mut l = self.lock();
        l.clock += 1;
        let now = l.clock;
        l.map.insert(key, (now, v));
        if l.map.len() > FOLDER_VIEWS_CAP {
            let mut stamps: Vec<u64> = l.map.values().map(|e| e.0).collect();
            let cut = stamps.len() - FOLDER_VIEWS_CAP * 3 / 4;
            let keep = *stamps.select_nth_unstable(cut).1;
            l.map.retain(|_, e| e.0 >= keep);
            let Lru { map, deep, .. } = &mut *l;
            deep.retain(|k| map.contains_key(k));
        }
    }

    pub fn mode(&self, key: u64) -> ViewMode {
        let l = self.lock();
        match (l.map.contains_key(&key), l.deep.contains(&key)) {
            (false, _) => ViewMode::None,
            (true, false) => ViewMode::Folder,
            (true, true) => ViewMode::Subfolders,
        }
    }

    pub fn set_mode(&self, key: u64, mode: ViewMode, v: FolderView) {
        if mode == ViewMode::None {
            let mut l = self.lock();
            l.map.remove(&key);
            l.deep.retain(|&k| k != key);
            return;
        }
        if !self.lock().map.contains_key(&key) {
            self.put(key, v);
        }
        let mut l = self.lock();
        l.deep.retain(|&k| k != key);
        if mode == ViewMode::Subfolders {
            l.deep.push(key);
        }
    }

    pub fn owner(&self, keys: &[u64]) -> Option<u64> {
        let l = self.lock();
        let (first, rest) = keys.split_first()?;
        if l.map.contains_key(first) {
            return Some(*first);
        }
        rest.iter().copied().find(|k| l.deep.contains(k) && l.map.contains_key(k))
    }

    pub fn deep_keys(&self) -> Vec<u64> {
        self.lock().deep.clone()
    }

    pub fn set_deep(&self, keys: Vec<u64>) {
        let mut l = self.lock();
        let Lru { map, deep, .. } = &mut *l;
        *deep = keys.into_iter().filter(|k| map.contains_key(k)).collect();
    }

    pub fn clear(&self) {
        let mut l = self.lock();
        l.map.clear();
        l.deep.clear();
    }

    pub fn len(&self) -> usize {
        self.lock().map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn entries(&self) -> Vec<(u64, FolderView)> {
        let l = self.lock();
        let mut v: Vec<(u64, u64, FolderView)> = l.map.iter().map(|(&k, &(at, f))| (at, k, f)).collect();
        v.sort_unstable_by_key(|e| e.0);
        v.into_iter().map(|(_, k, f)| (k, f)).collect()
    }

    pub fn from_entries(entries: Vec<(u64, FolderView)>) -> Self {
        let mut l = Lru::default();
        let skip = entries.len().saturating_sub(FOLDER_VIEWS_CAP);
        for (k, f) in entries.into_iter().skip(skip) {
            l.clock += 1;
            l.map.insert(k, (l.clock, f.sanitized()));
        }
        Self(Arc::new(Mutex::new(l)))
    }
}

impl PartialEq for FolderViews {
    fn eq(&self, o: &Self) -> bool {
        Arc::ptr_eq(&self.0, &o.0) || self.entries() == o.entries()
    }
}

impl std::fmt::Debug for FolderViews {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "FolderViews({})", self.len())
    }
}

fn lineage(loc: &Location) -> Vec<u64> {
    std::iter::successors(Some(loc.clone()), Location::parent).map(|l| folder_key(&l)).collect()
}

pub fn folder_key(loc: &Location) -> u64 {
    let mut h = FxHasher::default();
    match loc {
        Location::Local(p) if !p.case_sensitive() => p.as_bytes().iter().for_each(|b| h.write_u8(b.to_ascii_lowercase())),
        _ => loc.to_bytes().iter().for_each(|&b| h.write_u8(b)),
    }
    h.finish()
}

struct Pick {
    tab: TabId,
    idx: u32,
    at: Option<Instant>,
}

struct Autoscroll {
    anchor: f32,
    y: f32,
    moved: bool,
}

#[derive(Default)]
pub(super) struct Checks {
    pub hover: Option<usize>,
    seen: Vec<(u32, bool)>,
    fresh: Vec<(u32, Instant)>,
}

impl Checks {
    pub fn progress(&self, i: u32, now: Instant) -> f32 {
        self.fresh.iter().find(|f| f.0 == i).map_or(1.0, |f| now.saturating_duration_since(f.1).as_secs_f32() / CHECK_IN.as_secs_f32())
    }

    pub fn settle(&mut self, mut drawn: Vec<(u32, bool)>, animate: bool, now: Instant) -> bool {
        drawn.sort_unstable();
        if animate {
            for &(i, shown) in &drawn {
                let was_hidden = self.seen.binary_search_by_key(&i, |s| s.0).is_ok_and(|k| !self.seen[k].1);
                if shown && was_hidden && !self.fresh.iter().any(|f| f.0 == i) {
                    self.fresh.push((i, now));
                }
            }
        }
        self.fresh.retain(|f| animate && now.saturating_duration_since(f.1) < CHECK_IN && drawn.binary_search(&(f.0, true)).is_ok());
        self.seen = drawn;
        !self.fresh.is_empty()
    }
}

pub(super) fn shows_check(enabled: bool, selected: bool, icon_hovered: bool) -> bool {
    enabled && (selected || icon_hovered)
}

#[derive(Default)]
pub(super) struct Details {
    pub checks: Checks,
    pick: Option<Pick>,
    autoscroll: Option<Autoscroll>,
    swallow: Option<MouseButton>,
    pub middle_at: Option<(f32, f32)>,
    pub middle_on_empty: bool,
}

impl Column {
    pub(super) fn slot(self) -> usize {
        if self == Column::Tag { Column::Deleted as usize } else { self as usize }
    }
}

impl Tab {
    pub(super) fn folder_view(&self) -> FolderView {
        FolderView { layout: self.mode, sort: self.view_spec.sort, group: self.view_spec.group, columns: self.columns, sizes: self.sizes, zoom: self.zoom, subfolders: self.subfolders }
    }

    pub(super) fn set_folder_view(&mut self, f: FolderView) {
        self.mode = f.layout;
        self.sizes = f.sizes;
        self.zoom = super::layouts::zoom_of(f.layout, f.zoom);
        self.subfolders = f.subfolders;
        self.view_spec.sort = f.sort;
        self.view_spec.group = f.group;
        self.columns = f.columns;
    }
}

const DATE_GROUPS: [S; 6] = [S::DetailsGroupToday, S::DetailsGroupYesterday, S::DetailsGroupLastWeek, S::DetailsGroupLastMonth, S::DetailsGroupLastYear, S::DetailsGroupLongAgo];
const NAME_GROUPS: [S; 4] = [S::DetailsGroupNameAh, S::DetailsGroupNameIp, S::DetailsGroupNameQz, S::DetailsGroupOther];
const ITEM_GROUPS: [S; 5] = [S::DetailsGroupThousands, S::DetailsGroupHundreds, S::DetailsGroupDozens, S::DetailsGroupFew, S::DetailsGroupNoItems];

fn bucket_label(s: &mut impl std::fmt::Write, table: &[S], bucket: u64) {
    let _ = s.write_str(table.get(bucket as usize).map_or(S::DetailsGroupUnspecified, |&k| k).t());
}

const NO_ITEMS: &str = "--";
const G_CALENDAR: &str = "\u{E787}";

fn percent_text(s: &mut impl std::fmt::Write, part: Option<u64>, whole: u64) {
    if let (Some(part), true) = (part, whole > 0) {
        let p = (part as u128 * 1000 / whole as u128).min(1000) as u64;
        let _ = write!(s, "{}.{}%", p / 10, p % 10);
    }
}

fn percent_label(s: &mut impl std::fmt::Write, bucket: u64) {
    match bucket {
        sort::UNSPECIFIED => {
            let _ = s.write_str(S::DetailsGroupUnspecified.t());
        }
        b => {
            let lo = (9 - b.min(9)) * 10;
            let _ = write!(s, "{lo}% - {}%", lo + 9);
        }
    }
}

impl App {
    pub(super) fn column_kinds(&self) -> &'static [Column] {
        let t = self.tab();
        if t.view_loc.is_recycle_bin() {
            &BIN_COLUMNS
        } else if t.view_loc.path().kind() == PathKind::Computer {
            &COMPUTER_COLUMNS
        } else if t.git.as_ref().is_some_and(|g| g.root.is_some()) {
            &REPO_COLUMNS
        } else {
            &COLUMNS
        }
    }

    pub fn columns(&self) -> Vec<Column> {
        let cols = self.tab().columns;
        cols.ordered(self.column_kinds()).into_iter().filter(|&c| cols.shown(c)).collect()
    }

    pub fn folder_views(&self) -> &FolderViews {
        &self.folders
    }

    fn view_for(&self, t: &Tab) -> FolderView {
        let inherited = if t.mode == LayoutMode::Columns { t.folder_view() } else { self.prefs.defaults };
        self.folders.owner(&lineage(t.location())).and_then(|k| self.folders.get(k)).unwrap_or(inherited)
    }

    pub(super) fn view_mode(&self) -> ViewMode {
        self.folders.mode(folder_key(self.current_location()))
    }

    pub(super) fn set_view_mode(&mut self, mode: ViewMode) {
        let t = self.tab();
        self.folders.set_mode(folder_key(t.location()), mode, t.folder_view());
        self.dirty = true;
    }

    pub(super) fn cycle_view_mode(&mut self) {
        self.set_view_mode(match self.view_mode() {
            ViewMode::None => ViewMode::Folder,
            ViewMode::Folder => ViewMode::Subfolders,
            ViewMode::Subfolders => ViewMode::None,
        });
    }

    pub(super) fn view_mode_menu(&mut self) {
        let mode = self.view_mode();
        let r = self.layout.view;
        let menu = self.collect(|_, m| {
            m.header(S::DetailsViewSaved.t());
            for (v, label) in [(ViewMode::None, S::DetailsViewNone), (ViewMode::Folder, S::DetailsViewFolder), (ViewMode::Subfolders, S::DetailsViewSubfolders)] {
                m.radio_place(label.t(), PlaceCmd::ViewMode(v), view_mode_glyph(v), v == mode);
            }
            m.separator();
            m.place_icon(S::DetailsViewDefault.t(), PlaceCmd::DefaultView, "", mode == ViewMode::Folder);
        });
        self.button_menu(Hit::ViewMode, r.x, r.bottom(), menu);
    }

    pub(super) fn clear_folder_views(&mut self) {
        self.folders.clear();
        self.dirty = true;
    }

    pub(super) fn load_tab(&mut self, k: usize, select: Option<Vec<u8>>, now: Instant) {
        let gen = self.gen();
        if self.tabs[k].view_loc != *self.tabs[k].location() {
            let loc = self.tabs[k].location().clone();
            self.count_visit(&loc);
        }
        if !self.prefs.same_for_all {
            let fv = self.view_for(&self.tabs[k]);
            self.tabs[k].set_folder_view(fv);
        }
        self.sync_spec(k);
        self.tabs[k].retain_filter = self.prefs.retain_filter;
        let case = self.case;
        self.tabs[k].load(gen, select, case, now, &mut self.effects);
        self.git_refresh(k, false);
        self.place_loaded(k);
    }

    pub(super) fn sync_spec(&mut self, k: usize) {
        let (p, tz) = (&self.prefs, self.utc_offset_min);
        let t = &mut self.tabs[k];
        let (deep, tree) = (t.subfolders, t.mode == LayoutMode::Tree);
        let v = &mut t.view_spec;
        v.hide_dotfiles = !p.show_dotfiles;
        v.show_system = p.show_system;
        v.show_streams = p.show_streams;
        v.deep = deep;
        v.tree = tree;
        let dated = v.group.field.is_date();
        v.utc_offset_min = if dated { tz } else { 0 };
        v.today = if dated { format::local_days(format::now_ticks(), tz) } else { 0 };
    }

    pub(super) fn remember_view(&mut self) {
        if self.prefs.same_for_all {
            return;
        }
        let t = self.tab();
        let special = matches!(t.location().path().kind(), PathKind::RecycleBin | PathKind::Computer | PathKind::Home);
        match self.folders.owner(&lineage(t.location())) {
            Some(k) => self.folders.put(k, t.folder_view()),
            None if special => self.folders.put(folder_key(t.location()), t.folder_view()),
            None if t.mode != LayoutMode::Columns => self.prefs.defaults = t.folder_view(),
            None => {}
        }
    }

    pub(super) fn change_view(&mut self, f: impl FnOnce(&mut Tab)) {
        let k = self.cur;
        let before = self.tabs[k].view_spec;
        f(&mut self.tabs[k]);
        self.sync_spec(k);
        if self.tabs[k].view_spec != before {
            self.resort(k);
        }
        self.remember_view();
        self.layout();
        self.dirty = true;
    }

    pub(super) fn sort_by(&mut self, field: SortField) {
        if self.tab().view_spec.sort.field != field {
            self.change_view(|t| t.view_spec.sort = SortSpec { field, descending: false, ..t.view_spec.sort });
        }
    }

    pub(super) fn sort_column(&mut self, field: SortField) {
        self.change_view(|t| {
            let s = &mut t.view_spec.sort;
            s.descending = s.field == field && !s.descending;
            s.field = field;
        });
    }

    pub(super) fn set_descending(&mut self, descending: Option<bool>) {
        self.change_view(|t| {
            let s = &mut t.view_spec.sort;
            s.descending = descending.unwrap_or(!s.descending);
        });
    }

    pub(super) fn set_folders(&mut self, folders: Folders) {
        self.change_view(|t| t.view_spec.sort.folders = folders);
    }

    pub(super) fn group_by(&mut self, field: GroupField) {
        self.change_view(|t| t.view_spec.group.field = field);
    }

    pub(super) fn toggle_column(&mut self, c: Column) {
        if c != Column::Name {
            self.change_view(|t| t.columns.toggle(c));
            if c.is_git() {
                self.git_refresh(self.cur, false);
            }
            if self.prefs.auto_fit {
                self.request_fit();
            }
        }
    }

    pub(super) fn set_column_width(&mut self, c: Column, w: f32) {
        self.change_view(|t| t.columns.set_width(c, w.clamp(MIN_COL_W, MAX_COL_W).round()));
    }

    pub(super) fn request_fit(&mut self) {
        self.tab_mut().fit_pending = true;
        self.dirty = true;
    }

    pub(super) fn toggle_auto_fit(&mut self) {
        self.prefs.auto_fit = !self.prefs.auto_fit;
        if self.prefs.auto_fit {
            self.request_fit();
        }
    }

    pub(super) fn toggle_pref(&mut self, f: fn(&mut ViewPrefs) -> &mut bool) {
        let sizes = self.prefs.auto_sizes;
        let p = f(&mut self.prefs);
        *p = !*p;
        self.each_pane(|a, k| {
            let before = a.tabs[k].view_spec;
            a.sync_spec(k);
            if a.tabs[k].view_spec != before {
                a.resort(k);
            }
        });
        if self.prefs.auto_sizes && !sizes {
            self.compute_dir_sizes();
        }
        self.layout();
        self.dirty = true;
    }

    pub(super) fn set_pref(&mut self, f: fn(&mut ViewPrefs) -> &mut bool, on: bool) {
        if *f(&mut self.prefs) != on {
            self.toggle_pref(f);
        }
    }

    pub(super) fn toggle_streams(&mut self, now: Instant) {
        self.toggle_pref(|p| &mut p.show_streams);
        if self.prefs.show_streams {
            for k in 0..self.tabs.len() {
                self.reload(k, now);
            }
        }
    }

    pub(super) fn set_default_view(&mut self) {
        self.prefs.defaults = self.tab().folder_view();
        self.status(S::DetailsStatusDefaultView.t());
    }

    pub(super) fn fit_columns<B: TextBackend>(&mut self, g: &mut Gfx<B>, arena: &Bump) {
        let k = self.cur;
        if !std::mem::take(&mut self.tabs[k].fit_pending) {
            return;
        }
        let only = self.tabs[k].fit_column.take();
        let (font, hfont) = (self.ui_font(13.0), self.ui_font(12.0));
        let (pad, glyph) = (self.s(10.0), self.s(20.0));
        let lines = self.visible_lines(&self.flow());
        let t = &self.tabs[k];
        let Some(v) = &t.view else { return };
        let mut cols = t.columns;

        for &c in self.layout.columns.iter().filter(|&&c| c != Column::Name && only.is_none_or(|o| o == c)) {
            let mut w = g.measure(c.title(), hfont) + glyph;
            for line in lines.clone() {
                if let Some(Line::Row(r)) = v.line(line) {
                    w = w.max(g.measure(self.cell_text(arena, v.listing(), v.rows()[r], c), font));
                }
            }
            cols.set_width(c, ((w + 2.0 * pad) / self.scale).clamp(MIN_COL_W, MAX_COL_W).ceil());
        }
        self.change_view(|t| t.columns = cols);
    }

    pub(super) fn header_menu(&mut self, x: f32, y: f32, col: Option<Column>) {
        let menu = self.build_header_menu(col);
        self.flit_menu(x, y, menu);
    }

    pub(super) fn column_command(&mut self, c: ColumnCmd) {
        let kinds = self.column_kinds();
        match c {
            ColumnCmd::Toggle(c) => self.toggle_column(c),
            ColumnCmd::Shift(c, right) => self.change_view(|t| t.columns.shift(c, kinds, right)),
            ColumnCmd::Fit(c) => {
                self.tab_mut().fit_column = Some(c);
                self.request_fit();
            }
            ColumnCmd::Restore => self.change_view(|t| t.columns = t.columns.restored()),
        }
        self.layout();
    }

    pub(super) fn flit_menu(&mut self, x: f32, y: f32, menu: Vec<MenuItem>) {
        self.context_flit_menu(x, y, menu, super::MenuKind::None);
    }

    pub(super) fn context_flit_menu(&mut self, x: f32, y: f32, menu: Vec<MenuItem>, kind: super::MenuKind) {
        self.menu_owner = None;
        self.menu_right = None;
        self.tip_hide();
        let dir = self.tab().view_loc.path().clone();
        self.effects.push(Effect::ShellMenu(ShellMenu { dir, items: Vec::new(), x, y, extended: false, menu, inline: false, shell: false, kind }));
    }

    pub(super) fn shown_name<'a>(&self, l: &'a Listing, i: u32) -> Cow<'a, str> {
        let n = l.name(i);
        let ext = l.entries[i as usize].ext_len as usize;
        if self.prefs.show_extensions || ext == 0 || ext + 1 >= n.len() {
            return name::display(n);
        }
        name::display(&n[..n.len() - ext - 1])
    }

    pub(super) fn type_text<'a>(&self, arena: &'a Bump, l: &Listing, i: u32) -> &'a str {
        let e = &l.entries[i as usize];
        if e.attrs & attr::VOLUME != 0 {
            return S::DetailsTypeDrive.t();
        }
        if e.is_dir() {
            return S::DetailsTypeFolder.t();
        }
        let ext = l.ext(i);
        match self.type_names.get(&ext.to_ascii_lowercase()) {
            Some(Some(n)) if !n.is_empty() => arena.alloc_str(n),
            _ if ext.is_empty() => S::DetailsTypeFile.t(),
            _ => {
                let mut s = bumpalo::collections::String::new_in(arena);
                crate::tr_into!(&mut s, S::DetailsTypeExtFile, ext = name::display(ext).to_uppercase());
                s.into_bump_str()
            }
        }
    }

    pub(super) fn cell_text<'a>(&self, arena: &'a Bump, l: &Listing, i: u32, col: Column) -> &'a str {
        let e = &l.entries[i as usize];
        let mut s = bumpalo::collections::String::new_in(arena);
        match col {
            Column::Name => s.push_str(&self.shown_name(l, i)),
            Column::Modified | Column::Deleted => format::datetime_in(&mut s, e.modified, self.utc_offset_min, &self.style),
            Column::Created => format::datetime_in(&mut s, e.created, self.utc_offset_min, &self.style),
            Column::Origin => s.push_str(&name::display(recycle::origin(l, i))),
            Column::Tag => self.tag_text(&mut s, e.tags),
            Column::GitStatus | Column::GitMessage | Column::GitAuthor | Column::GitDate => self.git_text(&mut s, l.name(i), col),
            Column::Type => return self.type_text(arena, l, i),
            Column::Size => {
                if !e.is_dir() || e.attrs & attr::SIZE_KNOWN != 0 {
                    format::size_in(&mut s, e.size, self.style.decimal);
                }
            }
            Column::Accessed => {
                if e.accessed > 0 {
                    format::datetime_in(&mut s, e.accessed, self.utc_offset_min, &self.style);
                }
            }
            Column::Folders | Column::Files => {
                let count = |st: &crate::listing::DirStats| if col == Column::Folders { st.folders } else { st.files };
                match l.stats(i) {
                    Some(st) if e.is_dir() && count(&st) > 0 => {
                        let _ = write!(s, "{}", count(&st));
                    }
                    Some(_) => s.push_str(NO_ITEMS),
                    None if !e.is_dir() => s.push_str(NO_ITEMS),
                    None => {}
                }
            }
            Column::Volume => percent_text(&mut s, sort::known_size(e), l.volume),
            Column::FolderUsage => percent_text(&mut s, sort::known_size(e), l.total),
            Column::Free => {
                if let Some((free, _)) = l.drive(i) {
                    format::size_in(&mut s, free, self.style.decimal);
                }
            }
            Column::Percent => {
                if let Some(p) = sort::used_permille(l, i) {
                    let _ = write!(s, "{}.{}%", p / 10, p % 10);
                }
            }
            Column::FileSystem => s.push_str(&name::display(sort::file_system(l, i))),
        }
        s.into_bump_str()
    }

    fn group_label<'a>(&self, arena: &'a Bump, v: &DirView, gr: &Group) -> &'a str {
        let (l, spec, i) = (v.listing(), v.snap().view, gr.first);
        let e = &l.entries[i as usize];
        let mut s = bumpalo::collections::String::new_in(arena);
        let key = sort::group_key(l, i, &spec).0;
        match spec.group.field {
            GroupField::None => {}
            GroupField::Name => bucket_label(&mut s, &NAME_GROUPS, key),
            GroupField::Type => return self.type_text(arena, l, i),
            GroupField::Size | GroupField::Free => bucket_label(&mut s, &SIZE_GROUPS, key),
            GroupField::Modified | GroupField::Created | GroupField::Accessed => bucket_label(&mut s, &DATE_GROUPS, key),
            GroupField::Items => bucket_label(&mut s, &ITEM_GROUPS, key),
            GroupField::Volume | GroupField::FolderUsage | GroupField::Percent => percent_label(&mut s, key),
            GroupField::FileSystem => match sort::file_system(l, i) {
                [] => s.push_str(S::DetailsGroupUnspecified.t()),
                fs => s.push_str(&name::display(fs)),
            },
            GroupField::Origin => match recycle::origin(l, i) {
                [] => s.push_str(S::DetailsGroupUnknownLocation.t()),
                o => s.push_str(&name::display(o)),
            },
            GroupField::Tag => s.push_str(self.tag_defs().get(sort::first_tag(e.tags) as usize).map_or(S::DetailsGroupNoTags.t(), |t| t.name.as_str())),
        }
        s.into_bump_str()
    }

    pub(super) fn draw_group_line<B: TextBackend>(&self, g: &mut Gfx<B>, arena: &Bump, th: &Theme, v: &DirView, k: usize, r: Rect) {
        let gr = v.groups()[k];
        let (y, rh, right) = (r.y, r.h, r.right());
        if self.hover == Hit::Group(k) && self.drag == Drag::None {
            g.dl.outline(r.inset(self.s(4.0), 1.0), th.accent, self.s(4.0), self.s(1.0).max(1.0));
        }
        let x0 = r.x + self.s(10.0);
        centered(g, Rect::new(x0, y, self.s(16.0), rh), if gr.collapsed { G_CHEVRON } else { G_SORT_DOWN }, self.icon_font(8.0), th.text_dim);
        let mut tx = x0 + self.s(24.0);
        if let Some(color) = self.group_color(v, &gr) {
            centered(g, Rect::new(tx, y, self.s(16.0), rh), super::tags::G_TAG, self.icon_font(12.0), color);
            tx += self.s(22.0);
        } else if v.snap().view.group.field.is_date() {
            centered(g, Rect::new(tx, y, self.s(16.0), rh), G_CALENDAR, self.icon_font(12.0), th.text_dim);
            tx += self.s(22.0);
        }
        let bold = FontStyle::new(FontFamily::Ui, 14.0 * self.scale, true);
        let m = g.metrics(bold);
        let baseline = (y + (rh + m.ascent - m.descent) / 2.0).round();
        let room = right - tx;
        let w = g.text(tx, baseline, self.group_label(arena, v, &gr), bold, th.text, (room - self.s(80.0)).max(room * 0.6));
        let mut count = bumpalo::collections::String::new_in(arena);
        let _ = write!(count, "({})", gr.count);
        let cx = tx + w + self.s(8.0);
        let cw = g.text(cx, baseline, &count, self.ui_font(12.0), th.text_dim, f32::INFINITY);
        let sx = cx + cw + self.s(10.0);
        if sx < right - self.s(12.0) {
            g.dl.rect(Rect::new(sx, (y + rh / 2.0).round(), right - self.s(12.0) - sx, 1.0), th.separator);
        }
    }

    pub(super) fn on_loaded(&mut self, k: usize) {
        self.archive_loaded(k);
        self.tabs[k].fit_pending |= self.prefs.auto_fit;
        if self.prefs.auto_sizes && k == self.cur {
            self.compute_dir_sizes();
        }
    }

    pub(super) fn toggle_group(&mut self, k: usize) {
        if let Some(v) = &mut self.tab_mut().view {
            v.toggle_group(k);
        }
        self.layout();
        self.dirty = true;
    }

    pub(super) fn draw_unavailable<B: TextBackend>(&self, g: &mut Gfx<B>, th: &Theme, list: Rect, e: &FsError) {
        let title = match e.kind {
            _ if e.code == crate::archive::PASSWORD_CODE => S::ErrorsFsTitleEncrypted,
            FsErrorKind::NotFound => S::ErrorsFsTitleNotFound,
            FsErrorKind::AccessDenied => S::ErrorsFsTitleAccessDenied,
            FsErrorKind::NotADirectory => S::ErrorsFsTitleNotADirectory,
            FsErrorKind::Unavailable => S::ErrorsFsTitleUnavailable,
            FsErrorKind::Other => S::ErrorsFsTitleOther,
            FsErrorKind::NoMedia => S::ErrorsFsTitleNoMedia,
        }
        .t();
        let detail = crate::tr!(if e.kind == FsErrorKind::NoMedia { S::ErrorsFsInsertMedia } else { S::ErrorsFsRetry }, error = e);
        let (big, small) = (self.ui_font(16.0), self.ui_font(13.0));
        let y = list.y + self.s(48.0);
        centered(g, Rect::new(list.x, y, list.w, self.s(32.0)), G_WARNING, self.icon_font(24.0), th.error);
        for (k, (text, font, color)) in [(title, big, th.text), (detail.as_str(), small, th.text_dim)].into_iter().enumerate() {
            let w = g.measure(text, font).min(list.w);
            g.text(list.x + (list.w - w) / 2.0, y + self.s(56.0 + 24.0 * k as f32), text, font, color, list.w);
        }
        self.draw_eject(g, th);
    }

    pub(super) fn draw_checkbox<B: TextBackend>(&self, g: &mut Gfx<B>, th: &Theme, r: Rect, on: bool) {
        if on {
            g.dl.round_rect(r, th.accent, self.s(3.0));
            g.dl.check(r, crate::theme::ink_on(th.accent));
        } else {
            g.dl.outline(r, th.text_dim, self.s(3.0), self.s(1.0).max(1.0));
        }
    }

    pub(super) fn toggle_row(&mut self, row: usize) {
        let Some(v) = &mut self.tab_mut().view else { return };
        let Some(&i) = v.rows().get(row) else { return };
        let on = !v.is_selected(i);
        v.set_selected(i, on);
        v.focus = Some(i);
        v.anchor = Some(i);
        self.dirty = true;
    }

    pub(super) fn arm_timer(&mut self, now: Instant, at: Instant) {
        if self.timer_at.is_some_and(|t| t > now && t <= at) {
            return;
        }
        self.timer_at = Some(at);
        self.effects.push(Effect::Timer(at.saturating_duration_since(now)));
    }

    pub(super) fn hover_pick(&mut self, h: Hit, now: Instant) {
        if !self.prefs.hover_select {
            return;
        }
        let quiet = self.active && self.drag == Drag::None && self.pressed == Hit::None && !self.mods.ctrl && !self.mods.shift && self.ops_idle();
        let t = self.tab();
        let idx = match h {
            Hit::Row(r) if quiet => t.view.as_ref().and_then(|v| v.rows().get(r).copied()),
            _ => None,
        };
        let tab = t.id;
        match idx {
            Some(i) if self.details.pick.as_ref().is_some_and(|p| p.tab == tab && p.idx == i) => {}
            Some(i) => {
                self.details.pick = Some(Pick { tab, idx: i, at: Some(now + HOVER_SELECT) });
                self.arm_timer(now, now + HOVER_SELECT);
            }
            None => self.details.pick = None,
        }
    }

    pub(super) fn hover_timer(&mut self, now: Instant) {
        let Some(p) = &mut self.details.pick else { return };
        let Some(at) = p.at else { return };
        if now + TIMER_SLACK < at {
            return self.arm_timer(now, at);
        }
        p.at = None;
        let (tab, idx) = (p.tab, p.idx);
        let hovered = match self.hover {
            Hit::Row(r) => self.tab().view.as_ref().and_then(|v| v.rows().get(r).copied()),
            _ => None,
        };
        if self.tab().id != tab || hovered != Some(idx) {
            return;
        }
        if let Some(v) = &mut self.tab_mut().view {
            v.clear_selection();
            v.set_selected(idx, true);
            v.focus = Some(idx);
            v.anchor = Some(idx);
        }
        self.dirty = true;
    }

    pub(super) fn end_autoscroll(&mut self) {
        if self.details.autoscroll.take().is_some() {
            self.set_cursor(CursorIcon::Default);
        }
    }

    pub(super) fn autoscrolling(&self) -> bool {
        self.details.autoscroll.is_some()
    }

    pub(super) fn autoscroll_down(&mut self, h: Hit, along: f32, button: MouseButton) -> bool {
        if self.details.autoscroll.is_some() {
            self.end_autoscroll();
            self.details.swallow = Some(button);
            return true;
        }
        if button != MouseButton::Middle || !matches!(h, Hit::Row(_) | Hit::ListEmpty | Hit::Group(_)) {
            return false;
        }
        self.details.middle_on_empty = h == Hit::ListEmpty;
        self.details.autoscroll = Some(Autoscroll { anchor: along, y: along, moved: false });
        self.set_cursor(CursorIcon::Scroll);
        true
    }

    pub(super) fn autoscroll_move(&mut self, along: f32) -> bool {
        let dead = self.s(AUTOSCROLL_DEAD);
        let Some(a) = &mut self.details.autoscroll else { return false };
        a.y = along;
        a.moved |= (along - a.anchor).abs() > dead;
        self.dirty = true;
        true
    }

    pub(super) fn autoscroll_up(&mut self, x: f32, y: f32, button: MouseButton) -> bool {
        if self.details.swallow == Some(button) {
            self.details.swallow = None;
            return true;
        }
        if button == MouseButton::Middle && self.details.autoscroll.as_ref().is_some_and(|a| a.moved) {
            self.end_autoscroll();
            return true;
        }
        if button == MouseButton::Middle && std::mem::take(&mut self.details.middle_on_empty) && self.details.autoscroll.is_some() {
            self.end_autoscroll();
            self.details.middle_at = Some((x, y));
            self.quick_commands();
            return true;
        }
        self.details.autoscroll.is_some()
    }

    pub(super) fn autoscroll_step(&mut self, dt: f32) -> bool {
        let dead = self.s(AUTOSCROLL_DEAD);
        let Some(a) = &self.details.autoscroll else { return false };
        let off = a.y - a.anchor;
        if off.abs() <= dead {
            return false;
        }
        let max = self.max_scroll();
        let t = self.tab_mut();
        let was = t.scroll;
        t.scroll = (t.scroll + (off - dead * off.signum()) * AUTOSCROLL_RATE * dt).clamp(0.0, max);
        t.scroll_target = t.scroll;
        t.scroll != was || dt == 0.0
    }
}

#[cfg(test)]
#[path = "../../test/ui/details.rs"]
mod tests;
