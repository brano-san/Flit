use super::*;
use crate::a11y::Role;
use crate::input::{Event, Key, Mods};
use crate::ui::harness::{app, frame};

#[test]
fn the_tree_names_regions_and_controls_with_their_bounds() {
    let mut a = app(&["alpha.txt", "beta.txt"]);
    let t = a.accessibility();
    let root = &t.nodes[0];
    assert_eq!(root.role, Role::Window);
    let roles: Vec<Role> = root.children.iter().map(|&k| t.nodes[k].role).collect();
    assert!(roles.starts_with(&[Role::TabList, Role::ToolBar, Role::ToolBar, Role::Tree, Role::DataGrid]), "{roles:?}");
    let back = t.by_name(action_title("nav.back")).unwrap();
    assert_eq!(back.role, Role::Button);
    assert_eq!(back.bounds, a.layout.nav[0]);
    assert!(!back.help.is_empty());
    assert!(!back.enabled);
    let tab = t.nodes.iter().find(|n| n.role == Role::Tab).unwrap();
    assert!(tab.name == a.tab().title() && tab.selected);
    let row = t.by_name("alpha.txt").unwrap();
    assert_eq!(row.role, Role::DataItem);
    assert!(row.bounds.h > 0.0 && a.layout.list.contains(row.bounds.x + 1.0, row.bounds.y + 1.0));
    assert!(row.value.contains(S::DetailsColumnSize.t()));
    let header = t.by_name(S::DetailsColumnName.t()).unwrap();
    assert_eq!(header.role, Role::HeaderItem);
}

#[test]
fn only_visible_rows_are_realized() {
    let names: Vec<String> = (0..500).map(|i| format!("f{i:03}.txt")).collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let mut a = app(&refs);
    let t = a.accessibility();
    let rows = t.nodes.iter().filter(|n| n.role == Role::DataItem).count();
    assert!(rows > 5 && rows < 60, "{rows}");
}

#[test]
fn focus_follows_the_keyboard_and_invoke_runs_the_control() {
    let mut a = app(&["alpha.txt"]);
    a.enter_focus(Focus::TabBar);
    frame(&mut a);
    let t = a.accessibility();
    assert_eq!(t.nodes[t.focus.unwrap()].role, Role::Tab);
    let new_tab = t.by_name(S::A11yButtonNewTab.t()).unwrap().id;
    a.on_event(&Event::Invoke(new_tab), Instant::now());
    assert_eq!(a.tabs.items.len(), 2);
}
fn setting_title(id: &str) -> &'static str {
    crate::ui::settings::SETTINGS.iter().find(|s| s.id == id).unwrap().title.t()
}

#[test]
fn the_settings_page_exposes_pages_rows_values_and_toggles() {
    let mut a = app(&["alpha.txt"]);
    a.open_settings(Some(crate::ui::settings::Page::Files));
    frame(&mut a);
    let t = a.accessibility();
    let d = t.by_name(S::SettingsPageLineTitle.t()).unwrap();
    assert_eq!(d.role, Role::Dialog);
    let named = |role: Role| t.nodes.iter().any(|n| n.name == crate::ui::settings::Page::Files.title() && n.role == role);
    assert!(named(Role::Group), "the Files section is a group");
    let category = t.nodes.iter().find(|n| n.name == crate::ui::settings::Page::Files.title() && n.role == Role::ListItem).expect("the categories list names Files");
    assert!(category.selected, "the category on screen is selected");
    assert_eq!(t.by_name(S::SettingsPageLineCategories.t()).unwrap().role, Role::List);
    let describe = t.by_name(S::OptionsDescriptions.t()).unwrap();
    assert!(describe.role == Role::Button && describe.selected, "descriptions are on by default");
    let hidden = t.by_name(setting_title("files.show_hidden")).unwrap();
    assert_eq!(hidden.role, Role::CheckBox);
    assert!(!hidden.selected && !hidden.help.is_empty());
    let hidden_id = hidden.id;
    a.on_event(&Event::Invoke(hidden_id), Instant::now());
    frame(&mut a);
    assert!(a.accessibility().by_name(setting_title("files.show_hidden")).unwrap().selected);
    "units".chars().for_each(|c| a.on_event(&Event::Char(c), Instant::now()));
    frame(&mut a);
    let t = a.accessibility();
    assert_eq!(t.by_name(S::SettingsPageLineSearch.t()).unwrap().value, "units");
    let choice = t.by_name(setting_title("files.size_units")).unwrap();
    assert_eq!(choice.role, Role::ComboBox);
    assert!(!choice.value.is_empty() && !choice.help.is_empty());
    let choice_id = choice.id;
    a.on_event(&Event::A11yFocus(choice_id), Instant::now());
    let t = a.accessibility();
    assert_eq!(t.nodes[t.focus.unwrap()].id, choice_id);
}

