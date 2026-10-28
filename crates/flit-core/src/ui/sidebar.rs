use super::confirm::Confirmed;
use super::edit::EditAction;
use super::field::{Field, FieldKey};
use super::fuzzy::fuzzy;
use super::home::{Recent, Widget};
use super::menu::Menu;
use super::tags::G_TAG;
use super::widgets::{Button, Paint};
use super::{centered, App, Drag, Effect, Focus, Hit, IconKind, IconRequest, Route, ShellMenu, G_CLOSE, G_DRIVE, G_FILE, G_FOLDER, TOOL_H};
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
use crate::text::{FontFamily, FontStyle, Gfx, TextBackend};
use crate::theme::{ink_on, Theme};
use serde::{Deserialize, Serialize};
use std::ops::Range;
use std::sync::Arc;
use std::time::Instant;

pub const SIDEBAR_MIN_W: f32 = 160.0;
pub const SIDEBAR_MAX_W: f32 = 480.0;
const COMPACT_W: f32 = 48.0;
const ROW_H: f32 = 25.0;
const COMPACT_ROW_H: f32 = 32.0;
const DRIVE_H: f32 = 29.0;
const HEADER_H: f32 = 26.0;
const COMPACT_HEADER_H: f32 = 12.0;
const PIN_W: f32 = 22.0;
const SECTION_GAP: f32 = 11.0;
const ITEM_INDENT: f32 = 18.0;
pub(super) const PLACE_ICON: f32 = 20.0;
const HEADER_TITLE_X: f32 = 28.0;
const BAR_MARGIN: f32 = 12.0;
const MAX_LIST: usize = 256;
const RECENT_MAX: usize = 10;
const MAX_VISITS: usize = 64;
pub(super) const MOST_VISITED: usize = 5;
const PATH_GAP: f32 = 6.0;

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
const G_NET_OFFLINE: &str = "\u{E7BA}";
const G_NETWORK: &str = "\u{E72D}";
pub(super) const G_PIN: &str = "\u{E718}";
const G_TRASH: &str = "\u{E74D}";
pub(super) const G_EXPANDED: &str = "\u{E70D}";
pub(super) const G_COLLAPSED: &str = "\u{E76C}";
const G_RECENT: &str = "\u{E81C}";
const G_SEARCH: &str = "\u{E721}";
const G_SHIFT_UP: &str = "\u{E70E}";
const G_RESTORE_ORDER: &str = "\u{E777}";
const G_PATHS: &str = "\u{E8FD}";
pub(super) const G_SETTINGS: &str = "\u{E713}";

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum KnownFolder {
    Home,
    Desktop,
    Downloads,
    Documents,
    Pictures,
    Music,
    Videos,
    Favorites,
}

impl KnownFolder {
    pub const ALL: [KnownFolder; 8] = [Self::Home, Self::Desktop, Self::Downloads, Self::Documents, Self::Pictures, Self::Music, Self::Videos, Self::Favorites];
    const PLACES: [KnownFolder; 7] = [Self::Home, Self::Desktop, Self::Downloads, Self::Documents, Self::Music, Self::Pictures, Self::Videos];

