use super::home::Widget;
use super::sidebar::KnownFolder;
use super::tags::TagPick;
use super::{App, Column, Effect, LayoutMode, Page, Route, WindowCmd};
use crate::archive::{Format, Place};
use crate::input::{Key, Mods};
use crate::library::LibraryOp;
use crate::location::Location;
use crate::path::PathKind;
use crate::open::OpenTarget;
use crate::sort::{DateUnit, Folders, GroupField, SortField};
use std::time::Instant;

const MAX_CLOSED: usize = 32;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
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

pub const fn alt_shift(k: Key) -> Chord {
    chord(k, false, true, true)
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
    Action::new("nav.back", "Go back", &[alt(Key::Left), key(Key::Backspace), key(Key::BrowserBack)], |a, now| a.go_history(false, now)),
    Action::new("nav.forward", "Go forward", &[alt(Key::Right), key(Key::BrowserForward)], |a, now| a.go_history(true, now)),
    Action::new("nav.up", "Go up to the parent folder", &[alt(Key::Up)], |a, now| a.go_up(now)),
    Action::new("nav.refresh", "Refresh", &[key(Key::F(5)), ctrl(Key::Char('R'))], |a, now| a.refresh(now)),
    Action::new("nav.address", "Focus the address bar", &[ctrl(Key::Char('L')), alt(Key::Char('D')), key(Key::F(4))], |a, _| a.begin_edit()),
    Action::new("nav.home", "Go to the home folder", &[], |a, now| a.go_known(KnownFolder::Home, now)),
    Action::new("nav.home_page", "Go to the Home page", &[], |a, now| a.go_home_page(false, now)),
    Action::new("nav.computer", "Go to This PC", &[], |a, now| a.navigate(Location::computer(), now)),
    Action::new("nav.recycle_bin", "Go to the Recycle Bin", &[], |a, now| a.navigate(Location::recycle_bin(), now)),
    Action::new("tab.new", "New tab", &[ctrl(Key::Char('T'))], |a, now| a.new_tab_here(now)),
    Action::new("tab.close", "Close tab", &[ctrl(Key::Char('W')), ctrl(Key::F(4))], |a, _| a.close_tab(a.cur)),
    Action::new("tab.reopen", "Reopen closed tab", &[ctrl_shift(Key::Char('T'))], |a, now| a.reopen_tab(now)),
    Action::new("tab.close_all", "Close all tabs", &[ctrl_shift(Key::Char('W'))], |a, _| a.close_all_tabs()),
    Action::new("tab.close_others", "Close other tabs", &[], |a, _| a.close_tabs(|i, cur| i == cur)),
    Action::new("tab.close_left", "Close tabs to the left", &[], |a, _| a.close_tabs(|i, cur| i >= cur)),
    Action::new("tab.close_right", "Close tabs to the right", &[], |a, _| a.close_tabs(|i, cur| i <= cur)),
    Action::new("tab.move_to_window", "Move tab to a new window", &[], |a, _| a.move_tab_to_window()),
    Action::new("tab.next", "Next tab", &[ctrl(Key::Tab)], |a, _| a.select_tab((a.cur + 1) % a.tabs.len())),
    Action::new("tab.previous", "Previous tab", &[ctrl_shift(Key::Tab)], |a, _| a.select_tab((a.cur + a.tabs.len() - 1) % a.tabs.len())),
    Action::new("pane.toggle", "Toggle dual pane", &[ctrl_shift(Key::Char('S'))], |a, now| a.run_pane("pane.toggle", now)),
    Action::new("pane.split_vertical", "Split vertically: open this folder in a pane on the right", &[alt_shift(Key::Char('V'))], |a, now| a.run_pane("pane.split_vertical", now)),
    Action::new("pane.split_horizontal", "Split horizontally: open this folder in a pane below", &[alt_shift(Key::Char('H'))], |a, now| a.run_pane("pane.split_horizontal", now)),
    Action::new("pane.arrange_vertical", "Arrange panes side by side", &[], |a, now| a.run_pane("pane.arrange_vertical", now)),
    Action::new("pane.arrange_horizontal", "Arrange panes one above the other", &[], |a, now| a.run_pane("pane.arrange_horizontal", now)),
    Action::new("pane.close", "Close active pane", &[ctrl_alt(Key::Char('W'))], |a, now| a.run_pane("pane.close", now)),
    Action::new("pane.focus", "Focus other pane", &[ctrl_shift(Key::Right)], |a, now| a.run_pane("pane.focus", now)),
    Action::new("pane.open_other", "Open the selected folder in the other pane", &[ctrl_shift(Key::Enter)], |a, now| a.run_pane("pane.open_other", now)),
    Action::new("pane.here_other", "Open this folder in the other pane", &[], |a, now| a.run_pane("pane.here_other", now)),
    Action::new("pane.new_vertical", "Open the selected folder in a new pane on the right", &[], |a, now| a.run_pane("pane.new_vertical", now)),
    Action::new("pane.new_horizontal", "Open the selected folder in a new pane below", &[], |a, now| a.run_pane("pane.new_horizontal", now)),
    Action::new("pane.dual_default", "Open new tabs with two panes", &[], |a, now| a.run_pane("pane.dual_default", now)),
    Action::new("pane.default_direction", "Switch new panes between side by side and one above the other", &[], |a, now| a.run_pane("pane.default_direction", now)),
    Action::new("window.new", "New window", &[ctrl(Key::Char('N'))], |a, _| a.new_window_here()),
    Action::new("window.fullscreen", "Full screen", &[key(Key::F(11))], |a, _| a.effects.push(Effect::Window(WindowCmd::Fullscreen))),
    Action::new("window.compact", "Mini window on top of other windows", &[key(Key::F(12))], |a, _| a.effects.push(Effect::Window(WindowCmd::Compact(None)))),
    Action::new("window.compact_on", "Enter the mini window", &[ctrl_alt(Key::Up)], |a, _| a.effects.push(Effect::Window(WindowCmd::Compact(Some(true))))),
    Action::new("window.compact_off", "Leave the mini window", &[ctrl_alt(Key::Down)], |a, _| a.effects.push(Effect::Window(WindowCmd::Compact(Some(false))))),
    Action::new("view.filter", "Filter items in this folder", &[ctrl(Key::Char('F'))], |a, _| a.open_filter()),
    Action::new("search.open", "Search all drives", &[ctrl(Key::Char('E')), ctrl_shift(Key::Char('F'))], |a, _| a.open_search()),
    Action::new("search.group_folder", "Group search results by folder", &[], |a, _| a.toggle_search_groups()),
    Action::new("search.folder", "Search this folder and its subfolders", &[key(Key::F(3))], |a, _| a.open_folder_search()),
    Action::new("search.clear_history", "Clear the history of searches", &[], |a, _| a.clear_search_history()),
    Action::new("search.reveal", "Open the location of the found item", &[], |a, now| a.reveal_search_focus(now)),
    Action::new("view.hidden", "Show or hide hidden files", &[ctrl(Key::Char('H'))], |a, _| a.toggle_hidden()),
    Action::new("view.sort.name", "Sort by name", &[], |a, _| a.sort_by(SortField::Name)),
    Action::new("view.sort.modified", "Sort by date modified", &[], |a, _| a.sort_by(SortField::Modified)),
    Action::new("view.sort.type", "Sort by type", &[], |a, _| a.sort_by(SortField::Type)),
    Action::new("view.sort.size", "Sort by size", &[], |a, _| a.sort_by(SortField::Size)),
    Action::new("view.sort.origin", "Sort by original location", &[], |a, _| a.sort_by(SortField::Origin)),
    Action::new("view.sort.created", "Sort by date created", &[], |a, _| a.sort_by(SortField::Created)),
    Action::new("view.sort.tag", "Sort by tags", &[], |a, _| a.sort_by(SortField::Tag)),
    Action::new("view.sort.reverse", "Reverse sort order", &[], |a, _| a.set_descending(None)),
    Action::new("view.sort.ascending", "Sort in ascending order", &[], |a, _| a.set_descending(Some(false))),
    Action::new("view.sort.descending", "Sort in descending order", &[], |a, _| a.set_descending(Some(true))),
    Action::new("view.sort.folders_first", "Show folders before files", &[], |a, _| a.set_folders(Folders::First)),
    Action::new("view.sort.files_first", "Show files before folders", &[], |a, _| a.set_folders(Folders::Last)),
    Action::new("view.sort.mixed", "Mix files and folders when sorting", &[], |a, _| a.set_folders(Folders::Mixed)),
    Action::new("view.group.none", "No grouping", &[], |a, _| a.group_by(GroupField::None)),
    Action::new("view.group.name", "Group by name", &[], |a, _| a.group_by(GroupField::Name)),
    Action::new("view.group.type", "Group by type", &[], |a, _| a.group_by(GroupField::Type)),
    Action::new("view.group.size", "Group by size", &[], |a, _| a.group_by(GroupField::Size)),
    Action::new("view.group.modified", "Group by date modified", &[], |a, _| a.group_by(GroupField::Modified)),
    Action::new("view.group.created", "Group by date created", &[], |a, _| a.group_by(GroupField::Created)),
    Action::new("view.group.origin", "Group by original location", &[], |a, _| a.group_by(GroupField::Origin)),
    Action::new("view.group.tag", "Group by tags", &[], |a, _| a.group_by(GroupField::Tag)),
    Action::new("view.group.ascending", "Show groups in ascending order", &[], |a, _| a.set_group_descending(false)),
    Action::new("view.group.descending", "Show groups in descending order", &[], |a, _| a.set_group_descending(true)),
    Action::new("view.group.year", "Group dates by year", &[], |a, _| a.set_date_unit(DateUnit::Year)),
    Action::new("view.group.month", "Group dates by month", &[], |a, _| a.set_date_unit(DateUnit::Month)),
    Action::new("view.group.day", "Group dates by day", &[], |a, _| a.set_date_unit(DateUnit::Day)),
    Action::new("view.layout.details", "Details layout", &[ctrl_shift(Key::Char('1'))], |a, _| a.set_layout(LayoutMode::Details)),
    Action::new("view.layout.list", "List layout", &[ctrl_shift(Key::Char('2'))], |a, _| a.set_layout(LayoutMode::List)),
    Action::new("view.layout.cards", "Cards layout", &[ctrl_shift(Key::Char('3'))], |a, _| a.set_layout(LayoutMode::Cards)),
    Action::new("view.layout.columns", "Columns layout", &[ctrl_shift(Key::Char('4'))], |a, _| a.set_layout(LayoutMode::Columns)),
    Action::new("view.layout.grid", "Grid layout", &[ctrl_shift(Key::Char('5'))], |a, _| a.set_layout(LayoutMode::Grid)),
    Action::new("view.layout.adaptive", "Adaptive layout", &[ctrl_shift(Key::Char('6'))], |a, _| a.set_layout(LayoutMode::Adaptive)),
    Action::new("view.size.larger", "Larger items", &[ctrl(Key::Char('+')), ctrl_shift(Key::Char('+'))], |a, _| a.resize_items(1)),
    Action::new("view.size.smaller", "Smaller items", &[ctrl(Key::Char('-'))], |a, _| a.resize_items(-1)),
    Action::new("view.thumbnails", "Show thumbnails in the Grid and Cards layouts", &[], |a, _| a.toggle_pref(|p| &mut p.thumbnails)),
    Action::new("view.thumbnails.clear", "Clear the thumbnail cache", &[], |a, _| a.clear_thumbnails()),
    Action::new("view.columns.fit", "Size all columns to fit", &[ctrl(Key::NumPlus)], |a, _| a.request_fit()),
    Action::new("view.columns.autofit", "Always size columns to fit", &[], |a, _| a.toggle_auto_fit()),
    Action::new("view.column.modified", "Show or hide the Date modified column", &[], |a, _| a.toggle_column(Column::Modified)),
    Action::new("view.column.created", "Show or hide the Date created column", &[], |a, _| a.toggle_column(Column::Created)),
    Action::new("view.column.type", "Show or hide the Type column", &[], |a, _| a.toggle_column(Column::Type)),
    Action::new("view.column.size", "Show or hide the Size column", &[], |a, _| a.toggle_column(Column::Size)),
    Action::new("view.column.origin", "Show or hide the Original location column", &[], |a, _| a.toggle_column(Column::Origin)),
    Action::new("view.column.deleted", "Show or hide the Date deleted column", &[], |a, _| a.toggle_column(Column::Deleted)),
    Action::new("view.column.tag", "Show or hide the Tags column", &[], |a, _| a.toggle_column(Column::Tag)),
    Action::new("view.dotfiles", "Show or hide dotfiles", &[], |a, _| a.toggle_pref(|p| &mut p.show_dotfiles)),
    Action::new("view.system", "Show or hide protected operating system files", &[], |a, _| a.toggle_pref(|p| &mut p.show_system)),
    Action::new("view.extensions", "Show or hide file name extensions", &[], |a, _| a.toggle_pref(|p| &mut p.show_extensions)),
    Action::new("view.status_bar", "Show or hide the status bar", &[], |a, _| a.toggle_pref(|p| &mut p.status_bar)),
    Action::new("view.item_count", "Count files and folders separately", &[], |a, _| a.toggle_pref(|p| &mut p.split_count)),
    Action::new("view.streams", "Show or hide alternate data streams", &[], |a, now| a.toggle_streams(now)),
    Action::new("view.checkboxes", "Show or hide item check boxes", &[], |a, _| a.toggle_pref(|p| &mut p.checkboxes)),
    Action::new("view.hover_select", "Select items by hovering", &[], |a, _| a.toggle_pref(|p| &mut p.hover_select)),
    Action::new("view.auto_sizes", "Calculate folder sizes automatically", &[], |a, _| a.toggle_pref(|p| &mut p.auto_sizes)),
    Action::new("view.same_for_all", "Use the same view for all folders", &[], |a, _| a.toggle_pref(|p| &mut p.same_for_all)),
    Action::new("view.set_default", "Use this view for folders without their own", &[], |a, _| a.set_default_view()),
    Action::new("view.sizes", "Calculate folder sizes", &[ctrl_alt(Key::Char('S'))], |a, _| a.compute_dir_sizes()),
    Action::new("sidebar.toggle", "Show or hide the sidebar", &[ctrl(Key::Char('B'))], |a, _| a.toggle_sidebar()),
    Action::new("sidebar.focus", "Switch focus between sidebar and list", &[key(Key::F(6))], |a, _| a.cycle_sidebar_focus()),
    Action::new("sidebar.pin", "Pin or unpin the current folder", &[ctrl(Key::Char('D'))], |a, _| a.pin_current()),
    Action::new("toolbar.toggle", "Show or hide the toolbar", &[ctrl_shift(Key::Char('B'))], |a, _| a.toggle_toolbar()),
    Action::new("toolbar.customize", "Customize the toolbar", &[], |a, _| a.customize_toolbar()),
    Action::new("toolbar.reset", "Reset the toolbar to defaults", &[], |a, _| a.reset_toolbar()),
    Action::new("sidebar.compact", "Switch between the full and the compact sidebar", &[], |a, _| a.toggle_compact()),
    Action::new("sidebar.sections", "Show and expand all sidebar sections", &[], |a, _| a.show_all_sections()),
    Action::new("home.quick_access", "Show or hide Quick access on the Home page", &[], |a, _| a.toggle_widget(Widget::QuickAccess)),
    Action::new("home.drives", "Show or hide Drives on the Home page", &[], |a, _| a.toggle_widget(Widget::Drives)),
    Action::new("home.tags", "Show or hide Tags on the Home page", &[], |a, _| a.toggle_widget(Widget::Tags)),
    Action::new("home.recent","Show or hide Recent files on the Home page", &[], |a, _| a.toggle_widget(Widget::Recent)),
    Action::new("home.clear_recent", "Clear the list of recent files on the Home page", &[], |a, _| a.clear_recent()),
    Action::new("libraries.new", "New library", &[], |a, _| a.ask_new_library()),
    Action::new("libraries.restore", "Restore the default libraries", &[], |a, _| a.effects.push(Effect::Library(LibraryOp::RestoreDefaults))),
    Action::new("library.locations", "Choose the folders of this library", &[], |a, _| {
        if let Some(lib) = a.current_library() {
            a.library_dialog(lib);
        }
    }),
    Action::new("select.all", "Select all", &[ctrl(Key::Char('A'))], |a, _| a.select_all()),
    Action::new("select.none", "Clear selection", &[key(Key::Escape), ctrl_shift(Key::Char('A'))], |a, _| a.clear_selection()),
    Action::new("select.invert", "Invert selection", &[ctrl(Key::Char('I'))], |a, _| a.invert_selection()),
    Action::new("item.open", "Open selected items", &[key(Key::Enter)], |a, now| a.open_selection(now)),
    Action::new("item.open_tab", "Open the selected folders in new tabs", &[ctrl(Key::Enter)], |a, now| a.open_folders_in(OpenTarget::NewTab, now)),
    Action::new("item.open_window", "Open the selected folders in a new window", &[ctrl_alt(Key::Enter)], |a, now| a.open_folders_in(OpenTarget::NewWindow, now)),
    Action::new("item.pin", "Pin or unpin the selected folders", &[], |a, _| a.pin_selection()),
    Action::new("tags.new", "New tag for the selected items", &[], |a, _| a.new_tag(true)),
    Action::new("tags.remove", "Remove all tags from the selected items", &[], |a, _| a.remove_tags()),
    Action::new("tags.manage", "Manage tags: rename, recolor or delete", &[], |a, _| a.open_tag_pick(TagPick::Manage, "")),
    Action::new("tags.open_all", "Open all items with a tag", &[], |a, _| a.open_tag_pick(TagPick::OpenAll, "")),
    Action::new("tags.settings", "Open the Tags page of the settings", &[], |a, _| a.open_settings(Some(Page::Tags))),
    Action::new("settings.open", "Settings", &[ctrl(Key::Char(','))], |a, _| a.open_settings(None)),
    Action::new("settings.file", "Open the settings file in a text editor", &[ctrl_shift(Key::Char(','))], |a, _| a.open_settings_file()),
    Action::new("settings.export", "Export settings and tags to a file", &[], |a, _| a.ask_export_settings()),
    Action::new("settings.import", "Import settings and tags from a file", &[], |a, _| a.ask_import_settings()),
    Action::new("settings.reset", "Reset all settings to their defaults", &[], |a, _| a.ask_reset_settings()),
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
    Action::new("file.share", "Share", &[], |a, _| a.share_selection()),
    Action::new("file.new_folder", "New folder", &[ctrl_shift(Key::Char('N'))], |a, _| a.new_folder()),
    Action::new("file.new_item", "New item", &[ctrl_shift(Key::Char('I'))], |a, _| a.new_item_dialog()),
    Action::new("file.new_folder_with_selection", "New folder with the selected items", &[], |a, _| a.new_folder_with_selection()),
    Action::new("file.new_stream", "New alternate data stream", &[], |a, _| a.new_stream()),
    Action::new("file.folder_icon", "Change the folder icon", &[], |a, _| a.ask_folder_icon()),
    Action::new("file.shortcut", "Create shortcuts to the selected items", &[], |a, _| a.create_shortcuts()),
    Action::new("file.flatten", "Flatten folder: move files from all subfolders into it", &[], |a, _| a.flatten_selection()),
    Action::new("archive.compress", "Add to archive…", &[], |a, _| a.compress_dialog()),
    Action::new("archive.zip", "Compress to ZIP", &[], |a, _| a.compress_quick(Format::Zip)),
    Action::new("archive.7z", "Compress to 7z", &[], |a, _| a.compress_quick(Format::SevenZ)),
    Action::new("archive.extract", "Extract files…", &[alt(Key::Char('E'))], |a, _| a.extract_dialog()),
    Action::new("archive.extract_here", "Extract here", &[], |a, _| a.extract_with(Place::Here)),
    Action::new("archive.extract_folder", "Extract to a folder named after the archive", &[], |a, _| a.extract_with(Place::OwnFolder)),
    Action::new("archive.smart", "Smart extract", &[ctrl_shift(Key::Char('E'))], |a, _| a.extract_with(Place::Smart)),
    Action::new("archive.open", "Open the archive as a folder", &[], |a, now| a.open_as_folder(now)),
    Action::new("archive.encoding", "Choose the encoding of names in the archive", &[], |a, _| a.pick_encoding()),
    Action::new("file.hashes", "Hashes…", &[], |a, _| a.hashes_dialog()),
    Action::new("ops.show", "Show or hide file operations", &[], |a, _| a.toggle_jobs()),
    Action::new("ops.clear", "Clear completed file operations", &[], |a, _| a.clear_completed()),
    Action::new("edit.undo", "Undo", &[ctrl(Key::Char('Z'))], |a, _| a.undo()),
    Action::new("edit.redo", "Redo", &[ctrl(Key::Char('Y'))], |a, _| a.redo()),
    Action::new("item.properties", "Properties", &[alt(Key::Enter)], |a, _| a.properties()),
    Action::new("view.pane", "Show or hide the info and preview pane", &[ctrl_alt(Key::Char('I'))], |a, _| a.toggle_preview_pane()),
    Action::new("view.pane.preview", "Show the Preview tab of the pane", &[alt(Key::Char('P'))], |a, _| a.show_preview_tab(false)),
    Action::new("view.pane.details", "Show the Details tab of the pane", &[alt_shift(Key::Char('P'))], |a, _| a.show_preview_tab(true)),
    Action::new("item.quick_view", "Quick view", &[], |a, _| a.quick_view()),
    Action::new("preview.play", "Play or pause the previewed media", &[], |a, _| a.play_preview()),
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
        Key::NumPlus => s.push_str("Num+"),
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
            Key::Insert => "Ins",
            Key::Tab => "Tab",
            Key::Space => "Space",
            Key::Apps => "Menu",
            Key::BrowserBack => "BrowserBack",
            Key::BrowserForward => "BrowserForward",
            _ => "?",
        }),
    }
    s
}

