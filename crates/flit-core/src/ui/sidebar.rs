mod places_menu;
mod view;

use super::confirm::Confirmed;
use super::edit::EditAction;
use super::field::{Field, FieldKey};
use super::fuzzy::fuzzy;
use super::home::{Recent, Widget};
use super::menu::Menu;
use super::tags::G_TAG;
use super::widgets::{eased, Paint};
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
pub(super) const ROW_H: f32 = 24.0;
const ROW_GAP: f32 = 3.0;
const ROW_ICON: f32 = 20.0;
const COMPACT_ROW_H: f32 = 32.0;
const DRIVE_H: f32 = 28.0;
const HEADER_H: f32 = 26.0;
const COMPACT_HEADER_H: f32 = 12.0;
const PIN_W: f32 = 22.0;
const SECTION_GAP: f32 = 18.0;
const ITEM_INDENT: f32 = 18.0;
pub(super) const PLACE_ICON: f32 = ROW_ICON;
const HEADER_TITLE_X: f32 = 28.0;
const BAR_MARGIN: f32 = 12.0;
const MAX_LIST: usize = 256;
const RECENT_MAX: usize = 10;
const MAX_VISITS: usize = 64;
pub(super) const MOST_VISITED: usize = 5;
const PATH_GAP: f32 = 6.0;
pub(super) const FOLD: std::time::Duration = std::time::Duration::from_millis(90);

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
#[cfg(windows)] const G_CLOUD: &str = "\u{E753}";
#[cfg(not(windows))] const G_CLOUD: &str = G_FOLDER;

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
    OneDrive,
}

