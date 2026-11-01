use super::*;
use crate::input::{Event, MouseButton};
use crate::text::tests::Mono;
use crate::text::Gfx;
use crate::ui::harness;
use crate::FsPath;
use std::time::Instant;

fn prefetched(app: &mut App) -> Vec<Option<Vec<FsPath>>> {
    app.drain_effects().filter_map(|e| if let Effect::ShellPrefetch(m) = e { Some(m.map(|m| m.items)) } else { None }).collect()
}

fn hover(app: &mut App, row: usize) {
    let r = app.row_rect(row).expect("row on screen");
    app.on_event(&Event::MouseMove { x: r.x + 40.0, y: r.y + r.h / 2.0 }, Instant::now());
    app.frame(&mut Gfx::new(Mono, 1024), Instant::now());
}

fn p(name: &str) -> FsPath {
    FsPath::parse(&format!(r"C:\Work\{name}")).unwrap()
}

#[test]
fn resting_on_an_item_prepares_what_a_right_click_there_would_open() {
    let mut a = harness::app(&["a.txt", "b.txt", "c.txt"]);
    hover(&mut a, 1);
    assert_eq!(prefetched(&mut a), [Some(vec![p("b.txt")])], "an unselected item is prepared alone");
    hover(&mut a, 1);
    assert!(prefetched(&mut a).is_empty(), "the same target is not asked twice");
    a.toggle_row(0);
    a.toggle_row(2);
    hover(&mut a, 2);
    assert_eq!(prefetched(&mut a), [Some(vec![p("a.txt"), p("c.txt")])], "over a selected item the selection is prepared");
    let list = a.layout.list;
    a.on_event(&Event::MouseMove { x: list.x + 40.0, y: list.bottom() - 4.0 }, Instant::now());
    harness::frame(&mut a);
    assert_eq!(prefetched(&mut a), [Some(Vec::new())], "empty space prepares the background");
}

#[test]
fn nothing_is_prepared_while_dragging_and_a_right_click_starts_afresh() {
    let mut a = harness::app(&["a.txt", "b.txt"]);
    hover(&mut a, 0);
    assert_eq!(prefetched(&mut a).len(), 1);
    let r = a.row_rect(1).unwrap();
    a.on_event(&Event::MouseMove { x: r.x + 40.0, y: r.y + 4.0 }, Instant::now());
    a.on_event(&Event::MouseDown { x: r.x + 40.0, y: r.y + 4.0, button: MouseButton::Left, clicks: 1 }, Instant::now());
    harness::frame(&mut a);
    assert_eq!(prefetched(&mut a), [None], "a pressed button cancels the prepared menu");
    a.on_event(&Event::MouseUp { x: r.x + 40.0, y: r.y + 4.0, button: MouseButton::Left }, Instant::now());
    harness::frame(&mut a);
    assert_eq!(prefetched(&mut a), [Some(vec![p("b.txt")])]);
    a.on_event(&Event::MouseDown { x: r.x + 40.0, y: r.y + 4.0, button: MouseButton::Right, clicks: 1 }, Instant::now());
    a.on_event(&Event::MouseUp { x: r.x + 40.0, y: r.y + 4.0, button: MouseButton::Right }, Instant::now());
    let menu = a.drain_effects().find_map(|e| if let Effect::ShellMenu(m) = e { Some(m) } else { None }).expect("a menu");
    assert!(menu.same_target(&ShellMenu { x: 0.0, y: 0.0, menu: Vec::new(), ..menu.clone() }));
    assert_eq!(menu.items, [p("b.txt")]);
    a.on_event(&Event::ShowMenu { items: menu.menu.clone(), x: 0.0, y: 0.0, launches: Vec::new(), kind: menu.kind, shell: Vec::new(), pending: true }, Instant::now());
    harness::frame(&mut a);
    assert!(prefetched(&mut a).is_empty(), "no preparing while the menu is open");
    a.on_event(&Event::Key { key: crate::input::Key::Escape, mods: Default::default(), pressed: true }, Instant::now());
    harness::frame(&mut a);
    assert_eq!(prefetched(&mut a), [Some(vec![p("b.txt")])], "after the menu closes the target is prepared again");
}

#[test]
fn the_first_listing_warms_a_folder_a_file_and_the_background_once() {
    let mut a = App::new(crate::ui::Persisted { path: crate::Location::parse(r"C:\Work").unwrap(), ..Default::default() }, Vec::new(), 0, false, 1.0, (1000, 600), Instant::now());
    harness::frame(&mut a);
    assert!(!a.drain_effects().any(|e| matches!(e, Effect::ShellWarm(_))), "no listing yet");
    let mut c = harness::app(&["a.png", "notes.txt"]);
    c.prefetch = Default::default();
    harness::frame(&mut c);
    let warm: Vec<Vec<FsPath>> = c.drain_effects().filter_map(|e| if let Effect::ShellWarm(m) = e { Some(m) } else { None }).flatten().map(|m| m.items).collect();
    assert_eq!(warm, [vec![p("notes.txt")], Vec::new()], "a text file is preferred and the background follows");
    harness::frame(&mut c);
    assert!(!c.drain_effects().any(|e| matches!(e, Effect::ShellWarm(_))), "only once");
}
