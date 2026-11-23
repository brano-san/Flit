pub mod edit;
mod a11y;
mod admin;
mod file_icons;
mod shell_props;
mod shell_overlays;
mod focus;
#[cfg(test)]
#[path = "../../test/ui/harness.rs"]
mod harness;
mod appearance;
mod archive;
mod batch_rename;
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
mod view_rules;
mod marquee;
mod menu;
mod menu_editor;
mod nav;
pub use nav::PrimaryPaste;
mod network;
mod network_listing;
mod volumes;
pub mod palette;
mod preview_pane;
pub mod settings;
pub mod background;
mod picker;
mod settings_import;
mod segments;
mod dedup;
mod portable_meta;
mod disk_changes;
mod settings_page;
mod setting_editor;
pub mod sidebar;
mod address;
mod goto;
mod quick;
mod tabbar;
mod dock;
mod tags;
mod panels;
mod describe;
mod keymap;
mod popup;
mod context;
pub mod options;
mod own_menu;
mod open_with;
mod prefetch;
mod confirm;
mod fuzzy;
mod tooltip;
mod widgets;
mod properties;
mod permissions;
mod git;
mod tree;
mod bottom;
mod types;
mod user_menu;
mod update;
mod update_dialog;
mod pointer;
mod touch;
mod tab_navigation;
mod provider_events;
mod window_layout;
mod events;
mod frame;

pub use ops::{base_name_len, batch_names, changes_extension, choose_effect, drop_effect, same_volume, unique_name, validate_name, DragEvent, NewTemplate, ShellMenu, Spot, TemplateNames, SHELL_VERBS};
pub use palette::CommandUsage;
pub use search::{thousands, IndexState, IndexVolume, SearchBackend, SearchEvent, SearchKey, SearchRequest, SearchRow, SearchScope, SearchSort, SearchSource, MAX_HISTORY};
pub use sidebar::{Drive, KnownFolder, NavPrefs, NavState, Places, QuickPrefs, SavedServer, Section};
pub use home::{Recent, Widget};
pub use network::{saved_address, server_uri, ServerChange};
pub const UI_SCALE: (u32, u32) = (70, 200);
pub const SPRING_DELAY_MAX_MS: u32 = 3000;
pub use details::{folder_key, Columns, FolderView, FolderViews, ViewMode, ViewPrefs, EXTRA, ORDERED, THUMB_CACHE_MB};
pub use layouts::{adaptive_layout, has_thumbnail, is_media, LayoutMode, Sizes, LAYOUTS};
pub use view_rules::{count_pictures, PictureShare};
pub use jobs::JobState;
pub use preview_pane::PaneState;
pub use menu::{Mark, MenuImage, MenuItem};
pub use user_menu::UserEntry;
pub use update::UpdatePrefs;
pub use popup::{MenuOutcome, MenuView};
pub use context::MenuKind;
pub use tab::Tab;
pub use shell_overlays::{OverlayBatch, OverlayItem, OverlayReply, OVERLAY_BATCH, OVERLAY_SLOTS};
pub use bottom::WatchLimit;
pub use address::Typed;
pub use tabbar::{MovedTab, Startup, WindowSession};
pub use settings::{ConfirmDelete, Doc, Page, Setting, Value, SETTINGS};
pub use settings_page::{About, MissingTool, SystemChanges, LOG_FILE};
pub use keymap::{set_unix_defaults, Keymap};
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
use crate::provider::{GenToken, ProviderEvent, ProviderRequest, Resolve, TabId};
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
const THUMB_MIN_ICON: f32 = 32.0;
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
const TAB_MIN_W: f32 = 100.0;
const TAB_ARROW_W: f32 = 24.0;

const MAX_PENDING_DIFFS: usize = 1 << 16;
const SCROLL_TAU: f32 = 0.045;
const IDLE_DT: f32 = 1.0 / 120.0;

fn frame_dt(last: Option<Instant>, now: Instant, moving: bool) -> f32 {
    let dt = last.map_or(0.0, |t| now.saturating_duration_since(t).as_secs_f32());
    dt.min(if moving { 0.1 } else { IDLE_DT })
}

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
pub const MAX_ROUTES: usize = 32;