#[test]
fn each_option_of_a_checkbox_group_is_its_own_node() {
    let mut a = app(&["alpha.txt"]);
    let v = a.tabs.items[a.tabs.cur].view.as_mut().unwrap();
    let i = v.find(b"alpha.txt").unwrap();
    v.set_selected(i, true);
    v.focus = Some(i);
    a.hashes_dialog();
    a.layout();
    let t = a.accessibility();
    let group = t.nodes.iter().find(|n| n.role == Role::Group).expect("the algorithms group");
    let options: Vec<_> = group.children.iter().map(|&c| &t.nodes[c]).collect();
    let before = a.dialogs.form.as_ref().unwrap().flags(0);
    assert_eq!(options.len(), before.len());
    assert!(options.iter().all(|n| n.role == Role::CheckBox && !n.name.is_empty()));
    assert_eq!(options.iter().map(|n| n.selected).collect::<Vec<_>>(), before);
    let second = options[1].id;
    a.on_event(&Event::A11yFocus(second), Instant::now());
    assert_eq!(a.accessibility().focus.map(|k| a.accessibility().nodes[k].id), Some(second));
    a.on_event(&Event::Invoke(second), Instant::now());
    let after = a.dialogs.form.as_ref().unwrap().flags(0);
    assert_eq!(after[1], !before[1]);
    assert_eq!(after[0], before[0]);
}

#[test]
fn form_dialogs_expose_fields_with_values_and_their_buttons() {
    let mut a = app(&["alpha.txt"]);
    let v = a.tabs.items[a.tabs.cur].view.as_mut().unwrap();
    let i = v.find(b"alpha.txt").unwrap();
    v.set_selected(i, true);
    a.compress_dialog();
    a.layout();
    let t = a.accessibility();
    let form = a.dialogs.form.as_ref().unwrap();
    let d = t.by_name(&form.title).unwrap();
    assert_eq!(d.role, Role::Dialog);
    let edit = t.nodes.iter().find(|n| n.role == Role::Edit && n.value == "alpha.zip").unwrap();
    assert!(d.children.iter().any(|&c| t.nodes[c].id == edit.id));
    assert_eq!(t.nodes[t.focus.unwrap()].id, edit.id);
    assert!(t.nodes.iter().any(|n| n.role == Role::ComboBox && n.value == "ZIP"), "{:?}", t.nodes.iter().filter(|n| n.role == Role::ComboBox).map(|n| &n.value).collect::<Vec<_>>());
    let cancel = t.by_name(form.buttons[1]).unwrap();
    assert_eq!(cancel.role, Role::Button);
    let cancel = cancel.id;
    a.on_event(&Event::A11yFocus(cancel), Instant::now());
    let t = a.accessibility();
    assert_eq!(t.nodes[t.focus.unwrap()].id, cancel);
    a.on_event(&Event::Invoke(cancel), Instant::now());
    assert!(a.dialogs.form.is_none());
}

#[test]
fn set_focus_and_select_move_the_app_to_that_control() {
    let mut a = app(&["alpha.txt", "beta.txt"]);
    frame(&mut a);
    let t = a.accessibility();
    let beta = t.by_name("beta.txt").unwrap().id;
    a.on_event(&Event::A11ySelect(beta), Instant::now());
    let t = a.accessibility();
    let rows: Vec<(&str, bool)> = t.nodes.iter().filter(|n| n.role == Role::DataItem).map(|n| (n.name.as_str(), n.selected)).collect();
    assert_eq!(rows, [("alpha.txt", false), ("beta.txt", true)]);
    assert_eq!(t.nodes[t.focus.unwrap()].id, beta);
    let refresh = t.by_name(action_title("nav.up")).unwrap().id;
    a.on_event(&Event::A11yFocus(refresh), Instant::now());
    let t = a.accessibility();
    assert_eq!(t.nodes[t.focus.unwrap()].id, refresh);
}

#[test]
fn a_collapsed_section_header_keeps_the_sidebar_focus_and_the_old_item_is_not_reported() {
    let mut a = app(&["alpha.txt"]);
    let home = crate::location::Location::parse(r"C:\Users\u").unwrap();
    a.set_places(std::sync::Arc::new(crate::ui::Places { folders: vec![(crate::ui::KnownFolder::Home, home.clone())], recycle_bin: true, ..Default::default() }));
    a.enter_focus(Focus::Sidebar);
    frame(&mut a);
    let section = a.nav.sidebar.items[0].section;
    a.toggle_section(section);
    a.nav.sidebar.head = Some(section);
    crate::ui::harness::frame(&mut a);
    let t = a.accessibility();
    let node = t.by_name(section.title()).unwrap_or_else(|| panic!("the header {} is in the tree", section.title()));
    assert_eq!(t.nodes[t.focus.unwrap()].id, node.id, "the focused node is the section header, not a stale item");
}

