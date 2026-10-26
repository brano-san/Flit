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