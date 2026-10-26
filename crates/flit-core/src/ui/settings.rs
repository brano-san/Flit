use super::home::Widget;
use super::layouts::LAYOUTS;
use super::tabbar::Startup;
use super::{App, Column, Effect, Persisted, SplitDir};
use crate::archive::Format;
use crate::fmt::DateFormat;
use crate::i18n::{self, Lang, P, S};
use crate::location::Location;
use crate::path::{is_label, PathKind, COMPUTER_LABEL, HOME_LABEL, RECYCLE_BIN_LABEL};
use crate::open::OpenTarget;
use crate::provider::Resolve;
use crate::sort::{DateUnit, Folders, GroupField, SortField};
use crate::tags::{self, Tag, TagRequest, MAX_TAGS};
use super::keymap::{self, Keymap};
use crate::theme::{Area, Mode};
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

    pub fn label(self) -> S {
        match self {
            Page::General => S::SettingsPageGeneral,
            Page::Appearance => S::SettingsPageAppearance,
            Page::Files => S::SettingsPageFiles,
            Page::Layout => S::SettingsPageLayout,
            Page::Tags => S::SettingsPageTags,
            Page::Actions => S::SettingsPageActions,
            Page::Advanced => S::SettingsPageAdvanced,
            Page::About => S::SettingsPageAbout,
        }
    }

    pub fn title(self) -> &'static str {
        self.label().t()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ConfirmDelete {
    Always,
    #[default]
    Permanent,
    Never,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ClickOpen {
    #[default]
    Never,
    Touch,
    Mouse,
    Always,
}

impl ClickOpen {
    pub fn opens(self, touch: bool) -> bool {
        match self {
            ClickOpen::Never => false,
            ClickOpen::Touch => touch,
            ClickOpen::Mouse => !touch,
            ClickOpen::Always => true,
        }
    }
}

pub type Choices = &'static [(&'static str, S)];

