use flit_core::archive::Defaults;
use flit_core::sort::{GroupField, GroupSpec, SortField, SortSpec};
use flit_core::ui::{Column, FolderView, FolderViews, LayoutMode, NavPrefs, NavState, PaneState, Persisted, QuickPrefs, SavedServer, Sizes, ViewPrefs, EXTRA, MAX_HISTORY, ORDERED};
use flit_core::Location;
use flit_platform::{Placement, ProfilePlatform, ShellPlatform};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const VERSION: u32 = 2;
const STAGE_ONE_COLUMNS: [Column; 3] = [Column::Modified, Column::Type, Column::Size];

type Bounds = (i32, i32, i32, i32, bool);

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq)]
enum SplitDir {
    #[default]
    Vertical,
    Horizontal,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq)]
struct SplitPrefs {
    dual: bool,
    dir: SplitDir,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
struct Head {
    version: u32,
    path: Location,
    sort: SortField,
    descending: bool,
    show_hidden: bool,
    col_w: [f32; 3],
    placement: Option<Bounds>,
}

#[derive(Serialize, Deserialize)]
struct ViewRecord {
    view: FolderView,
    prefs: ViewPrefs,
    folders: Vec<(u64, FolderView)>,
}

type Look = (LayoutMode, Sizes);
type GitLook = (u8, [f32; 4]);

#[derive(Serialize, Deserialize)]
struct QuickRecord {
    quick: QuickPrefs,
    deep: Vec<u64>,
}

#[derive(Serialize, Deserialize)]
struct IntegrationRecord {
    git: bool,
    places: Vec<Location>,
    places_hidden: Vec<Location>,
}

#[derive(Serialize, Deserialize)]
struct GitRecord {
    view: GitLook,
    defaults: GitLook,
    folders: Vec<GitLook>,
}

#[derive(Serialize, Deserialize)]
struct GitHashRecord {
    view: f32,
    defaults: f32,
    folders: Vec<(u64, f32)>,
}

fn hash_width(v: &FolderView) -> f32 {
    v.columns.width(Column::GitHash)
}

fn git_look(v: &FolderView) -> GitLook {
    let mut widths = [0.0; 4];
    widths.copy_from_slice(&v.columns.git_widths[..4]);
    (v.columns.git, widths)
}

fn with_git(mut v: FolderView, (git, widths): GitLook) -> FolderView {
    v.columns.git = git & 0x1F;
    v.columns.git_widths[..4].copy_from_slice(&widths);
    v
}

type Zoomed = (u8, bool, u8, [f32; 7], [u8; 15]);

#[derive(Serialize, Deserialize)]
struct ZoomRecord {
    view: Zoomed,
    defaults: Zoomed,
    folders: Vec<Zoomed>,
    retain_filter: bool,
}

fn zoomed(v: &FolderView) -> Zoomed {
    let c = &v.columns;
    let mut widths = [0.0; 7];
    widths.copy_from_slice(&c.extra_widths[..7]);
    let mut order = [0; 15];
    for (slot, id) in order.iter_mut().zip(c.order.iter().filter(|&&id| id != Column::Files as u8 && id != Column::Attributes as u8 && attributed_id(id))) {
        *slot = *id;
    }
    (v.zoom, v.subfolders, c.extra as u8, widths, order)
}

fn with_zoom(mut v: FolderView, (zoom, subfolders, extra, widths, order): Zoomed) -> FolderView {
    (v.zoom, v.subfolders) = (zoom, subfolders);
    v.columns.extra = (v.columns.extra & !0xFF) | u32::from(extra);
    v.columns.extra_widths[..7].copy_from_slice(&widths);
    let folders = order.iter().position(|&id| id == Column::Folders as u8).map_or(order.len(), |k| k + 1);
    let (head, tail) = order.split_at(folders);
    for (slot, id) in v.columns.order.iter_mut().zip(head.iter().chain([&(Column::Files as u8)]).chain(tail)) {
        *slot = *id;
    }
    v.sanitized()
}

type Wide = ([f32; 8], [u8; 16]);

#[derive(Serialize, Deserialize)]
struct WideRecord {
    view: Wide,
    defaults: Wide,
    folders: Vec<Wide>,
}

fn wide(v: &FolderView) -> Wide {
    let (mut widths, mut order) = ([0.0; 8], [0; 16]);
    widths.copy_from_slice(&v.columns.extra_widths[..8]);
    for (slot, id) in order.iter_mut().zip(v.columns.order.iter().filter(|&&id| id != Column::Attributes as u8 && attributed_id(id))) {
        *slot = *id;
    }
    (widths, order)
}

const ATTRIBUTED_EXTRA: usize = 13;
const ATTRIBUTED_ORDER: usize = 21;
const ATTRIBUTED_MASK: u32 = (1 << ATTRIBUTED_EXTRA) - 1;

type Attributed = (u16, [f32; ATTRIBUTED_EXTRA], [u8; ATTRIBUTED_ORDER]);

#[derive(Serialize, Deserialize)]
struct AttributesRecord {
    view: Attributed,
    defaults: Attributed,
    folders: Vec<Attributed>,
}

fn attributed_id(id: u8) -> bool {
    usize::from(id) < ATTRIBUTED_ORDER
}

fn attributed(v: &FolderView) -> Attributed {
    let c = &v.columns;
    let (mut widths, mut order) = ([0.0; ATTRIBUTED_EXTRA], [0; ATTRIBUTED_ORDER]);
    widths.copy_from_slice(&c.extra_widths[..ATTRIBUTED_EXTRA]);
    for (slot, id) in order.iter_mut().zip(c.order.iter().filter(|&&id| attributed_id(id))) {
        *slot = *id;
    }
    ((c.extra & ATTRIBUTED_MASK) as u16, widths, order)
}

fn with_attributes(mut v: FolderView, (extra, widths, order): Attributed) -> FolderView {
    let c = &mut v.columns;
    c.extra = (c.extra & !ATTRIBUTED_MASK) | (u32::from(extra) & ATTRIBUTED_MASK);
    c.extra_widths[..ATTRIBUTED_EXTRA].copy_from_slice(&widths);
    for (slot, id) in c.order.iter_mut().zip(order.into_iter().chain(ATTRIBUTED_ORDER as u8..Column::GitStatus as u8).chain(Column::Rating as u8..=Column::PortableTags as u8)) {
        *slot = id;
    }
    v.sanitized()
}

type Shelled = (u32, Vec<f32>, Vec<u8>, u8, u8);

#[derive(Serialize, Deserialize)]
struct ShellRecord {
    view: Shelled,
    defaults: Shelled,
    folders: Vec<Shelled>,
}

fn shelled(v: &FolderView) -> Shelled {
    let c = &v.columns;
    let sort = SortField::ALL.iter().position(|&f| f == v.sort.field).unwrap_or(0) as u8;
    let group = GroupField::ALL.iter().position(|&f| f == v.group.field).unwrap_or(0) as u8;
    (c.extra, c.extra_widths.to_vec(), c.order.to_vec(), sort, group)
}

fn with_shell(mut v: FolderView, (extra, widths, order, sort, group): Shelled) -> FolderView {
    let c = &mut v.columns;
    c.extra = extra & ((1 << EXTRA) - 1);
    for (slot, w) in c.extra_widths.iter_mut().zip(widths) {
        *slot = w;
    }
    if order.len() == ORDERED {
        c.order.copy_from_slice(&order);
    } else if order.len() == Column::GitStatus as usize {
        c.order[..order.len()].copy_from_slice(&order);
    }
    if let Some(&f) = SortField::ALL.get(usize::from(sort)) {
        v.sort.field = f;
    }
    if let Some(&f) = GroupField::ALL.get(usize::from(group)) {
        v.group.field = f;
    }
    v.sanitized()
}

const FILES_COLUMN_CHOSEN: u8 = 1;

fn files_like_folders(mut v: FolderView) -> FolderView {
    if v.columns.shown(Column::Files) != v.columns.shown(Column::Folders) {
        v.columns.toggle(Column::Files);
    }
    v
}

fn with_wide(mut v: FolderView, (widths, order): Wide) -> FolderView {
    v.columns.extra_widths[..8].copy_from_slice(&widths);
    for (slot, id) in v.columns.order.iter_mut().zip(order.iter().chain([&(Column::Attributes as u8)])) {
        *slot = *id;
    }
    v.sanitized()
}

#[derive(Serialize, Deserialize)]
struct LayoutRecord {
    view: Look,
    defaults: Look,
    folders: Vec<Look>,
    thumbnails: bool,
    thumb_cache_mb: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Tools {
    toolbar: RetiredToolbar,
    pub searches: Vec<String>,
    pub streams: bool,
    #[serde(skip)]
    pub commands: flit_core::ui::CommandUsage,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
struct RetiredToolbar {
    visible: bool,
    sets: [Vec<RetiredTool>; 3],
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
enum RetiredTool {
    Button(String, RetiredShow),
    Separator,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
enum RetiredShow {
    Icon,
    Label,
    Both,
}

impl Tools {
    pub fn new(searches: Vec<String>) -> Self {
        Self { searches, ..Default::default() }
    }

    fn sanitized(mut self) -> Self {
        self.searches.truncate(MAX_HISTORY);
        self
    }
}

fn look(v: &FolderView) -> Look {
    (v.layout, v.sizes)
}

fn with_look(v: FolderView, (layout, sizes): Look) -> FolderView {
    FolderView { layout, sizes, ..v }
}

fn legacy(v: FolderView) -> FolderView {
    let sort = SortSpec { field: if v.sort.field.is_newer_than_0_7() { SortField::Name } else { v.sort.field }, ..v.sort };
    let group = GroupSpec { field: if v.group.field.is_shell() { GroupField::None } else { v.group.field }, ..v.group };
    FolderView { layout: LayoutMode::Details, sort, group, ..v }
}

#[derive(Clone, Debug)]
pub struct Saved {
    version: u32,
    pub persisted: Persisted,
    placement: Option<Bounds>,
    pub nav: NavState,
    pub pane: PaneState,
    pub side: NavPrefs,
    pub tools: Tools,
}

impl Saved {
    pub fn new(persisted: Persisted, placement: Option<Placement>) -> Self {
        Self { version: VERSION, persisted, placement: placement.map(|p| (p.x, p.y, p.width, p.height, p.maximized)), nav: NavState::default(), pane: PaneState::default(), side: NavPrefs::default(), tools: Tools::default() }
    }

    pub fn with_nav(mut self, nav: NavState) -> Self {
        self.nav = nav;
        self
    }

    pub fn with_pane(mut self, pane: PaneState) -> Self {
        self.pane = pane;
        self
    }

    pub fn with_side(mut self, side: NavPrefs) -> Self {
        self.side = side;
        self
    }

    pub fn with_tools(mut self, tools: Tools) -> Self {
        self.tools = tools;
        self
    }

    fn sanitized(mut self) -> Self {
        let p = &mut self.persisted;
        p.view = p.view.sanitized();
        p.prefs = p.prefs.clone().sanitized();
        self.placement = self.placement.filter(|&(_, _, w, h, _)| w > 0 && h > 0);
        self.nav = self.nav.sanitized();
        self.pane = self.pane.sanitized();
        self.side = self.side.sanitized();
        self.tools = self.tools.sanitized();
        self
    }

    fn head(&self) -> Head {
        let (p, v) = (&self.persisted, &self.persisted.view);
        let sort = if matches!(v.sort.field, SortField::Name | SortField::Modified | SortField::Type | SortField::Size | SortField::Origin) { v.sort.field } else { SortField::Name };
        Head { version: self.version, path: compatible_location(&p.path), sort, descending: v.sort.descending, show_hidden: p.show_hidden, col_w: STAGE_ONE_COLUMNS.map(|c| v.columns.width(c)), placement: self.placement }
    }

    fn from_head(h: Head) -> Self {
        let mut view = FolderView::default();
        (view.sort.field, view.sort.descending) = (h.sort, h.descending);
        for (c, w) in STAGE_ONE_COLUMNS.into_iter().zip(h.col_w) {
            view.columns.widths[c as usize] = w;
        }
        let persisted = Persisted { path: h.path, show_hidden: h.show_hidden, view, ..Default::default() };
        Self { version: h.version, persisted, placement: h.placement, nav: NavState::default(), pane: PaneState::default(), side: NavPrefs::default(), tools: Tools::default() }
    }

    fn decode(bytes: &[u8]) -> Option<Self> {
        let (head, tail) = postcard::take_from_bytes::<Head>(bytes).ok()?;
        let mut saved = Self::from_head(head);
        let Ok((nav, tail)) = postcard::take_from_bytes::<NavState>(tail) else { return Some(saved) };
        saved.nav = nav;
        let Ok((mut r, tail)) = postcard::take_from_bytes::<ViewRecord>(tail) else { return Some(saved) };
        let mut rest = None;
        if let Ok((l, tail)) = postcard::take_from_bytes::<LayoutRecord>(tail) {
            (r.view, r.prefs.defaults) = (with_look(r.view, l.view), with_look(r.prefs.defaults, l.defaults));
            (r.prefs.thumbnails, r.prefs.thumb_cache_mb) = (l.thumbnails, l.thumb_cache_mb);
            if l.folders.len() == r.folders.len() {
                r.folders.iter_mut().zip(l.folders).for_each(|((_, v), lk)| *v = with_look(*v, lk));
            }
            if let Ok((pane, tail)) = postcard::take_from_bytes::<PaneState>(tail) {
                saved.pane = pane;
                if let Ok((side, tail)) = postcard::take_from_bytes::<NavPrefs>(tail) {
                    saved.side = side;
                    rest = Some(tail);
                }
            }
        }
        let (mut deep, mut zoom, mut widened, mut files_chosen, mut attributes, mut shells, mut hashes) = (Vec::new(), None, None, false, None, None, None);
        let p = &mut saved.persisted;
        (p.view, p.prefs, p.folders) = (r.view, r.prefs, FolderViews::from_entries(r.folders));
        p.prefs.count_folders = p.prefs.auto_sizes;
        p.prefs.count_files = p.prefs.auto_sizes;
        if let Some((archive, tail)) = rest.and_then(|t| postcard::take_from_bytes::<Defaults>(t).ok()) {
            p.archive = archive.sanitized();
            if let Ok((tools, tail)) = postcard::take_from_bytes::<Tools>(tail) {
                p.prefs.show_streams = tools.streams;
                saved.tools = Tools { toolbar: RetiredToolbar::default(), ..tools };
                if let Ok((_, tail)) = postcard::take_from_bytes::<SplitPrefs>(tail) {
                    if let Ok((word, tail)) = postcard::take_from_bytes::<u32>(tail) {
                        p.archive.compression.word = word;
                        p.archive = std::mem::take(&mut p.archive).sanitized();
                        if let Ok((g, tail)) = postcard::take_from_bytes::<GitRecord>(tail) {
                            if let Ok((descriptions, tail)) = postcard::take_from_bytes::<bool>(tail) {
                                p.prefs.command_descriptions = descriptions;
                                if let Ok((q, tail)) = postcard::take_from_bytes::<QuickRecord>(tail) {
                                    saved.side.quick = q.quick;
                                    deep = q.deep;
                                    if let Ok((z, tail)) = postcard::take_from_bytes::<ZoomRecord>(tail) {
                                        zoom = Some(z);
                                        if let Ok((files, tail)) = postcard::take_from_bytes::<Vec<Location>>(tail) {
                                            saved.nav.files = files;
                                            if let Ok((w, tail)) = postcard::take_from_bytes::<WideRecord>(tail) {
                                                widened = Some(w);
                                                if let Ok((v, tail)) = postcard::take_from_bytes::<u8>(tail) {
                                                    files_chosen = v >= FILES_COLUMN_CHOSEN;
                                                    if let Ok((r, tail)) = postcard::take_from_bytes::<IntegrationRecord>(tail) {
                                                        p.prefs.git_integration = r.git;
                                                        saved.side.quick.places = r.places;
                                                        saved.side.quick.places_hidden = r.places_hidden;
                                                        if let Ok(((folders, files), tail)) = postcard::take_from_bytes::<(bool, bool)>(tail) {
                                                            p.prefs.count_folders = folders;
                                                            p.prefs.count_files = files;
                                                            if let Ok((a, tail)) = postcard::take_from_bytes::<AttributesRecord>(tail) {
                                                                attributes = Some(a);
                                                                if let Ok((s, tail)) = postcard::take_from_bytes::<ShellRecord>(tail) {
                                                                    shells = Some(s);
                                                                    hashes = postcard::take_from_bytes::<GitHashRecord>(tail).ok();
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            (p.view, p.prefs.defaults) = (with_git(p.view, g.view), with_git(p.prefs.defaults, g.defaults));
                            let mut folders = p.folders.entries();
                            if folders.len() == g.folders.len() {
                                folders.iter_mut().zip(g.folders).for_each(|((_, v), gl)| *v = with_git(*v, gl));
                                p.folders = FolderViews::from_entries(folders);
                            }
                            if let Some(z) = zoom.take() {
                                (p.view, p.prefs.defaults, p.prefs.retain_filter) = (with_zoom(p.view, z.view), with_zoom(p.prefs.defaults, z.defaults), z.retain_filter);
                                let mut folders = p.folders.entries();
                                if folders.len() == z.folders.len() {
                                    folders.iter_mut().zip(z.folders).for_each(|((_, v), zl)| *v = with_zoom(*v, zl));
                                    p.folders = FolderViews::from_entries(folders);
                                }
                            }
                            if let Some(w) = widened.take() {
                                (p.view, p.prefs.defaults) = (with_wide(p.view, w.view), with_wide(p.prefs.defaults, w.defaults));
                                let mut folders = p.folders.entries();
                                if folders.len() == w.folders.len() {
                                    folders.iter_mut().zip(w.folders).for_each(|((_, v), wl)| *v = with_wide(*v, wl));
                                    p.folders = FolderViews::from_entries(folders);
                                }
                            }
                            if let Some(a) = attributes.take() {
                                (p.view, p.prefs.defaults) = (with_attributes(p.view, a.view), with_attributes(p.prefs.defaults, a.defaults));
                                let mut folders = p.folders.entries();
                                if folders.len() == a.folders.len() {
                                    folders.iter_mut().zip(a.folders).for_each(|((_, v), al)| *v = with_attributes(*v, al));
                                    p.folders = FolderViews::from_entries(folders);
                                }
                            }
                            if let Some(s) = shells.take() {
                                (p.view, p.prefs.defaults) = (with_shell(p.view, s.view), with_shell(p.prefs.defaults, s.defaults));
                                let mut folders = p.folders.entries();
                                if folders.len() == s.folders.len() {
                                    folders.iter_mut().zip(s.folders).for_each(|((_, v), sl)| *v = with_shell(*v, sl));
                                    p.folders = FolderViews::from_entries(folders);
                                }
                            }
                            if !files_chosen {
                                (p.view, p.prefs.defaults) = (files_like_folders(p.view), files_like_folders(p.prefs.defaults));
                                let folders = p.folders.entries().into_iter().map(|(k, v)| (k, files_like_folders(v))).collect();
                                p.folders = FolderViews::from_entries(folders);
                            }
                            if let Some((h, tail)) = hashes.take() {
                                p.view.columns.set_width(Column::GitHash, h.view);
                                p.prefs.defaults.columns.set_width(Column::GitHash, h.defaults);
                                for (key, width) in h.folders {
                                    if let Some(mut v) = p.folders.get(key) {
                                        v.columns.set_width(Column::GitHash, width);
                                        p.folders.put(key, v);
                                    }
                                }
                                if let Ok((servers, tail)) = postcard::take_from_bytes::<Vec<SavedServer>>(tail) {
                                    saved.side.saved_servers = servers;
                                    if let Ok((commands, _)) = postcard::take_from_bytes::<flit_core::ui::CommandUsage>(tail) {
                                        saved.tools.commands = commands.sanitized();
                                    }
                                }
                            }
                            p.folders.set_deep(std::mem::take(&mut deep));
                        }
                    }
                }
            }
        }
        Some(saved)
    }

    fn encode(&self) -> Option<Vec<u8>> {
        let p = &self.persisted;
        let folders = p.folders.entries();
        let prefs = ViewPrefs { defaults: legacy(p.prefs.defaults), ..p.prefs.clone() };
        let layouts = LayoutRecord {
            view: look(&p.view),
            defaults: look(&p.prefs.defaults),
            folders: folders.iter().map(|(_, v)| look(v)).collect(),
            thumbnails: p.prefs.thumbnails,
            thumb_cache_mb: p.prefs.thumb_cache_mb,
        };
        let gits = GitRecord { view: git_look(&p.view), defaults: git_look(&p.prefs.defaults), folders: folders.iter().map(|(_, v)| git_look(v)).collect() };
        let zooms = ZoomRecord { view: zoomed(&p.view), defaults: zoomed(&p.prefs.defaults), folders: folders.iter().map(|(_, v)| zoomed(v)).collect(), retain_filter: p.prefs.retain_filter };
        let wides = WideRecord { view: wide(&p.view), defaults: wide(&p.prefs.defaults), folders: folders.iter().map(|(_, v)| wide(v)).collect() };
        let attributes = AttributesRecord { view: attributed(&p.view), defaults: attributed(&p.prefs.defaults), folders: folders.iter().map(|(_, v)| attributed(v)).collect() };
        let shells = ShellRecord { view: shelled(&p.view), defaults: shelled(&p.prefs.defaults), folders: folders.iter().map(|(_, v)| shelled(v)).collect() };
        let hashes = GitHashRecord { view: hash_width(&p.view), defaults: hash_width(&p.prefs.defaults), folders: folders.iter().map(|(k, v)| (*k, hash_width(v))).collect() };
        let views = ViewRecord { view: legacy(p.view), prefs, folders: folders.into_iter().map(|(k, v)| (k, legacy(v))).collect() };
        let mut bytes = postcard::to_stdvec(&self.head()).ok()?;
        bytes.extend(postcard::to_stdvec(&self.nav).ok()?);
        bytes.extend(postcard::to_stdvec(&views).ok()?);
        bytes.extend(postcard::to_stdvec(&layouts).ok()?);
        bytes.extend(postcard::to_stdvec(&self.pane).ok()?);
        let mut side = self.side.clone();
        side.sanitize_servers();
        side.quick.visits.retain(|(location, _)| !network_needs_home(location));
        bytes.extend(postcard::to_stdvec(&side).ok()?);
        bytes.extend(postcard::to_stdvec(&p.archive.clone().sanitized()).ok()?);
        bytes.extend(postcard::to_stdvec(&Tools { streams: p.prefs.show_streams, ..self.tools.clone() }).ok()?);
        bytes.extend(postcard::to_stdvec(&SplitPrefs::default()).ok()?);
        bytes.extend(postcard::to_stdvec(&p.archive.clone().sanitized().compression.word).ok()?);
        bytes.extend(postcard::to_stdvec(&gits).ok()?);
        bytes.extend(postcard::to_stdvec(&p.prefs.command_descriptions).ok()?);
        bytes.extend(postcard::to_stdvec(&QuickRecord { quick: side.quick, deep: p.folders.deep_keys() }).ok()?);
        bytes.extend(postcard::to_stdvec(&zooms).ok()?);
        bytes.extend(postcard::to_stdvec(&self.nav.files).ok()?);
        bytes.extend(postcard::to_stdvec(&wides).ok()?);
        bytes.extend(postcard::to_stdvec(&FILES_COLUMN_CHOSEN).ok()?);
        bytes.extend(postcard::to_stdvec(&IntegrationRecord { git: p.prefs.git_integration, places: self.side.quick.places.clone(), places_hidden: self.side.quick.places_hidden.clone() }).ok()?);
        bytes.extend(postcard::to_stdvec(&(p.prefs.count_folders, p.prefs.count_files)).ok()?);
        bytes.extend(postcard::to_stdvec(&attributes).ok()?);
        bytes.extend(postcard::to_stdvec(&shells).ok()?);
        bytes.extend(postcard::to_stdvec(&hashes).ok()?);
        bytes.extend(postcard::to_stdvec(&side.saved_servers).ok()?);
        if self.tools.commands != flit_core::ui::CommandUsage::default() {
            bytes.extend(postcard::to_stdvec(&self.tools.commands).ok()?);
        }
        Some(bytes)
    }

    pub fn placement(&self) -> Option<Placement> {
        self.placement.map(|(x, y, width, height, maximized)| Placement { x, y, width, height, maximized })
    }
}

fn network_needs_home(location: &Location) -> bool {
    cfg!(target_os = "linux") && location.path().is_network()
}

pub(crate) fn compatible_location(location: &Location) -> Location {
    if network_needs_home(location) {
        Location::from_local(crate::P::home())
    } else {
        location.clone()
    }
}

pub fn file<P: ProfilePlatform>() -> Option<PathBuf> {
    P::state_dir().map(|d| d.join("state.bin"))
}

const MIGRATIONS: [crate::schema::Step; 0] = [];

fn load_from(f: &std::path::Path) -> Option<Saved> {
    crate::schema::load(f, VERSION, &MIGRATIONS, Saved::decode).map(Saved::sanitized)
}

pub fn load<P: ProfilePlatform + ShellPlatform>() -> Saved {
    file::<P>()
        .as_deref()
        .and_then(load_from)
        .unwrap_or_else(|| {
            let pinned = if flit_platform::isolated_gui_test() { Vec::new() } else { P::bookmarks() };
            Saved::new(Persisted { path: Location::from_local(P::home()), ..Default::default() }, None).with_nav(NavState { pinned, ..NavState::default() })
        })
}

pub fn save<P: ProfilePlatform>(s: &Saved) {
    let Some(f) = file::<P>() else { return };
    save_to(&f, s);
}

pub fn save_to(f: &std::path::Path, s: &Saved) {
    let Some(bytes) = s.encode() else { return };
    if let Err(e) = crate::settings::write_atomic(f, &bytes) {
        tracing::warn!("cannot save the window state: {e}");
    }
}

#[cfg(test)]
#[path = "../test/state.rs"]
mod tests;
