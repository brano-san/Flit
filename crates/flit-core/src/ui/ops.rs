use super::actions::{keys_text, ACTIONS};
use super::edit::{EditAction, LineEdit};
use super::palette::{Palette, Pick};
use super::{App, Drag, Effect, Focus, Hit};
use crate::hash::FxHashSet;
use crate::input::{CursorIcon, Event, Key, Mods, MouseButton};
use crate::listing::attr;
use crate::location::{Location, ProviderId};
use crate::path::{FsPath, PathKind, RECYCLE_BIN_LABEL};
use crate::provider::{FileOp, Launch, OpCtl, OpId, OpReport, ProviderRequest, TabId, Template};
use crate::recycle;
use crate::render::Rect;
use crate::text::{Gfx, TextBackend};
use crate::theme::Theme;
use crate::undo::{Journal, Origin, Step};
use std::time::{Duration, Instant};

pub mod drop_effect {
    pub const NONE: u32 = 0;
    pub const COPY: u32 = 1;
    pub const MOVE: u32 = 2;
    pub const LINK: u32 = 4;
}

pub const SHELL_VERBS: [&str; 6] = ["rename", "copy", "cut", "paste", "delete", "newfolder"];

const NEW_FOLDER: &str = "New folder";
const REVEAL_FOR: Duration = Duration::from_secs(10);
const SPRING_DELAY: Duration = Duration::from_millis(1300);
const DRAG_START: f32 = 4.0;
const MAX_NAME_UNITS: usize = 255;
const RESERVED: [&str; 4] = ["CON", "PRN", "AUX", "NUL"];
const BIN_ONLY: &str = "Items in the Recycle Bin can only be restored or deleted";
const OPEN_BIN: &str = "Open the Recycle Bin to restore items";
const MAX_TERMINALS: usize = 5;
const RUNNABLE: [&[u8]; 4] = [b"exe", b"com", b"bat", b"cmd"];
pub(super) const TIMER_SLACK: Duration = Duration::from_millis(16);

#[derive(Clone, Debug, PartialEq)]
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
}

#[derive(Clone, Debug, PartialEq)]
pub enum DragEvent {
    Enter { paths: Vec<FsPath>, x: f32, y: f32, mods: Mods, allowed: u32 },
    Over { x: f32, y: f32, mods: Mods, allowed: u32 },
    Leave,
    Drop { x: f32, y: f32, mods: Mods, allowed: u32 },
}

#[derive(Clone, PartialEq, Eq, Debug)]
enum Then {
    Nothing,
    Select,
    Rename,
    Fill(Vec<FsPath>),
    Reveal(Vec<u8>),
}

struct Pending {
    id: OpId,
    tab: TabId,
    origin: Origin,
    then: Then,
    ctl: OpCtl,
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
}

