use super::layouts::{LayoutMode, Sizes};
use super::ops::TIMER_SLACK;
use super::{centered, App, Column, Drag, Effect, Hit, MenuItem, ShellMenu, Tab, BIN_COLUMNS, COLUMNS, G_CHEVRON, G_FILE, G_FOLDER, G_SORT_DOWN};
use crate::fmt as format;
use crate::hash::{FxHashMap, FxHasher};
use crate::input::{CursorIcon, MouseButton};
use crate::listing::{attr, FsError, FsErrorKind, Listing};
use crate::location::Location;
use crate::name;
use crate::provider::TabId;
use crate::recycle;
use crate::render::Rect;
use crate::sort::{self, DateUnit, Folders, GroupField, GroupSpec, SortField, SortSpec};
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
pub(super) const CHECK_GAP: f32 = 8.0;
pub const THUMB_CACHE_MB: (u32, u32, u32) = (100, 500, 5000);
const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
const SIZE_GROUPS: [&str; 8] = [
    "Folders",
    "Empty",
    "Tiny (under 16 KB)",
    "Small (16 KB to 1 MB)",
    "Medium (1 to 128 MB)",
    "Large (128 MB to 1 GB)",
    "Huge (1 to 4 GB)",
    "Gigantic (over 4 GB)",
];
const G_CALENDAR: &str = "\u{E787}";
const G_FONT: &str = "\u{E8D2}";
const G_PACKAGE: &str = "\u{E7B8}";
const G_CHECK: &str = "\u{E73E}";
const G_WARNING: &str = "\u{E7BA}";

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Columns {
    pub hidden: u8,
    pub widths: [f32; 7],
}

impl Default for Columns {
    fn default() -> Self {
        Self { hidden: 1 << Column::Created as u8, widths: [0.0, 150.0, 140.0, 90.0, 220.0, 150.0, 150.0] }
    }
}

impl Columns {
    pub fn shown(&self, c: Column) -> bool {
        c == Column::Name || ((self.hidden >> c as u8) & 1 == 1) == (c == Column::Tag)
    }

    pub fn width(&self, c: Column) -> f32 {
        self.widths[c.slot()]
    }

    pub fn sanitized(mut self) -> Self {
        let d = Self::default();
        for (w, dw) in self.widths.iter_mut().zip(d.widths) {
            if !(w.is_finite() && (MIN_COL_W..=MAX_COL_W).contains(w)) {
                *w = dw;
            }
        }
        self.hidden &= 0xFE;
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
}

impl FolderView {
    pub fn sanitized(mut self) -> Self {
        self.columns = self.columns.sanitized();
        self.sizes = self.sizes.sanitized();
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
    pub thumbnails: bool,
    #[serde(skip, default = "thumb_cache_mb")]
    pub thumb_cache_mb: u32,
    #[serde(skip)]
    pub show_streams: bool,
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
            thumbnails: true,
            thumb_cache_mb: THUMB_CACHE_MB.1,
            show_streams: false,
        }
    }
}

#[derive(Default)]
struct Lru {
    map: FxHashMap<u64, (u64, FolderView)>,
    clock: u64,
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
        }
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

