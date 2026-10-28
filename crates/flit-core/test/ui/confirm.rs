use super::*;
use crate::input::Key;
use crate::ui::{harness, Hit};

fn pinned() -> (App, Location) {
    let mut a = harness::app(&["a.txt"]);
    let loc = Location::parse("C:\\Work").unwrap();
    a.toggle_pin(loc.clone());
    harness::frame(&mut a);
    (a, loc)
}

fn ask(a: &mut App, loc: &Location) -> Confirm {
    a.ask(Rect::new(100.0, 100.0, 24.0, 24.0), ("Remove item from list?", "Yes"), true, Confirmed::Unpin(loc.clone()), Instant::now());
    a.confirm.clone().expect("the popover is open")
}

fn click(a: &mut App, r: Rect) {
    let (x, y) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
    let now = Instant::now();
    a.on_event(&Event::MouseMove { x, y }, now);
    a.on_event(&Event::MouseDown { x, y, button: MouseButton::Left, clicks: 1 }, now);
    a.on_event(&Event::MouseUp { x, y, button: MouseButton::Left }, now);
}

fn is_pinned(a: &App, loc: &Location) -> bool {
    a.nav.sidebar.state.pinned.contains(loc)
}

#[test]
fn only_the_confirm_button_runs_the_action() {
    let (mut a, loc) = pinned();
    let c = ask(&mut a, &loc);
    assert!(c.rect.y >= 124.0, "the popover opens under the element");
    assert!(c.buttons[0].x < c.buttons[1].x && c.buttons[0].w == c.buttons[1].w, "confirm on the left, Cancel on the right, equal widths");
    click(&mut a, c.buttons[1]);
    assert!(a.confirm.is_none() && is_pinned(&a, &loc), "Cancel closes and keeps the item");
    ask(&mut a, &loc);
    a.on_event(&Event::Key { key: Key::Escape, mods: Default::default(), pressed: true }, Instant::now());
    assert!(a.confirm.is_none() && is_pinned(&a, &loc), "Escape dismisses");
    ask(&mut a, &loc);
    click(&mut a, Rect::new(900.0, 500.0, 4.0, 4.0));
    assert!(a.confirm.is_none() && is_pinned(&a, &loc), "a click elsewhere dismisses");
    let c = ask(&mut a, &loc);
    click(&mut a, c.buttons[0]);
    assert!(a.confirm.is_none() && !is_pinned(&a, &loc), "Yes removes the item");
}

#[test]
fn the_sidebar_asks_before_unpinning_and_pins_at_once() {
    let (mut a, loc) = pinned();
    let i = a.nav.sidebar.items.iter().position(|it| it.loc == loc).expect("a pinned row");
    a.toggle_pin_item(i, Instant::now());
    assert!(a.confirm.is_some() && is_pinned(&a, &loc), "removing asks first");
    a.on_event(&Event::Key { key: Key::Enter, mods: Default::default(), pressed: true }, Instant::now());
    assert!(!is_pinned(&a, &loc), "Enter confirms");
    assert_eq!(a.hover, Hit::None);
}
