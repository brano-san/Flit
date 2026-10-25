use super::actions::{chord_text, ACTIONS};
use super::toolbar::{Overflow, ToolMenu};
use super::{App, LayoutMode};
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

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Cmd {
    Action(&'static str),
    NewItem(usize),
    ToggleTag(usize),
    ShowTag(usize),
    OpenTagged(usize),
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

fn keys_of(id: &str) -> String {
    ACTIONS.iter().find(|a| a.id == id).and_then(|a| a.keys.first()).map(chord_text).unwrap_or_default()
}

fn trimmed(mut items: Vec<MenuItem>) -> Vec<MenuItem> {
    if items.last() == Some(&MenuItem::Separator) {
        items.pop();
    }
    items
}

struct Menu<'a> {
    hidden: &'a [&'static str],
    items: Vec<MenuItem>,
    cmds: Vec<Cmd>,
}

impl Menu<'_> {
    fn shown(&self, id: &str) -> bool {
        !self.hidden.contains(&id)
    }

    fn command(&mut self, cmd: Cmd, label: &str, keys: String, checked: bool) {
        if matches!(cmd, Cmd::Action(id) if !self.shown(id)) {
            return;
        }
        self.items.push(MenuItem::Command { label: label.to_owned(), keys, checked });
        self.cmds.push(cmd);
    }

    fn action(&mut self, id: &'static str, label: &str) {
        self.command(Cmd::Action(id), label, keys_of(id), false);
    }

    fn check(&mut self, id: &'static str, label: &str, checked: bool) {
        self.command(Cmd::Action(id), label, String::new(), checked);
    }

    fn choices<T: PartialEq>(&mut self, items: &[(T, &'static str, &'static str)], current: T) {
        for (v, id, label) in items {
            self.check(id, label, *v == current);
        }
    }

    fn separator(&mut self) {
        if self.items.last().is_some_and(|i| *i != MenuItem::Separator) {
            self.items.push(MenuItem::Separator);
        }
    }

    fn submenu(&mut self, label: &str, fill: impl FnOnce(&mut Self)) {
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

    fn collect(&mut self, fill: impl FnOnce(&Self, &mut Menu)) -> Vec<MenuItem> {
        let mut m = Menu { hidden: &self.menu_hidden, items: Vec::new(), cmds: Vec::new() };
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

    pub(super) fn build_tag_menu(&mut self, tag: Option<usize>) -> Vec<MenuItem> {
        self.collect(|_, m| {
            if let Some(k) = tag {
                m.command(Cmd::ShowTag(k), "Show items with this tag", String::new(), false);
                m.command(Cmd::OpenTagged(k), "Open all tagged items", String::new(), false);
                m.separator();
            }
            m.action("tags.new", "New tag");
            m.action("tags.manage", "Manage tags");
        })
    }

    pub(super) fn build_search_menu(&mut self) -> Vec<MenuItem> {
        self.collect(|a, m| m.check("search.group_folder", "Group by folder", a.search.group_folder))
    }

    fn view_menus(&self, m: &mut Menu) {
        m.submenu("Layout", |m| self.layout_entries(m));
        m.submenu("Sort by", |m| self.sort_entries(m));
        m.submenu("Group by", |m| self.group_entries(m));
    }

    fn layout_entries(&self, m: &mut Menu) {
        let t = self.tab();
        for (mode, id) in LAYOUT_CHOICES {
            m.command(Cmd::Action(id), mode.title(), keys_of(id), mode == t.mode);
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
        m.command(Cmd::NewItem(0), "Folder", keys_of("file.new_folder"), false);
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
        let files = t.view_loc.path().kind() != PathKind::Computer;
        m.action("item.open", "Open");
        if !folders.is_empty() {
            m.action("item.open_tab", "Open in new tab");
            m.action("item.open_window", "Open in new window");
        }
        m.separator();
        if files {
            m.action("file.cut", "Cut");
            m.action("file.copy", "Copy");
            if count == 1 && folders.len() == 1 {
                m.action("file.paste_into", "Paste");
            }
        }
        m.action("item.copy_path", "Copy path");
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
        if t.view_loc.path().kind() == PathKind::Drive {
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
        }
        m.separator();
        m.action("item.properties", "Properties");
    }

    fn folder_menu(&self, m: &mut Menu) {
        let t = self.tab();
        let files = t.view_loc.path().kind() != PathKind::Computer;
        self.view_menus(m);
        m.action("nav.refresh", "Refresh");
        m.separator();
        if t.view_loc.is_recycle_bin() {
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
        match self.menu.get(k).copied() {
            Some(Cmd::Action(id)) => {
                self.run_action(id, now);
            }
            Some(Cmd::NewItem(n)) => self.new_item(n),
            Some(Cmd::ToggleTag(k)) => self.toggle_tag(k),
            Some(Cmd::ShowTag(k)) => self.show_tag(k),
            Some(Cmd::OpenTagged(k)) => self.open_all_tagged(k, now),
            None => {}
        }
    }
}

#[cfg(test)]
#[path = "../../test/ui/menu.rs"]
mod tests;
