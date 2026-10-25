use super::sidebar::KnownFolder;
use super::{App, Effect, Route};
use crate::input::{Key, Mods};
use crate::location::Location;
use crate::open::OpenTarget;
use crate::sort::{SortField, SortSpec};
use std::time::Instant;

const MAX_CLOSED: usize = 32;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Chord {
    pub key: Key,
    pub mods: Mods,
}

const fn chord(key: Key, ctrl: bool, shift: bool, alt: bool) -> Chord {
    Chord { key, mods: Mods { ctrl, shift, alt } }
}

pub const fn key(k: Key) -> Chord {
    chord(k, false, false, false)
}

pub const fn ctrl(k: Key) -> Chord {
    chord(k, true, false, false)
}

pub const fn ctrl_shift(k: Key) -> Chord {
    chord(k, true, true, false)
}

pub const fn alt(k: Key) -> Chord {
    chord(k, false, false, true)
}

pub const fn ctrl_alt(k: Key) -> Chord {
    chord(k, true, false, true)
}

pub const fn shift(k: Key) -> Chord {
    chord(k, false, true, false)
}

pub struct Action {
    pub id: &'static str,
    pub title: &'static str,
    pub keys: &'static [Chord],
    pub run: fn(&mut App, Instant),
}

impl Action {
    const fn new(id: &'static str, title: &'static str, keys: &'static [Chord], run: fn(&mut App, Instant)) -> Self {
        Self { id, title, keys, run }
    }
}

