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
mod tabbar;
mod tags;
mod split;
mod describe;
mod keymap;
mod toolbar;
mod popup;
mod own_menu;
mod properties;
mod git;

pub use ops::{base_name_len, batch_names, changes_extension, choose_effect, drop_effect, same_volume, unique_name, validate_name, DragEvent, NewTemplate, ShellMenu, Spot, SHELL_VERBS};
pub use search::{thousands, IndexState, IndexVolume, SearchBackend, SearchEvent, SearchKey, SearchRequest, SearchRow, SearchScope, SearchSort, SearchSource, MAX_HISTORY};
pub use sidebar::{Drive, KnownFolder, NavPrefs, NavState, Places, Section};
pub use home::{Recent, Widget};
pub const UI_SCALE: (u32, u32) = (70, 200);
pub const SPRING_DELAY_MAX_MS: u32 = 3000;
pub use details::{folder_key, Columns, FolderView, FolderViews, ViewPrefs, THUMB_CACHE_MB};
pub use layouts::{adaptive_layout, has_thumbnail, is_media, LayoutMode, Sizes, LAYOUTS};
pub use jobs::JobState;
pub use preview_pane::PaneState;
pub use menu::MenuItem;
pub use popup::{MenuOutcome, MenuView};
pub use tab::Tab;
pub use toolbar::{Show, ToolContext, ToolItem, ToolbarState, CONTEXTS};
pub use address::Typed;
pub use tabbar::{MovedTab, Startup, WindowSession};
pub use settings::{ConfirmDelete, Doc, Page, Setting, Value, SETTINGS};
pub use settings_page::{About, LOG_FILE};
pub use keymap::Keymap;
pub use split::{SplitDir, SplitPrefs, SplitSession};

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
use crate::theme::{Area, Theme};
use crate::thumbs;
use crate::view::Line;
use bumpalo::Bump;
use edit::{EditAction, LineEdit};
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
const G_BRANCH: &str = "\u{2387}";

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
    GitStatus,
    GitMessage,
    GitAuthor,
    GitDate,
}

pub const GIT_COLUMNS: [Column; 4] = [Column::GitStatus, Column::GitMessage, Column::GitAuthor, Column::GitDate];
pub const REPO_COLUMNS: [Column; 10] = [Column::Name, Column::Modified, Column::Created, Column::Type, Column::Size, Column::Tag, Column::GitStatus, Column::GitMessage, Column::GitAuthor, Column::GitDate];
pub const COLUMNS: [Column; 6] = [Column::Name, Column::Modified, Column::Created, Column::Type, Column::Size, Column::Tag];
pub const BIN_COLUMNS: [Column; 4] = [Column::Name, Column::Origin, Column::Deleted, Column::Size];

