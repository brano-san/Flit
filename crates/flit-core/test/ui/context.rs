use super::*;
use crate::input::{Key, Mods};

fn cmd(label: &str) -> MenuItem {
    MenuItem::command(label, String::new())
}

fn items() -> Vec<MenuItem> {
    vec![cmd("Open"), MenuItem::Separator, MenuItem::Submenu { label: "View".into(), items: vec![cmd("Details"), cmd("Icons")] }, cmd("Delete")]
}

fn menu(pins: &[&str]) -> ContextMenu {
    ContextMenu::new(&items(), MenuKind::File, pins.iter().map(|p| (*p).to_owned()).collect(), (100.0, 100.0), Rect::new(0.0, 0.0, 1000.0, 800.0), 1.0)
}

fn press(c: &mut ContextMenu, key: Key) -> MenuOutcome {
    c.key(key, Mods::default()).0
}

#[test]
fn submenus_expand_inline_under_their_parent_and_collapse_again() {
    let mut c = menu(&[]);
    assert_eq!(c.labels(), ["Open", "-", "View >", "Delete"]);
    press(&mut c, Key::Down);
    press(&mut c, Key::Down);
    press(&mut c, Key::Right);
    assert!(c.is_open("View"));
    assert_eq!(c.labels(), ["Open", "-", "View v", "-", "  Details", "  Icons", "-", "Delete"]);
    press(&mut c, Key::Down);
    assert_eq!(press(&mut c, Key::Enter), MenuOutcome::Command(1), "commands keep their order through submenus");
    press(&mut c, Key::Left);
    assert!(!c.is_open("View"));
    assert_eq!(c.labels(), ["Open", "-", "View >", "Delete"]);
}

#[test]
fn typing_filters_every_command_including_submenus_and_escape_clears_before_closing() {
    let mut c = menu(&[]);
    c.insert("icn");
    assert_eq!(c.labels(), ["View \u{203A} Icons"]);
    assert_eq!(press(&mut c, Key::Enter), MenuOutcome::Command(2), "the best match is selected while typing");
    c.insert("zzz");
    assert!(c.labels().is_empty());
    assert_eq!(press(&mut c, Key::Escape), MenuOutcome::Open, "the first Escape clears the query");
    assert_eq!(c.labels(), ["Open", "-", "View >", "Delete"]);
    assert_eq!(press(&mut c, Key::Escape), MenuOutcome::Closed);
}

#[test]
fn pinned_commands_move_to_the_top_and_leave_their_place() {
    let mut c = menu(&["View \u{203A} Icons", "Delete", "Gone"]);
    assert_eq!(c.labels(), ["* View \u{203A} Icons", "* Delete", "-", "Open", "-", "View >"]);
    press(&mut c, Key::End);
    press(&mut c, Key::Right);
    assert_eq!(c.labels(), ["* View \u{203A} Icons", "* Delete", "-", "Open", "-", "View v", "-", "  Details"]);
    c.set_pins(Vec::new());
    assert_eq!(c.pins(), [] as [String; 0]);
    assert_eq!(c.labels()[0], "Open");
}

#[test]
fn the_star_of_a_command_asks_to_pin_it_and_of_a_pinned_one_to_unpin_it() {
    let mut c = menu(&["Delete"]);
    let mut g = crate::text::Gfx::new(crate::text::tests::Mono, 256);
    c.measure(&mut g);
    let row = c.rows.iter().position(|r| matches!(r, Row::Node { node, .. } if c.nodes[*node].label == "Open")).unwrap();
    let star = c.star_rect(c.rects[row]);
    let (x, y) = (star.x + star.w / 2.0, star.y + star.h / 2.0);
    c.mouse_move(x, y);
    c.mouse_down(x, y, 1);
    assert!(matches!(c.mouse_up(x, y), MenuOutcome::Pin(k, true, _) if k == "Open"));
    let star = c.star_rect(c.rects[0]);
    let (x, y) = (star.x + star.w / 2.0, star.y + star.h / 2.0);
    c.mouse_move(x, y);
    c.mouse_down(x, y, 1);
    assert!(matches!(c.mouse_up(x, y), MenuOutcome::Pin(k, false, _) if k == "Delete"));
}

#[test]
fn long_menus_scroll_inside_the_window() {
    let many: Vec<MenuItem> = (0..80).map(|k| cmd(&format!("Item {k}"))).collect();
    let mut c = ContextMenu::new(&many, MenuKind::Background, Vec::new(), (10.0, 10.0), Rect::new(0.0, 0.0, 600.0, 400.0), 1.0);
    let mut g = crate::text::Gfx::new(crate::text::tests::Mono, 256);
    c.measure(&mut g);
    assert!(c.rect.bottom() <= 400.0);
    let first = c.rects[0].y;
    c.wheel(-1.0);
    assert!(c.rects[0].y < first, "the wheel scrolls the list");
    press(&mut c, Key::End);
    let last = *c.rects.last().unwrap();
    assert!(last.bottom() <= c.list.bottom() + 0.5, "End reveals the last command");
}