pub static ACTIONS: &[Action] = &[
    Action::new("palette.show", "Show command palette", &[ctrl_shift(Key::Char('P'))], |a, _| a.open_palette()),
    Action::new("nav.back", "Go back", &[alt(Key::Left), key(Key::Backspace)], |a, now| a.go_history(false, now)),
    Action::new("nav.forward", "Go forward", &[alt(Key::Right)], |a, now| a.go_history(true, now)),
    Action::new("nav.up", "Go up to the parent folder", &[alt(Key::Up)], |a, now| a.go_up(now)),
    Action::new("nav.refresh", "Refresh", &[key(Key::F(5)), ctrl(Key::Char('R'))], |a, now| a.refresh(now)),
    Action::new("nav.address", "Focus the address bar", &[ctrl(Key::Char('L')), alt(Key::Char('D')), key(Key::F(4))], |a, _| a.begin_edit()),
    Action::new("nav.home", "Go to the home folder", &[], |a, now| a.go_known(KnownFolder::Home, now)),
    Action::new("nav.computer", "Go to This PC", &[], |a, now| a.navigate(Location::computer(), now)),
    Action::new("nav.recycle_bin", "Go to the Recycle Bin", &[], |a, now| a.navigate(Location::recycle_bin(), now)),
    Action::new("tab.new", "New tab", &[ctrl(Key::Char('T'))], |a, now| a.new_tab_here(now)),
    Action::new("tab.close", "Close tab", &[ctrl(Key::Char('W')), ctrl(Key::F(4))], |a, _| a.close_tab(a.cur)),
    Action::new("tab.reopen", "Reopen closed tab", &[ctrl_shift(Key::Char('T'))], |a, now| a.reopen_tab(now)),
    Action::new("tab.next", "Next tab", &[ctrl(Key::Tab)], |a, _| a.select_tab((a.cur + 1) % a.tabs.len())),
    Action::new("tab.previous", "Previous tab", &[ctrl_shift(Key::Tab)], |a, _| a.select_tab((a.cur + a.tabs.len() - 1) % a.tabs.len())),
    Action::new("window.new", "New window", &[ctrl(Key::Char('N'))], |a, _| a.new_window_here()),
    Action::new("view.filter", "Filter items in this folder", &[ctrl(Key::Char('F'))], |a, _| a.open_filter()),
    Action::new("search.open", "Search all drives", &[ctrl(Key::Char('E')), ctrl_shift(Key::Char('F'))], |a, _| a.open_search()),
    Action::new("view.hidden", "Show or hide hidden files", &[ctrl(Key::Char('H'))], |a, _| a.toggle_hidden()),
    Action::new("view.sort.name", "Sort by name", &[], |a, _| a.sort_by(SortField::Name)),
    Action::new("view.sort.modified", "Sort by date modified", &[], |a, _| a.sort_by(SortField::Modified)),
    Action::new("view.sort.type", "Sort by type", &[], |a, _| a.sort_by(SortField::Type)),
    Action::new("view.sort.size", "Sort by size", &[], |a, _| a.sort_by(SortField::Size)),
    Action::new("view.sort.origin", "Sort by original location", &[], |a, _| a.sort_by(SortField::Origin)),
    Action::new("view.sort.reverse", "Reverse sort order", &[], |a, _| a.reverse_sort()),
    Action::new("view.sizes", "Calculate folder sizes", &[ctrl_shift(Key::Char('S'))], |a, _| a.compute_dir_sizes()),
    Action::new("sidebar.toggle", "Show or hide the sidebar", &[ctrl(Key::Char('B'))], |a, _| a.toggle_sidebar()),
    Action::new("sidebar.focus", "Switch focus between sidebar and list", &[key(Key::F(6))], |a, _| a.cycle_sidebar_focus()),
    Action::new("sidebar.pin", "Pin or unpin the current folder", &[ctrl(Key::Char('D'))], |a, _| a.pin_current()),
    Action::new("select.all", "Select all", &[ctrl(Key::Char('A'))], |a, _| a.select_all()),
    Action::new("select.none", "Clear selection", &[key(Key::Escape)], |a, _| a.clear_selection()),
    Action::new("item.open", "Open selected items", &[key(Key::Enter)], |a, now| a.open_selection(now)),
    Action::new("item.copy_path", "Copy path", &[ctrl_shift(Key::Char('C'))], |a, _| a.copy_paths(false)),
    Action::new("item.copy_quoted_path", "Copy path in quotes", &[ctrl_alt(Key::Char('C'))], |a, _| a.copy_paths(true)),
    Action::new("app.terminal", "Open in terminal", &[ctrl(Key::Char('`'))], |a, _| a.open_terminal(false)),
    Action::new("app.terminal_admin", "Open in terminal as administrator", &[ctrl_shift(Key::Char('`'))], |a, _| a.open_terminal(true)),
    Action::new("item.trash", "Move to Recycle Bin", &[key(Key::Delete)], |a, _| a.trash_selection()),
    Action::new("item.delete", "Delete permanently", &[shift(Key::Delete)], |a, _| a.delete_selection()),
    Action::new("bin.restore", "Restore selected items from the Recycle Bin", &[], |a, _| a.restore_selection()),
    Action::new("bin.restore_all", "Restore all items from the Recycle Bin", &[], |a, _| a.restore_all()),
    Action::new("bin.empty", "Empty the Recycle Bin", &[], |a, _| a.empty_bin()),
    Action::new("file.copy", "Copy", &[ctrl(Key::Char('C'))], |a, _| a.copy_selection()),
    Action::new("file.cut", "Cut", &[ctrl(Key::Char('X'))], |a, _| a.cut_selection()),
    Action::new("file.paste", "Paste", &[ctrl(Key::Char('V'))], |a, _| a.paste()),
    Action::new("file.paste_into", "Paste into the selected folder", &[ctrl_shift(Key::Char('V'))], |a, _| a.paste_into_selection()),
    Action::new("file.rename", "Rename", &[key(Key::F(2))], |a, _| a.rename_focused()),
    Action::new("file.new_folder", "New folder", &[ctrl_shift(Key::Char('N'))], |a, _| a.new_folder()),
    Action::new("file.new_item", "New item", &[ctrl_shift(Key::Char('I'))], |a, _| a.new_item_dialog()),
    Action::new("file.new_folder_with_selection", "New folder with the selected items", &[], |a, _| a.new_folder_with_selection()),
    Action::new("file.new_stream", "New alternate data stream", &[], |a, _| a.new_stream()),
    Action::new("file.flatten", "Flatten folder: move files from all subfolders into it", &[], |a, _| a.flatten_selection()),
    Action::new("ops.show", "Show or hide file operations", &[], |a, _| a.toggle_jobs()),
    Action::new("ops.clear", "Clear completed file operations", &[], |a, _| a.clear_completed()),
    Action::new("edit.undo", "Undo", &[ctrl(Key::Char('Z'))], |a, _| a.undo()),
    Action::new("edit.redo", "Redo", &[ctrl(Key::Char('Y'))], |a, _| a.redo()),
    Action::new("item.properties", "Properties", &[alt(Key::Enter)], |a, _| a.properties()),
    Action::new("item.context_menu", "Show context menu", &[shift(Key::F(10)), key(Key::Apps)], |a, _| a.context_menu()),
];