impl Column {
    fn title(self) -> &'static str {
        match self {
            Column::Name => S::DetailsColumnName,
            Column::Modified => S::DetailsColumnModified,
            Column::Created => S::DetailsColumnCreated,
            Column::Type => S::DetailsColumnType,
            Column::Size => S::DetailsColumnSize,
            Column::Origin => S::DetailsColumnOrigin,
            Column::Deleted => S::DetailsColumnDeleted,
            Column::Tag => S::DetailsColumnTag,
            Column::GitStatus => S::GitColumnStatus,
            Column::GitMessage => S::GitColumnMessage,
            Column::GitAuthor => S::GitColumnAuthor,
            Column::GitDate => S::GitColumnDate,
        }
        .t()
    }
    fn field(self) -> Option<SortField> {
        Some(match self {
            Column::Name => SortField::Name,
            Column::Modified | Column::Deleted => SortField::Modified,
            Column::Created => SortField::Created,
            Column::Type => SortField::Type,
            Column::Size => SortField::Size,
            Column::Origin => SortField::Origin,
            Column::Tag => SortField::Tag,
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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
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
    pub split: SplitPrefs,
    pub startup: Startup,
}

impl Default for Persisted {
    fn default() -> Self {
        Self { path: Location::computer(), show_hidden: false, view: FolderView::default(), prefs: ViewPrefs::default(), folders: FolderViews::default(), archive: Default::default(), split: SplitPrefs::default(), startup: Startup::default() }
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
    Sizes,
    Branch,
    Bin(usize),
    Side(usize),
    SidePin(usize),
    SideEmpty,
    SideHeader(Section),
    HomeItem(usize),
    HomeEmpty,
    HomeButton,
    Splitter,
    FilterField,
    FilterClose,
    PaletteRow(usize),
    PaletteField,
    PaletteBox,
    PaletteOutside,
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
    CrumbMenu(usize),
    CrumbOverflow,
    Suggestion(usize),
    Eject,
    SideSettings,
    Settings(settings_page::SHit),
    PaneSplit,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Drag {
    None,
    Thumb { grab: f32 },
    ColSep { col: Column, start_x: f32, start_w: f32 },
    EditSelect,
    Splitter { start_x: f32, start_w: f32 },
    PinMove { from: usize, to: usize },
    Preview { hit: preview_pane::PaneHit, start: f32, size: f32 },
    Marquee,
    Tab { origin: usize, x: f32, y: f32, moved: bool, torn: bool },
    PaneSplit { start: f32, ratio: f32 },
}

#[derive(Default, Clone)]
struct Layout {
    title: Rect,
    tabs: Vec<(Rect, Rect)>,
    new_tab: Rect,
    actions: Rect,
    caption: [Rect; 3],
    toolbar: Rect,
    commands: Rect,
    nav: [Rect; 4],
    home: Rect,
    address: Rect,
    ops_button: Rect,
    header: Rect,
    columns: Vec<Column>,
    cols: Vec<Rect>,
    list: Rect,
    track: Rect,
    thumb: Rect,
    status: Rect,
    sizes: Rect,
    branch: Rect,
    bin: [Rect; 3],
    crumbs: Vec<Rect>,
    row_h: f32,
    mode: LayoutMode,
    icon: f32,
    flow: flow::FlowParams,
    panes: Vec<Rect>,
    bottom: f32,
    pane: Rect,
    content: Rect,
    splitter: Rect,
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
    pub archives_as_folders: bool,
    pub spring_delay_ms: u32,
    pub switch_to_new_tab: bool,
    pub smooth_scroll: bool,
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
    pub language: usize,
    shown_lang: crate::i18n::Lang,
    pub split_prefs: SplitPrefs,
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
    pub preedit: crate::input::Preedit,

    focus: Focus,
    kb: usize,
    a11y_targets: Vec<(u64, Hit)>,
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
    alt: split::Alt,
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
            archives_as_folders: false,
            spring_delay_ms: 1300,
            switch_to_new_tab: true,
            smooth_scroll: true,
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
            language: 0,
            shown_lang: crate::i18n::lang(),
            split_prefs: state.split,
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
            preedit: Default::default(),
            focus: Focus::List,
            kb: 0,
            a11y_targets: Vec::new(),
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
            alt: Default::default(),
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
        for t in &mut self.tabs {
            t.shutdown(&mut self.effects);
        }
        self.cancel_ops();
    }

    pub fn persisted(&self) -> Persisted {
        let t = self.tab();
        Persisted { path: t.location().clone(), show_hidden: t.view_spec.show_hidden, view: t.folder_view(), prefs: self.prefs.clone(), folders: self.folders.clone(), archive: self.archive.prefs.clone(), split: self.split_prefs, startup: self.startup.clone() }
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
        self.search_ime_area().or_else(|| self.settings_ime_area()).or(self.editing.then_some(self.layout.address)).or_else(|| self.nav_ime_area()).or_else(|| self.rename_ime_area())
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
        self.dual_new_tab(now);
        self.after_tab_change();
    }

    fn after_tab_change(&mut self) {
        self.editing = false;
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
        let mut visible = false;
        for k in 0..self.tabs.len() {
            visible |= self.poll_tab(k);
            visible |= self.tabs[k].apply_diffs() && k == self.cur;
            if self.tabs[k].split.is_some() {
                visible |= self.on_other_at(k, |a| a.poll_tab(k) | a.tabs[k].apply_diffs());
            }
        }
        self.poll_bin();
        visible |= self.poll_panes();
        visible |= self.poll_address();
        self.dirty |= visible;
        visible
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
        if resort {
            self.resort(k);
        }
        if loaded {
            self.on_loaded(k);
        }
        k == self.cur
    }

    pub fn on_provider_event(&mut self, ev: ProviderEvent, now: Instant) {
        if let Some(k) = self.inactive_of_event(&ev) {
            return self.on_other_at(k, |a| a.on_provider_event(ev, now));
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
            ProviderEvent::DirSize { tab, gen, idx, size } => {
                if let Some(k) = tab_of(&self.tabs, tab, gen) {
                    if let Some(v) = self.tabs[k].view.as_mut().filter(|v| v.snap().gen == gen) {
                        v.set_dir_size(idx, size);
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
            ProviderEvent::Measured { id, totals, file_system, done, .. } => self.measured(id, totals, file_system, done),
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
        if let Some(k) = self.inactive_of(tab) {
            return self.on_other_at(k, |a| a.on_resorted(tab, gen, view, base_len, order));
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
        if !self.editing && self.nav_insert(text.lines().next().unwrap_or("")) {
            return;
        }
        if self.editing {
            self.edit.insert(text.lines().next().unwrap_or(""));
            self.dirty = true;
        }
    }

    fn resort(&mut self, k: usize) {
        self.sync_spec(k);
        let t = &self.tabs[k];
        if let Some(v) = &t.view {
            self.effects.push(Effect::Resort { tab: t.id, gen: t.gen, snap: v.snap().clone(), view: t.view_spec });
        }
    }

    fn layout(&mut self) {
        let (w, h) = self.size;
        let cx = self.sidebar_width();
        let ops_button = self.ops_button_visible();
        let status_bar = self.prefs.status_bar || self.tab().error.is_some();
        let tool_h = self.toolbar_h();
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
        l.actions = if self.tab_actions_button { Rect::new(l.caption[0].x - s(44.0), s(4.0), s(36.0), s(28.0)) } else { Rect::default() };
        let avail = (l.caption[0].x - s(48.0) - l.actions.w - x0 - add_w).max(0.0);
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
        l.home = Rect::new(l.nav[3].right(), l.nav[3].y, s(NAV_W), l.nav[3].h);
        let ax = l.home.right() + s(8.0);
        let right = if ops_button { w - s(10.0) - s(NAV_W) } else { w };
        l.ops_button = if ops_button { Rect::new(right + s(4.0), l.nav[0].y, s(NAV_W), l.nav[0].h) } else { Rect::default() };
        l.address = Rect::new(ax, l.toolbar.y + s(5.0), (right - s(12.0) - ax).max(s(40.0)), s(TOOL_H - 10.0));
        l.commands = Rect::new(0.0, l.toolbar.bottom(), w, tool_h);
        l.header = Rect::new(cx, l.commands.bottom(), w - cx, s(HEADER_H));
        let status_h = if status_bar { s(STATUS_H) } else { 0.0 };
        l.status = Rect::new(0.0, h - status_h, w, status_h);
        if !status_bar {
            l.sizes = Rect::default();
            l.bin = Default::default();
        }
        self.layout_preview();
        let l = &self.layout;
        let area = Rect::new(l.header.x, l.commands.bottom(), l.header.w, (l.bottom - l.commands.bottom()).max(0.0));
        let (mine, theirs) = self.split_areas(area);
        if let Some(r) = theirs {
            self.layout_other(r);
        }
        self.layout_pane(mine);
        self.layout_nav();
        self.layout_jobs();
        self.layout_address();
        self.layout_settings();
    }

    fn layout_pane(&mut self, area: Rect) {
        let filter_h = self.filter_bar_h();
        let (kinds, widths) = (self.column_kinds(), self.tab().columns);
        let sc = self.scale;
        let s = |v: f32| (v * sc).round();
        let l = &mut self.layout;
        l.pane = area;
        l.header = Rect::new(area.x, area.y + filter_h, area.w, s(HEADER_H));
        l.bottom = area.bottom();
        self.layout_look();
        let l = &mut self.layout;
        l.columns.clear();
        l.columns.extend(kinds.iter().copied().filter(|&c| widths.shown(c)));
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
        self.layout_filter(area.x, area.y);
        self.layout_home();
    }

    fn hit(&self, x: f32, y: f32) -> Hit {
        if let Some(h) = self.jobs_hit(x, y).or_else(|| self.palette_hit(x, y)).or_else(|| self.settings_hit(x, y)).or_else(|| self.flyout_hit(x, y)) {
            return h;
        }
        if let Some(h) = self.address_hit(x, y).or_else(|| self.nav_hit(x, y)).or_else(|| self.preview_hit(x, y)).or_else(|| self.toolbar_hit(x, y)) {
            return h;
        }
        let l = &self.layout;
        if l.title.contains(x, y) {
            if let Some(i) = l.caption.iter().position(|r| r.contains(x, y)) {
                return Hit::Caption(i);
            }
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
            return Hit::None;
        }
        if l.toolbar.contains(x, y) {
            if let Some(i) = l.nav.iter().position(|r| r.contains(x, y)) {
                return Hit::Nav(i);
            }
            if l.home.contains(x, y) {
                return Hit::HomeButton;
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
        if self.on_splitter(x, y) {
            return Hit::PaneSplit;
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
        if l.sizes.contains(x, y) {
            return Hit::Sizes;
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
        match self.hit(x, y) {
            Hit::Caption(0) => HitArea::Minimize,
            Hit::Caption(1) => HitArea::Maximize,
            Hit::Caption(_) => HitArea::Close,
            Hit::None if self.layout.title.contains(x, y) => HitArea::Caption,
            _ => HitArea::Client,
        }
    }

    pub fn on_event(&mut self, ev: &Event, now: Instant) {
        if self.own_menu_event(ev, now) {
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
                if self.settings_wheel(*dy) || self.wheel_other(ev, now) || self.jobs_wheel(*dy) || self.nav_wheel(*dy) || self.pane_wheel(*dy) || self.preview_wheel(*dy) || self.strip_wheel(*dy) {
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
                self.toolbar_timer(now);
                self.history_timer(now);
                self.tip_timer(now);
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
        self.strip_pointer(x, y, now);
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
            Drag::EditSelect => {
                self.pending_edit = Some((x, true));
                self.dirty = true;
                return;
            }
            Drag::Splitter { start_x, start_w } => return self.drag_splitter(x, start_x, start_w),
            Drag::PinMove { from, .. } => return self.drag_pin(from, y),
            Drag::Tab { .. } => return self.drag_tab(x, y),
            Drag::Preview { hit, start, size } => return self.preview_drag(hit, x, y, start, size),
            Drag::PaneSplit { start, ratio } => return self.drag_pane_split(x, y, start, ratio),
            Drag::Marquee | Drag::None => {}
        }
        if self.start_pin_drag(y) {
            return;
        }
        let h = self.hit(x, y);
        let cursor = match h {
            Hit::ColSep(_) | Hit::Splitter => CursorIcon::ResizeHorizontal,
            Hit::Address if self.editing => CursorIcon::Text,
            Hit::FilterField | Hit::PaletteField | Hit::Settings(settings_page::SHit::Search) => CursorIcon::Text,
            Hit::Preview(preview_pane::PaneHit::Split) => self.preview_cursor(),
            Hit::PaneSplit => self.split_cursor(),
            _ => CursorIcon::Default,
        };
        self.set_cursor(cursor);
        self.hover_pick(h, now);
        self.toolbar_hover(h, now);
        if h != self.hover {
            self.hover = h;
            self.dirty = true;
        }
    }

    fn mouse_down(&mut self, x: f32, y: f32, button: MouseButton, clicks: u8, now: Instant) {
        self.dirty = true;
        if self.form_down(x, y, clicks) {
            return;
        }
        if !self.settings_open() {
            self.pane_press(x, y);
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
            (MouseButton::Middle, _) => return,
            _ => {}
        }
        self.pressed = h;
        if self.editing && h != Hit::Address {
            self.cancel_edit();
        }
        match h {
            Hit::Tab(i) => self.press_tab(i, x, y, button),
            Hit::Nav(i @ 0..=1) if button == MouseButton::Left => self.arm_history_press(i, now),
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
                if button == MouseButton::Left && self.on_checkbox(x, y) {
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
            Hit::PaneSplit if button == MouseButton::Left => self.begin_pane_split(x, y, clicks),
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
        if let Drag::PaneSplit { .. } = was {
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
                if let Some(f) = self.layout.columns[c].field() {
                    self.sort_column(f);
                }
            }
            Hit::Group(g) => self.toggle_group(g),
            Hit::Pane(i, Some(row)) => self.open_pane_row(i, row, now),
            Hit::Sizes => self.sizes_button(),
            Hit::Branch => self.git_branches_menu(),
            Hit::Preview(p) => self.preview_click(p),
            Hit::Bin(k) => self.bin_button(k),
            Hit::Side(i) => self.open_side_item(i, self.mods.ctrl, now),
            Hit::SidePin(i) => self.toggle_pin_item(i),
            Hit::SideHeader(s) => self.toggle_section(s),
            Hit::HomeItem(i) => self.open_home_item(i, self.mods.ctrl, now),
            Hit::HomeButton => self.go_home_page(self.mods.ctrl, now),
            Hit::FilterClose => self.close_filter(),
            Hit::PaletteRow(i) => self.run_palette_item(i, now),
            Hit::Tool(t) => self.toolbar_click(t, now),
            Hit::TabActions => self.tab_actions_menu(),
            Hit::CrumbMenu(i) => self.crumb_menu(i),
            Hit::CrumbOverflow => self.overflow_menu(),
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
        if self.editing {
            if self.address_key(key, m) {
                return;
            }
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
        let mut moved = None;
        match (key, m.ctrl, m.alt) {
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
            (Key::Up | Key::Down | Key::Left | Key::Right, _, false) => {
                self.arrow_key(key, m, now);
            }
            (Key::PageUp, _, false) => moved = Some(-page),
            (Key::PageDown, _, false) => moved = Some(page),
            (Key::Home, _, false) => moved = Some(isize::MIN / 2),
            (Key::End, _, false) => moved = Some(isize::MAX / 2),
            _ => {}
        }
        if let Some(d) = moved {
            self.tab_mut().move_focus(d, m);
            self.reveal_focus(false);
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
        if c == ' ' && !self.tab().typing(now) {
            return self.quick_view();
        }
        if self.tab_mut().type_ahead(c, now) {
            self.reveal_focus(false);
        }
    }

    fn begin_edit(&mut self) {
        self.editing = true;
        self.focus = Focus::Address;
        let loc = self.current_location();
        let text = if matches!(loc.path().kind(), PathKind::Computer | PathKind::Home) { String::new() } else { loc.display().into_owned() };
        self.edit.set(&text);
        self.edit.select_all();
        self.edit_scroll = 0.0;
        self.addr.seen = None;
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
        if self.typed_address() {
            return;
        }
        if Location::parse(&self.edit.text).as_ref() == Some(self.current_location()) {
            return self.refresh(now);
        }
        self.effects.push(Effect::Open(cmd));
        self.dirty = true;
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

    fn copy_paths(&mut self, quoted: bool) {
        let t = self.tab();
        let quote = |l: &Location| if quoted { format!("\"{}\"", l.display()) } else { l.display().into_owned() };
        let sel: Vec<u32> = t.view.as_ref().map(|v| v.selected().collect()).unwrap_or_default();
        let text = if sel.is_empty() { quote(t.location()) } else { sel.iter().map(|&i| quote(&t.entry_location(i))).collect::<Vec<_>>().join("\r\n") };
        self.effects.push(Effect::SetClipboard(text));
    }

    pub fn frame<B: TextBackend>(&mut self, g: &mut Gfx<B>, now: Instant) -> bool {
        let _span = tracing::trace_span!("ui_frame").entered();
        let dt = self.last_frame.map_or(0.0, |t| now.duration_since(t).as_secs_f32()).min(0.1);
        self.last_frame = Some(now);
        self.poll();
        self.dirty = false;
        self.apply_tag_fixes(now);
        self.ops_frame(now);
        let mut animating = self.marquee_autoscroll(dt);
        animating |= self.autoscroll_step(dt);
        animating |= self.scroll_step(dt);
        animating |= self.glide_overlays(dt);
        animating |= self.step_other(dt);
        self.layout();
        self.sync_preview(now);

        let mut arena = std::mem::take(&mut self.arena);
        arena.reset();
        self.fit_columns(g, &arena);
        self.fit_other(g, &arena);
        self.fit_sidebar(g);
        self.look.font_missing = !g.set_ui_family(&self.look.font);
        let palette = self.look.palette;
        let (th, files) = (&palette.base, palette.area(Area::Files));
        for _ in 0..2 {
            arena.reset();
            g.begin(files.bg);
            self.draw_backdrop(&mut g.dl);
            if self.search.active {
                self.draw_search(g, &arena, th);
            } else {
                self.draw_other(g, &arena, files);
                if self.on_home() {
                    self.draw_home(g, files);
                } else {
                    self.draw_list(g, &arena, files);
                    self.draw_panes(g, files);
                    self.draw_preview(g, palette.area(Area::Pane));
                    self.draw_marquee(g, files);
                    self.draw_ops(g, files);
                    self.draw_header(g, files);
                }
                self.draw_split(g, files);
            }
            self.draw_title(g, th);
            if self.search.active {
                self.draw_search_bar(g, &arena, th);
                self.draw_sidebar(g, palette.area(Area::Sidebar));
            } else {
                self.draw_toolbar(g, palette.area(Area::Toolbar));
                self.draw_toolbar_row(g, palette.area(Area::Toolbar));
                if self.layout.status.h > 0.0 {
                    self.draw_status(g, &arena, th);
                }
                self.draw_sidebar(g, palette.area(Area::Sidebar));
                self.draw_kb_focus(g, th);
                self.draw_filter(g, files);
                self.draw_toolbar_popups(g, th);
                self.draw_suggestions(g, th);
            }
            self.draw_settings(g, th);
            if !self.search.active {
                self.draw_palette(g, th);
            }
            self.draw_tab_tip(g, th);
            self.draw_jobs(g, th);
            self.draw_own_menu(g, th);
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

    fn scroll_step(&mut self, dt: f32) -> bool {
        let smooth = self.smooth_scroll;
        let t = self.tab_mut();
        glide(&mut t.scroll, t.scroll_target, dt, smooth)
    }

    fn glide_overlays(&mut self, dt: f32) -> bool {
        let smooth = self.smooth_scroll;
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
        for (i, &(r, c)) in l.tabs.iter().enumerate() {
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
            centered(g, icon, tabbar::tab_glyph(self.tabs[i].location()), self.icon_font(12.0), if torn { color } else { t.accent.alpha(0xC0) });
            g.text(icon.right() + self.s(6.0), baseline, &label, font, color, c.x - icon.right() - self.s(10.0));
            if active || hovered {
                if self.hover == Hit::TabClose(i) {
                    g.dl.round_rect(c, t.button_hover, self.s(4.0));
                }
                centered(g, c, G_CLOSE, close_font, t.text_dim);
            }
        }
        if drop == Some(Spot::NewTab) {
            g.dl.round_rect(l.new_tab, t.selection, self.s(4.0));
            g.dl.outline(l.new_tab, t.accent, self.s(4.0), self.s(1.0).max(1.0));
        } else if self.hover == Hit::NewTab {
            g.dl.round_rect(l.new_tab, t.button_hover, self.s(4.0));
        }
        centered(g, l.new_tab, G_ADD, self.icon_font(10.0), t.text_dim);
        if l.actions.w > 0.0 {
            if self.hover == Hit::TabActions {
                g.dl.round_rect(l.actions, t.button_hover, self.s(4.0));
            }
            centered(g, l.actions, tabbar::G_TAB_ACTIONS, self.icon_font(12.0), t.text_dim);
        }

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
        if self.hover == Hit::HomeButton {
            let c = if self.pressed == Hit::HomeButton { t.button_pressed } else { t.button_hover };
            g.dl.round_rect(l.home.inset(self.s(2.0), 0.0), c, self.s(4.0));
        }
        centered(g, l.home, sidebar::G_HOME, icons, if self.on_home() { t.accent } else { t.text });
        let address = *self.look.palette.area(Area::Address);
        self.draw_address(g, &address);
        self.draw_ops_button(g, t);
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
            let line = field::EditLine { text: &self.edit.text, caret: self.edit.caret, selection: self.edit.selection() };
            let look = field::EditLook { font, baseline, line_y: r.y + self.s(6.0), line_h: r.h - self.s(12.0), stroke: self.s(1.0).max(1.0), focused: true };
            let preedit = Some(&self.preedit).filter(|p| !p.is_empty());
            field::draw_line(g, t, inner, &mut self.edit_scroll, line, preedit, look);
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
        let ov_w = g.measure(address::OVERFLOW, font) + 2.0 * gap;
        let n = widths.len();
        let fits = |first: usize| widths[first..].iter().sum::<f32>() + chev_w * (n - first) as f32 + if first > 0 { ov_w } else { 0.0 } <= inner.w;
        let mut first = 0;
        while first + 1 < n && !fits(first) {
            first += 1;
        }
        let (s1, s3, s6) = (self.s(1.0), self.s(3.0), self.s(6.0));
        let drop = self.drop_spot();
        let rects = &mut self.layout.crumbs;
        rects.clear();
        rects.resize(n, Rect::default());
        self.addr.chevrons.clear();
        self.addr.chevrons.resize(n, Rect::default());
        let mut x = inner.x - gap;
        self.addr.overflow = Rect::default();
        if first > 0 {
            let r = Rect::new(x, inner.y + s3, ov_w, inner.h - s6);
            if self.hover == Hit::CrumbOverflow {
                g.dl.round_rect(r, t.button_hover, s3);
            }
            g.text(x + gap, baseline, address::OVERFLOW, font, t.text_dim, f32::INFINITY);
            self.addr.overflow = r;
            x += ov_w;
        }
        for i in first..n {
            let w = widths[i];
            let cr = Rect::new(x, inner.y + s3, w, inner.h - s6);
            rects[i] = cr;
            if drop == Some(Spot::Crumb(i)) {
                g.dl.round_rect(cr, t.selection, s3);
                g.dl.outline(cr, t.accent, s3, s1.max(1.0));
            } else if self.hover == Hit::Crumb(i) {
                g.dl.round_rect(cr, t.button_hover, s3);
            }
            let last = i + 1 == n;
            g.text(x + gap, baseline, &crumbs[i].0, font, if last { t.text } else { t.text_dim }, inner.right() - x);
            x += w;
            let chev = Rect::new(x, inner.y + s3, chev_w, inner.h - s6);
            if self.hover == Hit::CrumbMenu(i) {
                g.dl.round_rect(chev, t.button_hover, s3);
            }
            g.text(x + gap, baseline - s1, G_CHEVRON, chevron, t.text_dim, f32::INFINITY);
            self.addr.chevrons[i] = chev;
            x += chev_w;
        }
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
            let w = g.text(r.x + pad, baseline, col.title(), font, t.text_dim, r.w - 2.0 * pad);
            if Some(sort.field) == col.field() {
                let glyph = if sort.descending { G_SORT_DOWN } else { G_SORT_UP };
                g.text(r.x + pad + w + self.s(6.0), baseline, glyph, self.icon_font(8.0), t.text_dim, f32::INFINITY);
            }
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
                self.draw_name(g, &p, &self.shown_name(listing, i), e.tags, font, name_color);
                want_type(i);
                match mode {
                    LayoutMode::Details => {
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
        } else if let Some(v) = tab.view.as_ref().filter(|v| v.snap().gen == tab.gen && !tab.location().is_home_page()) {
            self.count_text(&mut s, v);
            let k = v.selected_count();
            if k > 0 {
                s.push_str("    ");
                crate::trn_into!(&mut s, P::UiStatusSelected, k);
                let size = v.selected_size();
                if size > 0 {
                    s.push_str("  ");
                    format::size_in(&mut s, size, self.style.decimal);
                }
            }
            if let Some(total) = self.unpacked_total() {
                s.push_str("    ");
                let mut size = String::new();
                format::size_in(&mut size, total, self.style.decimal);
                crate::tr_into!(&mut s, S::UiStatusUnpacked, size = size);
            }
            if !v.snap().done {
                s.push_str("    ");
                s.push_str(S::UiStatusLoading.t());
            }
        } else if let Some(l) = &tab.loading {
            s.push_str(l.note.as_deref().unwrap_or(S::UiStatusLoading.t()));
        }
        g.text(r.x + pad, baseline, &s, font, color, r.w / 2.0);

        let mut right = bumpalo::collections::String::new_in(arena);
        if let (Some(first), Some(done)) = (tab.stats.first, tab.stats.done) {
            let (mut a, mut b) = (bumpalo::collections::String::new_in(arena), bumpalo::collections::String::new_in(arena));
            crate::i18n::decimal(&mut a, first.as_secs_f64() * 1e3, 1);
            crate::i18n::decimal(&mut b, done.as_secs_f64() * 1e3, 1);
            crate::tr_into!(&mut right, S::UiStatusTiming, first = a, all = b);
        }
        let in_bin = tab.view_loc.is_recycle_bin();
        let sizes = self.sizes_label();
        let sizes = [sizes.as_str()];
        let bin = BIN_BUTTONS.map(S::t);
        let labels: &[&str] = if in_bin { &bin } else if sizes[0].is_empty() { &[] } else { &sizes };
        let mut buttons = [Rect::default(); 3];
        let mut x = r.right() - pad;
        for (k, label) in labels.iter().enumerate().rev() {
            let w = g.measure(label, font) + self.s(16.0);
            x -= w;
            let br = Rect::new(x, r.y + self.s(2.0), w, r.h - self.s(4.0));
            buttons[k] = br;
            if self.hover == if in_bin { Hit::Bin(k) } else { Hit::Sizes } {
                g.dl.round_rect(br, t.button_hover, self.s(3.0));
            }
            g.text(br.x + self.s(8.0), baseline, label, font, t.text_dim, f32::INFINITY);
            x -= self.s(4.0);
        }
        self.layout.sizes = if in_bin { Rect::default() } else { buttons[0] };
        self.layout.bin = if in_bin { buttons } else { Default::default() };
        self.layout.branch = Rect::default();
        if let Some(branch) = self.git_branch().filter(|b| !b.is_empty() && !in_bin) {
            let label = arena.alloc_str(&format!("{G_BRANCH} {branch}"));
            let w = g.measure(label, font) + self.s(16.0);
            x -= w;
            let br = Rect::new(x, r.y + self.s(2.0), w, r.h - self.s(4.0));
            if self.hover == Hit::Branch {
                g.dl.round_rect(br, t.button_hover, self.s(3.0));
            }
            g.text(br.x + self.s(8.0), baseline, label, font, t.text_dim, f32::INFINITY);
            self.layout.branch = br;
            x -= self.s(4.0);
        }
        let rw = g.measure(&right, font);
        g.text(x - self.s(12.0) - rw, baseline, &right, font, t.text_dim.alpha(0xA0), f32::INFINITY);
    }
}

impl App {
    fn request_icons(&mut self, requests: Vec<IconRequest>) {
        for r in requests {
            self.icons.insert(r.key, IconState::Pending);
            self.effects.push(Effect::Icon(r));
        }
    }
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