pub fn folder_key(loc: &Location) -> u64 {
    let mut h = FxHasher::default();
    match loc {
        Location::Local(p) => p.as_bytes().iter().for_each(|b| h.write_u8(b.to_ascii_lowercase())),
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
pub(super) struct Details {
    pick: Option<Pick>,
    autoscroll: Option<Autoscroll>,
    swallow: Option<MouseButton>,
}

impl Column {
    pub(super) fn toggle_id(self) -> &'static str {
        match self {
            Column::Name => "",
            Column::Modified => "view.column.modified",
            Column::Created => "view.column.created",
            Column::Type => "view.column.type",
            Column::Size => "view.column.size",
            Column::Origin => "view.column.origin",
            Column::Deleted => "view.column.deleted",
            Column::Tag => "view.column.tag",
        }
    }

    pub(super) fn slot(self) -> usize {
        if self == Column::Tag { Column::Deleted as usize } else { self as usize }
    }
}

impl Tab {
    pub(super) fn folder_view(&self) -> FolderView {
        FolderView { layout: self.mode, sort: self.view_spec.sort, group: self.view_spec.group, columns: self.columns, sizes: self.sizes }
    }

    pub(super) fn set_folder_view(&mut self, f: FolderView) {
        self.mode = f.layout;
        self.sizes = f.sizes;
        self.view_spec.sort = f.sort;
        self.view_spec.group = f.group;
        self.columns = f.columns;
    }
}

fn date_label(s: &mut impl std::fmt::Write, value: Option<i64>, unit: DateUnit) {
    let Some(n) = value else {
        let _ = s.write_str("Unknown date");
        return;
    };
    let _ = match unit {
        DateUnit::Year => write!(s, "{n}"),
        DateUnit::Month => write!(s, "{} {}", MONTHS[n.rem_euclid(12) as usize], n.div_euclid(12)),
        DateUnit::Day => {
            let (y, m, d) = format::civil_from_days(n);
            write!(s, "{y:04}-{m:02}-{d:02}")
        }
    };
}

impl App {
    pub(super) fn column_kinds(&self) -> &'static [Column] {
        if self.tab().view_loc.is_recycle_bin() { &BIN_COLUMNS } else { &COLUMNS }
    }

    pub fn columns(&self) -> Vec<Column> {
        let cols = self.tab().columns;
        self.column_kinds().iter().copied().filter(|&c| cols.shown(c)).collect()
    }

    pub fn folder_views(&self) -> &FolderViews {
        &self.folders
    }

    fn view_for(&self, t: &Tab) -> FolderView {
        let inherited = if t.mode == LayoutMode::Columns { t.folder_view() } else { self.prefs.defaults };
        self.folders.get(folder_key(t.location())).unwrap_or(inherited)
    }

    pub(super) fn load_tab(&mut self, k: usize, select: Option<Vec<u8>>, now: Instant) {
        let gen = self.gen();
        if !self.prefs.same_for_all {
            let fv = self.view_for(&self.tabs[k]);
            self.tabs[k].set_folder_view(fv);
        }
        self.sync_spec(k);
        let case = self.case;
        self.tabs[k].load(gen, select, case, now, &mut self.effects);
        self.place_loaded(k);
    }

    pub(super) fn sync_spec(&mut self, k: usize) {
        let (p, tz) = (&self.prefs, self.utc_offset_min);
        let v = &mut self.tabs[k].view_spec;
        v.hide_dotfiles = !p.show_dotfiles;
        v.show_system = p.show_system;
        v.show_streams = p.show_streams;
        v.utc_offset_min = if v.group.field.is_date() { tz } else { 0 };
    }

    pub(super) fn remember_view(&mut self) {
        if !self.prefs.same_for_all {
            let t = self.tab();
            self.folders.put(folder_key(t.location()), t.folder_view());
        }
    }

    pub(super) fn change_view(&mut self, f: impl FnOnce(&mut Tab)) {
        let k = self.cur;
        let before = self.tabs[k].view_spec;
        f(&mut self.tabs[k]);
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

    pub(super) fn set_group_descending(&mut self, descending: bool) {
        self.change_view(|t| t.view_spec.group.descending = descending);
    }

    pub(super) fn set_date_unit(&mut self, unit: DateUnit) {
        self.change_view(|t| t.view_spec.group.unit = unit);
    }

    pub(super) fn toggle_column(&mut self, c: Column) {
        if c != Column::Name {
            self.change_view(|t| t.columns.hidden ^= 1 << c as u8);
            if self.prefs.auto_fit {
                self.request_fit();
            }
        }
    }

    pub(super) fn set_column_width(&mut self, c: Column, w: f32) {
        self.change_view(|t| t.columns.widths[c.slot()] = w.clamp(MIN_COL_W, MAX_COL_W).round());
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
        for k in 0..self.tabs.len() {
            let before = self.tabs[k].view_spec;
            self.sync_spec(k);
            if self.tabs[k].view_spec != before {
                self.resort(k);
            }
        }
        if self.prefs.auto_sizes && !sizes {
            self.compute_dir_sizes();
        }
        self.layout();
        self.dirty = true;
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
        self.status("New folders will open with this view");
    }

    pub(super) fn fit_columns<B: TextBackend>(&mut self, g: &mut Gfx<B>, arena: &Bump) {
        let k = self.cur;
        if !std::mem::take(&mut self.tabs[k].fit_pending) {
            return;
        }
        let (font, hfont) = (self.ui_font(13.0), self.ui_font(12.0));
        let (pad, glyph) = (self.s(10.0), self.s(20.0));
        let lines = self.visible_lines(&self.flow());
        let t = &self.tabs[k];
        let Some(v) = &t.view else { return };
        let mut widths = t.columns.widths;
        for &c in self.layout.columns.iter().filter(|&&c| c != Column::Name) {
            let mut w = g.measure(c.title(), hfont) + glyph;
            for line in lines.clone() {
                if let Some(Line::Row(r)) = v.line(line) {
                    w = w.max(g.measure(self.cell_text(arena, v.listing(), v.rows()[r], c), font));
                }
            }
            widths[c.slot()] = ((w + 2.0 * pad) / self.scale).clamp(MIN_COL_W, MAX_COL_W).ceil();
        }
        self.change_view(|t| t.columns.widths = widths);
    }

    pub(super) fn header_menu(&mut self, x: f32, y: f32) {
        let menu = self.build_header_menu();
        self.flit_menu(x, y, menu);
    }

    pub(super) fn flit_menu(&mut self, x: f32, y: f32, menu: Vec<MenuItem>) {
        let dir = self.tab().view_loc.path().clone();
        self.effects.push(Effect::ShellMenu(ShellMenu { dir, items: Vec::new(), x, y, extended: false, menu, inline: false, shell: false }));
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
            return "Drive";
        }
        if e.is_dir() {
            return "File folder";
        }
        let ext = l.ext(i);
        match self.type_names.get(&ext.to_ascii_lowercase()) {
            Some(Some(n)) if !n.is_empty() => arena.alloc_str(n),
            _ if ext.is_empty() => "File",
            _ => {
                let mut s = bumpalo::collections::String::new_in(arena);
                let _ = write!(s, "{} File", name::display(ext).to_uppercase());
                s.into_bump_str()
            }
        }
    }

    pub(super) fn cell_text<'a>(&self, arena: &'a Bump, l: &Listing, i: u32, col: Column) -> &'a str {
        let e = &l.entries[i as usize];
        let mut s = bumpalo::collections::String::new_in(arena);
        match col {
            Column::Name => s.push_str(&self.shown_name(l, i)),
            Column::Modified | Column::Deleted => format::datetime(&mut s, e.modified, self.utc_offset_min),
            Column::Created => format::datetime(&mut s, e.created, self.utc_offset_min),
            Column::Origin => s.push_str(&name::display(recycle::origin(l, i))),
            Column::Tag => self.tag_text(&mut s, e.tags),
            Column::Type => return self.type_text(arena, l, i),
            Column::Size => {
                if !e.is_dir() || e.attrs & (attr::SIZE_KNOWN | attr::VOLUME) != 0 {
                    format::size(&mut s, e.size);
                }
            }
        }
        s.into_bump_str()
    }

    fn group_label<'a>(&self, arena: &'a Bump, v: &DirView, gr: &Group) -> &'a str {
        let (l, spec, i) = (v.listing(), v.snap().view, gr.first);
        let e = &l.entries[i as usize];
        let mut s = bumpalo::collections::String::new_in(arena);
        match spec.group.field {
            GroupField::None => {}
            GroupField::Name => match sort::group_key(l, i, &spec).0 {
                1 => s.push('#'),
                2 => s.push_str("0-9"),
                _ => s.extend(name::display(l.name(i)).chars().take(1).flat_map(char::to_uppercase)),
            },
            GroupField::Type => return self.type_text(arena, l, i),
            GroupField::Size => s.push_str(SIZE_GROUPS[sort::size_bucket(e) as usize]),
            GroupField::Modified => date_label(&mut s, sort::date_unit_value(e.modified, &spec), spec.group.unit),
            GroupField::Created => date_label(&mut s, sort::date_unit_value(e.created, &spec), spec.group.unit),
            GroupField::Origin => match recycle::origin(l, i) {
                [] => s.push_str("Unknown location"),
                o => s.push_str(&name::display(o)),
            },
            GroupField::Tag => s.push_str(self.tag_defs().get(sort::first_tag(e.tags) as usize).map_or("No tags", |t| t.name.as_str())),
        }
        s.into_bump_str()
    }

    fn group_glyph(v: &DirView, gr: &Group) -> &'static str {
        match v.snap().view.group.field {
            GroupField::Name => G_FONT,
            GroupField::Type if v.listing().entries[gr.first as usize].is_dir() => G_FOLDER,
            GroupField::Type => G_FILE,
            GroupField::Size => G_PACKAGE,
            GroupField::Modified | GroupField::Created => G_CALENDAR,
            GroupField::Origin | GroupField::None => G_FOLDER,
            GroupField::Tag => super::tags::G_TAG,
        }
    }

    pub(super) fn draw_group_line<B: TextBackend>(&self, g: &mut Gfx<B>, arena: &Bump, th: &Theme, v: &DirView, k: usize, r: Rect) {
        let gr = v.groups()[k];
        let (y, rh, right) = (r.y, r.h, r.right());
        if self.hover == Hit::Group(k) && self.drag == Drag::None {
            g.dl.round_rect(r.inset(self.s(4.0), 1.0), th.hover, self.s(4.0));
        }
        let x0 = r.x + self.s(10.0);
        centered(g, Rect::new(x0, y, self.s(16.0), rh), if gr.collapsed { G_CHEVRON } else { G_SORT_DOWN }, self.icon_font(8.0), th.text_dim);
        let glyph_color = self.group_color(v, &gr).unwrap_or(th.accent.alpha(0xB0));
        centered(g, Rect::new(x0 + self.s(20.0), y, self.s(16.0), rh), Self::group_glyph(v, &gr), self.icon_font(12.0), glyph_color);
        let bold = FontStyle::new(FontFamily::Ui, 13.0 * self.scale, true);
        let m = g.metrics(bold);
        let baseline = (y + (rh + m.ascent - m.descent) / 2.0).round();
        let tx = x0 + self.s(44.0);
        let room = right - tx;
        let w = g.text(tx, baseline, self.group_label(arena, v, &gr), bold, th.text, (room - self.s(80.0)).max(room * 0.6));
        let mut count = bumpalo::collections::String::new_in(arena);
        let _ = write!(count, "{}", gr.count);
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
            _ if e.code == crate::archive::PASSWORD_CODE => "This archive is encrypted",
            FsErrorKind::NotFound => "This folder does not exist",
            FsErrorKind::AccessDenied => "You do not have permission to open this folder",
            FsErrorKind::NotADirectory => "This location is not a folder",
            FsErrorKind::Unavailable => "This location is not available",
            FsErrorKind::Other => "This folder cannot be opened",
            FsErrorKind::NoMedia => "There is no disc or memory card in this drive",
        };
        let detail = if e.kind == FsErrorKind::NoMedia { format!("{e}. Insert one and press F5, or eject the drive.") } else { format!("{e}. Press F5 to try again.") };
        let (big, small) = (self.ui_font(16.0), self.ui_font(13.0));
        let y = list.y + self.s(48.0);
        centered(g, Rect::new(list.x, y, list.w, self.s(32.0)), G_WARNING, self.icon_font(24.0), th.error);
        for (k, (text, font, color)) in [(title, big, th.text), (detail.as_str(), small, th.text_dim)].into_iter().enumerate() {
            let w = g.measure(text, font).min(list.w);
            g.text(list.x + (list.w - w) / 2.0, y + self.s(56.0 + 24.0 * k as f32), text, font, color, list.w);
        }
        self.draw_eject(g, th);
    }

    pub(super) fn count_text(&self, s: &mut impl std::fmt::Write, v: &DirView) {
        let n = v.base_rows().len();
        let plural = |k: usize, one: &'static str, many: &'static str| if k == 1 { one } else { many };
        let _ = if v.is_filtered() {
            write!(s, "{n} of {} {}", v.total_rows(), plural(v.total_rows(), "item", "items"))
        } else if self.prefs.split_count {
            let dirs = v.base_rows().iter().filter(|&&i| v.listing().entries[i as usize].is_dir()).count();
            write!(s, "{dirs} {}, {} {}", plural(dirs, "folder", "folders"), n - dirs, plural(n - dirs, "file", "files"))
        } else {
            write!(s, "{n} {}", plural(n, "item", "items"))
        };
    }

    pub(super) fn draw_checkbox<B: TextBackend>(&self, g: &mut Gfx<B>, th: &Theme, r: Rect, on: bool) {
        if on {
            g.dl.round_rect(r, th.accent, self.s(3.0));
            centered(g, r, G_CHECK, self.icon_font(9.0), th.bg);
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

    pub(super) fn autoscroll_up(&mut self, button: MouseButton) -> bool {
        if self.details.swallow == Some(button) {
            self.details.swallow = None;
            return true;
        }
        if button == MouseButton::Middle && self.details.autoscroll.as_ref().is_some_and(|a| a.moved) {
            self.end_autoscroll();
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
