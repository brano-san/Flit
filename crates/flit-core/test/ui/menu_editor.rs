use super::*;
use crate::input::{Event, Key, Mods};
use crate::text::tests::Mono;
use crate::ui::settings::{self, MENU_KIND};
use crate::ui::{Effect, MenuItem, Persisted, Value, SETTINGS};
use crate::Location;
use std::time::Instant;

fn app() -> App {
    let mut a = App::new(Persisted { path: Location::parse(r"C:\x").unwrap(), ..Default::default() }, Vec::new(), 0, false, 1.0, (1000, 700), Instant::now());
    a.animations = false;
    a.drain_effects().for_each(drop);
    a
}

fn cmd(label: &str) -> MenuItem {
    MenuItem::command(label, String::new())
}

fn own_items() -> Vec<MenuItem> {
    vec![cmd("Open"), cmd("Rename"), MenuItem::Separator, MenuItem::Submenu { label: "Send to".into(), items: vec![cmd("Desktop"), cmd("Mail")] }, cmd("Properties")]
}

fn show(a: &mut App, items: Vec<MenuItem>, pending: bool) {
    a.on_event(&Event::ShowMenu { items, x: 100.0, y: 100.0, launches: Vec::new(), kind: MenuKind::File, shell: Vec::new(), pending }, Instant::now());
    a.frame(&mut Gfx::new(Mono, 256), Instant::now());
}

fn shell_arrives(a: &mut App) {
    a.on_event(&Event::ShellItems { items: vec![cmd("Git Bash"), cmd("Scan")], shell: vec![1, 2] }, Instant::now());
    a.frame(&mut Gfx::new(Mono, 256), Instant::now());
}

fn labels(a: &App) -> Vec<String> {
    a.context_menu_open().expect("the context menu is open").labels()
}

fn strings(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_owned()).collect()
}

#[test]
fn remember_keeps_items_of_other_selections_after_their_neighbours() {
    let mut seen = strings(&["Open", "Extract", "Copy", "Compress", "Compress › a.zip"]);
    remember(&mut seen, strings(&["Open", "Copy", "Compress", "Compress › b.zip", "Git"]));
    assert_eq!(seen, ["Open", "Extract", "Copy", "Compress", "Compress › b.zip", "Git"], "a submenu seen again replaces its old children");
    let mut many: Vec<String> = (0..MAX_SEEN).map(|k| k.to_string()).collect();
    remember(&mut many, strings(&["new"]));
    assert_eq!(many.len(), MAX_SEEN);
    assert_eq!(many[0], "new");
}

#[test]
fn hidden_items_are_not_drawn_but_the_search_still_finds_them() {
    let mut a = app();
    a.opts.menu.hidden[0] = strings(&["Rename", "Send to", "Properties"]);
    show(&mut a, own_items(), false);
    assert_eq!(labels(&a), ["Open"]);
    for c in "desk".chars() {
        a.on_event(&Event::Char(c), Instant::now());
    }
    assert_eq!(labels(&a), ["Send to › Desktop"], "a command inside a hidden submenu is found");
    a.on_event(&Event::Key { key: Key::Escape, mods: Mods::default(), pressed: true }, Instant::now());
    for c in "renam".chars() {
        a.on_event(&Event::Char(c), Instant::now());
    }
    assert_eq!(labels(&a), ["Rename"]);
    a.opts.menu.hidden[0] = strings(&["Send to › Mail"]);
    show(&mut a, own_items(), false);
    a.on_event(&Event::Key { key: Key::Down, mods: Mods::default(), pressed: true }, Instant::now());
    for k in [Key::Down, Key::Down, Key::Right] {
        a.on_event(&Event::Key { key: k, mods: Mods::default(), pressed: true }, Instant::now());
    }
    assert_eq!(labels(&a), ["Open", "Rename", "-", "Send to v", "-", "  Desktop", "-", "Properties"], "a hidden child leaves its open submenu");
}

#[test]
fn the_order_applies_to_shell_items_and_new_ones_follow_their_neighbour() {
    let mut a = app();
    a.opts.menu.order[0] = strings(&["Properties", "Scan", "Open"]);
    show(&mut a, own_items(), true);
    assert!(a.opts.menu.seen[0].is_empty(), "a menu still waiting for Windows is not remembered");
    shell_arrives(&mut a);
    assert_eq!(labels(&a), ["Properties", "-", "Git Bash", "Scan", "-", "Open", "Rename", "-", "Send to >"]);
    a.drain_effects().for_each(drop);
    for k in [Key::Down, Key::Down, Key::Down, Key::Enter] {
        a.on_event(&Event::Key { key: k, mods: Mods::default(), pressed: true }, Instant::now());
    }
    assert!(a.drain_effects().any(|e| matches!(e, Effect::ShellPick(Some(2)))), "a moved shell item still runs its own command");
    assert_eq!(a.opts.menu.seen[0], ["Open", "Rename", "Send to", "Send to › Desktop", "Send to › Mail", "Properties", "Git Bash", "Scan"]);
}

#[test]
fn without_a_layout_the_menu_keeps_its_natural_order() {
    let mut a = app();
    show(&mut a, own_items(), false);
    assert_eq!(labels(&a), ["Open", "Rename", "-", "Send to >", "Properties"]);
}

#[test]
fn the_layout_and_the_old_pins_round_trip_through_the_settings_file() {
    let mut a = app();
    a.opts.pins[0] = strings(&["Open"]);
    a.opts.menu.hidden[1] = strings(&["Copy path"]);
    a.opts.menu.order[2] = strings(&["Paste", "Refresh"]);
    a.opts.menu.seen[0] = strings(&["Open", "Send to › Mail"]);
    a.opts.menu.kind = 2;
    let text = settings::render(&a.setting_values(), &serde_json::Map::new(), None);
    let mut b = app();
    b.apply_doc(&settings::parse(&text));
    assert_eq!((b.opts.pins.clone(), b.opts.menu.hidden.clone(), b.opts.menu.order.clone(), b.opts.menu.seen.clone(), b.opts.menu.kind), (a.opts.pins.clone(), a.opts.menu.hidden.clone(), a.opts.menu.order.clone(), a.opts.menu.seen.clone(), 2));
    assert!(text.contains("\"menu.edit_kind\": \"background\""));
    let old = settings::parse(r#"{ "menu.pins_folder": ["New › Folder"], "menu.someday": 1 }"#);
    let mut c = app();
    c.apply_doc(&old);
    assert_eq!(c.opts.pins[1], ["New › Folder"], "files written before the editor keep their pins");
    assert!(old.unknown.contains_key("menu.someday"));
    assert_eq!(c.opts.menu, MenuPrefs::default());
    assert!(SETTINGS[settings::index(MENU_KIND).unwrap()].set(&mut c, Value::Choice(1)));
    assert_eq!(c.opts.menu.kind, 1);
}