#[derive(Clone, Copy)]
pub enum Kind {
    Toggle(fn(&App) -> bool, fn(&mut App, bool)),
    Choice(Choices, fn(&App) -> usize, fn(&mut App, usize)),
    Number { min: i64, max: i64, step: i64, unit: S, get: fn(&App) -> i64, set: fn(&mut App, i64) },
    Paths(fn(&App) -> Vec<Location>, fn(&mut App, Vec<Location>)),
    Text(fn(&App) -> String, fn(&mut App, String)),
    Pick(Picker, fn(&App) -> String, fn(&mut App, String)),
    Keys(fn(&App) -> Arc<Keymap>, fn(&mut App, Arc<Keymap>)),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Picker {
    Color,
    Font,
}

pub struct Setting {
    pub id: &'static str,
    pub page: Page,
    pub title: S,
    pub help: S,
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

const fn toggle(id: &'static str, page: Page, title: S, help: S, get: fn(&App) -> bool, set: fn(&mut App, bool)) -> Setting {
    Setting { id, page, title, help, kind: Kind::Toggle(get, set) }
}

const fn choice(id: &'static str, page: Page, title: S, help: S, choices: Choices, get: fn(&App) -> usize, set: fn(&mut App, usize)) -> Setting {
    Setting { id, page, title, help, kind: Kind::Choice(choices, get, set) }
}

const fn pick(id: &'static str, picker: Picker, title: S, help: S, get: fn(&App) -> String, set: fn(&mut App, String)) -> Setting {
    Setting { id, page: Page::Appearance, title, help, kind: Kind::Pick(picker, get, set) }
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

const STARTUP: Choices = &[("continue", S::SettingsChoiceStartupContinue), ("new_tab", S::SettingsChoiceStartupNewTab), ("pages", S::SettingsChoiceStartupPages)];
const DATES: [DateFormat; 3] = [DateFormat::Universal, DateFormat::System, DateFormat::Application];
const DATE_CHOICES: Choices = &[("universal", S::SettingsChoiceDateUniversal), ("system", S::SettingsChoiceDateSystem), ("application", S::SettingsChoiceDateApplication)];
const UNITS: Choices = &[("binary", S::SettingsChoiceUnitsBinary), ("decimal", S::SettingsChoiceUnitsDecimal)];
const CONFIRMS: [ConfirmDelete; 3] = [ConfirmDelete::Always, ConfirmDelete::Permanent, ConfirmDelete::Never];
const CONFIRM_CHOICES: Choices = &[("always", S::SettingsChoiceConfirmAlways), ("permanent", S::SettingsChoiceConfirmPermanent), ("never", S::SettingsChoiceConfirmNever)];
const CONFLICTS: [Option<Resolve>; 4] = [None, Some(Resolve::KeepBoth), Some(Resolve::Replace), Some(Resolve::Skip)];
const CONFLICT_CHOICES: Choices = &[("ask", S::SettingsChoiceConflictAsk), ("keep_both", S::SettingsChoiceConflictKeepBoth), ("replace", S::SettingsChoiceConflictReplace), ("skip", S::SettingsChoiceConflictSkip)];
const LAYOUT_CHOICES: Choices = &[("details", S::SettingsChoiceLayoutDetails), ("list", S::SettingsChoiceLayoutList), ("cards", S::SettingsChoiceLayoutCards), ("columns", S::SettingsChoiceLayoutColumns), ("grid", S::SettingsChoiceLayoutGrid), ("adaptive", S::SettingsChoiceLayoutAdaptive)];
const SORTS: [SortField; 6] = [SortField::Name, SortField::Modified, SortField::Created, SortField::Size, SortField::Type, SortField::Tag];
const SORT_CHOICES: Choices = &[("name", S::SettingsChoiceSortName), ("modified", S::SettingsChoiceSortModified), ("created", S::SettingsChoiceSortCreated), ("size", S::SettingsChoiceSortSize), ("type", S::SettingsChoiceSortType), ("tags", S::SettingsChoiceSortTags)];
const FOLDERS: [Folders; 3] = [Folders::First, Folders::Last, Folders::Mixed];
const FOLDER_CHOICES: Choices = &[("first", S::SettingsChoiceFolderFirst), ("last", S::SettingsChoiceFolderLast), ("mixed", S::SettingsChoiceFolderMixed)];
const GROUPS: [GroupField; 7] = [GroupField::None, GroupField::Name, GroupField::Modified, GroupField::Created, GroupField::Size, GroupField::Type, GroupField::Tag];
const GROUP_CHOICES: Choices = &[("none", S::SettingsChoiceGroupNone), ("name", S::SettingsChoiceGroupName), ("modified", S::SettingsChoiceGroupModified), ("created", S::SettingsChoiceGroupCreated), ("size", S::SettingsChoiceGroupSize), ("type", S::SettingsChoiceGroupType), ("tags", S::SettingsChoiceGroupTags)];
const UNITS_OF_DATE: [DateUnit; 3] = [DateUnit::Year, DateUnit::Month, DateUnit::Day];
const DATE_UNIT_CHOICES: Choices = &[("year", S::SettingsChoiceDateUnitYear), ("month", S::SettingsChoiceDateUnitMonth), ("day", S::SettingsChoiceDateUnitDay)];
const SPLITS: [SplitDir; 2] = [SplitDir::Vertical, SplitDir::Horizontal];
const SPLIT_CHOICES: Choices = &[("vertical", S::SettingsChoiceSplitVertical), ("horizontal", S::SettingsChoiceSplitHorizontal)];
const ARCHIVES: [Format; 4] = Format::WRITABLE;
const ARCHIVE_CHOICES: Choices = &[("zip", S::SettingsChoiceArchiveZip), ("7z", S::SettingsChoiceArchive7z), ("tar", S::SettingsChoiceArchiveTar), ("gz", S::SettingsChoiceArchiveGz)];
const THEMES: [Mode; 3] = [Mode::System, Mode::Light, Mode::Dark];
const THEME_CHOICES: Choices = &[("system", S::SettingsChoiceThemeSystem), ("light", S::SettingsChoiceThemeLight), ("dark", S::SettingsChoiceThemeDark)];
const OPS_CHOICES: Choices = &[("during", S::SettingsChoiceOpsDuring), ("always", S::SettingsChoiceOpsAlways)];
const CLICKS: [ClickOpen; 4] = [ClickOpen::Never, ClickOpen::Touch, ClickOpen::Mouse, ClickOpen::Always];
const CLICK_CHOICES: Choices = &[("never", S::SettingsChoiceClickNever), ("touch", S::SettingsChoiceClickTouch), ("mouse", S::SettingsChoiceClickMouse), ("always", S::SettingsChoiceClickAlways)];
const COUNT_CHOICES: Choices = &[("total", S::SettingsChoiceCountTotal), ("separate", S::SettingsChoiceCountSeparate)];
const AREA_HELP: S = S::SettingsHelpAreaColor;
pub const LOG_LEVEL: &str = "advanced.log_level";
pub const LOG_DEFAULT: usize = 3;
const LOG_CHOICES: Choices = &[("off", S::SettingsChoiceLogOff), ("error", S::SettingsChoiceLogError), ("warn", S::SettingsChoiceLogWarn), ("info", S::SettingsChoiceLogInfo), ("debug", S::SettingsChoiceLogDebug)];

pub const LANGUAGE: &str = "general.language";
const LANGUAGE_CHOICES: Choices = &[("system", S::SettingsChoiceLanguageSystem), ("en", S::SettingsChoiceLanguageEn), ("ru", S::SettingsChoiceLanguageRu)];

pub fn language_of(choice: usize) -> Lang {
    match choice {
        1 => Lang::En,
        2 => Lang::Ru,
        _ => i18n::system_lang(),
    }
}

pub static SETTINGS: &[Setting] = &[
    choice(LANGUAGE, Page::General, S::SettingsTitleGeneralLanguage, S::SettingsHelpGeneralLanguage, LANGUAGE_CHOICES, |a| a.language, |a, i| a.set_language(i)),
    choice("general.startup", Page::General, S::SettingsTitleGeneralStartup, S::SettingsHelpGeneralStartup, STARTUP, |a| match a.startup {
        Startup::Continue => 0,
        Startup::NewTab => 1,
        Startup::Folders(_) => 2,
    }, |a, i| a.set_startup(i)),
    Setting { id: "general.startup_pages", page: Page::General, title: S::SettingsTitleGeneralStartupPages, help: S::SettingsHelpGeneralStartupPages, kind: Kind::Paths(|a| a.startup_pages.clone(), |a, v| a.set_startup_pages(v)) },
    toggle("general.open_in_running", Page::General, S::SettingsTitleGeneralOpenInRunning, S::SettingsHelpGeneralOpenInRunning, |a| a.open_target != OpenTarget::NewWindow, |a, on| a.open_target = if on { OpenTarget::NewTab } else { OpenTarget::NewWindow }),
    toggle("general.switch_to_new_tab", Page::General, S::SettingsTitleGeneralSwitchToNewTab, S::SettingsHelpGeneralSwitchToNewTab, |a| a.switch_to_new_tab, |a, on| a.switch_to_new_tab = on),
    toggle("general.dual_pane", Page::General, S::SettingsTitleGeneralDualPane, S::SettingsHelpGeneralDualPane, |a| a.split_prefs.dual, |a, on| a.split_prefs.dual = on),
    choice("general.split_direction", Page::General, S::SettingsTitleGeneralSplitDirection, S::SettingsHelpGeneralSplitDirection, SPLIT_CHOICES, |a| pos(&SPLITS, &a.split_prefs.dir), |a, i| a.split_prefs.dir = SPLITS[i]),
    toggle("general.smooth_scroll", Page::General, S::SettingsTitleGeneralSmoothScroll, S::SettingsHelpGeneralSmoothScroll, |a| a.smooth_scroll, |a, on| a.smooth_scroll = on),
    toggle("general.tab_wheel_reversed", Page::General, S::SettingsTitleGeneralTabWheelReversed, S::SettingsHelpGeneralTabWheelReversed, |a| a.tab_wheel_reversed, |a, on| a.tab_wheel_reversed = on),
    choice("general.date_format", Page::General, S::SettingsTitleGeneralDateFormat, S::SettingsHelpGeneralDateFormat, DATE_CHOICES, |a| pos(&DATES, &a.style.dates), |a, i| a.set_style(|s| s.dates = DATES[i])),
    toggle("general.home_quick_access", Page::General, S::SettingsTitleGeneralHomeQuickAccess, S::SettingsHelpGeneralHomeQuickAccess, |a| a.widget_shown(Widget::QuickAccess), |a, on| set_widget(a, Widget::QuickAccess, on)),
    toggle("general.home_drives", Page::General, S::SettingsTitleGeneralHomeDrives, S::SettingsHelpGeneralHomeDrives, |a| a.widget_shown(Widget::Drives), |a, on| set_widget(a, Widget::Drives, on)),
    toggle("general.home_tags", Page::General, S::SettingsTitleGeneralHomeTags, S::SettingsHelpGeneralHomeTags, |a| a.widget_shown(Widget::Tags), |a, on| set_widget(a, Widget::Tags, on)),
    toggle("general.home_recent", Page::General, S::SettingsTitleGeneralHomeRecent, S::SettingsHelpGeneralHomeRecent, |a| a.widget_shown(Widget::Recent), |a, on| set_widget(a, Widget::Recent, on)),
    toggle("general.shell_submenu", Page::General, S::SettingsTitleGeneralShellSubmenu, S::SettingsHelpGeneralShellSubmenu, |a| !a.shell_items_inline, |a, on| a.shell_items_inline = !on),
    toggle("general.menu_open_tab", Page::General, S::SettingsTitleGeneralMenuOpenTab, S::SettingsHelpGeneralMenuOpenTab, |a| shown_in_menu(a, "item.open_tab"), |a, on| show_in_menu(a, "item.open_tab", on)),
    toggle("general.menu_open_window", Page::General, S::SettingsTitleGeneralMenuOpenWindow, S::SettingsHelpGeneralMenuOpenWindow, |a| shown_in_menu(a, "item.open_window"), |a, on| show_in_menu(a, "item.open_window", on)),
    toggle("general.menu_copy_path", Page::General, S::SettingsTitleGeneralMenuCopyPath, S::SettingsHelpGeneralMenuCopyPath, |a| shown_in_menu(a, "item.copy_path"), |a, on| show_in_menu(a, "item.copy_path", on)),
    toggle("general.menu_shortcut", Page::General, S::SettingsTitleGeneralMenuShortcut, S::SettingsHelpGeneralMenuShortcut, |a| shown_in_menu(a, "file.shortcut"), |a, on| show_in_menu(a, "file.shortcut", on)),
    toggle("general.menu_new_folder_with_selection", Page::General, S::SettingsTitleGeneralMenuNewFolderWithSelection, S::SettingsHelpGeneralMenuNewFolderWithSelection, |a| shown_in_menu(a, "file.new_folder_with_selection"), |a, on| show_in_menu(a, "file.new_folder_with_selection", on)),
    toggle("general.menu_stream", Page::General, S::SettingsTitleGeneralMenuStream, S::SettingsHelpGeneralMenuStream, |a| shown_in_menu(a, "file.new_stream"), |a, on| show_in_menu(a, "file.new_stream", on)),
    toggle("general.menu_panes", Page::General, S::SettingsTitleGeneralMenuPanes, S::SettingsHelpGeneralMenuPanes, |a| shown_in_menu(a, "pane.menu"), |a, on| show_in_menu(a, "pane.menu", on)),
    toggle("general.menu_pin", Page::General, S::SettingsTitleGeneralMenuPin, S::SettingsHelpGeneralMenuPin, |a| shown_in_menu(a, "item.pin"), |a, on| show_in_menu(a, "item.pin", on)),
    toggle("general.menu_terminal", Page::General, S::SettingsTitleGeneralMenuTerminal, S::SettingsHelpGeneralMenuTerminal, |a| shown_in_menu(a, "app.terminal"), |a, on| show_in_menu(a, "app.terminal", on)),
    toggle("general.menu_tags", Page::General, S::SettingsTitleGeneralMenuTags, S::SettingsHelpGeneralMenuTags, |a| shown_in_menu(a, "tags.edit"), |a, on| show_in_menu(a, "tags.edit", on)),
    toggle("general.menu_compress", Page::General, S::SettingsTitleGeneralMenuCompress, S::SettingsHelpGeneralMenuCompress, |a| shown_in_menu(a, "archive.menu"), |a, on| show_in_menu(a, "archive.menu", on)),
    choice("appearance.theme", Page::Appearance, S::SettingsTitleAppearanceTheme, S::SettingsHelpAppearanceTheme, THEME_CHOICES, |a| pos(&THEMES, &a.look.mode), |a, i| a.set_theme_mode(THEMES[i])),
    pick("appearance.background", Picker::Color, S::SettingsTitleAppearanceBackground, S::SettingsHelpAppearanceBackground, |a| a.look.background.key(), |a, v| a.set_background(&v)),
    Setting { id: "appearance.background_image", page: Page::Appearance, title: S::SettingsTitleAppearanceBackgroundImage, help: S::SettingsHelpAppearanceBackgroundImage, kind: Kind::Text(|a| a.look.backdrop_path.clone(), |a, v| a.set_backdrop(v)) },
    pick("appearance.font", Picker::Font, S::SettingsTitleAppearanceFont, S::SettingsHelpAppearanceFont, |a| a.look.font.clone(), |a, v| a.set_ui_font(&v)),
    toggle("appearance.toolbar", Page::Appearance, S::SettingsTitleAppearanceToolbar, S::SettingsHelpAppearanceToolbar, |a| a.toolbar.state.visible, |a, on| {
        if a.toolbar.state.visible != on {
            a.toggle_toolbar();
        }
    }),
    toggle("appearance.tab_actions", Page::Appearance, S::SettingsTitleAppearanceTabActions, S::SettingsHelpAppearanceTabActions, |a| a.tab_actions_button, |a, on| a.tab_actions_button = on),
    choice("appearance.ops_button", Page::Appearance, S::SettingsTitleAppearanceOpsButton, S::SettingsHelpAppearanceOpsButton, OPS_CHOICES, |a| usize::from(a.ops_button_always), |a, i| a.ops_button_always = i == 1),
    toggle("appearance.status_bar", Page::Appearance, S::SettingsTitleAppearanceStatusBar, S::SettingsHelpAppearanceStatusBar, |a| a.prefs.status_bar, |a, on| a.set_pref(|p| &mut p.status_bar, on)),
    choice("appearance.item_count", Page::Appearance, S::SettingsTitleAppearanceItemCount, S::SettingsHelpAppearanceItemCount, COUNT_CHOICES, |a| usize::from(a.prefs.split_count), |a, i| a.set_pref(|p| &mut p.split_count, i == 1)),
    pick("appearance.color_address", Picker::Color, S::SettingsTitleAppearanceColorAddress, AREA_HELP, |a| a.look.areas[Area::Address as usize].key(), |a, v| a.set_area_color(Area::Address, &v)),
    pick("appearance.color_toolbar", Picker::Color, S::SettingsTitleAppearanceColorToolbar, AREA_HELP, |a| a.look.areas[Area::Toolbar as usize].key(), |a, v| a.set_area_color(Area::Toolbar, &v)),
    pick("appearance.color_sidebar", Picker::Color, S::SettingsTitleAppearanceColorSidebar, AREA_HELP, |a| a.look.areas[Area::Sidebar as usize].key(), |a, v| a.set_area_color(Area::Sidebar, &v)),
    pick("appearance.color_files", Picker::Color, S::SettingsTitleAppearanceColorFiles, AREA_HELP, |a| a.look.areas[Area::Files as usize].key(), |a, v| a.set_area_color(Area::Files, &v)),
    pick("appearance.color_pane", Picker::Color, S::SettingsTitleAppearanceColorPane, AREA_HELP, |a| a.look.areas[Area::Pane as usize].key(), |a, v| a.set_area_color(Area::Pane, &v)),
    toggle("files.show_hidden", Page::Files, S::SettingsTitleFilesShowHidden, S::SettingsHelpFilesShowHidden, |a| a.show_hidden, |a, on| a.set_show_hidden(on)),
    toggle("files.dotfiles", Page::Files, S::SettingsTitleFilesDotfiles, S::SettingsHelpFilesDotfiles, |a| a.prefs.show_dotfiles, |a, on| a.set_pref(|p| &mut p.show_dotfiles, on)),
    toggle("files.system", Page::Files, S::SettingsTitleFilesSystem, S::SettingsHelpFilesSystem, |a| a.prefs.show_system, |a, on| a.set_pref(|p| &mut p.show_system, on)),
    toggle("files.streams", Page::Files, S::SettingsTitleFilesStreams, S::SettingsHelpFilesStreams, |a| a.prefs.show_streams, |a, on| {
        if a.prefs.show_streams != on {
            a.toggle_streams(Instant::now());
        }
    }),
    toggle("files.extensions", Page::Files, S::SettingsTitleFilesExtensions, S::SettingsHelpFilesExtensions, |a| a.prefs.show_extensions, |a, on| a.set_pref(|p| &mut p.show_extensions, on)),
    toggle("files.thumbnails", Page::Files, S::SettingsTitleFilesThumbnails, S::SettingsHelpFilesThumbnails, |a| a.prefs.thumbnails, |a, on| a.set_pref(|p| &mut p.thumbnails, on)),
    toggle("files.checkboxes", Page::Files, S::SettingsTitleFilesCheckboxes, S::SettingsHelpFilesCheckboxes, |a| a.prefs.checkboxes, |a, on| a.set_pref(|p| &mut p.checkboxes, on)),
    toggle("files.hover_select", Page::Files, S::SettingsTitleFilesHoverSelect, S::SettingsHelpFilesHoverSelect, |a| a.prefs.hover_select, |a, on| a.set_pref(|p| &mut p.hover_select, on)),
    choice("files.single_click_files", Page::Files, S::SettingsTitleFilesSingleClickFiles, S::SettingsHelpFilesSingleClickFiles, CLICK_CHOICES, |a| pos(&CLICKS, &a.single_click_files), |a, i| a.single_click_files = CLICKS[i]),
    choice("files.single_click_folders", Page::Files, S::SettingsTitleFilesSingleClickFolders, S::SettingsHelpFilesSingleClickFolders, CLICK_CHOICES, |a| pos(&CLICKS, &a.single_click_folders), |a, i| a.single_click_folders = CLICKS[i]),
    choice("files.single_click_columns", Page::Files, S::SettingsTitleFilesSingleClickColumns, S::SettingsHelpFilesSingleClickColumns, CLICK_CHOICES, |a| pos(&CLICKS, &a.single_click_columns), |a, i| a.single_click_columns = CLICKS[i]),
    toggle("files.folders_in_new_tab", Page::Files, S::SettingsTitleFilesFoldersInNewTab, S::SettingsHelpFilesFoldersInNewTab, |a| a.folders_in_new_tab, |a, on| a.folders_in_new_tab = on),
    toggle("files.double_click_up", Page::Files, S::SettingsTitleFilesDoubleClickUp, S::SettingsHelpFilesDoubleClickUp, |a| a.double_click_up, |a, on| a.double_click_up = on),
    toggle("files.select_previous", Page::Files, S::SettingsTitleFilesSelectPrevious, S::SettingsHelpFilesSelectPrevious, |a| a.select_previous, |a, on| a.select_previous = on),
    choice("files.confirm_delete", Page::Files, S::SettingsTitleFilesConfirmDelete, S::SettingsHelpFilesConfirmDelete, CONFIRM_CHOICES, |a| pos(&CONFIRMS, &a.confirm_delete), |a, i| a.confirm_delete = CONFIRMS[i]),
    toggle("files.warn_extension", Page::Files, S::SettingsTitleFilesWarnExtension, S::SettingsHelpFilesWarnExtension, |a| a.warn_extension_change, |a, on| a.warn_extension_change = on),
    choice("files.conflicts", Page::Files, S::SettingsTitleFilesConflicts, S::SettingsHelpFilesConflicts, CONFLICT_CHOICES, |a| pos(&CONFLICTS, &a.conflict_default), |a, i| a.conflict_default = CONFLICTS[i]),
    choice("files.size_units", Page::Files, S::SettingsTitleFilesSizeUnits, S::SettingsHelpFilesSizeUnits, UNITS, |a| usize::from(a.style.decimal), |a, i| a.set_style(|s| s.decimal = i == 1)),
    toggle("files.auto_sizes", Page::Files, S::SettingsTitleFilesAutoSizes, S::SettingsHelpFilesAutoSizes, |a| a.prefs.auto_sizes, |a, on| a.set_pref(|p| &mut p.auto_sizes, on)),
    choice("files.archive_format", Page::Files, S::SettingsTitleFilesArchiveFormat, S::SettingsHelpFilesArchiveFormat, ARCHIVE_CHOICES, |a| pos(&ARCHIVES, &a.archive.prefs.compression.format), |a, i| {
        let c = &mut a.archive.prefs.compression;
        (c.format, c.level) = (ARCHIVES[i], ARCHIVES[i].default_level());
    }),
    toggle("layout.same_for_all", Page::Layout, S::SettingsTitleLayoutSameForAll, S::SettingsHelpLayoutSameForAll, |a| a.prefs.same_for_all, |a, on| a.set_pref(|p| &mut p.same_for_all, on)),
    choice("layout.default_layout", Page::Layout, S::SettingsTitleLayoutDefaultLayout, S::SettingsHelpLayoutDefaultLayout, LAYOUT_CHOICES, |a| pos(&LAYOUTS, &a.prefs.defaults.layout), |a, i| a.prefs.defaults.layout = LAYOUTS[i]),
    choice("layout.default_sort", Page::Layout, S::SettingsTitleLayoutDefaultSort, S::SettingsHelpLayoutDefaultSort, SORT_CHOICES, |a| pos(&SORTS, &a.prefs.defaults.sort.field), |a, i| a.prefs.defaults.sort.field = SORTS[i]),
    toggle("layout.default_descending", Page::Layout, S::SettingsTitleLayoutDefaultDescending, S::SettingsHelpLayoutDefaultDescending, |a| a.prefs.defaults.sort.descending, |a, on| a.prefs.defaults.sort.descending = on),
    choice("layout.default_folders", Page::Layout, S::SettingsTitleLayoutDefaultFolders, S::SettingsHelpLayoutDefaultFolders, FOLDER_CHOICES, |a| pos(&FOLDERS, &a.prefs.defaults.sort.folders), |a, i| a.prefs.defaults.sort.folders = FOLDERS[i]),
    choice("layout.default_group", Page::Layout, S::SettingsTitleLayoutDefaultGroup, S::SettingsHelpLayoutDefaultGroup, GROUP_CHOICES, |a| pos(&GROUPS, &a.prefs.defaults.group.field), |a, i| a.prefs.defaults.group.field = GROUPS[i]),
    toggle("layout.default_group_descending", Page::Layout, S::SettingsTitleLayoutDefaultGroupDescending, S::SettingsHelpLayoutDefaultGroupDescending, |a| a.prefs.defaults.group.descending, |a, on| a.prefs.defaults.group.descending = on),
    choice("layout.default_date_unit", Page::Layout, S::SettingsTitleLayoutDefaultDateUnit, S::SettingsHelpLayoutDefaultDateUnit, DATE_UNIT_CHOICES, |a| pos(&UNITS_OF_DATE, &a.prefs.defaults.group.unit), |a, i| a.prefs.defaults.group.unit = UNITS_OF_DATE[i]),
    toggle("layout.column_modified", Page::Layout, S::SettingsTitleLayoutColumnModified, S::SettingsHelpLayoutColumnModified, |a| a.prefs.defaults.columns.shown(Column::Modified), |a, on| set_column(a, Column::Modified, on)),
    toggle("layout.column_created", Page::Layout, S::SettingsTitleLayoutColumnCreated, S::SettingsHelpLayoutColumnCreated, |a| a.prefs.defaults.columns.shown(Column::Created), |a, on| set_column(a, Column::Created, on)),
    toggle("layout.column_type", Page::Layout, S::SettingsTitleLayoutColumnType, S::SettingsHelpLayoutColumnType, |a| a.prefs.defaults.columns.shown(Column::Type), |a, on| set_column(a, Column::Type, on)),
    toggle("layout.column_size", Page::Layout, S::SettingsTitleLayoutColumnSize, S::SettingsHelpLayoutColumnSize, |a| a.prefs.defaults.columns.shown(Column::Size), |a, on| set_column(a, Column::Size, on)),
    toggle("layout.column_tags", Page::Layout, S::SettingsTitleLayoutColumnTags, S::SettingsHelpLayoutColumnTags, |a| a.prefs.defaults.columns.shown(Column::Tag), |a, on| set_column(a, Column::Tag, on)),
    toggle("layout.auto_fit", Page::Layout, S::SettingsTitleLayoutAutoFit, S::SettingsHelpLayoutAutoFit, |a| a.prefs.auto_fit, |a, on| {
        if a.prefs.auto_fit != on {
            a.toggle_auto_fit();
        }
    }),
    toggle("advanced.flatten_in_menu", Page::Advanced, S::SettingsTitleAdvancedFlattenInMenu, S::SettingsHelpAdvancedFlattenInMenu, |a| a.show_flatten, |a, on| a.show_flatten = on),
    Setting {
        id: "advanced.thumb_cache_mb",
        page: Page::Advanced,
        title: S::SettingsTitleAdvancedThumbCacheMb,
        help: S::SettingsHelpAdvancedThumbCacheMb,
        kind: Kind::Number { min: super::THUMB_CACHE_MB.0 as i64, max: super::THUMB_CACHE_MB.2 as i64, step: 100, unit: S::SettingsUnitMb, get: |a| a.prefs.thumb_cache_mb as i64, set: |a, v| a.prefs.thumb_cache_mb = v as u32 },
    },
    choice(LOG_LEVEL, Page::Advanced, S::SettingsTitleAdvancedLogLevel, S::SettingsHelpAdvancedLogLevel, LOG_CHOICES, |a| a.log_level, |a, i| a.log_level = i),
    Setting { id: "keys", page: Page::Actions, title: S::SettingsTitleKeys, help: S::SettingsHelpKeys, kind: Kind::Keys(|a| a.keymap.clone(), |a, k| a.set_keymap(k)) },
];

impl Setting {
    pub fn get(&self, a: &App) -> Value {
        match self.kind {
            Kind::Toggle(get, _) => Value::Bool(get(a)),
            Kind::Choice(_, get, _) => Value::Choice(get(a)),
            Kind::Number { get, .. } => Value::Number(get(a)),
            Kind::Paths(get, _) => Value::Paths(get(a)),
            Kind::Text(get, _) | Kind::Pick(_, get, _) => Value::Text(get(a)),
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
            (Kind::Pick(picker, _, set), Value::Text(t)) if picker.accepts(&t) => set(a, t),
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
            Kind::Choice(c, ..) => {
                let key = j.as_bool().map_or(j.as_str(), |on| Some(if on { "always" } else { "never" }));
                c.iter().position(|(k, _)| Some(*k) == key).map(Value::Choice)
            }
            Kind::Number { min, max, .. } => j.as_i64().filter(|n| (min..=max).contains(n)).map(Value::Number),
            Kind::Paths(..) => {
                let items = j.as_array().filter(|a| a.len() <= MAX_PATHS)?;
                items.iter().map(|p| p.as_str().and_then(parse_path)).collect::<Option<Vec<_>>>().map(Value::Paths)
            }
            Kind::Text(..) => j.as_str().filter(|t| t.len() <= MAX_TEXT).map(|t| Value::Text(t.to_owned())),
            Kind::Pick(picker, ..) => j.as_str().filter(|t| picker.accepts(t)).map(|t| Value::Text(t.to_owned())),
            Kind::Keys(..) => keymap::parse(j).map(|(k, _)| Value::Keys(k)),
        }
    }

    pub fn words(&self) -> String {
        let mut s = String::new();
        for l in i18n::search_langs() {
            let labels: Vec<S> = match self.kind {
                Kind::Choice(c, ..) => c.iter().map(|c| c.1).collect(),
                Kind::Pick(Picker::Color, ..) => crate::theme::PRESETS.iter().map(|p| p.label).collect(),
                _ => Vec::new(),
            };
            for k in [self.title, self.help, self.page.label()].into_iter().chain(labels) {
                s.push_str(k.in_lang(*l));
                s.push(' ');
            }
        }
        s.to_lowercase()
    }
}

pub fn path_text(l: &Location) -> String {
    match l.path().kind() {
        PathKind::Home => HOME_LABEL.to_owned(),
        PathKind::Computer => COMPUTER_LABEL.to_owned(),
        PathKind::RecycleBin => RECYCLE_BIN_LABEL.to_owned(),
        _ => l.display().into_owned(),
    }
}

pub fn parse_path(s: &str) -> Option<Location> {
    let s = s.trim();
    match s {
        "" => None,
        _ if is_label(s.as_bytes(), S::PathHome) => Some(Location::home_page()),
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
            doc.problems.push(crate::tr_en!(S::ErrorsSettingsNotJson, error = e));
            return doc;
        }
    };
    for (k, v) in map {
        if k == "tags" {
            let list = v.as_array().map(|a| a.iter().filter_map(|t| Some(Tag { name: t.get("name")?.as_str()?.to_owned(), color: parse_color(t.get("color")?.as_str()?)? })).collect::<Vec<_>>());
            match list {
                Some(tags) => doc.tags = tags,
                None => doc.problems.push(S::ErrorsSettingsTags.en().to_owned()),
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
                    doc.problems.push(crate::tr_en!(S::ErrorsSettingsValue, key = k));
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
            [one] => self.status(i18n::localize(one).into_owned()),
            [first, rest @ ..] => self.status(crate::trn!(P::SettingsStatusMoreProblems, rest.len(), first = i18n::localize(first))),
        }
    }

    pub fn reset_settings(&mut self) {
        let values: Vec<(usize, Value)> = defaults().iter().cloned().enumerate().collect();
        self.apply_settings(&values);
        self.status(S::SettingsStatusReset.t());
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
            Some(p) => self.status(format!("{}; {}", self.imported(applied, tagged), i18n::localize(p))),
            None => self.status(self.imported(applied, tagged)),
        }
    }

    fn imported(&self, applied: usize, tagged: usize) -> String {
        crate::tr!(S::SettingsStatusImported, settings = crate::trn!(P::SettingsCountSettings, applied), tags = crate::trn!(P::SettingsCountTags, tagged))
    }

    fn set_language(&mut self, i: usize) {
        self.language = i;
        i18n::set_lang(language_of(i));
        self.relabel();
    }

    pub(super) fn relabel(&mut self) {
        let l = i18n::lang();
        if self.shown_lang == l {
            return;
        }
        let old = std::mem::replace(&mut self.shown_lang, l);
        for t in &mut self.tabs {
            if let Some(e) = &mut t.error {
                *e = i18n::relocalize(e, old).into_owned();
            }
        }
        self.relabel_form(old);
        self.nav.sidebar.rebuild();
        let title = if self.search.active { self.search_title() } else { self.window_title() };
        self.effects.push(Effect::Title(title));
        self.layout();
        self.dirty = true;
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