fn press_key(a: &mut App, key: Key, mods: Mods) {
    a.on_event(&Event::Key { key, mods, pressed: true }, Instant::now());
    a.on_event(&Event::Key { key, mods: Mods::default(), pressed: false }, Instant::now());
    frame(a);
}

fn type_text(a: &mut App, text: &str) {
    for c in text.chars() {
        a.on_event(&Event::Char(c), Instant::now());
    }
    frame(a);
}

fn edit_id(t: &Tree, label: S, value: &str) -> u64 {
    let edit = t.nodes.iter().find(|n| n.role == Role::Edit && n.name == label.t()).expect("the active editor is in the accessibility tree");
    assert_eq!(edit.value, value);
    assert!(edit.bounds.w > 0.0 && edit.bounds.h > 0.0);
    assert_eq!(t.focus.map(|k| t.nodes[k].id), Some(edit.id));
    edit.id
}

#[test]
fn the_address_editor_exposes_its_value_and_disappears_on_escape() {
    let mut a = app(&["alpha.txt"]);
    press_key(&mut a, Key::Char('L'), Mods { ctrl: true, ..Mods::default() });
    let id = edit_id(&a.accessibility(), S::A11yRegionAddress, "C:\\Work\\");
    type_text(&mut a, "C:\\Target");
    assert_eq!(edit_id(&a.accessibility(), S::A11yRegionAddress, "C:\\Target"), id);
    press_key(&mut a, Key::Escape, Mods::default());
    assert!(a.accessibility().find(id).is_none());
    assert_eq!(a.tab().location().display(), "C:\\Work");
}

#[test]
fn accessibility_redacts_passwords_in_address_values() {
    let mut a = app(&["alpha.txt"]);
    press_key(&mut a, Key::Char('L'), Mods { ctrl: true, ..Mods::default() });
    type_text(&mut a, "ftp://guest:private@example.test/");
    edit_id(&a.accessibility(), S::A11yRegionAddress, "ftp://guest:***@example.test/");
}

#[test]
fn the_rename_editor_exposes_its_draft_until_it_is_cancelled() {
    let mut a = app(&["alpha.txt", "beta.txt"]);
    let row = a.accessibility().by_name("alpha.txt").unwrap().id;
    a.on_event(&Event::A11ySelect(row), Instant::now());
    press_key(&mut a, Key::F(2), Mods::default());
    let id = edit_id(&a.accessibility(), S::ActionsFileRename, "alpha.txt");
    type_text(&mut a, "renamed");
    assert_eq!(edit_id(&a.accessibility(), S::ActionsFileRename, "renamed.txt"), id);
    press_key(&mut a, Key::Escape, Mods::default());
    let t = a.accessibility();
    assert!(t.find(id).is_none());
    assert_eq!(t.focus.map(|k| t.nodes[k].id), Some(row));
    assert!(t.by_name("renamed.txt").is_none());
}

#[test]
fn the_command_palette_exposes_the_query_and_invokes_a_visible_result() {
    let mut a = app(&["alpha.txt"]);
    press_key(&mut a, Key::Char('P'), Mods { ctrl: true, shift: true, alt: false });
    let query = edit_id(&a.accessibility(), S::PalettePlaceholderCommands, "");
    type_text(&mut a, "Toggle Hidden Files");
    let t = a.accessibility();
    assert_eq!(edit_id(&t, S::PalettePlaceholderCommands, "Toggle Hidden Files"), query);
    let result = t.by_name(S::ActionsViewHidden.t()).expect("the matching command is in the accessibility tree");
    assert_eq!(result.role, Role::ListItem);
    assert!(result.enabled && result.selected);
    let visible_before = a.tab().view_spec.show_hidden;
    a.on_event(&Event::Invoke(result.id), Instant::now());
    frame(&mut a);
    assert_ne!(a.tab().view_spec.show_hidden, visible_before);
    let t = a.accessibility();
    assert!(t.find(query).is_none() && t.find(result.id).is_none());
}

#[test]
fn a_command_removed_by_filtering_cannot_invoke_a_different_result() {
    let mut a = app(&["alpha.txt"]);
    let ctrl = Mods { ctrl: true, ..Mods::default() };
    press_key(&mut a, Key::Char('P'), Mods { shift: true, ..ctrl });
    type_text(&mut a, "Toggle Hidden Files");
    let hidden = a.accessibility().by_name(S::ActionsViewHidden.t()).expect("the matching command is in the accessibility tree").id;
    press_key(&mut a, Key::Char('A'), ctrl);
    type_text(&mut a, "New Tab");
    a.on_event(&Event::Invoke(hidden), Instant::now());
    assert_eq!(a.tabs.items.len(), 1);
    let t = a.accessibility();
    assert!(t.find(hidden).is_none());
    edit_id(&t, S::PalettePlaceholderCommands, "New Tab");
}
