use super::*;
use crate::a11y::Role;
use crate::input::Event;
use crate::ui::harness::{app, frame};

#[test]
fn the_tree_names_regions_and_controls_with_their_bounds() {
    let mut a = app(&["alpha.txt", "beta.txt"]);
    let t = a.accessibility();
    let root = &t.nodes[0];
    assert_eq!(root.role, Role::Window);
    let roles: Vec<Role> = root.children.iter().map(|&k| t.nodes[k].role).collect();
    assert!(roles.starts_with(&[Role::TabList, Role::ToolBar, Role::ToolBar, Role::ToolBar, Role::Tree, Role::DataGrid]), "{roles:?}");
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
    let paste = t.by_name(S::ToolbarButtonPaste.t()).unwrap();
    assert_eq!(paste.help, description("file.paste"));
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
    assert_eq!(a.tabs.len(), 2);
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
    let files = t.by_name(crate::ui::settings::Page::Files.title()).unwrap();
    assert!(files.role == Role::Tab && files.selected);
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
    let v = a.tabs[a.cur].view.as_mut().unwrap();
    let i = v.find(b"alpha.txt").unwrap();
    v.set_selected(i, true);
    v.focus = Some(i);
    a.hashes_dialog();
    a.layout();
    let t = a.accessibility();
    let group = t.nodes.iter().find(|n| n.role == Role::Group).expect("the algorithms group");
    let options: Vec<_> = group.children.iter().map(|&c| &t.nodes[c]).collect();
    let before = a.jobs.form.as_ref().unwrap().flags(0);
    assert_eq!(options.len(), before.len());
    assert!(options.iter().all(|n| n.role == Role::CheckBox && !n.name.is_empty()));
    assert_eq!(options.iter().map(|n| n.selected).collect::<Vec<_>>(), before);
    let second = options[1].id;
    a.on_event(&Event::A11yFocus(second), Instant::now());
    assert_eq!(a.accessibility().focus.map(|k| a.accessibility().nodes[k].id), Some(second));
    a.on_event(&Event::Invoke(second), Instant::now());
    let after = a.jobs.form.as_ref().unwrap().flags(0);
    assert_eq!(after[1], !before[1]);
    assert_eq!(after[0], before[0]);
}

#[test]
fn command_bar_buttons_are_focusable_before_the_first_frame() {
    let mut a = App::new(crate::ui::Persisted { path: crate::Location::parse("C:\\Work").unwrap(), ..Default::default() }, Vec::new(), 0, false, 1.0, (1000, 600), Instant::now());
    a.enter_focus(Focus::Toolbar);
    assert_eq!(a.focus, Focus::Toolbar);
    let t = a.accessibility();
    let paste = t.by_name(S::ToolbarButtonPaste.t()).unwrap();
    assert_eq!(paste.role, Role::Button);
    assert!(t.focus.is_some_and(|k| t.nodes[t.nodes[k].parent].role == Role::ToolBar));
}

#[test]
fn form_dialogs_expose_fields_with_values_and_their_buttons() {
    let mut a = app(&["alpha.txt"]);
    let v = a.tabs[a.cur].view.as_mut().unwrap();
    let i = v.find(b"alpha.txt").unwrap();
    v.set_selected(i, true);
    a.compress_dialog();
    a.layout();
    let t = a.accessibility();
    let form = a.jobs.form.as_ref().unwrap();
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
    assert!(a.jobs.form.is_none());
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
    let refresh = t.by_name(action_title("nav.refresh")).unwrap().id;
    a.on_event(&Event::A11yFocus(refresh), Instant::now());
    let t = a.accessibility();
    assert_eq!(t.nodes[t.focus.unwrap()].id, refresh);
}
