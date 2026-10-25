pub mod edit;
mod tab;
pub mod actions;
mod field;
mod filter;
mod nav;
pub mod palette;
pub mod sidebar;

pub use sidebar::{Drive, KnownFolder, NavState, Places};
pub use tab::Tab;

use crate::fmt as format;
use crate::hash::{self, FxHashMap};
use crate::input::{CursorIcon, Event, HitArea, Key, Mods, MouseButton};
use crate::listing::{attr, DirSnapshot};
use crate::location::{Location, ProviderId};
use crate::name;
use crate::open::{self, Open, OpenTarget, ResolveError, Route};
use crate::path::{FsPath, PathKind};
use crate::provider::{FileOp, ProviderEvent, ProviderRequest, TabId};
use crate::render::{Color, Rect};
use crate::sort::{CaseMode, SortField, SortSpec, ViewSpec};
use crate::text::{FontFamily, FontStyle, Gfx, TextBackend};
use crate::theme::{self, Theme};
use bumpalo::Bump;
use edit::{EditAction, LineEdit};
use std::fmt::Write as _;
use std::sync::Arc;
use std::time::{Duration, Instant};

const TAB_H: f32 = 36.0;
const TOOL_H: f32 = 40.0;
const HEADER_H: f32 = 28.0;
const ROW_H: f32 = 24.0;
const STATUS_H: f32 = 24.0;
const NAV_W: f32 = 34.0;
const CAPTION_W: f32 = 46.0;
const SCROLLBAR_W: f32 = 12.0;
const ICON: f32 = 16.0;
const NAME_MIN_W: f32 = 120.0;
const TAB_MAX_W: f32 = 220.0;
const TAB_MIN_W: f32 = 72.0;

const DIFF_BUDGET: Duration = Duration::from_millis(4);
const MAX_PENDING_DIFFS: usize = 1 << 16;
const SCROLL_TAU: f32 = 0.045;
const MAX_OPEN: usize = 16;
const MAX_ROUTES: usize = 32;

const PER_FILE_ICON_EXT: [&[u8]; 6] = [b"exe", b"ico", b"lnk", b"url", b"cur", b"ani"];

const G_BACK: &str = "\u{E72B}";
const G_FORWARD: &str = "\u{E72A}";
const G_UP: &str = "\u{E74A}";
const G_REFRESH: &str = "\u{E72C}";
const G_MIN: &str = "\u{E921}";
const G_MAX: &str = "\u{E922}";
const G_RESTORE: &str = "\u{E923}";
const G_CLOSE: &str = "\u{E8BB}";
const G_ADD: &str = "\u{E710}";
const G_CHEVRON: &str = "\u{E76C}";
const G_SORT_UP: &str = "\u{E70E}";
const G_SORT_DOWN: &str = "\u{E70D}";
const G_FOLDER: &str = "\u{E8B7}";
const G_FILE: &str = "\u{E7C3}";
const G_DRIVE: &str = "\u{EDA2}";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Column {
    Name,
    Modified,
    Type,
    Size,
}

pub const COLUMNS: [Column; 4] = [Column::Name, Column::Modified, Column::Type, Column::Size];

