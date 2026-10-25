use super::keymap::Keymap;
use super::toolbar::{Overflow, ToolMenu};
use super::{App, LayoutMode};
use crate::archive::{self, Format};
use crate::location::Location;
use crate::path::PathKind;
use crate::sort::{DateUnit, Folders, GroupField, SortField};
use std::time::Instant;

#[derive(Clone, Debug, PartialEq)]
pub enum MenuItem {
    Command { label: String, keys: String, checked: bool },
    Submenu { label: String, items: Vec<MenuItem> },
    Separator,
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
}

const SORTS: [(SortField, &str, &str); 6] = [
    (SortField::Name, "view.sort.name", "Name"),
    (SortField::Modified, "view.sort.modified", "Date modified"),
    (SortField::Created, "view.sort.created", "Date created"),
    (SortField::Type, "view.sort.type", "Type"),
    (SortField::Size, "view.sort.size", "Size"),
    (SortField::Tag, "view.sort.tag", "Tags"),
];

const BIN_SORTS: [(SortField, &str, &str); 4] = [
    (SortField::Name, "view.sort.name", "Name"),
    (SortField::Origin, "view.sort.origin", "Original location"),
    (SortField::Modified, "view.sort.modified", "Date deleted"),
    (SortField::Size, "view.sort.size", "Size"),
];

const FOLDERS: [(Folders, &str, &str); 3] = [
    (Folders::First, "view.sort.folders_first", "Show folders first"),
    (Folders::Last, "view.sort.files_first", "Show files first"),
    (Folders::Mixed, "view.sort.mixed", "Mix files and folders"),
];

const GROUPS: [(GroupField, &str, &str); 7] = [
    (GroupField::None, "view.group.none", "None"),
    (GroupField::Name, "view.group.name", "Name"),
    (GroupField::Modified, "view.group.modified", "Date modified"),
    (GroupField::Created, "view.group.created", "Date created"),
    (GroupField::Type, "view.group.type", "Type"),
    (GroupField::Size, "view.group.size", "Size"),
    (GroupField::Tag, "view.group.tag", "Tags"),
];

const BIN_GROUPS: [(GroupField, &str, &str); 5] = [
    (GroupField::None, "view.group.none", "None"),
    (GroupField::Name, "view.group.name", "Name"),
    (GroupField::Origin, "view.group.origin", "Original location"),
    (GroupField::Modified, "view.group.modified", "Date deleted"),
    (GroupField::Size, "view.group.size", "Size"),
];

const LAYOUT_CHOICES: [(LayoutMode, &str); 6] = [
    (LayoutMode::Details, "view.layout.details"),
    (LayoutMode::List, "view.layout.list"),
    (LayoutMode::Cards, "view.layout.cards"),
    (LayoutMode::Columns, "view.layout.columns"),
    (LayoutMode::Grid, "view.layout.grid"),
    (LayoutMode::Adaptive, "view.layout.adaptive"),
];

const UNITS: [(DateUnit, &str, &str); 3] = [(DateUnit::Year, "view.group.year", "Year"), (DateUnit::Month, "view.group.month", "Month"), (DateUnit::Day, "view.group.day", "Day")];


fn trimmed(mut items: Vec<MenuItem>) -> Vec<MenuItem> {
    if items.last() == Some(&MenuItem::Separator) {
        items.pop();
    }
    items
}

pub(super) struct Menu<'a> {
    hidden: &'a [&'static str],
    keys: &'a Keymap,
    items: Vec<MenuItem>,
    cmds: Vec<Cmd>,
}

