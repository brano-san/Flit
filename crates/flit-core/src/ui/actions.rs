use super::home::Widget;
use super::ops::Insert;
use super::preview_pane::PaneTool;
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
use crate::sort::{Folders, GroupField, SortField};
use super::layouts::{ZOOM_DETAILS, ZOOM_ICONS, ZOOM_LARGE, ZOOM_LIST, ZOOM_TREE, ZOOM_XL};
use std::time::Instant;

const MAX_CLOSED: usize = 32;
const FONT_STEP: i32 = 10;

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

const fn c(ch: char) -> Key {
    Key::Char(ch)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Group {
    Quick,
    Folder,
    View,
    Sort,
    GroupBy,
    Inspector,
    Panels,
    Tabs,
    GoTo,
    Misc,
    Options,
    Rename,
    Text,
    Scroll,
    Flit,
}

impl Group {
    pub const ALL: [Group; 15] = [Group::Quick, Group::Folder, Group::View, Group::Sort, Group::GroupBy, Group::Inspector, Group::Panels, Group::Tabs, Group::GoTo, Group::Misc, Group::Options, Group::Rename, Group::Text, Group::Scroll, Group::Flit];

    pub fn title(self) -> S {
        match self {
            Group::Quick => S::PaletteGroupQuick,
            Group::Folder => S::PaletteGroupFolder,
            Group::View => S::PaletteGroupView,
            Group::Sort => S::PaletteGroupSort,
            Group::GroupBy => S::PaletteGroupGroupBy,
            Group::Inspector => S::PaletteGroupInspector,
            Group::Panels => S::PaletteGroupPanels,
            Group::Tabs => S::PaletteGroupTabs,
            Group::GoTo => S::PaletteGroupGoTo,
            Group::Misc => S::PaletteGroupMisc,
            Group::Options => S::PaletteGroupOptions,
            Group::Rename => S::PaletteGroupRename,
            Group::Text => S::PaletteGroupText,
            Group::Scroll => S::PaletteGroupScroll,
            Group::Flit => S::PaletteGroupFlit,
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Group::Quick => "\u{E718}",
            Group::Folder => "\u{E8B7}",
            Group::View => "\u{E8B9}",
            Group::Sort => "\u{E8CB}",
            Group::GroupBy => "\u{E8FD}",
            Group::Inspector => "\u{E890}",
            Group::Panels => "\u{E8A0}",
            Group::Tabs => "\u{E923}",
            Group::GoTo => "\u{E72A}",
            Group::Misc => "\u{E71B}",
            Group::Options => "\u{E713}",
            Group::Rename => "\u{E8AC}",
            Group::Text => "\u{E8D2}",
            Group::Scroll => "\u{E70E}",
            Group::Flit => "\u{E7B8}",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ctx {
    Main,
    Field,
    Rename,
    GoTo,
    Tree,
    Dropdown,
}

pub struct Action {
    pub id: &'static str,
    pub group: Group,
    pub title: S,
    pub desc: S,
    pub keys: &'static [Chord],
    pub extra: &'static [&'static str],
    pub ctx: Ctx,
    pub run: Option<fn(&mut App, Instant)>,
}

impl Action {
    const fn new(id: &'static str, group: Group, title: S, desc: S, keys: &'static [Chord], run: fn(&mut App, Instant)) -> Self {
        Self { id, group, title, desc, keys, extra: &[], ctx: Ctx::Main, run: Some(run) }
    }

    const fn held(id: &'static str, group: Group, title: S, desc: S, keys: &'static [Chord], ctx: Ctx) -> Self {
        Self { id, group, title, desc, keys, extra: &[], ctx, run: None }
    }

    const fn also(mut self, extra: &'static [&'static str]) -> Self {
        self.extra = extra;
        self
    }

    const fn within(mut self, ctx: Ctx) -> Self {
        self.ctx = ctx;
        self
    }

    pub fn rebindable(&self) -> bool {
        self.ctx == Ctx::Main
    }

    pub fn shown_keys(&self, keys: &[Chord]) -> Vec<String> {
        let mouse = self.extra.iter().filter(|e| e.contains("Mouse")).map(|e| (*e).to_owned());
        let pad = self.extra.iter().filter(|e| !e.contains("Mouse")).map(|e| (*e).to_owned());
        mouse.chain(keys.iter().map(chord_text)).chain(pad).collect()
    }
}

use Group as G;

pub static ACTIONS: &[Action] = &[
    Action::new("subfolders.toggle", G::Quick, S::ActionsSubfoldersToggle, S::DescribeSubfoldersToggle, &[ctrl(c('E'))], |a, _| a.toggle_subfolders()),
    Action::new("inspector.toggle", G::Quick, S::ActionsInspectorToggle, S::DescribeInspectorToggle, &[key(Key::Space)], |a, _| a.toggle_preview_pane()),
    Action::new("panel.split_right", G::Quick, S::ActionsPanelSplitRight, S::DescribePanelSplitRight, &[ctrl_shift(c('S'))], |a, now| a.run_panel("panel.split_right", now)),
    Action::new("panel.split_bottom", G::Quick, S::ActionsPanelSplitBottom, S::DescribePanelSplitBottom, &[ctrl(c('S'))], |a, now| a.run_panel("panel.split_bottom", now)),
    Action::new("tab.new", G::Quick, S::ActionsTabNew, S::DescribeTabNew, &[ctrl(c('T'))], |a, now| a.open_new_tab(now)),
    Action::new("tab.close", G::Quick, S::ActionsTabClose, S::DescribeTabClose, &[ctrl(c('W')), ctrl(Key::F(4))], |a, _| a.close_tab(a.cur)).also(&["MouseMiddle"]),
    Action::new("options.open", G::Quick, S::ActionsOptionsOpen, S::DescribeOptionsOpen, &[ctrl(c(','))], |a, _| a.open_settings(None)),
    Action::new("folder.explorer", G::Quick, S::ActionsFolderExplorer, S::DescribeFolderExplorer, &[], |a, _| a.open_explorer()),
    Action::new("app.terminal", G::Quick, S::ActionsAppTerminal, S::DescribeAppTerminal, &[], |a, _| a.open_terminal(false)),
    Action::new("view.extensions", G::Quick, S::ActionsViewExtensions, S::DescribeViewExtensions, &[], |a, _| a.toggle_pref(|p| &mut p.show_extensions)),
    Action::new("view.hidden", G::Quick, S::ActionsViewHidden, S::DescribeViewHidden, &[ctrl(c('H'))], |a, _| a.toggle_hidden()),
    Action::new("view.system", G::Quick, S::ActionsViewSystem, S::DescribeViewSystem, &[], |a, _| a.toggle_pref(|p| &mut p.show_system)),
    Action::new("item.open", G::Folder, S::ActionsItemOpen, S::DescribeItemOpen, &[key(Key::Enter)], |a, now| a.open_selection(now)).also(&["KeypadEnter"]),
    Action::new("select.cursor", G::Folder, S::ActionsSelectCursor, S::DescribeSelectCursor, &[ctrl(Key::Space)], |a, _| a.toggle_cursor_selection()),
    Action::new("item.open_tab", G::Folder, S::ActionsItemOpenTab, S::DescribeItemOpenTab, &[ctrl(Key::Enter)], |a, now| a.open_folders_in(OpenTarget::NewTab, now)),
    Action::new("panel.open_right", G::Folder, S::ActionsPanelOpenRight, S::DescribePanelOpenRight, &[ctrl_shift(Key::Enter)], |a, now| a.run_panel("panel.open_right", now)),
    Action::new("panel.open_bottom", G::Folder, S::ActionsPanelOpenBottom, S::DescribePanelOpenBottom, &[], |a, now| a.run_panel("panel.open_bottom", now)),
    Action::new("item.open_window", G::Folder, S::ActionsItemOpenWindow, S::DescribeItemOpenWindow, &[], |a, now| a.open_folders_in(OpenTarget::NewWindow, now)),
    Action::new("tree.open", G::Folder, S::ActionsTreeOpen, S::DescribeTreeOpen, &[key(Key::Right)], |a, _| a.tree_open(false)).also(&["MouseLeft"]).within(Ctx::Tree),
    Action::new("tree.close", G::Folder, S::ActionsTreeClose, S::DescribeTreeClose, &[key(Key::Left)], |a, _| a.tree_close(false)).also(&["MouseLeft"]).within(Ctx::Tree),
    Action::new("tree.open_all", G::Folder, S::ActionsTreeOpenAll, S::DescribeTreeOpenAll, &[shift(Key::Right)], |a, _| a.tree_open(true)).also(&["Shift+MouseLeft"]).within(Ctx::Tree),
    Action::new("tree.close_all", G::Folder, S::ActionsTreeCloseAll, S::DescribeTreeCloseAll, &[shift(Key::Left)], |a, _| a.tree_close(true)).also(&["Shift+MouseLeft"]).within(Ctx::Tree),
    Action::new("file.copy", G::Folder, S::ActionsFileCopy, S::DescribeFileCopy, &[ctrl(c('C')), ctrl(Key::Insert)], |a, _| a.copy_selection()),
    Action::new("file.cut", G::Folder, S::ActionsFileCut, S::DescribeFileCut, &[ctrl(c('X'))], |a, _| a.cut_selection()),
    Action::new("file.paste", G::Folder, S::ActionsFilePaste, S::DescribeFilePaste, &[ctrl(c('V')), shift(Key::Insert)], |a, _| a.paste()),
    Action::new("file.paste_shortcut", G::Folder, S::ActionsFilePasteShortcut, S::DescribeFilePasteShortcut, &[], |a, _| a.paste_shortcut()),
    Action::new("panel.copy_previous", G::Folder, S::ActionsPanelCopyPrevious, S::DescribePanelCopyPrevious, &[], |a, _| a.send_to_panel(true, false)),
    Action::new("panel.copy_next", G::Folder, S::ActionsPanelCopyNext, S::DescribePanelCopyNext, &[ctrl(Key::F(6))], |a, _| a.send_to_panel(false, false)),
    Action::new("panel.move_previous", G::Folder, S::ActionsPanelMovePrevious, S::DescribePanelMovePrevious, &[], |a, _| a.send_to_panel(true, true)),
    Action::new("panel.move_next", G::Folder, S::ActionsPanelMoveNext, S::DescribePanelMoveNext, &[shift(Key::F(6))], |a, _| a.send_to_panel(false, true)),
    Action::new("item.trash", G::Folder, S::ActionsItemTrash, S::DescribeItemTrash, &[key(Key::Delete)], |a, _| a.trash_selection()),
    Action::new("item.delete", G::Folder, S::ActionsItemDelete, S::DescribeItemDelete, &[shift(Key::Delete)], |a, _| a.delete_selection()),
    Action::new("file.duplicate", G::Folder, S::ActionsFileDuplicate, S::DescribeFileDuplicate, &[], |a, _| a.duplicate_selection()),
    Action::new("file.rename", G::Folder, S::ActionsFileRename, S::DescribeFileRename, &[key(Key::F(2))], |a, _| a.rename_focused()),
    Action::new("item.inspect", G::Folder, S::ActionsItemInspect, S::DescribeItemInspect, &[alt(Key::Enter)], |a, _| a.inspect()).also(&["Alt+KeypadEnter"]),
    Action::new("bin.restore", G::Folder, S::ActionsBinRestore, S::DescribeBinRestore, &[], |a, _| a.restore_selection()),
    Action::new("item.copy_path", G::Folder, S::ActionsItemCopyPath, S::DescribeItemCopyPath, &[ctrl_shift(c('C'))], |a, _| a.copy_paths(Quote::Never)),
    Action::new("item.copy_path_smart", G::Folder, S::ActionsItemCopyPathSmart, S::DescribeItemCopyPathSmart, &[], |a, _| a.copy_paths(Quote::Needed)),
    Action::new("file.new_file", G::Folder, S::ActionsFileNewFile, S::DescribeFileNewFile, &[], |a, _| a.new_file()),
    Action::new("file.new_folder", G::Folder, S::ActionsFileNewFolder, S::DescribeFileNewFolder, &[ctrl_shift(c('N'))], |a, _| a.new_folder()),
    Action::new("item.open_relative", G::Folder, S::ActionsItemOpenRelative, S::DescribeItemOpenRelative, &[ctrl_shift(c('E'))], |a, now| a.open_relative(OpenTarget::CurrentTab, now)),
    Action::new("item.open_relative_tab", G::Folder, S::ActionsItemOpenRelativeTab, S::DescribeItemOpenRelativeTab, &[], |a, now| a.open_relative(OpenTarget::NewTab, now)).also(&["MouseMiddle"]),
    Action::new("item.open_containing", G::Folder, S::ActionsItemOpenContaining, S::DescribeItemOpenContaining, &[], |a, _| a.open_containing(false)),
    Action::new("item.open_containing_tab", G::Folder, S::ActionsItemOpenContainingTab, S::DescribeItemOpenContainingTab, &[], |a, _| a.open_containing(true)).also(&["MouseMiddle"]),
    Action::new("view.sort.cycle_folders", G::Folder, S::ActionsViewSortCycleFolders, S::DescribeViewSortCycleFolders, &[], |a, _| a.cycle_folders()),
    Action::new("view.toggle_folders", G::Folder, S::ActionsViewToggleFolders, S::DescribeViewToggleFolders, &[], |a, _| a.toggle_kind(true)),
    Action::new("view.toggle_files", G::Folder, S::ActionsViewToggleFiles, S::DescribeViewToggleFiles, &[], |a, _| a.toggle_kind(false)),
    Action::new("nav.address", G::Folder, S::ActionsNavAddress, S::DescribeNavAddress, &[ctrl(c('L')), alt(c('D'))], |a, _| a.open_address()),
    Action::new("view.columns.fit", G::Folder, S::ActionsViewColumnsFit, S::DescribeViewColumnsFit, &[ctrl(c('='))], |a, _| a.request_fit()),
    Action::new("view.filter", G::Folder, S::ActionsViewFilter, S::DescribeViewFilter, &[ctrl(c('F')), key(Key::F(3))], |a, _| a.open_filter()),
    Action::new("search.folder", G::Folder, S::ActionsSearchFolder, S::DescribeSearchFolder, &[ctrl_shift(c('F'))], |a, _| a.open_folder_search()),
    Action::new("folder.refresh_stats", G::Folder, S::ActionsFolderRefreshStats, S::DescribeFolderRefreshStats, &[alt(c('R'))], |a, _| a.compute_dir_sizes()),
    Action::new("nav.refresh", G::Folder, S::ActionsNavRefresh, S::DescribeNavRefresh, &[ctrl(c('R')), key(Key::F(5))], |a, now| a.refresh(now)),
    Action::new("nav.back", G::Folder, S::ActionsNavBack, S::DescribeNavBack, &[alt(Key::Left)], |a, now| a.go_history(false, now)).also(&["MouseX1"]),
    Action::new("nav.forward", G::Folder, S::ActionsNavForward, S::DescribeNavForward, &[alt(Key::Right)], |a, now| a.go_history(true, now)).also(&["MouseX2"]),
    Action::new("nav.up", G::Folder, S::ActionsNavUp, S::DescribeNavUp, &[alt(Key::Up), key(Key::Backspace)], |a, now| a.go_up(now)),
    Action::new("bin.empty", G::Folder, S::ActionsBinEmpty, S::DescribeBinEmpty, &[], |a, _| a.empty_bin()),
    Action::new("bin.restore_all", G::Folder, S::ActionsBinRestoreAll, S::DescribeBinRestoreAll, &[], |a, _| a.restore_all()),
    Action::new("folder.cycle_view", G::Folder, S::ActionsFolderCycleView, S::DescribeFolderCycleView, &[], |a, _| a.cycle_view_mode()),
    Action::new("folder.bookmark", G::Folder, S::ActionsFolderBookmark, S::DescribeFolderBookmark, &[], |a, _| a.pin_current()),
    Action::new("folder.dropdown", G::Folder, S::ActionsFolderDropdown, S::DescribeFolderDropdown, &[alt(Key::Down)], |a, _| a.open_folder_dropdown()),
    Action::held("folder.dropdown_left", G::Folder, S::ActionsFolderDropdownLeft, S::DescribeFolderDropdownLeft, &[alt(Key::Left)], Ctx::Dropdown),
    Action::held("folder.dropdown_right", G::Folder, S::ActionsFolderDropdownRight, S::DescribeFolderDropdownRight, &[alt(Key::Right)], Ctx::Dropdown),
    Action::new("folder.types", G::Folder, S::ActionsFolderTypes, S::DescribeFolderTypes, &[alt(c('T'))], |a, _| a.open_types()),
    Action::new("view.zoom_in", G::View, S::ActionsViewZoomIn, S::DescribeViewZoomIn, &[], |a, _| a.resize_items(1)).also(&["Ctrl+MouseWheelUp"]),
    Action::new("view.zoom_out", G::View, S::ActionsViewZoomOut, S::DescribeViewZoomOut, &[], |a, _| a.resize_items(-1)).also(&["Ctrl+MouseWheelDown"]),
    Action::new("view.layout.details", G::View, S::ActionsViewLayoutDetails, S::DescribeViewLayoutDetails, &[ctrl_shift(c('1'))], |a, _| a.set_zoom(ZOOM_DETAILS)),
    Action::new("view.layout.tree", G::View, S::ActionsViewLayoutTree, S::DescribeViewLayoutTree, &[ctrl_shift(c('2'))], |a, _| a.set_zoom(ZOOM_TREE)),
    Action::new("view.layout.list", G::View, S::ActionsViewLayoutList, S::DescribeViewLayoutList, &[ctrl_shift(c('3'))], |a, _| a.set_zoom(ZOOM_LIST)),
    Action::new("view.layout.icons_m", G::View, S::ActionsViewLayoutIconsM, S::DescribeViewLayoutIconsM, &[ctrl_shift(c('4'))], |a, _| a.set_zoom(ZOOM_ICONS)),
    Action::new("view.layout.icons_l", G::View, S::ActionsViewLayoutIconsL, S::DescribeViewLayoutIconsL, &[ctrl_shift(c('5'))], |a, _| a.set_zoom(ZOOM_LARGE)),
    Action::new("view.layout.icons_xl", G::View, S::ActionsViewLayoutIconsXl, S::DescribeViewLayoutIconsXl, &[ctrl_shift(c('6'))], |a, _| a.set_zoom(ZOOM_XL)),
    Action::new("view.sort.name", G::Sort, S::ActionsViewSortName, S::DescribeViewSortName, &[], |a, _| a.sort_by(SortField::Name)),
    Action::new("view.sort.size", G::Sort, S::ActionsViewSortSize, S::DescribeViewSortSize, &[], |a, _| a.sort_by(SortField::Size)),
    Action::new("view.sort.created", G::Sort, S::ActionsViewSortCreated, S::DescribeViewSortCreated, &[], |a, _| a.sort_by(SortField::Created)),
    Action::new("view.sort.modified", G::Sort, S::ActionsViewSortModified, S::DescribeViewSortModified, &[], |a, _| a.sort_by(SortField::Modified)),
    Action::new("view.sort.accessed", G::Sort, S::ActionsViewSortAccessed, S::DescribeViewSortAccessed, &[], |a, _| a.sort_by(SortField::Accessed)),
    Action::new("view.sort.deleted", G::Sort, S::ActionsViewSortDeleted, S::DescribeViewSortDeleted, &[], |a, _| a.sort_by(SortField::Modified)),
    Action::new("view.sort.items", G::Sort, S::ActionsViewSortItems, S::DescribeViewSortItems, &[], |a, _| a.sort_by(SortField::Items)),
    Action::new("view.sort.folders", G::Sort, S::ActionsViewSortFolders, S::DescribeViewSortFolders, &[], |a, _| a.sort_by(SortField::Folders)),
    Action::new("view.sort.files", G::Sort, S::ActionsViewSortFiles, S::DescribeViewSortFiles, &[], |a, _| a.sort_by(SortField::Files)),
    Action::new("view.sort.type", G::Sort, S::ActionsViewSortType, S::DescribeViewSortType, &[], |a, _| a.sort_by(SortField::Type)),
    Action::new("view.sort.origin", G::Sort, S::ActionsViewSortOrigin, S::DescribeViewSortOrigin, &[], |a, _| a.sort_by(SortField::Origin)),
    Action::new("view.sort.free", G::Sort, S::ActionsViewSortFree, S::DescribeViewSortFree, &[], |a, _| a.sort_by(SortField::Free)),
    Action::new("view.sort.file_system", G::Sort, S::ActionsViewSortFileSystem, S::DescribeViewSortFileSystem, &[], |a, _| a.sort_by(SortField::FileSystem)),
    Action::new("view.sort.percent", G::Sort, S::ActionsViewSortPercent, S::DescribeViewSortPercent, &[], |a, _| a.sort_by(SortField::Percent)),
    Action::new("view.sort.volume", G::Sort, S::ActionsViewSortVolume, S::DescribeViewSortVolume, &[], |a, _| a.sort_by(SortField::Volume)),
    Action::new("view.sort.folder_usage", G::Sort, S::ActionsViewSortFolderUsage, S::DescribeViewSortFolderUsage, &[], |a, _| a.sort_by(SortField::FolderUsage)),
    Action::new("view.group.none", G::GroupBy, S::ActionsViewGroupNone, S::DescribeViewGroupNone, &[], |a, _| a.group_by(GroupField::None)),
    Action::new("view.group.name", G::GroupBy, S::ActionsViewGroupName, S::DescribeViewGroupName, &[], |a, _| a.group_by(GroupField::Name)),
    Action::new("view.group.size", G::GroupBy, S::ActionsViewGroupSize, S::DescribeViewGroupSize, &[], |a, _| a.group_by(GroupField::Size)),
    Action::new("view.group.created", G::GroupBy, S::ActionsViewGroupCreated, S::DescribeViewGroupCreated, &[], |a, _| a.group_by(GroupField::Created)),
    Action::new("view.group.modified", G::GroupBy, S::ActionsViewGroupModified, S::DescribeViewGroupModified, &[], |a, _| a.group_by(GroupField::Modified)),
    Action::new("view.group.accessed", G::GroupBy, S::ActionsViewGroupAccessed, S::DescribeViewGroupAccessed, &[], |a, _| a.group_by(GroupField::Accessed)),
    Action::new("view.group.deleted", G::GroupBy, S::ActionsViewGroupDeleted, S::DescribeViewGroupDeleted, &[], |a, _| a.group_by(GroupField::Modified)),
    Action::new("view.group.items", G::GroupBy, S::ActionsViewGroupItems, S::DescribeViewGroupItems, &[], |a, _| a.group_by(GroupField::Items)),
    Action::new("view.group.type", G::GroupBy, S::ActionsViewGroupType, S::DescribeViewGroupType, &[], |a, _| a.group_by(GroupField::Type)),
    Action::new("view.group.origin", G::GroupBy, S::ActionsViewGroupOrigin, S::DescribeViewGroupOrigin, &[], |a, _| a.group_by(GroupField::Origin)),
    Action::new("view.group.free", G::GroupBy, S::ActionsViewGroupFree, S::DescribeViewGroupFree, &[], |a, _| a.group_by(GroupField::Free)),
    Action::new("view.group.file_system", G::GroupBy, S::ActionsViewGroupFileSystem, S::DescribeViewGroupFileSystem, &[], |a, _| a.group_by(GroupField::FileSystem)),
    Action::new("view.group.percent", G::GroupBy, S::ActionsViewGroupPercent, S::DescribeViewGroupPercent, &[], |a, _| a.group_by(GroupField::Percent)),
    Action::new("view.group.volume", G::GroupBy, S::ActionsViewGroupVolume, S::DescribeViewGroupVolume, &[], |a, _| a.group_by(GroupField::Volume)),
    Action::new("view.group.folder_usage", G::GroupBy, S::ActionsViewGroupFolderUsage, S::DescribeViewGroupFolderUsage, &[], |a, _| a.group_by(GroupField::FolderUsage)),
    Action::new("view.group.collapse_all", G::GroupBy, S::ActionsViewGroupCollapseAll, S::DescribeViewGroupCollapseAll, &[], |a, _| a.fold_groups(true)),
    Action::new("view.group.expand_all", G::GroupBy, S::ActionsViewGroupExpandAll, S::DescribeViewGroupExpandAll, &[], |a, _| a.fold_groups(false)),
    Action::new("inspector.details", G::Inspector, S::ActionsInspectorDetails, S::DescribeInspectorDetails, &[ctrl(c('I'))], |a, _| a.toggle_inspector_details()),
    Action::new("inspector.layout", G::Inspector, S::ActionsInspectorLayout, S::DescribeInspectorLayout, &[shift(Key::Space)], |a, _| a.toggle_inspector_layout()),
    Action::new("inspector.zoom_in", G::Inspector, S::ActionsInspectorZoomIn, S::DescribeInspectorZoomIn, &[], |a, _| a.inspector_zoom(true)).also(&["Ctrl+MouseWheelUp"]),
    Action::new("inspector.zoom_out", G::Inspector, S::ActionsInspectorZoomOut, S::DescribeInspectorZoomOut, &[], |a, _| a.inspector_zoom(false)).also(&["Ctrl+MouseWheelDown"]),
    Action::new("inspector.wrap", G::Inspector, S::ActionsInspectorWrap, S::DescribeInspectorWrap, &[], |a, _| a.inspector_toggle(PaneTool::Wrap)),
    Action::new("inspector.line_numbers", G::Inspector, S::ActionsInspectorLineNumbers, S::DescribeInspectorLineNumbers, &[], |a, _| a.inspector_toggle(PaneTool::Lines)),
    Action::new("inspector.whitespace", G::Inspector, S::ActionsInspectorWhitespace, S::DescribeInspectorWhitespace, &[], |a, _| a.inspector_toggle(PaneTool::Spaces)),
    Action::new("inspector.filter", G::Inspector, S::ActionsInspectorFilter, S::DescribeInspectorFilter, &[], |a, _| a.open_inspector_filter()),
    Action::new("inspector.grid", G::Inspector, S::ActionsInspectorGrid, S::DescribeInspectorGrid, &[], |a, _| a.inspector_toggle(PaneTool::Grid)),
    Action::new("inspector.scale", G::Inspector, S::ActionsInspectorScale, S::DescribeInspectorScale, &[], |a, _| a.cycle_inspector_scale()),
    Action::new("panel.next", G::Panels, S::ActionsPanelNext, S::DescribePanelNext, &[key(Key::Tab)], |a, now| a.run_panel("panel.next", now)),
    Action::new("panel.previous", G::Panels, S::ActionsPanelPrevious, S::DescribePanelPrevious, &[shift(Key::Tab)], |a, now| a.run_panel("panel.previous", now)),
    Action::new("panel.left", G::Panels, S::ActionsPanelLeft, S::DescribePanelLeft, &[], |a, now| a.run_panel("panel.left", now)),
    Action::new("panel.right", G::Panels, S::ActionsPanelRight, S::DescribePanelRight, &[], |a, now| a.run_panel("panel.right", now)),
    Action::new("panel.above", G::Panels, S::ActionsPanelAbove, S::DescribePanelAbove, &[], |a, now| a.run_panel("panel.above", now)),
    Action::new("panel.below", G::Panels, S::ActionsPanelBelow, S::DescribePanelBelow, &[], |a, now| a.run_panel("panel.below", now)),
    Action::new("panel.even_siblings", G::Panels, S::ActionsPanelEvenSiblings, S::DescribePanelEvenSiblings, &[], |a, now| a.run_panel("panel.even_siblings", now)),
    Action::new("panel.even_all", G::Panels, S::ActionsPanelEvenAll, S::DescribePanelEvenAll, &[], |a, now| a.run_panel("panel.even_all", now)),
    Action::new("tab.duplicate", G::Tabs, S::ActionsTabDuplicate, S::DescribeTabDuplicate, &[], |a, now| a.new_tab_here(now)),
    Action::new("tab.duplicate_window", G::Tabs, S::ActionsTabDuplicateWindow, S::DescribeTabDuplicateWindow, &[ctrl(c('N'))], |a, _| a.new_window_here()),
    Action::new("tab.shift_left", G::Tabs, S::ActionsTabShiftLeft, S::DescribeTabShiftLeft, &[ctrl_shift(Key::PageUp)], |a, _| a.shift_tab(false)),
    Action::new("tab.shift_right", G::Tabs, S::ActionsTabShiftRight, S::DescribeTabShiftRight, &[ctrl_shift(Key::PageDown)], |a, _| a.shift_tab(true)),
    Action::new("tab.close_others", G::Tabs, S::ActionsTabCloseOthers, S::DescribeTabCloseOthers, &[], |a, _| a.close_tabs(|i, cur| i == cur)),
    Action::new("tab.close_left", G::Tabs, S::ActionsTabCloseLeft, S::DescribeTabCloseLeft, &[], |a, _| a.close_tabs(|i, cur| i >= cur)),
    Action::new("tab.close_right", G::Tabs, S::ActionsTabCloseRight, S::DescribeTabCloseRight, &[], |a, _| a.close_tabs(|i, cur| i <= cur)),
    Action::new("tab.close_all", G::Tabs, S::ActionsTabCloseAll, S::DescribeTabCloseAll, &[ctrl_shift(c('W'))], |a, _| a.close_all_tabs()),
    Action::new("tab.reopen", G::Tabs, S::ActionsTabReopen, S::DescribeTabReopen, &[ctrl_shift(c('T'))], |a, now| a.reopen_tab(now)),
    Action::new("tab.next", G::Tabs, S::ActionsTabNext, S::DescribeTabNext, &[ctrl(Key::Tab), ctrl(Key::PageDown)], |a, _| a.select_tab((a.cur + 1) % a.tabs.len())),
    Action::new("tab.previous", G::Tabs, S::ActionsTabPrevious, S::DescribeTabPrevious, &[ctrl_shift(Key::Tab), ctrl(Key::PageUp)], |a, _| a.select_tab((a.cur + a.tabs.len() - 1) % a.tabs.len())),
    Action::new("tab.select_1", G::Tabs, S::ActionsTabSelect1, S::DescribeTabSelect1, &[ctrl(c('1'))], |a, _| a.select_tab(0)).also(&["Ctrl+Keypad1"]),
    Action::new("tab.select_2", G::Tabs, S::ActionsTabSelect2, S::DescribeTabSelect2, &[ctrl(c('2'))], |a, _| a.select_tab(1)).also(&["Ctrl+Keypad2"]),
    Action::new("tab.select_3", G::Tabs, S::ActionsTabSelect3, S::DescribeTabSelect3, &[ctrl(c('3'))], |a, _| a.select_tab(2)).also(&["Ctrl+Keypad3"]),
    Action::new("tab.select_4", G::Tabs, S::ActionsTabSelect4, S::DescribeTabSelect4, &[ctrl(c('4'))], |a, _| a.select_tab(3)).also(&["Ctrl+Keypad4"]),
    Action::new("tab.select_5", G::Tabs, S::ActionsTabSelect5, S::DescribeTabSelect5, &[ctrl(c('5'))], |a, _| a.select_tab(4)).also(&["Ctrl+Keypad5"]),
    Action::new("tab.select_6", G::Tabs, S::ActionsTabSelect6, S::DescribeTabSelect6, &[ctrl(c('6'))], |a, _| a.select_tab(5)).also(&["Ctrl+Keypad6"]),
    Action::new("tab.select_7", G::Tabs, S::ActionsTabSelect7, S::DescribeTabSelect7, &[ctrl(c('7'))], |a, _| a.select_tab(6)).also(&["Ctrl+Keypad7"]),
    Action::new("tab.select_8", G::Tabs, S::ActionsTabSelect8, S::DescribeTabSelect8, &[ctrl(c('8'))], |a, _| a.select_tab(7)).also(&["Ctrl+Keypad8"]),
    Action::new("tab.select_last", G::Tabs, S::ActionsTabSelectLast, S::DescribeTabSelectLast, &[ctrl(c('9'))], |a, _| a.select_tab(a.tabs.len() - 1)).also(&["Ctrl+Keypad9"]),
    Action::new("tab.menu", G::Tabs, S::ActionsTabMenu, S::DescribeTabMenu, &[ctrl_shift(Key::Down)], |a, _| a.open_tab_menu()).also(&["MouseRight"]),
    Action::new("tab.search", G::Tabs, S::ActionsTabSearch, S::DescribeTabSearch, &[ctrl_shift(c('A'))], |a, _| a.open_tab_search()),
    Action::held("goto.select", G::GoTo, S::ActionsGotoSelect, S::DescribeGotoSelect, &[ctrl(Key::Enter)], Ctx::GoTo),
    Action::held("goto.back", G::GoTo, S::ActionsGotoBack, S::DescribeGotoBack, &[alt(Key::Left)], Ctx::GoTo).also(&["MouseX1"]),
    Action::held("goto.forward", G::GoTo, S::ActionsGotoForward, S::DescribeGotoForward, &[alt(Key::Right)], Ctx::GoTo).also(&["MouseX2"]),
    Action::held("goto.up", G::GoTo, S::ActionsGotoUp, S::DescribeGotoUp, &[alt(Key::Up)], Ctx::GoTo),
    Action::new("config.create", G::Misc, S::ActionsConfigCreate, S::DescribeConfigCreate, &[], |a, _| a.create_config()),
    Action::new("config.open", G::Misc, S::ActionsConfigOpen, S::DescribeConfigOpen, &[], |a, _| a.open_settings_file()),
    Action::new("menu.main", G::Misc, S::ActionsMenuMain, S::DescribeMenuMain, &[key(Key::F(10))], |a, _| a.open_main_menu()),
    Action::new("quick_access.toggle", G::Misc, S::ActionsQuickAccessToggle, S::DescribeQuickAccessToggle, &[ctrl(c('B'))], |a, _| a.toggle_sidebar()),
    Action::new("quick_access.filter", G::Misc, S::ActionsQuickAccessFilter, S::DescribeQuickAccessFilter, &[alt(c('F'))], |a, _| a.focus_quick_filter()),
    Action::new("goto.open", G::Misc, S::ActionsGotoOpen, S::DescribeGotoOpen, &[ctrl(c('P')), key(Key::F(4))], |a, _| a.open_goto()),
    Action::new("palette.show", G::Misc, S::ActionsPaletteShow, S::DescribePaletteShow, &[ctrl_shift(c('P'))], |a, _| a.open_palette()),
    Action::new("item.context_menu", G::Misc, S::ActionsItemContextMenu, S::DescribeItemContextMenu, &[shift(Key::F(10)), key(Key::Apps)], |a, _| a.context_menu()),
    Action::new("quick.commands", G::Misc, S::ActionsQuickCommands, S::DescribeQuickCommands, &[], |a, _| a.quick_commands()).also(&["MouseMiddle"]),
    Action::new("bookmarks.clear", G::Misc, S::ActionsBookmarksClear, S::DescribeBookmarksClear, &[], |a, _| a.clear_bookmarks()),
    Action::new("home.clear_recent", G::Misc, S::ActionsHomeClearRecent, S::DescribeHomeClearRecent, &[], |a, _| a.clear_recent()),
    Action::new("views.clear", G::Misc, S::ActionsViewsClear, S::DescribeViewsClear, &[], |a, _| a.clear_folder_views()),
    Action::new("layout.save", G::Misc, S::ActionsLayoutSave, S::DescribeLayoutSave, &[], |a, _| a.ask_save_layout()),
    Action::new("layout.load", G::Misc, S::ActionsLayoutLoad, S::DescribeLayoutLoad, &[], |a, _| a.ask_load_layout()),
    Action::new("window.minimize", G::Misc, S::ActionsWindowMinimize, S::DescribeWindowMinimize, &[alt(c('M'))], |a, _| a.effects.push(Effect::Window(WindowCmd::Minimize))),
    Action::new("window.fullscreen", G::Misc, S::ActionsWindowFullscreen, S::DescribeWindowFullscreen, &[key(Key::F(11))], |a, _| a.effects.push(Effect::Window(WindowCmd::Fullscreen))),
    Action::new("window.close", G::Misc, S::ActionsWindowClose, S::DescribeWindowClose, &[alt(Key::F(4))], |a, _| a.effects.push(Effect::Window(WindowCmd::Close))),
    Action::new("font.increase", G::Options, S::ActionsFontIncrease, S::DescribeFontIncrease, &[ctrl_shift(c('='))], |a, _| a.step_font(FONT_STEP)).also(&["Alt+MouseWheelUp"]),
    Action::new("font.decrease", G::Options, S::ActionsFontDecrease, S::DescribeFontDecrease, &[ctrl(c('-'))], |a, _| a.step_font(-FONT_STEP)).also(&["Alt+MouseWheelDown"]),
    Action::new("font.reset", G::Options, S::ActionsFontReset, S::DescribeFontReset, &[ctrl(c('0'))], |a, _| a.set_ui_scale(100)),
    Action::new("spacing.increase", G::Options, S::ActionsSpacingIncrease, S::DescribeSpacingIncrease, &[], |a, _| a.step_spacing(super::options::SPACING_STEP)).also(&["Shift+Alt+MouseWheelUp"]),
    Action::new("spacing.decrease", G::Options, S::ActionsSpacingDecrease, S::DescribeSpacingDecrease, &[], |a, _| a.step_spacing(-super::options::SPACING_STEP)).also(&["Shift+Alt+MouseWheelDown"]),
    Action::new("rename.insert_id", G::Rename, S::ActionsRenameInsertId, S::DescribeRenameInsertId, &[], |a, _| a.rename_insert(Insert::Id)).within(Ctx::Rename),
    Action::new("rename.insert_date", G::Rename, S::ActionsRenameInsertDate, S::DescribeRenameInsertDate, &[], |a, _| a.rename_insert(Insert::Today)).within(Ctx::Rename),
    Action::new("rename.insert_created", G::Rename, S::ActionsRenameInsertCreated, S::DescribeRenameInsertCreated, &[], |a, _| a.rename_insert(Insert::Created)).within(Ctx::Rename),
    Action::new("rename.insert_modified", G::Rename, S::ActionsRenameInsertModified, S::DescribeRenameInsertModified, &[], |a, _| a.rename_insert(Insert::Modified)).within(Ctx::Rename),
    Action::held("rename.previous", G::Rename, S::ActionsRenamePrevious, S::DescribeRenamePrevious, &[shift(Key::Tab)], Ctx::Rename),
    Action::held("rename.next", G::Rename, S::ActionsRenameNext, S::DescribeRenameNext, &[key(Key::Tab)], Ctx::Rename),
    Action::held("text.left", G::Text, S::ActionsTextLeft, S::DescribeTextLeft, &[key(Key::Left)], Ctx::Field),
    Action::held("text.extend_left", G::Text, S::ActionsTextExtendLeft, S::DescribeTextExtendLeft, &[shift(Key::Left)], Ctx::Field),
    Action::held("text.right", G::Text, S::ActionsTextRight, S::DescribeTextRight, &[key(Key::Right)], Ctx::Field),
    Action::held("text.extend_right", G::Text, S::ActionsTextExtendRight, S::DescribeTextExtendRight, &[shift(Key::Right)], Ctx::Field),
    Action::held("text.word_left", G::Text, S::ActionsTextWordLeft, S::DescribeTextWordLeft, &[ctrl(Key::Left)], Ctx::Field),
    Action::held("text.extend_word_left", G::Text, S::ActionsTextExtendWordLeft, S::DescribeTextExtendWordLeft, &[ctrl_shift(Key::Left)], Ctx::Field),
    Action::held("text.word_right", G::Text, S::ActionsTextWordRight, S::DescribeTextWordRight, &[ctrl(Key::Right)], Ctx::Field),
    Action::held("text.extend_word_right", G::Text, S::ActionsTextExtendWordRight, S::DescribeTextExtendWordRight, &[ctrl_shift(Key::Right)], Ctx::Field),
    Action::held("text.home", G::Text, S::ActionsTextHome, S::DescribeTextHome, &[key(Key::Home)], Ctx::Field),
    Action::held("text.extend_home", G::Text, S::ActionsTextExtendHome, S::DescribeTextExtendHome, &[shift(Key::Home)], Ctx::Field),
    Action::held("text.end", G::Text, S::ActionsTextEnd, S::DescribeTextEnd, &[key(Key::End)], Ctx::Field),
    Action::held("text.extend_end", G::Text, S::ActionsTextExtendEnd, S::DescribeTextExtendEnd, &[shift(Key::End)], Ctx::Field),
    Action::held("text.delete_left", G::Text, S::ActionsTextDeleteLeft, S::DescribeTextDeleteLeft, &[key(Key::Backspace), shift(Key::Backspace)], Ctx::Field),
    Action::held("text.delete_word_left", G::Text, S::ActionsTextDeleteWordLeft, S::DescribeTextDeleteWordLeft, &[ctrl(Key::Backspace)], Ctx::Field),
    Action::held("text.delete_right", G::Text, S::ActionsTextDeleteRight, S::DescribeTextDeleteRight, &[key(Key::Delete)], Ctx::Field),
    Action::held("text.delete_word_right", G::Text, S::ActionsTextDeleteWordRight, S::DescribeTextDeleteWordRight, &[ctrl(Key::Delete)], Ctx::Field),
    Action::held("text.select_all", G::Text, S::ActionsTextSelectAll, S::DescribeTextSelectAll, &[ctrl(c('A'))], Ctx::Field),
    Action::held("text.copy", G::Text, S::ActionsTextCopy, S::DescribeTextCopy, &[ctrl(c('C'))], Ctx::Field),
    Action::held("text.cut", G::Text, S::ActionsTextCut, S::DescribeTextCut, &[ctrl(c('X'))], Ctx::Field),
    Action::held("text.paste", G::Text, S::ActionsTextPaste, S::DescribeTextPaste, &[ctrl(c('V')), shift(Key::Insert)], Ctx::Field),
    Action::new("text.uppercase", G::Text, S::ActionsTextUppercase, S::DescribeTextUppercase, &[], |a, _| a.change_case(true)).within(Ctx::Field),
    Action::new("text.lowercase", G::Text, S::ActionsTextLowercase, S::DescribeTextLowercase, &[], |a, _| a.change_case(false)).within(Ctx::Field),
    Action::held("text.autocomplete", G::Text, S::ActionsTextAutocomplete, S::DescribeTextAutocomplete, &[key(Key::Tab)], Ctx::Field),
    Action::held("text.undo", G::Text, S::ActionsTextUndo, S::DescribeTextUndo, &[ctrl(c('Z'))], Ctx::Field),
    Action::held("text.redo", G::Text, S::ActionsTextRedo, S::DescribeTextRedo, &[ctrl_shift(c('Z'))], Ctx::Field),
    Action::new("list.up", G::Scroll, S::ActionsListUp, S::DescribeListUp, &[key(Key::Up)], |a, now| a.list_key(Key::Up, Mods::default(), now)),
    Action::new("list.down", G::Scroll, S::ActionsListDown, S::DescribeListDown, &[key(Key::Down)], |a, now| a.list_key(Key::Down, Mods::default(), now)),
    Action::new("list.left", G::Scroll, S::ActionsListLeft, S::DescribeListLeft, &[key(Key::Left)], |a, now| a.list_key(Key::Left, Mods::default(), now)),
    Action::new("list.right", G::Scroll, S::ActionsListRight, S::DescribeListRight, &[key(Key::Right)], |a, now| a.list_key(Key::Right, Mods::default(), now)),
    Action::new("list.first", G::Scroll, S::ActionsListFirst, S::DescribeListFirst, &[key(Key::Home)], |a, now| a.list_key(Key::Home, Mods::default(), now)),
    Action::new("list.last", G::Scroll, S::ActionsListLast, S::DescribeListLast, &[key(Key::End)], |a, now| a.list_key(Key::End, Mods::default(), now)),
    Action::new("list.page_up", G::Scroll, S::ActionsListPageUp, S::DescribeListPageUp, &[key(Key::PageUp), ctrl(c('U'))], |a, now| a.list_key(Key::PageUp, Mods::default(), now)),
    Action::new("list.page_down", G::Scroll, S::ActionsListPageDown, S::DescribeListPageDown, &[key(Key::PageDown), ctrl(c('D'))], |a, now| a.list_key(Key::PageDown, Mods::default(), now)),
    Action::new("list.center", G::Scroll, S::ActionsListCenter, S::DescribeListCenter, &[], |a, _| a.reveal_focus(true)),
    Action::new("list.cursor_up", G::Scroll, S::ActionsListCursorUp, S::DescribeListCursorUp, &[ctrl(Key::Up), ctrl(c('K'))], |a, now| a.list_key(Key::Up, CURSOR, now)),
    Action::new("list.cursor_down", G::Scroll, S::ActionsListCursorDown, S::DescribeListCursorDown, &[ctrl(Key::Down), ctrl(c('J'))], |a, now| a.list_key(Key::Down, CURSOR, now)),
    Action::new("list.cursor_left", G::Scroll, S::ActionsListCursorLeft, S::DescribeListCursorLeft, &[ctrl(Key::Left)], |a, now| a.list_key(Key::Left, CURSOR, now)),
    Action::new("list.cursor_right", G::Scroll, S::ActionsListCursorRight, S::DescribeListCursorRight, &[ctrl(Key::Right)], |a, now| a.list_key(Key::Right, CURSOR, now)),
    Action::new("list.extend_up", G::Scroll, S::ActionsListExtendUp, S::DescribeListExtendUp, &[shift(Key::Up)], |a, now| a.list_key(Key::Up, EXTEND, now)),
    Action::new("list.extend_down", G::Scroll, S::ActionsListExtendDown, S::DescribeListExtendDown, &[shift(Key::Down)], |a, now| a.list_key(Key::Down, EXTEND, now)),
    Action::new("list.extend_left", G::Scroll, S::ActionsListExtendLeft, S::DescribeListExtendLeft, &[shift(Key::Left)], |a, now| a.list_key(Key::Left, EXTEND, now)),
    Action::new("list.extend_right", G::Scroll, S::ActionsListExtendRight, S::DescribeListExtendRight, &[shift(Key::Right)], |a, now| a.list_key(Key::Right, EXTEND, now)),
    Action::new("list.extend_first", G::Scroll, S::ActionsListExtendFirst, S::DescribeListExtendFirst, &[shift(Key::Home)], |a, now| a.list_key(Key::Home, EXTEND, now)),
    Action::new("list.extend_last", G::Scroll, S::ActionsListExtendLast, S::DescribeListExtendLast, &[shift(Key::End)], |a, now| a.list_key(Key::End, EXTEND, now)),
    Action::new("list.extend_page_up", G::Scroll, S::ActionsListExtendPageUp, S::DescribeListExtendPageUp, &[ctrl_shift(c('U')), shift(Key::PageUp)], |a, now| a.list_key(Key::PageUp, EXTEND, now)),
    Action::new("list.extend_page_down", G::Scroll, S::ActionsListExtendPageDown, S::DescribeListExtendPageDown, &[ctrl_shift(c('D')), shift(Key::PageDown)], |a, now| a.list_key(Key::PageDown, EXTEND, now)),
    Action::new("select.all", G::Scroll, S::ActionsSelectAll, S::DescribeSelectAll, &[ctrl(c('A'))], |a, _| a.select_all()),
    Action::new("search.open", G::Flit, S::ActionsSearchOpen, S::DescribeSearchOpen, &[ctrl_alt(c('F'))], |a, _| a.open_search()),
    Action::new("search.group_folder", G::Flit, S::ActionsSearchGroupFolder, S::DescribeSearchGroupFolder, &[], |a, _| a.toggle_search_groups()),
    Action::new("search.clear_history", G::Flit, S::ActionsSearchClearHistory, S::DescribeSearchClearHistory, &[], |a, _| a.clear_search_history()),
    Action::new("search.reveal", G::Flit, S::ActionsSearchReveal, S::DescribeSearchReveal, &[], |a, now| a.reveal_search_focus(now)),
    Action::new("nav.home", G::Flit, S::ActionsNavHome, S::DescribeNavHome, &[], |a, now| a.go_known(KnownFolder::Home, now)),
    Action::new("nav.home_page", G::Flit, S::ActionsNavHomePage, S::DescribeNavHomePage, &[], |a, now| a.go_home_page(false, now)),
    Action::new("nav.computer", G::Flit, S::ActionsNavComputer, S::DescribeNavComputer, &[], |a, now| a.navigate(Location::computer(), now)),
    Action::new("nav.recycle_bin", G::Flit, S::ActionsNavRecycleBin, S::DescribeNavRecycleBin, &[], |a, now| a.navigate(Location::recycle_bin(), now)),
    Action::new("nav.network", G::Flit, S::ActionsNavNetwork, S::DescribeNavNetwork, &[], |a, now| a.navigate(Location::Local(crate::FsPath::network()), now)),
    Action::new("drive.map", G::Flit, S::ActionsDriveMap, S::DescribeDriveMap, &[], |a, _| a.map_drive()),
    Action::new("drive.disconnect", G::Flit, S::ActionsDriveDisconnect, S::DescribeDriveDisconnect, &[], |a, _| a.launch(crate::provider::Launch::Disconnect(None))),
    Action::new("drive.connect_server", G::Flit, S::ActionsDriveConnectServer, S::DescribeDriveConnectServer, &[], |a, _| a.ask_server()),
    Action::new("tab.move_to_window", G::Flit, S::ActionsTabMoveToWindow, S::DescribeTabMoveToWindow, &[], |a, _| a.move_tab_to_window()),
    Action::new("window.compact", G::Flit, S::ActionsWindowCompact, S::DescribeWindowCompact, &[key(Key::F(12))], |a, _| a.effects.push(Effect::Window(WindowCmd::Compact(None)))),
    Action::new("window.compact_on", G::Flit, S::ActionsWindowCompactOn, S::DescribeWindowCompactOn, &[ctrl_alt(Key::Up)], |a, _| a.effects.push(Effect::Window(WindowCmd::Compact(Some(true))))),
    Action::new("window.compact_off", G::Flit, S::ActionsWindowCompactOff, S::DescribeWindowCompactOff, &[ctrl_alt(Key::Down)], |a, _| a.effects.push(Effect::Window(WindowCmd::Compact(Some(false))))),
    Action::new("view.dotfiles", G::Flit, S::ActionsViewDotfiles, S::DescribeViewDotfiles, &[], |a, _| a.toggle_pref(|p| &mut p.show_dotfiles)),
    Action::new("view.sort.tag", G::Flit, S::ActionsViewSortTag, S::DescribeViewSortTag, &[], |a, _| a.sort_by(SortField::Tag)),
    Action::new("view.sort.reverse", G::Flit, S::ActionsViewSortReverse, S::DescribeViewSortReverse, &[], |a, _| a.set_descending(None)),
    Action::new("view.sort.ascending", G::Flit, S::ActionsViewSortAscending, S::DescribeViewSortAscending, &[], |a, _| a.set_descending(Some(false))),
    Action::new("view.sort.descending", G::Flit, S::ActionsViewSortDescending, S::DescribeViewSortDescending, &[], |a, _| a.set_descending(Some(true))),
    Action::new("view.sort.folders_first", G::Flit, S::ActionsViewSortFoldersFirst, S::DescribeViewSortFoldersFirst, &[], |a, _| a.set_folders(Folders::First)),
    Action::new("view.sort.files_first", G::Flit, S::ActionsViewSortFilesFirst, S::DescribeViewSortFilesFirst, &[], |a, _| a.set_folders(Folders::Last)),
    Action::new("view.sort.mixed", G::Flit, S::ActionsViewSortMixed, S::DescribeViewSortMixed, &[], |a, _| a.set_folders(Folders::Mixed)),
    Action::new("view.group.tag", G::Flit, S::ActionsViewGroupTag, S::DescribeViewGroupTag, &[], |a, _| a.group_by(GroupField::Tag)),
    Action::new("view.layout.cards", G::Flit, S::ActionsViewLayoutCards, S::DescribeViewLayoutCards, &[], |a, _| a.set_layout(LayoutMode::Cards)),
    Action::new("view.layout.columns", G::Flit, S::ActionsViewLayoutColumns, S::DescribeViewLayoutColumns, &[], |a, _| a.set_layout(LayoutMode::Columns)),
    Action::new("view.layout.grid", G::Flit, S::ActionsViewLayoutGrid, S::DescribeViewLayoutGrid, &[], |a, _| a.set_layout(LayoutMode::Grid)),
    Action::new("view.layout.adaptive", G::Flit, S::ActionsViewLayoutAdaptive, S::DescribeViewLayoutAdaptive, &[], |a, _| a.set_layout(LayoutMode::Adaptive)),
    Action::new("view.thumbnails", G::Flit, S::ActionsViewThumbnails, S::DescribeViewThumbnails, &[], |a, _| a.toggle_pref(|p| &mut p.thumbnails)),
    Action::new("view.thumbnails.clear", G::Flit, S::ActionsViewThumbnailsClear, S::DescribeViewThumbnailsClear, &[], |a, _| a.clear_thumbnails()),
    Action::new("view.columns.autofit", G::Flit, S::ActionsViewColumnsAutofit, S::DescribeViewColumnsAutofit, &[], |a, _| a.toggle_auto_fit()),
    Action::new("view.column.modified", G::Flit, S::ActionsViewColumnModified, S::DescribeViewColumnModified, &[], |a, _| a.toggle_column(Column::Modified)),
    Action::new("view.column.created", G::Flit, S::ActionsViewColumnCreated, S::DescribeViewColumnCreated, &[], |a, _| a.toggle_column(Column::Created)),
    Action::new("view.column.type", G::Flit, S::ActionsViewColumnType, S::DescribeViewColumnType, &[], |a, _| a.toggle_column(Column::Type)),
    Action::new("view.column.size", G::Flit, S::ActionsViewColumnSize, S::DescribeViewColumnSize, &[], |a, _| a.toggle_column(Column::Size)),
    Action::new("view.column.origin", G::Flit, S::ActionsViewColumnOrigin, S::DescribeViewColumnOrigin, &[], |a, _| a.toggle_column(Column::Origin)),
    Action::new("view.column.deleted", G::Flit, S::ActionsViewColumnDeleted, S::DescribeViewColumnDeleted, &[], |a, _| a.toggle_column(Column::Deleted)),
    Action::new("view.column.tag", G::Flit, S::ActionsViewColumnTag, S::DescribeViewColumnTag, &[], |a, _| a.toggle_column(Column::Tag)),
    Action::new("view.column.git_status", G::Flit, S::ActionsViewColumnGitStatus, S::DescribeViewColumnGitStatus, &[], |a, _| a.toggle_column(Column::GitStatus)),
    Action::new("view.column.git_message", G::Flit, S::ActionsViewColumnGitMessage, S::DescribeViewColumnGitMessage, &[], |a, _| a.toggle_column(Column::GitMessage)),
    Action::new("view.column.git_author", G::Flit, S::ActionsViewColumnGitAuthor, S::DescribeViewColumnGitAuthor, &[], |a, _| a.toggle_column(Column::GitAuthor)),
    Action::new("view.column.git_date", G::Flit, S::ActionsViewColumnGitDate, S::DescribeViewColumnGitDate, &[], |a, _| a.toggle_column(Column::GitDate)),
    Action::new("git.fetch", G::Flit, S::ActionsGitFetch, S::DescribeGitFetch, &[], |a, _| a.git_run(GitOp::Fetch)),
    Action::new("git.pull", G::Flit, S::ActionsGitPull, S::DescribeGitPull, &[], |a, _| a.git_run(GitOp::Pull)),
    Action::new("git.push", G::Flit, S::ActionsGitPush, S::DescribeGitPush, &[], |a, _| a.git_run(GitOp::Push)),
    Action::new("git.init", G::Flit, S::ActionsGitInit, S::DescribeGitInit, &[], |a, _| a.git_run(GitOp::Init)),
    Action::new("git.clone", G::Flit, S::ActionsGitClone, S::DescribeGitClone, &[], |a, _| a.ask_clone()),
    Action::new("git.branches", G::Flit, S::ActionsGitBranches, S::DescribeGitBranches, &[], |a, _| a.git_branches_menu()),
    Action::new("git.open_remote", G::Flit, S::ActionsGitOpenRemote, S::DescribeGitOpenRemote, &[], |a, _| a.git_run(GitOp::OpenRemote)),
    Action::new("view.status_bar", G::Flit, S::ActionsViewStatusBar, S::DescribeViewStatusBar, &[], |a, _| a.toggle_pref(|p| &mut p.status_bar)),
    Action::new("theme.toggle", G::Flit, S::ActionsThemeToggle, S::DescribeThemeToggle, &[ctrl_alt(c('T'))], |a, _| a.toggle_theme()),
    Action::new("theme.light", G::Flit, S::ActionsThemeLight, S::DescribeThemeLight, &[], |a, _| a.set_theme_mode(crate::theme::Mode::Light)),
    Action::new("theme.dark", G::Flit, S::ActionsThemeDark, S::DescribeThemeDark, &[], |a, _| a.set_theme_mode(crate::theme::Mode::Dark)),
    Action::new("theme.system", G::Flit, S::ActionsThemeSystem, S::DescribeThemeSystem, &[], |a, _| a.set_theme_mode(crate::theme::Mode::System)),
    Action::new("view.streams", G::Flit, S::ActionsViewStreams, S::DescribeViewStreams, &[], |a, now| a.toggle_streams(now)),
    Action::new("view.checkboxes", G::Flit, S::ActionsViewCheckboxes, S::DescribeViewCheckboxes, &[], |a, _| a.toggle_pref(|p| &mut p.checkboxes)),
    Action::new("view.hover_select", G::Flit, S::ActionsViewHoverSelect, S::DescribeViewHoverSelect, &[], |a, _| a.toggle_pref(|p| &mut p.hover_select)),
    Action::new("view.auto_sizes", G::Flit, S::ActionsViewAutoSizes, S::DescribeViewAutoSizes, &[], |a, _| a.toggle_pref(|p| &mut p.auto_sizes)),
    Action::new("view.same_for_all", G::Flit, S::ActionsViewSameForAll, S::DescribeViewSameForAll, &[], |a, _| a.toggle_pref(|p| &mut p.same_for_all)),
    Action::new("view.set_default", G::Flit, S::ActionsViewSetDefault, S::DescribeViewSetDefault, &[], |a, _| a.set_default_view()),
    Action::new("quick_access.focus", G::Flit, S::ActionsQuickAccessFocus, S::DescribeQuickAccessFocus, &[key(Key::F(6))], |a, _| a.cycle_sidebar_focus()),
    Action::new("quick_access.compact", G::Flit, S::ActionsQuickAccessCompact, S::DescribeQuickAccessCompact, &[], |a, _| a.toggle_compact()),
    Action::new("quick_access.sections", G::Flit, S::ActionsQuickAccessSections, S::DescribeQuickAccessSections, &[], |a, _| a.show_all_sections()),
    Action::new("toolbar.toggle", G::Flit, S::ActionsToolbarToggle, S::DescribeToolbarToggle, &[ctrl_shift(c('B'))], |a, _| a.toggle_toolbar()),
    Action::new("toolbar.customize", G::Flit, S::ActionsToolbarCustomize, S::DescribeToolbarCustomize, &[], |a, _| a.customize_toolbar()),
    Action::new("toolbar.reset", G::Flit, S::ActionsToolbarReset, S::DescribeToolbarReset, &[], |a, _| a.reset_toolbar()),
    Action::new("home.quick_access", G::Flit, S::ActionsHomeQuickAccess, S::DescribeHomeQuickAccess, &[], |a, _| a.toggle_widget(Widget::QuickAccess)),
    Action::new("home.drives", G::Flit, S::ActionsHomeDrives, S::DescribeHomeDrives, &[], |a, _| a.toggle_widget(Widget::Drives)),
    Action::new("home.tags", G::Flit, S::ActionsHomeTags, S::DescribeHomeTags, &[], |a, _| a.toggle_widget(Widget::Tags)),
    Action::new("home.recent", G::Flit, S::ActionsHomeRecent, S::DescribeHomeRecent, &[], |a, _| a.toggle_widget(Widget::Recent)),
    Action::new("libraries.new", G::Flit, S::ActionsLibrariesNew, S::DescribeLibrariesNew, &[], |a, _| a.ask_new_library()),
    Action::new("libraries.restore", G::Flit, S::ActionsLibrariesRestore, S::DescribeLibrariesRestore, &[], |a, _| a.effects.push(Effect::Library(LibraryOp::RestoreDefaults))),
    Action::new("library.locations", G::Flit, S::ActionsLibraryLocations, S::DescribeLibraryLocations, &[], |a, _| {
        if let Some(lib) = a.current_library() {
            a.library_dialog(lib);
        }
    }),
    Action::new("select.none", G::Flit, S::ActionsSelectNone, S::DescribeSelectNone, &[key(Key::Escape)], |a, _| a.clear_selection()),
    Action::new("select.invert", G::Flit, S::ActionsSelectInvert, S::DescribeSelectInvert, &[], |a, _| a.invert_selection()),
    Action::new("item.pin", G::Flit, S::ActionsItemPin, S::DescribeItemPin, &[], |a, _| a.pin_selection()),
    Action::new("tags.new", G::Flit, S::ActionsTagsNew, S::DescribeTagsNew, &[], |a, _| a.new_tag(true)),
    Action::new("tags.remove", G::Flit, S::ActionsTagsRemove, S::DescribeTagsRemove, &[], |a, _| a.remove_tags()),
    Action::new("tags.manage", G::Flit, S::ActionsTagsManage, S::DescribeTagsManage, &[], |a, _| a.open_tag_pick(TagPick::Manage, "")),
    Action::new("tags.open_all", G::Flit, S::ActionsTagsOpenAll, S::DescribeTagsOpenAll, &[], |a, _| a.open_tag_pick(TagPick::OpenAll, "")),
    Action::new("tags.settings", G::Flit, S::ActionsTagsSettings, S::DescribeTagsSettings, &[], |a, _| a.open_settings(Some(Page::Tags))),
    Action::new("settings.export", G::Flit, S::ActionsSettingsExport, S::DescribeSettingsExport, &[], |a, _| a.ask_export_settings()),
    Action::new("settings.import", G::Flit, S::ActionsSettingsImport, S::DescribeSettingsImport, &[], |a, _| a.ask_import_settings()),
    Action::new("settings.reset", G::Flit, S::ActionsSettingsReset, S::DescribeSettingsReset, &[], |a, _| a.ask_reset_settings()),
    Action::new("help.docs", G::Flit, S::ActionsHelpDocs, S::DescribeHelpDocs, &[key(Key::F(1))], |a, _| a.open_link("help.docs")),
    Action::new("help.bug", G::Flit, S::ActionsHelpBug, S::DescribeHelpBug, &[], |a, _| a.open_link("help.bug")),
    Action::new("help.feature", G::Flit, S::ActionsHelpFeature, S::DescribeHelpFeature, &[], |a, _| a.open_link("help.feature")),
    Action::new("help.repository", G::Flit, S::ActionsHelpRepository, S::DescribeHelpRepository, &[], |a, _| a.open_link("help.repository")),
    Action::new("help.privacy", G::Flit, S::ActionsHelpPrivacy, S::DescribeHelpPrivacy, &[], |a, _| a.open_link("help.privacy")),
    Action::new("logs.file", G::Flit, S::ActionsLogsFile, S::DescribeLogsFile, &[ctrl(c('.'))], |a, _| a.open_log_file()),
    Action::new("logs.folder", G::Flit, S::ActionsLogsFolder, S::DescribeLogsFolder, &[ctrl_shift(c('.'))], |a, now| a.open_log_folder(now)),
    Action::new("item.copy_quoted_path", G::Flit, S::ActionsItemCopyQuotedPath, S::DescribeItemCopyQuotedPath, &[ctrl_alt(c('C'))], |a, _| a.copy_paths(Quote::Always)),
    Action::new("app.terminal_admin", G::Flit, S::ActionsAppTerminalAdmin, S::DescribeAppTerminalAdmin, &[ctrl_shift(c('`'))], |a, _| a.open_terminal(true)),
    Action::new("file.paste_into", G::Flit, S::ActionsFilePasteInto, S::DescribeFilePasteInto, &[ctrl_shift(c('V'))], |a, _| a.paste_into_selection()),
    Action::new("file.share", G::Flit, S::ActionsFileShare, S::DescribeFileShare, &[], |a, _| a.share_selection()),
    Action::new("file.new_item", G::Flit, S::ActionsFileNewItem, S::DescribeFileNewItem, &[ctrl_shift(c('I'))], |a, _| a.new_item_dialog()),
    Action::new("file.new_folder_with_selection", G::Flit, S::ActionsFileNewFolderWithSelection, S::DescribeFileNewFolderWithSelection, &[], |a, _| a.new_folder_with_selection()),
    Action::new("file.new_stream", G::Flit, S::ActionsFileNewStream, S::DescribeFileNewStream, &[], |a, _| a.new_stream()),
    Action::new("file.folder_icon", G::Flit, S::ActionsFileFolderIcon, S::DescribeFileFolderIcon, &[], |a, _| a.ask_folder_icon()),
    Action::new("file.folder_icon_reset", G::Flit, S::ActionsFileFolderIconReset, S::DescribeFileFolderIconReset, &[], |a, _| a.reset_folder_icon()),
    Action::new("file.shortcut", G::Flit, S::ActionsFileShortcut, S::DescribeFileShortcut, &[], |a, _| a.create_shortcuts()),
    Action::new("file.flatten", G::Flit, S::ActionsFileFlatten, S::DescribeFileFlatten, &[], |a, _| a.flatten_selection()),
    Action::new("file.install_fonts", G::Flit, S::OpsExtraInstallFonts, S::DescribeFileInstallFonts, &[], |a, _| a.install_fonts_selection()),
    Action::new("file.lock_screen", G::Flit, S::OpsExtraLockScreen, S::DescribeFileLockScreen, &[], |a, _| a.lock_screen_selection()),
    Action::new("file.set_backdrop", G::Flit, S::OpsExtraSetBackdrop, S::DescribeFileSetBackdrop, &[], |a, _| a.backdrop_selection()),
    Action::new("view.clear_backdrop", G::Flit, S::OpsExtraClearBackdrop, S::DescribeViewClearBackdrop, &[], |a, _| a.set_backdrop(String::new())),
    Action::new("archive.compress", G::Flit, S::ActionsArchiveCompress, S::DescribeArchiveCompress, &[], |a, _| a.compress_dialog()),
    Action::new("archive.zip", G::Flit, S::ActionsArchiveZip, S::DescribeArchiveZip, &[], |a, _| a.compress_quick(Format::Zip)),
    Action::new("archive.7z", G::Flit, S::ActionsArchive7z, S::DescribeArchive7z, &[], |a, _| a.compress_quick(Format::SevenZ)),
    Action::new("archive.extract", G::Flit, S::ActionsArchiveExtract, S::DescribeArchiveExtract, &[alt(c('E'))], |a, _| a.extract_dialog()),
    Action::new("archive.extract_here", G::Flit, S::ActionsArchiveExtractHere, S::DescribeArchiveExtractHere, &[], |a, _| a.extract_with(Place::Here)),
    Action::new("archive.extract_folder", G::Flit, S::ActionsArchiveExtractFolder, S::DescribeArchiveExtractFolder, &[], |a, _| a.extract_with(Place::OwnFolder)),
    Action::new("archive.smart", G::Flit, S::ActionsArchiveSmart, S::DescribeArchiveSmart, &[], |a, _| a.extract_with(Place::Smart)),
    Action::new("archive.open", G::Flit, S::ActionsArchiveOpen, S::DescribeArchiveOpen, &[], |a, now| a.open_as_folder(now)),
    Action::new("archive.encoding", G::Flit, S::ActionsArchiveEncoding, S::DescribeArchiveEncoding, &[], |a, _| a.pick_encoding()),
    Action::new("file.hashes", G::Flit, S::ActionsFileHashes, S::DescribeFileHashes, &[], |a, _| a.hashes_dialog()),
    Action::new("ops.show", G::Flit, S::ActionsOpsShow, S::DescribeOpsShow, &[], |a, _| a.toggle_jobs()),
    Action::new("ops.clear", G::Flit, S::ActionsOpsClear, S::DescribeOpsClear, &[], |a, _| a.clear_completed()),
    Action::new("edit.undo", G::Flit, S::ActionsEditUndo, S::DescribeEditUndo, &[ctrl(c('Z'))], |a, _| a.undo()),
    Action::new("edit.redo", G::Flit, S::ActionsEditRedo, S::DescribeEditRedo, &[ctrl(c('Y'))], |a, _| a.redo()),
    Action::new("item.properties", G::Flit, S::ActionsItemProperties, S::DescribeItemProperties, &[alt_shift(Key::Enter)], |a, _| a.properties_dialog()),
    Action::new("view.pane.preview", G::Flit, S::ActionsViewPanePreview, S::DescribeViewPanePreview, &[alt(c('P'))], |a, _| a.show_preview_tab(false)),
    Action::new("view.pane.details", G::Flit, S::ActionsViewPaneDetails, S::DescribeViewPaneDetails, &[alt_shift(c('P'))], |a, _| a.show_preview_tab(true)),
    Action::new("item.quick_view", G::Flit, S::ActionsItemQuickView, S::DescribeItemQuickView, &[], |a, _| a.quick_view()),
    Action::new("preview.play", G::Flit, S::ActionsPreviewPlay, S::DescribePreviewPlay, &[], |a, _| a.play_preview()),
];

const CURSOR: Mods = Mods { ctrl: true, shift: false, alt: false };
const EXTEND: Mods = Mods { ctrl: false, shift: true, alt: false };

pub const RENAMED: &[(&str, &str)] = &[
    ("sidebar.toggle", "quick_access.toggle"),
    ("sidebar.focus", "quick_access.focus"),
    ("sidebar.compact", "quick_access.compact"),
    ("sidebar.sections", "quick_access.sections"),
    ("sidebar.pin", "folder.bookmark"),
    ("window.new", "tab.duplicate_window"),
    ("view.pane", "inspector.toggle"),
    ("view.sizes", "folder.refresh_stats"),
    ("settings.open", "options.open"),
    ("settings.file", "config.open"),
    ("item.shell_properties", "item.inspect"),
    ("view.size.larger", "view.zoom_in"),
    ("view.size.smaller", "view.zoom_out"),
];

pub fn renamed(id: &str) -> &str {
    RENAMED.iter().find(|(old, _)| *old == id).map_or(id, |(_, new)| new)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Quote {
    Never,
    Needed,
    Always,
}

pub fn find(key: Key, mods: Mods) -> Option<&'static Action> {
    let pressed = Chord { key, mods };
    ACTIONS.iter().find(|a| a.rebindable() && a.keys.contains(&pressed))
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
            Key::PageUp => "PageUp",
            Key::PageDown => "PageDown",
            Key::Home => "Home",
            Key::End => "End",
            Key::Enter => "Enter",
            Key::Escape => "Escape",
            Key::Backspace => "Backspace",
            Key::Delete => "Delete",
            Key::Insert => "Insert",
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
    a.shown_keys(a.keys).join(", ")
}

pub fn index(id: &str) -> Option<usize> {
    ACTIONS.iter().position(|a| a.id == id)
}

fn smart_quote(text: &str) -> String {
    match text.chars().any(|c| c.is_whitespace() || "&()[]{}^=;!'+,`~".contains(c)) {
        true => format!("\"{text}\""),
        false => text.to_owned(),
    }
}

impl App {
    pub(super) fn run_bound(&mut self, key: Key, mods: Mods, now: Instant) -> bool {
        if key == Key::Space && mods == Mods::default() && self.tab().typing(now) {
            return false;
        }
        let Some(i) = self.keymap.owner(Chord { key, mods }).filter(|&i| self.supports(ACTIONS[i].id)) else { return false };
        if let Some(run) = ACTIONS[i].run {
            run(self, now);
        }
        true
    }

    pub fn run_action(&mut self, id: &str, now: Instant) -> bool {
        let Some(run) = ACTIONS.iter().find(|a| a.id == id && self.supports(id)).and_then(|a| a.run) else { return false };
        self.dirty = true;
        run(self, now);
        true
    }

    pub fn supports(&self, id: &str) -> bool {
        self.caps.supports(id) && (self.builtin_archives || !crate::caps::is_builtin_archive_action(id))
    }

    pub fn available(&self, id: &str) -> bool {
        let Some(a) = ACTIONS.iter().find(|a| a.id == id).filter(|a| self.supports(id) && a.run.is_some()) else { return false };
        match a.ctx {
            Ctx::Rename => return self.is_renaming(),
            Ctx::Field => return self.is_renaming() || self.inspector_field_focused() || matches!(self.focus, super::Focus::Filter),
            Ctx::Main => {}
            _ => return false,
        }
        let kind = self.current_location().path().kind();
        let (home, bin, library) = (kind == PathKind::Home, kind == PathKind::RecycleBin, self.current_library().is_some());
        let no_files = home || bin || kind == PathKind::Computer;
        match id.split('.').next().unwrap_or_default() {
            _ if id.ends_with("origin") || id.ends_with(".deleted") => bin,
            _ if id == "home.clear_recent" => true,
            _ if id == "layout.load" => !self.opts.layouts.is_empty(),
            _ if id.starts_with("tab.select_") => id == "tab.select_last" || id.trim_start_matches("tab.select_").parse::<usize>().is_ok_and(|n| n <= self.tabs.len()),
            "bin" => bin,
            "home" => home,
            "library" => library,
            "panel" => self.panel_available(id),
            "inspector" => self.inspector_available(id),
            "search" if id == "search.group_folder" => self.search.active,
            "search" if id == "search.folder" => !no_files,
            "search" if id == "search.clear_history" => !self.search_history().is_empty(),
            "file" | "archive" => !no_files,
            "view" | "select" | "item" | "list" => !home,
            "app" => !(no_files || library),
            _ => !(id == "nav.up" && self.current_location().parent().is_none()),
        }
    }

    pub(super) fn remember_closed(&mut self, loc: Location) {
        if self.nav.closed.len() == MAX_CLOSED {
            self.nav.closed.remove(0);
        }
        self.nav.closed.push(loc);
        self.note_closed(self.last_frame.unwrap_or_else(Instant::now));
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

    fn open_relative(&mut self, target: OpenTarget, now: Instant) {
        if self.search.active {
            return self.reveal_search_focus(now);
        }
        let t = self.tab();
        let Some(i) = t.view.as_ref().and_then(|v| v.focus.filter(|&f| v.is_visible(f)).or_else(|| v.selected().next())) else { return };
        let item = t.entry_location(i);
        let Some(folder) = item.parent() else { return };
        self.dispatch(vec![Route { folder, select: Some(item.file_name().to_vec()) }], target, now);
    }

    fn open_containing(&mut self, new_tab: bool) {
        let t = self.tab();
        let Some(i) = t.view.as_ref().and_then(|v| v.focus.filter(|&f| v.is_visible(f) && v.is_selected(f)).or_else(|| v.selected().next())) else {
            return self.status(S::ActionsStatusSelectItem.t());
        };
        let item = t.entry_location(i);
        match item.to_windows().filter(|_| !t.view_loc.is_recycle_bin() && self.archive_view().is_none()) {
            Some(path) => self.launch(crate::provider::Launch::Reveal { item: path, new_tab }),
            None => self.status(S::ActionsStatusNoRealPath.t()),
        }
    }

    pub(super) fn reveal_path(&mut self, item: Location, new_tab: bool, now: Instant) {
        let route = match item.parent() {
            Some(folder) => Route { select: Some(item.file_name().to_vec()), folder },
            None => Route { folder: item, select: None },
        };
        self.dispatch(vec![route], if new_tab { OpenTarget::NewTab } else { OpenTarget::CurrentTab }, now);
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

    fn toggle_cursor_selection(&mut self) {
        if let Some(v) = &mut self.tab_mut().view {
            if let Some(f) = v.focus {
                let on = !v.is_selected(f);
                v.set_selected(f, on);
            }
        }
    }

    fn list_key(&mut self, key: Key, m: Mods, now: Instant) {
        let page = self.page_rows();
        let d = match key {
            Key::PageUp => -page,
            Key::PageDown => page,
            Key::Home => isize::MIN / 2,
            Key::End => isize::MAX / 2,
            _ => {
                self.arrow_key(key, m, now);
                return;
            }
        };
        self.tab_mut().move_focus(d, m);
        self.reveal_focus(false);
    }

    fn copy_paths(&mut self, quote: Quote) {
        let t = self.tab();
        let text = |l: &Location| match quote {
            Quote::Never => l.display().into_owned(),
            Quote::Needed => smart_quote(&l.display()),
            Quote::Always => format!("\"{}\"", l.display()),
        };
        let sel: Vec<u32> = t.view.as_ref().map(|v| v.selected().collect()).unwrap_or_default();
        let text = if sel.is_empty() { text(t.location()) } else { sel.iter().map(|&i| text(&t.entry_location(i))).collect::<Vec<_>>().join("\r\n") };
        self.effects.push(Effect::SetClipboard(text));
    }

    fn cycle_folders(&mut self) {
        let next = match self.tab().view_spec.sort.folders {
            Folders::First => Folders::Last,
            Folders::Last => Folders::Mixed,
            Folders::Mixed => Folders::First,
        };
        self.set_folders(next);
    }

    fn fold_groups(&mut self, collapsed: bool) {
        if let Some(v) = &mut self.tab_mut().view {
            v.fold_all(collapsed);
        }
        self.layout();
        self.dirty = true;
    }

    fn shift_tab(&mut self, right: bool) {
        let to = if right { self.cur + 1 } else { self.cur.wrapping_sub(1) };
        if to < self.tabs.len() {
            self.move_tab(to);
        }
    }

    fn open_explorer(&mut self) {
        let shown = self.current_location().to_windows().filter(|p| p.kind() != PathKind::Computer).map(|p| p.display().into_owned());
        match shown {
            Some(path) => {
                let refused = path.contains('"');
                let note = self.hand_to_explorer(&path);
                if refused {
                    self.status(note);
                }
            }
            None => self.launch(crate::provider::Launch::Run { program: "explorer.exe".into(), args: String::new(), dir: None }),
        }
    }

    fn inspect(&mut self) {
        match self.caps.unix_paths {
            true => self.properties_dialog(),
            false => self.properties(),
        }
    }

    fn step_font(&mut self, d: i32) {
        self.set_ui_scale((self.ui_scale() as i32 + d).max(0) as u32);
    }
}

#[cfg(test)]
#[path = "../../test/ui/actions.rs"]
mod tests;
