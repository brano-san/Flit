use super::home::{Recent, Widget};
use super::menu::Menu;
use super::tags::G_TAG;
use super::{centered, App, Drag, Effect, Focus, Hit, IconKind, IconRequest, Route, ShellMenu, G_DRIVE, G_FOLDER};
use crate::hash;
use crate::i18n::S;
use crate::input::{Key, Mods};
use crate::library::{self, Library, LibraryOp};
use crate::location::{Location, ProviderId};
use crate::open::OpenTarget;
use crate::path::{FsPath, PathKind};
use crate::provider::{Launch, ProviderRequest};
use crate::render::{Color, Rect};
use crate::tags::Tags;
use crate::text::{Gfx, TextBackend};
use crate::theme::Theme;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Instant;

pub const SIDEBAR_MIN_W: f32 = 160.0;
pub const SIDEBAR_MAX_W: f32 = 480.0;
const COMPACT_W: f32 = 48.0;
const ROW_H: f32 = 28.0;
const COMPACT_ROW_H: f32 = 32.0;
const DRIVE_H: f32 = 38.0;
const HEADER_H: f32 = 28.0;
const COMPACT_HEADER_H: f32 = 12.0;
const PIN_W: f32 = 22.0;
const MAX_LIST: usize = 256;

pub(super) const G_HOME: &str = "\u{E80F}";
const G_DESKTOP: &str = "\u{E7F4}";
const G_DOWNLOADS: &str = "\u{E896}";
const G_DOCUMENTS: &str = "\u{E8A5}";
const G_PICTURES: &str = "\u{E8B9}";
const G_MUSIC: &str = "\u{E8D6}";
const G_VIDEOS: &str = "\u{E714}";
const G_LINUX: &str = "\u{E756}";
const G_RECYCLE_BIN: &str = "\u{E74D}";
const G_LIBRARY: &str = "\u{E8F1}";
const G_PIN: &str = "\u{E718}";
const G_UNPIN: &str = "\u{E77A}";
const G_EXPANDED: &str = "\u{E70D}";
const G_COLLAPSED: &str = "\u{E76C}";
pub(super) const G_SETTINGS: &str = "\u{E713}";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum KnownFolder {
    Home,
    Desktop,
    Downloads,
    Documents,
    Pictures,
    Music,
    Videos,
}

impl KnownFolder {
    pub const ALL: [KnownFolder; 7] = [Self::Home, Self::Desktop, Self::Downloads, Self::Documents, Self::Pictures, Self::Music, Self::Videos];

