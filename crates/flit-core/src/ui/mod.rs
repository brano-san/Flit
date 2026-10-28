pub mod edit;
mod a11y;
mod focus;
#[cfg(test)]
#[path = "../../test/ui/harness.rs"]
mod harness;
mod appearance;
mod archive;
mod columns;
mod details;
mod ops;
mod ops_extra;
mod search;
mod tab;
pub mod actions;
pub(crate) mod field;
mod filter;
mod form;
mod flow;
mod home;
mod jobs;
mod libraries;
mod layouts;
mod marquee;
mod menu;
mod nav;
mod network;
pub mod palette;
mod preview_pane;
pub mod settings;
mod settings_page;
pub mod sidebar;
mod address;
mod goto;
mod quick;
mod tabbar;
mod tags;
mod panels;
mod describe;
mod keymap;
mod toolbar;
mod popup;
mod own_menu;
mod confirm;
mod fuzzy;
mod tooltip;
mod widgets;
mod properties;
mod git;
mod tree;
mod bottom;
mod types;

pub use ops::{base_name_len, batch_names, changes_extension, choose_effect, drop_effect, same_volume, unique_name, validate_name, DragEvent, NewTemplate, ShellMenu, Spot, SHELL_VERBS};
pub use search::{thousands, IndexState, IndexVolume, SearchBackend, SearchEvent, SearchKey, SearchRequest, SearchRow, SearchScope, SearchSort, SearchSource, MAX_HISTORY};
pub use sidebar::{Drive, KnownFolder, NavPrefs, NavState, Places, QuickPrefs, Section};
pub use home::{Recent, Widget};
pub const UI_SCALE: (u32, u32) = (70, 200);
pub const SPRING_DELAY_MAX_MS: u32 = 3000;
pub use details::{folder_key, Columns, FolderView, FolderViews, ViewMode, ViewPrefs, THUMB_CACHE_MB};
pub use layouts::{adaptive_layout, has_thumbnail, is_media, LayoutMode, Sizes, LAYOUTS};
pub use jobs::JobState;
pub use preview_pane::PaneState;
pub use menu::{Mark, MenuItem};
pub use popup::{MenuOutcome, MenuView};
pub use tab::Tab;
pub use toolbar::{Show, ToolContext, ToolItem, ToolbarState, CONTEXTS};
pub use address::Typed;
pub use tabbar::{MovedTab, Startup, WindowSession};
pub use settings::{ConfirmDelete, Doc, Page, Setting, Value, SETTINGS};
pub use settings_page::{About, LOG_FILE};
pub use keymap::Keymap;
pub use tooltip::TipText;
pub use panels::{PanelsSession, Shape};

use crate::fmt as format;
use crate::hash::{self, FxHashMap};
use crate::i18n::{P, S};
use crate::input::{CursorIcon, Event, HitArea, Key, Mods, MouseButton};
use crate::listing::{attr, DirSnapshot};
use crate::location::{Location, ProviderId};
use crate::name;
use crate::open::{self, Open, OpenTarget, ResolveError, Route};
use crate::path::{FsPath, PathKind};
use crate::preview::{HostCmd, PreviewRequest};
use crate::provider::{ProviderEvent, ProviderRequest, Resolve, TabId};
use crate::recycle;
use crate::render::{Color, Rect};
use crate::sort::{CaseMode, GroupField, SortField, ViewSpec};
use crate::tags::TagRequest;
use crate::text::{FontFamily, FontStyle, Gfx, TextBackend};
use crate::theme::{self, Area, Theme};
use crate::thumbs;
use crate::view::Line;
use bumpalo::Bump;

use std::sync::Arc;
use std::time::{Duration, Instant};

const TAB_H: f32 = 32.0;
const TOOL_H: f32 = 40.0;
const HEADER_H: f32 = 28.0;
const ROW_H: f32 = 24.0;
const STATUS_H: f32 = 24.0;
const NAV_W: f32 = 34.0;
const VISITED_W: f32 = 22.0;
const CRUMB_STEP: f32 = 120.0;
const CAPTION_W: f32 = 46.0;
const SCROLLBAR_W: f32 = 12.0;
const ICON: f32 = 16.0;
const NAME_MIN_W: f32 = 120.0;
const TAB_W: f32 = 190.0;
const TAB_MIN_W: f32 = 72.0;

const MAX_PENDING_DIFFS: usize = 1 << 16;
const SCROLL_TAU: f32 = 0.045;