const PER_FILE_ICON_EXT: [&[u8]; 7] = [b"exe", b"ico", b"lnk", b"url", b"cur", b"ani", b"desktop"];

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
    Folders,
    Accessed,
    Volume,
    FolderUsage,
    Free,
    FileSystem,
    Percent,
    Files,
    Attributes,
    ImageSize,
    Duration,
    Artist,
    Version,
    DateTaken,
    CameraModel,
    Album,
    Title,
    Authors,
    Company,
    GitStatus,
    GitMessage,
    GitAuthor,
    GitDate,
    GitHash,
    Rating,
    Comment,
    PortableTags,
}

pub const GIT_COLUMNS: [Column; 5] = [Column::GitStatus, Column::GitMessage, Column::GitAuthor, Column::GitDate, Column::GitHash];
pub const SHELL_COLUMNS: [Column; 10] = [Column::ImageSize, Column::Duration, Column::Artist, Column::Version, Column::DateTaken, Column::CameraModel, Column::Album, Column::Title, Column::Authors, Column::Company];
pub const PORTABLE_COLUMNS: [Column; 3] = [Column::Rating, Column::Comment, Column::PortableTags];
pub const REPO_COLUMNS: [Column; 19] = [
    Column::Name,
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
    Column::GitStatus,
    Column::GitMessage,
    Column::GitAuthor,
    Column::GitDate,
    Column::GitHash,
    Column::Rating,
    Column::Comment,
    Column::PortableTags,
];
pub const COLUMNS: [Column; 14] = [Column::Name, Column::Type, Column::Modified, Column::Folders, Column::Files, Column::Size, Column::Volume, Column::Created, Column::Accessed, Column::FolderUsage, Column::Tag, Column::Rating, Column::Comment, Column::PortableTags];
pub const BIN_COLUMNS: [Column; 4] = [Column::Name, Column::Origin, Column::Deleted, Column::Size];
pub const COMPUTER_COLUMNS: [Column; 6] = [Column::Name, Column::Type, Column::Size, Column::Free, Column::Percent, Column::FileSystem];
pub const MENU_COLUMNS: [Column; 10] = [Column::Name, Column::Type, Column::Size, Column::Folders, Column::Files, Column::Created, Column::Modified, Column::Accessed, Column::Volume, Column::FolderUsage];

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
            Column::Folders => S::DetailsColumnFolders,
            Column::Files => S::DetailsColumnFiles,
            Column::Accessed => S::DetailsColumnAccessed,
            Column::Volume => S::DetailsColumnVolume,
            Column::FolderUsage => S::DetailsColumnFolderUsage,
            Column::Free => S::DetailsColumnFree,
            Column::FileSystem => S::DetailsColumnFileSystem,
            Column::Percent => S::DetailsColumnPercent,
            Column::Attributes => S::DetailsColumnAttributes,
            Column::ImageSize => S::DetailsColumnDimensions,
            Column::Duration => S::DetailsColumnDuration,
            Column::Artist => S::DetailsColumnArtist,
            Column::Version => S::DetailsColumnVersion,
            Column::DateTaken => S::DetailsColumnDateTaken,
            Column::CameraModel => S::DetailsColumnCameraModel,
            Column::Album => S::DetailsColumnAlbum,
            Column::Title => S::DetailsColumnTitle,
            Column::Authors => S::DetailsColumnAuthors,
            Column::Company => S::DetailsColumnCompany,
            Column::GitStatus => S::GitColumnStatus,
            Column::GitMessage => S::GitColumnMessage,
            Column::GitAuthor => S::GitColumnAuthor,
            Column::GitDate => S::GitColumnDate,
            Column::GitHash => S::GitColumnHash,
            Column::Rating => S::PortableMetaRating,
            Column::Comment => S::PortableMetaComment,
            Column::PortableTags => S::PortableMetaTags,
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
            Column::Folders => SortField::Folders,
            Column::Files => SortField::Files,
            Column::Accessed => SortField::Accessed,
            Column::Volume => SortField::Volume,
            Column::FolderUsage => SortField::FolderUsage,
            Column::Free => SortField::Free,
            Column::FileSystem => SortField::FileSystem,
            Column::Percent => SortField::Percent,
            Column::Attributes => SortField::Attributes,
            Column::ImageSize => SortField::Dimensions,
            Column::Duration => SortField::Duration,
            Column::Artist => SortField::Artist,
            Column::Version => SortField::Version,
            Column::GitHash => SortField::GitHash,
            Column::DateTaken | Column::CameraModel | Column::Album | Column::Title | Column::Authors | Column::Company | Column::GitStatus | Column::GitMessage | Column::GitAuthor | Column::GitDate | Column::Rating | Column::Comment | Column::PortableTags => {
                return None
            }
        })
    }

    pub fn is_git(self) -> bool {
        (Column::GitStatus as u8..=Column::GitHash as u8).contains(&(self as u8))
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WindowCmd {
    Minimize,
    ToggleMaximize,
    Close,
    Quit,
    KeepOpen,
    Fullscreen,
    Compact(Option<bool>),
    Attention,
    ServerDecorations(bool),
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
    Content,
    LinkEmblem,
    UnreadableEmblem,
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
    Overlays(OverlayBatch),
    ClearThumbnails,
    TypeName(Vec<u8>),
    SetClipboard(String),
    SetPrimary(String),
    PastePrimary(PrimaryPaste),
    SetClipboardImage(Bitmap),
    Paste,
    Window(WindowCmd),
    Notify(String),
    Cursor(CursorIcon),
    Title(String),
    NewWindow(Vec<Route>),
    NewNetworkWindow(Vec<Route>),
    Open(Open),
    SetClipboardFiles { paths: Vec<FsPath>, cut: bool },
    LinkChoices { generation: u64, items: Option<Vec<FsPath>>, dest: FsPath },
    ShellMenu(ShellMenu),
    ShellPrefetch(Option<ShellMenu>),
    ShellWarm(Vec<ShellMenu>),
    Properties(Vec<FsPath>),
    StartDrag(Vec<FsPath>),
    ShellProps { tab: TabId, gen: u64, ask: u64, dir: FsPath, names: Vec<(Vec<u8>, bool)>, shell: bool, portable: bool },
    PortableMetadata { id: u64, req: crate::portable_meta::Request },
    DiskChanges { id: u64, request: crate::disk_changes::Request },
    Pinned(Vec<Location>),
    PinnedCleared,
    SavedServer(ServerChange),
    PinnedFiles(Vec<Location>),
    PinFolders(Vec<FsPath>),
    CheckPlaces(Vec<FsPath>),
    Library(crate::library::LibraryOp),
    RefreshPlaces,
    Search(SearchRequest),
    Preview(PreviewRequest),
    PrettyPreview(PreviewRequest),
    PreviewPdfPage { req: PreviewRequest, page: u32 },
    ViewPictures { tab: TabId, ask: u64, snap: Arc<crate::listing::DirSnapshot>, cancel: GenToken },
    QuickView(FsPath),
    Backdrop { gen: u64, path: FsPath },
    Host(HostCmd),
    Timer(Duration),
    MoveTab { tab: MovedTab, at: Option<(f32, f32)>, last: bool },
    TabOver(Option<(f32, f32)>),
    RestoreSession,
    Tags(TagRequest),
    Settings(SettingsCmd),
    Share(Vec<FsPath>),
    SetVolumeLabel { drive: FsPath, label: String },
    Git(crate::git::GitRequest),
    ShellPick(Option<u32>),
    PaletteShell { ticket: u64, menu: ShellMenu },
    PaletteShellPick { ticket: u64, command: Option<u32> },
    Layout(LayoutCmd),
    Update(crate::update::UpdateRequest),
    NetworkDiscovery { tab: TabId, gen: u64 },
    Picked { paths: Vec<FsPath>, filter: usize, choices: crate::pick::Choices },
}

#[derive(Debug, Clone, PartialEq)]
pub enum LayoutCmd {
    Save(String, Box<options::SavedLayout>),
    Load(String),
    Delete(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum SettingsCmd {
    Export { path: FsPath, text: String, replace: bool },
    Import(FsPath),
    CacheSize,
    Fonts,
    Browse { exts: &'static [&'static str], folder: bool, start: Option<FsPath> },
    RaiseWatchLimit,
    RefreshSystemChanges,
    UndoSystemChanges,
    RegisterArchive,
    ArchiveOfferShown,
    ConfirmImport(Box<settings::ImportPlan>),
}

pub enum IconResult {
    Ready(Bitmap),
    Shared(u64, Bitmap),
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
    pub op: Option<Duration>,
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
    CrumbChevron(usize),
    Address,
    Header(usize),
    HeaderEmpty,
    ColSep(usize),
    Row(usize),
    Group(usize),
    Pane(usize, Option<usize>),
    Preview(preview_pane::PaneHit),
    ListEmpty,
    Track,
    Thumb,
    HTrack,
    HThumb,
    Encoding,
    Branch,
    GitAbort,
    GitConflicts,
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
    PaletteKeys(usize),
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
    OpsPause(usize),
    OpsMore(usize),
    OpsMenu(usize),
    Dialog,
    DialogButton(usize),
    DialogCheck,
    Form(usize),
    FormFlag(usize, usize),
    Modal,
    TabActions,
    TabScroll(bool),
    ViewMode,
    Bookmark,
    PanelMenu,
    CrumbScroll(bool),
    GoButton(usize),
    QuickField,
    QuickClear,
    Eject,
    GrantAccess,
    Settings(settings_page::SHit),
    Menu,
    QuickToggle,
    PanelSep(usize),
    Bottom(bottom::BottomHit),
    SearchRow(u64),
    NetworkSettings,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Drag {
    None,
    Thumb { grab: f32 },
    HThumb { grab: f32 },
    ColMove { col: Column, x: f32, grab: f32 },
    ColSep { col: Column, start_x: f32, start: details::Columns, moved: bool },
    Splitter { start_x: f32, start_w: f32 },
    PinMove { from: usize, to: usize },
    PlaceMove { from: usize, to: usize },
    Preview { hit: preview_pane::PaneHit, start: f32, size: f32 },
    Marquee,
    Network,
    Tab { origin: usize, x: f32, y: f32, moved: bool, torn: bool },
    PanelSep { sep: usize, start: f32, first: f32 },
    Slide(usize),
    OptionsThumb { grab: f32 },
    MenuMove { from: usize, to: Option<usize> },
}

#[derive(Default, Clone)]
struct Layout {
    title: Rect,
    tabs: Vec<(Rect, Rect)>,
    new_tab: Rect,
    actions: Rect,
    toolbar: Rect,
    nav: [Rect; 4],
    view: Rect,
    bookmark: Rect,
    more: Rect,
    crumb_arrows: [Rect; 2],
    tab_arrows: [Rect; 2],
    strip: tabbar::StripScroll,
    address: Rect,
    ops_button: Rect,
    header: Rect,
    columns: Vec<Column>,
    cols: Vec<Rect>,
    list: Rect,
    track: Rect,
    thumb: Rect,
    htrack: Rect,
    hthumb: Rect,
    hmax: f32,
    status: Rect,
    bar: Vec<(bottom::BottomHit, Rect)>,
    bar_pieces: [Rect; 2],
    encoding: Rect,
    branch: Rect,
    git_bar: [Rect; 4],
    bin: [Rect; 3],
    crumbs: Vec<Rect>,
    crumb_chevrons: Vec<Rect>,
    row_h: f32,
    mode: LayoutMode,
    icon: f32,
    flow: flow::FlowParams,
    panes: Vec<Rect>,
    pane_area: Rect,
    panes_over: f32,
    bottom: f32,
    gutter: f32,
}

pub struct App {
    pub utc_offset_min: i32,
    pub case: CaseMode,
    pub caps: crate::caps::Caps,
    pub open_target: OpenTarget,
    pub double_click: Duration,
    pub wheel_lines: u32,
    wheel_steps: pointer::WheelSteps,
    pub warn_extension_change: bool,
    pub ops_button_always: bool,
    pub conflict_default: Option<Resolve>,
    pub shell_items_inline: bool,
    raising_watch_limit: bool,
    pub menu_hidden: Vec<&'static str>,
    git_branches: Vec<crate::git::Branch>,
    pub tab_wheel_reversed: bool,
    pub mru_tabs: bool,
    command_usage: Arc<std::sync::Mutex<CommandUsage>>,
    palette_shell_id: u64,
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
    metadata_request_id: u64,
    disk_changes_id: u64,
    pub startup: Startup,
    pub startup_pages: Vec<Location>,
    pub about: About,
    pub log_level: usize,
    pub default_manager: bool,
    pub autostart: bool,
    pub protocol_links: bool,
    pub quick_access_sync: bool,
    pub(super) tray_available: bool,
    pub updates: update::UpdatePrefs,
    pub language: usize,
    pub prefs: ViewPrefs,
    clicked_open: bool,
    menu: Vec<menu::Cmd>,
    focused_at: Option<Instant>,
    pub look: appearance::Look,
    dark: bool,
    scale: f32,
    system_scale: f32,
    system_text: f32,
    size: (f32, f32),
    maximized: bool,
    pub(super) server_frames: Option<bool>,
    pub(super) system_title: bool,
    active: bool,

    quick_tags: bool,
    next_gen: u64,
    timer_at: Option<Instant>,
    error_shown: Option<(TabId, String, Instant)>,
    own_menu: Option<own_menu::OwnMenu>,
    menu_owner: Option<Hit>,
    menu_panel: u32,
    menu_right: Option<f32>,
    tip: Option<tooltip::Tip>,
    quick: Option<quick::QuickTip>,
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
    file_icons: file_icons::FileIcons,
    shell_overlays: shell_overlays::ShellOverlays,
    type_names: FxHashMap<Vec<u8>, Option<String>>,
    adaptive: FxHashMap<u64, LayoutMode>,

    layout: Layout,
    effects: Vec<Effect>,
    arena: Bump,
    last_frame: Option<Instant>,
    moving: bool,
    dirty: bool,
    group_press: Option<usize>,
    nav: nav::Nav,
    marquee: Option<marquee::Marquee>,
    details: details::Details,
    touch: touch::Touch,
    archive: archive::ArchiveUi,
    addr: address::Address,
    primary: String,
    paste_after_layout: Option<PrimaryPaste>,
    ghosts: widgets::Ghosts,
    bar_arrivals: widgets::Arrivals,
    rolls: [bottom::Roll; 2],
    row_glow: widgets::Glow<(TabId, u32)>,
    side_glow: widgets::Glow<usize>,
    head_hot: Option<(usize, bool)>,
    hover_glow: widgets::Glow<(u32, Hit)>,
    head_glow: widgets::Glow<(u32, usize, bool)>,
    prefetch: prefetch::Prefetch,
    shapes: own_menu::Shapes,
    tag_fixes: Vec<(FsPath, u32, Instant)>,
    tabs: tabbar::Tabs,
    ops: ops::Ops,
    search: search::SearchView,
    dialogs: form::Dialogs,
    preview: preview_pane::PreviewPane,
    settings: settings::SettingsState,
}

impl App {
    pub fn new(state: Persisted, start: Vec<Route>, tab_base: TabId, dark: bool, scale: f32, size: (u32, u32), now: Instant) -> Self {
        let mut app = Self {
            utc_offset_min: 0,
            case: CaseMode::Insensitive,
            caps: crate::caps::Caps::default(),
            open_target: OpenTarget::NewTab,
            double_click: Duration::from_millis(500),
            wheel_lines: pointer::WHEEL_LINES,
            wheel_steps: pointer::WheelSteps::default(),
            warn_extension_change: true,
            ops_button_always: false,
            conflict_default: None,
            shell_items_inline: false,
            raising_watch_limit: false,
            menu_hidden: Vec::new(),
            git_branches: Vec::new(),
            tab_wheel_reversed: false,
            mru_tabs: true,
            command_usage: Arc::default(),
            palette_shell_id: 0,
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
            metadata_request_id: 0,
            disk_changes_id: 0,
            startup_pages: match &state.startup {
                Startup::Folders(pages) => pages.clone(),
                _ => Vec::new(),
            },
            startup: state.startup,
            about: About::default(),
            log_level: settings::LOG_DEFAULT,
            default_manager: false,
            autostart: false,
            protocol_links: false,
            quick_access_sync: false,
            tray_available: false,
            updates: update::UpdatePrefs::default(),
            language: 0,
            prefs: state.prefs,
            clicked_open: false,
            menu: Vec::new(),
            focused_at: None,
            look: appearance::Look::new(dark),
            dark,
            scale,
            system_scale: scale,
            system_text: 1.0,
            size: (size.0 as f32, size.1 as f32),
            maximized: false,
            server_frames: None,
            system_title: false,
            active: true,
            quick_tags: false,
            next_gen: 0,
            timer_at: None,
            error_shown: None,
            own_menu: None,
            menu_owner: None,
            menu_panel: 0,
            menu_right: None,
            tip: None,
            quick: None,
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
            file_icons: file_icons::FileIcons::default(),
            shell_overlays: shell_overlays::ShellOverlays::default(),
            type_names: FxHashMap::default(),
            adaptive: FxHashMap::default(),
            layout: Layout::default(),
            effects: Vec::new(),
            arena: Bump::new(),
            last_frame: None,
            moving: false,
            dirty: true,
            group_press: None,
            nav: nav::Nav::default(),
            marquee: None,
            details: Default::default(),
            touch: Default::default(),
            archive: archive::ArchiveUi::new(state.archive),
            addr: Default::default(),
            primary: String::new(),
            paste_after_layout: None,
            ghosts: Default::default(),
            bar_arrivals: Default::default(),
            rolls: Default::default(),
            row_glow: Default::default(),
            side_glow: Default::default(),
            head_hot: None,
            hover_glow: Default::default(),
            head_glow: Default::default(),
            prefetch: Default::default(),
            shapes: Default::default(),
            tag_fixes: Vec::new(),
            tabs: tabbar::Tabs::new(tab_base),
            ops: Default::default(),
            search: Default::default(),
            dialogs: Default::default(),
            preview: Default::default(),
            settings: settings::SettingsState::new(state.folders),
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

    pub fn report_dropped(&mut self, total: usize) {
        if total > MAX_ROUTES {
            self.tab_mut().error = Some(crate::trn!(P::UiDroppedRoutes, total, shown = MAX_ROUTES));
        }
    }

    pub fn shutdown(&mut self) {
        for t in self.tabs.items.iter_mut().chain(self.tabs.panels.list.iter_mut().flat_map(|p| p.tabs.iter_mut())) {
            t.shutdown(&mut self.effects);
        }
        self.cancel_ops();
    }

    pub fn persisted(&self) -> Persisted {
        let t = self.tab();
        let path = if self.tabs.closing_all { Location::home_page() } else { t.location().clone() };
        Persisted { path, show_hidden: t.view_spec.show_hidden, view: t.folder_view(), prefs: self.prefs.clone(), folders: self.settings.folders.clone(), archive: self.archive.prefs.clone(), startup: self.startup.clone() }
    }

    pub fn tab(&self) -> &Tab {
        &self.tabs.items[self.tabs.cur]
    }

    fn tab_mut(&mut self) -> &mut Tab {
        &mut self.tabs.items[self.tabs.cur]
    }

    pub fn tabs(&self) -> &[Tab] {
        &self.tabs.items
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
        self.context_menu_open().map(|c| c.ime_area()).or_else(|| self.search_ime_area()).or_else(|| self.settings_ime_area()).or_else(|| self.nav_ime_area()).or_else(|| self.rename_ime_area())
    }

    pub fn ui_scale(&self) -> u32 {
        self.settings.ui_scale
    }

    pub fn set_ui_scale(&mut self, percent: u32) {
        self.settings.ui_scale = percent.clamp(UI_SCALE.0, UI_SCALE.1);
        self.rescale();
    }

    pub fn content_scale(&self) -> u32 {
        self.settings.content_scale
    }

    pub fn set_content_scale(&mut self, percent: u32) {
        self.settings.content_scale = percent.clamp(UI_SCALE.0, UI_SCALE.1);
        self.layout();
        self.dirty = true;
    }

    fn content_px(&self) -> f32 {
        self.system_scale * self.system_text * self.settings.content_scale as f32 / 100.0
    }

    fn cs(&self, v: f32) -> f32 {
        (v * self.content_px()).round()
    }

    fn group_chevron_click(&self, x: f32, y: f32) -> Option<usize> {
        let f = self.flow();
        let n = self.line_at(&f, x, y)?;
        let g = match self.tab().view.as_ref()?.line(n)? {
            crate::view::Line::Header(g) => g,
            _ => return None,
        };
        let r = self.line_rect(&f, n);
        (x <= r.x + self.cs(26.0)).then_some(g)
    }

    fn select_group(&mut self, g: usize) {
        let adding = self.mods.ctrl;
        let Some(v) = self.tab_mut().view.as_mut() else { return };
        let Some(gr) = v.groups().get(g).cloned() else { return };
        let (a, b) = (gr.start as usize, (gr.start + gr.count).saturating_sub(1) as usize);
        let (Some(&first), Some(&last)) = (v.rows().get(a), v.rows().get(b)) else { return };
        if !adding {
            v.clear_selection();
        }
        v.select_rows(a, b);
        v.anchor = Some(first);
        v.focus = Some(last);
        self.dirty = true;
    }

    fn content_font(&self, px: f32) -> FontStyle {
        FontStyle::new(FontFamily::Ui, px * self.content_px(), false)
    }

    fn content_icons(&self, px: f32) -> FontStyle {
        FontStyle::new(FontFamily::Icons, px * self.content_px(), false)
    }

    fn rescale(&mut self) {
        self.scale = self.system_scale * self.system_text * self.settings.ui_scale as f32 / 100.0;
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

    pub fn on_paste(&mut self, text: &str) {
        if self.caps.unix_paths {
            if let Some(menu) = self.context_menu_mut() {
                menu.insert(text.lines().next().unwrap_or(""));
                self.dirty = true;
                return;
            }
        }
        if self.form_insert(text) || self.dialogs.modal() || self.settings_insert(text.lines().next().unwrap_or("")) {
            return;
        }
        if self.search.active {
            return self.search_paste(text);
        }
        if self.ops_paste(text) || self.inspector_insert(text.lines().next().unwrap_or("")) {
            return;
        }
        self.nav_insert(text.lines().next().unwrap_or(""));
    }

    pub fn set_tray_available(&mut self, available: bool) {
        if self.tray_available != available {
            self.tray_available = available;
            self.layout();
            self.dirty = true;
        }
    }
}

impl App {
    fn request_icons(&mut self, requests: Vec<IconRequest>) {
        self.request_file_icons(requests);
    }

    fn system_icon<B: TextBackend>(&self, g: &mut Gfx<B>, icon: Option<sidebar::SystemIcon>, r: Rect, px: f32, alpha: u8, requests: &mut Vec<IconRequest>) -> bool {
        let Some((key, kind, path)) = icon else { return false };
        let key = if self.icon_epoch == 0 { key } else { crate::hash::hash(&(key, self.icon_epoch)) };
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
    if let Some(k) = l.tab_arrows.iter().position(|r| r.contains(x, y)) {
        return Hit::TabScroll(k == 1);
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
    let glyph = if e.attrs & attr::VOLUME != 0 {
        G_DRIVE
    } else if e.is_dir() {
        G_FOLDER
    } else {
        G_FILE
    };
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
        return (hash::hash(&("vol", name, size, epoch)), IconKind::Volume);
    }
    if e.attrs & attr::LINK != 0 && !cfg!(windows) {
        return (hash::hash(&("link-base", dir, name, e.attrs & attr::DIR, e.modified, size, epoch)), IconKind::File);
    }
    let shell_link = e.attrs & attr::LINK != 0 && dir.first() != Some(&b'/');
    if e.is_dir() && (e.attrs & (attr::READONLY | attr::SYSTEM) != 0 || shell_link || crate::location::wsl_drive_path(dir).is_some()) {
        return (hash::hash(&("custom", dir, name, e.modified, e.attrs, size, epoch)), IconKind::CustomFolder);
    }
    if cfg!(target_os = "linux") && e.is_dir() && dir.first() == Some(&b'/') && e.attrs & (attr::STREAM | attr::ARCHIVED | attr::NOT_LOCAL | attr::DELETED) == 0 {
        return (hash::hash(&("folder", dir, name, e.modified, size, epoch)), IconKind::CustomFolder);
    }
    if e.is_dir() {
        return file_icons::folder_key(size, epoch);
    }
    let folded = ext.to_ascii_lowercase();
    if PER_FILE_ICON_EXT.contains(&folded.as_slice()) || shell_link {
        return (hash::hash(&("file", dir, name, size, epoch)), IconKind::File);
    }
    if folded.is_empty() && e.attrs & attr::EXEC != 0 {
        return (hash::hash(&("exec", size, epoch)), IconKind::Type);
    }
    if cfg!(target_os = "linux") && dir.first() == Some(&b'/') && folded.is_empty() && e.size > 0 && e.attrs & (attr::STREAM | attr::ARCHIVED | attr::NOT_LOCAL | attr::DELETED | attr::UNREADABLE) == 0 {
        return (hash::hash(&("content", dir, name, e.modified, e.size, size, epoch)), IconKind::Content);
    }
    (hash::hash(&("ext", folded.as_slice(), size, epoch)), IconKind::Type)
}

#[cfg(test)]
#[path = "../../test/ui.rs"]
mod tests;

#[cfg(test)]
#[path = "../../test/ui/budgets.rs"]
mod budgets;
