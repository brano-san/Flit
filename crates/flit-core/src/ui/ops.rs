mod rename;
mod drag;

use super::edit::{EditAction, Grab, LineEdit};
use super::menu::{Cmd, Mark};
use super::palette::{Palette, Pick};
use super::sidebar::Section;
use super::{App, ConfirmDelete, Drag, Effect, Focus, Hit, Route};
use crate::archive::{self, Format};
use crate::library;
use crate::hash::FxHashSet;
use crate::open::OpenTarget;
use crate::input::{CursorIcon, Event, Key, Mods, MouseButton};
use crate::listing::{attr, ChangeKind, DirChange};
use crate::location::{Location, ProviderId};
use crate::i18n::{P, S};
use crate::path::{FsPath, PathKind};
use crate::provider::{FileOp, GenToken, Launch, LinkChoices, LinkKind, OpCtl, OpId, OpReport, ProviderRequest, SnapshotSlot, TabId, Template};
use crate::recycle;
use crate::render::Rect;
use crate::sort::ViewSpec;
use crate::text::{Gfx, TextBackend};
use crate::theme::Theme;
use crate::undo::{Journal, Origin, Step};
use std::sync::Arc;
use std::time::{Duration, Instant};

const RENAME_VEIL: u8 = 0xB0;
pub(super) const DROP_FILL: u8 = 0x38;

pub mod drop_effect {
    pub const NONE: u32 = 0;
    pub const COPY: u32 = 1;
    pub const MOVE: u32 = 2;
    pub const LINK: u32 = 4;
}

pub const SHELL_VERBS: [&str; 6] = ["rename", "copy", "cut", "paste", "delete", "newfolder"];

const REVEAL_FOR: Duration = Duration::from_secs(10);
const SECTION_SPRING_DELAY: Duration = Duration::from_millis(500);
const DRAG_START: f32 = 4.0;
const MAX_NAME_UNITS: usize = 255;
const RESERVED: [&str; 4] = ["CON", "PRN", "AUX", "NUL"];
const BAD_CHARS: &str = r#"<>:"/\|?*"#;
const WINDOWS_FS: [&str; 9] = ["vfat", "msdos", "exfat", "ntfs", "ntfs3", "fuseblk", "cifs", "smb3", "smbfs"];
const WINDOWS_PROGRAMS: [&[u8]; 5] = [b"exe", b"com", b"bat", b"cmd", b"ps1"];
const G_WARNING: &str = "\u{E7BA}";
const HINT_GAP: f32 = 4.0;
const HINT_PAD_X: f32 = 10.0;
const HINT_PAD_Y: f32 = 6.0;
const HINT_ICON: f32 = 22.0;
const BIN_ONLY: S = S::UiOpsBinOnly;
const STREAMS_VIEW_ONLY: S = S::UiOpsStreamsViewOnly;
const OPEN_BIN: S = S::UiOpsOpenBin;
const MIXED_PATHS: S = S::UiOpsMixedPaths;
const MAX_TERMINALS: usize = 5;
const RUNNABLE: [&[u8]; 4] = [b"exe", b"com", b"bat", b"cmd"];
pub(super) const TIMER_SLACK: Duration = Duration::from_millis(16);
pub(super) const ERROR_FOR: Duration = Duration::from_secs(5);
const UNDO_BAR_FOR: Duration = Duration::from_secs(5);
const DRAG_SCROLL_RATE: f32 = 15.0;

pub(super) const LINK_KINDS: [(LinkKind, S); 4] = [(LinkKind::Shortcut, S::MenuShortcut), (LinkKind::Symbolic, S::MenuLinksSymbolicLink), (LinkKind::Junction, S::MenuLinksJunction), (LinkKind::Hard, S::MenuLinksHardLink)];

