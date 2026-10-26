use super::home::Widget;
use super::sidebar::KnownFolder;
use super::tags::TagPick;
use super::{App, Column, Effect, LayoutMode, Page, Route, WindowCmd};
use crate::archive::{Format, Place};
use crate::git::GitOp;
use crate::i18n::S;
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
    pub title: S,
    pub keys: &'static [Chord],
    pub run: fn(&mut App, Instant),
}

impl Action {
    const fn new(id: &'static str, title: S, keys: &'static [Chord], run: fn(&mut App, Instant)) -> Self {
        Self { id, title, keys, run }
    }
}

pub static ACTIONS: &[Action] = &[
    Action::new("palette.show", S::ActionsPaletteShow, &[ctrl_shift(Key::Char('P'))], |a, _| a.open_palette()),
    Action::new("nav.back", S::ActionsNavBack, &[alt(Key::Left), key(Key::Backspace), key(Key::BrowserBack)], |a, now| a.go_history(false, now)),
    Action::new("nav.forward", S::ActionsNavForward, &[alt(Key::Right), key(Key::BrowserForward)], |a, now| a.go_history(true, now)),
    Action::new("nav.up", S::ActionsNavUp, &[alt(Key::Up)], |a, now| a.go_up(now)),
    Action::new("nav.refresh", S::ActionsNavRefresh, &[key(Key::F(5)), ctrl(Key::Char('R'))], |a, now| a.refresh(now)),
    Action::new("nav.address", S::ActionsNavAddress, &[ctrl(Key::Char('L')), alt(Key::Char('D')), key(Key::F(4))], |a, _| a.begin_edit()),
    Action::new("nav.home", S::ActionsNavHome, &[], |a, now| a.go_known(KnownFolder::Home, now)),
    Action::new("nav.home_page", S::ActionsNavHomePage, &[], |a, now| a.go_home_page(false, now)),
    Action::new("nav.computer", S::ActionsNavComputer, &[], |a, now| a.navigate(Location::computer(), now)),
    Action::new("nav.recycle_bin", S::ActionsNavRecycleBin, &[], |a, now| a.navigate(Location::recycle_bin(), now)),
    Action::new("tab.new", S::ActionsTabNew, &[ctrl(Key::Char('T'))], |a, now| a.new_tab_here(now)),
    Action::new("tab.close", S::ActionsTabClose, &[ctrl(Key::Char('W')), ctrl(Key::F(4))], |a, _| a.close_tab(a.cur)),
    Action::new("tab.reopen", S::ActionsTabReopen, &[ctrl_shift(Key::Char('T'))], |a, now| a.reopen_tab(now)),
    Action::new("tab.close_all", S::ActionsTabCloseAll, &[ctrl_shift(Key::Char('W'))], |a, _| a.close_all_tabs()),
    Action::new("tab.close_others", S::ActionsTabCloseOthers, &[], |a, _| a.close_tabs(|i, cur| i == cur)),
    Action::new("tab.close_left", S::ActionsTabCloseLeft, &[], |a, _| a.close_tabs(|i, cur| i >= cur)),
    Action::new("tab.close_right", S::ActionsTabCloseRight, &[], |a, _| a.close_tabs(|i, cur| i <= cur)),
    Action::new("tab.move_to_window", S::ActionsTabMoveToWindow, &[], |a, _| a.move_tab_to_window()),
    Action::new("tab.next", S::ActionsTabNext, &[ctrl(Key::Tab)], |a, _| a.select_tab((a.cur + 1) % a.tabs.len())),
    Action::new("tab.previous", S::ActionsTabPrevious, &[ctrl_shift(Key::Tab)], |a, _| a.select_tab((a.cur + a.tabs.len() - 1) % a.tabs.len())),
    Action::new("pane.toggle", S::ActionsPaneToggle, &[ctrl_shift(Key::Char('S'))], |a, now| a.run_pane("pane.toggle", now)),
    Action::new("pane.split_vertical", S::ActionsPaneSplitVertical, &[alt_shift(Key::Char('V'))], |a, now| a.run_pane("pane.split_vertical", now)),
    Action::new("pane.split_horizontal", S::ActionsPaneSplitHorizontal, &[alt_shift(Key::Char('H'))], |a, now| a.run_pane("pane.split_horizontal", now)),
    Action::new("pane.arrange_vertical", S::ActionsPaneArrangeVertical, &[], |a, now| a.run_pane("pane.arrange_vertical", now)),
    Action::new("pane.arrange_horizontal", S::ActionsPaneArrangeHorizontal, &[], |a, now| a.run_pane("pane.arrange_horizontal", now)),
    Action::new("pane.close", S::ActionsPaneClose, &[ctrl_alt(Key::Char('W'))], |a, now| a.run_pane("pane.close", now)),
    Action::new("pane.focus", S::ActionsPaneFocus, &[ctrl_shift(Key::Right)], |a, now| a.run_pane("pane.focus", now)),
    Action::new("pane.open_other", S::ActionsPaneOpenOther, &[ctrl_shift(Key::Enter)], |a, now| a.run_pane("pane.open_other", now)),
    Action::new("pane.here_other", S::ActionsPaneHereOther, &[], |a, now| a.run_pane("pane.here_other", now)),
    Action::new("pane.new_vertical", S::ActionsPaneNewVertical, &[], |a, now| a.run_pane("pane.new_vertical", now)),
    Action::new("pane.new_horizontal", S::ActionsPaneNewHorizontal, &[], |a, now| a.run_pane("pane.new_horizontal", now)),
    Action::new("pane.dual_default", S::ActionsPaneDualDefault, &[], |a, now| a.run_pane("pane.dual_default", now)),
    Action::new("pane.default_direction", S::ActionsPaneDefaultDirection, &[], |a, now| a.run_pane("pane.default_direction", now)),
    Action::new("window.new", S::ActionsWindowNew, &[ctrl(Key::Char('N'))], |a, _| a.new_window_here()),
    Action::new("window.fullscreen", S::ActionsWindowFullscreen, &[key(Key::F(11))], |a, _| a.effects.push(Effect::Window(WindowCmd::Fullscreen))),
    Action::new("window.compact", S::ActionsWindowCompact, &[key(Key::F(12))], |a, _| a.effects.push(Effect::Window(WindowCmd::Compact(None)))),
    Action::new("window.compact_on", S::ActionsWindowCompactOn, &[ctrl_alt(Key::Up)], |a, _| a.effects.push(Effect::Window(WindowCmd::Compact(Some(true))))),
    Action::new("window.compact_off", S::ActionsWindowCompactOff, &[ctrl_alt(Key::Down)], |a, _| a.effects.push(Effect::Window(WindowCmd::Compact(Some(false))))),
    Action::new("view.filter", S::ActionsViewFilter, &[ctrl(Key::Char('F'))], |a, _| a.open_filter()),
    Action::new("search.open", S::ActionsSearchOpen, &[ctrl(Key::Char('E')), ctrl_shift(Key::Char('F'))], |a, _| a.open_search()),
    Action::new("search.group_folder", S::ActionsSearchGroupFolder, &[], |a, _| a.toggle_search_groups()),
    Action::new("search.folder", S::ActionsSearchFolder, &[key(Key::F(3))], |a, _| a.open_folder_search()),
    Action::new("search.clear_history", S::ActionsSearchClearHistory, &[], |a, _| a.clear_search_history()),
    Action::new("search.reveal", S::ActionsSearchReveal, &[], |a, now| a.reveal_search_focus(now)),
    Action::new("view.hidden", S::ActionsViewHidden, &[ctrl(Key::Char('H'))], |a, _| a.toggle_hidden()),
    Action::new("view.sort.name", S::ActionsViewSortName, &[], |a, _| a.sort_by(SortField::Name)),
    Action::new("view.sort.modified", S::ActionsViewSortModified, &[], |a, _| a.sort_by(SortField::Modified)),
    Action::new("view.sort.type", S::ActionsViewSortType, &[], |a, _| a.sort_by(SortField::Type)),
    Action::new("view.sort.size", S::ActionsViewSortSize, &[], |a, _| a.sort_by(SortField::Size)),
    Action::new("view.sort.origin", S::ActionsViewSortOrigin, &[], |a, _| a.sort_by(SortField::Origin)),
    Action::new("view.sort.created", S::ActionsViewSortCreated, &[], |a, _| a.sort_by(SortField::Created)),
    Action::new("view.sort.tag", S::ActionsViewSortTag, &[], |a, _| a.sort_by(SortField::Tag)),
    Action::new("view.sort.reverse", S::ActionsViewSortReverse, &[], |a, _| a.set_descending(None)),
    Action::new("view.sort.ascending", S::ActionsViewSortAscending, &[], |a, _| a.set_descending(Some(false))),
    Action::new("view.sort.descending", S::ActionsViewSortDescending, &[], |a, _| a.set_descending(Some(true))),
    Action::new("view.sort.folders_first", S::ActionsViewSortFoldersFirst, &[], |a, _| a.set_folders(Folders::First)),
    Action::new("view.sort.files_first", S::ActionsViewSortFilesFirst, &[], |a, _| a.set_folders(Folders::Last)),
    Action::new("view.sort.mixed", S::ActionsViewSortMixed, &[], |a, _| a.set_folders(Folders::Mixed)),
    Action::new("view.group.none", S::ActionsViewGroupNone, &[], |a, _| a.group_by(GroupField::None)),
    Action::new("view.group.name", S::ActionsViewGroupName, &[], |a, _| a.group_by(GroupField::Name)),
    Action::new("view.group.type", S::ActionsViewGroupType, &[], |a, _| a.group_by(GroupField::Type)),
    Action::new("view.group.size", S::ActionsViewGroupSize, &[], |a, _| a.group_by(GroupField::Size)),
    Action::new("view.group.modified", S::ActionsViewGroupModified, &[], |a, _| a.group_by(GroupField::Modified)),
    Action::new("view.group.created", S::ActionsViewGroupCreated, &[], |a, _| a.group_by(GroupField::Created)),
    Action::new("view.group.origin", S::ActionsViewGroupOrigin, &[], |a, _| a.group_by(GroupField::Origin)),
    Action::new("view.group.tag", S::ActionsViewGroupTag, &[], |a, _| a.group_by(GroupField::Tag)),
    Action::new("view.group.ascending", S::ActionsViewGroupAscending, &[], |a, _| a.set_group_descending(false)),
    Action::new("view.group.descending", S::ActionsViewGroupDescending, &[], |a, _| a.set_group_descending(true)),
    Action::new("view.group.year", S::ActionsViewGroupYear, &[], |a, _| a.set_date_unit(DateUnit::Year)),
    Action::new("view.group.month", S::ActionsViewGroupMonth, &[], |a, _| a.set_date_unit(DateUnit::Month)),
    Action::new("view.group.day", S::ActionsViewGroupDay, &[], |a, _| a.set_date_unit(DateUnit::Day)),
    Action::new("view.layout.details", S::ActionsViewLayoutDetails, &[ctrl_shift(Key::Char('1'))], |a, _| a.set_layout(LayoutMode::Details)),
    Action::new("view.layout.list", S::ActionsViewLayoutList, &[ctrl_shift(Key::Char('2'))], |a, _| a.set_layout(LayoutMode::List)),
    Action::new("view.layout.cards", S::ActionsViewLayoutCards, &[ctrl_shift(Key::Char('3'))], |a, _| a.set_layout(LayoutMode::Cards)),
    Action::new("view.layout.columns", S::ActionsViewLayoutColumns, &[ctrl_shift(Key::Char('4'))], |a, _| a.set_layout(LayoutMode::Columns)),
    Action::new("view.layout.grid", S::ActionsViewLayoutGrid, &[ctrl_shift(Key::Char('5'))], |a, _| a.set_layout(LayoutMode::Grid)),
    Action::new("view.layout.adaptive", S::ActionsViewLayoutAdaptive, &[ctrl_shift(Key::Char('6'))], |a, _| a.set_layout(LayoutMode::Adaptive)),
    Action::new("view.size.larger", S::ActionsViewSizeLarger, &[ctrl(Key::Char('+')), ctrl_shift(Key::Char('+'))], |a, _| a.resize_items(1)),
    Action::new("view.size.smaller", S::ActionsViewSizeSmaller, &[ctrl(Key::Char('-'))], |a, _| a.resize_items(-1)),
    Action::new("view.thumbnails", S::ActionsViewThumbnails, &[], |a, _| a.toggle_pref(|p| &mut p.thumbnails)),
    Action::new("view.thumbnails.clear", S::ActionsViewThumbnailsClear, &[], |a, _| a.clear_thumbnails()),
    Action::new("view.columns.fit", S::ActionsViewColumnsFit, &[ctrl(Key::NumPlus)], |a, _| a.request_fit()),
    Action::new("view.columns.autofit", S::ActionsViewColumnsAutofit, &[], |a, _| a.toggle_auto_fit()),
    Action::new("view.column.modified", S::ActionsViewColumnModified, &[], |a, _| a.toggle_column(Column::Modified)),
    Action::new("view.column.created", S::ActionsViewColumnCreated, &[], |a, _| a.toggle_column(Column::Created)),
    Action::new("view.column.type", S::ActionsViewColumnType, &[], |a, _| a.toggle_column(Column::Type)),
    Action::new("view.column.size", S::ActionsViewColumnSize, &[], |a, _| a.toggle_column(Column::Size)),
    Action::new("view.column.origin", S::ActionsViewColumnOrigin, &[], |a, _| a.toggle_column(Column::Origin)),
    Action::new("view.column.deleted", S::ActionsViewColumnDeleted, &[], |a, _| a.toggle_column(Column::Deleted)),
    Action::new("view.column.tag", S::ActionsViewColumnTag, &[], |a, _| a.toggle_column(Column::Tag)),
    Action::new("view.column.git_status", S::ActionsViewColumnGitStatus, &[], |a, _| a.toggle_column(Column::GitStatus)),
    Action::new("view.column.git_message", S::ActionsViewColumnGitMessage, &[], |a, _| a.toggle_column(Column::GitMessage)),
    Action::new("view.column.git_author", S::ActionsViewColumnGitAuthor, &[], |a, _| a.toggle_column(Column::GitAuthor)),
    Action::new("view.column.git_date", S::ActionsViewColumnGitDate, &[], |a, _| a.toggle_column(Column::GitDate)),
    Action::new("git.fetch", S::ActionsGitFetch, &[], |a, _| a.git_run(GitOp::Fetch)),
    Action::new("git.pull", S::ActionsGitPull, &[], |a, _| a.git_run(GitOp::Pull)),
    Action::new("git.push", S::ActionsGitPush, &[], |a, _| a.git_run(GitOp::Push)),
    Action::new("git.init", S::ActionsGitInit, &[], |a, _| a.git_run(GitOp::Init)),
    Action::new("git.clone", S::ActionsGitClone, &[], |a, _| a.ask_clone()),
    Action::new("git.branches", S::ActionsGitBranches, &[], |a, _| a.git_branches_menu()),
    Action::new("git.open_remote", S::ActionsGitOpenRemote, &[], |a, _| a.git_run(GitOp::OpenRemote)),
    Action::new("view.dotfiles", S::ActionsViewDotfiles, &[], |a, _| a.toggle_pref(|p| &mut p.show_dotfiles)),
    Action::new("view.system", S::ActionsViewSystem, &[], |a, _| a.toggle_pref(|p| &mut p.show_system)),
    Action::new("view.extensions", S::ActionsViewExtensions, &[], |a, _| a.toggle_pref(|p| &mut p.show_extensions)),
    Action::new("view.status_bar", S::ActionsViewStatusBar, &[], |a, _| a.toggle_pref(|p| &mut p.status_bar)),
    Action::new("view.item_count", S::ActionsViewItemCount, &[], |a, _| a.toggle_pref(|p| &mut p.split_count)),
    Action::new("theme.toggle", S::ActionsThemeToggle, &[ctrl_alt(Key::Char('T'))], |a, _| a.toggle_theme()),
    Action::new("theme.light", S::ActionsThemeLight, &[], |a, _| a.set_theme_mode(crate::theme::Mode::Light)),
    Action::new("theme.dark", S::ActionsThemeDark, &[], |a, _| a.set_theme_mode(crate::theme::Mode::Dark)),
    Action::new("theme.system", S::ActionsThemeSystem, &[], |a, _| a.set_theme_mode(crate::theme::Mode::System)),
    Action::new("view.streams", S::ActionsViewStreams, &[], |a, now| a.toggle_streams(now)),
    Action::new("view.checkboxes", S::ActionsViewCheckboxes, &[], |a, _| a.toggle_pref(|p| &mut p.checkboxes)),
    Action::new("view.hover_select", S::ActionsViewHoverSelect, &[], |a, _| a.toggle_pref(|p| &mut p.hover_select)),
    Action::new("view.auto_sizes", S::ActionsViewAutoSizes, &[], |a, _| a.toggle_pref(|p| &mut p.auto_sizes)),
    Action::new("view.same_for_all", S::ActionsViewSameForAll, &[], |a, _| a.toggle_pref(|p| &mut p.same_for_all)),
    Action::new("view.set_default", S::ActionsViewSetDefault, &[], |a, _| a.set_default_view()),
    Action::new("view.sizes", S::ActionsViewSizes, &[ctrl_alt(Key::Char('S'))], |a, _| a.compute_dir_sizes()),
    Action::new("sidebar.toggle", S::ActionsSidebarToggle, &[ctrl(Key::Char('B'))], |a, _| a.toggle_sidebar()),
    Action::new("sidebar.focus", S::ActionsSidebarFocus, &[key(Key::F(6))], |a, _| a.cycle_sidebar_focus()),
    Action::new("sidebar.pin", S::ActionsSidebarPin, &[ctrl(Key::Char('D'))], |a, _| a.pin_current()),
    Action::new("toolbar.toggle", S::ActionsToolbarToggle, &[ctrl_shift(Key::Char('B'))], |a, _| a.toggle_toolbar()),
    Action::new("toolbar.customize", S::ActionsToolbarCustomize, &[], |a, _| a.customize_toolbar()),
    Action::new("toolbar.reset", S::ActionsToolbarReset, &[], |a, _| a.reset_toolbar()),
    Action::new("sidebar.compact", S::ActionsSidebarCompact, &[], |a, _| a.toggle_compact()),
    Action::new("sidebar.sections", S::ActionsSidebarSections, &[], |a, _| a.show_all_sections()),
    Action::new("home.quick_access", S::ActionsHomeQuickAccess, &[], |a, _| a.toggle_widget(Widget::QuickAccess)),
    Action::new("home.drives", S::ActionsHomeDrives, &[], |a, _| a.toggle_widget(Widget::Drives)),
    Action::new("home.tags", S::ActionsHomeTags, &[], |a, _| a.toggle_widget(Widget::Tags)),
    Action::new("home.recent", S::ActionsHomeRecent, &[], |a, _| a.toggle_widget(Widget::Recent)),
    Action::new("home.clear_recent", S::ActionsHomeClearRecent, &[], |a, _| a.clear_recent()),
    Action::new("libraries.new", S::ActionsLibrariesNew, &[], |a, _| a.ask_new_library()),
    Action::new("libraries.restore", S::ActionsLibrariesRestore, &[], |a, _| a.effects.push(Effect::Library(LibraryOp::RestoreDefaults))),
    Action::new("library.locations", S::ActionsLibraryLocations, &[], |a, _| {
        if let Some(lib) = a.current_library() {
            a.library_dialog(lib);
        }
    }),
    Action::new("select.all", S::ActionsSelectAll, &[ctrl(Key::Char('A'))], |a, _| a.select_all()),
    Action::new("select.none", S::ActionsSelectNone, &[key(Key::Escape), ctrl_shift(Key::Char('A'))], |a, _| a.clear_selection()),
    Action::new("select.invert", S::ActionsSelectInvert, &[ctrl(Key::Char('I'))], |a, _| a.invert_selection()),
    Action::new("item.open", S::ActionsItemOpen, &[key(Key::Enter)], |a, now| a.open_selection(now)),
    Action::new("item.open_tab", S::ActionsItemOpenTab, &[ctrl(Key::Enter)], |a, now| a.open_folders_in(OpenTarget::NewTab, now)),
    Action::new("item.open_window", S::ActionsItemOpenWindow, &[ctrl_alt(Key::Enter)], |a, now| a.open_folders_in(OpenTarget::NewWindow, now)),
    Action::new("item.pin", S::ActionsItemPin, &[], |a, _| a.pin_selection()),
    Action::new("tags.new", S::ActionsTagsNew, &[], |a, _| a.new_tag(true)),
    Action::new("tags.remove", S::ActionsTagsRemove, &[], |a, _| a.remove_tags()),
    Action::new("tags.manage", S::ActionsTagsManage, &[], |a, _| a.open_tag_pick(TagPick::Manage, "")),
    Action::new("tags.open_all", S::ActionsTagsOpenAll, &[], |a, _| a.open_tag_pick(TagPick::OpenAll, "")),
    Action::new("tags.settings", S::ActionsTagsSettings, &[], |a, _| a.open_settings(Some(Page::Tags))),
    Action::new("settings.open", S::ActionsSettingsOpen, &[ctrl(Key::Char(','))], |a, _| a.open_settings(None)),
    Action::new("settings.file", S::ActionsSettingsFile, &[ctrl_shift(Key::Char(','))], |a, _| a.open_settings_file()),
    Action::new("settings.export", S::ActionsSettingsExport, &[], |a, _| a.ask_export_settings()),
    Action::new("settings.import", S::ActionsSettingsImport, &[], |a, _| a.ask_import_settings()),
    Action::new("settings.reset", S::ActionsSettingsReset, &[], |a, _| a.ask_reset_settings()),
    Action::new("help.docs", S::ActionsHelpDocs, &[key(Key::F(1))], |a, _| a.open_link("help.docs")),
    Action::new("help.bug", S::ActionsHelpBug, &[], |a, _| a.open_link("help.bug")),
    Action::new("help.feature", S::ActionsHelpFeature, &[], |a, _| a.open_link("help.feature")),
    Action::new("help.repository", S::ActionsHelpRepository, &[], |a, _| a.open_link("help.repository")),
    Action::new("help.privacy", S::ActionsHelpPrivacy, &[], |a, _| a.open_link("help.privacy")),
    Action::new("logs.file", S::ActionsLogsFile, &[ctrl(Key::Char('.'))], |a, _| a.open_log_file()),
    Action::new("logs.folder", S::ActionsLogsFolder, &[ctrl_shift(Key::Char('.'))], |a, now| a.open_log_folder(now)),
    Action::new("item.copy_path", S::ActionsItemCopyPath, &[ctrl_shift(Key::Char('C'))], |a, _| a.copy_paths(false)),
    Action::new("item.copy_quoted_path", S::ActionsItemCopyQuotedPath, &[ctrl_alt(Key::Char('C'))], |a, _| a.copy_paths(true)),
    Action::new("app.terminal", S::ActionsAppTerminal, &[ctrl(Key::Char('`'))], |a, _| a.open_terminal(false)),
    Action::new("app.terminal_admin", S::ActionsAppTerminalAdmin, &[ctrl_shift(Key::Char('`'))], |a, _| a.open_terminal(true)),
    Action::new("item.trash", S::ActionsItemTrash, &[key(Key::Delete)], |a, _| a.trash_selection()),
    Action::new("item.delete", S::ActionsItemDelete, &[shift(Key::Delete)], |a, _| a.delete_selection()),
    Action::new("bin.restore", S::ActionsBinRestore, &[], |a, _| a.restore_selection()),
    Action::new("bin.restore_all", S::ActionsBinRestoreAll, &[], |a, _| a.restore_all()),
    Action::new("bin.empty", S::ActionsBinEmpty, &[], |a, _| a.empty_bin()),
    Action::new("file.copy", S::ActionsFileCopy, &[ctrl(Key::Char('C'))], |a, _| a.copy_selection()),
    Action::new("file.cut", S::ActionsFileCut, &[ctrl(Key::Char('X'))], |a, _| a.cut_selection()),
    Action::new("file.paste", S::ActionsFilePaste, &[ctrl(Key::Char('V'))], |a, _| a.paste()),
    Action::new("file.paste_into", S::ActionsFilePasteInto, &[ctrl_shift(Key::Char('V'))], |a, _| a.paste_into_selection()),
    Action::new("file.rename", S::ActionsFileRename, &[key(Key::F(2))], |a, _| a.rename_focused()),
    Action::new("file.share", S::ActionsFileShare, &[], |a, _| a.share_selection()),
    Action::new("file.new_folder", S::ActionsFileNewFolder, &[ctrl_shift(Key::Char('N'))], |a, _| a.new_folder()),
    Action::new("file.new_item", S::ActionsFileNewItem, &[ctrl_shift(Key::Char('I'))], |a, _| a.new_item_dialog()),
    Action::new("file.new_folder_with_selection", S::ActionsFileNewFolderWithSelection, &[], |a, _| a.new_folder_with_selection()),
    Action::new("file.new_stream", S::ActionsFileNewStream, &[], |a, _| a.new_stream()),
    Action::new("file.folder_icon", S::ActionsFileFolderIcon, &[], |a, _| a.ask_folder_icon()),
    Action::new("file.folder_icon_reset", S::ActionsFileFolderIconReset, &[], |a, _| a.reset_folder_icon()),
    Action::new("file.shortcut", S::ActionsFileShortcut, &[], |a, _| a.create_shortcuts()),
    Action::new("file.flatten", S::ActionsFileFlatten, &[], |a, _| a.flatten_selection()),
    Action::new("file.install_fonts", S::OpsExtraInstallFonts, &[], |a, _| a.install_fonts_selection()),
    Action::new("file.lock_screen", S::OpsExtraLockScreen, &[], |a, _| a.lock_screen_selection()),
    Action::new("file.set_backdrop", S::OpsExtraSetBackdrop, &[], |a, _| a.backdrop_selection()),
    Action::new("view.clear_backdrop", S::OpsExtraClearBackdrop, &[], |a, _| a.set_backdrop(String::new())),
    Action::new("archive.compress", S::ActionsArchiveCompress, &[], |a, _| a.compress_dialog()),
    Action::new("archive.zip", S::ActionsArchiveZip, &[], |a, _| a.compress_quick(Format::Zip)),
    Action::new("archive.7z", S::ActionsArchive7z, &[], |a, _| a.compress_quick(Format::SevenZ)),
    Action::new("archive.extract", S::ActionsArchiveExtract, &[alt(Key::Char('E'))], |a, _| a.extract_dialog()),
    Action::new("archive.extract_here", S::ActionsArchiveExtractHere, &[], |a, _| a.extract_with(Place::Here)),
    Action::new("archive.extract_folder", S::ActionsArchiveExtractFolder, &[], |a, _| a.extract_with(Place::OwnFolder)),
    Action::new("archive.smart", S::ActionsArchiveSmart, &[ctrl_shift(Key::Char('E'))], |a, _| a.extract_with(Place::Smart)),
    Action::new("archive.open", S::ActionsArchiveOpen, &[], |a, now| a.open_as_folder(now)),
    Action::new("archive.encoding", S::ActionsArchiveEncoding, &[], |a, _| a.pick_encoding()),
    Action::new("file.hashes", S::ActionsFileHashes, &[], |a, _| a.hashes_dialog()),
    Action::new("ops.show", S::ActionsOpsShow, &[], |a, _| a.toggle_jobs()),
    Action::new("ops.clear", S::ActionsOpsClear, &[], |a, _| a.clear_completed()),
    Action::new("edit.undo", S::ActionsEditUndo, &[ctrl(Key::Char('Z'))], |a, _| a.undo()),
    Action::new("edit.redo", S::ActionsEditRedo, &[ctrl(Key::Char('Y'))], |a, _| a.redo()),
    Action::new("item.properties", S::ActionsItemProperties, &[alt(Key::Enter)], |a, _| a.properties_dialog()),
    Action::new("item.shell_properties", S::PropertiesShellAction, &[alt_shift(Key::Enter)], |a, _| a.properties()),
    Action::new("view.pane", S::ActionsViewPane, &[ctrl_alt(Key::Char('I'))], |a, _| a.toggle_preview_pane()),
    Action::new("view.pane.preview", S::ActionsViewPanePreview, &[alt(Key::Char('P'))], |a, _| a.show_preview_tab(false)),
    Action::new("view.pane.details", S::ActionsViewPaneDetails, &[alt_shift(Key::Char('P'))], |a, _| a.show_preview_tab(true)),
    Action::new("item.quick_view", S::ActionsItemQuickView, &[], |a, _| a.quick_view()),
    Action::new("preview.play", S::ActionsPreviewPlay, &[], |a, _| a.play_preview()),
    Action::new("item.context_menu", S::ActionsItemContextMenu, &[shift(Key::F(10)), key(Key::Apps)], |a, _| a.context_menu()),
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
        let Some(i) = self.keymap.owner(Chord { key, mods }).filter(|&i| self.caps.supports(ACTIONS[i].id)) else { return false };
        (ACTIONS[i].run)(self, now);
        true
    }

    pub fn run_action(&mut self, id: &str, now: Instant) -> bool {
        let Some(a) = ACTIONS.iter().find(|a| a.id == id && self.caps.supports(id)) else { return false };
        self.dirty = true;
        (a.run)(self, now);
        true
    }

    pub fn available(&self, id: &str) -> bool {
        if !self.caps.supports(id) {
            return false;
        }
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
            return self.status(S::ActionsStatusSelectFolders.t());
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
