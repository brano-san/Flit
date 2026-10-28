use super::*;
use crate::ui::harness;
use crate::ui::menu::{Cmd, Mark, MenuItem};
use std::time::Instant;

fn checked(items: &[MenuItem]) -> Vec<(String, String, bool)> {
    items
        .iter()
        .filter_map(|m| match m {
            MenuItem::Command { label, keys, checked, mark: Mark::Check, .. } => Some((label.clone(), keys.clone(), *checked)),
            _ => None,
        })
        .collect()
}

#[test]
fn the_types_dropdown_lists_each_type_with_its_count_and_hides_unchecked_ones() {
    let mut app = harness::app(&["a.txt", "b.txt", "c.png", "d.TXT"]);
    let types = app.folder_types();
    assert_eq!(types.iter().map(|t| t.2).collect::<Vec<_>>(), [3, 1], "the same extension in any case is one type");
    assert!(app.run_action("folder.types", Instant::now()));
    let menu = app.build_types_menu(&types);
    let rows = checked(&menu);
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|r| r.2));
    assert_eq!(rows[0].1, "3");
    app.types_command(TypeCmd::Toggle(types[0].0));
    let names: Vec<_> = app.tab().view.as_ref().unwrap().rows().to_vec();
    assert_eq!(names.len(), 1, "only the png is left");
    assert!(app.tab().view.as_ref().unwrap().type_hidden(types[0].0));
    app.types_command(TypeCmd::All(false));
    assert!(app.tab().view.as_ref().unwrap().rows().is_empty());
    app.types_command(TypeCmd::All(true));
    assert_eq!(app.tab().view.as_ref().unwrap().rows().len(), 4);
    assert!(app.menu.iter().any(|c| matches!(c, Cmd::Types(TypeCmd::All(true)))), "Alt+T opens the dropdown");
}
