use super::actions::{chord_text, ACTIONS};
use super::App;
use crate::location::Location;
use crate::path::PathKind;
use crate::sort::SortField;
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
}

const SORTS: [(SortField, &str, &str); 4] = [
    (SortField::Name, "view.sort.name", "Name"),
    (SortField::Modified, "view.sort.modified", "Date modified"),
    (SortField::Type, "view.sort.type", "Type"),
    (SortField::Size, "view.sort.size", "Size"),
];

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

    pub(super) fn build_menu(&mut self, on_items: bool) -> Vec<MenuItem> {
        let mut m = Menu { hidden: &self.menu_hidden, items: Vec::new(), cmds: Vec::new() };
        if on_items {
            self.item_menu(&mut m);
        } else {
            self.folder_menu(&mut m);
        }
        let (items, cmds) = (trimmed(m.items), m.cmds);
        self.menu = cmds;
        items
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
        if files {
            m.action("file.new_folder_with_selection", "New folder with selection");
            if count == 1 && folders.is_empty() {
                m.action("file.new_stream", "New alternate data stream");
            }
            if !folders.is_empty() {
                m.action("app.terminal", "Open in terminal");
            }
        }
        m.separator();
        m.action("item.properties", "Properties");
    }

    fn folder_menu(&self, m: &mut Menu) {
        let t = self.tab();
        let files = t.view_loc.path().kind() != PathKind::Computer;
        let sort = t.view_spec.sort;
        m.submenu("Sort by", |m| {
            for (field, id, label) in SORTS {
                m.command(Cmd::Action(id), label, String::new(), sort.field == field);
            }
            m.separator();
            m.command(Cmd::Action("view.sort.reverse"), "Descending", String::new(), sort.descending);
        });
        m.action("nav.refresh", "Refresh");
        m.separator();
        if files {
            if m.shown("file.new_item") {
                m.submenu("New", |m| {
                    m.command(Cmd::NewItem(0), "Folder", keys_of("file.new_folder"), false);
                    m.command(Cmd::NewItem(1), "Shortcut", String::new(), false);
                    for (k, tpl) in self.nav.sidebar.places.templates.iter().enumerate() {
                        m.command(Cmd::NewItem(k + 2), &tpl.label, String::new(), false);
                    }
                });
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
            None => {}
        }
    }
}

#[cfg(test)]
#[path = "../../test/ui/menu.rs"]
mod tests;
