use super::home::Widget;
use super::layouts::LAYOUTS;
use super::tabbar::Startup;
use super::{App, Column, Effect, Persisted, SplitDir};
use crate::archive::Format;
use crate::fmt::DateFormat;
use crate::location::Location;
use crate::path::HOME_LABEL;
use crate::open::OpenTarget;
use crate::provider::Resolve;
use crate::sort::{DateUnit, Folders, GroupField, SortField};
use crate::tags::{self, Tag, TagRequest, MAX_TAGS};
use super::keymap::{self, Keymap};
use serde_json::{Map, Value as Json};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

const MAX_PATHS: usize = 64;
const MAX_TEXT: usize = 4096;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Page {
    General,
    Appearance,
    Files,
    Layout,
    Tags,
    Actions,
    Advanced,
    About,
}

impl Page {
    pub const ALL: [Page; 8] = [Page::General, Page::Appearance, Page::Files, Page::Layout, Page::Tags, Page::Actions, Page::Advanced, Page::About];

    pub fn title(self) -> &'static str {
        match self {
            Page::General => "General",
            Page::Appearance => "Appearance",
            Page::Files => "Files and folders",
            Page::Layout => "Layout",
            Page::Tags => "Tags",
            Page::Actions => "Actions",
            Page::Advanced => "Advanced",
            Page::About => "About",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ConfirmDelete {
    Always,
    #[default]
    Permanent,
    Never,
}

pub type Choices = &'static [(&'static str, &'static str)];

#[derive(Clone, Copy)]
pub enum Kind {
    Toggle(fn(&App) -> bool, fn(&mut App, bool)),
    Choice(Choices, fn(&App) -> usize, fn(&mut App, usize)),
    Number { min: i64, max: i64, step: i64, unit: &'static str, get: fn(&App) -> i64, set: fn(&mut App, i64) },
    Paths(fn(&App) -> Vec<Location>, fn(&mut App, Vec<Location>)),
    Text(fn(&App) -> String, fn(&mut App, String)),
    Keys(fn(&App) -> Arc<Keymap>, fn(&mut App, Arc<Keymap>)),
}

pub struct Setting {
    pub id: &'static str,
    pub page: Page,
    pub title: &'static str,
    pub help: &'static str,
    pub kind: Kind,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Bool(bool),
    Choice(usize),
    Number(i64),
    Paths(Vec<Location>),
    Text(String),
    Keys(Arc<Keymap>),
}

const fn toggle(id: &'static str, page: Page, title: &'static str, help: &'static str, get: fn(&App) -> bool, set: fn(&mut App, bool)) -> Setting {
    Setting { id, page, title, help, kind: Kind::Toggle(get, set) }
}

const fn choice(id: &'static str, page: Page, title: &'static str, help: &'static str, choices: Choices, get: fn(&App) -> usize, set: fn(&mut App, usize)) -> Setting {
    Setting { id, page, title, help, kind: Kind::Choice(choices, get, set) }
}

fn pos<T: PartialEq>(all: &[T], v: &T) -> usize {
    all.iter().position(|x| x == v).unwrap_or(0)
}

fn shown_in_menu(a: &App, id: &str) -> bool {
    !a.menu_hidden.contains(&id)
}

fn show_in_menu(a: &mut App, id: &'static str, on: bool) {
    a.menu_hidden.retain(|h| *h != id);
    if !on {
        a.menu_hidden.push(id);
    }
}

fn set_widget(a: &mut App, w: Widget, on: bool) {
    if a.widget_shown(w) != on {
        a.toggle_widget(w);
    }
}

fn set_column(a: &mut App, c: Column, on: bool) {
    if a.prefs.defaults.columns.shown(c) != on {
        a.prefs.defaults.columns.hidden ^= 1 << c as u8;
    }
}

const STARTUP: Choices = &[("continue", "Continue where you left off"), ("new_tab", "Open the Home page in a new tab"), ("pages", "Open specific pages")];
const DATES: [DateFormat; 3] = [DateFormat::Universal, DateFormat::System, DateFormat::Application];
const DATE_CHOICES: Choices = &[("universal", "Universal: 2024-03-15 14:07"), ("system", "As in Windows regional settings"), ("application", "Friendly: Today 14:07, Yesterday, Monday")];
const UNITS: Choices = &[("binary", "Binary: 1 KB is 1024 bytes, as in Explorer"), ("decimal", "Decimal: 1 kB is 1000 bytes")];
const CONFIRMS: [ConfirmDelete; 3] = [ConfirmDelete::Always, ConfirmDelete::Permanent, ConfirmDelete::Never];
const CONFIRM_CHOICES: Choices = &[("always", "Always"), ("permanent", "Only when deleting permanently"), ("never", "Never")];
const CONFLICTS: [Option<Resolve>; 4] = [None, Some(Resolve::KeepBoth), Some(Resolve::Replace), Some(Resolve::Skip)];
const CONFLICT_CHOICES: Choices = &[("ask", "Ask every time"), ("keep_both", "Keep both"), ("replace", "Replace"), ("skip", "Skip")];
const LAYOUT_CHOICES: Choices = &[("details", "Details"), ("list", "List"), ("cards", "Cards"), ("columns", "Columns"), ("grid", "Grid"), ("adaptive", "Adaptive")];
const SORTS: [SortField; 6] = [SortField::Name, SortField::Modified, SortField::Created, SortField::Size, SortField::Type, SortField::Tag];
const SORT_CHOICES: Choices = &[("name", "Name"), ("modified", "Date modified"), ("created", "Date created"), ("size", "Size"), ("type", "Type"), ("tags", "Tags")];
const FOLDERS: [Folders; 3] = [Folders::First, Folders::Last, Folders::Mixed];
const FOLDER_CHOICES: Choices = &[("first", "Folders first"), ("last", "Files first"), ("mixed", "Files and folders together")];
const GROUPS: [GroupField; 7] = [GroupField::None, GroupField::Name, GroupField::Modified, GroupField::Created, GroupField::Size, GroupField::Type, GroupField::Tag];
const GROUP_CHOICES: Choices = &[("none", "No grouping"), ("name", "Name"), ("modified", "Date modified"), ("created", "Date created"), ("size", "Size"), ("type", "Type"), ("tags", "Tags")];
const UNITS_OF_DATE: [DateUnit; 3] = [DateUnit::Year, DateUnit::Month, DateUnit::Day];
const DATE_UNIT_CHOICES: Choices = &[("year", "Year"), ("month", "Month"), ("day", "Day")];
const SPLITS: [SplitDir; 2] = [SplitDir::Vertical, SplitDir::Horizontal];
const SPLIT_CHOICES: Choices = &[("vertical", "Side by side"), ("horizontal", "One above the other")];
const ARCHIVES: [Format; 4] = Format::WRITABLE;
const ARCHIVE_CHOICES: Choices = &[("zip", "ZIP"), ("7z", "7z"), ("tar", "TAR"), ("gz", "GZip")];

pub static SETTINGS: &[Setting] = &[
    choice("general.startup", Page::General, "When Flit starts", "What the first window opens when Flit starts without folders on the command line", STARTUP, |a| match a.startup {
        Startup::Continue => 0,
        Startup::NewTab => 1,
        Startup::Folders(_) => 2,
    }, |a, i| a.set_startup(i)),
    Setting { id: "general.startup_pages", page: Page::General, title: "Pages to open at startup", help: "Used when Flit starts with specific pages: folders or the Home page, each in its own tab", kind: Kind::Paths(|a| a.startup_pages.clone(), |a, v| a.set_startup_pages(v)) },
    toggle("general.open_in_running", Page::General, "Open folders from other apps in a new tab", "When off, a folder opened from another app or the command line gets a new window of the running Flit", |a| a.open_target != OpenTarget::NewWindow, |a, on| a.open_target = if on { OpenTarget::NewTab } else { OpenTarget::NewWindow }),
    toggle("general.switch_to_new_tab", Page::General, "Switch to newly opened tabs", "When off, folders opened in new tabs from the list, the sidebar or the Home page open in the background", |a| a.switch_to_new_tab, |a, on| a.switch_to_new_tab = on),
    toggle("general.dual_pane", Page::General, "Open new tabs with two panes", "New tabs show a second pane next to the first; Ctrl+Shift+S switches the pane of the current tab", |a| a.split_prefs.dual, |a, on| a.split_prefs.dual = on),
    choice("general.split_direction", Page::General, "Where new panes open", "For new tabs with two panes and for Ctrl+Shift+S", SPLIT_CHOICES, |a| pos(&SPLITS, &a.split_prefs.dir), |a, i| a.split_prefs.dir = SPLITS[i]),
    toggle("general.smooth_scroll", Page::General, "Smooth scrolling", "Animate scrolling of the file list instead of jumping", |a| a.smooth_scroll, |a, on| a.smooth_scroll = on),
    toggle("general.tab_wheel_reversed", Page::General, "Reverse the mouse wheel over tabs", "Scrolling down over the tab strip switches to the previous tab instead of the next", |a| a.tab_wheel_reversed, |a, on| a.tab_wheel_reversed = on),
    choice("general.date_format", Page::General, "Date format", "How dates are shown in the list, search results, the preview pane and dialogs", DATE_CHOICES, |a| pos(&DATES, &a.style.dates), |a, i| a.set_style(|s| s.dates = DATES[i])),
    toggle("general.home_quick_access", Page::General, "Show Quick access on the Home page", "Pinned and well-known folders as cards", |a| a.widget_shown(Widget::QuickAccess), |a, on| set_widget(a, Widget::QuickAccess, on)),
    toggle("general.home_drives", Page::General, "Show Drives on the Home page", "Drives with their free space", |a| a.widget_shown(Widget::Drives), |a, on| set_widget(a, Widget::Drives, on)),
    toggle("general.home_tags", Page::General, "Show Tags on the Home page", "A card for every tag", |a| a.widget_shown(Widget::Tags), |a, on| set_widget(a, Widget::Tags, on)),
    toggle("general.home_recent", Page::General, "Show Recent files on the Home page", "Recently opened files and folders from Windows", |a| a.widget_shown(Widget::Recent), |a, on| set_widget(a, Widget::Recent, on)),
    toggle("general.shell_submenu", Page::General, "Put Windows shell items into a submenu", "Items added by other programs go to «Show more options»; Shift+right-click still shows them all", |a| !a.shell_items_inline, |a, on| a.shell_items_inline = !on),
    toggle("general.menu_open_tab", Page::General, "Show «Open in new tab» in the context menu", "For folders", |a| shown_in_menu(a, "item.open_tab"), |a, on| show_in_menu(a, "item.open_tab", on)),
    toggle("general.menu_open_window", Page::General, "Show «Open in new window» in the context menu", "For folders", |a| shown_in_menu(a, "item.open_window"), |a, on| show_in_menu(a, "item.open_window", on)),
    toggle("general.menu_copy_path", Page::General, "Show «Copy path» in the context menu", "Copies the full paths of the selected items", |a| shown_in_menu(a, "item.copy_path"), |a, on| show_in_menu(a, "item.copy_path", on)),
    toggle("general.menu_shortcut", Page::General, "Show «Create shortcut» in the context menu", "Creates shortcuts next to the selected items", |a| shown_in_menu(a, "file.shortcut"), |a, on| show_in_menu(a, "file.shortcut", on)),
    toggle("general.menu_new_folder_with_selection", Page::General, "Show «New folder with selection» in the context menu", "Moves the selected items into a new folder", |a| shown_in_menu(a, "file.new_folder_with_selection"), |a, on| show_in_menu(a, "file.new_folder_with_selection", on)),
    toggle("general.menu_stream", Page::General, "Show «New alternate data stream» in the context menu", "For a single selected file", |a| shown_in_menu(a, "file.new_stream"), |a, on| show_in_menu(a, "file.new_stream", on)),
    toggle("general.menu_panes", Page::General, "Show pane commands in the context menu", "«Open in other pane», «Open in new pane» and «Close pane»", |a| shown_in_menu(a, "pane.menu"), |a, on| show_in_menu(a, "pane.menu", on)),
    toggle("general.menu_pin", Page::General, "Show «Pin to sidebar» in the context menu", "For folders", |a| shown_in_menu(a, "item.pin"), |a, on| show_in_menu(a, "item.pin", on)),
    toggle("general.menu_terminal", Page::General, "Show «Open in terminal» in the context menu", "For folders and the folder background", |a| shown_in_menu(a, "app.terminal"), |a, on| show_in_menu(a, "app.terminal", on)),
    toggle("general.menu_tags", Page::General, "Show «Edit tags» in the context menu", "For items on local NTFS drives", |a| shown_in_menu(a, "tags.edit"), |a, on| show_in_menu(a, "tags.edit", on)),
    toggle("general.menu_compress", Page::General, "Show «Compress» and «Extract» in the context menu", "Archive commands for the selected items", |a| shown_in_menu(a, "archive.menu"), |a, on| show_in_menu(a, "archive.menu", on)),
    toggle("appearance.toolbar", Page::Appearance, "Show the toolbar", "The command bar under the address bar; Ctrl+Shift+B switches it too", |a| a.toolbar.state.visible, |a, on| {
        if a.toolbar.state.visible != on {
            a.toggle_toolbar();
        }
    }),
    toggle("files.show_hidden", Page::Files, "Show hidden files and folders", "Applies to all tabs; Ctrl+H still switches the current tab", |a| a.show_hidden, |a, on| a.set_show_hidden(on)),
    toggle("files.dotfiles", Page::Files, "Show files whose names start with a dot", "Dotfiles from Linux and development tools", |a| a.prefs.show_dotfiles, |a, on| a.set_pref(|p| &mut p.show_dotfiles, on)),
    toggle("files.system", Page::Files, "Show protected operating system files", "Hidden system files such as desktop.ini and pagefile.sys", |a| a.prefs.show_system, |a, on| a.set_pref(|p| &mut p.show_system, on)),
    toggle("files.streams", Page::Files, "Show alternate data streams", "NTFS streams of files appear as «name:stream» rows, for viewing only", |a| a.prefs.show_streams, |a, on| {
        if a.prefs.show_streams != on {
            a.toggle_streams(Instant::now());
        }
    }),
    toggle("files.extensions", Page::Files, "Show file name extensions", "When off, extensions of all files are hidden", |a| a.prefs.show_extensions, |a, on| a.set_pref(|p| &mut p.show_extensions, on)),
    toggle("files.thumbnails", Page::Files, "Show thumbnails", "Pictures and videos in the Grid and Cards layouts", |a| a.prefs.thumbnails, |a, on| a.set_pref(|p| &mut p.thumbnails, on)),
    toggle("files.checkboxes", Page::Files, "Show check boxes to select items", "A check box on every row toggles it without clearing the selection", |a| a.prefs.checkboxes, |a, on| a.set_pref(|p| &mut p.checkboxes, on)),
    toggle("files.hover_select", Page::Files, "Select items by hovering", "Resting the pointer on a row for 0.6 s selects it", |a| a.prefs.hover_select, |a, on| a.set_pref(|p| &mut p.hover_select, on)),
    toggle("files.single_click_files", Page::Files, "Open files with a single click", "Ctrl and Shift clicks still only select", |a| a.single_click_files, |a, on| a.single_click_files = on),
    toggle("files.single_click_folders", Page::Files, "Open folders with a single click", "Ctrl and Shift clicks still only select", |a| a.single_click_folders, |a, on| a.single_click_folders = on),
    toggle("files.single_click_columns", Page::Files, "Open folders with a single click in the Columns layout", "Only in the Columns layout", |a| a.single_click_columns, |a, on| a.single_click_columns = on),
    toggle("files.folders_in_new_tab", Page::Files, "Open folders in a new tab", "Double click or Enter on a folder in the list opens it in a new tab", |a| a.folders_in_new_tab, |a, on| a.folders_in_new_tab = on),
    toggle("files.double_click_up", Page::Files, "Double click on empty space goes up", "Opens the parent folder", |a| a.double_click_up, |a, on| a.double_click_up = on),
    toggle("files.select_previous", Page::Files, "Select the folder you came from", "After going up or back, the folder you left is selected and scrolled into view", |a| a.select_previous, |a, on| a.select_previous = on),
    choice("files.confirm_delete", Page::Files, "Ask before deleting", "Emptying the Recycle Bin always asks", CONFIRM_CHOICES, |a| pos(&CONFIRMS, &a.confirm_delete), |a, i| a.confirm_delete = CONFIRMS[i]),
    toggle("files.warn_extension", Page::Files, "Warn when renaming changes the extension", "A second Enter applies the new name", |a| a.warn_extension_change, |a, on| a.warn_extension_change = on),
    choice("files.conflicts", Page::Files, "When a name is already taken", "What copies and moves do when the destination already has an item with the same name", CONFLICT_CHOICES, |a| pos(&CONFLICTS, &a.conflict_default), |a, i| a.conflict_default = CONFLICTS[i]),
    choice("files.size_units", Page::Files, "Size units", "Sizes in the list, the status bar, the preview pane and the operations panel", UNITS, |a| usize::from(a.style.decimal), |a, i| a.set_style(|s| s.decimal = i == 1)),
    toggle("files.auto_sizes", Page::Files, "Calculate folder sizes automatically", "Walks every subfolder after a folder opens; this reads the disk and can take seconds on big trees", |a| a.prefs.auto_sizes, |a, on| a.set_pref(|p| &mut p.auto_sizes, on)),
    choice("files.archive_format", Page::Files, "Default archive format", "The format «Add to archive» starts with", ARCHIVE_CHOICES, |a| pos(&ARCHIVES, &a.archive.prefs.compression.format), |a, i| {
        let c = &mut a.archive.prefs.compression;
        (c.format, c.level) = (ARCHIVES[i], ARCHIVES[i].default_level());
    }),
    toggle("layout.same_for_all", Page::Layout, "Use the same view for all folders", "When off, every folder remembers its own layout, sorting, grouping and columns", |a| a.prefs.same_for_all, |a, on| a.set_pref(|p| &mut p.same_for_all, on)),
    choice("layout.default_layout", Page::Layout, "Default layout", "For folders without their own view; Adaptive is not available while all folders share one view", LAYOUT_CHOICES, |a| pos(&LAYOUTS, &a.prefs.defaults.layout), |a, i| a.prefs.defaults.layout = LAYOUTS[i]),
    choice("layout.default_sort", Page::Layout, "Sort by", "For folders without their own view", SORT_CHOICES, |a| pos(&SORTS, &a.prefs.defaults.sort.field), |a, i| a.prefs.defaults.sort.field = SORTS[i]),
    toggle("layout.default_descending", Page::Layout, "Sort in descending order", "For folders without their own view", |a| a.prefs.defaults.sort.descending, |a, on| a.prefs.defaults.sort.descending = on),
    choice("layout.default_folders", Page::Layout, "Folders and files", "Where folders go when sorting, for folders without their own view", FOLDER_CHOICES, |a| pos(&FOLDERS, &a.prefs.defaults.sort.folders), |a, i| a.prefs.defaults.sort.folders = FOLDERS[i]),
    choice("layout.default_group", Page::Layout, "Group by", "For folders without their own view", GROUP_CHOICES, |a| pos(&GROUPS, &a.prefs.defaults.group.field), |a, i| a.prefs.defaults.group.field = GROUPS[i]),
    toggle("layout.default_group_descending", Page::Layout, "Show groups in descending order", "For folders without their own view", |a| a.prefs.defaults.group.descending, |a, on| a.prefs.defaults.group.descending = on),
    choice("layout.default_date_unit", Page::Layout, "Group dates by", "For folders without their own view grouped by a date", DATE_UNIT_CHOICES, |a| pos(&UNITS_OF_DATE, &a.prefs.defaults.group.unit), |a, i| a.prefs.defaults.group.unit = UNITS_OF_DATE[i]),
    toggle("layout.column_modified", Page::Layout, "Show the Date modified column", "In the Details layout of folders without their own view", |a| a.prefs.defaults.columns.shown(Column::Modified), |a, on| set_column(a, Column::Modified, on)),
    toggle("layout.column_created", Page::Layout, "Show the Date created column", "In the Details layout of folders without their own view", |a| a.prefs.defaults.columns.shown(Column::Created), |a, on| set_column(a, Column::Created, on)),
    toggle("layout.column_type", Page::Layout, "Show the Type column", "In the Details layout of folders without their own view", |a| a.prefs.defaults.columns.shown(Column::Type), |a, on| set_column(a, Column::Type, on)),
    toggle("layout.column_size", Page::Layout, "Show the Size column", "In the Details layout of folders without their own view", |a| a.prefs.defaults.columns.shown(Column::Size), |a, on| set_column(a, Column::Size, on)),
    toggle("layout.column_tags", Page::Layout, "Show the Tags column", "In the Details layout of folders without their own view", |a| a.prefs.defaults.columns.shown(Column::Tag), |a, on| set_column(a, Column::Tag, on)),
    toggle("layout.auto_fit", Page::Layout, "Always size columns to fit", "Column widths follow the content after every folder load", |a| a.prefs.auto_fit, |a, on| {
        if a.prefs.auto_fit != on {
            a.toggle_auto_fit();
        }
    }),
    toggle("advanced.flatten_in_menu", Page::Advanced, "Show «Flatten folder» in the context menu", "Moves the files of all subfolders into the selected folder and deletes the emptied subfolders; Ctrl+Z undoes it", |a| a.show_flatten, |a, on| a.show_flatten = on),
    Setting {
        id: "advanced.thumb_cache_mb",
        page: Page::Advanced,
        title: "Thumbnail cache size",
        help: "Thumbnails kept on disk in %LOCALAPPDATA%\\Flit\\thumbs; the oldest go first",
        kind: Kind::Number { min: super::THUMB_CACHE_MB.0 as i64, max: super::THUMB_CACHE_MB.2 as i64, step: 100, unit: "MB", get: |a| a.prefs.thumb_cache_mb as i64, set: |a, v| a.prefs.thumb_cache_mb = v as u32 },
    },
    Setting { id: "keys", page: Page::Actions, title: "Keyboard shortcuts", help: "Shortcuts that differ from the defaults, by command id", kind: Kind::Keys(|a| a.keymap.clone(), |a, k| a.set_keymap(k)) },
];

impl Setting {
    pub fn get(&self, a: &App) -> Value {
        match self.kind {
            Kind::Toggle(get, _) => Value::Bool(get(a)),
            Kind::Choice(_, get, _) => Value::Choice(get(a)),
            Kind::Number { get, .. } => Value::Number(get(a)),
            Kind::Paths(get, _) => Value::Paths(get(a)),
            Kind::Text(get, _) => Value::Text(get(a)),
            Kind::Keys(get, _) => Value::Keys(get(a)),
        }
    }

    pub fn set(&self, a: &mut App, v: Value) -> bool {
        match (self.kind, v) {
            (Kind::Toggle(_, set), Value::Bool(b)) => set(a, b),
            (Kind::Choice(c, _, set), Value::Choice(i)) if i < c.len() => set(a, i),
            (Kind::Number { min, max, set, .. }, Value::Number(n)) if (min..=max).contains(&n) => set(a, n),
            (Kind::Paths(_, set), Value::Paths(p)) if p.len() <= MAX_PATHS => set(a, p),
            (Kind::Text(_, set), Value::Text(t)) if t.len() <= MAX_TEXT => set(a, t),
            (Kind::Keys(_, set), Value::Keys(k)) => set(a, k),
            _ => return false,
        }
        true
    }

    pub fn to_json(&self, v: &Value) -> Json {
        match (self.kind, v) {
            (_, Value::Bool(b)) => Json::Bool(*b),
            (Kind::Choice(c, ..), Value::Choice(i)) => Json::String(c.get(*i).map_or("", |c| c.0).to_owned()),
            (_, Value::Number(n)) => Json::from(*n),
            (_, Value::Paths(p)) => Json::Array(p.iter().map(|l| Json::String(path_text(l))).collect()),
            (_, Value::Text(t)) => Json::String(t.clone()),
            (_, Value::Choice(i)) => Json::from(*i),
            (_, Value::Keys(k)) => k.to_json(),
        }
    }

    pub fn from_json(&self, j: &Json) -> Option<Value> {
        match self.kind {
            Kind::Toggle(..) => j.as_bool().map(Value::Bool),
            Kind::Choice(c, ..) => c.iter().position(|(k, _)| Some(*k) == j.as_str()).map(Value::Choice),
            Kind::Number { min, max, .. } => j.as_i64().filter(|n| (min..=max).contains(n)).map(Value::Number),
            Kind::Paths(..) => {
                let items = j.as_array().filter(|a| a.len() <= MAX_PATHS)?;
                items.iter().map(|p| p.as_str().and_then(parse_path)).collect::<Option<Vec<_>>>().map(Value::Paths)
            }
            Kind::Text(..) => j.as_str().filter(|t| t.len() <= MAX_TEXT).map(|t| Value::Text(t.to_owned())),
            Kind::Keys(..) => keymap::parse(j).map(|(k, _)| Value::Keys(k)),
        }
    }

    pub fn words(&self) -> String {
        let mut s = format!("{} {} {}", self.title, self.help, self.page.title());
        if let Kind::Choice(c, ..) = self.kind {
            for (_, label) in c {
                s.push(' ');
                s.push_str(label);
            }
        }
        s.to_lowercase()
    }
}

pub fn path_text(l: &Location) -> String {
    l.display().into_owned()
}

pub fn parse_path(s: &str) -> Option<Location> {
    let s = s.trim();
    match s {
        "" => None,
        _ if s.eq_ignore_ascii_case(HOME_LABEL) => Some(Location::home_page()),
        _ => Location::parse(s),
    }
}

pub fn index(id: &str) -> Option<usize> {
    SETTINGS.iter().position(|s| s.id == id)
}

pub fn defaults() -> &'static [Value] {
    static DEFAULTS: OnceLock<Vec<Value>> = OnceLock::new();
    DEFAULTS.get_or_init(|| App::new(Persisted::default(), Vec::new(), 0, false, 1.0, (800, 600), Instant::now()).setting_values())
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Doc {
    pub values: Vec<(usize, Value)>,
    pub bad: Vec<usize>,
    pub unknown: Map<String, Json>,
    pub tags: Vec<Tag>,
    pub problems: Vec<String>,
    pub broken: bool,
}

impl Doc {
    pub fn value(&self, id: &str) -> Option<&Value> {
        let i = index(id)?;
        self.values.iter().find(|(k, _)| *k == i).map(|(_, v)| v)
    }

    pub fn startup(&self) -> Option<Startup> {
        let pages = match self.value("general.startup_pages") {
            Some(Value::Paths(p)) => p.clone(),
            _ => Vec::new(),
        };
        match self.value("general.startup")? {
            Value::Choice(0) => Some(Startup::Continue),
            Value::Choice(1) => Some(Startup::NewTab),
            Value::Choice(_) => Some(Startup::Folders(pages)),
            _ => None,
        }
    }
}

fn parse_color(s: &str) -> Option<u32> {
    let hex = s.strip_prefix('#')?;
    (hex.len() == 6).then(|| u32::from_str_radix(hex, 16).ok()).flatten()
}

pub fn parse(text: &str) -> Doc {
    let mut doc = Doc::default();
    let map = match serde_json::from_str::<Map<String, Json>>(text.trim_start_matches('\u{FEFF}')) {
        Ok(m) => m,
        Err(e) => {
            doc.broken = true;
            doc.problems.push(format!("The settings file is not valid JSON ({e}), so the settings of the last session stay"));
            return doc;
        }
    };
    for (k, v) in map {
        if k == "tags" {
            let list = v.as_array().map(|a| a.iter().filter_map(|t| Some(Tag { name: t.get("name")?.as_str()?.to_owned(), color: parse_color(t.get("color")?.as_str()?)? })).collect::<Vec<_>>());
            match list {
                Some(tags) => doc.tags = tags,
                None => doc.problems.push("The tags in the settings file are not a list, so they were ignored".into()),
            }
            continue;
        }
        match index(&k) {
            Some(i) => match SETTINGS[i].from_json(&v) {
                Some(value) => {
                    if let Value::Keys(_) = value {
                        doc.problems.extend(keymap::parse(&v).map(|(_, p)| p).unwrap_or_default());
                    }
                    doc.values.push((i, value));
                }
                None => {
                    doc.bad.push(i);
                    doc.problems.push(format!("The value of {k} in the settings file is not valid, so the default is used"));
                }
            },
            None => {
                doc.unknown.insert(k, v);
            }
        }
    }
    doc
}

pub fn render(values: &[Value], unknown: &Map<String, Json>, tags: Option<&[Tag]>) -> String {
    let mut out = String::from("{\n");
    let mut put = |k: &str, v: &Json| {
        if out.len() > 2 {
            out.push_str(",\n");
        }
        out.push_str("  ");
        out.push_str(&Json::String(k.to_owned()).to_string());
        out.push_str(": ");
        match v {
            Json::Object(m) if !m.is_empty() => {
                out.push('{');
                for (n, (k, x)) in m.iter().enumerate() {
                    out.push_str(if n == 0 { "\n    " } else { ",\n    " });
                    out.push_str(&Json::String(k.clone()).to_string());
                    out.push_str(": ");
                    out.push_str(&x.to_string());
                }
                out.push_str("\n  }");
            }
            _ => out.push_str(&v.to_string()),
        }
    };
    for (s, v) in SETTINGS.iter().zip(values) {
        put(s.id, &s.to_json(v));
    }
    for (k, v) in unknown.iter().filter(|(k, _)| index(k).is_none() && *k != "tags") {
        put(k, v);
    }
    if let Some(tags) = tags {
        let list = tags.iter().map(|t| serde_json::json!({ "name": t.name, "color": format!("#{:06X}", t.color & 0xFF_FFFF) })).collect();
        put("tags", &Json::Array(list));
    }
    out.push_str("\n}\n");
    out
}

impl App {
    pub fn setting_values(&self) -> Vec<Value> {
        SETTINGS.iter().map(|s| s.get(self)).collect()
    }

    pub fn apply_settings(&mut self, values: &[(usize, Value)]) -> usize {
        let mut applied = 0;
        for (i, v) in values {
            let Some(s) = SETTINGS.get(*i) else { continue };
            if s.get(self) != *v && s.set(self, v.clone()) {
                applied += 1;
            }
        }
        self.layout();
        self.dirty = true;
        applied
    }

    pub fn apply_doc(&mut self, doc: &Doc) {
        let mut values = doc.values.clone();
        if !doc.bad.is_empty() {
            values.extend(doc.bad.iter().map(|&i| (i, defaults()[i].clone())));
        }
        self.apply_settings(&values);
        self.report_problems(&doc.problems);
    }

    fn report_problems(&mut self, problems: &[String]) {
        match problems {
            [] => {}
            [one] => self.status(one.clone()),
            [first, rest @ ..] => self.status(format!("{first}; {} more problems in the settings file", rest.len())),
        }
    }

    pub fn reset_settings(&mut self) {
        let values: Vec<(usize, Value)> = defaults().iter().cloned().enumerate().collect();
        self.apply_settings(&values);
        self.status("All settings are back to their defaults");
    }

    pub fn export_text(&self) -> String {
        render(&self.setting_values(), &Map::new(), Some(self.tag_defs()))
    }

    pub fn import_settings(&mut self, text: &str) {
        let doc = parse(text);
        if doc.broken {
            return self.report_problems(&doc.problems);
        }
        let applied = self.apply_settings(&doc.values);
        let mut defs = self.tag_defs().to_vec();
        let mut tagged = 0;
        for t in &doc.tags {
            match tags::find(&defs, &t.name) {
                Some(k) if defs[k].color != t.color => {
                    self.effects.push(Effect::Tags(TagRequest::Recolor { tag: k, color: t.color }));
                    defs[k].color = t.color;
                    tagged += 1;
                }
                Some(_) => {}
                None if defs.len() < MAX_TAGS && tags::validate_name(&defs, &t.name, None).is_ok() => {
                    self.effects.push(Effect::Tags(TagRequest::Create { name: t.name.clone(), color: t.color, assign: Vec::new() }));
                    defs.push(t.clone());
                    tagged += 1;
                }
                None => {}
            }
        }
        match doc.problems.first() {
            Some(p) => self.status(format!("Imported {applied} settings and {tagged} tags; {p}")),
            None => self.status(format!("Imported {applied} settings and {tagged} tags")),
        }
    }

    fn set_startup(&mut self, i: usize) {
        self.startup = match i {
            0 => Startup::Continue,
            1 => Startup::NewTab,
            _ => Startup::Folders(self.startup_pages.clone()),
        };
    }

    fn set_startup_pages(&mut self, pages: Vec<Location>) {
        if let Startup::Folders(p) = &mut self.startup {
            p.clone_from(&pages);
        }
        self.startup_pages = pages;
    }

    fn set_style(&mut self, f: impl FnOnce(&mut crate::fmt::Style)) {
        f(&mut self.style);
        self.dirty = true;
    }

    pub fn set_date_patterns(&mut self, date: String, time: String) {
        let system = (date, time);
        if self.style.system != system {
            self.style.system = system;
            self.dirty |= self.style.dates == DateFormat::System;
        }
    }

    pub fn set_clock(&mut self, ticks: i64) {
        let today = Some(crate::fmt::local_days(ticks, self.utc_offset_min));
        if self.style.today != today {
            self.style.today = today;
            self.dirty |= self.style.dates == DateFormat::Application;
        }
    }

    fn set_show_hidden(&mut self, on: bool) {
        self.show_hidden = on;
        for k in 0..self.tabs.len() {
            if self.tabs[k].view_spec.show_hidden != on {
                self.tabs[k].view_spec.show_hidden = on;
                self.resort(k);
            }
        }
    }
}

#[cfg(test)]
#[path = "../../test/ui/settings.rs"]
mod tests;
