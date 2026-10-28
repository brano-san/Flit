use super::keymap::Keymap;
use super::toolbar::{Overflow, ToolMenu};
use super::layouts::{layout_of_zoom, zoom_title, ZOOM_DETAILS, ZOOM_ICONS, ZOOM_LARGE, ZOOM_LIST, ZOOM_TREE, ZOOM_XL};
use super::{App, Column, LayoutMode, MENU_COLUMNS};
use crate::archive::{self, Format};
use crate::i18n::S;
use crate::listing::attr;
use crate::location::Location;
use crate::path::PathKind;
use crate::sort::{Folders, GroupField, SortField};
use std::time::Instant;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mark {
    #[default]
    None,
    Check,
    Radio,
    Sort(bool),
}

#[derive(Clone, Debug, PartialEq)]
pub enum MenuItem {
    Command { label: String, keys: String, checked: bool, stay: bool, mark: Mark, icon: &'static str, disabled: bool },
    Submenu { label: String, items: Vec<MenuItem> },
    Header(String),
    Separator,
}

impl MenuItem {
    pub fn command(label: impl Into<String>, keys: impl Into<String>) -> Self {
        MenuItem::Command { label: label.into(), keys: keys.into(), checked: false, stay: false, mark: Mark::None, icon: "", disabled: false }
    }

    pub fn checked(mut self, on: bool) -> Self {
        if let MenuItem::Command { checked, .. } = &mut self {
            *checked = on;
        }
        self
    }