impl Column {
    fn title(self) -> &'static str {
        match self {
            Column::Name => "Name",
            Column::Modified => "Date modified",
            Column::Type => "Type",
            Column::Size => "Size",
        }
    }
    fn field(self) -> SortField {
        match self {
            Column::Name => SortField::Name,
            Column::Modified => SortField::Modified,
            Column::Type => SortField::Type,
            Column::Size => SortField::Size,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WindowCmd {
    Minimize,
    ToggleMaximize,
    Close,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IconKind {
    Type,
    File,
    Folder,
    Volume,
}

#[derive(Clone, Debug)]
pub struct IconRequest {
    pub key: u64,
    pub kind: IconKind,
    pub path: FsPath,
    pub size: u32,
    pub gen: u64,
    pub row: u32,
}

#[derive(Clone, Debug)]
pub struct Bitmap {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Debug)]
pub enum Effect {
    Provider { provider: ProviderId, req: ProviderRequest },
    Resort { tab: TabId, gen: u64, snap: Arc<DirSnapshot>, view: ViewSpec },
    Icon(IconRequest),
    TypeName(Vec<u8>),
    SetClipboard(String),
    Paste,
    Window(WindowCmd),
    Cursor(CursorIcon),
    Title(String),
    NewWindow(Vec<Route>),
    Open(Open),
    Pinned(Vec<Location>),
    RefreshPlaces,
}

pub enum IconResult {
    Ready(Bitmap),
    Failed,
    Skipped,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum IconState {
    Pending,
    Failed,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Persisted {
    pub path: Location,
    pub view: ViewSpec,
    pub col_w: [f32; 3],
}

impl Default for Persisted {
    fn default() -> Self {
        Self { path: Location::computer(), view: ViewSpec::default(), col_w: [150.0, 140.0, 90.0] }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct LoadStats {
    pub first: Option<Duration>,
    pub done: Option<Duration>,
    pub entries: usize,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Focus {
    List,
    Address,
    Filter,
    Sidebar,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Hit {
    None,
    Tab(usize),
    TabClose(usize),
    NewTab,
    Nav(usize),
    Caption(usize),
    Crumb(usize),
    Address,
    Header(usize),
    ColSep(usize),
    Row(usize),
    ListEmpty,
    Track,
    Thumb,
    Sizes,
    Side(usize),
    SidePin(usize),
    SideEmpty,
    Splitter,
    FilterField,
    FilterClose,
    PaletteRow(usize),
    PaletteField,
    PaletteBox,
    PaletteOutside,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Drag {
    None,
    Thumb { grab: f32 },
    ColSep { col: usize, start_x: f32, start_w: f32 },
    EditSelect,
    Splitter { start_x: f32, start_w: f32 },
    PinMove { from: usize, to: usize },
}

#[derive(Default)]
struct Layout {
    title: Rect,
    tabs: Vec<(Rect, Rect)>,
    new_tab: Rect,
    caption: [Rect; 3],
    toolbar: Rect,
    nav: [Rect; 4],
    address: Rect,
    header: Rect,
    cols: [Rect; 4],
    list: Rect,
    track: Rect,
    thumb: Rect,
    status: Rect,
    sizes: Rect,
    crumbs: Vec<Rect>,
    row_h: f32,
}

pub struct App {
    pub utc_offset_min: i32,
    pub case: CaseMode,
    pub open_target: OpenTarget,
    dark: bool,
    scale: f32,
    size: (f32, f32),
    maximized: bool,
    active: bool,

    tabs: Vec<Tab>,
    cur: usize,
    next_tab: TabId,
    next_gen: u64,
    col_w: [f32; 3],

    focus: Focus,
    edit: LineEdit,
    editing: bool,
    edit_scroll: f32,
    mods: Mods,
    hover: Hit,
    pressed: Hit,
    drag: Drag,
    pending_edit: Option<(f32, bool)>,
    cursor: CursorIcon,

    icons: FxHashMap<u64, IconState>,
    type_names: FxHashMap<Vec<u8>, Option<String>>,

    layout: Layout,
    effects: Vec<Effect>,
    arena: Bump,
    last_frame: Option<Instant>,
    dirty: bool,
    nav: nav::Nav,
}

impl App {
    pub fn new(state: Persisted, start: Vec<Route>, tab_base: TabId, dark: bool, scale: f32, size: (u32, u32), now: Instant) -> Self {
        let mut app = Self {
            utc_offset_min: 0,
            case: CaseMode::Insensitive,
            open_target: OpenTarget::NewTab,
            dark,
            scale,
            size: (size.0 as f32, size.1 as f32),
            maximized: false,
            active: true,
            tabs: Vec::new(),
            cur: 0,
            next_tab: tab_base + 1,
            next_gen: 0,
            col_w: state.col_w,
            focus: Focus::List,
            edit: LineEdit::default(),
            editing: false,
            edit_scroll: 0.0,
            mods: Mods::default(),
            hover: Hit::None,
            pressed: Hit::None,
            drag: Drag::None,
            pending_edit: None,
            cursor: CursorIcon::Default,
            icons: FxHashMap::default(),
            type_names: FxHashMap::default(),
            layout: Layout::default(),
            effects: Vec::new(),
            arena: Bump::new(),
            last_frame: None,
            dirty: true,
            nav: nav::Nav::default(),
        };
        let mut start = if start.is_empty() { vec![Route { folder: state.path, select: None }] } else { start };
        let total = start.len();
        start.truncate(MAX_ROUTES);
        for r in start {
            app.add_tab(r, state.view, now);
        }
        app.report_dropped(total);
        app.layout();
        app
    }

    fn report_dropped(&mut self, total: usize) {
        if total > MAX_ROUTES {
            self.tab_mut().error = Some(format!("Opened the first {MAX_ROUTES} of {total} locations"));
        }
    }

    pub fn shutdown(&mut self) {
        for t in &mut self.tabs {
            t.shutdown(&mut self.effects);
        }
    }

    pub fn persisted(&self) -> Persisted {
        let t = self.tab();
        Persisted { path: t.location().clone(), view: t.view_spec, col_w: self.col_w }
    }

    pub fn tab(&self) -> &Tab {
        &self.tabs[self.cur]
    }

    fn tab_mut(&mut self) -> &mut Tab {
        &mut self.tabs[self.cur]
    }

    pub fn tabs(&self) -> &[Tab] {
        &self.tabs
    }

    pub fn current_location(&self) -> &Location {
        self.tab().location()
    }

    pub fn needs_redraw(&self) -> bool {
        self.dirty
    }

    pub fn drain_effects(&mut self) -> std::vec::Drain<'_, Effect> {
        self.effects.drain(..)
    }

    pub fn has_effects(&self) -> bool {
        !self.effects.is_empty()
    }

    pub fn set_maximized(&mut self, m: bool) {
        if self.maximized != m {
            self.maximized = m;
            self.dirty = true;
        }
    }

    pub fn visible_rows(&self) -> (u64, u32, u32) {
        let t = self.tab();
        let (a, b) = t.row_range(self.layout.row_h, self.layout.list.h);
        (t.gen, a as u32, b as u32)
    }

    pub fn ime_area(&self) -> Option<Rect> {
        self.editing.then_some(self.layout.address).or_else(|| self.nav_ime_area())
    }

    fn s(&self, v: f32) -> f32 {
        (v * self.scale).round()
    }

    fn theme(&self) -> &'static Theme {
        theme::get(self.dark)
    }

    fn ui_font(&self, px: f32) -> FontStyle {
        FontStyle::new(FontFamily::Ui, px * self.scale, false)
    }

    fn icon_font(&self, px: f32) -> FontStyle {
        FontStyle::new(FontFamily::Icons, px * self.scale, false)
    }

    fn gen(&mut self) -> u64 {
        self.next_gen += 1;
        self.next_gen
    }

    pub fn open(&mut self, cmd: &Open, shell_folder: &dyn Fn(&str) -> Option<Location>, now: Instant) {
        let (routes, errors) = open::routes(cmd, shell_folder);
        let target = if cmd.target == OpenTarget::Default { self.open_target } else { cmd.target };
        self.dispatch(routes, target, now);
        self.show_open_errors(&errors);
    }

    pub fn show_open_errors(&mut self, errors: &[ResolveError]) {
        if let Some(e) = errors.first() {
            self.tab_mut().error = Some(match e {
                ResolveError::UnknownShellLocation(s) => format!("Cannot open {s} in Flit"),
                ResolveError::Invalid(s) => format!("Invalid path: {s}"),
            });
            self.dirty = true;
        }
    }

    pub fn dispatch(&mut self, mut routes: Vec<Route>, target: OpenTarget, now: Instant) {
        if routes.is_empty() {
            return;
        }
        let total = routes.len();
        routes.truncate(MAX_ROUTES);
        match target {
            OpenTarget::NewWindow => self.effects.push(Effect::NewWindow(routes)),
            OpenTarget::CurrentTab => {
                let mut it = routes.into_iter();
                let first = it.next().unwrap();
                self.route_here(first, now);
                for r in it {
                    let view = self.tab().view_spec;
                    self.add_tab(r, view, now);
                }
            }
            OpenTarget::NewTab | OpenTarget::Default => {
                for r in routes {
                    let view = self.tab().view_spec;
                    self.add_tab(r, view, now);
                }
            }
        }
        self.report_dropped(total);
        self.dirty = true;
    }

    fn route_here(&mut self, r: Route, now: Instant) {
        let gen = self.gen();
        let case = self.case;
        let (rh, lh) = (self.layout.row_h, self.layout.list.h);
        let t = &mut self.tabs[self.cur];
        if !t.history.go(r.folder) {
            let current = t.gen;
            if let (Some(name), Some(v)) = (&r.select, t.view.as_mut().filter(|v| v.snap().gen == current)) {
                if let Some(i) = v.find(name).filter(|&i| v.is_visible(i)) {
                    v.clear_selection();
                    v.set_selected(i, true);
                    v.focus = Some(i);
                    v.anchor = Some(i);
                    t.reveal_focus(true, rh, lh);
                    return;
                }
            }
        }
        t.load(gen, r.select, case, now, &mut self.effects);
        self.after_tab_change();
    }

    fn add_tab(&mut self, r: Route, view: ViewSpec, now: Instant) {
        let id = self.next_tab;
        self.next_tab += 1;
        let gen = self.gen();
        let mut t = Tab::new(id, r.folder, view);
        t.load(gen, r.select, self.case, now, &mut self.effects);
        self.tabs.push(t);
        self.cur = self.tabs.len() - 1;
        self.after_tab_change();
    }

    fn after_tab_change(&mut self) {
        self.editing = false;
        self.focus = Focus::List;
        self.effects.push(Effect::Title(format!("{} — Flit", self.tab().title())));
        self.dirty = true;
    }

    fn select_tab(&mut self, i: usize) {
        if i < self.tabs.len() && i != self.cur {
            self.cur = i;
            self.after_tab_change();
        }
    }

    fn close_tab(&mut self, i: usize) {
        if i >= self.tabs.len() {
            return;
        }
        if self.tabs.len() == 1 {
            self.effects.push(Effect::Window(WindowCmd::Close));
            return;
        }
        let mut t = self.tabs.remove(i);
        t.shutdown(&mut self.effects);
        self.remember_closed(t.location().clone());
        if self.cur > i || self.cur >= self.tabs.len() {
            self.cur = self.cur.saturating_sub(1);
        }
        self.after_tab_change();
    }

    fn navigate(&mut self, to: Location, now: Instant) {
        self.dispatch(vec![Route { folder: to, select: None }], OpenTarget::CurrentTab, now);
    }

    fn go_up(&mut self, now: Instant) {
        let cur = self.current_location().clone();
        if let Some(parent) = cur.parent() {
            self.dispatch(vec![Route { folder: parent, select: Some(cur.file_name().to_vec()) }], OpenTarget::CurrentTab, now);
        }
    }

    fn go_history(&mut self, forward: bool, now: Instant) {
        let gen = self.gen();
        let case = self.case;
        let t = &mut self.tabs[self.cur];
        let from = t.location().clone();
        let moved = if forward { t.history.forward().is_some() } else { t.history.back().is_some() };
        if moved {
            let select = (from.parent().as_ref() == Some(t.location())).then(|| from.file_name().to_vec());
            t.load(gen, select, case, now, &mut self.effects);
            self.after_tab_change();
        }
    }

    pub fn refresh(&mut self, now: Instant) {
        self.reload(self.cur, now);
    }

    fn reload(&mut self, k: usize, now: Instant) {
        let gen = self.gen();
        let t = &mut self.tabs[k];
        let select = t.focused_name();
        t.load(gen, select, self.case, now, &mut self.effects);
        self.dirty |= k == self.cur;
    }

    fn open_entry(&mut self, i: u32, now: Instant) {
        let t = self.tab();
        let e = t.view.as_ref().unwrap().listing().entries[i as usize];
        let loc = t.entry_location(i);
        if e.is_dir() {
            self.navigate(loc, now);
        } else {
            self.effects.push(Effect::Provider { provider: loc.provider(), req: ProviderRequest::Open { tab: t.id, path: loc.path().clone() } });
        }
    }

    fn open_selection(&mut self, now: Instant) {
        let t = self.tab();
        let Some(v) = &t.view else { return };
        let is_dir = |i: u32| v.listing().entries[i as usize].is_dir();
        if let Some(f) = v.focus.filter(|&f| is_dir(f)) {
            return self.open_entry(f, now);
        }
        let items: Vec<u32> = t.selection().into_iter().take(MAX_OPEN).collect();
        if let Some(&d) = items.iter().find(|&&i| is_dir(i)) {
            return self.open_entry(d, now);
        }
        for i in items {
            self.open_entry(i, now);
        }
    }

    pub fn poll(&mut self) -> bool {
        let mut visible = false;
        for k in 0..self.tabs.len() {
            let Some(snap) = self.tabs[k].slot.take() else { continue };
            let resort = self.tabs[k].on_snapshot(snap);
            if resort {
                self.resort(k);
            }
            visible |= k == self.cur;
        }
        self.dirty |= visible;
        visible
    }

    pub fn on_provider_event(&mut self, ev: ProviderEvent, now: Instant) {
        let tab_of = |tabs: &[Tab], id: TabId, gen: u64| tabs.iter().position(|t| t.id == id && t.gen == gen);
        match ev {
            ProviderEvent::Changes { tab, gen, changes } => {
                if let Some(k) = tab_of(&self.tabs, tab, gen) {
                    self.tabs[k].pending_diffs.extend(changes);
                    if self.tabs[k].pending_diffs.len() > MAX_PENDING_DIFFS {
                        return self.on_provider_event(ProviderEvent::Rescan { tab, gen }, now);
                    }
                    self.dirty |= k == self.cur;
                }
            }
            ProviderEvent::Rescan { tab, gen } => {
                if let Some(k) = tab_of(&self.tabs, tab, gen) {
                    self.reload(k, now);
                }
            }
            ProviderEvent::DirSize { tab, gen, idx, size } => {
                if let Some(k) = tab_of(&self.tabs, tab, gen) {
                    if let Some(v) = self.tabs[k].view.as_mut().filter(|v| v.snap().gen == gen) {
                        v.set_dir_size(idx, size);
                    }
                    self.dirty |= k == self.cur;
                }
            }
            ProviderEvent::OpDone { tab, error: Some(message) } => {
                let k = self.tabs.iter().position(|t| t.id == tab).unwrap_or(self.cur);
                self.tabs[k].error = Some(message);
                self.dirty |= k == self.cur;
            }
            ProviderEvent::OpDone { tab, error: None } => {
                let unwatched = |l: &Location| matches!(l, Location::Wsl { .. }) || l.path().kind() == PathKind::Unc;
                if let Some(k) = self.tabs.iter().position(|t| t.id == tab && unwatched(t.location())) {
                    self.reload(k, now);
                }
            }
        }
    }

    pub fn on_resorted(&mut self, tab: TabId, gen: u64, view: ViewSpec, base_len: usize, order: Vec<u32>) {
        if let Some(k) = self.tabs.iter().position(|t| t.id == tab && t.gen == gen && t.view_spec == view) {
            if let Some(v) = &mut self.tabs[k].view {
                v.merge_order(order, view, base_len);
                self.dirty |= k == self.cur;
            }
        }
    }

    pub fn on_icon<B: TextBackend>(&mut self, gfx: &mut Gfx<B>, key: u64, result: IconResult) {
        match result {
            IconResult::Ready(bm) => {
                self.icons.remove(&key);
                gfx.add_image(key, bm.width, bm.height, bm.rgba);
            }
            IconResult::Failed => {
                self.icons.insert(key, IconState::Failed);
            }
            IconResult::Skipped => {
                self.icons.remove(&key);
            }
        }
        self.dirty = true;
    }

    pub fn on_type_name(&mut self, ext: Vec<u8>, name: Option<String>) {
        self.type_names.insert(ext, Some(name.unwrap_or_default()));
        self.dirty = true;
    }

    pub fn on_paste(&mut self, text: &str) {
        if !self.editing && self.nav_insert(text.lines().next().unwrap_or("")) {
            return;
        }
        if self.editing {
            self.edit.insert(text.lines().next().unwrap_or(""));
            self.dirty = true;
        }
    }

    fn resort(&mut self, k: usize) {
        let t = &self.tabs[k];
        if let Some(v) = &t.view {
            self.effects.push(Effect::Resort { tab: t.id, gen: t.gen, snap: v.snap().clone(), view: t.view_spec });
        }
    }

    fn layout(&mut self) {
        let (w, h) = self.size;
        let (cx, filter_h) = self.content_origin();
        let sc = self.scale;
        let s = |v: f32| (v * sc).round();
        let l = &mut self.layout;
        l.row_h = s(ROW_H);
        l.title = Rect::new(0.0, 0.0, w, s(TAB_H));
        let cw = s(CAPTION_W);
        for (i, r) in l.caption.iter_mut().enumerate() {
            *r = Rect::new(w - (3 - i) as f32 * cw, 0.0, cw, s(32.0));
        }
        let n = self.tabs.len().max(1) as f32;
        let x0 = s(8.0);
        let add_w = s(32.0);
        let avail = (l.caption[0].x - s(48.0) - x0 - add_w).max(0.0);
        let tw = (avail / n).clamp(s(TAB_MIN_W), s(TAB_MAX_W));
        l.tabs.clear();
        for i in 0..self.tabs.len() {
            let r = Rect::new(x0 + i as f32 * tw, s(6.0), tw, l.title.h - s(6.0));
            let c = Rect::new(r.right() - s(28.0), r.y + (r.h - s(20.0)) / 2.0, s(20.0), s(20.0));
            l.tabs.push((r, c));
        }
        let after = x0 + self.tabs.len() as f32 * tw;
        l.new_tab = Rect::new(after + s(4.0), s(8.0), add_w, l.title.h - s(12.0));

        l.toolbar = Rect::new(0.0, l.title.bottom(), w, s(TOOL_H));
        for (i, r) in l.nav.iter_mut().enumerate() {
            *r = Rect::new(s(6.0) + i as f32 * s(NAV_W), l.toolbar.y + s(4.0), s(NAV_W), s(TOOL_H - 8.0));
        }
        let ax = l.nav[3].right() + s(8.0);
        l.address = Rect::new(ax, l.toolbar.y + s(5.0), (w - s(12.0) - ax).max(s(40.0)), s(TOOL_H - 10.0));
        l.header = Rect::new(cx, l.toolbar.bottom() + filter_h, w - cx, s(HEADER_H));
        l.status = Rect::new(0.0, h - s(STATUS_H), w, s(STATUS_H));
        l.list = Rect::new(cx, l.header.bottom(), (w - cx - s(SCROLLBAR_W)).max(0.0), (l.status.y - l.header.bottom()).max(0.0));
        l.track = Rect::new(l.list.right(), l.list.y, s(SCROLLBAR_W), l.list.h);
        let fixed: f32 = self.col_w.iter().map(|&c| s(c)).sum();
        let name_w = (l.list.w - fixed).max(s(NAME_MIN_W));
        let mut x = cx;
        for (i, r) in l.cols.iter_mut().enumerate() {
            let cw = if i == 0 { name_w } else { s(self.col_w[i - 1]) };
            *r = Rect::new(x, l.header.y, cw, l.header.h);
            x += cw;
        }
        let (rh, lh) = (l.row_h, l.list.h);
        let t = &mut self.tabs[self.cur];
        if std::mem::take(&mut t.reveal_pending) {
            t.reveal_focus(true, rh, lh);
        }
        let max_scroll = t.max_scroll(rh, lh);
        t.scroll_target = t.scroll_target.clamp(0.0, max_scroll);
        t.scroll = t.scroll.clamp(0.0, max_scroll);
        let content = t.rows() as f32 * rh;
        l.thumb = if content > lh {
            let th = (l.track.h * lh / content).max(s(24.0)).min(l.track.h);
            let ty = l.track.y + (l.track.h - th) * (t.scroll / max_scroll);
            Rect::new(l.track.x + s(3.0), ty, l.track.w - s(6.0), th)
        } else {
            Rect::default()
        };
        self.layout_nav();
    }

    fn hit(&self, x: f32, y: f32) -> Hit {
        if let Some(h) = self.nav_hit(x, y) {
            return h;
        }
        let l = &self.layout;
        if l.title.contains(x, y) {
            if let Some(i) = l.caption.iter().position(|r| r.contains(x, y)) {
                return Hit::Caption(i);
            }
            for (i, (r, c)) in l.tabs.iter().enumerate() {
                if c.contains(x, y) {
                    return Hit::TabClose(i);
                }
                if r.contains(x, y) {
                    return Hit::Tab(i);
                }
            }
            if l.new_tab.contains(x, y) {
                return Hit::NewTab;
            }
            return Hit::None;
        }
        if l.toolbar.contains(x, y) {
            if let Some(i) = l.nav.iter().position(|r| r.contains(x, y)) {
                return Hit::Nav(i);
            }
            if l.address.contains(x, y) {
                if !self.editing {
                    if let Some(i) = l.crumbs.iter().position(|r| r.contains(x, y)) {
                        return Hit::Crumb(i);
                    }
                }
                return Hit::Address;
            }
            return Hit::None;
        }
        if l.header.contains(x, y) {
            let grip = self.s(4.0);
            for i in 1..4 {
                if (x - l.cols[i].x).abs() <= grip {
                    return Hit::ColSep(i);
                }
            }
            return l.cols.iter().position(|r| r.contains(x, y)).map_or(Hit::None, Hit::Header);
        }
        if l.track.contains(x, y) {
            let th = l.thumb;
            return if y >= th.y && y < th.bottom() && th.h > 0.0 { Hit::Thumb } else { Hit::Track };
        }
        if l.list.contains(x, y) {
            let t = self.tab();
            let row = ((y - l.list.y + t.scroll) / l.row_h).floor() as usize;
            return if row < t.rows() { Hit::Row(row) } else { Hit::ListEmpty };
        }
        if l.sizes.contains(x, y) {
            return Hit::Sizes;
        }
        Hit::None
    }

    pub fn hit_test(&self, x: f32, y: f32) -> HitArea {
        match self.hit(x, y) {
            Hit::Caption(0) => HitArea::Minimize,
            Hit::Caption(1) => HitArea::Maximize,
            Hit::Caption(_) => HitArea::Close,
            Hit::None if self.layout.title.contains(x, y) => HitArea::Caption,
            _ => HitArea::Client,
        }
    }

    pub fn on_event(&mut self, ev: &Event, now: Instant) {
        match ev {
            Event::Resized { width, height } => {
                self.size = (*width as f32, *height as f32);
                self.layout();
                self.dirty = true;
            }
            Event::ScaleChanged(s) => {
                self.scale = *s;
                self.layout();
                self.dirty = true;
            }
            Event::ThemeChanged { dark } => {
                self.dark = *dark;
                self.dirty = true;
            }
            Event::Focus(f) => {
                self.active = *f;
                self.mods = Mods::default();
                self.dirty = true;
            }
            Event::MouseMove { x, y } => self.mouse_move(*x, *y),
            Event::MouseLeave => {
                if self.hover != Hit::None {
                    self.hover = Hit::None;
                    self.dirty = true;
                }
            }
            Event::MouseDown { x, y, button, clicks } => self.mouse_down(*x, *y, *button, *clicks, now),
            Event::MouseUp { x, y, button } => self.mouse_up(*x, *y, *button, now),
            Event::Wheel { dy, .. } => {
                if self.nav_wheel(*dy) {
                    return;
                }
                let (rh, lh) = (self.layout.row_h, self.layout.list.h);
                let t = self.tab_mut();
                t.scroll_target = (t.scroll_target - dy * 3.0 * rh).clamp(0.0, t.max_scroll(rh, lh));
                self.dirty = true;
            }
            Event::Key { key, mods, pressed } => {
                self.mods = *mods;
                if *pressed {
                    self.key(*key, *mods, now);
                }
            }
            Event::Char(c) => self.char_input(*c, now),
            Event::DroppedFiles(paths) => {
                let routes: Vec<Route> = paths
                    .iter()
                    .map(|p| {
                        let item = Location::from_local(p.clone());
                        match item.parent() {
                            Some(folder) => Route { select: Some(item.file_name().to_vec()), folder },
                            None => Route { folder: item, select: None },
                        }
                    })
                    .take(1)
                    .collect();
                self.dispatch(routes, OpenTarget::CurrentTab, now);
            }
            Event::CloseRequested => self.effects.push(Effect::Window(WindowCmd::Close)),
            Event::DevicesChanged => self.effects.push(Effect::RefreshPlaces),
        }
    }

    fn set_cursor(&mut self, c: CursorIcon) {
        if self.cursor != c {
            self.cursor = c;
            self.effects.push(Effect::Cursor(c));
        }
    }

    fn mouse_move(&mut self, x: f32, y: f32) {
        let (rh, lh) = (self.layout.row_h, self.layout.list.h);
        match self.drag {
            Drag::Thumb { grab } => {
                let l = &self.layout;
                let range = (l.track.h - l.thumb.h).max(1.0);
                let frac = ((y - grab - l.track.y) / range).clamp(0.0, 1.0);
                let t = self.tab_mut();
                t.scroll = frac * t.max_scroll(rh, lh);
                t.scroll_target = t.scroll;
                self.layout();
                self.dirty = true;
                return;
            }
            Drag::ColSep { col, start_x, start_w } => {
                let w = (start_w - (x - start_x) / self.scale).clamp(40.0, 600.0);
                self.col_w[col - 1] = w.round();
                self.layout();
                self.dirty = true;
                return;
            }
            Drag::EditSelect => {
                self.pending_edit = Some((x, true));
                self.dirty = true;
                return;
            }
            Drag::Splitter { start_x, start_w } => return self.drag_splitter(x, start_x, start_w),
            Drag::PinMove { from, .. } => return self.drag_pin(from, y),
            Drag::None => {}
        }
        if self.start_pin_drag(y) {
            return;
        }
        let h = self.hit(x, y);
        let cursor = match h {
            Hit::ColSep(_) | Hit::Splitter => CursorIcon::ResizeHorizontal,
            Hit::Address if self.editing => CursorIcon::Text,
            Hit::FilterField | Hit::PaletteField => CursorIcon::Text,
            _ => CursorIcon::Default,
        };
        self.set_cursor(cursor);
        if h != self.hover {
            self.hover = h;
            self.dirty = true;
        }
    }

    fn mouse_down(&mut self, x: f32, y: f32, button: MouseButton, clicks: u8, now: Instant) {
        self.dirty = true;
        let h = self.hit(x, y);
        if self.nav_mouse_down(h, (x, y), button, clicks, now) {
            return;
        }
        match (button, h) {
            (MouseButton::Back, _) => return self.go_history(false, now),
            (MouseButton::Forward, _) => return self.go_history(true, now),
            (MouseButton::Middle, Hit::Tab(i) | Hit::TabClose(i)) => return self.close_tab(i),
            (MouseButton::Middle, _) => return,
            _ => {}
        }
        self.pressed = h;
        if self.editing && h != Hit::Address {
            self.cancel_edit();
        }
        let (rh, lh) = (self.layout.row_h, self.layout.list.h);
        match h {
            Hit::Tab(i) => self.select_tab(i),
            Hit::Address => {
                if !self.editing {
                    self.begin_edit();
                } else if clicks >= 2 {
                    self.edit.select_all();
                } else {
                    self.pending_edit = Some((x, self.mods.shift));
                    self.drag = Drag::EditSelect;
                }
            }
            Hit::Row(row) => {
                self.focus = Focus::List;
                self.click_row(row, button, clicks, now);
            }
            Hit::ListEmpty => {
                self.focus = Focus::List;
                let ctrl = self.mods.ctrl;
                if let Some(v) = &mut self.tab_mut().view {
                    if !ctrl {
                        v.clear_selection();
                    }
                }
            }
            Hit::Thumb => self.drag = Drag::Thumb { grab: y - self.layout.thumb.y },
            Hit::Track => {
                let dir = if y < self.layout.thumb.y { -1.0 } else { 1.0 };
                let t = self.tab_mut();
                t.scroll_target = (t.scroll_target + dir * lh).clamp(0.0, t.max_scroll(rh, lh));
            }
            Hit::ColSep(col) => self.drag = Drag::ColSep { col, start_x: x, start_w: self.col_w[col - 1] },
            _ => {}
        }
    }

    fn mouse_up(&mut self, x: f32, y: f32, button: MouseButton, now: Instant) {
        let was = std::mem::replace(&mut self.drag, Drag::None);
        let pressed = std::mem::replace(&mut self.pressed, Hit::None);
        self.dirty = true;
        if let Drag::PinMove { from, to } = was {
            return self.drop_pin(from, to);
        }
        if button != MouseButton::Left || was != Drag::None {
            return;
        }
        let h = self.hit(x, y);
        if h != pressed {
            return;
        }
        match h {
            Hit::TabClose(i) => self.close_tab(i),
            Hit::NewTab => self.new_tab_here(now),
            Hit::Nav(0) => self.go_history(false, now),
            Hit::Nav(1) => self.go_history(true, now),
            Hit::Nav(2) => self.go_up(now),
            Hit::Nav(_) => self.refresh(now),
            Hit::Caption(0) => self.effects.push(Effect::Window(WindowCmd::Minimize)),
            Hit::Caption(1) => self.effects.push(Effect::Window(WindowCmd::ToggleMaximize)),
            Hit::Caption(_) => self.effects.push(Effect::Window(WindowCmd::Close)),
            Hit::Crumb(i) => {
                if let Some((_, p)) = self.tab().crumbs.get(i).cloned() {
                    self.navigate(p, now);
                }
            }
            Hit::Header(c) => {
                let field = COLUMNS[c].field();
                let t = self.tab_mut();
                let s = &mut t.view_spec.sort;
                *s = if s.field == field { SortSpec { field, descending: !s.descending } } else { SortSpec { field, descending: false } };
                let k = self.cur;
                self.resort(k);
            }
            Hit::Sizes => self.compute_dir_sizes(),
            Hit::Side(i) => self.open_side_item(i, self.mods.ctrl, now),
            Hit::SidePin(i) => self.toggle_pin_item(i),
            Hit::FilterClose => self.close_filter(),
            Hit::PaletteRow(i) => self.run_palette_item(i, now),
            _ => {}
        }
    }

    fn new_tab_here(&mut self, now: Instant) {
        let folder = self.current_location().clone();
        self.dispatch(vec![Route { folder, select: None }], OpenTarget::NewTab, now);
    }

    fn click_row(&mut self, row: usize, button: MouseButton, clicks: u8, now: Instant) {
        let m = self.mods;
        let Some(v) = &mut self.tabs[self.cur].view else { return };
        let i = v.rows()[row];
        if button == MouseButton::Right {
            if !v.is_selected(i) {
                v.clear_selection();
                v.set_selected(i, true);
            }
            v.focus = Some(i);
            return;
        }
        if clicks >= 2 && !m.ctrl && !m.shift {
            return self.open_entry(i, now);
        }
        if m.shift {
            let anchor_row = v.anchor.and_then(|a| v.row_of(a)).unwrap_or(0);
            if !m.ctrl {
                v.clear_selection();
            }
            v.select_rows(anchor_row, row);
        } else if m.ctrl {
            let on = !v.is_selected(i);
            v.set_selected(i, on);
            v.anchor = Some(i);
        } else {
            v.clear_selection();
            v.set_selected(i, true);
            v.anchor = Some(i);
        }
        v.focus = Some(i);
    }

    fn page_rows(&self) -> isize {
        ((self.layout.list.h / self.layout.row_h.max(1.0)).floor() as isize).max(1)
    }

    fn key(&mut self, key: Key, m: Mods, now: Instant) {
        self.dirty = true;
        if self.editing {
            match self.edit.key(key, m) {
                EditAction::Submit => self.submit_edit(now),
                EditAction::Cancel => self.cancel_edit(),
                EditAction::Copy(s) => self.effects.push(Effect::SetClipboard(s)),
                EditAction::Paste => self.effects.push(Effect::Paste),
                EditAction::Changed | EditAction::None => {}
            }
            return;
        }
        if self.nav_key(key, m, now) {
            return;
        }
        let page = self.page_rows();
        let (rh, lh) = (self.layout.row_h, self.layout.list.h);
        let mut moved = None;
        match (key, m.ctrl, m.alt) {
            (Key::BrowserBack, ..) => self.go_history(false, now),
            (Key::BrowserForward, ..) => self.go_history(true, now),
            (Key::Char(c @ '1'..='9'), true, false) => {
                let i = if c == '9' { self.tabs.len() - 1 } else { c as usize - '1' as usize };
                self.select_tab(i);
            }
            (Key::Space, true, false) => {
                if let Some(v) = &mut self.tab_mut().view {
                    if let Some(f) = v.focus {
                        let on = !v.is_selected(f);
                        v.set_selected(f, on);
                    }
                }
            }
            (Key::Up, _, false) => moved = Some(-1),
            (Key::Down, _, false) => moved = Some(1),
            (Key::PageUp, _, false) => moved = Some(-page),
            (Key::PageDown, _, false) => moved = Some(page),
            (Key::Home, _, false) => moved = Some(isize::MIN / 2),
            (Key::End, _, false) => moved = Some(isize::MAX / 2),
            _ => {}
        }
        if let Some(d) = moved {
            let t = self.tab_mut();
            t.move_focus(d, m);
            t.reveal_focus(false, rh, lh);
        }
    }

    fn char_input(&mut self, c: char, now: Instant) {
        if c.is_control() {
            return;
        }
        self.dirty = true;
        if self.editing {
            let mut b = [0; 4];
            self.edit.insert(c.encode_utf8(&mut b));
            return;
        }
        if self.nav_insert(c.encode_utf8(&mut [0; 4])) {
            return;
        }
        if self.mods.ctrl != self.mods.alt {
            return;
        }
        let (rh, lh) = (self.layout.row_h, self.layout.list.h);
        let t = self.tab_mut();
        if t.type_ahead(c, now) {
            t.reveal_focus(false, rh, lh);
        }
    }

    fn begin_edit(&mut self) {
        self.editing = true;
        self.focus = Focus::Address;
        let loc = self.current_location();
        let text = if loc.path().kind() == PathKind::Computer { String::new() } else { loc.display().into_owned() };
        self.edit.set(&text);
        self.edit.select_all();
        self.edit_scroll = 0.0;
        self.dirty = true;
    }

    fn cancel_edit(&mut self) {
        self.editing = false;
        self.focus = Focus::List;
        self.drag = Drag::None;
        self.set_cursor(CursorIcon::Default);
    }

    fn submit_edit(&mut self, now: Instant) {
        let cwd = self.current_location().to_windows().map(|p| p.as_bytes().to_vec());
        let cmd = Open { paths: vec![self.edit.text.as_bytes().to_vec()], target: OpenTarget::CurrentTab, cwd, ..Default::default() };
        self.cancel_edit();
        if Location::parse(&self.edit.text).as_ref() == Some(self.current_location()) {
            return self.refresh(now);
        }
        self.effects.push(Effect::Open(cmd));
        self.dirty = true;
    }

    fn compute_dir_sizes(&mut self) {
        let t = self.tab();
        if t.view.as_ref().is_none_or(|v| v.snap().gen != t.gen) {
            return;
        }
        let items = t.size_items();
        if items.is_empty() {
            return;
        }
        let loc = t.location();
        let req = ProviderRequest::DirSizes { tab: t.id, gen: t.gen, dir: loc.path().clone(), items, cancel: t.cancel.clone() };
        self.effects.push(Effect::Provider { provider: loc.provider(), req });
    }

    fn copy_paths(&mut self) {
        let t = self.tab();
        let sel = t.selection();
        let text = if sel.is_empty() {
            t.location().display().into_owned()
        } else {
            sel.iter().map(|&i| t.entry_location(i).display().into_owned()).collect::<Vec<_>>().join("\r\n")
        };
        self.effects.push(Effect::SetClipboard(text));
    }

    fn trash_selection(&mut self) {
        let t = self.tab();
        let Some(v) = t.view.as_ref().filter(|_| t.view_loc.path().kind() != PathKind::Computer) else { return };
        let paths: Vec<FsPath> = v.selected().map(|i| t.entry_location(i).path().clone()).collect();
        if !paths.is_empty() {
            let provider = t.view_loc.provider();
            self.effects.push(Effect::Provider { provider, req: ProviderRequest::Op { tab: t.id, op: FileOp::Trash(paths) } });
        }
    }

    pub fn frame<B: TextBackend>(&mut self, g: &mut Gfx<B>, now: Instant) -> bool {
        let _span = tracing::trace_span!("ui_frame").entered();
        let dt = self.last_frame.map_or(0.0, |t| now.duration_since(t).as_secs_f32()).min(0.1);
        self.last_frame = Some(now);
        self.poll();
        self.dirty = false;
        let mut animating = self.tab_mut().apply_diffs(DIFF_BUDGET);

        let t = self.tab_mut();
        let diff = t.scroll_target - t.scroll;
        if diff.abs() > 0.5 {
            t.scroll += diff * (1.0 - (-dt / SCROLL_TAU).exp()).max(0.15);
            animating = true;
        } else {
            t.scroll = t.scroll_target;
        }
        self.layout();

        let mut arena = std::mem::take(&mut self.arena);
        let th = self.theme();
        for _ in 0..2 {
            arena.reset();
            g.begin(th.bg);
            self.draw_list(g, &arena, th);
            self.draw_header(g, th);
            self.draw_title(g, th);
            self.draw_toolbar(g, th);
            self.draw_status(g, &arena, th);
            self.draw_nav(g, th);
            if !g.atlas_reset {
                break;
            }
        }
        self.arena = arena;

        let t = self.tab();
        if let Some(l) = &t.loading {
            if t.view.as_ref().is_none_or(|v| v.snap().gen != l.gen) && now.duration_since(l.since) < Duration::from_millis(300) {
                animating = true;
            }
        }
        animating || g.atlas_reset
    }

    fn draw_title<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme) {
        let l = &self.layout;
        g.dl.rect(l.title, t.chrome);
        let font = self.ui_font(12.5);
        let m = g.metrics(font);
        let close_font = self.icon_font(8.0);
        for (i, &(r, c)) in l.tabs.iter().enumerate() {
            let active = i == self.cur;
            let hovered = matches!(self.hover, Hit::Tab(k) | Hit::TabClose(k) if k == i);
            if active {
                g.dl.round_rect(Rect::new(r.x, r.y, r.w, r.h + self.s(8.0)), t.bg, self.s(6.0));
            } else if hovered {
                g.dl.round_rect(r.inset(0.0, self.s(2.0)), t.button_hover, self.s(6.0));
            }
            let baseline = (r.y + (r.h + m.ascent - m.descent) / 2.0).round();
            let label = self.tabs[i].title();
            let color = if active { t.text } else { t.text_dim };
            g.text(r.x + self.s(12.0), baseline, &label, font, color, c.x - r.x - self.s(16.0));
            if active || hovered {
                if self.hover == Hit::TabClose(i) {
                    g.dl.round_rect(c, t.button_hover, self.s(4.0));
                }
                centered(g, c, G_CLOSE, close_font, t.text_dim);
            }
        }
        if self.hover == Hit::NewTab {
            g.dl.round_rect(l.new_tab, t.button_hover, self.s(4.0));
        }
        centered(g, l.new_tab, G_ADD, self.icon_font(10.0), t.text_dim);

        let caption_icons = self.icon_font(10.0);
        let max_glyph = if self.maximized { G_RESTORE } else { G_MAX };
        for (i, glyph) in [G_MIN, max_glyph, G_CLOSE].into_iter().enumerate() {
            let r = l.caption[i];
            let mut color = if self.active { t.text } else { t.text_dim };
            if self.hover == Hit::Caption(i) {
                g.dl.rect(r, if i == 2 { t.close_hover } else { t.button_hover });
                if i == 2 {
                    color = Color::rgb(0xFFFFFF);
                }
            }
            centered(g, r, glyph, caption_icons, color);
        }
    }

    fn draw_toolbar<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme) {
        let l = &self.layout;
        g.dl.rect(l.toolbar, t.bg);
        let icons = self.icon_font(12.0);
        let tab = self.tab();
        let enabled = [tab.history.can_back(), tab.history.can_forward(), tab.location().parent().is_some(), true];
        for (i, glyph) in [G_BACK, G_FORWARD, G_UP, G_REFRESH].into_iter().enumerate() {
            let r = l.nav[i];
            if self.hover == Hit::Nav(i) && enabled[i] {
                let c = if self.pressed == Hit::Nav(i) { t.button_pressed } else { t.button_hover };
                g.dl.round_rect(r.inset(self.s(2.0), 0.0), c, self.s(4.0));
            }
            let color = if enabled[i] { t.text } else { t.text_dim.alpha(0x70) };
            centered(g, r, glyph, icons, color);
        }
        self.draw_address(g, t);
    }

    fn draw_address<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme) {
        let r = self.layout.address;
        let radius = self.s(4.0);
        g.dl.round_rect(r, t.input_bg, radius);
        g.dl.outline(r, if self.editing { t.accent } else { t.input_border }, radius, self.s(1.0));
        let font = self.ui_font(13.0);
        let m = g.metrics(font);
        let baseline = (r.y + (r.h + m.ascent - m.descent) / 2.0).round();
        let pad = self.s(8.0);
        let inner = Rect::new(r.x + pad, r.y, r.w - 2.0 * pad, r.h);
        g.dl.set_clip(Some(inner));
        if self.editing {
            if let Some((x, extend)) = self.pending_edit.take() {
                let pos = g.hit(&self.edit.text, font, x - inner.x + self.edit_scroll);
                self.edit.move_to(pos, extend);
            }
            let caret_x = g.caret_x(&self.edit.text, font, self.edit.caret);
            if caret_x - self.edit_scroll > inner.w {
                self.edit_scroll = caret_x - inner.w;
            } else if caret_x < self.edit_scroll {
                self.edit_scroll = caret_x;
            }
            let x0 = inner.x - self.edit_scroll;
            let sel = self.edit.selection();
            if !sel.is_empty() {
                let a = g.caret_x(&self.edit.text, font, sel.start);
                let b = g.caret_x(&self.edit.text, font, sel.end);
                g.dl.rect(Rect::new(x0 + a, r.y + self.s(5.0), b - a, r.h - self.s(10.0)), t.text_selection);
            }
            g.text(x0, baseline, &self.edit.text, font, t.text, f32::INFINITY);
            g.dl.rect(Rect::new(x0 + caret_x, r.y + self.s(6.0), self.s(1.0).max(1.0), r.h - self.s(12.0)), t.text);
        } else {
            self.draw_crumbs(g, t, inner, baseline, font);
        }
        g.dl.set_clip(None);
    }

    fn draw_crumbs<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme, inner: Rect, baseline: f32, font: FontStyle) {
        let chevron = self.icon_font(8.0);
        let gap = self.s(4.0);
        let chev_w = g.measure(G_CHEVRON, chevron) + 2.0 * gap;
        let crumbs = &self.tabs[self.cur].crumbs;
        let widths: Vec<f32> = crumbs.iter().map(|(label, _)| g.measure(label, font) + 2.0 * gap).collect();
        let mut first = 0;
        while first + 1 < widths.len() && widths[first..].iter().sum::<f32>() + chev_w * (widths.len() - first - 1) as f32 > inner.w {
            first += 1;
        }
        let (s1, s3, s6) = (self.s(1.0), self.s(3.0), self.s(6.0));
        let rects = &mut self.layout.crumbs;
        rects.clear();
        rects.resize(crumbs.len(), Rect::default());
        let mut x = inner.x - gap;
        for i in first..crumbs.len() {
            let w = widths[i];
            let cr = Rect::new(x, inner.y + s3, w, inner.h - s6);
            rects[i] = cr;
            if self.hover == Hit::Crumb(i) {
                g.dl.round_rect(cr, t.button_hover, s3);
            }
            let last = i + 1 == crumbs.len();
            g.text(x + gap, baseline, &crumbs[i].0, font, if last { t.text } else { t.text_dim }, inner.right() - x);
            x += w;
            if !last {
                g.text(x + gap, baseline - s1, G_CHEVRON, chevron, t.text_dim, f32::INFINITY);
                x += chev_w;
            }
        }
    }

    fn draw_header<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme) {
        let l = &self.layout;
        g.dl.rect(l.header, t.header);
        g.dl.rect(Rect::new(l.header.x, l.header.bottom() - 1.0, l.header.w, 1.0), t.separator);
        let font = self.ui_font(12.0);
        let m = g.metrics(font);
        let baseline = (l.header.y + (l.header.h + m.ascent - m.descent) / 2.0).round();
        let pad = self.s(10.0);
        let sort = self.tab().view_spec.sort;
        for (i, col) in COLUMNS.iter().enumerate() {
            let r = l.cols[i];
            if self.hover == Hit::Header(i) {
                g.dl.rect(r, t.button_hover);
            }
            if i > 0 {
                g.dl.rect(Rect::new(r.x, r.y + self.s(6.0), 1.0, r.h - self.s(12.0)), t.separator);
            }
            g.dl.set_clip(Some(r));
            let w = g.text(r.x + pad, baseline, col.title(), font, t.text_dim, r.w - 2.0 * pad);
            if sort.field == col.field() {
                let glyph = if sort.descending { G_SORT_DOWN } else { G_SORT_UP };
                g.text(r.x + pad + w + self.s(6.0), baseline, glyph, self.icon_font(8.0), t.text_dim, f32::INFINITY);
            }
            g.dl.set_clip(None);
        }
    }

    fn draw_list<B: TextBackend>(&mut self, g: &mut Gfx<B>, arena: &Bump, t: &Theme) {
        let l = &self.layout;
        let (list, cols, rh) = (l.list, l.cols, l.row_h);
        let font = self.ui_font(13.0);
        let m = g.metrics(font);
        let text_dy = ((rh + m.ascent - m.descent) / 2.0).round();
        let pad = self.s(10.0);
        let icon_px = self.s(ICON);
        let list_focused = self.active && self.focus == Focus::List;
        let tab = &self.tabs[self.cur];
        let (first, last) = tab.row_range(rh, list.h);
        g.dl.set_clip(Some(list));
        let mut requests: Vec<IconRequest> = Vec::new();
        let mut type_requests: Vec<Vec<u8>> = Vec::new();
        if let Some(v) = &tab.view {
            if v.rows().is_empty() && v.snap().done && v.snap().error.is_none() {
                let msg = if v.is_filtered() { "No items match the filter." } else { "This folder is empty." };
                let w = g.measure(msg, font);
                g.text(list.x + (list.w - w) / 2.0, list.y + self.s(40.0), msg, font, t.text_dim, f32::INFINITY);
            }
            let loc = &tab.view_loc;
            let shell_dir = loc.to_windows();
            let in_root = loc.path().kind() == PathKind::Computer;
            let listing = v.listing();
            for row in first..last {
                let i = v.rows()[row];
                let e = listing.entries[i as usize];
                let y = (list.y + row as f32 * rh - tab.scroll).round();
                let rr = Rect::new(list.x + self.s(4.0), y + 1.0, list.w - self.s(8.0), rh - 2.0);
                if v.is_selected(i) {
                    g.dl.round_rect(rr, if list_focused { t.selection } else { t.selection_inactive }, self.s(4.0));
                } else if self.hover == Hit::Row(row) && self.drag == Drag::None {
                    g.dl.round_rect(rr, t.hover, self.s(4.0));
                }
                if list_focused && v.focus == Some(i) {
                    g.dl.outline(rr, t.focus_ring, self.s(4.0), 1.0);
                }

                let name = listing.name(i);
                let dir_key = shell_dir.as_ref().map_or(&b""[..], |d| d.as_bytes());
                let (key, kind) = icon_key(name, listing.ext(i), &e, icon_px as u32, dir_key);
                let ir = Rect::new(cols[0].x + pad, y + ((rh - icon_px) / 2.0).round(), icon_px, icon_px);
                if !g.image(key, ir) {
                    let glyph = if e.attrs & attr::VOLUME != 0 { G_DRIVE } else if e.is_dir() { G_FOLDER } else { G_FILE };
                    let color = if e.is_dir() { t.accent.alpha(0xB0) } else { t.text_dim.alpha(0x90) };
                    centered(g, ir, glyph, FontStyle::new(FontFamily::Icons, icon_px * 0.85, false), color);
                    let wanted = !self.icons.contains_key(&key) && !requests.iter().any(|r| r.key == key);
                    if let (true, Some(dir)) = (wanted, &shell_dir) {
                        requests.push(IconRequest { key, kind, path: dir.join(name), size: icon_px as u32, gen: tab.gen, row: row as u32 });
                    }
                }

                let ty = y + text_dy;
                let name_x = ir.right() + self.s(8.0);
                let name_color = if e.attrs & attr::HIDDEN != 0 { t.text_dim } else { t.text };
                g.text(name_x, ty, &name::display(name), font, name_color, cols[0].right() - name_x - pad);

                let mut s = bumpalo::collections::String::new_in(arena);
                format::datetime(&mut s, e.modified, self.utc_offset_min);
                g.text(cols[1].x + pad, ty, &s, font, t.text_dim, cols[1].w - 2.0 * pad);

                let type_text: &str = if e.attrs & attr::VOLUME != 0 {
                    "Drive"
                } else if e.is_dir() {
                    "File folder"
                } else {
                    let ext = listing.ext(i);
                    let folded = ext.to_ascii_lowercase();
                    match self.type_names.get(&folded) {
                        Some(Some(n)) if !n.is_empty() => arena.alloc_str(n),
                        known => {
                            if known.is_none() && !in_root && !type_requests.contains(&folded) {
                                type_requests.push(folded);
                            }
                            let mut s = bumpalo::collections::String::new_in(arena);
                            if ext.is_empty() {
                                s.push_str("File");
                            } else {
                                let _ = write!(s, "{} File", name::display(ext).to_uppercase());
                            }
                            s.into_bump_str()
                        }
                    }
                };
                g.text(cols[2].x + pad, ty, type_text, font, t.text_dim, cols[2].w - 2.0 * pad);

                if !e.is_dir() || e.attrs & (attr::SIZE_KNOWN | attr::VOLUME) != 0 {
                    let mut s = bumpalo::collections::String::new_in(arena);
                    format::size(&mut s, e.size);
                    let w = g.measure(&s, font);
                    let x = (cols[3].right() - pad - w).max(cols[3].x + pad);
                    g.text(x, ty, &s, font, t.text_dim, cols[3].w - 2.0 * pad);
                }
            }
        }
        g.dl.set_clip(None);
        for r in requests {
            self.icons.insert(r.key, IconState::Pending);
            self.effects.push(Effect::Icon(r));
        }
        for ext in type_requests {
            if !self.type_names.contains_key(&ext) {
                self.type_names.insert(ext.clone(), None);
                self.effects.push(Effect::TypeName(ext));
            }
        }
        let th = self.layout.thumb;
        if th.h > 0.0 {
            let hot = matches!(self.hover, Hit::Thumb | Hit::Track) || matches!(self.drag, Drag::Thumb { .. });
            g.dl.round_rect(th, if hot { t.scrollbar_hover } else { t.scrollbar }, th.w / 2.0);
        }
    }

    fn draw_status<B: TextBackend>(&mut self, g: &mut Gfx<B>, arena: &Bump, t: &Theme) {
        let r = self.layout.status;
        g.dl.rect(r, t.chrome);
        let font = self.ui_font(12.0);
        let m = g.metrics(font);
        let baseline = (r.y + (r.h + m.ascent - m.descent) / 2.0).round();
        let pad = self.s(12.0);
        let tab = self.tab();
        let mut s = bumpalo::collections::String::new_in(arena);
        let mut color = t.text_dim;
        if let Some(err) = &tab.error {
            s.push_str(err);
            color = t.error;
        } else if let Some(v) = tab.view.as_ref().filter(|v| v.snap().gen == tab.gen) {
            let (n, total) = (v.rows().len(), v.total_rows());
            if v.is_filtered() {
                let _ = write!(s, "{n} of {total} {}", if total == 1 { "item" } else { "items" });
            } else {
                let _ = write!(s, "{n} {}", if n == 1 { "item" } else { "items" });
            }
            let k = v.selected_count();
            if k > 0 {
                let _ = write!(s, "    {k} selected");
                let size = v.selected_size();
                if size > 0 {
                    s.push_str("  ");
                    format::size(&mut s, size);
                }
            }
            if !v.snap().done {
                s.push_str("    Loading…");
            }
        } else if tab.loading.is_some() {
            s.push_str("Loading…");
        }
        g.text(r.x + pad, baseline, &s, font, color, r.w / 2.0);

        let mut right = bumpalo::collections::String::new_in(arena);
        if let (Some(first), Some(done)) = (tab.stats.first, tab.stats.done) {
            let _ = write!(right, "{:.1} ms first · {:.1} ms all", first.as_secs_f64() * 1e3, done.as_secs_f64() * 1e3);
        }
        let label = "Folder sizes";
        let sw = g.measure(label, font) + self.s(16.0);
        let sr = Rect::new(r.right() - pad - sw, r.y + self.s(2.0), sw, r.h - self.s(4.0));
        self.layout.sizes = sr;
        if self.hover == Hit::Sizes {
            g.dl.round_rect(sr, t.button_hover, self.s(3.0));
        }
        g.text(sr.x + self.s(8.0), baseline, label, font, t.text_dim, f32::INFINITY);
        let rw = g.measure(&right, font);
        g.text(sr.x - self.s(16.0) - rw, baseline, &right, font, t.text_dim.alpha(0xA0), f32::INFINITY);
    }
}

fn centered<B: TextBackend>(g: &mut Gfx<B>, r: Rect, s: &str, font: FontStyle, color: Color) {
    let w = g.measure(s, font);
    let m = g.metrics(font);
    let x = (r.x + (r.w - w) / 2.0).round();
    let y = (r.y + (r.h + m.ascent - m.descent) / 2.0).round();
    g.text(x, y, s, font, color, f32::INFINITY);
}

pub fn icon_key(name: &[u8], ext: &[u8], e: &crate::listing::Entry, size: u32, dir: &[u8]) -> (u64, IconKind) {
    if e.attrs & attr::VOLUME != 0 {
        return (hash::hash(&("vol", name, size)), IconKind::Volume);
    }
    if e.is_dir() {
        return (hash::hash(&("dir", size)), IconKind::Folder);
    }
    let folded = ext.to_ascii_lowercase();
    if PER_FILE_ICON_EXT.contains(&folded.as_slice()) {
        return (hash::hash(&("file", dir, name, size)), IconKind::File);
    }
    (hash::hash(&("ext", folded.as_slice(), size)), IconKind::Type)
}

#[cfg(test)]
#[path = "../../test/ui.rs"]
mod tests;