struct LinkProbe {
    generation: u64,
    tab: TabId,
    location: Location,
    dest: FsPath,
    items: Option<Vec<FsPath>>,
    choices: LinkChoices,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct NewTemplate {
    pub label: String,
    pub ext: String,
    pub stem: String,
    pub template: Template,
    #[serde(skip)]
    pub names: Option<TemplateNames>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TemplateNames {
    pub label: S,
    pub stem: S,
}

impl NewTemplate {
    pub fn label(&self) -> &str {
        self.names.map_or(&self.label, |n| n.label.t())
    }

    pub fn stem(&self) -> &str {
        self.names.map_or(&self.stem, |n| n.stem.t())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ShellMenu {
    pub dir: FsPath,
    pub items: Vec<FsPath>,
    pub x: f32,
    pub y: f32,
    pub extended: bool,
    pub menu: Vec<super::MenuItem>,
    pub inline: bool,
    pub shell: bool,
    pub kind: super::MenuKind,
}

#[derive(Clone, Debug, PartialEq)]
pub enum DragEvent {
    Enter { paths: Vec<FsPath>, x: f32, y: f32, mods: Mods, allowed: u32, right: bool },
    Over { x: f32, y: f32, mods: Mods, allowed: u32 },
    Leave,
    Drop { x: f32, y: f32, mods: Mods, allowed: u32 },
    DropData { x: f32, y: f32, data: std::sync::Arc<crate::provider::DroppedFile> },
    DropRepository { x: f32, y: f32, url: String },
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub(super) enum Then {
    Nothing,
    Select,
    Rename,
    Fill(Vec<FsPath>),
    Reveal(Vec<u8>),
    Open(FsPath),
    Launch,
    Terminal,
    TrashArchives(Vec<FsPath>),
    Clipboard,
    Drag,
    Reload,
}

struct Pending {
    id: OpId,
    tab: TabId,
    origin: Origin,
    then: Then,
    ctl: OpCtl,
    provider: ProviderId,
    optimistic_removal: bool,
}

struct Slow {
    tab: TabId,
    gen: u64,
    idx: u32,
    at: Instant,
}

struct Cut {
    dir: Location,
    names: FxHashSet<Vec<u8>>,
    seq: Option<u32>,
}

struct Reveal {
    tab: TabId,
    dir: Location,
    names: Vec<Vec<u8>>,
    rename: bool,
    started: bool,
    until: Instant,
}

struct Rename {
    tab: TabId,
    gen: u64,
    idx: u32,
    edit: LineEdit,
    scroll: f32,
    rect: Rect,
    click: Option<(f32, Grab)>,
    selecting: Option<Grab>,
    batch: bool,
    confirmed: Option<String>,
    hint: Option<String>,
    volume: Option<String>,
}

struct Retry {
    id: OpId,
    tab: TabId,
    dir: Location,
    old: Vec<u8>,
    text: String,
}

struct Press {
    row: usize,
    x: f32,
    y: f32,
    deferred: bool,
    slow: bool,
    open: bool,
    toggle: bool,
    right: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Spot {
    List,
    Row(usize),
    Tab(usize),
    Crumb(usize),
    Side(usize),
    NewTab,
    SideHeader(Section),
}

#[derive(Clone, PartialEq, Debug)]
enum Target {
    Reveal,
    NewTab,
    Into { dest: FsPath, spot: Spot },
    Trash(Spot),
    Run { program: FsPath, spot: Spot },
    Pin,
}

pub(super) struct Dropping {
    paths: Vec<FsPath>,
    target: Option<Target>,
    effect: u32,
    spring: Option<(Spot, Instant)>,
    right: bool,
    ours: bool,
    onto: Option<FsPath>,
}

struct Asked {
    paths: Vec<FsPath>,
    dest: FsPath,
    admin: bool,
}

#[derive(Default)]
pub(super) struct Ops {
    pub(super) jobs: super::jobs::Jobs,
    link_generation: u64,
    link_probe: Option<LinkProbe>,
    next_id: OpId,
    pending: Vec<Pending>,
    journal: Journal,
    undo_remaining: Vec<u64>,
    reveal: Vec<Reveal>,
    rename: Option<Rename>,
    retry: Option<Retry>,
    press: Option<Press>,
    pub(super) dropping: Option<Dropping>,
    pub(super) drag_scroll: Option<(f32, f32, Mods, u32)>,
    right_drag: Option<Vec<FsPath>>,
    pub(super) network_drag: Vec<Route>,
    asked: Option<Asked>,
    slow: Option<Slow>,
    cut: Option<Cut>,
    bin: Option<BinProbe>,
    ids: u32,
    unpack_drag: bool,
    pub(super) admin_drag: Option<Vec<FsPath>>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Insert {
    Id,
    Today,
    Created,
    Modified,
}

pub fn iso_date(ticks: i64, offset_min: i32) -> String {
    let (y, m, d) = crate::fmt::civil_from_days(crate::fmt::local_days(ticks, offset_min));
    format!("{y:04}-{m:02}-{d:02}")
}

struct BinProbe {
    gen: u64,
    slot: Arc<SnapshotSlot>,
    cancel: GenToken,
}

pub fn validate_name(text: &str, windows: bool) -> Result<String, &'static str> {
    let mut name = text.trim();
    if windows {
        name = name.trim_end_matches(['.', ' ']);
    }
    if name.is_empty() {
        return Err(S::UiOpsNameEmpty.t());
    }
    if name == "." || name == ".." {
        return Err(S::UiOpsNameNotAllowed.t());
    }
    if windows {
        if name.chars().any(|c| bad_name_char(c, true)) {
            return Err(S::UiOpsNameBadChars.t());
        }
        let stem = name.split('.').next().unwrap_or("").trim_end().to_ascii_uppercase();
        let port = |n: &str| (n.len() == 1 && n.as_bytes()[0].is_ascii_digit()) || matches!(n, "¹" | "²" | "³");
        if RESERVED.contains(&stem.as_str()) || ["COM", "LPT"].iter().any(|p| stem.strip_prefix(p).is_some_and(port)) {
            return Err(S::UiOpsNameReserved.t());
        }
    } else if name.contains(['/', '\0']) {
        return Err(S::UiOpsNameSlash.t());
    }
    if (if windows { name.encode_utf16().count() } else { name.len() }) > MAX_NAME_UNITS {
        return Err(S::UiOpsNameTooLong.t());
    }
    Ok(name.to_owned())
}

pub fn bad_name_char(c: char, windows: bool) -> bool {
    if windows { c.is_control() || BAD_CHARS.contains(c) } else { c == '/' || c == '\0' }
}

pub fn windows_names(dir: &FsPath, drives: &[super::Drive]) -> bool {
    if dir.kind() != PathKind::Unix {
        return true;
    }
    let mount = drives.iter().filter(|d| !d.unmounted && d.path.kind() == PathKind::Unix && (dir.same(&d.path) || dir.is_inside(&d.path))).max_by_key(|d| d.path.as_bytes().len());
    mount.is_some_and(|d| WINDOWS_FS.iter().any(|f| d.file_system.eq_ignore_ascii_case(f)) || d.remote.as_deref().is_some_and(|r| r.starts_with("smb-share:")))
}

fn bad_chars_hint(windows: bool) -> &'static str {
    if windows { S::UiOpsNameBadChars.t() } else { S::UiOpsNameSlash.t() }
}

fn same_name(a: &[u8], b: &[u8], folded: Option<&str>) -> bool {
    match folded {
        None => a == b,
        Some(f) => a.eq_ignore_ascii_case(b) || (!a.is_ascii() && crate::name::display(a).to_lowercase() == f),
    }
}

pub fn icon_location(text: &str) -> Option<(FsPath, i32)> {
    let text = text.trim();
    let (path, index) = text.rsplit_once(',').and_then(|(p, i)| Some((p.trim(), i.trim().parse().ok()?))).unwrap_or((text, 0));
    FsPath::parse(path.trim_matches('"')).filter(|p| matches!(p.kind(), PathKind::Drive | PathKind::Unc | PathKind::Verbatim)).map(|p| (p, index))
}

pub fn unique_name(base: &str, taken: impl Fn(&str) -> bool) -> String {
    let mut k = 1;
    loop {
        let name = if k == 1 { base.to_owned() } else { format!("{base} ({k})") };
        if !taken(&name) {
            return name;
        }
        k += 1;
    }
}

pub fn base_name_len(name: &str, dir: bool) -> usize {
    name.len() - extension(name.as_bytes(), dir).len()
}

fn extension(name: &[u8], dir: bool) -> &[u8] {
    let ext = crate::name::ext_len(name);
    if dir || ext == 0 || ext + 1 >= name.len() {
        b""
    } else {
        &name[name.len() - ext - 1..]
    }
}

pub fn changes_extension(old: &str, new: &str) -> bool {
    !extension(old.as_bytes(), false).eq_ignore_ascii_case(extension(new.as_bytes(), false))
}

pub fn batch_names(base: &str, items: &[(&[u8], bool)], taken: impl Fn(&[u8]) -> bool) -> Vec<Vec<u8>> {
    let mut k = 0;
    items
        .iter()
        .map(|&(name, dir)| loop {
            k += 1;
            let mut n = format!("{base} ({k})").into_bytes();
            n.extend_from_slice(extension(name, dir));
            if !taken(&n.to_ascii_lowercase()) {
                return n;
            }
        })
        .collect()
}

pub fn same_volume(a: &FsPath, b: &FsPath, drives: &[super::Drive]) -> bool {
    if a.kind() != b.kind() {
        return false;
    }
    if a.kind() != PathKind::Unix {
        return a.root_bytes().eq_ignore_ascii_case(b.root_bytes());
    }
    let mount = |p: &FsPath| drives.iter().map(|d| &d.path).filter(|m| m.kind() == PathKind::Unix && (p.same(m) || p.is_inside(m))).max_by_key(|m| m.as_bytes().len()).map(FsPath::as_bytes);
    mount(a) == mount(b)
}

pub fn choose_effect(allowed: u32, m: Mods, same_volume: bool) -> u32 {
    use drop_effect::*;
    let wanted = if m.alt || (m.ctrl && m.shift) {
        LINK
    } else if m.ctrl {
        COPY
    } else if m.shift || same_volume {
        MOVE
    } else {
        COPY
    };
    if allowed & wanted != 0 {
        return wanted;
    }
    if m.alt || m.ctrl || m.shift {
        return NONE;
    }
    [COPY, MOVE, LINK].into_iter().find(|e| allowed & e != 0).unwrap_or(NONE)
}

pub(super) fn unpacked_copies() -> Option<FsPath> {
    static COPIES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
    let n = COPIES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join("Flit").join(format!("{}-{stamp}-copy-{n}", std::process::id()));
    FsPath::parse(&dir.to_string_lossy())
}

fn op_path(loc: &Location) -> Option<FsPath> {
    match loc {
        Location::Wsl { path, .. } => Some(path.clone()),
        _ => loc.to_windows(),
    }
}

fn in_distro(op: &FileOp) -> Option<(String, FileOp)> {
    if !matches!(op, FileOp::Copy { .. } | FileOp::Move { .. } | FileOp::MoveTo(_) | FileOp::Rename { .. } | FileOp::NewFolder { .. } | FileOp::Delete(_) | FileOp::Trash(_) | FileOp::Paste { .. }) {
        return None;
    }
    let distro: std::cell::RefCell<Option<String>> = Default::default();
    let linux = op.map_paths(|p| {
        let Location::Wsl { distro: d, path } = Location::from_local(p.clone()) else { return None };
        let mut seen = distro.borrow_mut();
        let same = seen.get_or_insert_with(|| d.clone()).eq_ignore_ascii_case(&d);
        (same && archive::split(p).is_none()).then_some(path)
    })?;
    let real = |dir: &FsPath, name: &[u8]| Location::from_local(dir.join(name)).file_name().to_vec();
    let linux = match (op, linux) {
        (FileOp::Rename { item, name }, FileOp::Rename { item: path, .. }) => FileOp::Rename { name: real(&item.parent()?, name), item: path },
        (FileOp::NewFolder { dir, name }, FileOp::NewFolder { dir: path, .. }) => FileOp::NewFolder { name: real(dir, name), dir: path },
        (_, linux) => linux,
    };
    Some((distro.into_inner()?, linux))
}

pub fn route(provider: ProviderId, op: FileOp, unix_local: bool) -> Option<(ProviderId, FileOp)> {
    let unix = |p: &FsPath| p.kind() == PathKind::Unix;
    if let (ProviderId::Wsl(distro), false) = (&provider, unix_local) {
        if !op.all_paths(unix) && !op.all_paths(|p| !unix(p)) {
            let shared = op.map_paths(|p| if unix(p) { Location::Wsl { distro: distro.clone(), path: p.clone() }.to_windows() } else { Some(p.clone()) })?;
            return route(ProviderId::Local, shared, false);
        }
    }
    if op.all_paths(|p| !unix(p)) {
        return Some(match in_distro(&op) {
            Some((distro, linux)) => (ProviderId::Wsl(distro), linux),
            None => (ProviderId::Local, op),
        });
    }
    ((unix_local || matches!(provider, ProviderId::Wsl(_))) && op.all_paths(unix)).then_some((provider, op))
}

fn same_location(a: &Location, b: &Location) -> bool {
    let (x, y) = (a.to_bytes(), b.to_bytes());
    if a.path().case_sensitive() { x == y } else { x.eq_ignore_ascii_case(&y) }
}

fn removal_count(op: &FileOp) -> usize {
    match op {
        FileOp::Trash(items) | FileOp::Delete(items) => items.len(),
        _ => 0,
    }
}

impl App {
    pub fn status(&mut self, msg: impl Into<String>) {
        let msg = msg.into();
        self.tab_mut().error = Some(crate::redact::passwords(&crate::i18n::localize(&msg)).into_owned());
        self.dirty = true;
    }

    pub(super) fn expire_error(&mut self, now: Instant) {
        let (id, error) = (self.tab().id, self.tab().error.clone());
        let Some(error) = error else {
            self.error_shown = None;
            return;
        };
        match &self.error_shown {
            Some((at, shown, since)) if *at == id && *shown == error => {
                let until = *since + ERROR_FOR;
                if now >= until {
                    self.tab_mut().error = None;
                    self.error_shown = None;
                    self.dirty = true;
                } else {
                    self.arm_timer(now, until);
                }
            }
            _ => {
                self.error_shown = Some((id, error, now));
                self.arm_timer(now, now + ERROR_FOR);
            }
        }
    }

    pub(super) fn expire_undo_bar(&mut self, now: Instant) {
        let Some((_, since)) = self.tab().undo else { return };
        let until = since + UNDO_BAR_FOR;
        if now >= until {
            self.tab_mut().undo = None;
            self.dirty = true;
        } else {
            self.arm_timer(now, until);
        }
    }

    pub(super) fn in_bin(&self) -> bool {
        self.tab().view_loc.is_recycle_bin()
    }

    pub(super) fn shell_dir(&self) -> Result<FsPath, &'static str> {
        let loc = &self.tab().view_loc;
        match loc.path().kind() {
            PathKind::Computer => return Err(S::UiOpsNotInComputer.t()),
            PathKind::RecycleBin => return Err(S::UiOpsNotInBin.t()),
            PathKind::Home => return Err(S::UiOpsNotOnHome.t()),
            _ if library::is_library(loc.path()) => return self.library_save(loc.path()).ok_or(S::UiOpsLibraryNoFolder.t()),
            _ => {}
        }
        loc.to_windows().ok_or(S::UiOpsFolderNotShell.t())
    }

    pub(super) fn share_selection(&mut self) {
        let items = self.selected_items();
        if items.is_empty() {
            return self.status(S::UiOpsSelectShare.t());
        }
        match self.item_paths(&items) {
            Ok(paths) => self.effects.push(Effect::Share(paths)),
            Err(e) => self.status(e),
        }
    }

    pub(super) fn item_paths(&self, items: &[u32]) -> Result<Vec<FsPath>, &'static str> {
        if self.in_bin() {
            return Err(BIN_ONLY.t());
        }
        let t = self.tab();
        if t.view.as_ref().is_some_and(|v| items.iter().any(|&i| v.listing().entries[i as usize].attrs & attr::STREAM != 0)) {
            return Err(STREAMS_VIEW_ONLY.t());
        }
        let plain = match (&t.view, &t.view_loc) {
            (Some(v), Location::Local(dir)) if !t.view_loc.is_recycle_bin() && !library::is_library(dir) && !matches!(dir.kind(), PathKind::Home | PathKind::Computer) => Some((v.listing(), dir)),
            _ => None,
        };
        match plain {
            Some((l, dir)) => Ok(items.iter().map(|&i| dir.join(l.name(i))).collect()),
            None => items.iter().map(|&i| t.entry_location(i)?.to_windows()).collect::<Option<Vec<_>>>().ok_or(S::UiOpsNamesNotShell.t()),
        }
    }

    pub fn selection_key(&self) -> (TabId, u64, u64, Option<u32>) {
        let t = self.tab();
        let v = t.view.as_ref();
        (t.id, t.gen, v.map_or(0, |v| v.selection_rev()), v.and_then(|v| v.focus))
    }

    pub fn selection_source(&self) -> std::sync::Arc<dyn Fn() -> (Vec<FsPath>, Option<FsPath>) + Send + Sync> {
        let t = self.tab();
        let plain = match (&t.view, &t.view_loc) {
            (Some(v), Location::Local(dir)) if !self.in_bin() && !library::is_library(dir) && !matches!(dir.kind(), PathKind::Home | PathKind::Computer) => Some((v.snap().clone(), dir.clone(), v.focus)),
            _ => None,
        };
        let Some((snap, dir, focus)) = plain else {
            let picked = self.selection_paths();
            return std::sync::Arc::new(move || picked.clone());
        };
        let items = self.selected_items();
        std::sync::Arc::new(move || {
            let l = &snap.listing;
            let path = |i: u32| (l.entries[i as usize].attrs & attr::STREAM == 0).then(|| dir.join(l.name(i)));
            let paths = items.iter().map(|&i| path(i)).collect::<Option<Vec<_>>>().unwrap_or_default();
            (paths, focus.and_then(path))
        })
    }

    pub fn selection_paths(&self) -> (Vec<FsPath>, Option<FsPath>) {
        let focused = self.tab().view.as_ref().and_then(|v| v.focus).and_then(|i| self.item_paths(&[i]).ok()?.pop());
        (self.item_paths(&self.selected_items()).unwrap_or_default(), focused)
    }

    pub(super) fn selected_items(&self) -> Vec<u32> {
        let t = self.tab();
        match &t.view {
            Some(v) if t.view_loc.path().kind() != PathKind::Computer => v.selected().collect(),
            _ => Vec::new(),
        }
    }

    pub(super) fn start_op(&mut self, op: FileOp, origin: Origin, then: Then) {
        self.start_op_for(self.tab().id, op, origin, then);
    }

    pub(super) fn start_op_for(&mut self, tab: TabId, op: FileOp, origin: Origin, then: Then) {
        self.start_op_on(ProviderId::Local, tab, op, origin, then);
    }

    fn start_op_on(&mut self, provider: ProviderId, tab: TabId, op: FileOp, origin: Origin, then: Then) {
        self.start_op_with_ctl(provider, tab, op, origin, then, OpCtl::new(self.conflict_default));
    }

    fn start_op_with_ctl(&mut self, provider: ProviderId, tab: TabId, op: FileOp, origin: Origin, then: Then, ctl: OpCtl) {
        if let Some(t) = self.tabs.items.iter_mut().find(|t| t.id == tab) {
            t.undo = None;
        }
        let routed = if ctl.replay().is_some() { Some((provider, op)) } else { route(self.admin_route(provider, tab), op, self.caps.unix_paths) };
        let Some((provider, op)) = routed else { return self.status(MIXED_PATHS.t()) };
        let id = self.allocate_op_id();
        if self.settings.hide_companions && provider == ProviderId::Local && origin == Origin::Fresh
            && matches!(op, FileOp::Copy { .. } | FileOp::Move { .. } | FileOp::MoveTo(_) | FileOp::Rename { .. } | FileOp::Paste { .. })
            && op.all_paths(|p| matches!(p.kind(), PathKind::Drive | PathKind::Unix))
        {
            ctl.attach_companions();
        }
        self.track_job(id, &provider, &op, &ctl);
        let optimistic_removal = ctl.replay().is_none() && provider == ProviderId::Local
            && match &op {
                FileOp::Trash(items) | FileOp::Delete(items) => self.tabs.items.iter().find(|t| t.id == tab).is_some_and(|t| {
                    let (Some(view), Some(dir)) = (&t.view, t.view_loc.to_windows()) else { return false };
                    let listing = view.listing();
                    items.iter().any(|path| {
                        path.parent().is_some_and(|parent| parent.same(&dir))
                            && (0..listing.len() as u32).find(|&i| listing.name(i).eq_ignore_ascii_case(path.file_name())).is_some_and(|i| listing.entries[i as usize].is_dir())
                    })
                }),
                _ => false,
            };
        self.ops.pending.push(Pending { id, tab, origin, then, ctl: ctl.clone(), provider: provider.clone(), optimistic_removal });
        if optimistic_removal {
            let sources = match &op {
                FileOp::Trash(items) | FileOp::Delete(items) => items,
                _ => unreachable!(),
            };
            let report = OpReport::new(op.clone(), sources.iter().cloned().map(|path| (path.clone(), path)).collect());
            if let Some(k) = self.tabs.items.iter().position(|t| t.id == tab) {
                self.drop_gone(k, &provider, &report);
            }
        }
        self.effects.push(Effect::Provider { provider, req: ProviderRequest::Op { tab, id, op, ctl } });
    }

    pub(super) fn start_renames(&mut self, tab: TabId, pairs: Vec<(Location, Location)>) {
        let provider = pairs.first().map_or(ProviderId::Local, |(from, _)| from.provider());
        match pairs.iter().map(|(from, to)| Some((op_path(from)?, op_path(to)?))).collect::<Option<Vec<_>>>() {
            Some(p) => self.start_op_on(provider, tab, FileOp::MoveTo(p), Origin::Fresh, Then::Select),
            None => self.status(S::UiOpsNamesNotShell.t()),
        }
    }

    pub(super) fn start_confirmed(&mut self, provider: ProviderId, op: FileOp) {
        self.start_op_on(provider, self.tab().id, op, Origin::Fresh, Then::Nothing);
    }

    fn delete_or_ask(&mut self, provider: ProviderId, op: FileOp) {
        let ask = match self.confirm_delete {
            ConfirmDelete::Always => true,
            ConfirmDelete::Permanent => !matches!(op, FileOp::Trash(_)),
            ConfirmDelete::Never => provider == ProviderId::Admin,
        };
        if ask {
            self.ask_confirm_on(provider, op);
        } else {
            self.start_confirmed(provider, op);
        }
    }

    pub(super) fn op_ctl(&self, id: OpId) -> Option<OpCtl> {
        self.ops.pending.iter().find(|p| p.id == id).map(|p| p.ctl.clone())
    }

    pub(super) fn allocate_op_id(&mut self) -> OpId {
        self.ops.next_id += 1;
        self.ops.next_id
    }

    pub(super) fn cancel_ops(&mut self) {
        for p in &self.ops.pending {
            p.ctl.cancel();
        }
        self.cancel_git_jobs();
    }

    pub fn ops_pending(&self) -> bool {
        !self.ops.pending.is_empty() || self.running_jobs() != 0
    }

    pub(super) fn ops_in_flight(&self) -> usize {
        self.ops.pending.len()
    }

    fn delete_with(&mut self, permanent: bool) {
        if self.in_bin() {
            return self.purge_selection();
        }
        let items = self.selected_items();
        if items.is_empty() {
            return;
        }
        let t = self.tab();
        let paths = match &t.view_loc {
            Location::Wsl { .. } | Location::Admin(_) => items.iter().map(|&i| t.entry_location(i).map(|item| item.path().clone()))
                .collect::<Option<Vec<_>>>().map(|paths| (t.view_loc.provider(), paths)).ok_or(S::UiOpsNamesNotShell.t()),
            _ => self.item_paths(&items).map(|p| (ProviderId::Local, p)),
        };
        let permanent = permanent || t.view_loc.is_admin();
        match paths {
            Ok((provider, paths)) if permanent => self.delete_or_ask(provider, FileOp::Delete(paths)),
            Ok((provider, paths)) => self.delete_or_ask(provider, FileOp::Trash(paths)),
            Err(e) => self.status(e),
        }
    }

    pub fn trash_selection(&mut self) {
        self.delete_with(false);
    }

    pub fn delete_selection(&mut self) {
        self.delete_with(true);
    }

    pub fn flatten_selection(&mut self) {
        let items: Vec<u32> = match &self.tab().view {
            Some(v) => self.selected_items().into_iter().filter(|&i| v.listing().entries[i as usize].is_dir()).collect(),
            None => Vec::new(),
        };
        if items.is_empty() {
            return self.status(S::UiOpsSelectFlatten.t());
        }
        match self.item_paths(&items) {
            Ok(dirs) => self.start_op(FileOp::Flatten(dirs), Origin::Fresh, Then::Nothing),
            Err(e) => self.status(e),
        }
    }

    fn bin_pairs(&self, items: impl Iterator<Item = u32>) -> Vec<(FsPath, FsPath)> {
        let Some(v) = &self.tab().view else { return Vec::new() };
        let l = v.listing();
        items.filter_map(|i| Some((recycle::recycled(l, i)?, recycle::original(l, i)?))).collect()
    }

    pub(super) fn shell_bin(&self) -> bool {
        self.in_bin() && !self.caps.unix_paths
    }

    fn recycled_paths(&self, items: &[u32]) -> Vec<FsPath> {
        self.bin_pairs(items.iter().copied()).into_iter().map(|(recycled, _)| recycled).collect()
    }

    fn purge_selection(&mut self) {
        let pairs = self.bin_pairs(self.selected_items().into_iter());
        if !pairs.is_empty() {
            self.delete_or_ask(ProviderId::Local, FileOp::Purge(pairs));
        }
    }

    fn restore(&mut self, all: bool) {
        if !self.in_bin() {
            return if all { self.probe_bin() } else { self.status(OPEN_BIN.t()) };
        }
        let pairs = match (all, &self.tab().view) {
            (true, Some(v)) => {
                let l = v.listing();
                self.bin_pairs((0..l.len() as u32).filter(|&i| l.entries[i as usize].attrs & attr::DELETED == 0))
            }
            _ => self.bin_pairs(self.selected_items().into_iter()),
        };
        match pairs.is_empty() {
            true if all => self.status(S::UiEmptyBin.t()),
            true => self.status(S::UiOpsSelectRestore.t()),
            false if all => self.ask_confirm(FileOp::Restore(pairs)),
            false => self.start_op(FileOp::Restore(pairs), Origin::Fresh, Then::Nothing),
        }
    }

    fn probe_bin(&mut self) {
        let (gen, bin) = (self.gen(), Location::recycle_bin());
        let probe = BinProbe { gen, slot: Arc::default(), cancel: GenToken::default() };
        probe.cancel.set(gen);
        let req = ProviderRequest::List { tab: 0, gen, path: bin.path().clone(), view: ViewSpec::default(), case: self.case, out: probe.slot.clone(), cancel: probe.cancel.clone() };
        self.effects.push(Effect::Provider { provider: bin.provider(), req });
        if let Some(old) = self.ops.bin.replace(probe) {
            old.cancel.set(u64::MAX);
        }
    }

    pub(super) fn poll_bin(&mut self) {
        let Some(p) = &self.ops.bin else { return };
        let Some(snap) = p.slot.take().filter(|s| s.gen == p.gen && s.done) else { return };
        self.ops.bin = None;
        let l = &snap.listing;
        let pairs: Vec<(FsPath, FsPath)> = (0..l.len() as u32).filter(|&i| l.entries[i as usize].attrs & attr::DELETED == 0).filter_map(|i| Some((recycle::recycled(l, i)?, recycle::original(l, i)?))).collect();
        match pairs.is_empty() {
            true => self.status(S::UiEmptyBin.t()),
            false => self.ask_confirm(FileOp::Restore(pairs)),
        }
    }

    pub fn restore_selection(&mut self) {
        self.restore(false);
    }

    pub fn restore_all(&mut self) {
        self.restore(true);
    }

    pub fn empty_bin(&mut self) {
        self.ask_confirm(FileOp::EmptyBin);
    }

    pub(super) fn bin_button(&mut self, k: usize) {
        match k {
            0 => self.restore_selection(),
            1 => self.restore_all(),
            _ => self.empty_bin(),
        }
    }

    fn put_on_clipboard(&mut self, cut: bool) {
        let items = self.selected_items();
        if items.is_empty() {
            return;
        }
        if cut && self.archive_view().is_some() {
            return self.status(crate::i18n::S::ErrorsArchiveInside.t());
        }
        if self.archive_view().is_some() {
            return match (self.item_paths(&items), unpacked_copies()) {
                (Ok(items), Some(dest)) => self.start_op(FileOp::Copy { items, dest }, Origin::Fresh, Then::Clipboard),
                (Err(e), _) => self.status(e),
                (_, None) => self.status(S::ErrorsArchiveInside.t()),
            };
        }
        let paths = match cut && self.shell_bin() {
            true => Ok(self.recycled_paths(&items)),
            false => self.item_paths(&items),
        };
        match paths {
            Ok(paths) => {
                let t = self.tab();
                self.ops.cut = t.view.as_ref().filter(|_| cut).map(|v| Cut { dir: t.view_loc.clone(), names: items.iter().map(|&i| v.listing().name(i).to_vec()).collect(), seq: None });
                self.effects.push(Effect::SetClipboardFiles { paths, cut });
                self.dirty = true;
            }
            Err(e) => self.status(e),
        }
    }

    pub(super) fn clipboard_changed(&mut self, seq: u32) {
        if self.ops.link_probe.as_ref().is_some_and(|p| p.items.is_none()) {
            self.ops.link_probe = None;
            for (id, cmd) in self.menu.clone().iter().enumerate() {
                if matches!(cmd, Cmd::PasteAs(kind) if *kind != LinkKind::Shortcut) {
                    self.set_menu_disabled(id, true);
                }
            }
        }
        let Some(c) = &mut self.ops.cut else { return };
        match c.seq {
            None => c.seq = Some(seq),
            Some(s) if s != seq => {
                self.ops.cut = None;
                self.dirty = true;
            }
            _ => {}
        }
    }

    pub(super) fn cut_names(&self) -> Option<&FxHashSet<Vec<u8>>> {
        self.ops.cut.as_ref().filter(|c| c.dir == self.tab().view_loc).map(|c| &c.names)
    }

    pub fn is_cut(&self, name: &[u8]) -> bool {
        self.cut_names().is_some_and(|n| n.contains(name))
    }

    pub fn copy_selection(&mut self) {
        self.put_on_clipboard(false);
    }

    pub fn cut_selection(&mut self) {
        self.put_on_clipboard(true);
    }

    pub fn paste(&mut self) {
        match self.shell_dir() {
            Ok(dest) => self.start_op(FileOp::Paste { dest }, Origin::Fresh, Then::Select),
            Err(e) => self.status(e),
        }
    }

    pub fn paste_shortcut(&mut self) {
        match self.shell_dir() {
            Ok(dest) => self.start_op(FileOp::PasteLink { dest }, Origin::Fresh, Then::Select),
            Err(e) => self.status(e),
        }
    }

    fn probe_links(&mut self, dest: FsPath, items: Option<Vec<FsPath>>) {
        self.ops.link_generation = self.ops.link_generation.wrapping_add(1);
        let generation = self.ops.link_generation;
        self.effects.push(Effect::LinkChoices { generation, items: items.clone(), dest: dest.clone() });
        self.ops.link_probe = Some(LinkProbe { generation, tab: self.tab().id, location: self.current_location().clone(), dest, items, choices: LinkChoices::default() });
    }

    pub(super) fn on_link_choices(&mut self, generation: u64, choices: LinkChoices) {
        let tab = self.tab().id;
        let location = self.current_location().clone();
        let Some(probe) = self.ops.link_probe.as_mut().filter(|p| p.generation == generation && p.tab == tab && p.location == location) else { return };
        probe.choices = choices;
        self.refresh_link_choices();
    }

    pub(super) fn refresh_link_choices(&mut self) {
        for (id, command) in self.menu.clone().iter().enumerate() {
            let allowed = match command {
                Cmd::PasteAs(kind) => Some(self.paste_link_allowed(*kind)),
                Cmd::DropAs(kind) => Some(self.ops.asked.as_ref().is_some_and(|a| self.ops.link_probe.as_ref().is_some_and(|p| p.tab == self.tab().id && p.location == *self.current_location() && p.dest == a.dest && p.items.as_ref() == Some(&a.paths) && p.choices.allows(*kind)))),
                _ => None,
            };
            if let Some(allowed) = allowed {
                self.set_menu_disabled(id, !allowed);
            }
        }
    }

    pub(super) fn paste_link_allowed(&self, kind: LinkKind) -> bool {
        if kind == LinkKind::Shortcut {
            return self.shell_dir().is_ok();
        }
        self.ops.link_probe.as_ref().is_some_and(|p| p.tab == self.tab().id && p.items.is_none() && self.shell_dir().is_ok_and(|d| d == p.dest) && p.choices.allows(kind))
    }

    pub(super) fn paste_as(&mut self, kind: LinkKind) {
        if !self.paste_link_allowed(kind) {
            return;
        }
        if let Ok(dest) = self.shell_dir() {
            let op = if kind == LinkKind::Shortcut { FileOp::PasteLink { dest } } else { FileOp::PasteLinkAs { dest, kind } };
            self.start_op(op, Origin::Fresh, Then::Select);
        }
    }

    pub fn paste_into_selection(&mut self) {
        let items = self.selected_items();
        let is_dir = |i: u32| self.tab().view.as_ref().is_some_and(|v| v.listing().entries[i as usize].is_dir());
        let dest = match items.as_slice() {
            &[i] if is_dir(i) => self.item_paths(&[i]).ok().and_then(|mut p| p.pop()),
            _ => None,
        };
        match dest {
            Some(dest) => self.start_op(FileOp::Paste { dest }, Origin::Fresh, Then::Nothing),
            None => self.status(S::UiOpsSelectPasteInto.t()),
        }
    }

    pub(super) fn free_name(&self, stem: &str, ext: &str) -> String {
        let prefix = stem.to_ascii_lowercase().into_bytes();
        let taken: FxHashSet<Vec<u8>> = match &self.tab().view {
            Some(v) => {
                let l = v.listing();
                (0..l.len() as u32).filter(|&i| l.entries[i as usize].attrs & attr::DELETED == 0).map(|i| l.name(i).to_ascii_lowercase()).filter(|n| n.starts_with(&prefix)).collect()
            }
            None => FxHashSet::default(),
        };
        unique_name(stem, |n| taken.contains(format!("{n}{ext}").to_ascii_lowercase().as_bytes())) + ext
    }

    fn create(&mut self, op: impl FnOnce(FsPath, Vec<u8>) -> FileOp, stem: &str, ext: &str) {
        let dir = match self.shell_dir() {
            Ok(d) => d,
            Err(e) => return self.status(e),
        };
        if self.tab().filter.is_some() {
            self.close_filter();
        }
        let name = self.free_name(stem, ext);
        self.start_op(op(dir, name.into_bytes()), Origin::Fresh, Then::Rename);
    }

    pub fn new_file(&mut self) {
        let stem = S::UiOpsNewFileName.in_lang(crate::i18n::lang().catalogue());
        self.create(|dir, name| FileOp::NewFile { dir, name, template: Template::Empty }, stem, "");
    }

    pub(super) fn duplicate_selection(&mut self) {
        let items = self.selected_items();
        if items.is_empty() {
            return;
        }
        match (self.item_paths(&items), self.shell_dir()) {
            (Ok(items), Ok(dest)) => self.start_op(FileOp::Copy { items, dest }, Origin::Fresh, Then::Select),
            (Err(e), _) | (_, Err(e)) => self.status(e),
        }
    }

    pub(super) fn send_to_panel(&mut self, back: bool, moving: bool) {
        let order = self.order();
        let n = order.len();
        let items = self.selected_items();
        if n < 2 || items.is_empty() {
            return;
        }
        let at = order.iter().position(|&id| id == self.tabs.panels.active).unwrap_or(0);
        let target = order[if back { (at + n - 1) % n } else { (at + 1) % n }];
        let dest = self.visit(target, |a| a.shell_dir());
        match (self.item_paths(&items), dest) {
            (Ok(items), Ok(dest)) => {
                let op = if moving { FileOp::Move { items, dest } } else { FileOp::Copy { items, dest } };
                self.start_op(op, Origin::Fresh, Then::Nothing);
            }
            (Err(e), _) | (_, Err(e)) => self.status(e),
        }
    }

    pub fn new_folder(&mut self) {
        let stem = S::UiOpsNewFolderName.in_lang(crate::i18n::lang().catalogue());
        self.create(|dir, name| FileOp::NewFolder { dir, name }, stem, "");
    }

    pub fn new_folder_with_selection(&mut self) {
        let items = self.selected_items();
        if items.is_empty() {
            return self.status(S::UiOpsSelectNewFolder.t());
        }
        let (paths, dir) = match (self.item_paths(&items), self.shell_dir()) {
            (Ok(p), Ok(d)) => (p, d),
            (Err(e), _) | (_, Err(e)) => return self.status(e),
        };
        let stem = S::UiOpsNewFolderName.in_lang(crate::i18n::lang().catalogue());
        let name = self.free_name(stem, "").into_bytes();
        self.start_op(FileOp::NewFolder { dir, name }, Origin::Fresh, Then::Fill(paths));
    }

    pub fn create_shortcuts(&mut self) {
        let items = self.selected_items();
        if items.is_empty() {
            return self.status(S::UiOpsSelectShortcut.t());
        }
        match (self.item_paths(&items), self.shell_dir()) {
            (Ok(items), Ok(dest)) => self.start_op(FileOp::Link { items, dest }, Origin::Fresh, Then::Select),
            (Err(e), _) | (_, Err(e)) => self.status(e),
        }
    }

    pub fn new_item_dialog(&mut self) {
        if let Err(e) = self.shell_dir() {
            return self.status(e);
        }
        let folder_keys = self.settings.keymap.text_of("file.new_folder");
        let mut items = vec![(S::MenuFolder.t().to_owned(), folder_keys), (S::MenuShortcut.t().to_owned(), String::new())];
        items.extend(self.nav.sidebar.places.templates.iter().map(|t| (t.label().to_owned(), t.ext.clone())));
        self.open_pick(Palette::new(Pick::NewItem, items, ""));
    }

    pub(super) fn new_item(&mut self, k: usize) {
        match k {
            0 => self.new_folder(),
            1 => self.ask_shortcut(),
            _ => {
                let Some(t) = self.nav.sidebar.places.templates.get(k - 2).cloned() else { return };
                self.create(|dir, name| FileOp::NewFile { dir, name, template: t.template.clone() }, t.stem(), &t.ext);
            }
        }
    }

    fn ask_shortcut(&mut self) {
        let dir = match self.shell_dir() {
            Ok(d) => d,
            Err(e) => return self.status(e),
        };
        let t = self.tab();
        let focused = t.view.as_ref().and_then(|v| v.focus.filter(|&f| v.is_selected(f)));
        let target = focused.and_then(|i| t.entry_location(i)?.to_windows()).map(|p| p.display().into_owned()).unwrap_or_default();
        self.open_pick(Palette::new(Pick::Shortcut(dir), Vec::new(), &target));
    }

    pub fn new_stream(&mut self) {
        let Some(i) = self.rename_target() else { return self.status(S::UiOpsSelectStream.t()) };
        match self.item_paths(&[i]) {
            Ok(mut p) => self.open_pick(Palette::new(Pick::Stream(p.remove(0)), Vec::new(), "")),
            Err(e) => self.status(e),
        }
    }

    fn icon_folder(&self) -> Result<FsPath, &'static str> {
        let folder = self.rename_target().filter(|&i| self.tab().view.as_ref().is_some_and(|v| v.listing().entries[i as usize].is_dir()));
        match folder {
            Some(i) => self.item_paths(&[i]).map(|mut p| p.remove(0)),
            None => self.shell_dir(),
        }
    }

    pub fn ask_folder_icon(&mut self) {
        match self.icon_folder() {
            Ok(folder) => self.open_pick(Palette::new(Pick::FolderIcon(folder), Vec::new(), "")),
            Err(e) => self.status(e),
        }
    }

    pub fn reset_folder_icon(&mut self) {
        match self.icon_folder() {
            Ok(folder) => self.start_op(FileOp::FolderIcon { folder, icon: None, index: 0 }, Origin::Fresh, Then::Nothing),
            Err(e) => self.status(e),
        }
    }

    pub(super) fn submit_pick(&mut self, pick: Pick, text: &str) {
        match pick {
            Pick::FolderIcon(folder) => match icon_location(text) {
                Some((icon, index)) => self.start_op(FileOp::FolderIcon { folder, icon: Some(icon), index }, Origin::Fresh, Then::Nothing),
                None => self.status(S::UiOpsIconHint.t()),
            },
            Pick::Shortcut(dest) => match self.caps.native_path(text) {
                Some(target) => self.start_op(FileOp::Link { items: vec![target], dest }, Origin::Fresh, Then::Rename),
                None => self.status(S::UiOpsTargetHint.t()),
            },
            Pick::Stream(item) => match validate_name(text, true) {
                Ok(name) => self.start_op(FileOp::NewStream { item, name: name.into_bytes() }, Origin::Fresh, Then::Nothing),
                Err(e) => self.status(e),
            },
            p @ (Pick::NewLibrary | Pick::LibraryAdd(_)) => self.submit_library(p, text),
            Pick::Tags(t) => self.tag_input(t, text),
            Pick::Setting(p) => self.setting_input(p, text),
            Pick::Server => self.connect_server(text),
            Pick::Label(drive) => self.relabel_drive(drive, text),
            Pick::SaveLayout => self.save_layout(text),
            Pick::OpenWithCommand(c) => self.chooser_command(*c, text),
            Pick::OpenWith(_) | Pick::Tabs | Pick::SavedFilters | Pick::LoadLayout | Pick::Commands | Pick::NewItem | Pick::Library(_) | Pick::Encoding(_) |Pick::GoTo | Pick::Crumb(_) | Pick::Keys(_) => {}
        }
    }

    pub fn open_terminal(&mut self, admin: bool) {
        let t = self.tab();
        let folders: Vec<u32> = match &t.view {
            Some(v) => self.selected_items().into_iter().filter(|&i| v.listing().entries[i as usize].is_dir()).collect(),
            None => Vec::new(),
        };
        let dirs = match (folders.is_empty(), self.item_paths(&folders)) {
            (false, Ok(paths)) => paths.into_iter().take(MAX_TERMINALS).collect(),
            _ => match t.view_loc.to_windows().filter(|p| p.kind() != PathKind::Computer).or_else(|| self.known_folder(super::sidebar::KnownFolder::Home).and_then(|h| h.to_windows())) {
                Some(dir) => vec![dir],
                None => return self.status(S::UiOpsNoTerminal.t()),
            },
        };
        if folders.len() > MAX_TERMINALS {
            self.status(crate::trn!(P::UiOpsTerminals, folders.len(), shown = MAX_TERMINALS));
        }
        let tab = self.tab().id;
        self.effects.push(Effect::Provider { provider: ProviderId::Local, req: ProviderRequest::Launch { tab, launch: Launch::Terminal { dirs, admin } } });
    }

    fn takes_dropped_files(&self, l: &crate::listing::Listing, i: u32) -> bool {
        let e = &l.entries[i as usize];
        if !self.caps.unix_paths {
            return RUNNABLE.iter().any(|x| l.ext(i).eq_ignore_ascii_case(x));
        }
        !e.is_dir() && e.attrs & attr::STREAM == 0 && (e.attrs & attr::EXEC != 0 || l.ext(i).eq_ignore_ascii_case(b"desktop"))
    }

    pub(super) fn runs_in_terminal(&self, l: &crate::listing::Listing, i: u32) -> bool {
        let e = &l.entries[i as usize];
        let windows_program = !self.caps.unix_paths && !e.is_dir() && e.attrs & attr::STREAM == 0 && WINDOWS_PROGRAMS.iter().any(|x| l.ext(i).eq_ignore_ascii_case(x));
        e.attrs & attr::EXEC != 0 || windows_program
    }

    pub fn run_in_terminal(&mut self) {
        let t = self.tab();
        let program = self.rename_target().filter(|&i| t.view.as_ref().is_some_and(|v| self.runs_in_terminal(v.listing(), i)));
        let Some(i) = program else { return self.status(S::ActionsStatusSelectProgram.t()) };
        match self.item_paths(&[i]) {
            Ok(mut p) => {
                let tab = self.tab().id;
                self.effects.push(Effect::Provider { provider: ProviderId::Local, req: ProviderRequest::Launch { tab, launch: Launch::RunInTerminal { program: p.remove(0) } } });
            }
            Err(e) => self.status(e),
        }
    }

    pub fn undo(&mut self) {
        self.replay(Origin::Undo);
    }

    pub fn redo(&mut self) {
        self.replay(Origin::Redo);
    }

    fn replay(&mut self, origin: Origin) {
        self.replay_entry(origin, None, false);
    }

    fn replay_entry(&mut self, origin: Origin, entry: Option<u64>, recheck_tree: bool) {
        if self.ops.journal.running() {
            if self.ops.journal.entries(origin).is_empty() {
                self.status(if origin == Origin::Undo { S::UiOpsNothingToUndo } else { S::UiOpsNothingToRedo }.t());
            }
            return;
        }
        let op_before = self.ops.next_id;
        match self.ops.journal.begin(origin, entry) {
            None => self.status(if origin == Origin::Undo { S::UiOpsNothingToUndo } else { S::UiOpsNothingToRedo }.t()),
            Some((_, Step::Refuse(msg))) => self.status(msg),
            Some((provider, Step::Run(op))) => self.start_op_on(provider, self.tab().id, op, origin, Then::Select),
            Some((provider, Step::Checked(mut replay))) => {
                if let Some(op) = replay.steps.first().cloned() {
                    replay.recheck_tree = recheck_tree;
                    self.start_op_with_ctl(provider, self.tab().id, op, origin, Then::Select, OpCtl::checked(replay));
                }
            }
        }
        if self.ops.next_id == op_before {
            self.ops.journal.retry();
            self.ops.undo_remaining.clear();
        }
    }

    pub(super) fn undo_history_command(&mut self, id: u64, origin: Origin, group: bool) {
        if self.ops.journal.running() {
            return;
        }
        let blockers = self.ops.journal.blockers(origin, id);
        if blockers.is_empty() {
            self.replay_entry(origin, Some(id), false);
        } else if group && origin == Origin::Undo {
            self.ops.undo_remaining = std::iter::once(id).chain(blockers.into_iter().rev()).collect();
            self.continue_undo_history(false);
        }
    }

    fn continue_undo_history(&mut self, recheck_tree: bool) {
        if let Some(next) = self.ops.undo_remaining.pop() {
            self.replay_entry(Origin::Undo, Some(next), recheck_tree);
        }
    }

    pub(super) fn open_undo_history(&mut self) {
        use super::menu::{Cmd, Mark};
        let menu = self.collect(|a, menu| {
            let journal = &a.ops.journal;
            for (origin, label) in [(Origin::Undo, S::UiOpsUndoHistoryUndo), (Origin::Redo, S::UiOpsUndoHistoryRedo)] {
                menu.separator();
                for entry in journal.entries(origin).iter().rev() {
                    let operation = entry.label();
                    let label = crate::tr!(label, operation = operation);
                    let blocked = journal.blockers(origin, entry.id);
                    let command = Cmd::UndoHistory(entry.id, origin, false);
                    if let Some(last) = blocked.first().and_then(|id| journal.entries(origin).iter().find(|e| e.id == *id)) {
                        let operation = last.label();
                        let reason = crate::tr!(S::UiOpsUndoHistoryBlock, operation = operation);
                        menu.submenu(&label, |menu| {
                            menu.header(&reason);
                            if origin == Origin::Undo {
                                let label = if blocked.len() == 1 { S::UiOpsUndoHistoryBoth } else { S::UiOpsUndoHistoryGroup };
                                let runnable = blocked.iter().all(|id| journal.entries(origin).iter().find(|e| e.id == *id).is_some_and(|e| !matches!(e.step, Step::Refuse(_))));
                                menu.entry(Cmd::UndoHistory(entry.id, origin, true), label.t(), String::new(), Mark::None, false, journal.running() || !runnable);
                            }
                        });
                    } else {
                        menu.entry(command, &label, String::new(), Mark::None, false, journal.running());
                    }
                }
            }
            if journal.depth() == (0, 0) {
                menu.header(S::UiOpsUndoHistoryEmpty.t());
            }
        });
        self.flit_menu(self.layout.list.x, self.layout.list.y, menu);
    }

    pub fn undo_depth(&self) -> (usize, usize) {
        self.ops.journal.depth()
    }

    pub fn properties(&mut self) {
        let t = self.tab();
        let Some(v) = &t.view else { return };
        let items: Vec<u32> = v.selected().collect();
        let paths = if self.shell_bin() {
            match items.is_empty() {
                true => vec![FsPath::recycle_bin()],
                false => self.recycled_paths(&items),
            }
        } else if items.is_empty() {
            match t.view_loc.to_windows().filter(|p| p.kind() != PathKind::Computer) {
                Some(p) => vec![p],
                None => return,
            }
        } else {
            match self.item_paths(&items) {
                Ok(p) => p,
                Err(e) => return self.status(e),
            }
        };
        self.effects.push(Effect::Properties(paths));
    }

    pub fn context_menu(&mut self) {
        let list = self.layout.list;
        let t = self.tab();
        let row = t.view.as_ref().and_then(|v| v.focus.filter(|&f| v.is_selected(f)).and_then(|f| v.row_of(f)));
        let (x, y) = match row.and_then(|r| self.row_rect(r)) {
            Some(c) => ((c.x + self.s(48.0).min(c.w / 2.0)).clamp(list.x, list.right()), c.bottom().clamp(list.y, list.bottom())),
            None => (list.x + self.s(48.0), list.y + self.s(8.0)),
        };
        self.shell_menu(x, y, row.is_some(), false);
    }

    fn shell_menu(&mut self, x: f32, y: f32, on_items: bool, extended: bool) {
        if !on_items && !self.caps.unix_paths && !self.in_bin() && self.archive_view().is_none() {
            if let Ok(dest) = self.shell_dir() {
                self.probe_links(dest, None);
            }
        }
        self.show_shell_menu(x, y, on_items, extended);
    }

    fn show_shell_menu(&mut self, x: f32, y: f32, on_items: bool, extended: bool) {
        if self.in_bin() || self.archive_view().is_some() {
            let menu = self.build_menu(on_items);
            let kind = if on_items { self.selection_kind() } else { super::MenuKind::Background };
            return self.context_flit_menu(x, y, menu, kind);
        }
        let items: Vec<u32> = match (&self.tab().view, on_items) {
            (Some(v), true) => v.selected().collect(),
            _ => Vec::new(),
        };
        let target = match self.shell_target(&items, on_items, extended) {
            Ok(m) => m,
            Err(e) => return self.status(e),
        };
        let menu = self.build_menu(on_items);
        self.push_shell_menu(ShellMenu { x, y, menu, ..target });
    }

    pub(super) fn shell_verb(&mut self, verb: &str, now: Instant) {
        match verb.to_ascii_lowercase().as_str() {
            "back" => self.go_history(false, now),
            "forward" => self.go_history(true, now),
            "rename" => self.rename_focused(),
            "copy" => self.copy_selection(),
            "cut" => self.cut_selection(),
            "paste" => self.paste(),
            "delete" => self.trash_selection(),
            "newfolder" => self.new_folder(),
            _ => {}
        }
    }

    pub(super) fn op_done(&mut self, tab: TabId, id: OpId, error: Option<crate::provider::OpError>, report: Option<OpReport>, now: Instant) {
        let error_kind = error.as_ref().map(|e| e.kind);
        let cancelled = error_kind == Some(crate::provider::PlatformErrorKind::Cancelled);
        let error = error.map(|e| crate::redact::passwords(&crate::i18n::localize(&e.message)).into_owned());
        self.job_done(id, error.as_deref().filter(|_| !cancelled), cancelled, now);
        self.reload_released_preview();
        if report.is_some() {
            self.check_places();
            self.effects.push(Effect::RefreshPlaces);
        }
        let pending = self.ops.pending.iter().position(|p| p.id == id && p.tab == tab).map(|k| self.ops.pending.swap_remove(k));
        let k = self.tabs.items.iter().position(|t| t.id == tab);
        let retry = self.ops.retry.take_if(|r| r.id == id && r.tab == tab);
        if let Some(message) = error.as_ref().filter(|_| !cancelled) {
            if retry.is_some_and(|r| self.rename_again(r, message)) {
                self.dirty = true;
            } else {
                let k = k.unwrap_or(self.tabs.cur);
                self.tabs.items[k].error = Some(message.clone());
                self.dirty |= k == self.tabs.cur;
            }
        }
        let any_done = report.as_ref().is_some_and(|r| !r.items.is_empty());
        let unwatched = |l: &Location| matches!(l, Location::Wsl { .. }) || l.path().kind() == PathKind::Unc || archive::split(l.path()).is_some();
        if let Some(k) = k.filter(|&k| (error.is_none() || any_done) && unwatched(self.tabs.items[k].location())) {
            self.reload(k, now);
        }
        if let Some(r) = &report {
            let provider = pending.as_ref().map_or(ProviderId::Local, |p| p.provider.clone());
            self.tags_moved(&provider, r);
        }
        let Some(p) = pending else { return };
        let Some(report) = report else {
            if p.origin != Origin::Fresh {
                self.ops.journal.retry();
                self.ops.undo_remaining.clear();
            }
            return;
        };
        if error.is_none() {
            if let FileOp::Trash(items) = &report.op {
                let owner = k.unwrap_or(self.tabs.cur);
                self.tabs.items[owner].undo = Some((items.len(), now));
                self.arm_timer(now, now + UNDO_BAR_FOR);
                self.dirty = true;
            }
        }
        let incomplete_removal = p.optimistic_removal && (error.is_some() || report.items.len() != removal_count(&report.op));
        let composite = p.ctl.replay().is_some_and(|plan| plan.steps.len() > 1);
        if incomplete_removal || (composite && any_done) {
            if let Some(k) = k {
                self.reload(k, now);
            }
        } else if !p.optimistic_removal {
            if let (FileOp::Trash(_) | FileOp::Delete(_), Some(k)) = (&report.op, k) {
                self.drop_gone(k, &p.provider, &report);
            }
        }
        if !matches!(p.then, Then::Clipboard | Then::Drag) {
            self.ops.journal.record(p.origin, p.provider.clone(), &report);
        }
        if p.origin == Origin::Undo {
            if error.is_none() && any_done {
                self.continue_undo_history(true);
            } else {
                self.ops.undo_remaining.clear();
            }
        }
        self.each_pane(|a, k| {
            let t = &mut a.tabs.items[k];
            if t.view_loc.provider() != p.provider {
                return;
            }
            let root = t.view_loc.path().clone();
            let Some(v) = &mut t.view else { return };
            let changes_source = matches!(report.op, FileOp::Move { .. } | FileOp::MoveTo(_) | FileOp::Rename { .. } | FileOp::Trash(_) | FileOp::Delete(_) | FileOp::Restore(_) | FileOp::Flatten(_));
            for path in report.items.iter().flat_map(|(from, to)| std::iter::once(to).chain(changes_source.then_some(from))) {
                let mut child = path.clone();
                while let Some(parent) = child.parent() {
                    if parent.same(&root) {
                        if let Some(i) = v.find(child.file_name()) {
                            v.invalidate_dir_stats(i);
                        }
                        break;
                    }
                    if !parent.is_inside(&root) {
                        break;
                    }
                    child = parent;
                }
            }
        });
        self.compute_dir_sizes();
        if self.archive_then(tab, &p.then, &report, error_kind, now) {
            return;
        }
        if let (Then::Fill(items), Some((_, folder))) = (&p.then, report.items.first()) {
            let folder = Location::on(&p.provider, folder.clone());
            let reveal = Then::Reveal(folder.file_name().to_vec());
            return match folder.to_windows() {
                Some(dest) => self.start_op_for(tab, FileOp::Move { items: items.clone(), dest }, Origin::Fresh, reveal),
                None => self.status(S::UiOpsNamesNotShell.t()),
            };
        }
        if let (FileOp::NewStream { item, name }, Some(k), None) = (&report.op, k, &error) {
            self.tabs.items[k].error = Some(crate::tr!(S::UiOpsStreamCreated, name = format_args!("{}:{}", item.display(), crate::name::display(name))));
            self.dirty |= k == self.tabs.cur;
        }
        if let (FileOp::FolderIcon { folder, icon, .. }, None) = (&report.op, &error) {
            self.icon_epoch += 1;
            let done = if icon.is_some() { S::UiOpsIconChanged } else { S::UiOpsIconRestored };
            self.tabs.items[k.unwrap_or(self.tabs.cur)].error = Some(crate::tr!(done, folder = folder.display()));
            self.dirty = true;
        }
        let Some(k) = k.filter(|_| p.then != Then::Nothing && !matches!(report.op, FileOp::Trash(_))) else { return };
        let dir = self.tabs.items[k].view_loc.clone();
        let names: Vec<Vec<u8>> = match &p.then {
            Then::Reveal(name) => vec![name.clone()],
            _ => report
                .items
                .iter()
                .map(|(_, to)| Location::on(&p.provider, to.clone()))
                .filter(|to| to.parent().is_some_and(|p| same_location(&p, &dir)))
                .map(|to| to.file_name().to_vec())
                .collect(),
        };
        if !names.is_empty() {
            self.ops.reveal.retain(|r| r.tab != tab);
            let rename = matches!(p.then, Then::Rename | Then::Reveal(_));
            self.ops.reveal.push(Reveal { tab, dir, names, rename, started: false, until: now + REVEAL_FOR });
            self.dirty |= k == self.tabs.cur;
        }
    }

    fn drop_gone(&mut self, k: usize, provider: &ProviderId, report: &OpReport) {
        let t = &mut self.tabs.items[k];
        let dir = &t.view_loc;
        let gone: Vec<DirChange> = report
            .items
            .iter()
            .map(|(from, _)| Location::on(provider, from.clone()))
            .filter(|from| from.parent().is_some_and(|p| same_location(&p, dir)))
            .map(|from| DirChange { label: crate::listing::Label::Keep, kind: ChangeKind::Removed, name: from.file_name().to_vec(), meta: None })
            .collect();
        t.pending_diffs.extend(gone);
        self.dirty |= t.apply_diffs() && k == self.tabs.cur;
    }

    pub(super) fn ops_frame(&mut self, now: Instant) {
        if let Some(r) = &self.ops.rename {
            let t = self.tab();
            let alive = t.id == r.tab && t.view.as_ref().is_some_and(|v| v.snap().gen == r.gen && v.is_visible(r.idx));
            if !alive {
                self.ops.rename = None;
            }
        }
        self.ops.reveal.retain(|r| r.until > now);
        let cur = self.tab().id;
        let Some(k) = self.ops.reveal.iter().position(|r| r.tab == cur) else { return };
        let App { ops, tabs: super::tabbar::Tabs { items: tabs, cur, .. }, .. } = self;
        let (r, t) = (&mut ops.reveal[k], &mut tabs[*cur]);
        if !same_location(&r.dir, &t.view_loc) {
            ops.reveal.swap_remove(k);
            return;
        }
        let Some(v) = t.view.as_mut() else { return };
        let mut found = Vec::new();
        r.names.retain(|n| match v.find(n).filter(|&i| v.is_visible(i)) {
            Some(i) => {
                found.push(i);
                false
            }
            None => true,
        });
        let Some(&first) = found.first() else { return };
        if !r.started {
            v.clear_selection();
            r.started = true;
        }
        for &i in &found {
            v.set_selected(i, true);
        }
        v.focus = Some(first);
        v.anchor = Some(first);
        let rename = r.rename;
        if r.names.is_empty() {
            ops.reveal.swap_remove(k);
        }
        self.reveal_focus(false);
        self.dirty = true;
        if rename {
            self.begin_rename(first, false);
        }
    }

    pub(super) fn draw_ops<B: TextBackend>(&mut self, g: &mut Gfx<B>, th: &Theme) {
        let list = self.layout.list;
        let row = match self.drop_spot() {
            Some(Spot::Row(row)) => self.row_rect(row).map(|c| self.item_parts(c).body),
            _ => None,
        };
        if let Some(r) = row {
            g.dl.set_clip(Some(list));
            g.dl.round_rect(r, th.selection, self.s(4.0));
            g.dl.outline(r, th.accent, self.s(4.0), self.s(1.0).max(1.0));
            g.dl.set_clip(None);
        }
        self.draw_rename(g, th);
    }
}

#[cfg(test)]
#[path = "../../test/ui/ops.rs"]
mod tests;