impl KnownFolder {
    pub const ALL: [KnownFolder; 9] = [Self::Home, Self::Desktop, Self::Downloads, Self::Documents, Self::Pictures, Self::Music, Self::Videos, Self::Favorites, Self::OneDrive];
    const PLACES: [KnownFolder; 8] = [Self::Home, Self::OneDrive, Self::Desktop, Self::Downloads, Self::Documents, Self::Music, Self::Pictures, Self::Videos];

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
            Self::OneDrive => S::SidebarFolderOneDrive.t(),
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
            Self::OneDrive => "shell:OneDrive",
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
            Self::OneDrive => G_CLOUD,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Drive {
    pub path: FsPath,
    pub label: String,
    pub total: u64,
    pub free: u64,
    #[serde(default)]
    pub file_system: String,
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

    pub fn volume_label(&self) -> Option<String> {
        if self.remote.is_some() || self.unmounted || self.path.kind() != PathKind::Drive || !self.path.is_root() {
            return None;
        }
        if self.kind.is_some() {
            return Some(String::new());
        }
        let letter = char::from(*self.path.as_bytes().first()?);
        let suffix = crate::tr_en!(S::SidebarDriveLabel, name = "", letter = letter);
        Some(self.label.strip_suffix(suffix.as_str()).unwrap_or(&self.label).to_owned())
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
    pub never_shown: Vec<String>,
    #[serde(skip)]
    pub business_onedrive: Vec<(String, Location)>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NavState {
    pub pinned: Vec<Location>,
    pub sidebar_w: f32,
    pub sidebar: bool,
    #[serde(skip)]
    pub files: Vec<Location>,
}

impl Default for NavState {
    fn default() -> Self {
        Self { pinned: Vec::new(), sidebar_w: 220.0, sidebar: true, files: Vec::new() }
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
    #[serde(skip)]
    pub saved_servers: Vec<SavedServer>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedServer {
    pub address: String,
    pub name: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct QuickPrefs {
    pub order: Vec<String>,
    #[serde(skip)]
    pub places: Vec<Location>,
    #[serde(skip)]
    pub places_hidden: Vec<Location>,
    pub show_paths: bool,
    pub visits: Vec<(Location, u32)>,
}

impl NavPrefs {
    pub fn sanitize_servers(&mut self) {
        let mut seen = std::collections::HashSet::new();
        self.saved_servers.retain_mut(|server| {
            let Some(address) = super::saved_address(&server.address) else { return false };
            server.address = address;
            seen.insert(server.address.clone())
        });
    }

    pub fn sanitized(mut self) -> Self {
        for v in [&mut self.collapsed, &mut self.hidden, &mut self.widgets_off, &mut self.quick.order] {
            v.truncate(MAX_LIST);
        }
        self.quick.places.truncate(MAX_LIST);
        self.quick.places_hidden.truncate(MAX_LIST);
        self.sanitize_servers();
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
    MovePlace(Location, isize),
    HidePlace(Location),
    RestorePlaces,
    Eject(FsPath),
    Mount(FsPath),
    Label(FsPath),
    Properties(FsPath),
    FileProperties(FsPath),
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

#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) struct Fold {
    pub section: Section,
    pub opening: bool,
    pub since: Instant,
}

impl Fold {
    pub fn shown(&self, now: Instant) -> f32 {
        let e = eased(self.since, now, FOLD);
        if self.opening { e } else { 1.0 - e }
    }
}

pub(super) struct Leaving {
    pub index: usize,
    pub h: f32,
    pub label: String,
    pub glyph: &'static str,
    pub since: Instant,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Stop {
    Item(usize),
    Head(Section),
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
    pub rect: Rect,
    pub list: Rect,
    pub splitter: Rect,
    pub bar: Rect,
    pub field: Rect,
    pub clear: Rect,
    pub filter: Field,
    pub typing: bool,
    pub(super) scroll: super::Glide,
    pub(super) content_h: f32,
    pub cursor: usize,
    pub head: Option<Section>,
    pub press_y: f32,
    pub dragging_out: bool,
    pub slide: Option<super::widgets::Tween>,
    pub fold: Option<Fold>,
    pub fold_clip: Option<(Section, Rect)>,
    pub leaving: Vec<Leaving>,
    pub gaps: Vec<(Rect, usize)>,
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
            Section::Recents => self.recent().take(RECENT_MAX).map(|(_, r)| item(crate::redact::passwords(&r.name).into_owned(), if r.dir { G_FOLDER } else { G_FILE }, Location::from_local(r.target.clone()), None)).collect(),
            Section::Bookmarks => {
                let library_name = |l: &Location| p.libraries.iter().find(|lib| lib.path == *l.path()).map(|lib| lib.name.clone());
                let glyph = |l: &Location| if self.state.files.contains(l) { G_FILE } else { pinned_glyph(l) };
                self.state.pinned.iter().map(|l| item(library_name(l).unwrap_or_else(|| pinned_label(l)), glyph(l), l.clone(), None)).collect()
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
                    if k == KnownFolder::OneDrive {
                        for (name, loc) in &p.business_onedrive {
                            let brand = S::SidebarFolderOneDrive.en();
                            let label = if name.get(..brand.len()).is_some_and(|s| s.eq_ignore_ascii_case(brand)) {
                                name.clone()
                            } else {
                                crate::tr!(S::WindowsOneDriveBusiness, name = name)
                            };
                            out.push(item(label, G_CLOUD, loc.clone(), None));
                        }
                    }
                }
                if p.recycle_bin {
                    out.push(item(S::PathRecycleBin.t().to_owned(), G_RECYCLE_BIN, Location::recycle_bin(), None));
                }
                if let Some(l) = self.known(KnownFolder::Favorites) {
                    out.push(item(KnownFolder::Favorites.label().to_owned(), G_PIN, l.clone(), None));
                }
                out.push(item(S::PathHome.t().to_owned(), G_HOME, Location::home_page(), None));
                let order = &self.prefs.quick.places;
                out.sort_by_key(|item| order.iter().position(|loc| loc == &item.loc).unwrap_or(usize::MAX));
                out.retain(|it| !self.prefs.quick.places_hidden.contains(&it.loc));
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
            if self.is_hidden(s) || (query.is_empty() && self.is_collapsed(s) && !self.fold.is_some_and(|f| f.section == s && !f.opening)) {
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
        if self.head.is_some_and(|s| !self.folded(s) || self.is_hidden(s)) {
            self.head = None;
        }
    }

    fn folded(&self, s: Section) -> bool {
        self.is_collapsed(s) && !self.filtering()
    }

    pub(super) fn point(&mut self, i: usize) {
        self.cursor = i;
        self.head = None;
    }

    fn stops(&self) -> Vec<Stop> {
        let mut out = Vec::new();
        for s in self.shown_sections() {
            match self.folded(s) {
                true => out.push(Stop::Head(s)),
                false => out.extend(self.items.iter().enumerate().filter(|(_, it)| it.section == s).map(|(i, _)| Stop::Item(i))),
            }
        }
        out
    }

    fn stop(&self) -> Stop {
        self.head.map_or(Stop::Item(self.cursor), Stop::Head)
    }

    fn stop_rect(&self, stop: Stop) -> Option<Rect> {
        match stop {
            Stop::Item(i) => self.rects.get(i).copied(),
            Stop::Head(s) => self.headers.iter().find(|(_, h)| *h == s).map(|(r, _)| *r),
        }
    }

    fn go_to(&mut self, stop: Stop) {
        match stop {
            Stop::Item(i) => self.point(i),
            Stop::Head(s) => self.head = Some(s),
        }
    }

    fn page(&self, stops: &[Stop], at: usize, down: bool) -> usize {
        let Some(from) = stops.get(at).and_then(|&s| self.stop_rect(s)) else { return at };
        let reach = (self.list.h - from.h).max(0.0);
        let within = |k: &usize| self.stop_rect(stops[*k]).is_some_and(|r| (r.y - from.y).abs() <= reach);
        let far = match down {
            true => (at + 1..stops.len()).take_while(within).last(),
            false => (0..at).rev().take_while(within).last(),
        };
        let next = if down { (at + 1).min(stops.len().saturating_sub(1)) } else { at.saturating_sub(1) };
        far.unwrap_or(next)
    }

    pub(super) fn filtering(&self) -> bool {
        !self.filter.text().trim().is_empty()
    }

    pub(super) fn shown_sections(&self) -> Vec<Section> {
        self.prefs.order().into_iter().filter(|&s| !self.is_hidden(s) && self.available(s) && (!self.filtering() || self.items.iter().any(|it| it.section == s))).collect()
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

    fn first_place(&self) -> usize {
        self.items.iter().position(|it| it.section == Section::Places).unwrap_or(0)
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

    pub(super) fn available(&self, s: Section) -> bool {
        let p = &self.places;
        match s {
            Section::Recents | Section::Bookmarks | Section::Places => true,
            Section::Libraries => !p.libraries.is_empty(),
            Section::Storage => !p.drives.is_empty(),
            Section::Wsl => !p.wsl.is_empty(),
            Section::Tags => true,
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
    pub(super) fn check_places(&mut self) {
        let paths: Vec<FsPath> = self.nav.sidebar.state.pinned.iter().filter_map(Location::to_windows).collect();
        if !paths.is_empty() {
            self.effects.push(Effect::CheckPlaces(paths));
        }
    }

    pub fn forget_missing(&mut self, gone: &[FsPath]) {
        let missing = |l: &Location| l.to_windows().is_some_and(|p| gone.iter().any(|g| g.same(&p)));
        let sb = &mut self.nav.sidebar;
        if !sb.state.pinned.iter().any(missing) {
            return;
        }
        let now = Instant::now();
        if self.animations && !sb.filtering() {
            let mut kept = 0;
            for (i, it) in sb.items.iter().enumerate().filter(|(_, it)| it.section == Section::Bookmarks) {
                if !missing(&it.loc) {
                    kept += 1;
                    continue;
                }
                let h = sb.rects.get(i).map_or(0.0, |r| r.h);
                let first = sb.items.iter().position(|it| it.section == Section::Bookmarks).unwrap_or(0);
                sb.leaving.push(Leaving { index: first + kept, h, label: it.label.clone(), glyph: it.glyph, since: now });
            }
        }
        sb.state.pinned.retain(|l| !missing(l));
        self.pins_changed();
    }

    pub fn set_places(&mut self, places: Arc<Places>) {
        self.check_places();
        let old = std::mem::replace(&mut self.nav.sidebar.places, places);
        self.nav.sidebar.rebuild();
        self.places_changed(&old, Instant::now());
        if self.settings.opts.view_rules.location_rules() {
            self.refresh_view_rules();
        }
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

    pub fn set_pinned_files(&mut self, files: Vec<Location>) {
        if self.nav.sidebar.state.files != files {
            self.nav.sidebar.state.files = files;
            self.nav.sidebar.rebuild();
            self.dirty = true;
        }
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
        let prefs = prefs.sanitized();
        let servers_changed = self.nav.sidebar.prefs.saved_servers != prefs.saved_servers;
        self.nav.sidebar.prefs = prefs;
        if servers_changed {
            self.refresh_saved_servers();
        }
        self.nav_prefs_changed();
    }

    pub(super) fn nav_prefs_changed(&mut self) {
        self.nav.sidebar.rebuild();
        self.layout();
        self.dirty = true;
    }

    pub(super) fn sidebar_width(&self) -> f32 {
        let sb = &self.nav.sidebar;
        let full = if sb.prefs.compact { self.s(COMPACT_W) } else { self.s(sb.state.sidebar_w).min((self.size.0 / 2.0).round()) };
        match sb.slide {
            Some(tw) => (full * tw.value(self.clock(), FOLD)).round(),
            None if sb.state.sidebar => full,
            None => 0.0,
        }
    }

    pub(super) fn toggle_sidebar(&mut self) {
        let now = self.clock();
        let sb = &mut self.nav.sidebar;
        let mut slide = sb.slide.unwrap_or(super::widgets::Tween::at(if sb.state.sidebar { 1.0 } else { 0.0 }));
        let moving = slide.aim(if sb.state.sidebar { 0.0 } else { 1.0 }, self.animations, now, FOLD);
        sb.slide = moving.then_some(slide);
        let st = &mut sb.state;
        st.sidebar = !st.sidebar;
        if !st.sidebar {
            sb.filter.edit.primary_id = 0;
        }
        if !st.sidebar && self.focus == Focus::Sidebar {
            self.focus = Focus::List;
        }
        self.layout();
        self.dirty = true;
    }

    pub(super) fn toggle_compact(&mut self) {
        let p = &mut self.nav.sidebar.prefs;
        p.compact = !p.compact;
        if p.compact {
            self.nav.sidebar.filter.edit.primary_id = 0;
        }
        self.nav_prefs_changed();
    }

    pub(super) fn toggle_section(&mut self, s: Section) {
        let now = self.clock();
        let sb = &mut self.nav.sidebar;
        sb.fold = (self.animations && !sb.filtering()).then(|| Fold { section: s, opening: sb.is_collapsed(s), since: now });
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
        self.effects.push(Effect::PinnedCleared);
        self.refresh_pins();
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
        sb.point(0);
        self.focus = Focus::Sidebar;
        self.layout();
        self.dirty = true;
    }

    fn quick_filter_changed(&mut self) {
        let sb = &mut self.nav.sidebar;
        sb.scroll.snap(0.0);
        sb.point(0);
        self.nav_prefs_changed();
    }

    pub(super) fn quick_insert(&mut self, s: &str) -> bool {
        let sb = &mut self.nav.sidebar;
        if !sb.typing {
            return false;
        }
        sb.filter.insert(s);
        self.quick_filter_changed();
        true
    }

    pub(super) fn clear_quick_filter(&mut self) {
        let sb = &mut self.nav.sidebar;
        sb.filter.edit.set("");
        sb.filter.edit.primary_id = 0;
        sb.typing = false;
        if self.focus == Focus::Sidebar {
            self.focus = Focus::List;
        }
        self.quick_filter_changed();
    }

    pub(super) fn quick_filter_click(&mut self, x: f32, clicks: u8, extend: bool) {
        self.nav.sidebar.filter.click(x, clicks, extend);
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
        if !loc.is_home_page() && self.settings.opts.recents != super::options::Recents::Never {
            self.nav.sidebar.prefs.count_visit(loc);
        }
    }

    pub(super) fn cycle_sidebar_focus(&mut self) {
        if self.focus == Focus::List && self.tabs.panels.list.len() > 1 {
            let order = self.order();
            let at = order.iter().position(|&id| id == self.tabs.panels.active).unwrap_or(0);
            let next = order[(at + 1) % order.len()];
            let from = self.tabs.panels.cycle_from.replace(self.tabs.panels.active);
            if from != Some(next) {
                return self.activate_panel(next);
            }
            self.tabs.panels.cycle_from = None;
        }
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
                sb.point(i);
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
        self.pin_entries(locs, Vec::new());
    }

    pub fn pin_entries(&mut self, folders: Vec<Location>, files: Vec<Location>) {
        let state = &mut self.nav.sidebar.state;
        let before = state.pinned.len();
        for l in folders.into_iter().chain(files.iter().cloned()) {
            if !state.pinned.contains(&l) {
                state.pinned.push(l);
            }
        }
        for f in files {
            if !state.files.contains(&f) {
                state.files.push(f);
            }
        }
        let pinned = &state.pinned;
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
        self.effects.push(Effect::Pinned(self.nav.sidebar.state.pinned.clone()));
        self.refresh_pins();
    }

    fn refresh_pins(&mut self) {
        let state = &mut self.nav.sidebar.state;
        let pinned = &state.pinned;
        state.files.retain(|f| pinned.contains(f));
        let files = state.files.clone();
        self.nav.sidebar.rebuild();
        self.effects.push(Effect::PinnedFiles(files));
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
        if (y - sb.press_y).abs() < self.s(5.0) {
            return false;
        }
        if sb.filtering() {
            return false;
        }
        match sb.items.get(i).map(|it| it.section) {
            Some(Section::Bookmarks) => {
                self.drag_pin(i - sb.first_bookmark(), y);
                true
            }
            Some(Section::Places) => {
                self.drag_place(i - sb.first_place(), y);
                true
            }
            _ => self.drag_out_of_quick_access(i),
        }
    }

    pub(super) fn step_sidebar(&mut self, now: Instant) -> bool {
        let sb = &mut self.nav.sidebar;
        if sb.fold.is_some_and(|f| now.saturating_duration_since(f.since) >= FOLD) {
            sb.fold = None;
            sb.rebuild();
        }
        sb.leaving.retain(|l| now.saturating_duration_since(l.since) < FOLD);
        if sb.slide.as_mut().is_some_and(|tw| !tw.aim(tw.target(), true, now, FOLD)) {
            sb.slide = None;
        }
        sb.fold.is_some() || !sb.leaving.is_empty() || sb.slide.is_some()
    }

    fn drag_out_of_quick_access(&mut self, i: usize) -> bool {
        let sb = &self.nav.sidebar;
        let Some(it) = sb.items.get(i).filter(|it| it.section != Section::Tags) else { return false };
        let Some(path) = sb.recent_file(i).or_else(|| it.loc.to_windows()) else { return false };
        self.nav.sidebar.dragging_out = true;
        self.pressed = Hit::None;
        self.hover = Hit::None;
        self.effects.push(Effect::StartDrag(vec![path]));
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

    pub(super) fn drag_place(&mut self, from: usize, y: f32) {
        let sb = &self.nav.sidebar;
        let to = sb.items.iter().enumerate().filter(|(i, it)| it.section == Section::Places && sb.rects.get(*i).is_some_and(|r| r.y + r.h / 2.0 < y)).count();
        self.drag = Drag::PlaceMove { from, to };
        self.dirty = true;
    }

    pub(super) fn drag_place_out(&mut self, from: usize) -> bool {
        self.drag_out_of_quick_access(self.nav.sidebar.first_place() + from)
    }

    pub(super) fn drop_place(&mut self, from: usize, to: usize) {
        let mut places: Vec<Location> = self.nav.sidebar.section_items(Section::Places).into_iter().map(|it| it.loc).collect();
        let target = if to > from { to - 1 } else { to };
        if from >= places.len() || target == from {
            return;
        }
        let item = places.remove(from);
        places.insert(target.min(places.len()), item);
        self.nav.sidebar.prefs.quick.places = places;
        self.nav_prefs_changed();
    }

    pub(super) fn open_side_item(&mut self, i: usize, new_tab: bool, now: Instant) {
        if let Some(k) = self.nav.sidebar.tag_of(i) {
            return self.show_tag(k);
        }
        if let Some(file) = self.nav.sidebar.recent_file(i) {
            return self.place_command(PlaceCmd::OpenFile(file), now);
        }
        if let Some(it) = self.nav.sidebar.items.get(i).filter(|it| it.section == Section::Bookmarks && it.glyph == G_FILE) {
            let file = it.loc.path().clone();
            return self.place_command(PlaceCmd::OpenFile(file), now);
        }
        let Some(folder) = self.nav.sidebar.items.get(i).map(|it| it.loc.clone()) else { return };
        if self.mount_drive(folder.path()) {
            return;
        }
        match new_tab {
            true => self.open_in_tabs(vec![Route { folder, select: None }], now),
            false => self.dispatch(vec![Route { folder, select: None }], OpenTarget::CurrentTab, now),
        }
    }

    pub(super) fn known_folder(&self, k: KnownFolder) -> Option<Location> {
        self.nav.sidebar.places.folders.iter().find(|(f, _)| *f == k).map(|(_, l)| l.clone())
    }
}

fn clip_to(outer: Rect, inner: Rect) -> Rect {
    let (x, y) = (outer.x.max(inner.x), outer.y.max(inner.y));
    Rect::new(x, y, (outer.right().min(inner.right()) - x).max(0.0), (outer.bottom().min(inner.bottom()) - y).max(0.0))
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
        #[cfg(windows)]
        (_, Section::Places) if it.glyph == G_CLOUD => {
            let loc = places.folders.iter().find(|(k, _)| *k == KnownFolder::OneDrive).map_or(&it.loc, |(_, personal)| personal);
            location_icon(loc, places, px)
        }
        (_, Section::Tags) | (Location::Wsl { .. }, Section::Recents) => None,
        (Location::Local(path), Section::Recents) => places.recent.iter().find(|r| r.target == *path).and_then(|r| recent_icon(r, places, px)),
        (Location::Local(path), Section::Bookmarks) if it.glyph == G_FILE => Some((hash::hash(&("place", path.as_bytes(), px)), IconKind::File, path.clone())),
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
        Location::Admin(_) => None,
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
