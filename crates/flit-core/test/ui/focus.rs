use super::*;
use crate::input::Key;
use crate::ui::harness::{app, press};

#[test]
fn tab_walks_every_shown_region_in_order_and_shift_tab_walks_back() {
    let mut a = app(&["a.txt", "b.txt"]);
    assert_eq!(a.focus, Focus::List);
    let mut seen = Vec::new();
    for _ in 0..6 {
        press(&mut a, Key::Tab, false);
        seen.push(a.focus);
    }
    assert_eq!(seen, [Focus::TabBar, Focus::Nav, Focus::Address, Focus::Toolbar, Focus::List, Focus::TabBar]);
    press(&mut a, Key::Tab, true);
    assert_eq!(a.focus, Focus::List);
    press(&mut a, Key::Tab, true);
    assert_eq!(a.focus, Focus::Toolbar);
    a.preview.state.visible = true;
    crate::ui::harness::frame(&mut a);
    a.enter_focus(Focus::List);
    press(&mut a, Key::Tab, false);
    assert_eq!(a.focus, Focus::Preview);
}

#[test]
fn entering_the_address_bar_edits_it_and_leaving_stops_editing() {
    let mut a = app(&["a.txt"]);
    a.enter_focus(Focus::Nav);
    press(&mut a, Key::Tab, false);
    assert_eq!(a.focus, Focus::Address);
    assert!(a.editing);
    press(&mut a, Key::Tab, false);
    assert!(!a.editing);
    assert_eq!(a.focus, Focus::Toolbar);
}

#[test]
fn arrows_move_inside_the_toolbar_and_wrap() {
    let mut a = app(&["a.txt"]);
    a.enter_focus(Focus::Toolbar);
    let n = a.focus_targets(Focus::Toolbar).len();
    assert!(n > 2);
    press(&mut a, Key::Right, false);
    assert_eq!(a.kb, 1);
    press(&mut a, Key::Left, false);
    press(&mut a, Key::Left, false);
    assert_eq!(a.kb, n - 1);
    press(&mut a, Key::Escape, false);
    assert_eq!(a.focus, Focus::List);
}

#[test]
fn the_focused_control_gets_a_ring_only_in_keyboard_regions() {
    let mut a = app(&["a.txt"]);
    assert_eq!(a.kb_rect(), None);
    a.enter_focus(Focus::Nav);
    press(&mut a, Key::Right, false);
    assert_eq!(a.kb_rect(), Some(a.layout.nav[1]));
}

#[test]
fn enter_on_the_new_tab_button_opens_a_tab() {
    let mut a = app(&["a.txt"]);
    a.enter_focus(Focus::TabBar);
    press(&mut a, Key::End, false);
    press(&mut a, Key::Enter, false);
    assert_eq!(a.tabs.len(), 2);
}