fn glide(pos: &mut f32, target: f32, dt: f32, smooth: bool) -> bool {
    let diff = target - *pos;
    if diff.abs() > 0.5 && smooth {
        *pos += diff * (1.0 - (-dt / SCROLL_TAU).exp()).max(0.15);
        return true;
    }
    *pos = target;
    false
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct Glide {
    pub pos: f32,
    pub target: f32,
}

impl Glide {
    pub(super) fn snap(&mut self, v: f32) {
        *self = Self { pos: v, target: v };
    }

    pub(super) fn by(&mut self, delta: f32, max: f32) {
        self.target = (self.target + delta).clamp(0.0, max.max(0.0));
    }

    pub(super) fn clamp(&mut self, max: f32) {
        let max = max.max(0.0);
        self.pos = self.pos.clamp(0.0, max);
        self.target = self.target.clamp(0.0, max);
    }

    pub(super) fn step(&mut self, dt: f32, smooth: bool) -> bool {
        glide(&mut self.pos, self.target, dt, smooth)
    }
}
const MAX_OPEN: usize = 16;
const MAX_THUMB_STATES: usize = 1 << 16;
const MAX_ROUTES: usize = 32;

const PER_FILE_ICON_EXT: [&[u8]; 6] = [b"exe", b"ico", b"lnk", b"url", b"cur", b"ani"];

const G_BACK: &str = "\u{E72B}";
const G_FORWARD: &str = "\u{E72A}";
const G_UP: &str = "\u{E74A}";
const G_MORE: &str = "\u{E712}";
const G_CRUMB_LEFT: &str = "\u{E76B}";
const G_CRUMB_RIGHT: &str = "\u{E76C}";
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
const G_BRANCH: &str = "\u{2387}";
const NESTED_MARK: &str = "\u{203A} ";
const G_FUNNEL: &str = "\u{E71C}";

const BIN_BUTTONS: [S; 3] = [S::UiBinRestore, S::UiBinRestoreAll, S::UiBinEmpty];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Column {
    Name,
    Modified,
    Type,
    Size,
    Origin,
    Deleted,
    Created,
    Tag,
    Items,
    Accessed,
    Volume,
    FolderUsage,
    Free,
    FileSystem,
    Percent,
    GitStatus,
    GitMessage,
    GitAuthor,
    GitDate,
}

pub const GIT_COLUMNS: [Column; 4] = [Column::GitStatus, Column::GitMessage, Column::GitAuthor, Column::GitDate];
pub const REPO_COLUMNS: [Column; 14] = [
    Column::Name,
    Column::Type,
    Column::Modified,
    Column::Items,
    Column::Size,
    Column::Volume,
    Column::Created,
    Column::Accessed,
    Column::FolderUsage,
    Column::Tag,
    Column::GitStatus,
    Column::GitMessage,
    Column::GitAuthor,
    Column::GitDate,
];
pub const COLUMNS: [Column; 10] = [Column::Name, Column::Type, Column::Modified, Column::Items, Column::Size, Column::Volume, Column::Created, Column::Accessed, Column::FolderUsage, Column::Tag];
pub const BIN_COLUMNS: [Column; 4] = [Column::Name, Column::Origin, Column::Deleted, Column::Size];
pub const COMPUTER_COLUMNS: [Column; 6] = [Column::Name, Column::Type, Column::Size, Column::Free, Column::Percent, Column::FileSystem];
pub const MENU_COLUMNS: [Column; 9] = [Column::Name, Column::Type, Column::Size, Column::Items, Column::Created, Column::Modified, Column::Accessed, Column::Volume, Column::FolderUsage];

impl Column {
    pub(super) fn title(self) -> &'static str {
        match self {
            Column::Name => S::DetailsColumnName,
            Column::Modified => S::DetailsColumnModified,
            Column::Created => S::DetailsColumnCreated,
            Column::Type => S::DetailsColumnType,
            Column::Size => S::DetailsColumnSize,
            Column::Origin => S::DetailsColumnOrigin,
            Column::Deleted => S::DetailsColumnDeleted,
            Column::Tag => S::DetailsColumnTag,
            Column::Items => S::DetailsColumnItems,
            Column::Accessed => S::DetailsColumnAccessed,
            Column::Volume => S::DetailsColumnVolume,
            Column::FolderUsage => S::DetailsColumnFolderUsage,
            Column::Free => S::DetailsColumnFree,
            Column::FileSystem => S::DetailsColumnFileSystem,
            Column::Percent => S::DetailsColumnPercent,
            Column::GitStatus => S::GitColumnStatus,
            Column::GitMessage => S::GitColumnMessage,
            Column::GitAuthor => S::GitColumnAuthor,
            Column::GitDate => S::GitColumnDate,
        }
        .t()
    }
    pub(super) fn field(self) -> Option<SortField> {
        Some(match self {
            Column::Name => SortField::Name,
            Column::Modified | Column::Deleted => SortField::Modified,
            Column::Created => SortField::Created,
            Column::Type => SortField::Type,
            Column::Size => SortField::Size,
            Column::Origin => SortField::Origin,
            Column::Tag => SortField::Tag,
            Column::Items => SortField::Items,
            Column::Accessed => SortField::Accessed,
            Column::Volume => SortField::Volume,
            Column::FolderUsage => SortField::FolderUsage,
            Column::Free => SortField::Free,
            Column::FileSystem => SortField::FileSystem,
            Column::Percent => SortField::Percent,
            Column::GitStatus | Column::GitMessage | Column::GitAuthor | Column::GitDate => return None,
        })
    }

    pub fn is_git(self) -> bool {
        self as u8 >= Column::GitStatus as u8
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WindowCmd {
    Minimize,
    ToggleMaximize,
    Close,
    Fullscreen,
    Compact(Option<bool>),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum IconKind {
    Type,
    File,
    Folder,
    CustomFolder,
    Volume,
    Place,
    Thumbnail,
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

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
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
    Thumbnail(IconRequest),
    ClearThumbnails,
    TypeName(Vec<u8>),
    SetClipboard(String),
    Paste,
    Window(WindowCmd),
    Cursor(CursorIcon),
    Title(String),
    NewWindow(Vec<Route>),
    Open(Open),
    SetClipboardFiles { paths: Vec<FsPath>, cut: bool },
    ShellMenu(ShellMenu),
    Properties(Vec<FsPath>),
    StartDrag(Vec<FsPath>),
    Pinned(Vec<Location>),
    PinFolders(Vec<FsPath>),
    Library(crate::library::LibraryOp),
    RefreshPlaces,
    Search(SearchRequest),
    Preview(PreviewRequest),
    QuickView(FsPath),
    Backdrop { gen: u64, path: FsPath },
    Host(HostCmd),
    Timer(Duration),
    MoveTab { tab: MovedTab, at: Option<(f32, f32)>, last: bool },
    RestoreSession,
    Tags(TagRequest),
    Settings(SettingsCmd),
    Share(Vec<FsPath>),
    Git(crate::git::GitRequest),
}

#[derive(Debug, Clone, PartialEq)]
pub enum SettingsCmd {
    Export { path: FsPath, text: String, replace: bool },
    Import(FsPath),
    CacheSize,
    Fonts,
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

#[derive(Clone, Debug, PartialEq)]
pub struct Persisted {
    pub path: Location,
    pub show_hidden: bool,
    pub view: FolderView,
    pub prefs: ViewPrefs,
    pub folders: FolderViews,
    pub archive: crate::archive::Defaults,
    pub startup: Startup,
}

impl Default for Persisted {
    fn default() -> Self {
        Self { path: Location::computer(), show_hidden: false, view: FolderView::default(), prefs: ViewPrefs::default(), folders: FolderViews::default(), archive: Default::default(), startup: Startup::default() }
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
    TabBar,
    Nav,
    Toolbar,
    Preview,
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
    Group(usize),
    Pane(usize, Option<usize>),
    Preview(preview_pane::PaneHit),
    Tool(toolbar::ToolHit),
    ListEmpty,
    Track,
    Thumb,
    Encoding,
    Branch,
    Bin(usize),
    Side(usize),
    SidePin(usize),
    SideEmpty,
    SideHeader(Section),
    HomeItem(usize),
    HomeEmpty,
    Splitter,
    FilterField,
    FilterClose,
    PaletteRow(usize),
    PaletteField,
    PaletteBox,
    PaletteOutside,
    PaletteClose,
    PaletteDescribe,
    PaletteScroll(bool),
    OpsButton,
    OpsPanel,
    OpsRow(usize),
    OpsClear,
    OpsCancel(usize),
    OpsMore(usize),
    OpsMenu(usize),
    Dialog,
    DialogButton(usize),
    DialogCheck,
    Form(usize),
    FormFlag(usize, usize),
    Modal,
    TabActions,
    ViewMode,
    Bookmark,
    PanelMenu,
    CrumbScroll(bool),
    GoButton(usize),
    QuickField,
    QuickClear,
    Eject,
    SideSettings,
    Settings(settings_page::SHit),
    Menu,
    QuickToggle,
    PanelSep(usize),
    Bottom(bottom::BottomHit),
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Drag {
    None,
    Thumb { grab: f32 },
    ColSep { col: Column, start_x: f32, start_w: f32 },
    Splitter { start_x: f32, start_w: f32 },
    PinMove { from: usize, to: usize },
    Preview { hit: preview_pane::PaneHit, start: f32, size: f32 },
    Marquee,
    Tab { origin: usize, x: f32, y: f32, moved: bool, torn: bool },
    PanelSep { sep: usize, start: f32, first: f32 },
}

#[derive(Default, Clone)]
struct Layout {
    title: Rect,
    tabs: Vec<(Rect, Rect)>,
    new_tab: Rect,
    actions: Rect,
    toolbar: Rect,
    commands: Rect,
    nav: [Rect; 4],
    view: Rect,
    bookmark: Rect,
    more: Rect,
    crumb_arrows: [Rect; 2],
    address: Rect,
    ops_button: Rect,
    header: Rect,
    columns: Vec<Column>,
    cols: Vec<Rect>,
    list: Rect,
    track: Rect,
    thumb: Rect,
    status: Rect,
    bar: Vec<(bottom::BottomHit, Rect)>,
    encoding: Rect,
    branch: Rect,
    bin: [Rect; 3],
    crumbs: Vec<Rect>,
    row_h: f32,
    mode: LayoutMode,
    icon: f32,
    flow: flow::FlowParams,
    panes: Vec<Rect>,
    bottom: f32,
}

pub struct App {
    pub utc_offset_min: i32,
    pub case: CaseMode,
    pub caps: crate::caps::Caps,
    pub open_target: OpenTarget,
    pub double_click: Duration,
    pub warn_extension_change: bool,
    pub ops_button_always: bool,
    pub conflict_default: Option<Resolve>,
    pub shell_items_inline: bool,
    pub menu_hidden: Vec<&'static str>,
    git_branches: Vec<String>,
    pub tab_wheel_reversed: bool,
    pub tab_actions_button: bool,
    pub folders_in_new_tab: bool,
    pub double_click_up: bool,
    pub builtin_archives: bool,
    pub indexing: bool,
    pub archives_as_folders: bool,
    pub spring_delay_ms: u32,
    pub switch_to_new_tab: bool,
    pub smooth_scroll: bool,
    pub animations: bool,
    pub round_corners: bool,
    pub tooltips: bool,
    pub touch_input: bool,
    pub single_click_files: settings::ClickOpen,
    pub single_click_folders: settings::ClickOpen,
    pub single_click_columns: settings::ClickOpen,
    pub select_previous: bool,
    pub show_flatten: bool,
    pub confirm_delete: ConfirmDelete,
    pub style: format::Style,
    pub show_hidden: bool,
    pub startup: Startup,
    pub startup_pages: Vec<Location>,
    pub about: About,
    pub log_level: usize,
    pub default_manager: bool,
    pub language: usize,
    shown_lang: crate::i18n::Lang,
    pub prefs: ViewPrefs,
    folders: FolderViews,
    settings: Option<settings_page::SettingsUi>,
    clicked_open: bool,
    menu: Vec<menu::Cmd>,
    focused_at: Option<Instant>,
    pub look: appearance::Look,
    dark: bool,
    scale: f32,
    system_scale: f32,
    ui_scale: u32,
    size: (f32, f32),
    maximized: bool,
    active: bool,

    tabs: Vec<Tab>,
    cur: usize,
    next_tab: TabId,
    next_gen: u64,
    timer_at: Option<Instant>,
    own_menu: Option<own_menu::OwnMenu>,
    menu_owner: Option<Hit>,
    menu_right: Option<f32>,
    tip: Option<tooltip::Tip>,
    quick: Option<quick::QuickTip>,
    confirm: Option<confirm::Confirm>,
    motion: bool,
    pub preedit: crate::input::Preedit,

    focus: Focus,
    kb: usize,
    a11y_targets: Vec<(u64, Hit)>,
    mods: Mods,
    hover: Hit,
    pressed: Hit,
    drag: Drag,
    cursor: CursorIcon,

    icons: FxHashMap<u64, IconState>,
    thumbs: FxHashMap<u64, IconState>,
    icon_epoch: u64,
    type_names: FxHashMap<Vec<u8>, Option<String>>,
    adaptive: FxHashMap<u64, LayoutMode>,

    layout: Layout,
    effects: Vec<Effect>,
    arena: Bump,
    last_frame: Option<Instant>,
    dirty: bool,
    ops: ops::Ops,
    nav: nav::Nav,
    marquee: Option<marquee::Marquee>,
    search: search::SearchView,
    jobs: jobs::Jobs,
    details: details::Details,
    archive: archive::ArchiveUi,
    strip: tabbar::Strip,
    addr: address::Address,
    tag_fixes: Vec<(FsPath, u32, Instant)>,
    preview: preview_pane::PreviewPane,
    panels: panels::Panels,
    toolbar: toolbar::Toolbar,
    keymap: Arc<Keymap>,
}

impl App {
    pub fn new(state: Persisted, start: Vec<Route>, tab_base: TabId, dark: bool, scale: f32, size: (u32, u32), now: Instant) -> Self {
        let mut app = Self {
            utc_offset_min: 0,
            case: CaseMode::Insensitive,
            caps: crate::caps::Caps::default(),
            open_target: OpenTarget::NewTab,
            double_click: Duration::from_millis(500),
            warn_extension_change: true,
            ops_button_always: false,
            conflict_default: None,
            shell_items_inline: false,
            menu_hidden: Vec::new(),
            git_branches: Vec::new(),
            tab_wheel_reversed: false,
            tab_actions_button: true,
            folders_in_new_tab: false,
            double_click_up: true,
            builtin_archives: false,
            indexing: true,
            archives_as_folders: false,
            spring_delay_ms: 1300,
            switch_to_new_tab: true,
            smooth_scroll: true,
            animations: true,
            round_corners: true,
            tooltips: true,
            touch_input: false,
            single_click_files: settings::ClickOpen::Never,
            single_click_folders: settings::ClickOpen::Never,
            single_click_columns: settings::ClickOpen::Never,
            select_previous: true,
            show_flatten: false,
            confirm_delete: ConfirmDelete::default(),
            style: format::Style { system: ("yyyy-MM-dd".into(), "HH:mm".into()), ..Default::default() },
            show_hidden: state.show_hidden,
            startup_pages: match &state.startup {
                Startup::Folders(pages) => pages.clone(),
                _ => Vec::new(),
            },
            startup: state.startup,
            about: About::default(),
            log_level: settings::LOG_DEFAULT,
            default_manager: false,
            language: 0,
            shown_lang: crate::i18n::lang(),
            prefs: state.prefs,
            folders: state.folders,
            settings: None,
            clicked_open: false,
            menu: Vec::new(),
            focused_at: None,
            look: appearance::Look::new(dark),
            dark,
            scale,
            system_scale: scale,
            ui_scale: 100,
            size: (size.0 as f32, size.1 as f32),
            maximized: false,
            active: true,
            tabs: Vec::new(),
            cur: 0,
            next_tab: tab_base + 1,
            next_gen: 0,
            timer_at: None,
            own_menu: None,
            menu_owner: None,
            menu_right: None,
            tip: None,
            quick: None,
            confirm: None,
            motion: false,
            preedit: Default::default(),
            focus: Focus::List,
            kb: 0,
            a11y_targets: Vec::new(),
            mods: Mods::default(),
            hover: Hit::None,
            pressed: Hit::None,
            drag: Drag::None,
            cursor: CursorIcon::Default,
            icons: FxHashMap::default(),
            thumbs: FxHashMap::default(),
            icon_epoch: 0,
            type_names: FxHashMap::default(),
            adaptive: FxHashMap::default(),
            layout: Layout::default(),
            effects: Vec::new(),
            arena: Bump::new(),
            last_frame: None,
            dirty: true,
            ops: ops::Ops::default(),
            nav: nav::Nav::default(),
            marquee: None,
            search: Default::default(),
            jobs: Default::default(),
            details: Default::default(),
            archive: archive::ArchiveUi::new(state.archive),
            strip: Default::default(),
            addr: Default::default(),
            tag_fixes: Vec::new(),
            preview: Default::default(),
            panels: Default::default(),
            toolbar: Default::default(),
            keymap: keymap::defaults(),
        };
        let mut start = if start.is_empty() { vec![Route { folder: state.path, select: None }] } else { start };
        let total = start.len();
        start.truncate(MAX_ROUTES);
        for r in start {
            app.add_tab(r, state.view, state.show_hidden, now);
        }
        app.report_dropped(total);
        app.layout();
        app
    }

    fn report_dropped(&mut self, total: usize) {
        if total > MAX_ROUTES {
            self.tab_mut().error = Some(crate::trn!(P::UiDroppedRoutes, total, shown = MAX_ROUTES));
        }
    }

    pub fn shutdown(&mut self) {
        for t in self.tabs.iter_mut().chain(self.panels.list.iter_mut().flat_map(|p| p.tabs.iter_mut())) {
            t.shutdown(&mut self.effects);
        }
        self.cancel_ops();
    }

    pub fn persisted(&self) -> Persisted {
        let t = self.tab();
        Persisted { path: t.location().clone(), show_hidden: t.view_spec.show_hidden, view: t.folder_view(), prefs: self.prefs.clone(), folders: self.folders.clone(), archive: self.archive.prefs.clone(), startup: self.startup.clone() }
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
        let lines = self.visible_lines(&self.flow());
        let rows = |n: usize| t.view.as_ref().map_or(0, |v| v.row_at_line(n)) as u32;
        (t.gen, rows(lines.start), rows(lines.end))
    }

    pub fn ime_area(&self) -> Option<Rect> {
        self.search_ime_area().or_else(|| self.settings_ime_area()).or_else(|| self.nav_ime_area()).or_else(|| self.rename_ime_area())
    }

    pub fn ui_scale(&self) -> u32 {
        self.ui_scale
    }

    pub fn set_ui_scale(&mut self, percent: u32) {
        self.ui_scale = percent.clamp(UI_SCALE.0, UI_SCALE.1);
        self.rescale();
    }

    fn rescale(&mut self) {
        self.scale = self.system_scale * self.ui_scale as f32 / 100.0;
        self.layout();
        self.dirty = true;
    }

    fn s(&self, v: f32) -> f32 {
        (v * self.scale).round()
    }

    fn theme(&self) -> &Theme {
        &self.look.palette.base
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
        let typed = self.addr.pending.take();
        if let Some(text) = typed.as_ref().filter(|_| routes.is_empty() && matches!(errors.first(), Some(ResolveError::Invalid(_)))) {
            return self.run_typed(text);
        }
        let watched = !routes.is_empty();
        self.dispatch(routes, target, now);
        if let Some(text) = typed.filter(|_| watched) {
            self.watch_typed(text);
        }
        self.show_open_errors(&errors);
    }

    pub fn show_open_errors(&mut self, errors: &[ResolveError]) {
        if let Some(e) = errors.first() {
            self.tab_mut().error = Some(match e {
                ResolveError::UnknownShellLocation(s) => self.hand_to_explorer(s),
                ResolveError::Invalid(s) => crate::tr!(S::UiInvalidPath, path = s),
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
        routes.iter_mut().for_each(|r| r.folder.canonicalize(&self.nav.sidebar.places.wsl));
        match target {
            OpenTarget::NewWindow => self.effects.push(Effect::NewWindow(routes)),
            OpenTarget::CurrentTab => {
                let mut it = routes.into_iter();
                let first = it.next().unwrap();
                self.route_here(first, now);
                for r in it {
                    self.add_tab_like_current(r, now);
                }
            }
            OpenTarget::NewTab | OpenTarget::Default => {
                for r in routes {
                    self.add_tab_like_current(r, now);
                }
            }
        }
        self.report_dropped(total);
        self.dirty = true;
    }

    fn route_here(&mut self, r: Route, now: Instant) {
        let k = self.cur;
        let t = &mut self.tabs[k];
        if !t.history.go(r.folder) {
            let current = t.gen;
            if let (Some(name), Some(v)) = (&r.select, t.view.as_mut().filter(|v| v.snap().gen == current)) {
                if let Some(i) = v.find(name).filter(|&i| v.is_visible(i)) {
                    v.clear_selection();
                    v.set_selected(i, true);
                    v.focus = Some(i);
                    v.anchor = Some(i);
                    return self.reveal_focus(true);
                }
            }
        }
        self.load_tab(k, r.select, now);
        self.after_tab_change();
    }

    fn add_tab_like_current(&mut self, r: Route, now: Instant) {
        let t = self.tab();
        let (view, show_hidden) = (t.folder_view(), t.view_spec.show_hidden);
        self.add_tab(r, view, show_hidden, now);
    }

    fn add_tab(&mut self, r: Route, view: FolderView, show_hidden: bool, now: Instant) {
        let id = self.next_tab;
        self.next_tab += 1;
        let mut t = Tab::new(id, r.folder, ViewSpec { show_hidden, ..ViewSpec::default() });
        t.set_folder_view(view);
        self.tabs.push(t);
        self.cur = self.tabs.len() - 1;
        self.load_tab(self.cur, r.select, now);
        self.after_tab_change();
    }

    fn after_tab_change(&mut self) {
        self.focus = Focus::List;
        self.wake_tab();
        self.effects.push(Effect::Title(self.window_title()));
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
            return self.close_panel();
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
            let select = self.select_previous.then(|| cur.file_name().to_vec());
            self.dispatch(vec![Route { folder: parent, select }], OpenTarget::CurrentTab, now);
        }
    }

    fn go_history(&mut self, forward: bool, now: Instant) {
        self.jump_history(if forward { 1 } else { -1 }, now);
    }

    fn jump_history(&mut self, n: isize, now: Instant) {
        let (k, previous) = (self.cur, self.select_previous);
        let t = &mut self.tabs[k];
        let from = t.location().clone();
        if t.history.jump(n) {
            let select = (previous && from.parent().as_ref() == Some(t.location())).then(|| from.file_name().to_vec());
            self.load_tab(k, select, now);
            self.after_tab_change();
        }
    }

    pub fn refresh(&mut self, now: Instant) {
        self.reload(self.cur, now);
    }

    fn reload(&mut self, k: usize, now: Instant) {
        let select = self.tabs[k].focused_name();
        self.load_tab(k, select, now);
        self.dirty |= k == self.cur;
    }

    fn open_entry(&mut self, i: u32, now: Instant) {
        let t = self.tab();
        if t.view_loc.is_recycle_bin() {
            let name = name::display(t.view.as_ref().unwrap().listing().name(i)).into_owned();
            self.tab_mut().error = Some(crate::tr!(S::UiInRecycleBin, name = name));
            return;
        }
        let e = t.view.as_ref().unwrap().listing().entries[i as usize];
        let loc = t.entry_location(i);
        if !e.is_dir() && self.open_archived(&loc) {
            return;
        }
        let t = self.tab();
        if e.is_dir() || self.browses_as_folder(loc.file_name()) || crate::library::is_library(loc.path()) {
            let route = vec![Route { folder: loc, select: None }];
            if self.folders_in_new_tab {
                self.open_in_tabs(route, now);
            } else {
                self.dispatch(route, OpenTarget::CurrentTab, now);
            }
        } else {
            self.effects.push(Effect::Provider { provider: loc.provider(), req: ProviderRequest::Open { tab: t.id, path: loc.path().clone() } });
        }
    }

    fn open_selection(&mut self, now: Instant) {
        let t = self.tab();
        let Some(v) = &t.view else { return };
        let is_dir = |i: u32| v.listing().entries[i as usize].is_dir();
        if v.selected_count() > 1 && !t.view_loc.is_recycle_bin() {
            return self.open_many(now);
        }
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
        let mut visible = self.poll_here();
        visible |= self.poll_parked();
        self.poll_bin();
        visible |= self.poll_address();
        visible |= self.poll_quick();
        self.dirty |= visible;
        visible
    }

    fn poll_here(&mut self) -> bool {
        let mut visible = false;
        for k in 0..self.tabs.len() {
            visible |= self.poll_tab(k);
            visible |= self.tabs[k].apply_diffs() && k == self.cur;
        }
        visible | self.poll_panes() | self.poll_tree()
    }

    fn poll_tab(&mut self, k: usize) -> bool {
        let Some(snap) = self.tabs[k].slot.take() else { return false };
        if self.redirect(k, &snap) {
            return false;
        }
        self.sync_spec(k);
        let current = snap.gen == self.tabs[k].gen;
        let (loaded, fresh) = (snap.done && current, current && self.tabs[k].view.as_ref().is_none_or(|v| v.snap().gen != snap.gen));
        if fresh && self.tabs[k].look == LayoutMode::Columns {
            self.keep_as_pane(k);
        }
        let resort = self.tabs[k].on_snapshot(snap);
        if fresh || loaded {
            self.settle_look(k, fresh, loaded);
        }
        if loaded {
            self.reopen_tree(k);
        }
        if resort {
            self.resort(k);
        }
        if loaded {
            self.on_loaded(k);
        }
        k == self.cur
    }

    pub fn on_provider_event(&mut self, ev: ProviderEvent, now: Instant) {
        if let Some(id) = self.parked_of_event(&ev) {
            return self.visit(id, |a| a.on_provider_event(ev, now));
        }
        let tab_of = |tabs: &[Tab], id: TabId, gen: u64| tabs.iter().position(|t| t.id == id && t.gen == gen);
        match ev {
            ProviderEvent::Changes { tab, gen, changes } => {
                if let Some(k) = tab_of(&self.tabs, tab, gen) {
                    self.tabs[k].pending_diffs.extend(changes);
                    self.git_refresh(k, true);
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
            ProviderEvent::DirSize { tab, gen, idx, stats } => {
                if let Some(k) = tab_of(&self.tabs, tab, gen) {
                    if let Some(v) = self.tabs[k].view.as_mut().filter(|v| v.snap().gen == gen) {
                        v.set_dir_stats(idx, stats);
                    }
                    self.dirty |= k == self.cur;
                }
            }
            ProviderEvent::OpDone { tab, id, error, report } => self.op_done(tab, id, error.map(|e| crate::i18n::localize(&e).into_owned()), report, now),
            ProviderEvent::OpProgress { id, progress, .. } => self.job_progress(id, progress, now),
            ProviderEvent::OpConflict { id, conflict, .. } => self.job_conflict(id, conflict),
            ProviderEvent::Navigate { tab, path } => {
                if self.tab().id == tab {
                    self.navigate(Location::from_local(path), now);
                }
            }
            ProviderEvent::Hashed { id, result, .. } => self.hashed(id, result.map_err(|e| crate::i18n::localize(&e).into_owned())),
            ProviderEvent::Measured { id, totals, file_system, done, .. } => {
                if !self.quick_measured(id, totals, done) {
                    self.measured(id, totals, file_system, done);
                }
            }
            ProviderEvent::Status { tab, gen, text } => {
                if let Some(k) = tab_of(&self.tabs, tab, gen) {
                    if let Some(l) = self.tabs[k].loading.as_mut() {
                        l.note = Some(crate::i18n::localize(&text).into_owned());
                        self.dirty |= k == self.cur;
                    }
                }
            }
        }
    }

    pub fn on_resorted(&mut self, tab: TabId, gen: u64, view: ViewSpec, base_len: usize, order: Vec<u32>) {
        if let Some(id) = self.parked_of(tab) {
            return self.visit(id, |a| a.on_resorted(tab, gen, view, base_len, order));
        }
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

    pub fn on_thumb<B: TextBackend>(&mut self, gfx: &mut Gfx<B>, key: u64, result: IconResult) {
        match result {
            IconResult::Ready(bm) if gfx.thumbs.insert(key, bm.width, bm.height, &bm.rgba) => {
                self.thumbs.remove(&key);
            }
            IconResult::Ready(_) | IconResult::Failed => {
                if self.thumbs.len() >= MAX_THUMB_STATES {
                    self.thumbs.retain(|_, s| *s == IconState::Pending);
                }
                self.thumbs.insert(key, IconState::Failed);
            }
            IconResult::Skipped => {
                self.thumbs.remove(&key);
            }
        }
        self.dirty = true;
    }

    pub fn on_type_name(&mut self, ext: Vec<u8>, name: Option<String>) {
        self.type_names.insert(ext, Some(name.unwrap_or_default()));
        self.dirty = true;
    }

    pub fn on_paste(&mut self, text: &str) {
        if self.form_insert(text) || self.jobs.modal() || self.settings_insert(text.lines().next().unwrap_or("")) {
            return;
        }
        if self.search.active {
            return self.search_paste(text);
        }
        if self.ops_paste(text) {
            return;
        }
        self.nav_insert(text.lines().next().unwrap_or(""));
    }

    fn resort(&mut self, k: usize) {
        self.sync_spec(k);
        let t = &self.tabs[k];
        if let Some(v) = &t.view {
            self.effects.push(Effect::Resort { tab: t.id, gen: t.gen, snap: v.snap().clone(), view: t.view_spec });
        }
    }

    fn layout(&mut self) {
        if self.panels.here != self.panels.active {
            let r = self.panel_rect();
            return self.layout_panel(r);
        }
        self.layout_panels();
        self.layout_jobs();
        self.layout_settings();
    }

    fn layout_panel(&mut self, area: Rect) {
        let active = self.panels.here == self.panels.active;
        let (right, inset) = self.panel_edges(area);
        let ops_button = active && self.ops_button_visible();
        let status_bar = active && (self.prefs.status_bar || self.tab().error.is_some() || self.tab().filter.is_some());
        let tool_h = self.toolbar_h();
        let (n, cur, actions) = (self.tabs.len(), self.cur, self.tab_actions_button);
        let sc = self.scale;
        let s = |v: f32| (v * sc).round();
        let l = &mut self.layout;
        l.row_h = s(ROW_H);
        l.title = Rect::new(area.x, area.y, area.w, s(TAB_H));
        let limit = l.title.right() - if right { 3.0 * s(CAPTION_W) } else { 0.0 };
        let bh = l.title.h - s(8.0);
        let button = |x: f32| Rect::new(x, area.y + s(4.0), s(30.0), bh);
        l.actions = if actions { button(limit - s(34.0)) } else { Rect::default() };
        let x0 = area.x + inset + s(6.0);
        let end = if actions { l.actions.x } else { limit };
        let room = (end - s(40.0) - x0).max(0.0);
        let tw = s(TAB_W).min(room).max(s(TAB_MIN_W).min(room));
        let fit = ((room / tw.max(1.0)).floor() as usize).max(1);
        let first = (cur + 1).saturating_sub(fit);
        l.tabs.clear();
        for i in 0..n {
            let shown = (first..first + fit).contains(&i);
            let x = x0 + (i.clamp(first, first + fit) - first) as f32 * tw;
            let r = Rect::new(x, area.y + s(4.0), if shown { tw } else { 0.0 }, l.title.h - s(4.0));
            let c = if shown { Rect::new(r.right() - s(26.0), r.y + (r.h - s(20.0)) / 2.0, s(20.0), s(20.0)) } else { Rect::default() };
            l.tabs.push((r, c));
        }
        l.new_tab = button(x0 + (n.min(first + fit) - first) as f32 * tw + s(4.0));

        l.toolbar = Rect::new(area.x, l.title.bottom(), area.w, s(TOOL_H));
        for (i, r) in l.nav.iter_mut().enumerate() {
            *r = Rect::new(area.x + s(6.0) + i as f32 * s(NAV_W), l.toolbar.y + s(4.0), s(NAV_W), s(TOOL_H - 8.0));
        }
        l.nav[3] = Rect::new(l.nav[2].right(), l.nav[2].y, s(VISITED_W), l.nav[2].h);
        l.view = Rect::new(l.nav[3].right() + s(9.0), l.nav[3].y, s(NAV_W), l.nav[3].h);
        l.bookmark = Rect::new(l.view.right(), l.view.y, s(NAV_W), l.view.h);
        let ax = l.bookmark.right() + s(9.0);
        l.more = Rect::new(area.right() - s(6.0) - s(NAV_W), l.nav[0].y, s(NAV_W), l.nav[0].h);
        l.ops_button = if ops_button { Rect::new(l.more.x - s(NAV_W), l.nav[0].y, s(NAV_W), l.nav[0].h) } else { Rect::default() };
        let edge = if ops_button { l.ops_button.x } else { l.more.x };
        l.address = Rect::new(ax, l.toolbar.y + s(5.0), (edge - s(9.0) - ax).max(s(40.0)), s(TOOL_H - 10.0));
        l.commands = Rect::new(area.x, l.toolbar.bottom(), area.w, tool_h);
        l.header = Rect::new(area.x, l.commands.bottom(), area.w, s(HEADER_H));
        let status_h = if status_bar { s(STATUS_H) } else { 0.0 };
        l.status = Rect::new(area.x, area.bottom() - status_h, area.w, status_h);
        l.bottom = l.status.y;
        if !status_bar {
            l.encoding = Rect::default();
            l.bin = Default::default();
            l.branch = Rect::default();
        }
        if active {
            self.layout_preview();
        }
        let l = &self.layout;
        let content = Rect::new(l.header.x, l.commands.bottom(), l.header.w, (l.bottom - l.commands.bottom()).max(0.0));
        self.layout_pane(content);
    }

    fn layout_pane(&mut self, area: Rect) {
        let (kinds, widths) = (self.column_kinds(), self.tab().columns);
        let sc = self.scale;
        let s = |v: f32| (v * sc).round();
        let l = &mut self.layout;
        l.header = Rect::new(area.x, area.y, area.w, s(HEADER_H));
        l.bottom = area.bottom();
        self.layout_look();
        let l = &mut self.layout;
        l.columns.clear();
        l.columns.extend(widths.ordered(kinds).into_iter().filter(|&c| widths.shown(c)));
        let fixed: f32 = l.columns.iter().skip(1).map(|&c| s(widths.width(c))).sum();
        let name_w = (l.list.w - fixed).max(s(NAME_MIN_W));
        let mut x = l.header.x;
        l.cols.clear();
        for &c in &l.columns {
            let cw = if c == Column::Name { name_w } else { s(widths.width(c)) };
            l.cols.push(Rect::new(x, l.header.y, cw, l.header.h));
            x += cw;
        }
        if std::mem::take(&mut self.tab_mut().reveal_pending) {
            self.reveal_focus(true);
        }
        let f = self.flow();
        let (max_scroll, len, content) = (self.max_scroll_of(&f), self.view_len(), f.extent());
        let t = self.tab_mut();
        t.scroll_target = t.scroll_target.clamp(0.0, max_scroll);
        t.scroll = t.scroll.clamp(0.0, max_scroll);
        let frac = if max_scroll > 0.0 { t.scroll / max_scroll } else { 0.0 };
        let l = &mut self.layout;
        let tr = l.track;
        l.thumb = match (content > len, l.flow.across) {
            (false, _) => Rect::default(),
            (true, false) => {
                let th = (tr.h * len / content).max(s(24.0)).min(tr.h);
                Rect::new(tr.x + s(3.0), tr.y + (tr.h - th) * frac, tr.w - s(6.0), th)
            }
            (true, true) => {
                let tw = (tr.w * len / content).max(s(24.0)).min(tr.w);
                Rect::new(tr.x + (tr.w - tw) * frac, tr.y + s(3.0), tw, tr.h - s(6.0))
            }
        };
        self.sync_panes();
        self.layout_filter();
        self.layout_home();
    }

    fn hit(&self, x: f32, y: f32) -> Hit {
        if let Some(h) = self.jobs_hit(x, y).or_else(|| self.palette_hit(x, y)).or_else(|| self.settings_hit(x, y)).or_else(|| self.flyout_hit(x, y)) {
            return h;
        }
        if let Some(h) = self.chrome_hit(x, y).or_else(|| self.nav_hit(x, y)).or_else(|| self.preview_hit(x, y)).or_else(|| self.toolbar_hit(x, y)).or_else(|| self.sep_hit(x, y)) {
            return h;
        }
        let l = &self.layout;
        if l.title.contains(x, y) {
            return row_hit(l, x, y);
        }
        if l.toolbar.contains(x, y) {
            if let Some(i) = l.nav.iter().position(|r| r.contains(x, y)) {
                return Hit::Nav(i);
            }
            if l.view.contains(x, y) {
                return Hit::ViewMode;
            }
            if l.bookmark.contains(x, y) {
                return Hit::Bookmark;
            }
            if l.more.contains(x, y) {
                return Hit::PanelMenu;
            }
            if l.address.contains(x, y) {
                if let Some(k) = l.crumb_arrows.iter().position(|r| r.contains(x, y)) {
                    return Hit::CrumbScroll(k == 1);
                }
                if let Some(i) = l.crumbs.iter().position(|r| r.contains(x, y)) {
                    return Hit::Crumb(i);
                }
                return Hit::Address;
            }
            return Hit::None;
        }
        if l.header.contains(x, y) {
            let grip = self.s(4.0);
            if let Some(i) = (1..l.cols.len()).find(|&i| (x - l.cols[i].x).abs() <= grip) {
                return Hit::ColSep(i);
            }
            return l.cols.iter().position(|r| r.contains(x, y)).map_or(Hit::None, Hit::Header);
        }
        if l.track.contains(x, y) {
            let th = l.thumb;
            return if y >= th.y && y < th.bottom() && th.h > 0.0 { Hit::Thumb } else { Hit::Track };
        }
        if let Some(i) = l.panes.iter().position(|r| r.contains(x, y)) {
            return self.pane_hit(i, y);
        }
        if self.eject_rect().is_some_and(|r| r.contains(x, y)) {
            return Hit::Eject;
        }
        if l.list.contains(x, y) {
            return self.list_hit(x, y);
        }
        if let Some(h) = self.bottom_hit(x, y) {
            return h;
        }
        if l.encoding.contains(x, y) {
            return Hit::Encoding;
        }
        if l.branch.contains(x, y) {
            return Hit::Branch;
        }
        if let Some(k) = l.bin.iter().position(|r| r.contains(x, y)) {
            return Hit::Bin(k);
        }
        Hit::None
    }

    pub fn hit_test(&self, x: f32, y: f32) -> HitArea {
        if self.over_own_menu(x, y) || self.confirm.as_ref().is_some_and(|c| c.rect.contains(x, y)) {
            return HitArea::Client;
        }
        match self.hit(x, y) {
            Hit::Caption(0) => HitArea::Minimize,
            Hit::Caption(1) => HitArea::Maximize,
            Hit::Caption(_) => HitArea::Close,
            Hit::None if self.in_title(y) && self.parked_row_hit(x, y).is_none() => HitArea::Caption,
            _ => HitArea::Client,
        }
    }

    pub fn on_event(&mut self, ev: &Event, now: Instant) {
        if matches!(ev, Event::MouseDown { .. } | Event::Key { pressed: true, .. } | Event::Wheel { .. } | Event::MouseLeave | Event::Focus(false) | Event::ShowMenu { .. }) {
            self.tip_hide();
            self.quick_hide();
        }
        if self.own_menu_event(ev, now) || self.confirm_event(ev, now) {
            return;
        }
        if self.search.active && !self.jobs.modal() && self.search_event(ev, now) {
            return;
        }
        match ev {
            Event::Resized { width, height } => {
                self.size = (*width as f32, *height as f32);
                self.layout();
                self.dirty = true;
            }
            Event::ScaleChanged(s) => {
                self.system_scale = *s;
                self.rescale();
            }
            Event::ThemeChanged { dark } => self.set_system_dark(*dark),
            Event::Contrast(colors) => self.set_contrast(*colors),
            Event::Invoke(id) => self.a11y_invoke(*id, now),
            Event::A11yFocus(id) => self.a11y_focus(*id),
            Event::A11ySelect(id) => self.a11y_select(*id, now),
            Event::Focus(f) => {
                self.active = *f;
                self.mods = Mods::default();
                self.focused_at = f.then_some(now);
                self.ops_timer_cancel();
                self.end_autoscroll();
                self.dirty = true;
            }
            Event::MouseMove { x, y } => self.mouse_move(*x, *y, now),
            Event::MouseLeave => {
                self.hide_settings_tip();
                self.hover_pick(Hit::None, now);
                if self.hover != Hit::None {
                    self.hover = Hit::None;
                    self.dirty = true;
                }
            }
            Event::Pointer { touch } => self.touch_input = *touch,
            Event::MouseDown { x, y, button, clicks } => self.mouse_down(*x, *y, *button, *clicks, now),
            Event::MouseUp { x, y, button } => self.mouse_up(*x, *y, *button, now),
            Event::Wheel { dy, .. } => {
                if self.mods.alt && !self.mods.ctrl && !self.mods.shift && !self.settings_open() {
                    self.run_action(if *dy > 0.0 { "font.increase" } else { "font.decrease" }, now);
                    return;
                }
                if self.settings_wheel(*dy) || self.panel_wheel(ev, *dy, now) || self.jobs_wheel(*dy) || self.nav_wheel(*dy) || self.pane_wheel(*dy) || self.preview_wheel(*dy) || self.strip_wheel(*dy) {
                    return;
                }
                if self.bottom_wheel(*dy) {
                    return;
                }
                if self.mods.ctrl {
                    return self.resize_items(if *dy > 0.0 { 1 } else { -1 });
                }
                let (step, max) = (self.wheel_step(), self.max_scroll());
                let t = self.tab_mut();
                t.scroll_target = (t.scroll_target - dy * step).clamp(0.0, max);
                self.dirty = true;
            }
            Event::Key { key, mods, pressed } => {
                self.mods = *mods;
                if *pressed {
                    self.key(*key, *mods, now);
                }
            }
            Event::Char(c) => self.char_input(*c, now),
            Event::Pasted(text) => self.on_paste(text),
            Event::Preedit(text) => {
                self.preedit.clone_from(text);
                self.dirty = true;
            }
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
            Event::ShellVerb(verb) => self.shell_verb(verb),
            Event::MenuCommand(k) => self.menu_command(*k, now),
            Event::ShowMenu { .. } => {}
            Event::DevicesChanged => self.effects.push(Effect::RefreshPlaces),
            Event::Timer => {
                self.timer_at = None;
                self.ops_timer(now);
                self.hover_timer(now);
                self.history_timer(now);
                self.tip_timer(now);
                self.quick_timer(now);
            }
            Event::ClipboardChanged(seq) => self.clipboard_changed(*seq),
            Event::SessionEnding => {}
            Event::Hosted(ev) => self.on_hosted(ev.clone()),
        }
    }

    fn set_cursor(&mut self, c: CursorIcon) {
        if self.cursor != c {
            self.cursor = c;
            self.effects.push(Effect::Cursor(c));
        }
    }

    fn mouse_move(&mut self, x: f32, y: f32, now: Instant) {
        self.strip_pointer(x, y);
        self.settings_tip(x, y);
        if self.toolbar_mouse_move(x) || self.autoscroll_move(self.along_of(x, y)) || self.ops_mouse_move(x, y) || self.marquee_move(x, y) {
            return;
        }
        match self.drag {
            Drag::Thumb { grab } => {
                let (l, max) = (&self.layout, self.max_scroll());
                let (track, thumb, at) = if l.flow.across { (l.track.x, l.track.w - l.thumb.w, x) } else { (l.track.y, l.track.h - l.thumb.h, y) };
                let frac = ((at - grab - track) / thumb.max(1.0)).clamp(0.0, 1.0);
                let t = self.tab_mut();
                t.scroll = frac * max;
                t.scroll_target = t.scroll;
                self.layout();
                self.dirty = true;
                return;
            }
            Drag::ColSep { col, start_x, start_w } => return self.set_column_width(col, start_w - (x - start_x) / self.scale),
            Drag::Splitter { start_x, start_w } => return self.drag_splitter(x, start_x, start_w),
            Drag::PinMove { from, .. } => return self.drag_pin(from, y),
            Drag::Tab { .. } => return self.drag_tab(x, y),
            Drag::Preview { hit, start, size } => return self.preview_drag(hit, x, y, start, size),
            Drag::PanelSep { sep, start, first } => return self.drag_sep(sep, x, y, start, first),
            Drag::Marquee | Drag::None => {}
        }
        if self.start_pin_drag(y) {
            return;
        }
        let h = self.hit(x, y);
        let cursor = match h {
            Hit::ColSep(_) | Hit::Splitter => CursorIcon::ResizeHorizontal,
            Hit::QuickField | Hit::FilterField | Hit::PaletteField | Hit::Settings(settings_page::SHit::Search) => CursorIcon::Text,
            Hit::Preview(preview_pane::PaneHit::Split) => self.preview_cursor(),
            Hit::PanelSep(k) => self.sep_cursor(k),
            _ => CursorIcon::Default,
        };
        self.set_cursor(cursor);
        self.parked_hover(x, y);
        self.hover_pick(h, now);
        if h != self.hover {
            self.hover = h;
            self.tip_hover(h, now);
            self.quick_hover(h, now);
            self.dirty = true;
        }
    }

    fn mouse_down(&mut self, x: f32, y: f32, button: MouseButton, clicks: u8, now: Instant) {
        self.dirty = true;
        if self.form_down(x, y, clicks) {
            return;
        }
        if !self.settings_open() {
            self.panel_press(x, y);
        }
        if self.jobs_mouse_down(self.hit(x, y)) || self.settings_mouse_down(self.hit(x, y), x, button, clicks) || self.strip_mouse_down(self.hit(x, y), button, clicks, now) || self.autoscroll_down(self.hit(x, y), self.along_of(x, y), button) || self.ops_mouse_down(x, y, button, clicks) {
            return;
        }
        let h = self.hit(x, y);
        if self.nav_mouse_down(h, (x, y), button, clicks, now) || self.toolbar_mouse_down(h, x, button) {
            return;
        }
        match (button, h) {
            (MouseButton::Back, _) => return self.go_history(false, now),
            (MouseButton::Forward, _) => return self.go_history(true, now),
            (MouseButton::Middle, Hit::Tab(i) | Hit::TabClose(i)) => return self.close_tab(i),
            (MouseButton::Middle, Hit::Header(c)) => {
                if let Some(&col) = self.layout.columns.get(c) {
                    self.toggle_column(col);
                    self.layout();
                }
                return;
            }
            (MouseButton::Middle, _) => return,
            _ => {}
        }
        self.pressed = h;
        match h {
            Hit::Tab(i) => self.press_tab(i, x, y, button),
            Hit::Nav(i @ 0..=1) if button == MouseButton::Left => self.arm_history_press(i, now),
            Hit::Address if button == MouseButton::Left => self.open_address(),
            Hit::Row(row) => {
                self.focus = Focus::List;
                if button == MouseButton::Left && self.on_chevron(x, y, row) {
                    self.tree_toggle_row(row, self.mods.shift);
                } else if button == MouseButton::Left && self.on_checkbox(x, y) {
                    self.toggle_row(row);
                } else if button == MouseButton::Left && clicks == 1 && !self.mods.shift && self.marquee_column(x) {
                    let base = self.marquee_base();
                    self.click_row(row, button, clicks, now);
                    self.arm_marquee(x, y, self.mods.ctrl, base);
                } else if !self.press_row(row, button, clicks, x, y, now) {
                    self.click_row(row, button, clicks, now);
                }
            }
            Hit::ListEmpty => {
                self.focus = Focus::List;
                let base = self.marquee_base();
                let ctrl = self.mods.ctrl;
                if let Some(v) = &mut self.tab_mut().view {
                    if !ctrl {
                        v.clear_selection();
                    }
                }
                if button == MouseButton::Left {
                    self.arm_marquee(x, y, ctrl, base);
                }
            }
            Hit::Thumb => {
                let th = self.layout.thumb;
                self.drag = Drag::Thumb { grab: self.along_of(x - th.x, y - th.y) };
            }
            Hit::Track => {
                let th = self.layout.thumb;
                let dir = if self.along_of(x - th.x, y - th.y) < 0.0 { -1.0 } else { 1.0 };
                let (len, max) = (self.view_len(), self.max_scroll());
                let t = self.tab_mut();
                t.scroll_target = (t.scroll_target + dir * len).clamp(0.0, max);
            }
            Hit::Group(_) => self.focus = Focus::List,
            Hit::Preview(p) => self.preview_mouse_down(p, x, y),
            Hit::ColSep(i) => {
                let col = self.layout.columns[i];
                self.drag = Drag::ColSep { col, start_x: x, start_w: self.tab().columns.width(col) };
            }
            Hit::PanelSep(k) if button == MouseButton::Left => self.press_sep(k, x, y, clicks),
            _ => {}
        }
    }

    fn mouse_up(&mut self, x: f32, y: f32, button: MouseButton, now: Instant) {
        if self.autoscroll_up(button) {
            return;
        }
        self.ops_mouse_up(x, y, button, now);
        self.strip_mouse_up(x, y, button);
        self.end_marquee();
        let was = std::mem::replace(&mut self.drag, Drag::None);
        let pressed = std::mem::replace(&mut self.pressed, Hit::None);
        self.dirty = true;
        if let Drag::PinMove { from, to } = was {
            return self.drop_pin(from, to);
        }
        if let Drag::PanelSep { .. } = was {
            self.strip.rev += 1;
        }
        if let Drag::Tab { torn: true, .. } = was {
            return self.tear_off(x, y);
        }
        if self.toolbar_mouse_up(x, y, button) || button != MouseButton::Left || was != Drag::None {
            return;
        }
        let h = self.hit(x, y);
        if h != pressed || self.jobs_click(h, now) {
            return;
        }
        self.click(h, x, y, now);
    }

    fn click(&mut self, h: Hit, x: f32, y: f32, now: Instant) {
        match h {
            Hit::TabClose(i) => self.close_tab(i),
            Hit::NewTab => self.new_tab_here(now),
            Hit::Nav(0) => self.go_history(false, now),
            Hit::Nav(1) => self.go_history(true, now),
            Hit::Nav(2) => self.go_up(now),
            Hit::Nav(_) => self.visited_menu(),
            Hit::ViewMode => self.view_mode_menu(),
            Hit::Bookmark => self.pin_current(),
            Hit::PanelMenu => self.panel_quick_menu(),
            Hit::CrumbScroll(right) => self.scroll_crumbs(right),
            Hit::GoButton(k) => self.go_button(k, now),
            Hit::QuickClear => self.clear_quick_filter(),
            Hit::Caption(0) => self.effects.push(Effect::Window(WindowCmd::Minimize)),
            Hit::Caption(1) => self.effects.push(Effect::Window(WindowCmd::ToggleMaximize)),
            Hit::Caption(_) => self.effects.push(Effect::Window(WindowCmd::Close)),
            Hit::Menu => self.open_main_menu(),
            Hit::QuickToggle => self.toggle_sidebar(),
            Hit::Crumb(i) => {
                if i < self.tab().crumbs.len() {
                    self.open_crumb(i);
                } else if let Some((_, p)) = self.tab().crumb(i).cloned() {
                    self.navigate(p, now);
                }
            }
            Hit::Header(c) => {
                let (col, r) = (self.layout.columns[c], self.layout.cols[c]);
                if col == Column::Type && self.funnel_rect(r).contains(x, y) {
                    self.open_types();
                } else if col == Column::Name && self.prefs.checkboxes && self.header_check(r).inset(-self.s(4.0), -self.s(4.0)).contains(x, y) {
                    self.toggle_select_all();
                } else if let Some(f) = col.field() {
                    self.sort_column(f);
                }
            }
            Hit::Group(g) => self.toggle_group(g),
            Hit::Pane(i, Some(row)) => self.open_pane_row(i, row, now),
            Hit::Encoding => self.pick_encoding(),
            Hit::Bottom(b) => self.bottom_click(b),
            Hit::Branch => self.git_branches_menu(),
            Hit::Preview(p) => self.preview_click(p),
            Hit::Bin(k) => self.bin_button(k),
            Hit::Side(i) => self.open_side_item(i, self.mods.ctrl, now),
            Hit::SidePin(i) => self.toggle_pin_item(i, now),
            Hit::SideHeader(s) => self.toggle_section(s),
            Hit::HomeItem(i) => self.open_home_item(i, self.mods.ctrl, now),
            Hit::FilterClose => self.close_filter(),
            Hit::PaletteRow(i) => self.run_palette_item(i, now),
            Hit::PaletteClose => self.close_palette(),
            Hit::PaletteDescribe => self.toggle_descriptions(),
            Hit::Tool(t) => self.toolbar_click(t, now),
            Hit::TabActions => self.tab_actions_menu(),
            Hit::Eject => self.eject(),
            Hit::SideSettings => self.open_settings(None),
            Hit::Settings(s) => self.settings_click(s, x, y),
            _ => {}
        }
    }

    fn new_tab_here(&mut self, now: Instant) {
        let folder = self.current_location().clone();
        self.dispatch(vec![Route { folder, select: None }], OpenTarget::NewTab, now);
    }

    fn click_row(&mut self, row: usize, button: MouseButton, clicks: u8, now: Instant) {
        let m = self.mods;
        let plain = !m.ctrl && !m.shift;
        let opened_by_first_click = std::mem::take(&mut self.clicked_open);
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
        if clicks >= 2 && plain && !opened_by_first_click {
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
        if clicks == 1 && plain && self.single_click_opens(i) {
            self.clicked_open = true;
            self.open_entry(i, now);
        }
    }

    fn single_click_opens(&self, i: u32) -> bool {
        let t = self.tab();
        let Some(v) = t.view.as_ref().filter(|_| !t.view_loc.is_recycle_bin()) else { return false };
        match v.listing().entries.get(i as usize).is_some_and(|e| e.is_dir()) {
            true if t.look == LayoutMode::Columns => self.single_click_columns.opens(self.touch_input),
            true => self.single_click_folders.opens(self.touch_input),
            false => self.single_click_files.opens(self.touch_input),
        }
    }

    fn open_in_tabs(&mut self, routes: Vec<Route>, now: Instant) {
        let back = self.cur;
        self.dispatch(routes, OpenTarget::NewTab, now);
        if !self.switch_to_new_tab && self.cur != back {
            self.select_tab(back);
        }
    }

    fn key(&mut self, key: Key, m: Mods, now: Instant) {
        self.dirty = true;
        if self.toolbar_key(key) || self.jobs_key(key, m) || self.settings_key(key, m) || self.ops_key(key, m) || self.strip_key(key) {
            return;
        }
        if self.focus_key(key, m, now) {
            return;
        }
        if self.nav_key(key, m, now) {
            return;
        }
        match key {
            Key::BrowserBack => self.go_history(false, now),
            Key::BrowserForward => self.go_history(true, now),
            _ => {}
        }
    }

    fn char_input(&mut self, c: char, now: Instant) {
        if c.is_control() {
            return;
        }
        self.dirty = true;
        if self.form_insert(c.encode_utf8(&mut [0; 4])) || self.jobs.modal() || self.settings_insert(c.encode_utf8(&mut [0; 4])) || self.ops_char(c) {
            return;
        }
        if self.nav_insert(c.encode_utf8(&mut [0; 4])) {
            return;
        }
        if self.mods.ctrl != self.mods.alt {
            return;
        }
        if c == ' ' && !self.tab().typing(now) {
            return;
        }
        if self.tab_mut().type_ahead(c, now) {
            self.reveal_focus(false);
        }
    }

    fn compute_dir_sizes(&mut self) {
        let t = self.tab();
        if t.view.as_ref().is_none_or(|v| v.snap().gen != t.gen) || t.view_loc.is_recycle_bin() || crate::library::is_library(t.view_loc.path()) {
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


    pub fn frame<B: TextBackend>(&mut self, g: &mut Gfx<B>, now: Instant) -> bool {
        let _span = tracing::trace_span!("ui_frame").entered();
        let dt = self.last_frame.map_or(0.0, |t| now.duration_since(t).as_secs_f32()).min(0.1);
        self.last_frame = Some(now);
        self.poll();
        self.dirty = false;
        self.motion = false;
        g.dl.round = self.round_corners;
        self.apply_tag_fixes(now);
        self.ops_frame(now);
        let mut animating = self.marquee_autoscroll(dt);
        animating |= self.autoscroll_step(dt);
        animating |= self.scroll_step(dt);
        animating |= self.glide_overlays(dt);
        animating |= self.step_parked(dt);
        self.layout();
        self.sync_preview(now);

        let mut arena = std::mem::take(&mut self.arena);
        arena.reset();
        self.fit_columns(g, &arena);
        self.fit_parked(g, &arena);
        self.fit_sidebar(g);
        self.look.font_missing = !g.set_ui_family(&self.look.font);
        let palette = self.look.palette;
        let (th, files) = (&palette.base, palette.area(Area::Files));
        for _ in 0..2 {
            arena.reset();
            g.begin(files.bg);
            self.draw_backdrop(&mut g.dl);
            if self.search.active {
                self.clear_parked_rows();
                self.draw_search(g, &arena, th);
                self.draw_title(g, th);
                self.draw_search_bar(g, &arena, th);
                self.draw_chrome(g, th);
                self.draw_sidebar(g, palette.area(Area::Sidebar));
            } else {
                self.draw_panels(g, &arena, &palette);
                self.draw_chrome(g, th);
                self.draw_sidebar(g, palette.area(Area::Sidebar));
                self.draw_kb_focus(g, th);
                self.draw_toolbar_popups(g, th);
            }
            self.draw_settings(g, th);
            if !self.search.active {
                self.draw_palette(g, th);
            }
            self.draw_jobs(g, th);
            self.draw_confirm(g, th);
            self.draw_own_menu(g, th);
            self.draw_tooltip(g, th);
            self.draw_quick_tip(g, th);
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
        animating || self.motion || g.atlas_reset
    }

    fn smooth(&self) -> bool {
        self.smooth_scroll && self.animations
    }

    fn scroll_step(&mut self, dt: f32) -> bool {
        let smooth = self.smooth();
        let t = self.tab_mut();
        glide(&mut t.scroll, t.scroll_target, dt, smooth)
    }

    fn glide_overlays(&mut self, dt: f32) -> bool {
        let smooth = self.smooth();
        let settings = self.settings.as_mut().is_some_and(|ui| ui.scroll.step(dt, smooth));
        let palette = self.nav.palette.as_mut().is_some_and(|p| p.scroll.step(dt, smooth));
        settings | palette
    }

    fn draw_title<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme) {
        let l = &self.layout;
        g.dl.rect(l.title, t.chrome);
        let font = self.ui_font(12.5);
        let m = g.metrics(font);
        let close_font = self.icon_font(8.0);
        let drop = self.drop_spot();
        let mut requests: Vec<IconRequest> = Vec::new();
        for (i, &(r, c)) in l.tabs.iter().enumerate().filter(|(_, (r, _))| r.w > 0.0) {
            let active = i == self.cur;
            let hovered = matches!(self.hover, Hit::Tab(k) | Hit::TabClose(k) if k == i);
            if active {
                g.dl.round_rect(Rect::new(r.x, r.y, r.w, r.h + self.s(8.0)), t.bg, self.s(6.0));
            } else if hovered {
                g.dl.round_rect(r.inset(0.0, self.s(2.0)), t.button_hover, self.s(6.0));
            }
            if drop == Some(Spot::Tab(i)) {
                let target = r.inset(0.0, self.s(2.0));
                g.dl.round_rect(target, t.selection, self.s(6.0));
                g.dl.outline(target, t.accent, self.s(6.0), self.s(1.0).max(1.0));
            }
            let baseline = (r.y + (r.h + m.ascent - m.descent) / 2.0).round();
            let label = self.tabs[i].title();
            let torn = active && matches!(self.drag, Drag::Tab { torn: true, .. });
            let color = if torn { t.text_dim.alpha(0x60) } else if active { t.text } else { t.text_dim };
            let icon = Rect::new(r.x + self.s(10.0), r.y + (r.h - self.s(16.0)) / 2.0, self.s(16.0), self.s(16.0));
            let loc = self.tabs[i].location();
            if !self.system_icon(g, sidebar::folder_icon(loc, &self.nav.sidebar, icon.w as u32), icon, icon.w, if torn { 0x60 } else { 0xFF }, &mut requests) {
                centered(g, icon, tabbar::tab_glyph(loc), self.icon_font(12.0), if torn { color } else { t.accent.alpha(0xC0) });
            }
            g.text(icon.right() + self.s(6.0), baseline, &label, font, color, c.x - icon.right() - self.s(10.0));
            if active || hovered {
                let state = widgets::Button::of(self.hover == Hit::TabClose(i), false, true);
                widgets::Paint::new(g, t, self.scale).icon_button(c, G_CLOSE, close_font, state, t.text_dim);
            }
        }
        let mut w = widgets::Paint::new(g, t, self.scale);
        if drop == Some(Spot::NewTab) {
            w.g.dl.round_rect(l.new_tab, t.teal, self.s(4.0));
            w.centered(l.new_tab, G_ADD, self.icon_font(10.0), theme::ink_on(t.teal));
        } else {
            w.icon_button(l.new_tab, G_ADD, self.icon_font(10.0), widgets::Button::of(self.hover == Hit::NewTab, false, true), t.text_dim);
        }
        if l.actions.w > 0.0 {
            let state = widgets::Button::of(self.hover == Hit::TabActions, self.menu_open_from(Hit::TabActions), true);
            w.icon_button(l.actions, tabbar::G_TAB_ACTIONS, self.icon_font(12.0), state, t.text_dim);
        }
        self.request_icons(requests);
    }

    fn draw_body<B: TextBackend>(&mut self, g: &mut Gfx<B>, arena: &Bump, palette: &theme::Palette) {
        let (th, files) = (&palette.base, palette.area(Area::Files));
        let active = self.panels.here == self.panels.active;
        if self.on_home() {
            self.draw_home(g, files);
        } else {
            self.draw_list(g, arena, files);
            self.draw_panes(g, files);
            if active {
                self.draw_preview(g, palette.area(Area::Pane));
                self.draw_marquee(g, files);
                self.draw_ops(g, files);
            }
            self.draw_header(g, files);
        }
        self.draw_title(g, th);
        self.draw_toolbar(g, palette.area(Area::Toolbar));
        self.draw_toolbar_row(g, palette.area(Area::Toolbar));
        if active && self.layout.status.h > 0.0 {
            self.draw_status(g, arena, th);
        }
    }

    fn draw_toolbar<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme) {
        let l = &self.layout;
        g.dl.rect(l.toolbar, t.bg);
        let icons = self.icon_font(12.0);
        let tab = self.tab();
        let enabled = [tab.history.can_back(), tab.history.can_forward(), tab.location().parent().is_some(), !self.nav.sidebar.prefs.quick.visits.is_empty()];
        let mut w = widgets::Paint::new(g, t, self.scale);
        for (i, glyph) in [G_BACK, G_FORWARD, G_UP, sidebar::G_EXPANDED].into_iter().enumerate() {
            let state = widgets::Button::of(self.hover == Hit::Nav(i), self.menu_open_from(Hit::Nav(i)), enabled[i]);
            let font = if i == 3 { self.icon_font(8.0) } else { icons };
            w.icon_button(l.nav[i].inset(self.s(2.0), 0.0), glyph, font, state, t.text);
        }
        let sep = |r: Rect| Rect::new(r.x - self.s(5.0), r.y + self.s(8.0), 1.0, r.h - self.s(16.0));
        w.g.dl.rect(sep(l.view), t.separator);
        w.g.dl.rect(sep(Rect::new(l.address.x, l.view.y, 0.0, l.view.h)), t.separator);
        let view = widgets::Button::of(self.hover == Hit::ViewMode, self.menu_open_from(Hit::ViewMode), true);
        w.icon_button(l.view.inset(self.s(2.0), 0.0), details::view_mode_glyph(self.view_mode()), icons, view, t.text);
        let marked = self.nav.sidebar.is_pinned(self.current_location());
        let bookmark = widgets::Button::of(self.hover == Hit::Bookmark, marked, true);
        w.icon_button(l.bookmark.inset(self.s(2.0), 0.0), sidebar::G_PIN, icons, bookmark, if marked { t.accent } else { t.text_dim });
        let more = widgets::Button::of(self.hover == Hit::PanelMenu, self.menu_open_from(Hit::PanelMenu), true);
        w.icon_button(l.more.inset(self.s(2.0), 0.0), G_MORE, icons, more, t.text);
        let address = *self.look.palette.area(Area::Address);
        self.draw_address(g, &address);
        self.draw_ops_button(g, t);
    }

    fn draw_address<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme) {
        let r = self.layout.address;
        if t.bg != self.look.palette.area(Area::Toolbar).bg {
            g.dl.round_rect(r, t.bg, self.s(4.0));
        }
        if self.hover == Hit::Address {
            g.dl.round_rect(r, t.hover, self.s(4.0));
        }
        let font = self.ui_font(13.0);
        let m = g.metrics(font);
        let baseline = (r.y + (r.h + m.ascent - m.descent) / 2.0).round();
        self.draw_crumbs(g, t, r, baseline, font);
    }

    fn draw_crumbs<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme, area: Rect, baseline: f32, font: FontStyle) {
        let chevron = self.icon_font(8.0);
        let gap = self.s(6.0);
        let chev_w = g.measure(G_CHEVRON, chevron) + 2.0 * self.s(2.0);
        let here = self.current_location().clone();
        if self.addr.shift_for.as_ref() != Some(&here) {
            self.addr.shift = 0.0;
            self.addr.shift_for = Some(here);
        }
        let tab = &self.tabs[self.cur];
        let real = tab.crumbs.len();
        let crumbs: Vec<&(String, Location)> = tab.crumbs.iter().chain(&tab.ghosts).collect();
        let open = match self.nav.palette.as_ref().map(|p| &p.pick) {
            Some(palette::Pick::Crumb(i)) => Some(*i),
            _ => None,
        };
        let mark_w = g.measure(sidebar::G_EXPANDED, chevron) + gap;
        let widths: Vec<f32> = crumbs.iter().enumerate().map(|(i, (label, _))| g.measure(label, font) + 2.0 * gap + if open == Some(i) { mark_w } else { 0.0 }).collect();
        let n = widths.len();
        let span = |end: usize| widths[..end].iter().sum::<f32>() + chev_w * end.saturating_sub(1) as f32;
        let (all, upto) = (span(n), span(real));
        let arrow_w = self.s(18.0);
        let over = all > area.w;
        let view = if over { Rect::new(area.x + 2.0 * arrow_w, area.y, area.w - 2.0 * arrow_w, area.h) } else { area };
        let x0 = if upto > view.w { view.right() - upto } else { view.x };
        let (lo, hi) = ((view.right() - (x0 + all)).min(0.0), (view.x - x0).max(0.0));
        self.addr.shift = self.addr.shift.clamp(lo, hi);
        let mut x = x0 + self.addr.shift;
        let (s1, s3, s6) = (self.s(1.0), self.s(3.0), self.s(6.0));
        let drop = self.drop_spot();
        let (left, right) = (x < view.x - 0.5, x + all > view.right() + 0.5);
        self.layout.crumb_arrows = if over { [Rect::new(area.x, area.y + s3, arrow_w, area.h - s6), Rect::new(area.x + arrow_w, area.y + s3, arrow_w, area.h - s6)] } else { [Rect::default(); 2] };
        let mut rects = vec![Rect::default(); n];
        g.dl.set_clip(Some(view));
        for i in 0..n {
            let w = widths[i];
            let cr = Rect::new(x, area.y + s3, w, area.h - s6);
            if cr.right() > view.x && cr.x < view.right() {
                let (a, b) = (cr.x.max(view.x), cr.right().min(view.right()));
                rects[i] = Rect::new(a, cr.y, b - a, cr.h);
            }
            let lit = open == Some(i);
            if drop == Some(Spot::Crumb(i)) {
                g.dl.round_rect(cr, t.selection, s3);
                g.dl.outline(cr, t.accent, s3, s1.max(1.0));
            } else if lit {
                g.dl.round_rect(cr, t.accent_fill, s3);
            } else if self.hover == Hit::Crumb(i) {
                g.dl.round_rect(cr, t.button_hover, s3);
            }
            let ink = match () {
                _ if lit => theme::ink_on(t.accent_fill),
                _ if i + 1 == real => t.text,
                _ if i >= real => t.text_dim.alpha(0x80),
                _ => t.text_dim,
            };
            g.text(x + gap, baseline, &crumbs[i].0, font, ink, f32::INFINITY);
            if lit {
                g.text(x + w - mark_w, baseline - s1, sidebar::G_EXPANDED, chevron, ink, f32::INFINITY);
            }
            x += w;
            if i + 1 < n {
                g.text(x + self.s(2.0), baseline - s1, G_CHEVRON, chevron, t.text_dim.alpha(if i + 1 >= real { 0x80 } else { 0xFF }), f32::INFINITY);
                x += chev_w;
            }
        }
        g.dl.set_clip(None);
        self.layout.crumbs = rects;
        if over {
            let mut w = widgets::Paint::new(g, t, self.scale);
            for (k, (glyph, on)) in [(G_CRUMB_LEFT, left), (G_CRUMB_RIGHT, right)].into_iter().enumerate() {
                w.icon_button(self.layout.crumb_arrows[k], glyph, chevron, widgets::Button::of(self.hover == Hit::CrumbScroll(k == 1), false, on), t.text_dim);
            }
        }
    }

    pub(super) fn crumb_rect(&self, i: usize) -> Option<Rect> {
        self.layout.crumbs.get(i).copied().filter(|r| r.w > 0.0).or(Some(self.layout.address))
    }

    fn scroll_crumbs(&mut self, right: bool) {
        let step = self.s(CRUMB_STEP);
        self.addr.shift += if right { -step } else { step };
        self.dirty = true;
    }

    fn draw_header<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme) {
        let l = &self.layout;
        if l.header.h <= 0.0 {
            return;
        }
        g.dl.rect(l.header, t.header);
        g.dl.rect(Rect::new(l.header.x, l.header.bottom() - 1.0, l.header.w, 1.0), t.separator);
        let font = self.ui_font(12.0);
        let m = g.metrics(font);
        let baseline = (l.header.y + (l.header.h + m.ascent - m.descent) / 2.0).round();
        let pad = self.s(10.0);
        let sort = self.tab().view_spec.sort;
        for (i, col) in l.columns.iter().enumerate() {
            let r = l.cols[i];
            if self.hover == Hit::Header(i) {
                g.dl.rect(r, t.button_hover);
            }
            if i > 0 {
                g.dl.rect(Rect::new(r.x, r.y + self.s(6.0), 1.0, r.h - self.s(12.0)), t.separator);
            }
            g.dl.set_clip(Some(r));
            let sorted = Some(sort.field) == col.field();
            let mut x = r.x + pad;
            if *col == Column::Name && self.prefs.checkboxes {
                let b = self.header_check(r);
                let all = self.tab().view.as_ref().is_some_and(|v| !v.rows().is_empty() && v.selected_count() == v.rows().len());
                self.draw_checkbox(g, t, b, all);
                x = b.right() + self.s(6.0);
            }
            let mut room = r.right() - pad - x;
            if *col == Column::Type {
                let fr = self.funnel_rect(r);
                let active = !self.tab().hidden_types.is_empty();
                centered(g, fr, G_FUNNEL, self.icon_font(11.0), if active { t.accent } else { t.text_dim });
                room = fr.x - x;
            }
            if sorted {
                let glyph = if sort.descending { G_SORT_DOWN } else { G_SORT_UP };
                let gw = self.s(14.0);
                let right = if *col == Column::Type { self.funnel_rect(r).x } else { r.right() - pad };
                g.text(right - gw, baseline, glyph, self.icon_font(8.0), t.text, f32::INFINITY);
                room = room.min(right - gw - x);
            }
            g.text(x, baseline, col.title(), font, if sorted { t.text } else { t.text_dim }, room.max(0.0));
            g.dl.set_clip(None);
        }
    }

    fn draw_list<B: TextBackend>(&mut self, g: &mut Gfx<B>, arena: &Bump, t: &Theme) {
        let l = &self.layout;
        let (list, mode) = (l.list, l.mode);
        let font = self.ui_font(13.0);
        let m = g.metrics(font);
        let baseline = |r: Rect| (r.y + (r.h + m.ascent - m.descent) / 2.0).round();
        let pad = self.s(10.0);
        let icon_px = l.icon;
        let list_focused = self.active && self.focus == Focus::List;
        let tab = &self.tabs[self.cur];
        let in_bin = tab.view_loc.is_recycle_bin();
        let f = self.flow();
        let lines = self.visible_lines(&f);
        g.dl.set_clip(Some(list));
        let mut requests: Vec<IconRequest> = Vec::new();
        let mut thumb_requests: Vec<IconRequest> = Vec::new();
        let mut type_requests: Vec<Vec<u8>> = Vec::new();
        let want_thumbs = self.prefs.thumbnails && matches!(mode, LayoutMode::Grid | LayoutMode::Cards) && !in_bin;
        let thumb_px = thumbs::class_px(icon_px as u32);
        if let Some(v) = &tab.view {
            if let Some(e) = v.snap().error.as_ref().filter(|_| v.total_rows() == 0) {
                self.draw_unavailable(g, t, list, e);
            } else if v.rows().is_empty() && v.groups().is_empty() && v.snap().done {
                let msg = if v.is_filtered() {
                    S::UiEmptyFiltered
                } else if in_bin {
                    S::UiEmptyBin
                } else {
                    S::UiEmptyFolder
                }
                .t();
                let w = g.measure(msg, font);
                g.text(list.x + (list.w - w) / 2.0, list.y + self.s(40.0), msg, font, t.text_dim, f32::INFINITY);
            }
            let loc = &tab.view_loc;
            let shell_dir = loc.to_windows();
            let in_library = crate::library::is_library(loc.path());
            let in_root = loc.path().kind() == PathKind::Computer;
            let listing = v.listing();
            let cut = self.cut_names();
            let want_types = !in_root && (mode == LayoutMode::Cards || l.columns.contains(&Column::Type) || v.snap().view.group.field == GroupField::Type);
            let mut want_type = |i: u32| {
                let folded = listing.ext(i).to_ascii_lowercase();
                if want_types && !listing.entries[i as usize].is_dir() && !self.type_names.contains_key(&folded) && !type_requests.contains(&folded) {
                    type_requests.push(folded);
                }
            };
            for line in lines {
                let cell = self.line_rect(&f, line);
                let row = match v.line(line) {
                    Some(Line::Row(r)) => r,
                    Some(Line::Header(k)) => {
                        want_type(v.groups()[k].first);
                        self.draw_group_line(g, arena, t, v, k, cell);
                        continue;
                    }
                    None => break,
                };
                let i = v.rows()[row];
                let e = listing.entries[i as usize];
                let p = self.item_parts(cell);
                let rr = p.body;
                if v.is_selected(i) {
                    g.dl.round_rect(rr, if list_focused { t.selection } else { t.selection_inactive }, self.s(4.0));
                } else if self.hover == Hit::Row(row) && self.drag == Drag::None {
                    g.dl.round_rect(rr, t.hover, self.s(4.0));
                }
                if list_focused && v.focus == Some(i) {
                    g.dl.outline(rr, t.focus_ring, self.s(4.0), 1.0);
                }
                if self.prefs.checkboxes {
                    self.draw_checkbox(g, t, p.check, v.is_selected(i));
                }

                let name = listing.name(i);
                let faded = e.attrs & attr::HIDDEN != 0 || cut.is_some_and(|c| c.contains(name));
                let row_dir = if in_library { FsPath::parse_exact(crate::library::folder(listing, i)) } else { None };
                let dir = row_dir.as_ref().or(shell_dir.as_ref());
                let dir_key = if in_bin { recycle::origin(listing, i) } else { dir.map_or(&b""[..], |d| d.as_bytes()) };
                let (key, kind) = icon_key(name, listing.ext(i), &e, icon_px as u32, dir_key, self.icon_epoch);
                let ir = p.icon;
                let alpha = if faded { 0x70 } else { 0xFF };
                let thumb = (want_thumbs && !e.is_dir() && has_thumbnail(listing.ext(i))).then(|| hash::hash(&("thumb", dir_key, name, e.modified, e.size, thumb_px)));
                let thumbed = thumb.is_some_and(|k| g.thumb(k, ir, alpha));
                if let (Some(k), false, Some(d)) = (thumb, thumbed, dir) {
                    if !self.thumbs.contains_key(&k) && !thumb_requests.iter().any(|r| r.key == k) {
                        thumb_requests.push(IconRequest { key: k, kind: IconKind::Thumbnail, path: d.join(name), size: thumb_px, gen: tab.gen, row: row as u32 });
                    }
                }
                let badge = tab.git_status(name).map(|st| {
                    let d = (ir.h * 0.42).max(self.s(9.0)).round();
                    (Rect::new(ir.right() - d * 0.8, ir.bottom() - d * 0.8, d, d), st)
                });
                if !thumbed && !g.image_alpha(key, ir, alpha) {
                    icon_glyph(g, t, &e, ir);
                    let wanted = !self.icons.contains_key(&key) && !requests.iter().any(|r| r.key == key);
                    let path = match (wanted, in_bin) {
                        (false, _) => None,
                        (true, true) => recycle::recycled(listing, i),
                        (true, false) => row_dir.as_ref().map_or_else(|| loc.join(name).to_windows(), |d| Some(d.join(name))),
                    };
                    if let Some(path) = path {
                        requests.push(IconRequest { key, kind, path, size: icon_px as u32, gen: tab.gen, row: row as u32 });
                    }
                }

                let ty = baseline(p.name);
                let picked = v.is_selected(i) && list_focused;
                let name_color = if picked { t.selection_text } else if faded { t.text_dim } else { t.text };
                if let Some((br, st)) = badge {
                    g.dl.round_rect(br, git::status_color(st), br.w / 2.0);
                    centered(g, br, st.letter(), FontStyle::new(FontFamily::Ui, br.h * 0.72, true), Color::rgb(0xFF_FF_FF));
                }
                let end = self.draw_name(g, &p, &self.shown_name(listing, i), e.tags, font, name_color);
                if mode == LayoutMode::Tree && e.is_dir() {
                    if let Some(cr) = self.chevron_rect(row) {
                        let open = self.is_open_in_tree(i);
                        centered(g, cr, if open { G_SORT_DOWN } else { G_CHEVRON }, self.icon_font(8.0), t.text_dim);
                    }
                }
                let folder = listing.folder(i);
                if !folder.is_empty() && mode != LayoutMode::Tree && !p.center {
                    let label = arena.alloc_str(&format!("{NESTED_MARK}{}", crate::name::display(folder)));
                    let x = end + self.s(8.0);
                    g.text(x, ty, label, font, t.text_dim.alpha(0xA0), (p.name.right() - x).max(0.0));
                }
                want_type(i);
                match mode {
                    LayoutMode::Details | LayoutMode::Tree => {
                        for (c, &col) in l.columns.iter().enumerate().skip(1) {
                            let r = l.cols[c];
                            let s = self.cell_text(arena, listing, i, col);
                            let x = if col == Column::Size { (r.right() - pad - g.measure(s, font)).max(r.x + pad) } else { r.x + pad };
                            g.text(x, ty, s, font, if picked { t.selection_text } else { t.text_dim }, r.w - 2.0 * pad);
                        }
                    }
                    LayoutMode::Cards => {
                        let size = self.cell_text(arena, listing, i, Column::Size);
                        let last = if size.is_empty() { self.cell_text(arena, listing, i, Column::Modified) } else { size };
                        for (k, s) in [self.type_text(arena, listing, i), last].into_iter().enumerate() {
                            let r = Rect { y: p.name.y + (k + 1) as f32 * p.name.h, ..p.name };
                            g.text(r.x, baseline(r), s, self.ui_font(12.0), t.text_dim, r.w);
                        }
                    }
                    LayoutMode::Grid if tab.mode.zoomed() => {
                        let size = self.cell_text(arena, listing, i, Column::Size);
                        let small = self.ui_font(11.0);
                        let r = Rect { y: p.name.bottom(), h: self.s(14.0), ..p.name };
                        let w = g.measure(size, small);
                        g.text(r.x + ((r.w - w) / 2.0).max(0.0).round(), baseline(r), size, small, t.text_dim, r.w);
                    }
                    _ => {}
                }
            }
        }
        g.dl.set_clip(None);
        self.request_icons(requests);
        for r in thumb_requests {
            self.thumbs.insert(r.key, IconState::Pending);
            self.effects.push(Effect::Thumbnail(r));
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
}

impl App {
    fn request_icons(&mut self, requests: Vec<IconRequest>) {
        for r in requests {
            self.icons.insert(r.key, IconState::Pending);
            self.effects.push(Effect::Icon(r));
        }
    }

    fn system_icon<B: TextBackend>(&self, g: &mut Gfx<B>, icon: Option<sidebar::SystemIcon>, r: Rect, px: f32, alpha: u8, requests: &mut Vec<IconRequest>) -> bool {
        let Some((key, kind, path)) = icon else { return false };
        if g.image_alpha(key, Rect::new((r.x + (r.w - px) / 2.0).round(), (r.y + (r.h - px) / 2.0).round(), px, px), alpha) {
            return true;
        }
        if !self.icons.contains_key(&key) && !requests.iter().any(|q| q.key == key) {
            requests.push(IconRequest { key, kind, path, size: px as u32, gen: 0, row: 0 });
        }
        false
    }
}

fn row_hit(l: &Layout, x: f32, y: f32) -> Hit {
    if l.actions.contains(x, y) {
        return Hit::TabActions;
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
    Hit::None
}

fn icon_glyph<B: TextBackend>(g: &mut Gfx<B>, t: &Theme, e: &crate::listing::Entry, ir: Rect) {
    let glyph = if e.attrs & attr::VOLUME != 0 { G_DRIVE } else if e.is_dir() { G_FOLDER } else { G_FILE };
    let color = if e.is_dir() { t.accent.alpha(0xB0) } else { t.text_dim.alpha(0x90) };
    centered(g, ir, glyph, FontStyle::new(FontFamily::Icons, ir.h * 0.85, false), color);
}

fn centered<B: TextBackend>(g: &mut Gfx<B>, r: Rect, s: &str, font: FontStyle, color: Color) {
    let w = g.measure(s, font);
    let m = g.metrics(font);
    let x = (r.x + (r.w - w) / 2.0).round();
    let y = (r.y + (r.h + m.ascent - m.descent) / 2.0).round();
    g.text(x, y, s, font, color, f32::INFINITY);
}

pub fn icon_key(name: &[u8], ext: &[u8], e: &crate::listing::Entry, size: u32, dir: &[u8], epoch: u64) -> (u64, IconKind) {
    if e.attrs & attr::VOLUME != 0 {
        return (hash::hash(&("vol", name, size)), IconKind::Volume);
    }
    if e.is_dir() && e.attrs & (attr::READONLY | attr::SYSTEM) != 0 {
        return (hash::hash(&("custom", dir, name, e.modified, e.attrs, size, epoch)), IconKind::CustomFolder);
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

#[cfg(test)]
#[path = "../../test/ui/budgets.rs"]
mod budgets;