    pub fn staying(mut self) -> Self {
        if let MenuItem::Command { stay, .. } = &mut self {
            *stay = true;
        }
        self
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Cmd {
    Action(&'static str),
    NewItem(usize),
    Go(usize),
    Jump(isize),
    Place(super::sidebar::PlaceCmd),
    ToggleTag(usize),
    ShowTag(usize),
    OpenTagged(usize),
    Setting(usize, usize),
    GitBranch(usize),
    Column(ColumnCmd),
    Types(TypeCmd),
    SortColumn(SortField),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum ColumnCmd {
    Toggle(Column),
    Shift(Column, bool),
    Fit(Column),
    Restore,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum TypeCmd {
    Toggle(u64),
    All(bool),
}

const SORTS: [(SortField, &str, S); 10] = [
    (SortField::Name, "view.sort.name", S::MenuSortName),
    (SortField::Type, "view.sort.type", S::MenuSortType),
    (SortField::Size, "view.sort.size", S::MenuSortSize),
    (SortField::Items, "view.sort.items", S::DetailsColumnItems),
    (SortField::Created, "view.sort.created", S::MenuSortCreated),
    (SortField::Modified, "view.sort.modified", S::MenuSortModified),
    (SortField::Accessed, "view.sort.accessed", S::DetailsColumnAccessed),
    (SortField::Volume, "view.sort.volume", S::DetailsColumnVolume),
    (SortField::FolderUsage, "view.sort.folder_usage", S::DetailsColumnFolderUsage),
    (SortField::Tag, "view.sort.tag", S::MenuSortTags),
];

const COMPUTER_SORTS: [(SortField, &str, S); 6] = [
    (SortField::Name, "view.sort.name", S::MenuSortName),
    (SortField::Type, "view.sort.type", S::MenuSortType),
    (SortField::Size, "view.sort.size", S::MenuSortSize),
    (SortField::Free, "view.sort.free", S::DetailsColumnFree),
    (SortField::Percent, "view.sort.percent", S::DetailsColumnPercent),
    (SortField::FileSystem, "view.sort.file_system", S::DetailsColumnFileSystem),
];

const BIN_SORTS: [(SortField, &str, S); 4] = [
    (SortField::Name, "view.sort.name", S::MenuSortName),
    (SortField::Origin, "view.sort.origin", S::MenuSortOrigin),
    (SortField::Modified, "view.sort.modified", S::MenuSortDeleted),
    (SortField::Size, "view.sort.size", S::MenuSortSize),
];

const FOLDERS: [(Folders, &str, S); 3] = [
    (Folders::First, "view.sort.folders_first", S::MenuSortFoldersFirst),
    (Folders::Last, "view.sort.files_first", S::MenuSortFilesFirst),
    (Folders::Mixed, "view.sort.mixed", S::MenuSortMixed),
];

const GROUPS: [(GroupField, &str, S); 11] = [
    (GroupField::None, "view.group.none", S::MenuGroupNone),
    (GroupField::Name, "view.group.name", S::MenuSortName),
    (GroupField::Size, "view.group.size", S::MenuSortSize),
    (GroupField::Created, "view.group.created", S::MenuSortCreated),
    (GroupField::Modified, "view.group.modified", S::MenuSortModified),
    (GroupField::Accessed, "view.group.accessed", S::DetailsColumnAccessed),
    (GroupField::Items, "view.group.items", S::DetailsColumnItems),
    (GroupField::Type, "view.group.type", S::MenuSortType),
    (GroupField::Volume, "view.group.volume", S::DetailsColumnVolume),
    (GroupField::FolderUsage, "view.group.folder_usage", S::DetailsColumnFolderUsage),
    (GroupField::Tag, "view.group.tag", S::MenuSortTags),
];

const COMPUTER_GROUPS: [(GroupField, &str, S); 7] = [
    (GroupField::None, "view.group.none", S::MenuGroupNone),
    (GroupField::Name, "view.group.name", S::MenuSortName),
    (GroupField::Type, "view.group.type", S::MenuSortType),
    (GroupField::Size, "view.group.size", S::MenuSortSize),
    (GroupField::Free, "view.group.free", S::DetailsColumnFree),
    (GroupField::Percent, "view.group.percent", S::DetailsColumnPercent),
    (GroupField::FileSystem, "view.group.file_system", S::DetailsColumnFileSystem),
];

const BIN_GROUPS: [(GroupField, &str, S); 5] = [
    (GroupField::None, "view.group.none", S::MenuGroupNone),
    (GroupField::Name, "view.group.name", S::MenuSortName),
    (GroupField::Origin, "view.group.origin", S::MenuSortOrigin),
    (GroupField::Modified, "view.group.modified", S::MenuSortDeleted),
    (GroupField::Size, "view.group.size", S::MenuSortSize),
];

const ZOOM_CHOICES: [(u8, &str); 6] = [
    (ZOOM_DETAILS, "view.layout.details"),
    (ZOOM_TREE, "view.layout.tree"),
    (ZOOM_LIST, "view.layout.list"),
    (ZOOM_ICONS, "view.layout.icons_m"),
    (ZOOM_LARGE, "view.layout.icons_l"),
    (ZOOM_XL, "view.layout.icons_xl"),
];

const FLIT_LAYOUTS: [(LayoutMode, &str); 3] = [(LayoutMode::Cards, "view.layout.cards"), (LayoutMode::Columns, "view.layout.columns"), (LayoutMode::Adaptive, "view.layout.adaptive")];


fn trimmed(mut items: Vec<MenuItem>) -> Vec<MenuItem> {
    if items.last() == Some(&MenuItem::Separator) {
        items.pop();
    }
    items
}

pub(super) struct Menu<'a> {
    hidden: &'a [&'static str],
    caps: crate::caps::Caps,
    builtin_archives: bool,
    pub(super) keys: &'a Keymap,
    items: Vec<MenuItem>,
    cmds: Vec<Cmd>,
}

impl Menu<'_> {
    fn shown(&self, id: &str) -> bool {
        !self.hidden.contains(&id) && self.caps.supports(id) && (self.builtin_archives || !crate::caps::is_builtin_archive_action(id))
    }

    pub(super) fn place(&mut self, label: &str, cmd: super::sidebar::PlaceCmd, checked: bool) {
        self.command(Cmd::Place(cmd), label, String::new(), checked);
    }

    pub(super) fn place_icon(&mut self, label: &str, cmd: super::sidebar::PlaceCmd, icon: &'static str, disabled: bool) {
        self.items.push(MenuItem::Command { label: label.to_owned(), keys: String::new(), checked: false, stay: false, mark: Mark::None, icon, disabled });
        self.cmds.push(Cmd::Place(cmd));
    }

    pub(super) fn go(&mut self, label: &str, k: usize, icon: &'static str) {
        self.items.push(MenuItem::Command { label: label.to_owned(), keys: String::new(), checked: false, stay: false, mark: Mark::None, icon, disabled: false });
        self.cmds.push(Cmd::Go(k));
    }

    pub(super) fn header(&mut self, label: &str) {
        self.items.push(MenuItem::Header(label.to_owned()));
    }

    pub(super) fn radio_place(&mut self, label: &str, cmd: super::sidebar::PlaceCmd, icon: &'static str, checked: bool) {
        self.items.push(MenuItem::Command { label: label.to_owned(), keys: String::new(), checked, stay: false, mark: Mark::Radio, icon, disabled: false });
        self.cmds.push(Cmd::Place(cmd));
    }

    pub(super) fn toggle(&mut self, label: &str, cmd: super::sidebar::PlaceCmd, checked: bool) {
        self.items.push(MenuItem::Command { label: label.to_owned(), keys: String::new(), checked, stay: true, mark: Mark::Check, icon: "", disabled: false });
        self.cmds.push(Cmd::Place(cmd));
    }

    fn command(&mut self, cmd: Cmd, label: &str, keys: String, checked: bool) {
        self.marked(cmd, label, keys, checked, Mark::None);
    }

    pub(super) fn entry(&mut self, cmd: Cmd, label: &str, keys: String, mark: Mark, checked: bool, disabled: bool) {
        self.items.push(MenuItem::Command { label: label.to_owned(), keys, checked, stay: mark == Mark::Check && !disabled, mark, icon: "", disabled });
        self.cmds.push(cmd);
    }

    fn marked(&mut self, cmd: Cmd, label: &str, keys: String, checked: bool, mark: Mark) {
        if matches!(cmd, Cmd::Action(id) if !self.shown(id)) {
            return;
        }
        let disabled = matches!(cmd, Cmd::Action(id) if super::actions::ACTIONS.iter().any(|a| a.id == id && a.run.is_none()));
        self.items.push(MenuItem::Command { label: label.to_owned(), keys, checked, stay: false, mark, icon: "", disabled });
        self.cmds.push(cmd);
    }

    pub(super) fn action(&mut self, id: &'static str, label: &str) {
        let keys = self.keys.all_text(id);
        self.command(Cmd::Action(id), label, keys, false);
    }

    pub(super) fn check(&mut self, id: &'static str, label: &str, checked: bool) {
        self.marked(Cmd::Action(id), label, String::new(), checked, Mark::Check);
    }

    fn radio(&mut self, id: &'static str, label: &str, keys: String, checked: bool) {
        self.marked(Cmd::Action(id), label, keys, checked, Mark::Radio);
    }

    fn choices<T: PartialEq>(&mut self, items: &[(T, &'static str, S)], current: T) {
        for (v, id, label) in items {
            self.radio(id, label.t(), String::new(), *v == current);
        }
    }

    pub(super) fn iconed(&mut self, glyph: &'static str) {
        if let Some(MenuItem::Command { icon, .. }) = self.items.last_mut() {
            *icon = glyph;
        }
    }

    pub(super) fn separator(&mut self) {
        if self.items.last().is_some_and(|i| *i != MenuItem::Separator) {
            self.items.push(MenuItem::Separator);
        }
    }

    pub(super) fn submenu(&mut self, label: &str, fill: impl FnOnce(&mut Self)) {
        let outer = std::mem::take(&mut self.items);
        fill(self);
        let items = trimmed(std::mem::replace(&mut self.items, outer));
        if !items.is_empty() {
            self.items.push(MenuItem::Submenu { label: label.to_owned(), items });
        }
    }
}

impl App {
    pub(super) fn selected_folders(&self) -> Vec<Location> {
        let t = self.tab();
        match &t.view {
            Some(v) if !t.view_loc.is_recycle_bin() => v.selected().filter(|&i| v.listing().entries[i as usize].is_dir()).map(|i| t.entry_location(i)).collect(),
            _ => Vec::new(),
        }
    }

    pub(super) fn collect(&mut self, fill: impl FnOnce(&Self, &mut Menu)) -> Vec<MenuItem> {
        let mut m = Menu { hidden: &self.menu_hidden, caps: self.caps, builtin_archives: self.builtin_archives, keys: &self.keymap, items: Vec::new(), cmds: Vec::new() };
        fill(self, &mut m);
        let (items, cmds) = (trimmed(m.items), m.cmds);
        self.menu = cmds;
        items
    }

    pub(super) fn build_menu(&mut self, on_items: bool) -> Vec<MenuItem> {
        self.collect(|a, m| if on_items { a.item_menu(m) } else { a.folder_menu(m) })
    }

    pub(super) fn build_header_menu(&mut self, col: Option<Column>) -> Vec<MenuItem> {
        self.collect(|a, m| {
            let (cols, kinds) = (a.tab().columns, a.column_kinds());
            let shown: Vec<Column> = cols.ordered(kinds).into_iter().filter(|&c| cols.shown(c)).collect();
            if let Some(c) = col {
                let k = shown.iter().position(|&x| x == c).unwrap_or(0);
                let fixed = c == Column::Name;
                m.entry(Cmd::Column(ColumnCmd::Shift(c, false)), S::MenuShiftLeft.t(), String::new(), Mark::None, false, fixed || k <= 1);
                m.entry(Cmd::Column(ColumnCmd::Shift(c, true)), S::MenuShiftRight.t(), String::new(), Mark::None, false, fixed || k + 1 >= shown.len());
                m.entry(Cmd::Column(ColumnCmd::Fit(c)), S::MenuSizeFit.t(), String::new(), Mark::None, false, false);
                m.entry(Cmd::Column(ColumnCmd::Toggle(c)), S::MenuCloseColumn.t(), "MouseMiddle".into(), Mark::None, false, fixed);
                m.separator();
            }
            let keys = m.keys.all_text("view.columns.fit");
            m.entry(Cmd::Action("view.columns.fit"), S::MenuSizeAll.t(), keys, Mark::None, false, false);
            m.entry(Cmd::Column(ColumnCmd::Restore), S::MenuRestoreColumns.t(), String::new(), Mark::None, false, cols.is_default());
            m.separator();
            let listed = MENU_COLUMNS.iter().copied().filter(|c| kinds.contains(c));
            let extras = kinds.iter().copied().filter(|c| !MENU_COLUMNS.contains(c));
            for c in listed.chain(extras) {
                m.entry(Cmd::Column(ColumnCmd::Toggle(c)), c.title(), String::new(), Mark::Check, cols.shown(c), c == Column::Name);
            }
            m.separator();
            m.check("view.columns.autofit", S::MenuAutofitColumns.t(), a.prefs.auto_fit);
        })
    }

    pub(super) fn build_layout_menu(&mut self) -> Vec<MenuItem> {
        self.collect(|a, m| a.layout_entries(m))
    }

    pub(super) fn build_types_menu(&mut self, types: &[(u64, String, usize)]) -> Vec<MenuItem> {
        self.collect(|a, m| {
            let t = a.tab();
            m.entry(Cmd::Types(TypeCmd::All(true)), S::FilterToggleAll.t(), String::new(), Mark::None, false, t.hidden_types.is_empty());
            m.entry(Cmd::Types(TypeCmd::All(false)), S::FilterUntoggleAll.t(), String::new(), Mark::None, false, types.iter().all(|x| t.hidden_types.contains(&x.0)));
            m.separator();
            for (key, label, n) in types {
                m.entry(Cmd::Types(TypeCmd::Toggle(*key)), label, n.to_string(), Mark::Check, !t.hidden_types.contains(key), false);
            }
        })
    }

    pub(super) fn build_search_menu(&mut self, on_row: bool) -> Vec<MenuItem> {
        self.collect(|a, m| {
            if on_row {
                m.command(Cmd::Action("search.reveal"), S::MenuOpenFileLocation.t(), "Ctrl+Enter".into(), false);
                m.separator();
            }
            m.check("search.group_folder", S::MenuGroupByFolder.t(), a.search.group_folder);
        })
    }

    pub(super) fn build_tab_menu(&mut self) -> Vec<MenuItem> {
        self.collect(|a, m| {
            let (n, cur) = (a.tabs.len(), a.cur);
            let groups: [&[(&'static str, S, &'static str, bool)]; 4] = [
                &[("tab.shift_left", S::ActionsTabShiftLeft, "\u{E76B}", cur > 0), ("tab.shift_right", S::ActionsTabShiftRight, "\u{E76C}", cur + 1 < n)],
                &[("tab.duplicate", S::ActionsTabDuplicate, "\u{E8C8}", true), ("tab.duplicate_window", S::ActionsTabDuplicateWindow, "\u{E72A}", true)],
                &[
                    ("tab.close", S::ActionsTabClose, "\u{E8BB}", true),
                    ("tab.close_others", S::ActionsTabCloseOthers, "\u{E8BB}", n > 1),
                    ("tab.close_left", S::ActionsTabCloseLeft, "\u{E8BB}", cur > 0),
                    ("tab.close_right", S::ActionsTabCloseRight, "\u{E8BB}", cur + 1 < n),
                    ("tab.close_all", S::ActionsTabCloseAll, "\u{E8BB}", true),
                ],
                &[("tab.move_to_window", S::MenuMoveTabToWindow, "", n > 1), ("tab.reopen", S::MenuReopenClosedTab, "", !a.nav.closed.is_empty())],
            ];
            for group in groups {
                for &(id, label, icon, live) in group {
                    m.entry(Cmd::Action(id), label.t(), m.keys.all_text(id), Mark::None, false, !live);
                    m.iconed(icon);
                }
                m.separator();
            }
        })
    }

    pub(super) fn build_branch_menu(&mut self) -> Vec<MenuItem> {
        let current = self.git_branch().unwrap_or_default().to_owned();
        let branches = self.git_branches.clone();
        self.collect(|_, m| {
            for (k, b) in branches.iter().enumerate() {
                m.command(Cmd::GitBranch(k), b, String::new(), *b == current);
            }
            m.separator();
            m.action("git.fetch", S::ActionsGitFetch.t());
            m.action("git.pull", S::ActionsGitPull.t());
            m.action("git.push", S::ActionsGitPush.t());
            m.separator();
            m.action("git.open_remote", S::ActionsGitOpenRemote.t());
        })
    }

    pub(super) fn list_menu(&mut self, entries: Vec<(String, Cmd, bool)>) -> Vec<MenuItem> {
        let (items, cmds) = entries.into_iter().map(|(label, cmd, checked)| (MenuItem::command(label, String::new()).checked(checked), cmd)).unzip();
        self.menu = cmds;
        items
    }

    pub(super) fn build_tag_menu(&mut self, tag: Option<usize>) -> Vec<MenuItem> {
        self.collect(|_, m| {
            if let Some(k) = tag {
                m.command(Cmd::ShowTag(k), S::MenuShowTagged.t(), String::new(), false);
                m.command(Cmd::OpenTagged(k), S::MenuOpenTagged.t(), String::new(), false);
                m.separator();
            }
            m.action("tags.new", S::MenuNewTag.t());
            m.action("tags.settings", S::MenuManageTags.t());
        })
    }

    fn view_menus(&self, m: &mut Menu) {
        m.submenu(S::MenusView.t(), |m| self.layout_entries(m));
        m.submenu(S::MenusSort.t(), |m| self.column_sort_entries(m));
        m.submenu(S::MenusGroup.t(), |m| self.group_entries(m));
    }

    fn column_sort_entries(&self, m: &mut Menu) {
        let t = self.tab();
        let sort = t.view_spec.sort;
        let (cols, kinds) = (t.columns, self.column_kinds());
        for c in cols.ordered(kinds).into_iter().filter(|&c| cols.shown(c)) {
            if let Some(f) = c.field() {
                m.entry(Cmd::SortColumn(f), c.title(), String::new(), Mark::Sort(sort.descending), f == sort.field, false);
            }
        }
        m.separator();
        m.choices(&FOLDERS, sort.folders);
    }

    fn layout_entries(&self, m: &mut Menu) {
        let t = self.tab();
        let current = t.mode.zoomed().then(|| zoom_title(t.zoom));
        for (z, id) in ZOOM_CHOICES {
            let keys = m.keys.all_text(id);
            m.radio(id, zoom_title(z), keys, current == Some(zoom_title(z)));
            m.iconed(layout_of_zoom(z).icon());
        }
        m.separator();
        for (mode, id) in FLIT_LAYOUTS {
            let keys = m.keys.all_text(id);
            m.radio(id, mode.title(), keys, mode == t.mode);
            m.iconed(mode.icon());
        }
        m.separator();
        m.action("view.zoom_in", S::MenuLargerItems.t());
        m.action("view.zoom_out", S::MenuSmallerItems.t());
        m.separator();
        m.check("view.thumbnails", S::MenuShowThumbnails.t(), self.prefs.thumbnails);
        m.check("view.auto_sizes", S::ToolbarFlyoutFolderSizes.t(), self.prefs.auto_sizes);
    }

    fn sort_entries(&self, m: &mut Menu) {
        let t = self.tab();
        let sort = t.view_spec.sort;
        let computer = t.view_loc.path().kind() == PathKind::Computer;
        m.choices(if t.view_loc.is_recycle_bin() { &BIN_SORTS[..] } else if computer { &COMPUTER_SORTS[..] } else { &SORTS[..] }, sort.field);
        m.separator();
        m.radio("view.sort.ascending", S::MenuSortAscending.t(), String::new(), !sort.descending);
        m.radio("view.sort.descending", S::MenuSortDescending.t(), String::new(), sort.descending);
        m.separator();
        m.choices(&FOLDERS, sort.folders);
    }

    fn group_entries(&self, m: &mut Menu) {
        let t = self.tab();
        let group = t.view_spec.group;
        let computer = t.view_loc.path().kind() == PathKind::Computer;
        m.choices(if t.view_loc.is_recycle_bin() { &BIN_GROUPS[..] } else if computer { &COMPUTER_GROUPS[..] } else { &GROUPS[..] }, group.field);
    }

    fn new_entries(&self, m: &mut Menu) {
        let keys = m.keys.all_text("file.new_folder");
        m.command(Cmd::NewItem(0), S::MenuFolder.t(), keys, false);
        m.command(Cmd::NewItem(1), S::MenuShortcut.t(), String::new(), false);
        for (k, tpl) in self.nav.sidebar.places.templates.iter().enumerate() {
            m.command(Cmd::NewItem(k + 2), &tpl.label, String::new(), false);
        }
    }

    fn select_entries(&self, m: &mut Menu) {
        m.action("select.all", S::MenuSelectAll.t());
        m.action("select.invert", S::MenuInvertSelection.t());
        m.action("select.none", S::MenuClearSelection.t());
    }

    fn tool_entries(&self, m: &mut Menu, tool: ToolMenu) {
        match tool {
            ToolMenu::New => self.new_entries(m),
            ToolMenu::Select => self.select_entries(m),
            ToolMenu::Sort => self.sort_entries(m),
            ToolMenu::Group => self.group_entries(m),
            ToolMenu::Layout => self.layout_entries(m),
            ToolMenu::Extract if self.archive_view().is_some() => {
                m.action("archive.extract", S::MenuExtractDialog.t());
                m.action("archive.extract_here", S::MenuExtractNextToArchive.t());
            }
            ToolMenu::Extract => {
                m.action("archive.extract", S::MenuExtractFiles.t());
                m.action("archive.extract_here", S::MenuExtractHere.t());
                m.action("archive.smart", S::MenuSmartExtract.t());
                m.action("archive.extract_folder", S::MenuExtractToNamedFolder.t());
                if self.selected_archives().len() == 1 {
                    m.action("archive.open", S::MenuOpenAsFolder.t());
                }
            }
        }
    }

    pub(super) fn build_tool_menu(&mut self, tool: ToolMenu) -> Vec<MenuItem> {
        self.collect(|a, m| a.tool_entries(m, tool))
    }

    pub(super) fn build_overflow_menu(&mut self, items: &[Overflow]) -> Vec<MenuItem> {
        self.collect(|a, m| {
            for item in items {
                match *item {
                    Overflow::Action(id, label) => m.action(id, label),
                    Overflow::Menu(tool, label) => m.submenu(label, |m| a.tool_entries(m, tool)),
                    Overflow::Separator => m.separator(),
                }
            }
        })
    }

    pub(super) fn build_toolbar_menu(&mut self) -> Vec<MenuItem> {
        self.collect(|_, m| {
            m.action("toolbar.customize", S::MenuCustomizeToolbar.t());
            m.action("toolbar.reset", S::MenuResetToolbar.t());
            m.separator();
            m.action("toolbar.toggle", S::MenuHideToolbar.t());
        })
    }

    fn item_menu(&self, m: &mut Menu) {
        let t = self.tab();
        let Some(v) = &t.view else { return };
        if t.view_loc.is_recycle_bin() {
            let shell = self.shell_bin();
            m.action("bin.restore", S::UiBinRestore.t());
            m.separator();
            if shell {
                m.action("file.cut", S::MenuCut.t());
                m.separator();
            }
            m.action("item.trash", S::MenuDelete.t());
            if shell {
                m.separator();
                m.action("item.properties", S::MenuProperties.t());
            }
            return;
        }
        let count = v.selected_count();
        let folders = self.selected_folders();
        let inside = self.archive_view().is_some();
        let files = t.view_loc.path().kind() != PathKind::Computer;
        m.action("item.open", S::MenuOpen.t());
        if !folders.is_empty() {
            m.action("item.open_tab", S::MenuOpenInNewTab.t());
            m.action("item.open_window", S::MenuOpenInNewWindow.t());
            if m.shown("pane.menu") {
                self.item_pane_items(m);
            }
        }
        m.separator();
        if files {
            if !inside {
                m.action("file.cut", S::MenuCut.t());
            }
            m.action("file.copy", S::MenuCopy.t());
            if count == 1 && folders.len() == 1 {
                m.action("file.paste_into", S::MenuPaste.t());
            }
        }
        m.action("item.copy_path", S::MenuCopyPath.t());
        if inside {
            m.separator();
            m.action("archive.extract", S::MenuExtractDialog.t());
            m.action("archive.extract_here", S::MenuExtractNextToArchive.t());
            return;
        }
        if files {
            m.action("file.shortcut", S::MenuCreateShortcut.t());
            m.action("file.rename", S::MenuRename.t());
            m.action("item.trash", S::MenuDelete.t());
        }
        m.separator();
        if folders.len() == count {
            let pinned = folders.iter().all(|l| self.nav.sidebar.state.pinned.contains(l));
            m.action("item.pin", if pinned { S::MenuUnpinFromSidebar.t() } else { S::MenuPinToSidebar.t() });
        }
        if t.view_loc.path().kind() == PathKind::Drive && m.shown("tags.edit") {
            let (any, all) = self.selection_tags();
            m.submenu(S::MenuEditTags.t(), |m| {
                for (k, tag) in self.tag_defs().iter().enumerate() {
                    m.command(Cmd::ToggleTag(k), &tag.name, String::new(), all & (1 << k) != 0);
                }
                m.separator();
                m.action("tags.new", S::MenuNewTag.t());
                if any != 0 {
                    m.action("tags.remove", S::MenuRemoveTags.t());
                }
            });
        }
        if files {
            m.action("file.new_folder_with_selection", S::MenuNewFolderWithSelection.t());
            if count == 1 && folders.is_empty() {
                m.action("file.new_stream", S::MenuNewStream.t());
            }
            if !folders.is_empty() {
                m.action("app.terminal", S::MenuOpenInTerminal.t());
            }
            if count == 1 && folders.len() == 1 {
                m.action("file.folder_icon", S::MenuChangeIcon.t());
                let custom = attr::READONLY | attr::SYSTEM;
                if self.rename_target().is_some_and(|i| v.listing().entries[i as usize].attrs & custom != 0) {
                    m.action("file.folder_icon_reset", S::MenuRestoreDefaultIcon.t());
                }
            }
            if self.show_flatten && !folders.is_empty() && folders.len() == count {
                m.action("file.flatten", S::MenuFlattenFolder.t());
            }
            self.media_entries(m);
            m.separator();
            if m.shown("archive.menu") {
                self.archive_menu(m);
            }
            if count == 1 && folders.is_empty() {
                m.action("file.hashes", S::MenuHashes.t());
            }
        }
        m.separator();
        m.action("item.properties", S::MenuProperties.t());
    }

    fn archive_menu(&self, m: &mut Menu) {
        let archives = self.selected_archives();
        if !archives.is_empty() {
            let own = match archives.as_slice() {
                [one] => crate::tr!(S::MenuExtractTo, name = crate::name::display(archive::stem(one.file_name()))),
                _ => S::MenuExtractEach.t().to_owned(),
            };
            m.submenu(S::MenuExtract.t(), |m| {
                m.action("archive.extract", S::MenuExtractFiles.t());
                m.action("archive.extract_here", S::MenuExtractHere.t());
                m.action("archive.smart", S::MenuSmartExtract.t());
                m.action("archive.extract_folder", &own);
                if archives.len() == 1 {
                    m.action("archive.open", S::MenuOpenAsFolder.t());
                }
            });
        }
        m.submenu(S::MenuCompress.t(), |m| {
            m.action("archive.compress", S::MenuAddToArchive.t());
            m.action("archive.zip", &crate::tr!(S::MenuCompressTo, name = self.quick_name(Format::Zip)));
            m.action("archive.7z", &crate::tr!(S::MenuCompressTo, name = self.quick_name(Format::SevenZ)));
        });
    }

    fn folder_menu(&self, m: &mut Menu) {
        let t = self.tab();
        let files = t.view_loc.path().kind() != PathKind::Computer;
        self.view_menus(m);
        m.action("nav.refresh", S::MenuRefresh.t());
        m.separator();
        if m.shown("pane.menu") {
            self.here_pane_items(m);
        }
        if t.view_loc.is_recycle_bin() {
            m.action("bin.empty", S::UiBinEmpty.t());
            m.action("bin.restore_all", S::SidebarRestoreAll.t());
            if self.shell_bin() {
                m.separator();
                m.action("item.properties", S::MenuProperties.t());
            }
            return;
        }
        if self.archive_view().is_some() {
            m.action("file.paste", S::MenuPaste.t());
            m.action("archive.extract", S::MenuExtractAll.t());
            if !self.encoding_label().is_empty() {
                m.action("archive.encoding", S::MenuEncodingOfNames.t());
            }
            m.action("item.copy_path", S::MenuCopyPath.t());
            return;
        }
        if files {
            if m.shown("file.new_item") {
                m.submenu(S::MenuNew.t(), |m| self.new_entries(m));
            }
            m.action("file.paste", S::MenuPaste.t());
            m.action("app.terminal", S::MenuOpenInTerminal.t());
        }
        let pinned = self.nav.sidebar.state.pinned.contains(t.location());
        m.action("folder.bookmark", if pinned { S::MenuUnpinFromSidebar.t() } else { S::MenuPinToSidebar.t() });
        m.action("item.copy_path", S::MenuCopyPath.t());
        m.separator();
        if files {
            m.action("item.properties", S::MenuProperties.t());
        }
    }

    pub(super) fn menu_command(&mut self, k: usize, now: Instant) {
        match self.menu.get(k).cloned() {
            Some(Cmd::Action(id)) => {
                self.run_action(id, now);
            }
            Some(Cmd::NewItem(n)) => self.new_item(n),
            Some(Cmd::Go(k)) => self.go_target(k, now),
            Some(Cmd::Jump(n)) => self.jump_history(n, now),
            Some(Cmd::Place(c)) => self.place_command(c, now),
            Some(Cmd::ToggleTag(k)) => self.toggle_tag(k),
            Some(Cmd::ShowTag(k)) => self.show_tag(k),
            Some(Cmd::OpenTagged(k)) => self.open_all_tagged(k, now),
            Some(Cmd::Setting(i, choice)) => self.choose_setting(i, choice),
            Some(Cmd::GitBranch(k)) => self.git_switch(k),
            Some(Cmd::Column(c)) => self.column_command(c),
            Some(Cmd::Types(c)) => self.types_command(c),
            Some(Cmd::SortColumn(f)) => self.sort_column(f),
            None => {}
        }
    }
}

#[cfg(test)]
#[path = "../../test/ui/menu.rs"]
mod tests;
