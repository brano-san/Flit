use super::edit::{EditAction, LineEdit};
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
use crate::provider::{FileOp, GenToken, Launch, OpCtl, OpId, OpReport, ProviderRequest, SnapshotSlot, TabId, Template};
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

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct NewTemplate {
    pub label: String,
    pub ext: String,
    pub stem: String,
    pub template: Template,
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
    TrashArchives(Vec<FsPath>),
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
    click: Option<(f32, bool)>,
    selecting: bool,
    batch: bool,
    confirmed: Option<String>,
    hint: Option<String>,
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
}

struct Asked {
    paths: Vec<FsPath>,
    dest: FsPath,
}

#[derive(Default)]
pub(super) struct Ops {
    next_id: OpId,
    pending: Vec<Pending>,
    journal: Journal,
    reveal: Vec<Reveal>,
    rename: Option<Rename>,
    retry: Option<Retry>,
    press: Option<Press>,
    pub(super) dropping: Option<Dropping>,
    right_drag: Option<Vec<FsPath>>,
    asked: Option<Asked>,
    slow: Option<Slow>,
    cut: Option<Cut>,
    bin: Option<BinProbe>,
    ids: u32,
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
    (1..).map(|k| if k == 1 { base.to_owned() } else { format!("{base} ({k})") }).find(|n| !taken(n)).unwrap()
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
        self.tab_mut().error = Some(crate::i18n::localize(&msg).into_owned());
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
            None => items.iter().map(|&i| t.entry_location(i).to_windows()).collect::<Option<Vec<_>>>().ok_or(S::UiOpsNamesNotShell.t()),
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
        let Some((provider, op)) = route(provider, op, self.caps.unix_paths) else { return self.status(MIXED_PATHS.t()) };
        self.ops.next_id += 1;
        let id = self.ops.next_id;
        let ctl = OpCtl::new(self.conflict_default);
        self.track_job(id, &provider, &op, &ctl);
        let optimistic_removal = provider == ProviderId::Local
            && match &op {
                FileOp::Trash(items) | FileOp::Delete(items) => self.tabs.iter().find(|t| t.id == tab).is_some_and(|t| {
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
        // Explorer removes a folder from its view as soon as deletion starts.
        // Do the same here, so a long-running delete cannot be opened or
        // started a second time from the still-visible source list.
        if optimistic_removal {
            let sources = match &op {
                FileOp::Trash(items) | FileOp::Delete(items) => items,
                _ => unreachable!(),
            };
            let report = OpReport { op: op.clone(), items: sources.iter().cloned().map(|path| (path.clone(), path)).collect() };
            if let Some(k) = self.tabs.iter().position(|t| t.id == tab) {
                self.drop_gone(k, &provider, &report);
            }
        }
        self.effects.push(Effect::Provider { provider, req: ProviderRequest::Op { tab, id, op, ctl } });
    }

    pub(super) fn start_confirmed(&mut self, provider: ProviderId, op: FileOp) {
        self.start_op_on(provider, self.tab().id, op, Origin::Fresh, Then::Nothing);
    }

    fn delete_or_ask(&mut self, provider: ProviderId, op: FileOp) {
        let ask = match self.confirm_delete {
            ConfirmDelete::Always => true,
            ConfirmDelete::Permanent => !matches!(op, FileOp::Trash(_)),
            ConfirmDelete::Never => false,
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

    pub(super) fn cancel_ops(&mut self) {
        for p in &self.ops.pending {
            p.ctl.cancel();
        }
    }

    pub fn ops_pending(&self) -> bool {
        !self.ops.pending.is_empty()
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
            Location::Wsl { .. } => Ok((t.view_loc.provider(), items.iter().map(|&i| t.entry_location(i).path().clone()).collect())),
            _ => self.item_paths(&items).map(|p| (ProviderId::Local, p)),
        };
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
        self.create(|dir, name| FileOp::NewFile { dir, name, template: Template::Empty }, S::UiOpsNewFileName.t(), "");
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
        let at = order.iter().position(|&id| id == self.panels.active).unwrap_or(0);
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
        self.create(|dir, name| FileOp::NewFolder { dir, name }, S::UiOpsNewFolderName.t(), "");
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
        let name = self.free_name(S::UiOpsNewFolderName.t(), "").into_bytes();
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
        let folder_keys = self.keymap.text_of("file.new_folder");
        let mut items = vec![(S::MenuFolder.t().to_owned(), folder_keys), (S::MenuShortcut.t().to_owned(), String::new())];
        items.extend(self.nav.sidebar.places.templates.iter().map(|t| (t.label.clone(), t.ext.clone())));
        self.open_pick(Palette::new(Pick::NewItem, items, ""));
    }

    pub(super) fn new_item(&mut self, k: usize) {
        match k {
            0 => self.new_folder(),
            1 => self.ask_shortcut(),
            _ => {
                let Some(t) = self.nav.sidebar.places.templates.get(k - 2).cloned() else { return };
                self.create(|dir, name| FileOp::NewFile { dir, name, template: t.template }, &t.stem, &t.ext);
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
        let target = focused.and_then(|i| t.entry_location(i).to_windows()).map(|p| p.display().into_owned()).unwrap_or_default();
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
            Pick::SaveLayout => self.save_layout(text),
            Pick::OpenWithCommand(c) => self.chooser_command(*c, text),
            Pick::OpenWith(_) | Pick::Tabs | Pick::LoadLayout | Pick::Commands | Pick::NewItem | Pick::Library(_) | Pick::Encoding(_) |Pick::GoTo | Pick::Crumb(_) | Pick::Keys(_) => {}
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
            _ => match t.view_loc.to_windows().filter(|p| p.kind() != PathKind::Computer) {
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

    pub fn run_in_terminal(&mut self) {
        let t = self.tab();
        let program = self.rename_target().filter(|&i| t.view.as_ref().is_some_and(|v| v.listing().entries[i as usize].attrs & attr::EXEC != 0));
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
        match self.ops.journal.take(origin) {
            None => self.status(if origin == Origin::Undo { S::UiOpsNothingToUndo } else { S::UiOpsNothingToRedo }.t()),
            Some((_, Step::Refuse(msg))) => self.status(msg),
            Some((provider, Step::Run(op))) => self.start_op_on(provider, self.tab().id, op, origin, Then::Select),
        }
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

    pub(super) fn shell_verb(&mut self, verb: &str) {
        match verb.to_ascii_lowercase().as_str() {
            "rename" => self.rename_focused(),
            "copy" => self.copy_selection(),
            "cut" => self.cut_selection(),
            "paste" => self.paste(),
            "delete" => self.trash_selection(),
            "newfolder" => self.new_folder(),
            _ => {}
        }
    }

    pub(super) fn op_done(&mut self, tab: TabId, id: OpId, error: Option<String>, report: Option<OpReport>, now: Instant) {
        let cancelled = error.as_deref().is_some_and(super::jobs::cancelled_error);
        self.job_done(id, error.as_deref().filter(|_| !cancelled), cancelled, now);
        if report.is_some() {
            self.check_places();
            self.effects.push(Effect::RefreshPlaces);
        }
        let pending = self.ops.pending.iter().position(|p| p.id == id && p.tab == tab).map(|k| self.ops.pending.swap_remove(k));
        let k = self.tabs.iter().position(|t| t.id == tab);
        let retry = self.ops.retry.take_if(|r| r.id == id && r.tab == tab);
        if let Some(message) = error.as_ref().filter(|_| !cancelled) {
            if retry.is_some_and(|r| self.rename_again(r, message)) {
                self.dirty = true;
            } else {
                let k = k.unwrap_or(self.cur);
                self.tabs[k].error = Some(message.clone());
                self.dirty |= k == self.cur;
            }
        }
        let any_done = report.as_ref().is_some_and(|r| !r.items.is_empty());
        let unwatched = |l: &Location| matches!(l, Location::Wsl { .. }) || l.path().kind() == PathKind::Unc || archive::split(l.path()).is_some();
        if let Some(k) = k.filter(|&k| (error.is_none() || any_done) && unwatched(self.tabs[k].location())) {
            self.reload(k, now);
        }
        if let Some(r) = &report {
            let provider = pending.as_ref().map_or(ProviderId::Local, |p| p.provider.clone());
            self.tags_moved(&provider, r);
        }
        let (Some(report), Some(p)) = (report, pending) else { return };
        let incomplete_removal = p.optimistic_removal && (error.is_some() || report.items.len() != removal_count(&report.op));
        if incomplete_removal {
            if let Some(k) = k {
                self.reload(k, now);
            }
        } else if !p.optimistic_removal {
            if let (FileOp::Trash(_) | FileOp::Delete(_), Some(k)) = (&report.op, k) {
                self.drop_gone(k, &p.provider, &report);
            }
        }
        self.ops.journal.record(p.origin, p.provider.clone(), &report);
        self.each_pane(|a, k| {
            let t = &mut a.tabs[k];
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
        if self.archive_then(tab, &p.then, &report, error.as_deref(), now) {
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
            self.tabs[k].error = Some(crate::tr!(S::UiOpsStreamCreated, name = format_args!("{}:{}", item.display(), crate::name::display(name))));
            self.dirty |= k == self.cur;
        }
        if let (FileOp::FolderIcon { folder, icon, .. }, None) = (&report.op, &error) {
            self.icon_epoch += 1;
            let done = if icon.is_some() { S::UiOpsIconChanged } else { S::UiOpsIconRestored };
            self.tabs[k.unwrap_or(self.cur)].error = Some(crate::tr!(done, folder = folder.display()));
            self.dirty = true;
        }
        let Some(k) = k.filter(|_| p.then != Then::Nothing && !matches!(report.op, FileOp::Trash(_))) else { return };
        let dir = self.tabs[k].view_loc.clone();
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
            self.dirty |= k == self.cur;
        }
    }

    fn drop_gone(&mut self, k: usize, provider: &ProviderId, report: &OpReport) {
        let t = &mut self.tabs[k];
        let dir = &t.view_loc;
        let gone: Vec<DirChange> = report
            .items
            .iter()
            .map(|(from, _)| Location::on(provider, from.clone()))
            .filter(|from| from.parent().is_some_and(|p| same_location(&p, dir)))
            .map(|from| DirChange { kind: ChangeKind::Removed, name: from.file_name().to_vec(), meta: None })
            .collect();
        t.pending_diffs.extend(gone);
        self.dirty |= t.apply_diffs() && k == self.cur;
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
        let App { ops, tabs, cur, .. } = self;
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

    pub(super) fn rename_target(&self) -> Option<u32> {
        let t = self.tab();
        let v = t.view.as_ref().filter(|_| !matches!(t.view_loc.path().kind(), PathKind::Computer | PathKind::RecycleBin) && self.archive_view().is_none())?;
        v.focus.filter(|&f| v.is_visible(f)).or_else(|| if v.selected_count() == 1 { v.selected().next() } else { None })
    }

    pub fn rename_focused(&mut self) {
        let Some(i) = self.rename_target() else { return };
        self.ops.ids = 0;
        let batch = self.tab().view.as_ref().is_some_and(|v| v.selected_count() > 1 && v.is_selected(i));
        self.begin_rename(i, batch);
    }

    fn begin_rename(&mut self, idx: u32, batch: bool) {
        let t = &mut self.tabs[self.cur];
        let Some(v) = t.view.as_mut() else { return };
        if v.listing().entries[idx as usize].attrs & attr::STREAM != 0 {
            t.error = Some(STREAMS_VIEW_ONLY.t().into());
            return;
        }
        let Ok(name) = std::str::from_utf8(v.listing().name(idx)) else {
            t.error = Some(S::UiOpsNameNotEditable.t().into());
            return;
        };
        let mut edit = LineEdit::default();
        edit.set(name);
        edit.anchor = 0;
        edit.caret = base_name_len(name, v.listing().entries[idx as usize].is_dir());
        if !batch {
            v.clear_selection();
            v.set_selected(idx, true);
        }
        v.focus = Some(idx);
        v.anchor = Some(idx);
        let gen = v.snap().gen;
        t.error = None;
        let tab = t.id;
        self.reveal_focus(false);
        self.ops.rename = Some(Rename { tab, gen, idx, edit, scroll: 0.0, rect: Rect::default(), click: None, selecting: false, batch, confirmed: None, hint: None });
        self.ops.slow = None;
        self.focus = Focus::List;
        self.dirty = true;
    }

    pub fn is_renaming(&self) -> bool {
        self.ops.rename.is_some()
    }

    pub(super) fn rename_insert(&mut self, what: Insert) {
        let Some(r) = &self.ops.rename else { return };
        let t = self.tab();
        let entry = t.view.as_ref().filter(|v| t.id == r.tab && v.snap().gen == r.gen).map(|v| v.listing().entries[r.idx as usize]);
        let off = self.utc_offset_min;
        let text = match what {
            Insert::Id => {
                self.ops.ids += 1;
                self.ops.ids.to_string()
            }
            Insert::Today => iso_date(crate::fmt::now_ticks(), off),
            Insert::Created => match entry.filter(|e| e.created > 0) {
                Some(e) => iso_date(e.created, off),
                None => return,
            },
            Insert::Modified => match entry.filter(|e| e.modified > 0) {
                Some(e) => iso_date(e.modified, off),
                None => return,
            },
        };
        if let Some(r) = &mut self.ops.rename {
            r.edit.insert(&text);
        }
        self.dirty = true;
    }

    pub(super) fn change_case(&mut self, upper: bool) {
        let edit = match self.ops.rename.as_mut() {
            Some(r) => Some(&mut r.edit),
            None => None,
        };
        let edit = match edit {
            Some(e) => Some(e),
            None if self.focus == Focus::Filter => self.tabs[self.cur].filter.as_mut().map(|f| &mut f.edit),
            None => self.inspector_field(),
        };
        let Some(edit) = edit else { return };
        if edit.change_case(upper) {
            if self.focus == Focus::Filter && self.ops.rename.is_none() {
                self.tab_mut().refilter_view();
            }
            self.dirty = true;
        }
    }

    fn rename_menu(&mut self, x: f32, y: f32) {
        let menu = self.collect(|_, m| {
            for (id, label) in [
                ("rename.insert_id", S::ActionsRenameInsertId),
                ("rename.insert_date", S::ActionsRenameInsertDate),
                ("rename.insert_created", S::ActionsRenameInsertCreated),
                ("rename.insert_modified", S::ActionsRenameInsertModified),
            ] {
                m.action(id, label.t());
            }
            m.separator();
            m.action("text.uppercase", S::ActionsTextUppercase.t());
            m.action("text.lowercase", S::ActionsTextLowercase.t());
        });
        self.flit_menu(x, y, menu);
    }

    pub fn rename_text(&self) -> Option<&str> {
        self.ops.rename.as_ref().map(|r| r.edit.text.as_str())
    }

    pub fn rename_hint(&self) -> Option<&str> {
        self.ops.rename.as_ref().and_then(|r| r.hint.as_deref())
    }

    fn names_like_windows(&self) -> bool {
        windows_names(self.tab().view_loc.path(), &self.nav.sidebar.places.drives)
    }

    fn keep_renaming(&mut self, mut r: Rename, hint: impl Into<String>) -> bool {
        r.hint = Some(hint.into());
        self.ops.rename = Some(r);
        self.dirty = true;
        false
    }

    fn name_taken(&self, idx: u32, name: &[u8], nocase: bool) -> bool {
        let t = self.tab();
        let Some(v) = &t.view else { return false };
        let l = v.listing();
        let folded = nocase.then(|| crate::name::display(name).to_lowercase());
        let dir = t.entry_dir(idx);
        (0..l.len() as u32)
            .filter(|&i| i != idx && l.entries[i as usize].attrs & attr::DELETED == 0 && l.folder(i) == l.folder(idx))
            .any(|i| same_name(l.name(i), name, folded.as_deref()) && t.entry_dir(i) == dir)
    }

    fn commit_rename(&mut self) -> bool {
        let Some(mut r) = self.ops.rename.take() else { return true };
        let windows = self.names_like_windows();
        let t = self.tab();
        let Some(v) = t.view.as_ref().filter(|v| t.id == r.tab && v.snap().gen == r.gen) else { return true };
        let old = v.listing().name(r.idx).to_vec();
        let is_dir = v.listing().entries[r.idx as usize].is_dir();
        let name = match validate_name(&r.edit.text, windows) {
            Ok(n) => n,
            Err(e) => return self.keep_renaming(r, e),
        };
        if name.as_bytes() == old {
            return true;
        }
        if r.batch && v.selected_count() > 1 {
            let ext = extension(&old, is_dir);
            let base = match name.len().checked_sub(ext.len()) {
                Some(n) if n > 0 && name.as_bytes()[n..].eq_ignore_ascii_case(ext) => &name[..n],
                _ => &name,
            };
            self.batch_rename(base);
            return true;
        }
        if self.name_taken(r.idx, name.as_bytes(), windows) {
            return self.keep_renaming(r, S::UiOpsNameTaken.t());
        }
        let old_str = String::from_utf8_lossy(&old);
        if !is_dir && self.warn_extension_change && r.confirmed.as_deref() != Some(&name) && changes_extension(&old_str, &name) {
            let msg = crate::tr!(S::UiOpsExtensionWarning, name = old_str);
            r.confirmed = Some(name);
            self.ops.rename = Some(r);
            self.status(msg);
            return false;
        }
        let dir = t.entry_dir(r.idx);
        let (item, target) = (op_path(&dir.join(&old)), op_path(&dir.join(name.as_bytes())));
        let (Some(item), Some(target)) = (item, target) else {
            self.status(S::UiOpsNameNotShell.t());
            return true;
        };
        self.tab_mut().error = None;
        let (tab, before) = (self.tab().id, self.ops.next_id);
        self.start_op_on(dir.provider(), tab, FileOp::Rename { item, name: target.file_name().to_vec() }, Origin::Fresh, Then::Select);
        if self.ops.next_id != before {
            let dir = self.tab().view_loc.clone();
            self.ops.retry = Some(Retry { id: self.ops.next_id, tab, dir, old, text: r.edit.text });
        }
        true
    }

    fn rename_again(&mut self, retry: Retry, error: &str) -> bool {
        if self.ops.rename.is_some() || self.tab().id != retry.tab || !same_location(&retry.dir, &self.tab().view_loc) {
            return false;
        }
        let Some(v) = self.tab_mut().view.as_mut() else { return false };
        let Some(i) = v.find(&retry.old).filter(|&i| v.is_visible(i)) else { return false };
        let dir = v.listing().entries[i as usize].is_dir();
        self.begin_rename(i, false);
        let Some(r) = self.ops.rename.as_mut() else { return false };
        r.edit.set(&retry.text);
        r.edit.anchor = 0;
        r.edit.caret = base_name_len(&retry.text, dir);
        r.hint = Some(error.to_owned());
        true
    }

    fn batch_rename(&mut self, base: &str) {
        let t = self.tab();
        let Some(v) = &t.view else { return };
        let l = v.listing();
        let live = |i: &u32| l.entries[*i as usize].attrs & attr::DELETED == 0;
        let taken: FxHashSet<Vec<u8>> = (0..l.len() as u32).filter(live).filter(|&i| !v.is_selected(i)).map(|i| l.name(i).to_ascii_lowercase()).collect();
        let items: Vec<(&[u8], bool)> = v.rows().iter().filter(|&&i| v.is_selected(i)).map(|&i| (l.name(i), l.entries[i as usize].is_dir())).collect();
        let names = batch_names(base, &items, |n| taken.contains(n));
        let dirs: Vec<Location> = v.rows().iter().filter(|&&i| v.is_selected(i)).map(|&i| t.entry_dir(i)).collect();
        let renamed = items.iter().zip(&names).zip(&dirs).filter(|(((old, _), new), _)| *old != new.as_slice());
        let pairs: Option<Vec<(FsPath, FsPath)>> = renamed.map(|(((old, _), new), dir)| Some((op_path(&dir.join(old))?, op_path(&dir.join(new))?))).collect();
        let (provider, tab) = (dirs.first().map_or(ProviderId::Local, Location::provider), t.id);
        match pairs {
            Some(p) if p.is_empty() => {}
            Some(p) => {
                self.tab_mut().error = None;
                self.start_op_on(provider, tab, FileOp::MoveTo(p), Origin::Fresh, Then::Select);
            }
            None => self.status(S::UiOpsNamesNotShell.t()),
        }
    }

    pub(super) fn ops_key(&mut self, key: Key, m: Mods) -> bool {
        self.ops.slow = None;
        if self.ops.rename.is_some() && key == Key::Tab && !m.ctrl && !m.alt {
            self.rename_step(m.shift);
            return true;
        }
        if let Some(r) = self.ops.rename.as_ref().filter(|_| key == Key::Apps || (key == Key::F(10) && m.shift && !m.ctrl && !m.alt)) {
            let row = self.tab().view.as_ref().and_then(|v| v.row_of(r.idx));
            let rect = row.and_then(|row| self.row_rect(row)).map_or(r.rect, |c| self.name_edit_rect(c));
            self.rename_menu(rect.x, rect.bottom());
            return true;
        }
        let Some(action) = self.ops.rename.as_mut().map(|r| r.edit.key(key, m)) else { return false };
        match action {
            EditAction::Submit => {
                self.commit_rename();
            }
            EditAction::Cancel => self.ops.rename = None,
            EditAction::Copy(s) => self.effects.push(Effect::SetClipboard(s)),
            EditAction::Paste => self.effects.push(Effect::Paste),
            EditAction::Changed => {
                if let Some(r) = &mut self.ops.rename {
                    r.hint = None;
                }
            }
            EditAction::None => {}
        }
        true
    }

    fn rename_step(&mut self, back: bool) {
        let Some(idx) = self.ops.rename.as_ref().filter(|r| !r.batch).map(|r| r.idx) else {
            self.commit_rename();
            return;
        };
        let next = self.tab().view.as_ref().and_then(|v| {
            let row = v.row_of(idx)?;
            let to = if back { row.checked_sub(1)? } else { row + 1 };
            v.rows().get(to).copied()
        });
        if self.commit_rename() {
            if let Some(i) = next {
                self.begin_rename(i, false);
            }
        }
    }

    pub(super) fn ops_char(&mut self, c: char) -> bool {
        if self.ops.rename.is_none() {
            return false;
        }
        let windows = self.names_like_windows();
        let Some(r) = &mut self.ops.rename else { return false };
        if !bad_name_char(c, windows) {
            let mut b = [0; 4];
            r.edit.insert(c.encode_utf8(&mut b));
            r.hint = None;
        } else if !c.is_control() {
            r.hint = Some(bad_chars_hint(windows).into());
            self.dirty = true;
        }
        true
    }

    pub(super) fn ops_paste(&mut self, text: &str) -> bool {
        if self.ops.rename.is_none() {
            return false;
        }
        let windows = self.names_like_windows();
        let Some(r) = &mut self.ops.rename else { return false };
        let line = text.lines().next().unwrap_or("");
        let kept: String = line.chars().filter(|&c| !bad_name_char(c, windows)).collect();
        r.edit.insert(&kept);
        r.hint = line.chars().any(|c| bad_name_char(c, windows) && !c.is_control()).then(|| bad_chars_hint(windows).into());
        self.dirty = true;
        true
    }

    pub(super) fn ops_mouse_down(&mut self, x: f32, y: f32, button: MouseButton, clicks: u8) -> bool {
        self.ops.press = None;
        self.ops.slow = None;
        self.ops.right_drag = None;
        let Some(r) = &mut self.ops.rename else { return false };
        if r.rect.contains(x, y) {
            if button == MouseButton::Right {
                self.rename_menu(x, y);
                return true;
            }
            if button == MouseButton::Left {
                if clicks >= 2 {
                    r.edit.select_all();
                } else {
                    r.click = Some((x, self.mods.shift));
                    r.selecting = true;
                }
            }
            return true;
        }
        !self.commit_rename()
    }

    pub(super) fn ops_mouse_move(&mut self, x: f32, y: f32) -> bool {
        if let Some(r) = &mut self.ops.rename {
            if r.selecting {
                r.click = Some((x, true));
                self.dirty = true;
                return true;
            }
            if r.rect.contains(x, y) {
                self.set_cursor(CursorIcon::Text);
                return true;
            }
        }
        let Some(p) = &self.ops.press else { return false };
        if (x - p.x).abs().max((y - p.y).abs()) < self.s(DRAG_START) {
            return false;
        }
        let right = p.right;
        self.ops.press = None;
        let items = self.selected_items();
        let Ok(paths) = self.item_paths(&items) else { return false };
        if paths.is_empty() || self.archive_view().is_some() {
            return false;
        }
        self.pressed = Hit::None;
        self.drag = Drag::None;
        self.hover = Hit::None;
        self.ops.right_drag = right.then(|| paths.clone());
        self.effects.push(Effect::StartDrag(paths));
        true
    }

    pub(super) fn ops_mouse_up(&mut self, x: f32, y: f32, button: MouseButton, now: Instant) {
        if let Some(r) = &mut self.ops.rename {
            r.selecting = false;
        }
        if button == MouseButton::Right && self.drag == Drag::None && self.ops.rename.is_none() && self.ops.right_drag.is_none() {
            match self.hit(x, y) {
                Hit::Row(_) => self.shell_menu(x, y, true, self.mods.shift),
                Hit::ListEmpty | Hit::Group(_) => self.shell_menu(x, y, false, self.mods.shift),
                Hit::Header(c) => self.header_menu(x, y, self.layout.columns.get(c).copied()),
                Hit::ColSep(_) => self.header_menu(x, y, None),
                h @ (Hit::Side(_) | Hit::SidePin(_) | Hit::SideHeader(_) | Hit::SideEmpty) => self.side_menu(x, y, h),
                h @ (Hit::HomeItem(_) | Hit::HomeEmpty) => self.home_menu(x, y, h),
                _ => {}
            }
        }
        let Some(p) = self.ops.press.take().filter(|p| (p.deferred || p.open || p.toggle) && button == MouseButton::Left) else { return };
        if self.hit(x, y) != Hit::Row(p.row) {
            return;
        }
        if p.toggle {
            if let Some(v) = &mut self.tabs[self.cur].view {
                if let Some(&i) = v.rows().get(p.row) {
                    v.set_selected(i, false);
                }
            }
            self.dirty = true;
            return;
        }
        if !p.deferred {
            let Some(i) = self.tab().view.as_ref().and_then(|v| v.rows().get(p.row).copied()) else { return };
            self.clicked_open = true;
            return self.open_entry(i, now);
        }
        let (tab, wait) = (self.tab().id, self.double_click);
        let Some(v) = &mut self.tabs[self.cur].view else { return };
        let Some(&i) = v.rows().get(p.row) else { return };
        v.clear_selection();
        v.set_selected(i, true);
        v.anchor = Some(i);
        v.focus = Some(i);
        let gen = v.snap().gen;
        if self.single_click_opens(i) {
            self.clicked_open = true;
            return self.open_entry(i, now);
        }
        if p.slow {
            self.ops.slow = Some(Slow { tab, gen, idx: i, at: now + wait });
            self.arm_timer(now, now + wait);
        }
    }

    pub(super) fn ops_idle(&self) -> bool {
        self.ops.rename.is_none() && self.ops.press.is_none() && self.ops.dropping.is_none()
    }

    pub(super) fn ops_timer_cancel(&mut self) {
        self.ops.slow = None;
    }

    pub(super) fn ops_timer(&mut self, now: Instant) {
        let Some(s) = self.ops.slow.take() else { return };
        if now + TIMER_SLACK < s.at {
            self.arm_timer(now, s.at);
            self.ops.slow = Some(s);
            return;
        }
        let t = self.tab();
        let still = t.id == s.tab && t.view.as_ref().is_some_and(|v| v.snap().gen == s.gen && v.selected_count() == 1 && v.is_selected(s.idx));
        if still && self.rename_target() == Some(s.idx) {
            self.begin_rename(s.idx, false);
        }
    }

    pub(super) fn open_on_release(&mut self, row: usize) -> bool {
        let Some(p) = self.ops.press.as_mut().filter(|p| p.row == row) else { return false };
        p.open = true;
        true
    }

    pub(super) fn press_row(&mut self, row: usize, button: MouseButton, clicks: u8, x: f32, y: f32, now: Instant) -> bool {
        if button == MouseButton::Right {
            self.ops.press = Some(Press { row, x, y, deferred: false, slow: false, open: false, toggle: false, right: true });
            return false;
        }
        if button != MouseButton::Left || clicks >= 2 {
            return false;
        }
        let m = self.mods;
        let settled = self.focused_at.is_none_or(|t| now.duration_since(t) >= self.double_click);
        let on_name = self.on_name(x, y, row);
        let Some(v) = &mut self.tabs[self.cur].view else { return false };
        let Some(&i) = v.rows().get(row) else { return false };
        let deferred = !m.ctrl && !m.shift && v.is_selected(i);
        let toggle = m.ctrl && !m.shift && v.is_selected(i);
        if deferred || toggle {
            v.focus = Some(i);
            v.anchor = Some(i);
        }
        let slow = deferred && settled && on_name && v.selected_count() == 1;
        self.ops.press = Some(Press { row, x, y, deferred, slow, open: false, toggle, right: false });
        deferred || toggle
    }

    pub fn drag(&mut self, ev: &DragEvent, now: Instant) -> u32 {
        self.dirty = true;
        self.drag_panel(ev);
        match ev {
            DragEvent::Enter { paths, x, y, mods, allowed, right } => {
                let ours = !paths.is_empty() && self.ops.right_drag.as_ref() == Some(paths);
                self.ops.dropping = Some(Dropping { paths: paths.clone(), target: None, effect: drop_effect::NONE, spring: None, right: *right || ours, ours });
                self.drag_over(*x, *y, *mods, *allowed)
            }
            DragEvent::Over { x, y, mods, allowed } => {
                self.spring_load(*x, *y, now);
                self.drag_over(*x, *y, *mods, *allowed)
            }
            DragEvent::Leave => {
                self.nav.sidebar.dragging_out = false;
                self.ops.dropping = None;
                drop_effect::NONE
            }
            DragEvent::Drop { x, y, mods, allowed } => {
                let effect = self.drag_over(*x, *y, *mods, *allowed);
                let Some(d) = self.ops.dropping.take() else { return drop_effect::NONE };
                self.nav.sidebar.dragging_out = false;
                if d.ours {
                    self.ops.right_drag = None;
                }
                match d.target {
                    Some(Target::Reveal) if effect != drop_effect::NONE => self.on_event(&Event::DroppedFiles(d.paths), now),
                    Some(Target::NewTab) if effect != drop_effect::NONE => {
                        let routes = d.paths.into_iter().map(|p| Route { folder: Location::from_local(p), select: None }).collect();
                        self.dispatch(routes, OpenTarget::NewTab, now);
                    }
                    Some(Target::Trash(_)) if effect != drop_effect::NONE => self.start_op(FileOp::Trash(d.paths), Origin::Fresh, Then::Nothing),
                    Some(Target::Pin) if effect != drop_effect::NONE => self.effects.push(Effect::PinFolders(d.paths)),
                    Some(Target::Run { program, .. }) if effect != drop_effect::NONE => {
                        let tab = self.tab().id;
                        let launch = Launch::With { program, args: d.paths };
                        self.effects.push(Effect::Provider { provider: ProviderId::Local, req: ProviderRequest::Launch { tab, launch } });
                    }
                    Some(Target::Into { .. }) if d.paths.is_empty() => {}
                    Some(Target::Into { dest, .. }) if d.right && effect != drop_effect::NONE => {
                        let choices = self.drop_choices(&d.paths, d.ours, &dest, *allowed);
                        self.ask_drop(Asked { paths: d.paths, dest }, choices, effect, (*x, *y));
                        return allowed & drop_effect::COPY;
                    }
                    Some(Target::Into { dest, .. }) => {
                        let (items, dest) = (d.paths, dest);
                        let op = match effect {
                            drop_effect::COPY => FileOp::Copy { items, dest },
                            drop_effect::MOVE => FileOp::Move { items, dest },
                            drop_effect::LINK => FileOp::Link { items, dest },
                            _ => return drop_effect::NONE,
                        };
                        self.start_op(op, Origin::Fresh, Then::Select);
                    }
                    _ => {}
                }
                effect
            }
        }
    }

    fn drop_target(&self, x: f32, y: f32) -> Option<Target> {
        let t = self.tab();
        let into = |loc: &Location, spot: Spot| match loc.path().kind() {
            PathKind::RecycleBin => Some(Target::Trash(spot)),
            PathKind::Computer => None,
            _ if library::is_library(loc.path()) => self.library_save(loc.path()).map(|dest| Target::Into { dest, spot }),
            _ => loc.to_windows().map(|dest| Target::Into { dest, spot }),
        };
        let here = || into(&t.view_loc, Spot::List);
        match self.hit(x, y) {
            Hit::Row(_) if self.in_bin() => here(),
            Hit::Row(row) => {
                let v = t.view.as_ref()?;
                let i = *v.rows().get(row)?;
                let runnable = RUNNABLE.iter().any(|e| v.listing().ext(i).eq_ignore_ascii_case(e));
                if v.listing().entries[i as usize].is_dir() || archive::browsable(v.listing().name(i)).is_some_and(Format::can_add) {
                    into(&t.entry_location(i), Spot::Row(row))
                } else if runnable {
                    t.entry_location(i).to_windows().map(|program| Target::Run { program, spot: Spot::Row(row) })
                } else {
                    here()
                }
            }
            Hit::ListEmpty | Hit::Group(_) | Hit::Track | Hit::Thumb => here(),
            Hit::Tab(i) | Hit::TabClose(i) => into(self.tabs.get(i)?.location(), Spot::Tab(i)),
            Hit::Crumb(i) => into(&t.crumb(i)?.1, Spot::Crumb(i)),
            Hit::Side(i) | Hit::SidePin(i) if self.nav.sidebar.dragging_out && self.nav.sidebar.items.get(i).is_some_and(|it| it.section == Section::Bookmarks) => Some(Target::Pin),
            Hit::Side(i) | Hit::SidePin(i) if self.nav.sidebar.items.get(i).is_some_and(|it| it.glyph != super::G_FILE) => into(&self.nav.sidebar.items.get(i)?.loc, Spot::Side(i)),
            Hit::Side(_) | Hit::SidePin(_) => None,
            Hit::NewTab => Some(Target::NewTab),
            Hit::Header(_) | Hit::ColSep(_) | Hit::Encoding | Hit::Bin(_) => None,
            Hit::SideHeader(Section::Bookmarks) | Hit::SideEmpty => Some(Target::Pin),
            Hit::SideHeader(_) | Hit::HomeItem(_) | Hit::HomeEmpty => None,
            Hit::Splitter | Hit::FilterField | Hit::FilterClose | Hit::PanelSep(_) | Hit::Menu | Hit::QuickToggle => None,
            Hit::PaletteRow(_) | Hit::PaletteKeys(_) | Hit::PaletteField | Hit::PaletteBox | Hit::PaletteOutside | Hit::PaletteClose | Hit::PaletteDescribe | Hit::PaletteScroll(_) => None,
            Hit::OpsButton | Hit::OpsPanel | Hit::OpsRow(_) | Hit::OpsClear | Hit::OpsCancel(_) | Hit::OpsMore(_) | Hit::OpsMenu(_) => None,
            Hit::Dialog | Hit::DialogButton(_) | Hit::DialogCheck | Hit::Form(_) | Hit::Modal | Hit::Preview(_) => None,
            _ if self.layout.status.contains(x, y) => None,
            _ => Some(Target::Reveal),
        }
    }

    fn drag_over(&mut self, x: f32, y: f32, m: Mods, allowed: u32) -> u32 {
        use drop_effect::*;
        let target = self.drop_target(x, y);
        let Some(d) = &self.ops.dropping else { return NONE };
        let effect = match &target {
            None => NONE,
            Some(Target::Into { dest, .. }) if d.paths.is_empty() && archive::split(dest).is_none() => choose_effect(allowed & (COPY | MOVE), m, false),
            Some(_) if d.paths.is_empty() => NONE,
            Some(Target::Reveal | Target::NewTab) => [COPY, LINK].into_iter().find(|e| allowed & e != 0).unwrap_or(NONE),
            Some(Target::Trash(_)) if d.paths.is_empty() || d.paths.iter().any(recycle::in_bin) => NONE,
            Some(Target::Trash(_)) => allowed & MOVE,
            Some(Target::Run { program, .. }) if d.paths.is_empty() || d.paths.contains(program) => NONE,
            Some(Target::Run { .. }) => [COPY, LINK, MOVE].into_iter().find(|e| allowed & e != 0).unwrap_or(NONE),
            Some(Target::Pin) if d.paths.is_empty() => NONE,
            Some(Target::Pin) => [LINK, COPY, MOVE].into_iter().find(|e| allowed & e != 0).unwrap_or(NONE),
            Some(Target::Into { dest, .. }) => {
                if d.paths.iter().any(|p| p.same(dest) || dest.is_inside(p)) {
                    NONE
                } else if archive::split(dest).is_some() {
                    allowed & COPY
                } else {
                    let e = choose_effect(allowed, m, d.paths.iter().all(|p| same_volume(p, dest, &self.nav.sidebar.places.drives)));
                    let all_here = d.paths.iter().all(|p| p.parent().is_some_and(|q| q.same(dest)));
                    if d.right {
                        let choices = self.drop_choices(&d.paths, d.ours, dest, allowed);
                        [e, COPY, MOVE, LINK].into_iter().find(|c| choices & c != 0).unwrap_or(NONE)
                    } else if e == MOVE && all_here {
                        NONE
                    } else {
                        e
                    }
                }
            }
        };
        if let Some(d) = &mut self.ops.dropping {
            d.target = target;
            d.effect = effect;
        }
        effect
    }

    fn drop_choices(&self, paths: &[FsPath], ours: bool, dest: &FsPath, allowed: u32) -> u32 {
        use drop_effect::*;
        let mut choices = if ours { COPY | MOVE | LINK } else { allowed & (COPY | MOVE | LINK) };
        if archive::split(dest).is_some() {
            choices &= COPY;
        }
        if paths.iter().all(|p| p.parent().is_some_and(|q| q.same(dest))) {
            choices &= !MOVE;
        }
        if !self.caps.supports("file.shortcut") {
            choices &= !LINK;
        }
        choices
    }

    fn ask_drop(&mut self, asked: Asked, choices: u32, default: u32, (x, y): (f32, f32)) {
        use drop_effect::*;
        let link = self.caps.supports("file.shortcut").then_some(if self.caps.unix_paths { S::UiDropLinkHere } else { S::UiDropShortcutHere });
        let menu = self.collect(|_, m| {
            for (effect, label) in [(COPY, Some(S::UiDropCopyHere)), (MOVE, Some(S::UiDropMoveHere)), (LINK, link)] {
                if let Some(label) = label {
                    m.entry(Cmd::Drop(effect), label.t(), String::new(), Mark::None, effect == default, choices & effect == 0);
                }
            }
            m.separator();
            m.entry(Cmd::Drop(NONE), S::UiButtonCancel.t(), String::new(), Mark::None, false, false);
        });
        self.ops.asked = Some(asked);
        self.flit_menu(x, y, menu);
    }

    pub(super) fn drop_chosen(&mut self, effect: u32) {
        let Some(Asked { paths: items, dest }) = self.ops.asked.take() else { return };
        let op = match effect {
            drop_effect::COPY => FileOp::Copy { items, dest },
            drop_effect::MOVE => FileOp::Move { items, dest },
            drop_effect::LINK => FileOp::Link { items, dest },
            _ => return,
        };
        self.start_op(op, Origin::Fresh, Then::Select);
    }

    fn spring_spot(&self, x: f32, y: f32) -> Option<Spot> {
        let t = self.tab();
        match self.hit(x, y) {
            Hit::Tab(i) | Hit::TabClose(i) if i != self.cur => Some(Spot::Tab(i)),
            Hit::Crumb(i) if i + 1 != t.crumbs.len() => Some(Spot::Crumb(i)),
            Hit::Row(row) if !self.in_bin() => {
                let v = t.view.as_ref()?;
                v.listing().entries[*v.rows().get(row)? as usize].is_dir().then_some(Spot::Row(row))
            }
            Hit::SideHeader(s) if self.nav.sidebar.is_collapsed(s) => Some(Spot::SideHeader(s)),
            _ => None,
        }
    }

    fn spring_load(&mut self, x: f32, y: f32, now: Instant) {
        if self.spring_delay_ms == 0 {
            return;
        }
        let delay = Duration::from_millis(self.spring_delay_ms.into());
        let spot = self.spring_spot(x, y);
        let Some(d) = &mut self.ops.dropping else { return };
        let due = match (spot, d.spring) {
            (Some(s), Some((held, since))) if s == held => now.duration_since(since) >= if matches!(s, Spot::SideHeader(_)) { SECTION_SPRING_DELAY.min(delay) } else { delay },
            _ => {
                d.spring = spot.map(|s| (s, now));
                false
            }
        };
        if !due {
            return;
        }
        d.spring = None;
        let t = self.tab();
        let folder = match spot {
            Some(Spot::Tab(i)) => return self.select_tab(i),
            Some(Spot::SideHeader(s)) => return self.toggle_section(s),
            Some(Spot::Crumb(i)) => t.crumb(i).map(|c| c.1.clone()),
            Some(Spot::Row(row)) => t.view.as_ref().and_then(|v| v.rows().get(row).map(|&i| t.entry_location(i))),
            _ => None,
        };
        if let Some(folder) = folder {
            self.navigate(folder, now);
        }
    }

    pub fn drop_spot(&self) -> Option<Spot> {
        let d = self.ops.dropping.as_ref().filter(|d| d.effect != drop_effect::NONE)?;
        match d.target.as_ref()? {
            Target::Into { spot, .. } | Target::Run { spot, .. } | Target::Trash(spot) => Some(*spot),
            Target::NewTab => Some(Spot::NewTab),
            Target::Pin => Some(Spot::SideHeader(Section::Bookmarks)),
            Target::Reveal => None,
        }
    }

    pub fn drop_folder(&self) -> Option<FsPath> {
        let d = self.ops.dropping.as_ref().filter(|d| matches!(d.effect, drop_effect::COPY | drop_effect::MOVE))?;
        match d.target.as_ref()? {
            Target::Into { dest, .. } if archive::split(dest).is_none() => Some(dest.clone()),
            _ => None,
        }
    }

    pub fn drop_caption(&self) -> Option<(String, String)> {
        let d = self.ops.dropping.as_ref().filter(|d| d.effect != drop_effect::NONE)?;
        let name = |p: &FsPath| crate::name::display(p.file_name()).into_owned();
        let (verb, target) = match d.target.as_ref()? {
            Target::Into { dest, .. } if d.effect == drop_effect::COPY => (S::UiDropCopy, name(dest)),
            Target::Into { dest, .. } if d.effect == drop_effect::LINK => (S::UiDropLink, name(dest)),
            Target::Into { dest, .. } => (S::UiDropMove, name(dest)),
            Target::Trash(_) => (S::UiDropMove, S::PathRecycleBin.t().to_owned()),
            Target::Run { program, .. } => (S::UiDropOpenWith, name(program)),
            Target::NewTab if d.paths.len() == 1 => (S::UiDropNewTab, name(&d.paths[0])),
            Target::NewTab => (S::UiDropNewTabs, String::new()),
            Target::Pin => (S::UiDropPin, S::UiDropSidebar.t().to_owned()),
            Target::Reveal => return None,
        };
        Some((verb.t().to_owned(), target))
    }

    pub(super) fn rename_ime_area(&self) -> Option<Rect> {
        self.ops.rename.as_ref().map(|r| r.rect)
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

    pub(super) fn draw_drop_panel<B: TextBackend>(&self, g: &mut Gfx<B>, th: &Theme) {
        if self.drop_spot() == Some(Spot::List) {
            let panel = self.panel_rect();
            g.dl.rect(panel, th.teal.alpha(DROP_FILL));
            g.dl.outline(panel.inset(1.0, 1.0), th.teal, self.s(4.0), self.s(2.0).max(1.0));
        }
    }

    fn draw_rename<B: TextBackend>(&mut self, g: &mut Gfx<B>, th: &Theme) {
        let Some(row) = self.ops.rename.as_ref().and_then(|r| {
            let t = self.tab();
            t.view.as_ref().filter(|_| t.id == r.tab).and_then(|v| v.row_of(r.idx))
        }) else {
            return;
        };
        let list = self.layout.list;
        let Some(rect) = self.row_rect(row).map(|c| self.name_edit_rect(c)) else { return };
        if let Some(cell) = self.row_rect(row) {
            let veil = self.look.palette.area(crate::theme::Area::Files).bg.alpha(RENAME_VEIL);
            g.dl.set_clip(Some(list));
            g.dl.rect(Rect::new(list.x, list.y, list.w, (cell.y - list.y).max(0.0)), veil);
            g.dl.rect(Rect::new(list.x, cell.bottom(), list.w, (list.bottom() - cell.bottom()).max(0.0)), veil);
            g.dl.set_clip(None);
        }
        let mut th = *th;
        th.text_selection = th.accent;
        let th = &th;
        let font = self.content_font(13.0);
        let m = g.metrics(font);
        let baseline = (rect.y + (rect.h + m.ascent - m.descent) / 2.0).round();
        let (pad, radius, caret_w, inset) = (self.cs(4.0), self.cs(3.0), self.cs(1.0).max(1.0), self.cs(3.0));
        let Some(r) = self.ops.rename.as_mut() else { return };
        r.rect = rect;
        let inner = Rect::new(rect.x + pad, rect.y, rect.w - 2.0 * pad, rect.h);
        if let Some((x, extend)) = r.click.take() {
            let pos = g.hit(&r.edit.text, font, x - inner.x + r.scroll);
            r.edit.move_to(pos, extend);
        }
        g.dl.set_clip(Some(list));
        g.dl.round_rect(rect, th.input_bg, radius);
        g.dl.outline(rect, th.accent, radius, caret_w);
        g.dl.set_clip(Some(Rect::new(inner.x, inner.y.max(list.y), inner.w, inner.h.min(list.bottom() - inner.y.max(list.y)).max(0.0))));
        let line = super::field::EditLine { text: &r.edit.text, caret: r.edit.caret, selection: r.edit.selection() };
        let look = super::field::EditLook { font, baseline, line_y: rect.y + inset, line_h: rect.h - 2.0 * inset, stroke: caret_w, focused: true };
        let preedit = Some(&self.preedit).filter(|p| !p.is_empty());
        super::field::draw_line(g, th, inner, &mut r.scroll, line, preedit, look);
        g.dl.set_clip(None);
        self.draw_rename_hint(g, th, rect);
    }

    fn draw_rename_hint<B: TextBackend>(&self, g: &mut Gfx<B>, th: &Theme, edit: Rect) {
        let Some(hint) = self.rename_hint() else { return };
        let (font, icons, list) = (self.ui_font(12.5), self.icon_font(14.0), self.layout.list);
        let m = g.metrics(font);
        let (gap, pad_x, pad_y, icon) = (self.s(HINT_GAP), self.s(HINT_PAD_X), self.s(HINT_PAD_Y), self.s(HINT_ICON));
        let room = (list.w - 2.0 * gap).max(0.0);
        let w = (pad_x / 2.0 + icon + g.measure(hint, font) + pad_x).ceil().min(room);
        let h = (m.ascent + m.descent).ceil() + 2.0 * pad_y;
        let below = edit.bottom() + gap;
        let y = if below + h <= list.bottom() || edit.y - gap - h < list.y { below } else { edit.y - gap - h };
        let x = edit.x.min(list.right() - gap - w).max(list.x + gap);
        let r = Rect::new(x.round(), y.round(), w, h);
        g.dl.round_rect(r, th.popup, self.s(super::widgets::RADIUS));
        g.dl.outline(r, th.error, self.s(super::widgets::RADIUS), 1.0);
        super::centered(g, Rect::new(r.x + pad_x / 2.0, r.y, icon, r.h), G_WARNING, icons, th.error);
        let left = r.x + pad_x / 2.0 + icon;
        g.text(left, (r.y + (r.h + m.ascent - m.descent) / 2.0).round(), hint, font, th.text, r.right() - pad_x - left);
    }
}

#[cfg(test)]
#[path = "../../test/ui/ops.rs"]
mod tests;