    pub fn label(self) -> &'static str {
        match self {
            Self::Home => S::SidebarFolderHome.t(),
            Self::Desktop => S::SidebarFolderDesktop.t(),
            Self::Downloads => S::SidebarFolderDownloads.t(),
            Self::Documents => S::SidebarFolderDocuments.t(),
            Self::Pictures => S::SidebarFolderPictures.t(),
            Self::Music => S::SidebarFolderMusic.t(),
            Self::Videos => S::SidebarFolderVideos.t(),
        }
    }

    pub fn shell_name(self) -> &'static str {
        match self {
            Self::Home => "shell:Profile",
            Self::Desktop => "shell:Desktop",
            Self::Downloads => "shell:Downloads",
            Self::Documents => "shell:Personal",
            Self::Pictures => "shell:My Pictures",
            Self::Music => "shell:My Music",
            Self::Videos => "shell:My Video",
        }
    }

    fn glyph(self) -> &'static str {
        match self {
            Self::Home => G_HOME,
            Self::Desktop => G_DESKTOP,
            Self::Downloads => G_DOWNLOADS,
            Self::Documents => G_DOCUMENTS,
            Self::Pictures => G_PICTURES,
            Self::Music => G_MUSIC,
            Self::Videos => G_VIDEOS,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Drive {
    pub path: FsPath,
    pub label: String,
    pub total: u64,
    pub free: u64,
    pub removable: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Places {
    pub folders: Vec<(KnownFolder, Location)>,
    pub drives: Vec<Drive>,
    pub wsl: Vec<String>,
    pub recycle_bin: bool,
    pub templates: Vec<super::NewTemplate>,
    pub libraries: Vec<Library>,
    pub recent: Vec<Recent>,
    pub recent_off: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NavState {
    pub pinned: Vec<Location>,
    pub sidebar_w: f32,
    pub sidebar: bool,
}

impl Default for NavState {
    fn default() -> Self {
        Self { pinned: Vec::new(), sidebar_w: 220.0, sidebar: true }
    }
}

impl NavState {
    pub fn sanitized(mut self) -> Self {
        if !(self.sidebar_w.is_finite() && (SIDEBAR_MIN_W..=SIDEBAR_MAX_W).contains(&self.sidebar_w)) {
            self.sidebar_w = Self::default().sidebar_w;
        }
        let mut seen = Vec::with_capacity(self.pinned.len());
        self.pinned.retain(|l| {
            let fresh = !seen.contains(l);
            seen.push(l.clone());
            fresh
        });
        self
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct NavPrefs {
    pub compact: bool,
    pub collapsed: Vec<String>,
    pub hidden: Vec<String>,
    pub widgets_off: Vec<String>,
    pub recent_hidden: Vec<(FsPath, i64)>,
    pub recent_cleared: i64,
}

impl NavPrefs {
    pub fn sanitized(mut self) -> Self {
        for v in [&mut self.collapsed, &mut self.hidden, &mut self.widgets_off] {
            v.truncate(MAX_LIST);
        }
        let n = self.recent_hidden.len();
        self.recent_hidden.drain(..n.saturating_sub(MAX_LIST));
        self
    }
}

fn toggle(list: &mut Vec<String>, id: &str) {
    match list.iter().position(|s| s == id) {
        Some(i) => {
            list.remove(i);
        }
        None => list.push(id.to_owned()),
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Section {
    Pinned,
    Folders,
    Libraries,
    Drives,
    Wsl,
    Tags,
}

impl Section {
    pub const ALL: [Section; 6] = [Section::Pinned, Section::Folders, Section::Libraries, Section::Drives, Section::Wsl, Section::Tags];

    pub fn title(self) -> &'static str {
        match self {
            Section::Pinned => S::SidebarSectionPinned.t(),
            Section::Folders => S::SidebarSectionFolders.t(),
            Section::Libraries => S::SidebarSectionLibraries.t(),
            Section::Drives => S::SidebarSectionDrives.t(),
            Section::Wsl => "WSL",
            Section::Tags => S::SidebarSectionTags.t(),
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Section::Pinned => "pinned",
            Section::Folders => "folders",
            Section::Libraries => "libraries",
            Section::Drives => "drives",
            Section::Wsl => "wsl",
            Section::Tags => "tags",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum PlaceCmd {
    Open(Location, OpenTarget),
    OpenFile(FsPath),
    Reveal(FsPath),
    CopyPath(Location),
    Terminal(FsPath),
    Pin(Location),
    MovePin(Location, isize),
    Eject(FsPath),
    Properties(FsPath),
    Collapse(Section),
    Hide(Section),
    Compact,
    NewLibrary,
    RestoreLibraries,
    Library(FsPath),
    EmptyBin,
    RestoreAll,
    Widget(Widget),
    Forget(FsPath),
    ClearRecent,
    Pane(Location, Option<super::SplitDir>),
}

#[derive(Clone, Debug)]
pub(super) struct Item {
    section: Section,
    pub label: String,
    glyph: &'static str,
    pub loc: Location,
    pub usage: Option<f32>,
}

#[derive(Default)]
pub(super) struct Sidebar {
    pub state: NavState,
    pub prefs: NavPrefs,
    pub places: Arc<Places>,
    pub items: Vec<Item>,
    pub rects: Vec<Rect>,
    pub headers: Vec<(Rect, Section)>,
    pub hint: Option<Rect>,
    pub rect: Rect,
    pub splitter: Rect,
    pub settings: Rect,
    scroll: f32,
    content_h: f32,
    pub cursor: usize,
    pub press_y: f32,
    pub tags: Arc<Tags>,
}

fn pinned_label(loc: &Location) -> String {
    loc.crumbs().pop().map(|(label, _)| label).unwrap_or_default()
}

fn pinned_glyph(loc: &Location) -> &'static str {
    match loc {
        Location::Wsl { path, .. } if path.is_root() => G_LINUX,
        _ if library::is_library(loc.path()) => G_LIBRARY,
        _ if loc.is_home_page() => G_HOME,
        _ if loc.path().kind() == PathKind::Computer || (loc.is_root() && loc.path().kind() == PathKind::Drive) => G_DRIVE,
        _ => G_FOLDER,
    }
}

impl Sidebar {
    pub(super) fn rebuild(&mut self) {
        let p = self.places.clone();
        let library_name = |l: &Location| p.libraries.iter().find(|lib| lib.path == *l.path()).map(|lib| lib.name.clone());
        let mut items: Vec<Item> = self
            .state
            .pinned
            .iter()
            .map(|l| Item { section: Section::Pinned, label: library_name(l).unwrap_or_else(|| pinned_label(l)), glyph: pinned_glyph(l), loc: l.clone(), usage: None })
            .collect();
        items.extend(p.folders.iter().map(|(k, l)| Item { section: Section::Folders, label: k.label().to_owned(), glyph: k.glyph(), loc: l.clone(), usage: None }));
        if p.recycle_bin {
            let label = S::PathRecycleBin.t().to_owned();
            items.push(Item { section: Section::Folders, label, glyph: G_RECYCLE_BIN, loc: Location::recycle_bin(), usage: None });
        }
        items.extend(p.libraries.iter().map(|l| Item { section: Section::Libraries, label: l.name.clone(), glyph: G_LIBRARY, loc: Location::Local(l.path.clone()), usage: None }));
        items.extend(p.drives.iter().map(|d| Item {
            section: Section::Drives,
            label: crate::i18n::relocalize(&d.label, crate::i18n::Lang::En).into_owned(),
            glyph: G_DRIVE,
            loc: Location::Local(d.path.clone()),
            usage: (d.total > 0).then(|| (d.total.saturating_sub(d.free)) as f32 / d.total as f32),
        }));
        items.extend(p.wsl.iter().filter_map(|d| {
            let loc = Location::parse(&format!("wsl://{d}/"))?;
            Some(Item { section: Section::Wsl, label: d.clone(), glyph: G_LINUX, loc, usage: None })
        }));
        items.extend(self.tags.defs.iter().map(|t| Item { section: Section::Tags, label: t.name.clone(), glyph: G_TAG, loc: Location::computer(), usage: None }));
        items.retain(|it| !self.is_collapsed(it.section) && !self.is_hidden(it.section));
        self.items = items;
        self.cursor = self.cursor.min(self.items.len().saturating_sub(1));
    }

    pub fn tag_of(&self, i: usize) -> Option<usize> {
        let first = self.items.iter().position(|it| it.section == Section::Tags)?;
        (self.items.get(i)?.section == Section::Tags).then(|| i - first)
    }

    pub fn is_pinned(&self, loc: &Location) -> bool {
        self.state.pinned.contains(loc)
    }

    pub fn is_collapsed(&self, s: Section) -> bool {
        self.prefs.collapsed.iter().any(|c| c == s.id())
    }

    pub fn is_hidden(&self, s: Section) -> bool {
        self.prefs.hidden.iter().any(|c| c == s.id())
    }

    fn has_items(&self, s: Section) -> bool {
        let p = &self.places;
        match s {
            Section::Pinned => true,
            Section::Folders => !p.folders.is_empty() || p.recycle_bin,
            Section::Libraries => !p.libraries.is_empty(),
            Section::Drives => !p.drives.is_empty(),
            Section::Wsl => !p.wsl.is_empty(),
            Section::Tags => !self.tags.defs.is_empty(),
        }
    }

    pub fn pin_rect(&self, r: Rect, scale: f32) -> Rect {
        let w = (PIN_W * scale).round();
        Rect::new(r.right() - w - (4.0 * scale).round(), r.y + ((r.h - w) / 2.0).round(), w, w)
    }

    pub fn library(&self, path: &FsPath) -> Option<&Library> {
        self.places.libraries.iter().find(|l| l.path == *path)
    }
}

impl App {
    pub fn set_places(&mut self, places: Arc<Places>) {
        let old = std::mem::replace(&mut self.nav.sidebar.places, places);
        self.nav.sidebar.rebuild();
        self.places_changed(&old, Instant::now());
        self.layout();
        self.dirty = true;
    }

    pub fn set_tags(&mut self, tags: Arc<Tags>) {
        self.nav.sidebar.tags = tags;
        self.nav.sidebar.rebuild();
        self.layout();
        self.dirty = true;
    }

    pub fn set_pinned(&mut self, pinned: Vec<Location>) {
        if self.nav.sidebar.state.pinned != pinned {
            self.nav.sidebar.state.pinned = pinned;
            self.nav.sidebar.rebuild();
            self.layout();
            self.dirty = true;
        }
    }

    pub fn nav_state(&self) -> NavState {
        self.nav.sidebar.state.clone()
    }

    pub fn set_nav_state(&mut self, state: NavState) {
        self.nav.sidebar.state = state.sanitized();
        self.nav.sidebar.rebuild();
        self.layout();
        self.dirty = true;
    }

    pub fn nav_prefs(&self) -> NavPrefs {
        self.nav.sidebar.prefs.clone()
    }

    pub fn set_nav_prefs(&mut self, prefs: NavPrefs) {
        self.nav.sidebar.prefs = prefs.sanitized();
        self.nav_prefs_changed();
    }

    fn nav_prefs_changed(&mut self) {
        self.nav.sidebar.rebuild();
        self.layout();
        self.dirty = true;
    }

    pub(super) fn sidebar_width(&self) -> f32 {
        let st = &self.nav.sidebar.state;
        match (st.sidebar, self.nav.sidebar.prefs.compact) {
            (false, _) => 0.0,
            (true, true) => self.s(COMPACT_W),
            (true, false) => self.s(st.sidebar_w).min((self.size.0 / 2.0).round()),
        }
    }

    pub(super) fn toggle_sidebar(&mut self) {
        let st = &mut self.nav.sidebar.state;
        st.sidebar = !st.sidebar;
        if !st.sidebar && self.focus == Focus::Sidebar {
            self.focus = Focus::List;
        }
        self.layout();
        self.dirty = true;
    }

    pub(super) fn toggle_compact(&mut self) {
        let p = &mut self.nav.sidebar.prefs;
        p.compact = !p.compact;
        self.nav_prefs_changed();
    }

    pub(super) fn toggle_section(&mut self, s: Section) {
        toggle(&mut self.nav.sidebar.prefs.collapsed, s.id());
        self.nav_prefs_changed();
    }

    pub(super) fn toggle_section_shown(&mut self, s: Section) {
        toggle(&mut self.nav.sidebar.prefs.hidden, s.id());
        self.nav_prefs_changed();
    }

    pub(super) fn show_all_sections(&mut self) {
        let p = &mut self.nav.sidebar.prefs;
        p.hidden.clear();
        p.collapsed.clear();
        self.nav_prefs_changed();
    }

    pub(super) fn cycle_sidebar_focus(&mut self) {
        if self.focus == Focus::Sidebar {
            self.focus = Focus::List;
        } else {
            if !self.nav.sidebar.state.sidebar {
                self.toggle_sidebar();
            }
            self.editing = false;
            self.focus = Focus::Sidebar;
            let here = self.current_location().clone();
            let sb = &mut self.nav.sidebar;
            if let Some(i) = sb.items.iter().position(|it| it.loc == here && it.section != Section::Tags) {
                sb.cursor = i;
            }
            self.reveal_side_cursor();
        }
        self.dirty = true;
    }

    pub(super) fn toggle_pin(&mut self, loc: Location) {
        let pinned = &mut self.nav.sidebar.state.pinned;
        match pinned.iter().position(|l| *l == loc) {
            Some(i) => {
                pinned.remove(i);
            }
            None => pinned.push(loc),
        }
        self.pins_changed();
    }

    pub fn pin_locations(&mut self, locs: Vec<Location>) {
        let pinned = &mut self.nav.sidebar.state.pinned;
        let before = pinned.len();
        for l in locs {
            if !pinned.contains(&l) {
                pinned.push(l);
            }
        }
        if pinned.len() == before {
            return self.status(S::SidebarOnlyUnpinned.t());
        }
        self.pins_changed();
    }

    fn move_pin(&mut self, loc: Location, by: isize) {
        let pinned = &mut self.nav.sidebar.state.pinned;
        let Some(i) = pinned.iter().position(|l| *l == loc) else { return };
        let to = (i as isize + by).clamp(0, pinned.len() as isize - 1) as usize;
        if to != i {
            let l = pinned.remove(i);
            pinned.insert(to, l);
            self.pins_changed();
        }
    }

    fn pins_changed(&mut self) {
        let list = self.nav.sidebar.state.pinned.clone();
        self.nav.sidebar.rebuild();
        self.effects.push(Effect::Pinned(list));
        self.layout();
        self.dirty = true;
    }

    pub(super) fn pin_selection(&mut self) {
        let folders = self.selected_folders();
        if folders.is_empty() {
            return self.status(S::SidebarSelectToPin.t());
        }
        let pinned = &mut self.nav.sidebar.state.pinned;
        if folders.iter().all(|l| pinned.contains(l)) {
            pinned.retain(|l| !folders.contains(l));
        } else {
            for l in folders {
                if !pinned.contains(&l) {
                    pinned.push(l);
                }
            }
        }
        self.pins_changed();
    }

    pub(super) fn pin_current(&mut self) {
        let here = self.current_location().clone();
        self.toggle_pin(here);
    }

    pub(super) fn toggle_pin_item(&mut self, i: usize) {
        if let Some(loc) = self.nav.sidebar.items.get(i).map(|it| it.loc.clone()) {
            self.toggle_pin(loc);
        }
    }

    pub(super) fn start_pin_drag(&mut self, y: f32) -> bool {
        let Hit::Side(i) = self.pressed else { return false };
        let sb = &self.nav.sidebar;
        if sb.items.get(i).is_none_or(|it| it.section != Section::Pinned) || (y - sb.press_y).abs() < self.s(5.0) {
            return false;
        }
        self.drag_pin(i, y);
        true
    }

    pub(super) fn drag_pin(&mut self, from: usize, y: f32) {
        let sb = &self.nav.sidebar;
        let to = sb.rects.iter().take(sb.state.pinned.len()).filter(|r| r.y + r.h / 2.0 < y).count();
        self.drag = Drag::PinMove { from, to };
        self.dirty = true;
    }

    pub(super) fn drop_pin(&mut self, from: usize, to: usize) {
        let pinned = &mut self.nav.sidebar.state.pinned;
        let target = if to > from { to - 1 } else { to };
        if from >= pinned.len() || target == from {
            return;
        }
        let loc = pinned.remove(from);
        pinned.insert(target.min(pinned.len()), loc);
        self.pins_changed();
    }

    pub(super) fn open_side_item(&mut self, i: usize, new_tab: bool, now: Instant) {
        if let Some(k) = self.nav.sidebar.tag_of(i) {
            return self.show_tag(k);
        }
        let Some(folder) = self.nav.sidebar.items.get(i).map(|it| it.loc.clone()) else { return };
        match new_tab {
            true => self.open_in_tabs(vec![Route { folder, select: None }], now),
            false => self.dispatch(vec![Route { folder, select: None }], OpenTarget::CurrentTab, now),
        }
    }

    pub(super) fn known_folder(&self, k: KnownFolder) -> Option<Location> {
        self.nav.sidebar.places.folders.iter().find(|(f, _)| *f == k).map(|(_, l)| l.clone())
    }

    pub(super) fn place_command(&mut self, c: PlaceCmd, now: Instant) {
        let tab = self.tab().id;
        let launch = |launch| Effect::Provider { provider: ProviderId::Local, req: ProviderRequest::Launch { tab, launch } };
        match c {
            PlaceCmd::Open(folder, target) => self.dispatch(vec![Route { folder, select: None }], target, now),
            PlaceCmd::OpenFile(path) => self.effects.push(Effect::Provider { provider: ProviderId::Local, req: ProviderRequest::Open { tab, path } }),
            PlaceCmd::Reveal(path) => {
                let item = Location::from_local(path);
                if let Some(folder) = item.parent() {
                    self.dispatch(vec![Route { select: Some(item.file_name().to_vec()), folder }], OpenTarget::CurrentTab, now);
                }
            }
            PlaceCmd::CopyPath(loc) => self.effects.push(Effect::SetClipboard(loc.display().into_owned())),
            PlaceCmd::Terminal(dir) => self.effects.push(launch(Launch::Terminal { dirs: vec![dir], admin: false })),
            PlaceCmd::Pin(loc) => self.toggle_pin(loc),
            PlaceCmd::MovePin(loc, by) => self.move_pin(loc, by),
            PlaceCmd::Eject(drive) => self.effects.push(launch(Launch::Eject(drive))),
            PlaceCmd::Properties(path) => self.effects.push(Effect::Properties(vec![path])),
            PlaceCmd::Collapse(s) => self.toggle_section(s),
            PlaceCmd::Hide(s) => self.toggle_section_shown(s),
            PlaceCmd::Compact => self.toggle_compact(),
            PlaceCmd::NewLibrary => self.ask_new_library(),
            PlaceCmd::RestoreLibraries => self.effects.push(Effect::Library(LibraryOp::RestoreDefaults)),
            PlaceCmd::Library(path) => self.library_dialog(path),
            PlaceCmd::EmptyBin => self.empty_bin(),
            PlaceCmd::RestoreAll => self.restore_all(),
            PlaceCmd::Widget(w) => self.toggle_widget(w),
            PlaceCmd::Forget(path) => self.forget_recent(path),
            PlaceCmd::ClearRecent => self.clear_recent(),
            PlaceCmd::Pane(loc, dir) => self.open_in_pane(loc, dir, now),
        }
    }

    pub(super) fn place_menu(&mut self, x: f32, y: f32, loc: Location, extra: impl FnOnce(&Self, &mut Menu)) {
        let path = loc.to_windows();
        let lib = library::is_library(loc.path());
        let menu = self.collect(|a, m| {
            m.place(S::MenuOpen.t(), PlaceCmd::Open(loc.clone(), OpenTarget::CurrentTab), false);
            m.place(S::MenuOpenInNewTab.t(), PlaceCmd::Open(loc.clone(), OpenTarget::NewTab), false);
            m.place(S::MenuOpenInNewWindow.t(), PlaceCmd::Open(loc.clone(), OpenTarget::NewWindow), false);
            a.place_pane_items(m, &loc);
            m.separator();
            if let Some(p) = &path {
                m.place(S::MenuCopyPath.t(), PlaceCmd::CopyPath(loc.clone()), false);
                if !lib {
                    m.place(S::MenuOpenInTerminal.t(), PlaceCmd::Terminal(p.clone()), false);
                }
            }
            m.separator();
            let pinned = &a.nav.sidebar.state.pinned;
            if !loc.is_recycle_bin() {
                m.place(if pinned.contains(&loc) { S::MenuUnpinFromSidebar.t() } else { S::MenuPinToSidebar.t() }, PlaceCmd::Pin(loc.clone()), false);
            }
            if let Some(i) = pinned.iter().position(|l| *l == loc) {
                if i > 0 {
                    m.place(S::SidebarMoveUp.t(), PlaceCmd::MovePin(loc.clone(), -1), false);
                }
                if i + 1 < pinned.len() {
                    m.place(S::SidebarMoveDown.t(), PlaceCmd::MovePin(loc.clone(), 1), false);
                }
            }
            if a.nav.sidebar.places.drives.iter().any(|d| d.removable && d.path == *loc.path()) {
                m.place(S::MenuEject.t(), PlaceCmd::Eject(loc.path().clone()), false);
            }
            if lib {
                m.place(S::SidebarLibraryLocations.t(), PlaceCmd::Library(loc.path().clone()), false);
            }
            if loc.is_recycle_bin() {
                m.place(S::SidebarRestoreAll.t(), PlaceCmd::RestoreAll, false);
                m.place(S::UiBinEmpty.t(), PlaceCmd::EmptyBin, false);
            }
            extra(a, m);
            m.separator();
            if let Some(p) = &path {
                m.place(S::MenuProperties.t(), PlaceCmd::Properties(p.clone()), false);
            }
        });
        self.show_place_menu(x, y, path, menu);
    }

    pub(super) fn show_place_menu(&mut self, x: f32, y: f32, path: Option<FsPath>, menu: Vec<super::MenuItem>) {
        let Some(p) = path else { return self.flit_menu(x, y, menu) };
        let shift = self.mods.shift;
        let dir = p.parent().unwrap_or_else(FsPath::computer);
        self.effects.push(Effect::ShellMenu(ShellMenu { dir, items: vec![p], x, y, extended: shift, menu, inline: self.shell_items_inline || shift, shell: true }));
    }

    fn section_menu(&mut self, x: f32, y: f32, section: Option<Section>) {
        let menu = self.collect(|a, m| {
            let sb = &a.nav.sidebar;
            if let Some(s) = section {
                m.place(if sb.is_collapsed(s) { S::SidebarExpand.t() } else { S::SidebarCollapse.t() }, PlaceCmd::Collapse(s), false);
                m.place(S::SidebarHideSection.t(), PlaceCmd::Hide(s), false);
                m.separator();
            }
            if section == Some(Section::Tags) {
                m.action("tags.new", S::MenuNewTag.t());
                m.action("tags.manage", S::MenuManageTags.t());
                m.separator();
            }
            if section.is_none_or(|s| s == Section::Libraries) {
                m.place(S::SidebarNewLibrary.t(), PlaceCmd::NewLibrary, false);
                m.place(S::SidebarRestoreLibraries.t(), PlaceCmd::RestoreLibraries, false);
                m.separator();
            }
            m.submenu(S::SidebarSections.t(), |m| {
                for s in Section::ALL {
                    m.place(s.title(), PlaceCmd::Hide(s), !sb.is_hidden(s));
                }
            });
            m.place(S::SidebarCompact.t(), PlaceCmd::Compact, sb.prefs.compact);
        });
        self.flit_menu(x, y, menu);
    }

    pub(super) fn side_menu(&mut self, x: f32, y: f32, h: Hit) {
        match h {
            Hit::Side(i) | Hit::SidePin(i) => {
                if let Some(k) = self.nav.sidebar.tag_of(i) {
                    let menu = self.build_tag_menu(Some(k));
                    return self.flit_menu(x, y, menu);
                }
                if let Some(loc) = self.nav.sidebar.items.get(i).map(|it| it.loc.clone()) {
                    self.place_menu(x, y, loc, |_, _| {});
                }
            }
            Hit::SideHeader(s) => self.section_menu(x, y, Some(s)),
            _ => self.section_menu(x, y, None),
        }
    }

    pub(super) fn layout_sidebar(&mut self, top: f32, bottom: f32) {
        let width = self.sidebar_width();
        let sc = self.scale;
        let s = |v: f32| (v * sc).round();
        let sb = &mut self.nav.sidebar;
        let compact = sb.prefs.compact;
        sb.rect = Rect::new(0.0, top, width, (bottom - top).max(0.0));
        sb.splitter = if width > 0.0 && !compact { Rect::new(width - s(3.0), top, s(6.0), sb.rect.h) } else { Rect::default() };
        sb.rects.clear();
        sb.headers.clear();
        sb.hint = None;
        sb.settings = Rect::default();
        if width <= 0.0 {
            return;
        }
        let (x, w) = if compact { (s(4.0), (width - s(8.0)).max(0.0)) } else { (s(6.0), (width - s(12.0)).max(0.0)) };
        let (header_h, row_h) = if compact { (s(COMPACT_HEADER_H), s(COMPACT_ROW_H)) } else { (s(HEADER_H), s(ROW_H)) };
        let footer = row_h + s(8.0);
        sb.settings = Rect::new(x, sb.rect.bottom() - row_h - s(4.0), w, row_h);
        let mut y = s(4.0);
        for section in Section::ALL {
            if sb.is_hidden(section) || !sb.has_items(section) {
                continue;
            }
            sb.headers.push((Rect::new(x, y, w, header_h), section));
            y += header_h;
            let count = sb.items.iter().filter(|it| it.section == section).count();
            if count == 0 && section == Section::Pinned && !compact && !sb.is_collapsed(section) {
                sb.hint = Some(Rect::new(x, y, w, row_h));
                y += row_h;
            }
            for it in sb.items.iter().filter(|it| it.section == section) {
                let h = if it.usage.is_some() && !compact { s(DRIVE_H) } else { row_h };
                sb.rects.push(Rect::new(x, y, w, h));
                y += h;
            }
            y += if compact { s(2.0) } else { s(6.0) };
        }
        sb.content_h = y + footer;
        sb.scroll = sb.scroll.clamp(0.0, (sb.content_h - sb.rect.h).max(0.0));
        let dy = top - sb.scroll;
        for r in sb.rects.iter_mut().chain(sb.headers.iter_mut().map(|(r, _)| r)).chain(sb.hint.iter_mut()) {
            r.y += dy;
        }
    }

    pub(super) fn sidebar_hit(&self, x: f32, y: f32) -> Option<Hit> {
        let sb = &self.nav.sidebar;
        if sb.rect.w <= 0.0 {
            return None;
        }
        if sb.splitter.contains(x, y) {
            return Some(Hit::Splitter);
        }
        if !sb.rect.contains(x, y) {
            return None;
        }
        if sb.settings.contains(x, y) {
            return Some(Hit::SideSettings);
        }
        for (i, r) in sb.rects.iter().enumerate() {
            if r.contains(x, y) {
                let pin = !sb.prefs.compact && sb.pin_rect(*r, self.scale).contains(x, y) && sb.tag_of(i).is_none();
                return Some(if pin { Hit::SidePin(i) } else { Hit::Side(i) });
            }
        }
        if let Some(&(_, s)) = sb.headers.iter().find(|(r, _)| r.contains(x, y)) {
            return Some(Hit::SideHeader(s));
        }
        Some(Hit::SideEmpty)
    }

    pub(super) fn sidebar_wheel(&mut self, dy: f32) {
        let sb = &mut self.nav.sidebar;
        let step = 3.0 * (ROW_H * self.scale).round();
        sb.scroll = (sb.scroll - dy * step).clamp(0.0, (sb.content_h - sb.rect.h).max(0.0));
        self.layout();
        self.dirty = true;
    }

    pub(super) fn begin_splitter_drag(&mut self, x: f32) {
        self.drag = Drag::Splitter { start_x: x, start_w: self.nav.sidebar.state.sidebar_w };
    }

    pub(super) fn drag_splitter(&mut self, x: f32, start_x: f32, start_w: f32) {
        let w = (start_w + (x - start_x) / self.scale).clamp(SIDEBAR_MIN_W, SIDEBAR_MAX_W);
        self.nav.sidebar.state.sidebar_w = w.round();
        self.layout();
        self.dirty = true;
    }

    fn reveal_side_cursor(&mut self) {
        self.layout();
        let sb = &mut self.nav.sidebar;
        let Some(r) = sb.rects.get(sb.cursor).copied() else { return };
        if r.y < sb.rect.y {
            sb.scroll -= sb.rect.y - r.y;
        } else if r.bottom() > sb.rect.bottom() {
            sb.scroll += r.bottom() - sb.rect.bottom();
        }
        self.layout();
    }

    pub(super) fn sidebar_key(&mut self, key: Key, m: Mods, now: Instant) -> bool {
        let n = self.nav.sidebar.items.len();
        let plain = !m.ctrl && !m.alt && !m.shift;
        let cur = self.nav.sidebar.cursor;
        let to = match key {
            Key::Escape => {
                self.focus = Focus::List;
                return true;
            }
            Key::Enter if !m.alt => {
                self.open_side_item(cur, m.ctrl, now);
                return true;
            }
            Key::Delete if plain => {
                if self.nav.sidebar.items.get(cur).is_some_and(|it| it.section == Section::Pinned) {
                    self.toggle_pin_item(cur);
                }
                return true;
            }
            Key::Up if plain => cur.saturating_sub(1),
            Key::Down if plain => (cur + 1).min(n.saturating_sub(1)),
            Key::Home if plain => 0,
            Key::End if plain => n.saturating_sub(1),
            _ => return false,
        };
        self.nav.sidebar.cursor = to;
        self.reveal_side_cursor();
        true
    }

    pub(super) fn draw_sidebar<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme) {
        let sb = &self.nav.sidebar;
        let area = sb.rect;
        if area.w <= 0.0 {
            return;
        }
        let compact = sb.prefs.compact;
        g.dl.rect(area, t.chrome);
        let line = if matches!(self.hover, Hit::Splitter) || matches!(self.drag, Drag::Splitter { .. }) { t.accent } else { t.separator };
        g.dl.rect(Rect::new(area.right() - 1.0, area.y, 1.0, area.h), line);
        g.dl.set_clip(Some(Rect::new(area.x, area.y, area.w - 1.0, (sb.settings.y - area.y - self.s(4.0)).max(0.0))));
        let font = self.ui_font(13.0);
        let small = self.ui_font(12.0);
        let icons = self.icon_font(14.0);
        let pin_font = self.icon_font(11.0);
        let chevron = self.icon_font(8.0);
        let m = g.metrics(font);
        let ms = g.metrics(small);
        let (s4, s8, s12, s36) = (self.s(4.0), self.s(8.0), self.s(12.0), self.s(36.0));
        let here = self.tabs[self.cur].location();
        let focused = self.active && self.focus == Focus::Sidebar;
        let (drop, drop_header) = match self.drop_spot() {
            Some(super::Spot::Side(i)) => (Some(i), None),
            Some(super::Spot::SideHeader(s)) => (None, Some(s)),
            _ => (None, None),
        };
        for &(r, section) in &sb.headers {
            if drop_header == Some(section) {
                g.dl.round_rect(r, t.selection, s4);
                g.dl.outline(r, t.accent, s4, self.s(1.0).max(1.0));
            } else if self.hover == Hit::SideHeader(section) {
                g.dl.round_rect(r, t.button_hover, s4);
            }
            if compact {
                g.dl.rect(Rect::new(r.x + s8, (r.y + r.h / 2.0).round(), r.w - 2.0 * s8, 1.0), t.separator);
                continue;
            }
            let baseline = (r.y + (r.h + ms.ascent - ms.descent) / 2.0 + self.s(3.0)).round();
            g.text(r.x + s8, baseline, section.title(), small, t.text_dim, r.w - s8 - s12 - s8);
            let glyph = if sb.is_collapsed(section) { G_COLLAPSED } else { G_EXPANDED };
            centered(g, Rect::new(r.right() - s12 - s8, r.y + self.s(3.0), s12 + s4, r.h), glyph, chevron, t.text_dim);
        }
        if let Some(r) = sb.hint {
            let baseline = (r.y + (r.h + ms.ascent - ms.descent) / 2.0).round();
            g.text(r.x + s12, baseline, S::SidebarPinHint.t(), small, t.text_dim.alpha(0xA0), r.w - s12);
        }
        let mut requests: Vec<IconRequest> = Vec::new();
        for (i, (it, &r)) in sb.items.iter().zip(&sb.rects).enumerate() {
            if r.bottom() < area.y || r.y > area.bottom() {
                continue;
            }
            let hovered = matches!(self.hover, Hit::Side(k) | Hit::SidePin(k) if k == i);
            let row = Rect::new(r.x, r.y + 1.0, r.w, r.h - 2.0);
            let tag = sb.tag_of(i).and_then(|k| sb.tags.defs.get(k));
            if (it.loc == *here && tag.is_none()) || drop == Some(i) {
                g.dl.round_rect(row, t.selection, s4);
            } else if hovered {
                g.dl.round_rect(row, t.button_hover, s4);
            }
            if drop == Some(i) {
                g.dl.outline(row, t.accent, s4, self.s(1.0).max(1.0));
            } else if focused && sb.cursor == i {
                g.dl.outline(row, t.focus_ring, s4, 1.0);
            }
            let color = match tag {
                Some(tag) => Color::rgb(tag.color),
                None if it.section == Section::Drives || it.section == Section::Wsl => t.text_dim,
                None => t.accent,
            };
            if compact {
                if !self.distro_icon(g, it, r, &mut requests) {
                    centered(g, r, it.glyph, icons, color);
                }
                if let Some(used) = it.usage {
                    let bar = Rect::new(r.x + s8, r.bottom() - self.s(5.0), (r.w - 2.0 * s8).max(0.0), self.s(3.0));
                    g.dl.round_rect(bar, t.separator, bar.h / 2.0);
                    let fill = if used >= 0.9 { t.error } else { t.accent };
                    g.dl.round_rect(Rect::new(bar.x, bar.y, (bar.w * used.clamp(0.0, 1.0)).max(bar.h), bar.h), fill, bar.h / 2.0);
                }
                continue;
            }
            let text_h = self.s(ROW_H);
            let icon_r = Rect::new(r.x + s4, r.y, self.s(28.0), text_h);
            if !self.distro_icon(g, it, icon_r, &mut requests) {
                centered(g, icon_r, it.glyph, icons, color);
            }
            let baseline = (r.y + (text_h + m.ascent - m.descent) / 2.0).round();
            let pin_r = sb.pin_rect(Rect::new(r.x, r.y, r.w, text_h), self.scale);
            let text_right = if hovered && tag.is_none() { pin_r.x - s4 } else { r.right() - s8 };
            g.text(r.x + s36, baseline, &it.label, font, t.text, text_right - r.x - s36);
            if let Some(used) = it.usage {
                let bar = Rect::new(r.x + s36, r.y + text_h - self.s(2.0), (r.right() - s12 - r.x - s36).max(0.0), self.s(4.0));
                g.dl.round_rect(bar, t.separator, bar.h / 2.0);
                let fill = if used >= 0.9 { t.error } else { t.accent };
                g.dl.round_rect(Rect::new(bar.x, bar.y, (bar.w * used.clamp(0.0, 1.0)).max(bar.h), bar.h), fill, bar.h / 2.0);
            }
            if hovered && tag.is_none() {
                if self.hover == Hit::SidePin(i) {
                    g.dl.round_rect(pin_r, t.button_hover, s4);
                }
                let glyph = if sb.is_pinned(&it.loc) { G_UNPIN } else { G_PIN };
                centered(g, pin_r, glyph, pin_font, t.text_dim);
            }
        }
        if let Drag::PinMove { to, .. } = self.drag {
            let n = sb.state.pinned.len().min(sb.rects.len());
            let before = sb.rects.get(to).filter(|_| to < n).map(|r| r.y);
            let y = before.or_else(|| sb.rects.get(n.saturating_sub(1)).map(|r| r.bottom())).unwrap_or(area.y);
            g.dl.rect(Rect::new(area.x + s8, y - 1.0, area.w - 2.0 * s8, self.s(2.0)), t.accent);
        }
        g.dl.set_clip(None);
        let r = sb.settings;
        g.dl.rect(Rect::new(area.x + s8, r.y - self.s(4.0), area.w - 2.0 * s8, 1.0), t.separator);
        if self.hover == Hit::SideSettings || self.settings_open() {
            g.dl.round_rect(r, if self.settings_open() { t.selection } else { t.button_hover }, s4);
        }
        if compact {
            centered(g, r, G_SETTINGS, icons, t.text_dim);
        } else {
            centered(g, Rect::new(r.x + s4, r.y, self.s(28.0), r.h), G_SETTINGS, icons, t.text_dim);
            let baseline = (r.y + (r.h + m.ascent - m.descent) / 2.0).round();
            g.text(r.x + s36, baseline, S::MenuSettings.t(), font, t.text, r.w - s36);
        }
        self.request_icons(requests);
    }

    fn distro_icon<B: TextBackend>(&self, g: &mut Gfx<B>, it: &Item, r: Rect, requests: &mut Vec<IconRequest>) -> bool {
        let Location::Wsl { distro, .. } = &it.loc else { return false };
        let Some(path) = it.loc.to_windows().filter(|_| it.section == Section::Wsl) else { return false };
        let px = self.s(16.0);
        let key = hash::hash(&("vol", distro.as_bytes(), px as u32));
        if g.image(key, Rect::new((r.x + (r.w - px) / 2.0).round(), (r.y + (r.h - px) / 2.0).round(), px, px)) {
            return true;
        }
        if !self.icons.contains_key(&key) && !requests.iter().any(|q| q.key == key) {
            requests.push(IconRequest { key, kind: IconKind::Volume, path, size: px as u32, gen: 0, row: 0 });
        }
        false
    }
}

#[cfg(test)]
#[path = "../../test/ui/sidebar.rs"]
mod tests;