pub fn keys_text(a: &Action) -> String {
    a.keys.iter().map(chord_text).collect::<Vec<_>>().join(", ")
}

pub fn index(id: &str) -> Option<usize> {
    ACTIONS.iter().position(|a| a.id == id)
}

impl App {
    pub(super) fn run_bound(&mut self, key: Key, mods: Mods, now: Instant) -> bool {
        let Some(i) = self.keymap.owner(Chord { key, mods }) else { return false };
        (ACTIONS[i].run)(self, now);
        true
    }

    pub fn run_action(&mut self, id: &str, now: Instant) -> bool {
        let Some(a) = ACTIONS.iter().find(|a| a.id == id) else { return false };
        self.dirty = true;
        (a.run)(self, now);
        true
    }

    pub fn available(&self, id: &str) -> bool {
        let kind = self.current_location().path().kind();
        let (home, bin, library) = (kind == PathKind::Home, kind == PathKind::RecycleBin, self.current_library().is_some());
        let no_files = home || bin || kind == PathKind::Computer;
        match id.split('.').next().unwrap_or_default() {
            _ if id.ends_with("origin") || id == "view.column.deleted" => bin,
            "bin" => bin,
            "home" => home,
            "library" => library,
            "pane" => self.pane_available(id),
            "search" if id == "search.group_folder" => self.search.active,
            "search" if id == "search.folder" => !no_files,
            "search" if id == "search.clear_history" => !self.search_history().is_empty(),
            "file" | "archive" => !no_files,
            "view" | "select" | "item" => !home,
            "app" => !(no_files || library),
            _ => !(id == "nav.up" && self.current_location().parent().is_none()),
        }
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

    fn open_folders_in(&mut self, target: OpenTarget, now: Instant) {
        let routes: Vec<Route> = self.selected_folders().into_iter().map(|folder| Route { folder, select: None }).collect();
        if routes.is_empty() {
            return self.status("Select the folders to open");
        }
        if target == OpenTarget::NewTab {
            return self.open_in_tabs(routes, now);
        }
        self.dispatch(routes, target, now);
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

    fn invert_selection(&mut self) {
        if let Some(v) = &mut self.tab_mut().view {
            v.invert_selection();
        }
    }
}

#[cfg(test)]
#[path = "../../test/ui/actions.rs"]
mod tests;