struct Press {
    row: usize,
    x: f32,
    y: f32,
    deferred: bool,
    slow: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Spot {
    List,
    Row(usize),
    Tab(usize),
    Crumb(usize),
    Side(usize),
}

#[derive(Clone, PartialEq, Debug)]
enum Target {
    Reveal,
    Into { dest: FsPath, spot: Spot },
    Trash(Spot),
    Run { program: FsPath, spot: Spot },
}

struct Dropping {
    paths: Vec<FsPath>,
    target: Option<Target>,
    effect: u32,
    spring: Option<(Spot, Instant)>,
}

#[derive(Default)]
pub(super) struct Ops {
    next_id: OpId,
    pending: Vec<Pending>,
    journal: Journal,
    reveal: Vec<Reveal>,
    rename: Option<Rename>,
    press: Option<Press>,
    dropping: Option<Dropping>,
    slow: Option<Slow>,
    cut: Option<Cut>,
}

pub fn validate_name(text: &str, windows: bool) -> Result<String, &'static str> {
    let mut name = text.trim();
    if windows {
        name = name.trim_end_matches(['.', ' ']);
    }
    if name.is_empty() {
        return Err("A name cannot be empty");
    }
    if name == "." || name == ".." {
        return Err("This name is not allowed");
    }
    if windows {
        if name.chars().any(|c| c.is_control() || r#"<>:"/\|?*"#.contains(c)) {
            return Err(r#"A name cannot contain any of these characters: \ / : * ? " < > |"#);
        }
        let stem = name.split('.').next().unwrap_or("").trim_end().to_ascii_uppercase();
        let port = |n: &str| (n.len() == 1 && n.as_bytes()[0].is_ascii_digit()) || matches!(n, "¹" | "²" | "³");
        if RESERVED.contains(&stem.as_str()) || ["COM", "LPT"].iter().any(|p| stem.strip_prefix(p).is_some_and(port)) {
            return Err("This name is reserved by Windows");
        }
    } else if name.contains(['/', '\0']) {
        return Err("A name cannot contain a slash");
    }
    if name.encode_utf16().count() > MAX_NAME_UNITS {
        return Err("The name is too long");
    }
    Ok(name.to_owned())
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

pub fn same_volume(a: &FsPath, b: &FsPath) -> bool {
    a.kind() == b.kind() && a.root_bytes().eq_ignore_ascii_case(b.root_bytes())
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

fn same_location(a: &Location, b: &Location) -> bool {
    a.to_bytes().eq_ignore_ascii_case(&b.to_bytes())
}

fn inside(path: &FsPath, folder: &FsPath) -> bool {
    let (p, f) = (path.as_bytes(), folder.as_bytes());
    p.len() > f.len() && p[..f.len()].eq_ignore_ascii_case(f) && (folder.is_root() || p[f.len()] == folder.sep())
}

impl App {
    pub fn status(&mut self, msg: impl Into<String>) {
        self.tab_mut().error = Some(msg.into());
        self.dirty = true;
    }

    fn in_bin(&self) -> bool {
        self.tab().view_loc.is_recycle_bin()
    }

    fn shell_dir(&self) -> Result<FsPath, &'static str> {
        let loc = &self.tab().view_loc;
        match loc.path().kind() {
            PathKind::Computer => return Err("Files cannot be placed in This PC"),
            PathKind::RecycleBin => return Err("Files cannot be placed in the Recycle Bin"),
            _ => {}
        }
        loc.to_windows().ok_or("This folder cannot be used by Windows shell operations")
    }

    pub(super) fn item_paths(&self, items: &[u32]) -> Result<Vec<FsPath>, &'static str> {
        if self.in_bin() {
            return Err(BIN_ONLY);
        }
        let t = self.tab();
        items.iter().map(|&i| t.entry_location(i).to_windows()).collect::<Option<Vec<_>>>().ok_or("Some names cannot be used by Windows shell operations")
    }

    pub(super) fn selected_items(&self) -> Vec<u32> {
        let t = self.tab();
        match &t.view {
            Some(v) if t.view_loc.path().kind() != PathKind::Computer => v.selected().collect(),
            _ => Vec::new(),
        }
    }

    fn start_op(&mut self, op: FileOp, origin: Origin, then: Then) {
        self.start_op_for(self.tab().id, op, origin, then);
    }

    fn start_op_for(&mut self, tab: TabId, op: FileOp, origin: Origin, then: Then) {
        self.ops.next_id += 1;
        let id = self.ops.next_id;
        let ctl = OpCtl::new(self.conflict_default);
        self.track_job(id, &op, &ctl);
        self.ops.pending.push(Pending { id, tab, origin, then, ctl: ctl.clone() });
        self.effects.push(Effect::Provider { provider: ProviderId::Local, req: ProviderRequest::Op { tab, id, op, ctl } });
    }

    pub(super) fn start_confirmed(&mut self, op: FileOp) {
        self.start_op(op, Origin::Fresh, Then::Nothing);
    }

    pub(super) fn op_ctl(&self, id: OpId) -> Option<OpCtl> {
        self.ops.pending.iter().find(|p| p.id == id).map(|p| p.ctl.clone())
    }

    pub(super) fn cancel_ops(&mut self) {
        for p in &self.ops.pending {
            p.ctl.cancel();
        }
    }

    fn delete_with(&mut self, permanent: bool) {
        if self.in_bin() {
            return self.purge_selection();
        }
        let items = self.selected_items();
        if items.is_empty() {
            return;
        }
        match self.item_paths(&items) {
            Ok(paths) if permanent => self.ask_confirm(FileOp::Delete(paths)),
            Ok(paths) => self.start_op(FileOp::Trash(paths), Origin::Fresh, Then::Nothing),
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
            return self.status("Select the folders to flatten");
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

    fn purge_selection(&mut self) {
        let pairs = self.bin_pairs(self.selected_items().into_iter());
        if !pairs.is_empty() {
            self.ask_confirm(FileOp::Purge(pairs));
        }
    }

    fn restore(&mut self, all: bool) {
        if !self.in_bin() {
            return self.status(OPEN_BIN);
        }
        let pairs = match (all, &self.tab().view) {
            (true, Some(v)) => {
                let l = v.listing();
                self.bin_pairs((0..l.len() as u32).filter(|&i| l.entries[i as usize].attrs & attr::DELETED == 0))
            }
            _ => self.bin_pairs(self.selected_items().into_iter()),
        };
        match pairs.is_empty() {
            true if all => self.status("The Recycle Bin is empty"),
            true => self.status("Select the items to restore"),
            false => self.start_op(FileOp::Restore(pairs), Origin::Fresh, Then::Nothing),
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
        match self.item_paths(&items) {
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

    pub fn paste_into_selection(&mut self) {
        let items = self.selected_items();
        let is_dir = |i: u32| self.tab().view.as_ref().is_some_and(|v| v.listing().entries[i as usize].is_dir());
        let dest = match items.as_slice() {
            &[i] if is_dir(i) => self.item_paths(&[i]).ok().and_then(|mut p| p.pop()),
            _ => None,
        };
        match dest {
            Some(dest) => self.start_op(FileOp::Paste { dest }, Origin::Fresh, Then::Nothing),
            None => self.status("Select one folder to paste into"),
        }
    }

    fn free_name(&self, stem: &str, ext: &str) -> String {
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

    pub fn new_folder(&mut self) {
        self.create(|dir, name| FileOp::NewFolder { dir, name }, NEW_FOLDER, "");
    }

    pub fn new_folder_with_selection(&mut self) {
        let items = self.selected_items();
        if items.is_empty() {
            return self.status("Select the items to put into a new folder");
        }
        let (paths, dir) = match (self.item_paths(&items), self.shell_dir()) {
            (Ok(p), Ok(d)) => (p, d),
            (Err(e), _) | (_, Err(e)) => return self.status(e),
        };
        let name = self.free_name(NEW_FOLDER, "").into_bytes();
        self.start_op(FileOp::NewFolder { dir, name }, Origin::Fresh, Then::Fill(paths));
    }

    pub fn create_shortcuts(&mut self) {
        let items = self.selected_items();
        if items.is_empty() {
            return self.status("Select the items to create shortcuts to");
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
        let folder_keys = ACTIONS.iter().find(|a| a.id == "file.new_folder").map(keys_text).unwrap_or_default();
        let mut items = vec![("Folder".to_owned(), folder_keys), ("Shortcut".to_owned(), String::new())];
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
        let Some(i) = self.rename_target() else { return self.status("Select a file to add a stream to") };
        match self.item_paths(&[i]) {
            Ok(mut p) => self.open_pick(Palette::new(Pick::Stream(p.remove(0)), Vec::new(), "")),
            Err(e) => self.status(e),
        }
    }

    pub fn ask_folder_icon(&mut self) {
        let folder = self.rename_target().filter(|&i| self.tab().view.as_ref().is_some_and(|v| v.listing().entries[i as usize].is_dir()));
        let path = match folder {
            Some(i) => self.item_paths(&[i]).map(|mut p| p.remove(0)),
            None => self.shell_dir(),
        };
        match path {
            Ok(folder) => self.open_pick(Palette::new(Pick::FolderIcon(folder), Vec::new(), "")),
            Err(e) => self.status(e),
        }
    }

    pub(super) fn submit_pick(&mut self, pick: Pick, text: &str) {
        match pick {
            Pick::FolderIcon(folder) => match icon_location(text) {
                Some((icon, index)) => self.start_op(FileOp::FolderIcon { folder, icon, index }, Origin::Fresh, Then::Nothing),
                None => self.status("Enter the full path of an icon, program or library, optionally followed by a comma and the icon index"),
            },
            Pick::Shortcut(dest) => match FsPath::parse(text.trim_matches('"')).filter(|p| matches!(p.kind(), PathKind::Drive | PathKind::Unc | PathKind::Verbatim)) {
                Some(target) => self.start_op(FileOp::Link { items: vec![target], dest }, Origin::Fresh, Then::Rename),
                None => self.status("Enter the full path of the file or folder the shortcut points to"),
            },
            Pick::Stream(item) => match validate_name(text, true) {
                Ok(name) => self.start_op(FileOp::NewStream { item, name: name.into_bytes() }, Origin::Fresh, Then::Nothing),
                Err(e) => self.status(e),
            },
            Pick::Tags(t) => self.tag_input(t, text),
            Pick::Commands | Pick::NewItem => {}
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
                None => return self.status("This location cannot be opened in a terminal"),
            },
        };
        if folders.len() > MAX_TERMINALS {
            self.status(format!("Opened the first {MAX_TERMINALS} of {} folders in the terminal", folders.len()));
        }
        let tab = self.tab().id;
        self.effects.push(Effect::Provider { provider: ProviderId::Local, req: ProviderRequest::Launch { tab, launch: Launch::Terminal { dirs, admin } } });
    }

    pub fn undo(&mut self) {
        self.replay(Origin::Undo);
    }

    pub fn redo(&mut self) {
        self.replay(Origin::Redo);
    }

    fn replay(&mut self, origin: Origin) {
        match self.ops.journal.take(origin) {
            None => self.status(if origin == Origin::Undo { "Nothing to undo" } else { "Nothing to redo" }),
            Some(Step::Refuse(msg)) => self.status(msg),
            Some(Step::Run(op)) => self.start_op(op, origin, Then::Select),
        }
    }

    pub fn undo_depth(&self) -> (usize, usize) {
        self.ops.journal.depth()
    }

    pub fn properties(&mut self) {
        let t = self.tab();
        let Some(v) = &t.view else { return };
        let items: Vec<u32> = v.selected().collect();
        let paths = if items.is_empty() {
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
        if self.in_bin() && on_items {
            return self.status(BIN_ONLY);
        }
        if self.in_bin() {
            let menu = self.build_menu(false);
            return self.flit_menu(x, y, menu);
        }
        let t = self.tab();
        let Some(dir) = t.view_loc.to_windows() else { return self.status("This folder cannot be used by Windows shell operations") };
        let items: Vec<u32> = match (&t.view, on_items) {
            (Some(v), true) => v.selected().collect(),
            _ => Vec::new(),
        };
        let items = match self.item_paths(&items) {
            Ok(p) => p,
            Err(e) => return self.status(e),
        };
        let inline = self.shell_items_inline || extended;
        let menu = self.build_menu(on_items);
        self.effects.push(Effect::ShellMenu(ShellMenu { dir, items, x, y, extended, menu, inline, shell: true }));
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
        self.job_done(id, error.as_deref(), now);
        let pending = self.ops.pending.iter().position(|p| p.id == id && p.tab == tab).map(|k| self.ops.pending.swap_remove(k));
        let k = self.tabs.iter().position(|t| t.id == tab);
        if let Some(message) = &error {
            let k = k.unwrap_or(self.cur);
            self.tabs[k].error = Some(message.clone());
            self.dirty |= k == self.cur;
        }
        let any_done = report.as_ref().is_some_and(|r| !r.items.is_empty());
        let unwatched = |l: &Location| matches!(l, Location::Wsl { .. }) || l.path().kind() == PathKind::Unc;
        if let Some(k) = k.filter(|&k| (error.is_none() || any_done) && unwatched(self.tabs[k].location())) {
            self.reload(k, now);
        }
        if let Some(r) = &report {
            self.tags_moved(r);
        }
        let (Some(report), Some(p)) = (report, pending) else { return };
        self.ops.journal.record(p.origin, &report);
        if let (Then::Fill(items), Some((_, folder))) = (&p.then, report.items.first()) {
            let reveal = Then::Reveal(Location::from_local(folder.clone()).file_name().to_vec());
            return self.start_op_for(tab, FileOp::Move { items: items.clone(), dest: folder.clone() }, Origin::Fresh, reveal);
        }
        if let (FileOp::NewStream { item, name }, Some(k), None) = (&report.op, k, &error) {
            self.tabs[k].error = Some(format!("Created the stream {}:{}", item.display(), crate::name::display(name)));
            self.dirty |= k == self.cur;
        }
        if let (FileOp::FolderIcon { folder, .. }, None) = (&report.op, &error) {
            self.icon_epoch += 1;
            self.tabs[k.unwrap_or(self.cur)].error = Some(format!("Changed the icon of {}", folder.display()));
            self.dirty = true;
        }
        let Some(k) = k.filter(|_| p.then != Then::Nothing && !matches!(report.op, FileOp::Trash(_))) else { return };
        let dir = self.tabs[k].view_loc.clone();
        let names: Vec<Vec<u8>> = match &p.then {
            Then::Reveal(name) => vec![name.clone()],
            _ => report
                .items
                .iter()
                .map(|(_, to)| Location::from_local(to.clone()))
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

    fn rename_target(&self) -> Option<u32> {
        let t = self.tab();
        let v = t.view.as_ref().filter(|_| !matches!(t.view_loc.path().kind(), PathKind::Computer | PathKind::RecycleBin))?;
        v.focus.filter(|&f| v.is_visible(f)).or_else(|| if v.selected_count() == 1 { v.selected().next() } else { None })
    }

    pub fn rename_focused(&mut self) {
        let Some(i) = self.rename_target() else { return };
        let batch = self.tab().view.as_ref().is_some_and(|v| v.selected_count() > 1 && v.is_selected(i));
        self.begin_rename(i, batch);
    }

    fn begin_rename(&mut self, idx: u32, batch: bool) {
        let t = &mut self.tabs[self.cur];
        let Some(v) = t.view.as_mut() else { return };
        let Ok(name) = std::str::from_utf8(v.listing().name(idx)) else {
            t.error = Some("This name cannot be edited in Flit".into());
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
        self.ops.rename = Some(Rename { tab, gen, idx, edit, scroll: 0.0, rect: Rect::default(), click: None, selecting: false, batch, confirmed: None });
        self.ops.slow = None;
        self.editing = false;
        self.focus = Focus::List;
        self.dirty = true;
    }

    pub fn is_renaming(&self) -> bool {
        self.ops.rename.is_some()
    }

    pub fn rename_text(&self) -> Option<&str> {
        self.ops.rename.as_ref().map(|r| r.edit.text.as_str())
    }

    fn commit_rename(&mut self) -> bool {
        let Some(mut r) = self.ops.rename.take() else { return true };
        let t = self.tab();
        let Some(v) = t.view.as_ref().filter(|v| t.id == r.tab && v.snap().gen == r.gen) else { return true };
        let old = v.listing().name(r.idx).to_vec();
        let is_dir = v.listing().entries[r.idx as usize].is_dir();
        let windows = !matches!(t.view_loc, Location::Wsl { .. });
        let name = match validate_name(&r.edit.text, windows) {
            Ok(n) => n,
            Err(e) => {
                self.ops.rename = Some(r);
                self.status(e);
                return false;
            }
        };
        if r.batch && v.selected_count() > 1 {
            let ext = extension(&old, is_dir);
            let base = match name.len().checked_sub(ext.len()) {
                Some(n) if n > 0 && name.as_bytes()[n..].eq_ignore_ascii_case(ext) => &name[..n],
                _ => &name,
            };
            self.batch_rename(base);
            return true;
        }
        if name.as_bytes() == old {
            return true;
        }
        let old_str = String::from_utf8_lossy(&old);
        if !is_dir && self.warn_extension_change && r.confirmed.as_deref() != Some(&name) && changes_extension(&old_str, &name) {
            let msg = format!("Changing the extension of \"{old_str}\" can make it unusable. Press Enter again to rename it, or Esc to cancel.");
            r.confirmed = Some(name);
            self.ops.rename = Some(r);
            self.status(msg);
            return false;
        }
        let (item, target) = (t.view_loc.join(&old).to_windows(), t.view_loc.join(name.as_bytes()).to_windows());
        let (Some(item), Some(target)) = (item, target) else {
            self.status("This name cannot be used by Windows shell operations");
            return true;
        };
        self.tab_mut().error = None;
        self.start_op(FileOp::Rename { item, name: target.file_name().to_vec() }, Origin::Fresh, Then::Select);
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
        let renamed = items.iter().zip(&names).filter(|((old, _), new)| *old != new.as_slice());
        let pairs: Option<Vec<(FsPath, FsPath)>> = renamed.map(|((old, _), new)| Some((t.view_loc.join(old).to_windows()?, t.view_loc.join(new).to_windows()?))).collect();
        match pairs {
            Some(p) if p.is_empty() => {}
            Some(p) => {
                self.tab_mut().error = None;
                self.start_op(FileOp::MoveTo(p), Origin::Fresh, Then::Select);
            }
            None => self.status("Some names cannot be used by Windows shell operations"),
        }
    }

    pub(super) fn ops_key(&mut self, key: Key, m: Mods) -> bool {
        self.ops.slow = None;
        let Some(action) = self.ops.rename.as_mut().map(|r| r.edit.key(key, m)) else { return false };
        match action {
            EditAction::Submit => {
                self.commit_rename();
            }
            EditAction::Cancel => self.ops.rename = None,
            EditAction::Copy(s) => self.effects.push(Effect::SetClipboard(s)),
            EditAction::Paste => self.effects.push(Effect::Paste),
            EditAction::Changed | EditAction::None => {}
        }
        true
    }

    pub(super) fn ops_char(&mut self, c: char) -> bool {
        let Some(r) = &mut self.ops.rename else { return false };
        let mut b = [0; 4];
        r.edit.insert(c.encode_utf8(&mut b));
        true
    }

    pub(super) fn ops_paste(&mut self, text: &str) -> bool {
        let Some(r) = &mut self.ops.rename else { return false };
        r.edit.insert(text.lines().next().unwrap_or(""));
        self.dirty = true;
        true
    }

    pub(super) fn ops_mouse_down(&mut self, x: f32, y: f32, button: MouseButton, clicks: u8) -> bool {
        self.ops.press = None;
        self.ops.slow = None;
        let Some(r) = &mut self.ops.rename else { return false };
        if r.rect.contains(x, y) {
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
        self.ops.press = None;
        let items = self.selected_items();
        let Ok(paths) = self.item_paths(&items) else { return false };
        if paths.is_empty() {
            return false;
        }
        self.pressed = Hit::None;
        self.drag = Drag::None;
        self.hover = Hit::None;
        self.effects.push(Effect::StartDrag(paths));
        true
    }

    pub(super) fn ops_mouse_up(&mut self, x: f32, y: f32, button: MouseButton, now: Instant) {
        if let Some(r) = &mut self.ops.rename {
            r.selecting = false;
        }
        if button == MouseButton::Right && self.drag == Drag::None && self.ops.rename.is_none() {
            match self.hit(x, y) {
                Hit::Row(_) => self.shell_menu(x, y, true, self.mods.shift),
                Hit::ListEmpty | Hit::Group(_) => self.shell_menu(x, y, false, self.mods.shift),
                Hit::Header(_) | Hit::ColSep(_) => self.header_menu(x, y),
                _ => {}
            }
        }
        let Some(p) = self.ops.press.take().filter(|p| p.deferred && button == MouseButton::Left) else { return };
        if self.hit(x, y) != Hit::Row(p.row) {
            return;
        }
        let (tab, wait) = (self.tab().id, self.double_click);
        let Some(v) = &mut self.tabs[self.cur].view else { return };
        let Some(&i) = v.rows().get(p.row) else { return };
        v.clear_selection();
        v.set_selected(i, true);
        v.anchor = Some(i);
        v.focus = Some(i);
        if p.slow {
            self.ops.slow = Some(Slow { tab, gen: v.snap().gen, idx: i, at: now + wait });
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

    pub(super) fn press_row(&mut self, row: usize, button: MouseButton, clicks: u8, x: f32, y: f32, now: Instant) -> bool {
        if button != MouseButton::Left || clicks >= 2 {
            return false;
        }
        let m = self.mods;
        let settled = self.focused_at.is_none_or(|t| now.duration_since(t) >= self.double_click);
        let on_name = self.on_name(x, y, row);
        let Some(v) = &mut self.tabs[self.cur].view else { return false };
        let Some(&i) = v.rows().get(row) else { return false };
        let deferred = !m.ctrl && !m.shift && v.is_selected(i);
        if deferred {
            v.focus = Some(i);
            v.anchor = Some(i);
        }
        let slow = deferred && settled && on_name && v.selected_count() == 1;
        self.ops.press = Some(Press { row, x, y, deferred, slow });
        deferred
    }

    pub fn drag(&mut self, ev: &DragEvent, now: Instant) -> u32 {
        self.dirty = true;
        match ev {
            DragEvent::Enter { paths, x, y, mods, allowed } => {
                self.ops.dropping = Some(Dropping { paths: paths.clone(), target: None, effect: drop_effect::NONE, spring: None });
                self.drag_over(*x, *y, *mods, *allowed)
            }
            DragEvent::Over { x, y, mods, allowed } => {
                self.spring_load(*x, *y, now);
                self.drag_over(*x, *y, *mods, *allowed)
            }
            DragEvent::Leave => {
                self.ops.dropping = None;
                drop_effect::NONE
            }
            DragEvent::Drop { x, y, mods, allowed } => {
                let effect = self.drag_over(*x, *y, *mods, *allowed);
                let Some(d) = self.ops.dropping.take() else { return drop_effect::NONE };
                match d.target {
                    Some(Target::Reveal) if effect != drop_effect::NONE => self.on_event(&Event::DroppedFiles(d.paths), now),
                    Some(Target::Trash(_)) if effect != drop_effect::NONE => self.start_op(FileOp::Trash(d.paths), Origin::Fresh, Then::Nothing),
                    Some(Target::Run { program, .. }) if effect != drop_effect::NONE => {
                        let tab = self.tab().id;
                        let launch = Launch::With { program, args: d.paths };
                        self.effects.push(Effect::Provider { provider: ProviderId::Local, req: ProviderRequest::Launch { tab, launch } });
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
            _ => loc.to_windows().map(|dest| Target::Into { dest, spot }),
        };
        let here = || into(&t.view_loc, Spot::List);
        match self.hit(x, y) {
            Hit::Row(_) if self.in_bin() => here(),
            Hit::Row(row) => {
                let v = t.view.as_ref()?;
                let i = *v.rows().get(row)?;
                let runnable = RUNNABLE.iter().any(|e| v.listing().ext(i).eq_ignore_ascii_case(e));
                if v.listing().entries[i as usize].is_dir() {
                    into(&t.entry_location(i), Spot::Row(row))
                } else if runnable {
                    t.entry_location(i).to_windows().map(|program| Target::Run { program, spot: Spot::Row(row) })
                } else {
                    here()
                }
            }
            Hit::ListEmpty | Hit::Group(_) | Hit::Track | Hit::Thumb => here(),
            Hit::Tab(i) | Hit::TabClose(i) => into(self.tabs.get(i)?.location(), Spot::Tab(i)),
            Hit::Crumb(i) => into(&t.crumbs.get(i)?.1, Spot::Crumb(i)),
            Hit::Side(i) | Hit::SidePin(i) => into(&self.nav.sidebar.items.get(i)?.loc, Spot::Side(i)),
            Hit::Header(_) | Hit::ColSep(_) | Hit::Sizes | Hit::Bin(_) => None,
            Hit::SideEmpty | Hit::Splitter | Hit::FilterField | Hit::FilterClose => None,
            Hit::PaletteRow(_) | Hit::PaletteField | Hit::PaletteBox | Hit::PaletteOutside => None,
            Hit::OpsButton | Hit::OpsPanel | Hit::OpsRow(_) | Hit::OpsClear | Hit::OpsCancel(_) | Hit::OpsMore(_) | Hit::OpsMenu(_) => None,
            Hit::Dialog | Hit::DialogButton(_) | Hit::DialogCheck | Hit::Modal => None,
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
            Some(Target::Reveal) => [COPY, LINK].into_iter().find(|e| allowed & e != 0).unwrap_or(NONE),
            Some(Target::Trash(_)) if d.paths.is_empty() || d.paths.iter().any(recycle::in_bin) => NONE,
            Some(Target::Trash(_)) => allowed & MOVE,
            Some(Target::Run { program, .. }) if d.paths.is_empty() || d.paths.contains(program) => NONE,
            Some(Target::Run { .. }) => [COPY, LINK, MOVE].into_iter().find(|e| allowed & e != 0).unwrap_or(NONE),
            Some(Target::Into { dest, .. }) => {
                if d.paths.is_empty() || d.paths.iter().any(|p| p.as_bytes().eq_ignore_ascii_case(dest.as_bytes()) || inside(dest, p)) {
                    NONE
                } else {
                    let e = choose_effect(allowed, m, d.paths.iter().all(|p| same_volume(p, dest)));
                    let all_here = d.paths.iter().all(|p| p.parent().is_some_and(|q| q.as_bytes().eq_ignore_ascii_case(dest.as_bytes())));
                    if e == MOVE && all_here {
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

    fn spring_spot(&self, x: f32, y: f32) -> Option<Spot> {
        let t = self.tab();
        match self.hit(x, y) {
            Hit::Tab(i) | Hit::TabClose(i) if i != self.cur => Some(Spot::Tab(i)),
            Hit::Crumb(i) if i + 1 < t.crumbs.len() => Some(Spot::Crumb(i)),
            Hit::Row(row) if !self.in_bin() => {
                let v = t.view.as_ref()?;
                v.listing().entries[*v.rows().get(row)? as usize].is_dir().then_some(Spot::Row(row))
            }
            _ => None,
        }
    }

    fn spring_load(&mut self, x: f32, y: f32, now: Instant) {
        let spot = self.spring_spot(x, y);
        let Some(d) = &mut self.ops.dropping else { return };
        let due = match (spot, d.spring) {
            (Some(s), Some((held, since))) if s == held => now.duration_since(since) >= SPRING_DELAY,
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
            Some(Spot::Crumb(i)) => t.crumbs.get(i).map(|c| c.1.clone()),
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
            Target::Reveal => None,
        }
    }

    pub fn drop_caption(&self) -> Option<(String, String)> {
        let d = self.ops.dropping.as_ref().filter(|d| d.effect != drop_effect::NONE)?;
        let name = |p: &FsPath| crate::name::display(p.file_name()).into_owned();
        let (verb, target) = match d.target.as_ref()? {
            Target::Into { dest, .. } if d.effect == drop_effect::COPY => ("Copy to %1", name(dest)),
            Target::Into { dest, .. } if d.effect == drop_effect::LINK => ("Create link in %1", name(dest)),
            Target::Into { dest, .. } => ("Move to %1", name(dest)),
            Target::Trash(_) => ("Move to %1", RECYCLE_BIN_LABEL.to_owned()),
            Target::Run { program, .. } => ("Open with %1", name(program)),
            Target::Reveal => return None,
        };
        Some((verb.to_owned(), target))
    }

    pub(super) fn rename_ime_area(&self) -> Option<Rect> {
        self.ops.rename.as_ref().map(|r| r.rect)
    }

    pub(super) fn draw_ops<B: TextBackend>(&mut self, g: &mut Gfx<B>, th: &Theme) {
        let list = self.layout.list;
        match self.drop_spot() {
            Some(Spot::Row(row)) => {
                let Some(r) = self.row_rect(row).map(|c| self.item_parts(c).body) else { return self.draw_rename(g, th) };
                g.dl.set_clip(Some(list));
                g.dl.round_rect(r, th.selection, self.s(4.0));
                g.dl.outline(r, th.accent, self.s(4.0), self.s(1.0).max(1.0));
                g.dl.set_clip(None);
            }
            Some(Spot::List) => g.dl.outline(list.inset(1.0, 1.0), th.accent, self.s(4.0), self.s(2.0).max(1.0)),
            _ => {}
        }
        self.draw_rename(g, th);
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
        let font = self.ui_font(13.0);
        let m = g.metrics(font);
        let baseline = (rect.y + (rect.h + m.ascent - m.descent) / 2.0).round();
        let (pad, radius, caret_w, inset) = (self.s(4.0), self.s(3.0), self.s(1.0).max(1.0), self.s(3.0));
        let Some(r) = self.ops.rename.as_mut() else { return };
        r.rect = rect;
        let inner = Rect::new(rect.x + pad, rect.y, rect.w - 2.0 * pad, rect.h);
        if let Some((x, extend)) = r.click.take() {
            let pos = g.hit(&r.edit.text, font, x - inner.x + r.scroll);
            r.edit.move_to(pos, extend);
        }
        let caret_x = g.caret_x(&r.edit.text, font, r.edit.caret);
        if caret_x - r.scroll > inner.w {
            r.scroll = caret_x - inner.w;
        } else if caret_x < r.scroll {
            r.scroll = caret_x;
        }
        g.dl.set_clip(Some(list));
        g.dl.round_rect(rect, th.input_bg, radius);
        g.dl.outline(rect, th.accent, radius, caret_w);
        g.dl.set_clip(Some(Rect::new(inner.x, inner.y.max(list.y), inner.w, inner.h.min(list.bottom() - inner.y.max(list.y)).max(0.0))));
        let x0 = inner.x - r.scroll;
        let sel = r.edit.selection();
        if !sel.is_empty() {
            let a = g.caret_x(&r.edit.text, font, sel.start);
            let b = g.caret_x(&r.edit.text, font, sel.end);
            g.dl.rect(Rect::new(x0 + a, rect.y + inset, b - a, rect.h - 2.0 * inset), th.text_selection);
        }
        g.text(x0, baseline, &r.edit.text, font, th.text, f32::INFINITY);
        g.dl.rect(Rect::new(x0 + caret_x, rect.y + inset, caret_w, rect.h - 2.0 * inset), th.text);
        g.dl.set_clip(None);
    }
}

#[cfg(test)]
#[path = "../../test/ui/ops.rs"]
mod tests;