    pub fn label(self) -> &'static str {
        match self {
            Self::Home => S::SidebarFolderHome.t(),
            Self::Desktop => S::SidebarFolderDesktop.t(),
            Self::Downloads => S::SidebarFolderDownloads.t(),
            Self::Documents => S::SidebarFolderDocuments.t(),
            Self::Pictures => S::SidebarFolderPictures.t(),
            Self::Music => S::SidebarFolderMusic.t(),
            Self::Videos => S::SidebarFolderVideos.t(),
            Self::Favorites => S::SidebarFolderFavorites.t(),
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
            Self::Favorites => "shell:Favorites",
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
            Self::Favorites => G_PIN,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Drive {
    pub path: FsPath,
    pub label: String,
    pub total: u64,
    pub free: u64,
    pub removable: bool,
    pub unmounted: bool,
    pub kind: Option<S>,
    pub remote: Option<String>,
}

impl Drive {
    pub fn title(&self) -> String {
        let title = match self.kind.and_then(|k| Some((k, self.label.strip_prefix(k.en())?))) {
            Some((k, rest)) => format!("{}{rest}", k.t()),
            None => self.label.clone(),
        };
        match self.offline() {
            true => format!("{title} \u{2014} {}", S::SidebarDriveOffline.t()),
            false => title,
        }
    }

    pub fn offline(&self) -> bool {
        self.unmounted && self.remote.is_some()
    }

    fn glyph(&self) -> &'static str {
        match () {
            _ if self.path.is_network() => G_NETWORK,
            _ if self.offline() => G_NET_OFFLINE,
            _ => G_DRIVE,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
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
    #[serde(skip)]
    pub quick: QuickPrefs,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct QuickPrefs {
    pub order: Vec<String>,
    pub show_paths: bool,
    pub visits: Vec<(Location, u32)>,
}

impl NavPrefs {
    pub fn sanitized(mut self) -> Self {
        for v in [&mut self.collapsed, &mut self.hidden, &mut self.widgets_off, &mut self.quick.order] {
            v.truncate(MAX_LIST);
        }
        let n = self.recent_hidden.len();
        self.recent_hidden.drain(..n.saturating_sub(MAX_LIST));
        self.quick.visits.truncate(MAX_VISITS);
        self
    }

    pub fn order(&self) -> Vec<Section> {
        let mut out: Vec<Section> = self.quick.order.iter().filter_map(|id| Section::ALL.into_iter().find(|s| s.id() == id)).collect();
        out.dedup();
        for s in Section::ALL {
            if !out.contains(&s) {
                out.push(s);
            }
        }
        out
    }

    pub fn count_visit(&mut self, loc: &Location) {
        let v = &mut self.quick.visits;
        match v.iter().position(|(l, _)| l == loc) {
            Some(i) => v[i].1 = v[i].1.saturating_add(1),
            None => {
                if v.len() >= MAX_VISITS {
                    if let Some(i) = v.iter().enumerate().min_by_key(|(_, e)| e.1).map(|(i, _)| i) {
                        v.remove(i);
                    }
                }
                v.push((loc.clone(), 1));
            }
        }
    }

    pub fn most_visited(&self) -> Vec<Location> {
        let mut v: Vec<&(Location, u32)> = self.quick.visits.iter().collect();
        v.sort_by_key(|e| std::cmp::Reverse(e.1));
        v.into_iter().take(MOST_VISITED).map(|e| e.0.clone()).collect()
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
    Recents,
    Bookmarks,
    Storage,
    Places,
    Libraries,
    Wsl,
    Tags,
}

impl Section {
    pub const ALL: [Section; 7] = [Section::Recents, Section::Bookmarks, Section::Storage, Section::Places, Section::Libraries, Section::Wsl, Section::Tags];

    pub fn title(self) -> &'static str {
        match self {
            Section::Recents => S::SidebarSectionRecents.t(),
            Section::Bookmarks => S::SidebarSectionBookmarks.t(),
            Section::Storage => S::SidebarSectionStorage.t(),
            Section::Places => S::SidebarSectionPlaces.t(),
            Section::Libraries => S::SidebarSectionLibraries.t(),
            Section::Wsl => "WSL",
            Section::Tags => S::SidebarSectionTags.t(),
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Section::Recents => "recent",
            Section::Bookmarks => "pinned",
            Section::Storage => "drives",
            Section::Places => "folders",
            Section::Libraries => "libraries",
            Section::Wsl => "wsl",
            Section::Tags => "tags",
        }
    }

    pub fn glyph(self) -> &'static str {
        match self {
            Section::Recents => G_RECENT,
            Section::Bookmarks => G_PIN,
            Section::Storage => G_DRIVE,
            Section::Places => G_FOLDER,
            Section::Libraries => G_LIBRARY,
            Section::Wsl => G_LINUX,
            Section::Tags => G_TAG,
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
    Pane(Location, bool),
    Launch(Launch),
    Fold(bool),
    ShowPaths,
    Shift(Section, isize),
    RestoreOrder,
    ViewMode(super::details::ViewMode),
    DefaultView,
}

#[derive(Clone, Debug)]
pub(super) struct Item {
    pub section: Section,
    pub label: String,
    pub glyph: &'static str,
    pub loc: Location,
    pub usage: Option<f32>,
    pub path: String,
    pub marks: Vec<Range<usize>>,
}

#[derive(Default)]
pub(super) struct Sidebar {
    pub full_paths: bool,
    pub state: NavState,
    pub prefs: NavPrefs,
    pub places: Arc<Places>,
    pub items: Vec<Item>,
    pub rects: Vec<Rect>,
    pub headers: Vec<(Rect, Section)>,
    pub hint: Option<Rect>,
    pub rect: Rect,
    pub list: Rect,
    pub splitter: Rect,
    pub settings: Rect,
    pub bar: Rect,
    pub field: Rect,
    pub clear: Rect,
    pub filter: Field,
    pub typing: bool,
    scroll: f32,
    content_h: f32,
    pub cursor: usize,
    pub press_y: f32,
    pub tags: Arc<Tags>,
    pub bar_w: f32,
}

pub(super) fn pinned_label(loc: &Location) -> String {
    loc.crumbs().pop().map(|(label, _)| label).unwrap_or_default()
}

pub(super) fn short_path(loc: &Location, home: Option<&Location>) -> String {
    if matches!(loc.path().kind(), PathKind::Computer | PathKind::RecycleBin | PathKind::Home) {
        return String::new();
    }
    let seps: &[char] = &['\\', '/'];
    let shown = loc.display();
    let full = match shown.trim_end_matches(seps) {
        "" => shown.as_ref(),
        trimmed => trimmed,
    };
    let base = home.and_then(Location::parent).map(|b| b.display().trim_end_matches(seps).to_owned());
    if let Some(b) = base.filter(|b| !b.is_empty() && full.len() > b.len() + 1) {
        if full.is_char_boundary(b.len()) && full[..b.len()].eq_ignore_ascii_case(&b) && full[b.len()..].starts_with(seps) {
            return full[b.len() + 1..].to_owned();
        }
    }
    full.to_owned()
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
    fn known(&self, k: KnownFolder) -> Option<&Location> {
        self.places.folders.iter().find(|(f, _)| *f == k).map(|(_, l)| l)
    }

    pub(super) fn section_items(&self, section: Section) -> Vec<Item> {
        let p = &self.places;
        let home = self.known(KnownFolder::Home).filter(|_| !self.full_paths);
        let item = |label: String, glyph, loc: Location, usage| Item { section, label, glyph, path: short_path(&loc, home), loc, usage, marks: Vec::new() };
        match section {
            Section::Recents => self.recent().take(RECENT_MAX).map(|(_, r)| item(r.name.clone(), if r.dir { G_FOLDER } else { G_FILE }, Location::from_local(r.target.clone()), None)).collect(),
            Section::Bookmarks => {
                let library_name = |l: &Location| p.libraries.iter().find(|lib| lib.path == *l.path()).map(|lib| lib.name.clone());
                self.state.pinned.iter().map(|l| item(library_name(l).unwrap_or_else(|| pinned_label(l)), pinned_glyph(l), l.clone(), None)).collect()
            }
            Section::Storage => p.drives.iter().map(|d| item(d.title(), d.glyph(), Location::Local(d.path.clone()), (d.total > 0).then(|| (d.total.saturating_sub(d.free)) as f32 / d.total as f32))).collect(),
            Section::Places => {
                let computer = Location::computer();
                let mut out = vec![item(pinned_label(&computer), G_DRIVE, computer, None)];
                for k in KnownFolder::PLACES {
                    if let Some(l) = self.known(k) {
                        let label = if k == KnownFolder::Home { pinned_label(l) } else { k.label().to_owned() };
                        out.push(item(label, k.glyph(), l.clone(), None));
                    }
                }
                if p.recycle_bin {
                    out.push(item(S::PathRecycleBin.t().to_owned(), G_RECYCLE_BIN, Location::recycle_bin(), None));
                }
                if let Some(l) = self.known(KnownFolder::Favorites) {
                    out.push(item(KnownFolder::Favorites.label().to_owned(), G_PIN, l.clone(), None));
                }
                out.push(item(S::PathHome.t().to_owned(), G_HOME, Location::home_page(), None));
                out
            }
            Section::Libraries => p.libraries.iter().map(|l| item(l.name.clone(), G_LIBRARY, Location::Local(l.path.clone()), None)).collect(),
            Section::Wsl => p.wsl.iter().filter_map(|d| Some(item(d.clone(), G_LINUX, Location::parse(&format!("wsl://{d}/"))?, None))).collect(),
            Section::Tags => self.tags.defs.iter().map(|t| Item { section, label: t.name.clone(), glyph: G_TAG, loc: Location::computer(), usage: None, path: String::new(), marks: Vec::new() }).collect(),
        }
    }

    pub(super) fn rebuild(&mut self) {
        let query = self.filter.text().trim().to_owned();
        let mut items = Vec::new();
        for s in self.prefs.order() {
            if self.is_hidden(s) || (query.is_empty() && self.is_collapsed(s)) {
                continue;
            }
            let mut found = self.section_items(s);
            if !query.is_empty() {
                found.retain_mut(|it| fuzzy(&query, &it.label).map(|m| it.marks = m.ranges).is_some());
            }
            items.extend(found);
        }
        self.items = items;
        self.cursor = self.cursor.min(self.items.len().saturating_sub(1));
    }

    pub(super) fn filtering(&self) -> bool {
        !self.filter.text().trim().is_empty()
    }

    pub(super) fn shown_sections(&self) -> Vec<Section> {
        self.prefs.order().into_iter().filter(|&s| !self.is_hidden(s) && self.has_items(s) && (!self.filtering() || self.items.iter().any(|it| it.section == s))).collect()
    }

    pub(super) fn recent(&self) -> impl Iterator<Item = (usize, &Recent)> {
        let p = &self.prefs;
        self.places.recent.iter().enumerate().filter(move |(_, r)| r.at > p.recent_cleared && !p.recent_hidden.iter().any(|(path, at)| *path == r.target && r.at <= *at))
    }

    pub(super) fn recent_entry(&self, i: usize) -> Option<usize> {
        let it = self.items.get(i).filter(|it| it.section == Section::Recents)?;
        self.places.recent.iter().position(|r| Location::from_local(r.target.clone()) == it.loc)
    }

    pub fn recent_file(&self, i: usize) -> Option<FsPath> {
        let r = &self.places.recent[self.recent_entry(i)?];
        (!r.dir).then(|| r.target.clone())
    }

    pub fn removable(&self, i: usize) -> bool {
        self.items.get(i).is_some_and(|it| it.section == Section::Bookmarks)
    }

    fn first_bookmark(&self) -> usize {
        self.items.iter().position(|it| it.section == Section::Bookmarks).unwrap_or(0)
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
            Section::Bookmarks => true,
            Section::Recents => self.recent().next().is_some(),
            Section::Places => true,
            Section::Libraries => !p.libraries.is_empty(),
            Section::Storage => !p.drives.is_empty(),
            Section::Wsl => !p.wsl.is_empty(),
            Section::Tags => !self.tags.defs.is_empty(),
        }
    }

    pub fn pin_rect(&self, r: Rect, scale: f32) -> Rect {
        let w = (PIN_W * scale).round();
        Rect::new(r.right() - w - (4.0 * scale).round(), r.y + ((r.h - w) / 2.0).round(), w, w)
    }

    pub fn min_w(&self) -> f32 {
        (6.0 + ITEM_INDENT + 36.0 + self.bar_w + 12.0 + 6.0).ceil().clamp(SIDEBAR_MIN_W, SIDEBAR_MAX_W)
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
        self.go_refresh();
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
            self.go_refresh();
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

    pub(super) fn nav_prefs_changed(&mut self) {
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

    fn fold_sections(&mut self, collapsed: bool) {
        let shown = self.nav.sidebar.shown_sections();
        let p = &mut self.nav.sidebar.prefs;
        p.collapsed.retain(|id| !shown.iter().any(|s| s.id() == id));
        if collapsed {
            p.collapsed.extend(shown.iter().map(|s| s.id().to_owned()));
        }
        self.nav_prefs_changed();
    }

    fn shift_section(&mut self, s: Section, by: isize) {
        let shown = self.nav.sidebar.shown_sections();
        let Some(at) = shown.iter().position(|&x| x == s) else { return };
        let Some(&other) = at.checked_add_signed(by).and_then(|k| shown.get(k)) else { return };
        let mut order = self.nav.sidebar.prefs.order();
        let (a, b) = (order.iter().position(|&x| x == s), order.iter().position(|&x| x == other));
        if let (Some(a), Some(b)) = (a, b) {
            order.swap(a, b);
        }
        self.nav.sidebar.prefs.quick.order = order.iter().map(|s| s.id().to_owned()).collect();
        self.nav_prefs_changed();
    }

    pub(super) fn clear_bookmarks(&mut self) {
        self.nav.sidebar.state.pinned.clear();
        self.pins_changed();
    }

    pub(super) fn focus_quick_filter(&mut self) {
        if !self.nav.sidebar.state.sidebar {
            self.toggle_sidebar();
        }
        if self.nav.sidebar.prefs.compact {
            self.toggle_compact();
        }
        self.close_palette();
        let sb = &mut self.nav.sidebar;
        sb.typing = true;
        sb.filter.edit.select_all();
        sb.cursor = 0;
        self.focus = Focus::Sidebar;
        self.dirty = true;
    }

    fn quick_filter_changed(&mut self) {
        let sb = &mut self.nav.sidebar;
        sb.scroll = 0.0;
        sb.cursor = 0;
        self.nav_prefs_changed();
    }

    pub(super) fn quick_insert(&mut self, s: &str) -> bool {
        let sb = &mut self.nav.sidebar;
        if self.focus != Focus::Sidebar || !sb.typing {
            return false;
        }
        sb.filter.insert(s);
        self.quick_filter_changed();
        true
    }

    pub(super) fn clear_quick_filter(&mut self) {
        let sb = &mut self.nav.sidebar;
        sb.filter.edit.set("");
        sb.typing = false;
        if self.focus == Focus::Sidebar {
            self.focus = Focus::List;
        }
        self.quick_filter_changed();
    }

    pub(super) fn quick_filter_click(&mut self, x: f32, clicks: u8) {
        self.nav.sidebar.filter.click(x, clicks);
        self.nav.sidebar.typing = true;
        self.focus = Focus::Sidebar;
        self.dirty = true;
    }

    fn quick_filter_key(&mut self, key: Key, m: Mods) -> bool {
        match self.nav.sidebar.filter.key(key, m) {
            FieldKey::Edited { changed, action } => {
                match action {
                    EditAction::Copy(s) => self.effects.push(Effect::SetClipboard(s)),
                    EditAction::Paste => self.effects.push(Effect::Paste),
                    _ => {}
                }
                if changed {
                    self.quick_filter_changed();
                }
                true
            }
            FieldKey::Ignored => false,
        }
    }

    pub(super) fn count_visit(&mut self, loc: &Location) {
        if !loc.is_home_page() && self.opts.recents != super::options::Recents::Never {
            self.nav.sidebar.prefs.count_visit(loc);
        }
    }

    pub(super) fn cycle_sidebar_focus(&mut self) {
        if self.focus == Focus::Sidebar {
            self.focus = Focus::List;
        } else {
            if !self.nav.sidebar.state.sidebar {
                self.toggle_sidebar();
            }
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

    pub(super) fn toggle_pin_item(&mut self, i: usize, now: Instant) {
        let sb = &self.nav.sidebar;
        let Some(loc) = sb.items.get(i).filter(|_| sb.removable(i)).map(|it| it.loc.clone()) else { return };
        if let Some(&r) = sb.rects.get(i) {
            let anchor = if sb.prefs.compact { r } else { sb.pin_rect(r, self.scale) };
            self.ask(anchor, (S::WidgetsRemoveItem.t(), S::WidgetsYes.t()), true, Confirmed::Unpin(loc), now);
        }
    }

    pub(super) fn unpin(&mut self, loc: Location, _now: Instant) {
        if self.nav.sidebar.state.pinned.contains(&loc) {
            self.toggle_pin(loc);
        }
    }

    pub(super) fn start_pin_drag(&mut self, y: f32) -> bool {
        let Hit::Side(i) = self.pressed else { return false };
        let sb = &self.nav.sidebar;
        if !sb.removable(i) || sb.filtering() || (y - sb.press_y).abs() < self.s(5.0) {
            return false;
        }
        self.drag_pin(i - sb.first_bookmark(), y);
        true
    }

    pub(super) fn drag_pin(&mut self, from: usize, y: f32) {
        let sb = &self.nav.sidebar;
        let to = sb.rects.iter().skip(sb.first_bookmark()).take(sb.state.pinned.len()).filter(|r| r.y + r.h / 2.0 < y).count();
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
        if let Some(file) = self.nav.sidebar.recent_file(i) {
            return self.place_command(PlaceCmd::OpenFile(file), now);
        }
        let Some(folder) = self.nav.sidebar.items.get(i).map(|it| it.loc.clone()) else { return };
        if let Some(d) = self.nav.sidebar.places.drives.iter().find(|d| d.unmounted && d.path == *folder.path()) {
            let (tab, launch) = (self.tab().id, Launch::Mount(d.path.clone()));
            return self.effects.push(Effect::Provider { provider: ProviderId::Local, req: ProviderRequest::Launch { tab, launch } });
        }
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
            PlaceCmd::Launch(l) => self.effects.push(launch(l)),
            PlaceCmd::Properties(path) => self.effects.push(Effect::Properties(vec![path])),
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
            PlaceCmd::Pane(loc, down) => self.open_in_panel(loc, down, now),
            PlaceCmd::Fold(collapsed) => self.fold_sections(collapsed),
            PlaceCmd::ShowPaths => {
                let q = &mut self.nav.sidebar.prefs.quick;
                q.show_paths = !q.show_paths;
                self.nav_prefs_changed();
            }
            PlaceCmd::Shift(s, by) => self.shift_section(s, by),
            PlaceCmd::ViewMode(mode) => self.set_view_mode(mode),
            PlaceCmd::DefaultView => self.set_default_view(),
            PlaceCmd::RestoreOrder => {
                self.nav.sidebar.prefs.quick.order.clear();
                self.nav_prefs_changed();
            }
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
            if a.supports("drive.disconnect") && a.nav.sidebar.places.drives.iter().any(|d| d.remote.is_some() && d.path == *loc.path() && d.path.kind() == PathKind::Drive) {
                m.place(S::SidebarDriveDisconnect.t(), PlaceCmd::Launch(Launch::Disconnect(Some(loc.path().clone()))), false);
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
        self.show_place_menu(x, y, path, menu, super::MenuKind::Folder);
    }

    pub(super) fn show_place_menu(&mut self, x: f32, y: f32, path: Option<FsPath>, menu: Vec<super::MenuItem>, kind: super::MenuKind) {
        let Some(p) = path else { return self.context_flit_menu(x, y, menu, kind) };
        let shift = self.mods.shift;
        let dir = p.parent().unwrap_or_else(FsPath::computer);
        self.effects.push(Effect::ShellMenu(ShellMenu { dir, items: vec![p], x, y, extended: shift, menu, inline: self.shell_items_inline || shift, shell: true, kind }));
    }

    pub(super) fn build_section_menu(&mut self, s: Section) -> Vec<super::MenuItem> {
        self.collect(|a, m| {
            let sb = &a.nav.sidebar;
            let shown = sb.shown_sections();
            let at = shown.iter().position(|&x| x == s).unwrap_or(0);
            let none_collapsed = shown.iter().all(|&x| !sb.is_collapsed(x));
            m.place_icon(S::SidebarCollapseAll.t(), PlaceCmd::Fold(true), G_COLLAPSED, false);
            m.place_icon(S::SidebarExpandAll.t(), PlaceCmd::Fold(false), G_EXPANDED, none_collapsed);
            m.toggle(S::SidebarShowPaths.t(), PlaceCmd::ShowPaths, sb.prefs.quick.show_paths);
            m.iconed(G_PATHS);
            m.separator();
            m.place_icon(S::SidebarShiftUp.t(), PlaceCmd::Shift(s, -1), G_SHIFT_UP, at == 0);
            m.place_icon(S::SidebarShiftDown.t(), PlaceCmd::Shift(s, 1), G_EXPANDED, at + 1 >= shown.len());
            m.place_icon(S::SidebarRestoreOrder.t(), PlaceCmd::RestoreOrder, G_RESTORE_ORDER, sb.prefs.order() == Section::ALL);
            m.separator();
            match s {
                Section::Recents => {
                    m.action("home.clear_recent", S::SidebarClearRecents.t());
                    m.iconed(G_TRASH);
                }
                Section::Bookmarks => {
                    m.action("bookmarks.clear", S::SidebarClearBookmarks.t());
                    m.iconed(G_TRASH);
                }
                Section::Tags => {
                    m.action("tags.new", S::MenuNewTag.t());
                    m.action("tags.manage", S::MenuManageTags.t());
                }
                Section::Libraries => {
                    m.place(S::SidebarNewLibrary.t(), PlaceCmd::NewLibrary, false);
                    m.place(S::SidebarRestoreLibraries.t(), PlaceCmd::RestoreLibraries, false);
                }
                Section::Storage | Section::Places | Section::Wsl => {}
            }
        })
    }

    fn section_menu(&mut self, x: f32, y: f32) {
        let menu = self.collect(|a, m| {
            let sb = &a.nav.sidebar;
            m.action("drive.map", S::SidebarDriveMap.t());
            m.action("drive.disconnect", S::SidebarDriveDisconnectDialog.t());
            m.action("drive.connect_server", S::SidebarDriveConnectServer.t());
            m.separator();
            m.place(S::SidebarNewLibrary.t(), PlaceCmd::NewLibrary, false);
            m.place(S::SidebarRestoreLibraries.t(), PlaceCmd::RestoreLibraries, false);
            m.separator();
            m.submenu(S::SidebarSections.t(), |m| {
                for s in Section::ALL {
                    m.toggle(s.title(), PlaceCmd::Hide(s), !sb.is_hidden(s));
                }
            });
            m.toggle(S::SidebarCompact.t(), PlaceCmd::Compact, sb.prefs.compact);
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
                if let Some(r) = self.nav.sidebar.recent_entry(i) {
                    return self.recent_menu(x, y, r);
                }
                if let Some(loc) = self.nav.sidebar.items.get(i).map(|it| it.loc.clone()) {
                    self.place_menu(x, y, loc, |_, _| {});
                }
            }
            Hit::SideHeader(s) => {
                let r = self.nav.sidebar.headers.iter().find(|h| h.1 == s).map_or(Rect::new(x, y, 0.0, 0.0), |h| h.0);
                let menu = self.build_section_menu(s);
                self.button_menu(Hit::SideHeader(s), r.x, r.bottom(), menu);
            }
            _ => self.section_menu(x, y),
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
        (sb.bar, sb.field, sb.clear) = (Rect::default(), Rect::default(), Rect::default());
        if width <= 0.0 {
            sb.list = Rect::default();
            return;
        }
        let (x, w) = if compact { (s(4.0), (width - s(8.0)).max(0.0)) } else { (s(6.0), (width - s(12.0)).max(0.0)) };
        let (header_h, row_h) = if compact { (s(COMPACT_HEADER_H), s(COMPACT_ROW_H)) } else { (s(HEADER_H), s(ROW_H)) };
        if !compact {
            sb.bar = Rect::new(0.0, top, width - 1.0, s(TOOL_H));
            sb.field = Rect::new(x, top + s(5.0), w, s(TOOL_H - 10.0));
            let c = s(22.0);
            sb.clear = if sb.filtering() { Rect::new(sb.field.right() - c - s(4.0), sb.field.y + ((sb.field.h - c) / 2.0).round(), c, c) } else { Rect::default() };
        }
        let list_top = sb.bar.bottom().max(top);
        sb.settings = Rect::new(x, sb.rect.bottom() - row_h - s(4.0), w, row_h);
        sb.list = Rect::new(0.0, list_top, width - 1.0, (sb.settings.y - s(4.0) - list_top).max(0.0));
        let mut y = s(4.0);
        let mut it = 0;
        for section in sb.shown_sections() {
            sb.headers.push((Rect::new(x, y, w, header_h), section));
            y += header_h;
            let count = sb.items.iter().filter(|it| it.section == section).count();
            if count == 0 && section == Section::Bookmarks && !compact && !sb.is_collapsed(section) {
                sb.hint = Some(Rect::new(x, y, w, row_h));
                y += row_h;
            }
            while sb.items.get(it).is_some_and(|i| i.section == section) {
                let h = if sb.items[it].usage.is_some() && !compact { s(DRIVE_H) } else { row_h };
                sb.rects.push(Rect::new(x, y, w, h));
                y += h;
                it += 1;
            }
            y += if compact { s(2.0) } else { s(SECTION_GAP) };
        }
        sb.content_h = y;
        sb.scroll = sb.scroll.clamp(0.0, (sb.content_h - sb.list.h).max(0.0));
        let dy = list_top - sb.scroll;
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
        if sb.clear.contains(x, y) {
            return Some(Hit::QuickClear);
        }
        if sb.field.contains(x, y) {
            return Some(Hit::QuickField);
        }
        if !sb.list.contains(x, y) {
            return Some(Hit::SideEmpty);
        }
        for (i, r) in sb.rects.iter().enumerate() {
            if r.contains(x, y) {
                let button = !sb.prefs.compact && sb.pin_rect(*r, self.scale).contains(x, y) && sb.removable(i);
                return Some(if button { Hit::SidePin(i) } else { Hit::Side(i) });
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
        sb.scroll = (sb.scroll - dy * step).clamp(0.0, (sb.content_h - sb.list.h).max(0.0));
        self.layout();
        self.dirty = true;
    }

    pub(super) fn begin_splitter_drag(&mut self, x: f32) {
        self.drag = Drag::Splitter { start_x: x, start_w: self.nav.sidebar.state.sidebar_w };
    }

    pub(super) fn drag_splitter(&mut self, x: f32, start_x: f32, start_w: f32) {
        let w = (start_w + (x - start_x) / self.scale).clamp(self.nav.sidebar.min_w(), SIDEBAR_MAX_W);
        self.nav.sidebar.state.sidebar_w = w.round();
        self.layout();
        self.dirty = true;
    }

    fn reveal_side_cursor(&mut self) {
        self.layout();
        let sb = &mut self.nav.sidebar;
        let Some(r) = sb.rects.get(sb.cursor).copied() else { return };
        if r.y < sb.list.y {
            sb.scroll -= sb.list.y - r.y;
        } else if r.bottom() > sb.list.bottom() {
            sb.scroll += r.bottom() - sb.list.bottom();
        }
        self.layout();
    }

    pub(super) fn sidebar_key(&mut self, key: Key, m: Mods, now: Instant) -> bool {
        let n = self.nav.sidebar.items.len();
        let plain = !m.ctrl && !m.alt && !m.shift;
        let cur = self.nav.sidebar.cursor;
        let typing = self.nav.sidebar.typing;
        if typing && !matches!(key, Key::Escape | Key::Enter | Key::Up | Key::Down | Key::PageUp | Key::PageDown) && self.quick_filter_key(key, m) {
            return true;
        }
        let to = match key {
            Key::Escape if typing && self.nav.sidebar.filtering() => {
                self.clear_quick_filter();
                return true;
            }
            Key::Escape => {
                self.nav.sidebar.typing = false;
                self.focus = Focus::List;
                return true;
            }
            Key::Enter if !m.alt => {
                self.open_side_item(cur, m.ctrl, now);
                return true;
            }
            Key::Delete if plain => {
                if self.nav.sidebar.items.get(cur).is_some_and(|it| it.section == Section::Bookmarks) {
                    self.toggle_pin_item(cur, now);
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
        let font = self.ui_font(13.0);
        let small = self.ui_font(12.0);
        let icons = self.icon_font(14.0);
        let pin_font = self.icon_font(11.0);
        let chevron = self.icon_font(8.0);
        let header_font = self.header_font();
        let header_icons = self.icon_font(13.0);
        let m = g.metrics(font);
        let ms = g.metrics(small);
        let mh = g.metrics(header_font);
        let (s4, s8, s36) = (self.s(4.0), self.s(8.0), self.s(36.0));
        let here = self.tabs[self.cur].location();
        let focused = self.active && self.focus == Focus::Sidebar;
        if sb.field.w > 0.0 {
            let typing = focused && sb.typing;
            let mut w = Paint::new(g, t, self.scale);
            w.field(sb.field, typing);
            let fr = sb.field;
            w.centered(Rect::new(fr.x, fr.y, fr.h, fr.h), G_SEARCH, self.icon_font(11.0), t.text_dim);
            let right = if sb.clear.w > 0.0 { sb.clear.x - s4 } else { fr.right() - s8 };
            let inner = Rect::new(fr.x + fr.h, fr.y, (right - fr.x - fr.h).max(0.0), fr.h);
            let preedit = typing.then_some(&self.preedit);
            self.nav.sidebar.filter.draw(g, t, inner, font, typing, S::SidebarFilter.t(), self.scale, preedit);
            let sb = &self.nav.sidebar;
            if sb.clear.w > 0.0 {
                Paint::new(g, t, self.scale).icon_button(sb.clear, G_CLOSE, self.icon_font(8.0), Button::of(self.hover == Hit::QuickClear, false, true), t.text_dim);
            }
        }
        let sb = &self.nav.sidebar;
        g.dl.set_clip(Some(sb.list));
        let (drop, drop_header) = match self.drop_spot() {
            Some(super::Spot::Side(i)) => (Some(i), None),
            Some(super::Spot::SideHeader(s)) => (None, Some(s)),
            _ => (None, None),
        };
        let target = t.teal.alpha(0x70);
        for &(r, section) in &sb.headers {
            let open = self.menu_open_from(Hit::SideHeader(section));
            let lit = open || self.hover == Hit::SideHeader(section);
            if drop_header == Some(section) {
                g.dl.round_rect(r, target, s4);
            } else if lit {
                g.dl.round_rect(r, t.accent_fill, s4);
            }
            if compact {
                g.dl.rect(Rect::new(r.x + s8, (r.y + r.h / 2.0).round(), r.w - 2.0 * s8, 1.0), t.separator);
                continue;
            }
            let ink = if lit && drop_header != Some(section) { ink_on(t.accent_fill) } else { t.text };
            let (icon_r, title_x, baseline, chevron_r) = header_parts(r, mh.ascent, mh.descent, self.scale);
            centered(g, icon_r, section.glyph(), header_icons, ink);
            g.text(title_x, baseline, section.title(), header_font, ink, chevron_r.x - title_x - s4);
            let glyph = if sb.is_collapsed(section) && !sb.filtering() { G_COLLAPSED } else { G_EXPANDED };
            centered(g, chevron_r, glyph, chevron, if lit { ink } else { t.text_dim });
        }
        if let Some(r) = sb.hint {
            let baseline = (r.y + (r.h + ms.ascent - ms.descent) / 2.0).round();
            let x = r.x + self.s(HEADER_TITLE_X);
            g.text(x, baseline, S::SidebarPinHint.t(), small, t.text_dim.alpha(0xA0), r.right() - x);
        }
        let mut requests: Vec<IconRequest> = Vec::new();
        let list = sb.list;
        for (i, (it, &r)) in sb.items.iter().zip(&sb.rects).enumerate() {
            if r.bottom() < list.y || r.y > list.bottom() {
                continue;
            }
            let hovered = matches!(self.hover, Hit::Side(k) | Hit::SidePin(k) if k == i) && self.drag == Drag::None;
            let row = Rect::new(r.x, r.y + 1.0, r.w, r.h - 2.0);
            let tag = sb.tag_of(i).and_then(|k| sb.tags.defs.get(k));
            if drop == Some(i) {
                g.dl.round_rect(row, target, s4);
            } else if hovered {
                g.dl.round_rect(row, t.accent_fill, s4);
            } else if it.loc == *here && tag.is_none() {
                g.dl.round_rect(row, t.selection, s4);
            }
            if focused && !sb.typing && sb.cursor == i && drop != Some(i) {
                g.dl.outline(row, t.focus_ring, s4, 1.0);
            }
            let lit = hovered && drop != Some(i);
            let ink = if lit { ink_on(t.accent_fill) } else { t.text };
            let color = match tag {
                Some(tag) => Color::rgb(tag.color),
                None if lit => ink,
                None if it.section == Section::Storage || it.section == Section::Wsl => t.text_dim,
                None => t.accent,
            };
            if compact {
                if !self.place_icon(g, it, r, &mut requests) {
                    centered(g, r, it.glyph, icons, color);
                }
                if let Some(used) = it.usage {
                    let bar = Rect::new(r.x + s8, r.bottom() - self.s(5.0), (r.w - 2.0 * s8).max(0.0), self.s(3.0));
                    self.draw_usage(g, t, bar, used);
                }
                continue;
            }
            let text_h = self.s(ROW_H);
            let (icon_r, text_x) = (item_icon(r, self.scale, it.usage.is_some()), item_parts(r, self.scale).1);
            if !self.place_icon(g, it, icon_r, &mut requests) {
                centered(g, icon_r, it.glyph, icons, color);
            }
            let baseline = (r.y + (text_h + m.ascent - m.descent) / 2.0).round();
            let pin_r = sb.pin_rect(Rect::new(r.x, r.y, r.w, text_h), self.scale);
            let trash = hovered && sb.removable(i);
            let text_right = if trash { pin_r.x - s4 } else { r.right() - s8 };
            let mark = if lit { ink } else { t.accent };
            let w = Paint::new(g, t, self.scale).marked((text_x, baseline), &it.label, &it.marks, font, (ink, mark), text_right - text_x);
            if sb.prefs.quick.show_paths && !it.path.is_empty() {
                let gap = self.s(PATH_GAP);
                let x = text_x + w + gap;
                if x < text_right {
                    let cw = g.text(x, baseline - self.s(1.0), G_COLLAPSED, chevron, if lit { ink } else { t.text_dim }, text_right - x);
                    let px = x + cw + gap;
                    g.text(px, baseline, &it.path, small, if lit { ink.alpha(0xC0) } else { t.text_dim }, text_right - px);
                }
            }
            if let Some(used) = it.usage {
                let bar = self.drive_bar(r);
                self.draw_usage(g, t, bar, used);
            }
            if trash {
                let open = self.confirm.as_ref().is_some_and(|c| c.anchor == pin_r);
                Paint::new(g, t, self.scale).icon_button(pin_r, G_TRASH, pin_font, Button::of(self.hover == Hit::SidePin(i), open, true), ink);
            }
        }
        if let Drag::PinMove { to, .. } = self.drag {
            let first = sb.first_bookmark();
            let n = sb.state.pinned.len().min(sb.rects.len().saturating_sub(first));
            let before = sb.rects.get(first + to).filter(|_| to < n).map(|r| r.y);
            let y = before.or_else(|| sb.rects.get((first + n).saturating_sub(1)).map(|r| r.bottom())).unwrap_or(list.y);
            g.dl.rect(Rect::new(area.x + s8, y - 1.0, area.w - 2.0 * s8, self.s(2.0)), t.accent);
        }
        g.dl.set_clip(None);
        let r = sb.settings;
        g.dl.rect(Rect::new(area.x + s8, r.y - self.s(4.0), area.w - 2.0 * s8, 1.0), t.separator);
        let open = self.settings_open();
        let hot = self.hover == Hit::SideSettings && !open;
        if open || hot {
            g.dl.round_rect(r, if open { t.accent_fill } else { t.teal }, s4);
        }
        let ink = if open { ink_on(t.accent_fill) } else if hot { ink_on(t.teal) } else { t.text_dim };
        if compact {
            centered(g, r, G_SETTINGS, icons, ink);
        } else {
            centered(g, Rect::new(r.x + s4, r.y, self.s(28.0), r.h), G_SETTINGS, icons, ink);
            let baseline = (r.y + (r.h + m.ascent - m.descent) / 2.0).round();
            g.text(r.x + s36, baseline, S::MenuSettings.t(), font, if open || hot { ink } else { t.text }, r.w - s36);
        }
        self.request_icons(requests);
    }

    fn draw_usage<B: TextBackend>(&self, g: &mut Gfx<B>, t: &Theme, bar: Rect, used: f32) {
        g.dl.round_rect(bar, t.separator, bar.h / 2.0);
        let fill = if used >= 0.9 { t.error } else { t.accent };
        g.dl.round_rect(Rect::new(bar.x, bar.y, (bar.w * used.clamp(0.0, 1.0)).max(bar.h), bar.h), fill, bar.h / 2.0);
    }

    fn header_font(&self) -> FontStyle {
        FontStyle::new(FontFamily::Ui, 13.0 * self.scale, true)
    }

    fn drive_bar(&self, r: Rect) -> Rect {
        let (_, x) = item_parts(r, self.scale);
        let avail = (r.right() - self.s(12.0) - x).max(0.0);
        Rect::new(x, r.y + self.s(ROW_H) - self.s(2.0), self.s(self.nav.sidebar.bar_w).min(avail), self.s(4.0))
    }

    pub(super) fn fit_sidebar<B: TextBackend>(&mut self, g: &mut Gfx<B>) {
        let font = self.ui_font(13.0);
        let sb = &mut self.nav.sidebar;
        let widest = sb.items.iter().filter(|it| it.usage.is_some()).map(|it| g.measure(&it.label, font)).fold(0.0, f32::max);
        let bar_w = if widest > 0.0 { (widest / self.scale).ceil() + BAR_MARGIN } else { 0.0 };
        sb.bar_w = bar_w;
        let min = sb.min_w();
        if sb.state.sidebar_w < min {
            sb.state.sidebar_w = min;
            self.layout();
        }
    }

    fn place_icon<B: TextBackend>(&self, g: &mut Gfx<B>, it: &Item, r: Rect, requests: &mut Vec<IconRequest>) -> bool {
        let px = self.s(PLACE_ICON);
        self.system_icon(g, place_icon_request(it, &self.nav.sidebar.places, px as u32), r, px, 0xFF, requests)
    }
}

fn header_parts(r: Rect, ascent: f32, descent: f32, scale: f32) -> (Rect, f32, f32, Rect) {
    let s = |v: f32| (v * scale).round();
    let baseline = (r.y + (r.h + ascent - descent) / 2.0).round();
    let icon = Rect::new(r.x + s(4.0), r.y, s(20.0), r.h);
    (icon, r.x + s(HEADER_TITLE_X), baseline, Rect::new(r.right() - s(12.0) - s(8.0), r.y, s(12.0) + s(4.0), r.h))
}

fn item_parts(r: Rect, scale: f32) -> (Rect, f32) {
    let s = |v: f32| (v * scale).round();
    let icon = Rect::new(r.x + s(ITEM_INDENT) + s(4.0), r.y, s(28.0), s(ROW_H));
    (icon, icon.right() + s(4.0))
}

fn item_icon(r: Rect, scale: f32, drive: bool) -> Rect {
    let icon = item_parts(r, scale).0;
    if drive { Rect::new(icon.x, r.y, icon.w, r.h) } else { icon }
}

pub(super) fn place_icon_request(it: &Item, places: &Places, px: u32) -> Option<SystemIcon> {
    match (&it.loc, it.section) {
        (_, Section::Tags) | (Location::Wsl { .. }, Section::Recents) => None,
        (Location::Local(path), Section::Recents) => places.recent.iter().find(|r| r.target == *path).and_then(|r| recent_icon(r, places, px)),
        (loc, _) => location_icon(loc, places, px),
    }
}

pub(super) type SystemIcon = (u64, IconKind, FsPath);

pub(super) fn location_icon(loc: &Location, places: &Places, px: u32) -> Option<SystemIcon> {
    let place = |path: &FsPath, kind| Some((hash::hash(&("place", path.as_bytes(), px)), kind, path.clone()));
    match loc {
        _ if loc.is_home_page() => None,
        Location::Wsl { distro, path } if path.is_root() => Some((hash::hash(&("vol", distro.as_bytes(), px)), IconKind::Volume, loc.to_windows()?)),
        Location::Wsl { .. } => Some((hash::hash(&("dir", px)), IconKind::Folder, loc.to_windows()?)),
        Location::Local(path) => match places.drives.iter().find(|d| d.path == *path) {
            Some(d) if d.offline() => None,
            Some(_) => place(path, IconKind::Volume),
            None if matches!(path.kind(), PathKind::Drive | PathKind::Unix) && path.is_root() => place(path, IconKind::Volume),
            None => place(path, IconKind::Place),
        },
    }
}

pub(super) fn recent_icon(r: &Recent, places: &Places, px: u32) -> Option<SystemIcon> {
    match Location::from_local(r.target.clone()) {
        Location::Wsl { .. } => None,
        _ if !r.dir => Some((hash::hash(&("place", r.target.as_bytes(), px)), IconKind::File, r.target.clone())),
        loc => location_icon(&loc, places, px),
    }
}

pub(super) fn folder_icon(loc: &Location, sb: &Sidebar, px: u32) -> Option<SystemIcon> {
    let path = loc.path();
    let special = loc.is_root()
        || matches!(path.kind(), PathKind::Computer | PathKind::RecycleBin | PathKind::Home | PathKind::Shell)
        || library::is_library(path)
        || sb.is_pinned(loc)
        || sb.places.folders.iter().any(|(_, l)| l == loc);
    if special {
        return location_icon(loc, &sb.places, px);
    }
    Some((hash::hash(&("dir", px)), IconKind::Folder, loc.to_windows()?))
}

#[cfg(test)]
#[path = "../../test/ui/sidebar.rs"]
mod tests;