pub fn find(key: Key, mods: Mods) -> Option<&'static Action> {
    let pressed = Chord { key, mods };
    ACTIONS.iter().find(|a| a.keys.contains(&pressed))
}

pub fn chord_text(c: &Chord) -> String {
    let mut s = String::new();
    for (on, name) in [(c.mods.ctrl, "Ctrl+"), (c.mods.shift, "Shift+"), (c.mods.alt, "Alt+")] {
        if on {
            s.push_str(name);
        }
    }
    match c.key {
        Key::Char(ch) => s.push(ch),
        Key::F(n) => s.push_str(&format!("F{n}")),
        k => s.push_str(match k {
            Key::Up => "Up",
            Key::Down => "Down",
            Key::Left => "Left",
            Key::Right => "Right",
            Key::PageUp => "PgUp",
            Key::PageDown => "PgDn",
            Key::Home => "Home",
            Key::End => "End",
            Key::Enter => "Enter",
            Key::Escape => "Esc",
            Key::Backspace => "Backspace",
            Key::Delete => "Del",
            Key::Tab => "Tab",
            Key::Space => "Space",
            Key::Apps => "Menu",
            _ => "?",
        }),
    }
    s
}

pub fn keys_text(a: &Action) -> String {
    a.keys.iter().map(chord_text).collect::<Vec<_>>().join(", ")
}

impl App {
    pub(super) fn run_bound(&mut self, key: Key, mods: Mods, now: Instant) -> bool {
        let Some(a) = find(key, mods) else { return false };
        (a.run)(self, now);
        true
    }

    pub fn run_action(&mut self, id: &str, now: Instant) -> bool {
        let Some(a) = ACTIONS.iter().find(|a| a.id == id) else { return false };
        self.dirty = true;
        (a.run)(self, now);
        true
    }

    pub(super) fn remember_closed(&mut self, loc: Location) {
        if self.nav.closed.len() == MAX_CLOSED {
            self.nav.closed.remove(0);
        }
        self.nav.closed.push(loc);
    }

    fn reopen_tab(&mut self, now: Instant) {
        if let Some(folder) = self.nav.closed.pop() {
            self.dispatch(vec![Route { folder, select: None }], OpenTarget::NewTab, now);
        }
    }

    fn new_window_here(&mut self) {
        let folder = self.current_location().clone();
        self.effects.push(Effect::NewWindow(vec![Route { folder, select: None }]));
    }

    fn go_known(&mut self, k: KnownFolder, now: Instant) {
        if let Some(loc) = self.known_folder(k) {
            self.navigate(loc, now);
        }
    }

    fn toggle_hidden(&mut self) {
        let k = self.cur;
        let t = self.tab_mut();
        t.view_spec.show_hidden = !t.view_spec.show_hidden;
        self.resort(k);
    }

    fn sort_by(&mut self, field: SortField) {
        let k = self.cur;
        let s = &mut self.tab_mut().view_spec.sort;
        if s.field != field {
            *s = SortSpec { field, descending: false };
            self.resort(k);
        }
    }

    fn reverse_sort(&mut self) {
        let k = self.cur;
        let s = &mut self.tab_mut().view_spec.sort;
        s.descending = !s.descending;
        self.resort(k);
    }

    fn select_all(&mut self) {
        if let Some(v) = &mut self.tab_mut().view {
            v.select_all();
        }
    }

    fn clear_selection(&mut self) {
        if let Some(v) = &mut self.tab_mut().view {
            v.clear_selection();
        }
    }
}

#[cfg(test)]
#[path = "../../test/ui/actions.rs"]
mod tests;