impl Menu<'_> {
    fn shown(&self, id: &str) -> bool {
        !self.hidden.contains(&id)
    }

    pub(super) fn place(&mut self, label: &str, cmd: super::sidebar::PlaceCmd, checked: bool) {
        self.command(Cmd::Place(cmd), label, String::new(), checked);
    }

    fn command(&mut self, cmd: Cmd, label: &str, keys: String, checked: bool) {
        if matches!(cmd, Cmd::Action(id) if !self.shown(id)) {
            return;
        }
        self.items.push(MenuItem::Command { label: label.to_owned(), keys, checked });
        self.cmds.push(cmd);
    }

    pub(super) fn action(&mut self, id: &'static str, label: &str) {
        let keys = self.keys.first_text(id).unwrap_or_default();
        self.command(Cmd::Action(id), label, keys, false);
    }

    pub(super) fn check(&mut self, id: &'static str, label: &str, checked: bool) {
        self.command(Cmd::Action(id), label, String::new(), checked);
    }

    fn choices<T: PartialEq>(&mut self, items: &[(T, &'static str, &'static str)], current: T) {
        for (v, id, label) in items {
            self.check(id, label, *v == current);
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
        let mut m = Menu { hidden: &self.menu_hidden, keys: &self.keymap, items: Vec::new(), cmds: Vec::new() };
        fill(self, &mut m);
        let (items, cmds) = (trimmed(m.items), m.cmds);
        self.menu = cmds;
        items
    }

    pub(super) fn build_menu(&mut self, on_items: bool) -> Vec<MenuItem> {
        self.collect(|a, m| if on_items { a.item_menu(m) } else { a.folder_menu(m) })
    }

    pub(super) fn build_header_menu(&mut self) -> Vec<MenuItem> {
        self.collect(|a, m| {
            let cols = a.tab().columns;
            for &c in a.column_kinds().iter().skip(1) {
                m.check(c.toggle_id(), c.title(), cols.shown(c));
            }
            m.separator();
            m.action("view.columns.fit", "Size all columns to fit");
            m.check("view.columns.autofit", "Always size columns to fit", a.prefs.auto_fit);
        })
    }

    pub(super) fn build_search_menu(&mut self, on_row: bool) -> Vec<MenuItem> {
        self.collect(|a, m| {
            if on_row {
                m.command(Cmd::Action("search.reveal"), "Open file location", "Ctrl+Enter".into(), false);
                m.separator();
            }
            m.check("search.group_folder", "Group by folder", a.search.group_folder);
        })
    }

    pub(super) fn build_tab_menu(&mut self) -> Vec<MenuItem> {
        self.collect(|a, m| {
            let (n, cur) = (a.tabs.len(), a.cur);
            m.action("tab.new", "New tab");
            m.separator();
            if n > 1 {
                m.action("tab.move_to_window", "Move tab to a new window");
                m.separator();
            }
            m.action("tab.close", "Close tab");
            if n > 1 {
                m.action("tab.close_others", "Close other tabs");
            }
            if cur > 0 {
                m.action("tab.close_left", "Close tabs to the left");
            }
            if cur + 1 < n {
                m.action("tab.close_right", "Close tabs to the right");
            }
            if !a.nav.closed.is_empty() {
                m.separator();
                m.action("tab.reopen", "Reopen closed tab");
            }
        })
    }

    pub(super) fn build_tab_actions_menu(&mut self) -> Vec<MenuItem> {
        self.collect(|a, m| {
            m.action("tab.new", "New tab");
            m.action("window.new", "New window");
            if a.tabs.len() > 1 {
                m.action("tab.move_to_window", "Move tab to a new window");
            }
            m.separator();
            m.action("window.compact", "Mini window");
            m.action("window.fullscreen", "Full screen");
            a.tab_pane_items(m);
            m.separator();
            m.action("tab.close_all", "Close all tabs");
            m.separator();
            m.action("settings.open", "Settings");
        })
    }

    pub(super) fn list_menu(&mut self, entries: Vec<(String, Cmd, bool)>) -> Vec<MenuItem> {
        let (items, cmds) = entries.into_iter().map(|(label, cmd, checked)| (MenuItem::Command { label, keys: String::new(), checked }, cmd)).unzip();
        self.menu = cmds;
        items
    }

    pub(super) fn build_tag_menu(&mut self, tag: Option<usize>) -> Vec<MenuItem> {
        self.collect(|_, m| {
            if let Some(k) = tag {
                m.command(Cmd::ShowTag(k), "Show items with this tag", String::new(), false);
                m.command(Cmd::OpenTagged(k), "Open all tagged items", String::new(), false);
                m.separator();
            }
            m.action("tags.new", "New tag");
            m.action("tags.settings", "Manage tags");
        })
    }

    fn view_menus(&self, m: &mut Menu) {
        m.submenu("Layout", |m| self.layout_entries(m));
        m.submenu("Sort by", |m| self.sort_entries(m));
        m.submenu("Group by", |m| self.group_entries(m));
    }

    fn layout_entries(&self, m: &mut Menu) {
        let t = self.tab();
        for (mode, id) in LAYOUT_CHOICES {
            let keys = m.keys.first_text(id).unwrap_or_default();
            m.command(Cmd::Action(id), mode.title(), keys, mode == t.mode);
        }
        m.separator();
        m.action("view.size.larger", "Larger items");
        m.action("view.size.smaller", "Smaller items");
        m.separator();
        m.check("view.thumbnails", "Show thumbnails", self.prefs.thumbnails);
    }

    fn sort_entries(&self, m: &mut Menu) {
        let t = self.tab();
        let sort = t.view_spec.sort;
        m.choices(if t.view_loc.is_recycle_bin() { &BIN_SORTS[..] } else { &SORTS[..] }, sort.field);
        m.separator();
        m.check("view.sort.ascending", "Ascending", !sort.descending);
        m.check("view.sort.descending", "Descending", sort.descending);
        m.separator();
        m.choices(&FOLDERS, sort.folders);
    }

    fn group_entries(&self, m: &mut Menu) {
        let t = self.tab();
        let group = t.view_spec.group;
        m.choices(if t.view_loc.is_recycle_bin() { &BIN_GROUPS[..] } else { &GROUPS[..] }, group.field);
        if group.field != GroupField::None {
            m.separator();
            m.check("view.group.ascending", "Ascending", !group.descending);
            m.check("view.group.descending", "Descending", group.descending);
        }
        if group.field.is_date() {
            m.separator();
            m.choices(&UNITS, group.unit);
        }
    }

    fn new_entries(&self, m: &mut Menu) {
        let keys = m.keys.first_text("file.new_folder").unwrap_or_default();
        m.command(Cmd::NewItem(0), "Folder", keys, false);
        m.command(Cmd::NewItem(1), "Shortcut", String::new(), false);
        for (k, tpl) in self.nav.sidebar.places.templates.iter().enumerate() {
            m.command(Cmd::NewItem(k + 2), &tpl.label, String::new(), false);
        }
    }

    fn select_entries(&self, m: &mut Menu) {
        m.action("select.all", "Select all");
        m.action("select.invert", "Invert selection");
        m.action("select.none", "Clear selection");
    }

    fn tool_entries(&self, m: &mut Menu, tool: ToolMenu) {
        match tool {
            ToolMenu::New => self.new_entries(m),
            ToolMenu::Select => self.select_entries(m),
            ToolMenu::Sort => self.sort_entries(m),
            ToolMenu::Group => self.group_entries(m),
            ToolMenu::Layout => self.layout_entries(m),
            ToolMenu::Extract if self.archive_view().is_some() => {
                m.action("archive.extract", "Extract…");
                m.action("archive.extract_here", "Extract next to the archive");
            }
            ToolMenu::Extract => {
                m.action("archive.extract", "Extract files…");
                m.action("archive.extract_here", "Extract here");
                m.action("archive.smart", "Smart extract");
                m.action("archive.extract_folder", "Extract to a folder named after the archive");
                if self.selected_archives().len() == 1 {
                    m.action("archive.open", "Open as folder");
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
            m.action("toolbar.customize", "Customize the toolbar");
            m.action("toolbar.reset", "Reset the toolbar to defaults");
            m.separator();
            m.action("toolbar.toggle", "Hide the toolbar");
        })
    }

    fn item_menu(&self, m: &mut Menu) {
        let t = self.tab();
        let Some(v) = &t.view else { return };
        let count = v.selected_count();
        let folders = self.selected_folders();
        let inside = self.archive_view().is_some();
        let files = t.view_loc.path().kind() != PathKind::Computer;
        m.action("item.open", "Open");
        if !folders.is_empty() {
            m.action("item.open_tab", "Open in new tab");
            m.action("item.open_window", "Open in new window");
            if m.shown("pane.menu") {
                self.item_pane_items(m);
            }
        }
        m.separator();
        if files {
            if !inside {
                m.action("file.cut", "Cut");
            }
            m.action("file.copy", "Copy");
            if count == 1 && folders.len() == 1 {
                m.action("file.paste_into", "Paste");
            }
        }
        m.action("item.copy_path", "Copy path");
        if inside {
            m.separator();
            m.action("archive.extract", "Extract…");
            m.action("archive.extract_here", "Extract next to the archive");
            return;
        }
        if files {
            m.action("file.shortcut", "Create shortcut");
            m.action("file.rename", "Rename");
            m.action("item.trash", "Delete");
        }
        m.separator();
        if folders.len() == count {
            let pinned = folders.iter().all(|l| self.nav.sidebar.state.pinned.contains(l));
            m.action("item.pin", if pinned { "Unpin from sidebar" } else { "Pin to sidebar" });
        }
        if t.view_loc.path().kind() == PathKind::Drive && m.shown("tags.edit") {
            let (any, all) = self.selection_tags();
            m.submenu("Edit tags", |m| {
                for (k, tag) in self.tag_defs().iter().enumerate() {
                    m.command(Cmd::ToggleTag(k), &tag.name, String::new(), all & (1 << k) != 0);
                }
                m.separator();
                m.action("tags.new", "New tag");
                if any != 0 {
                    m.action("tags.remove", "Remove tags");
                }
            });
        }
        if files {
            m.action("file.new_folder_with_selection", "New folder with selection");
            if count == 1 && folders.is_empty() {
                m.action("file.new_stream", "New alternate data stream");
            }
            if !folders.is_empty() {
                m.action("app.terminal", "Open in terminal");
            }
            if count == 1 && folders.len() == 1 {
                m.action("file.folder_icon", "Change icon");
            }
            if self.show_flatten && !folders.is_empty() && folders.len() == count {
                m.action("file.flatten", "Flatten folder");
            }
            m.separator();
            if m.shown("archive.menu") {
                self.archive_menu(m, count, folders.is_empty());
            }
        }
        m.separator();
        m.action("item.properties", "Properties");
    }

    fn archive_menu(&self, m: &mut Menu, count: usize, no_folders: bool) {
        let archives = self.selected_archives();
        if !archives.is_empty() {
            let own = match archives.as_slice() {
                [one] => format!("Extract to \u{201C}{}\\\u{201D}", crate::name::display(archive::stem(one.file_name()))),
                _ => "Extract each to its own folder".to_owned(),
            };
            m.submenu("Extract", |m| {
                m.action("archive.extract", "Extract files…");
                m.action("archive.extract_here", "Extract here");
                m.action("archive.smart", "Smart extract");
                m.action("archive.extract_folder", &own);
                if archives.len() == 1 {
                    m.action("archive.open", "Open as folder");
                }
            });
        }
        m.submenu("Compress", |m| {
            m.action("archive.compress", "Add to archive…");
            m.action("archive.zip", &format!("Compress to \u{201C}{}\u{201D}", self.quick_name(Format::Zip)));
            m.action("archive.7z", &format!("Compress to \u{201C}{}\u{201D}", self.quick_name(Format::SevenZ)));
        });
        if count == 1 && no_folders {
            m.action("file.hashes", "Hashes…");
        }
    }

    fn folder_menu(&self, m: &mut Menu) {
        let t = self.tab();
        let files = t.view_loc.path().kind() != PathKind::Computer;
        self.view_menus(m);
        m.action("nav.refresh", "Refresh");
        m.separator();
        if m.shown("pane.menu") {
            self.here_pane_items(m);
        }
        if t.view_loc.is_recycle_bin() {
            return;
        }
        if self.archive_view().is_some() {
            m.action("file.paste", "Paste");
            m.action("archive.extract", "Extract all…");
            if !self.sizes_label().is_empty() {
                m.action("archive.encoding", "Encoding of names…");
            }
            m.action("item.copy_path", "Copy path");
            return;
        }
        if files {
            if m.shown("file.new_item") {
                m.submenu("New", |m| self.new_entries(m));
            }
            m.action("file.paste", "Paste");
            m.action("app.terminal", "Open in terminal");
        }
        let pinned = self.nav.sidebar.state.pinned.contains(t.location());
        m.action("sidebar.pin", if pinned { "Unpin from sidebar" } else { "Pin to sidebar" });
        m.action("item.copy_path", "Copy path");
        m.separator();
        if files {
            m.action("item.properties", "Properties");
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
            None => {}
        }
    }
}

#[cfg(test)]
#[path = "../../test/ui/menu.rs"]
mod tests;
