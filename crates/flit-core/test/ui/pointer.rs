use super::*;
use crate::input::{Key, Mods, MouseButton};
use crate::listing::{attr, DirSnapshot, Listing, Meta};
use crate::location::Location;
use crate::render::Rect;
use crate::sort::{self, CaseMode, SortKeys, ViewSpec};
use crate::ui::{harness, LayoutMode, Persisted};
use std::sync::Arc;

const NO: Mods = Mods { ctrl: false, shift: false, alt: false };
const CTRL: Mods = Mods { ctrl: true, shift: false, alt: false };
const ALT: Mods = Mods { ctrl: false, shift: false, alt: true };
const SHIFT: Mods = Mods { ctrl: false, shift: true, alt: false };
const ALT_SHIFT: Mods = Mods { ctrl: false, shift: true, alt: true };

fn loc(s: &str) -> Location {
    Location::parse(s).unwrap()
}

fn hold(app: &mut App, mods: Mods) {
    app.on_event(&Event::Key { key: Key::Other, mods, pressed: false }, Instant::now());
}

fn wheel(app: &mut App, dx: f32, dy: f32) {
    app.on_event(&Event::Wheel { dx, dy }, Instant::now());
}

fn deliver(app: &mut App, items: &[(&str, bool)]) {
    let t = app.tab();
    let mut listing = Listing::default();
    let mut keys = SortKeys::new(CaseMode::Insensitive);
    for &(n, dir) in items {
        let i = listing.push(n.as_bytes(), Meta { attrs: if dir { attr::DIR } else { 0 }, ..Default::default() });
        keys.push(listing.name(i));
    }
    let mut order = Vec::new();
    sort::sort(&listing, &keys, ViewSpec::default(), &mut order);
    let snap = DirSnapshot { gen: t.gen, path: t.location().path().clone(), listing, keys, order, view: ViewSpec::default(), done: true, error: None, elapsed: Duration::ZERO };
    t.slot.publish(Arc::new(snap));
    app.poll();
    harness::frame(app);
    app.drain_effects().for_each(drop);
}

fn app_at(path: &str, items: &[(&str, bool)]) -> App {
    let mut app = App::new(Persisted::default(), vec![Route { folder: loc(path), select: None }], 0, false, 1.0, (1200, 600), Instant::now());
    deliver(&mut app, items);
    app
}

fn press(app: &mut App, x: f32, y: f32, button: MouseButton, clicks: u8) {
    app.on_event(&Event::MouseMove { x, y }, Instant::now());
    app.on_event(&Event::MouseDown { x, y, button, clicks }, Instant::now());
    app.on_event(&Event::MouseUp { x, y, button }, Instant::now());
}

fn center(r: Rect) -> (f32, f32) {
    (r.x + r.w / 2.0, r.y + r.h / 2.0)
}

fn locations(app: &App) -> Vec<Location> {
    app.tabs.iter().map(|t| t.location().clone()).collect()
}

fn empty_strip(app: &App) -> (f32, f32) {
    let t = app.layout.title;
    let y = t.y + t.h / 2.0;
    let x = (0..t.w as usize).rev().map(|k| t.x + k as f32).find(|&x| app.hit(x, y) == Hit::None).expect("the tab strip has empty room");
    (x, y)
}

fn level(app: &App) -> usize {
    app.tab().sizes.level(app.tab().look)
}

#[test]
fn sideways_wheel_with_ctrl_or_alt_keeps_zoom_font_and_spacing() {
    let mut app = harness::app(&["a.txt", "b.txt"]);
    let (size, font, spacing) = (level(&app), app.ui_scale(), app.opts.spacing);
    for mods in [CTRL, ALT, ALT_SHIFT] {
        hold(&mut app, mods);
        for d in [1.0, -1.0] {
            wheel(&mut app, d, 0.0);
        }
    }
    hold(&mut app, NO);
    assert_eq!((level(&app), app.ui_scale(), app.opts.spacing), (size, font, spacing));
    hold(&mut app, ALT);
    wheel(&mut app, 0.0, 1.0);
    hold(&mut app, NO);
    assert!(app.ui_scale() > font, "Alt+wheel still changes the font");
}

#[test]
fn fine_ctrl_wheel_and_pinch_deltas_add_up_to_whole_zoom_steps() {
    let mut app = harness::app(&["a.txt"]);
    app.set_layout(LayoutMode::Grid);
    let zoom = |app: &App| app.tab().zoom;
    let start = zoom(&app);
    hold(&mut app, CTRL);
    for _ in 0..3 {
        wheel(&mut app, 0.0, 0.25);
    }
    assert_eq!(zoom(&app), start, "three quarters of a notch do not zoom");
    wheel(&mut app, 0.0, 0.25);
    let one = zoom(&app);
    assert!(one > start);
    wheel(&mut app, 0.0, 0.75);
    wheel(&mut app, 0.0, -0.5);
    wheel(&mut app, 0.0, -0.5);
    assert_eq!(zoom(&app), start, "a turn back starts counting afresh");
    hold(&mut app, NO);
    let t0 = Instant::now();
    app.on_event(&Event::Pinch(PINCH_PER_STEP / 2.0), t0);
    assert_eq!(zoom(&app), start);
    app.on_event(&Event::Pinch(PINCH_PER_STEP / 2.0), t0);
    assert_eq!(zoom(&app), one, "a pinch zooms without Ctrl");
    app.on_event(&Event::Pinch(PINCH_PER_STEP * 0.9), t0);
    app.on_event(&Event::Pinch(PINCH_PER_STEP * 0.9), t0 + Duration::from_secs(2));
    assert_eq!(zoom(&app), one, "a pause drops the leftover");
    app.on_event(&Event::Pinch(f32::NAN), t0);
    assert_eq!(zoom(&app), one);
}

#[test]
fn steps_follow_kind_sign_and_pauses() {
    let (mut s, t0) = (WheelSteps::default(), Instant::now());
    assert_eq!(s.add(Steps::Zoom, 2.5, t0), 2);
    assert_eq!(s.add(Steps::Zoom, 0.5, t0), 1);
    assert_eq!(s.add(Steps::Zoom, 0.6, t0), 0);
    assert_eq!(s.add(Steps::Font, 0.6, t0), 0, "another target starts from zero");
    assert_eq!(s.add(Steps::Font, -1.2, t0), -1);
    assert_eq!(s.add(Steps::Font, -0.9, t0 + STEP_IDLE * 2), 0);
    assert!(upright(0.0, 1.0) && upright(-0.2, 1.0) && !upright(1.0, 0.0) && !upright(1.0, 0.5) && !upright(0.0, 0.0));
    assert_eq!((sideways(2.0, 1.0, false), sideways(0.0, -1.0, true), sideways(0.0, -1.0, false)), (2.0, 1.0, 0.0));
}

#[test]
fn the_list_layout_scrolls_with_the_horizontal_wheel_and_details_ignores_it() {
    let names: Vec<String> = (0..400).map(|i| format!("file {i:03}.txt")).collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let mut app = harness::app(&refs);
    wheel(&mut app, 1.0, 0.0);
    assert_eq!(app.tab().scroll_target, 0.0, "Details has nothing to the side");
    app.set_layout(LayoutMode::List);
    harness::frame(&mut app);
    assert!(app.layout.flow.across && app.max_scroll() > 0.0);
    wheel(&mut app, 1.0, 0.0);
    let right = app.tab().scroll_target;
    assert!(right > 0.0, "tilting right reveals later columns");
    hold(&mut app, SHIFT);
    wheel(&mut app, 0.0, -1.0);
    hold(&mut app, NO);
    assert!(app.tab().scroll_target > right, "Shift+wheel down moves right too");
    wheel(&mut app, -5.0, 0.0);
    assert_eq!(app.tab().scroll_target, 0.0);
}

#[test]
fn the_wheel_scrolls_by_the_system_line_count() {
    let names: Vec<String> = (0..400).map(|i| format!("{i}.txt")).collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let mut app = harness::app(&refs);
    let rh = app.layout.row_h;
    wheel(&mut app, 0.0, -1.0);
    assert_eq!(app.tab().scroll_target, 3.0 * rh);
    app.wheel_lines = 5;
    wheel(&mut app, 0.0, -1.0);
    assert_eq!(app.tab().scroll_target, 8.0 * rh);
    app.wheel_lines = WHEEL_PAGE;
    wheel(&mut app, 0.0, -1.0);
    assert_eq!(app.tab().scroll_target, 8.0 * rh + app.view_len());
    app.wheel_lines = 0;
    wheel(&mut app, 0.0, -1.0);
    assert_eq!(app.tab().scroll_target, 8.0 * rh + app.view_len(), "zero lines turns the wheel off like Windows does");
}

#[test]
fn middle_click_on_back_forward_up_and_crumbs_opens_new_tabs() {
    let mut app = app_at(r"C:\work", &[("src", true)]);
    let now = Instant::now();
    app.navigate(loc(r"C:\work\src"), now);
    deliver(&mut app, &[("main.rs", false)]);
    app.go_history(false, now);
    deliver(&mut app, &[("src", true)]);
    assert_eq!(locations(&app), [loc(r"C:\work")]);
    let (x, y) = center(app.layout.nav[1]);
    press(&mut app, x, y, MouseButton::Middle, 1);
    assert_eq!(locations(&app), [loc(r"C:\work"), loc(r"C:\work\src")]);
    let fresh = &app.tabs[1].history;
    assert!(fresh.can_back() && !fresh.can_forward(), "the new tab keeps the history behind it");
    app.select_tab(1);
    harness::frame(&mut app);
    let (x, y) = center(app.layout.nav[0]);
    press(&mut app, x, y, MouseButton::Middle, 1);
    assert_eq!(app.tabs.last().map(|t| t.location().clone()), Some(loc(r"C:\work")));
    assert!(app.tabs.last().unwrap().history.can_forward());
    app.select_tab(1);
    harness::frame(&mut app);
    let (x, y) = center(app.layout.nav[2]);
    press(&mut app, x, y, MouseButton::Middle, 1);
    assert_eq!(app.tabs.len(), 4);
    assert_eq!(app.tabs[3].location(), &loc(r"C:\work"));
    app.select_tab(1);
    harness::frame(&mut app);
    assert_eq!(app.tabs[1].location(), &loc(r"C:\work\src"), "middle clicks never move the tab itself");
    let first = app.tab().crumb(0).map(|c| c.1.clone()).unwrap();
    let (x, y) = center(app.layout.crumbs[0]);
    press(&mut app, x, y, MouseButton::Middle, 1);
    assert_eq!(app.tabs.len(), 5);
    assert_eq!(app.tabs[4].location(), &first);
    app.select_tab(1);
    harness::frame(&mut app);
    hold(&mut app, CTRL);
    let (x, y) = center(app.layout.crumbs[0]);
    press(&mut app, x, y, MouseButton::Left, 1);
    hold(&mut app, NO);
    assert_eq!(app.tabs.len(), 6);
    assert_eq!(app.tabs[5].location(), &first);
    assert_eq!(app.tabs[1].location(), &loc(r"C:\work\src"));
}

#[test]
fn middle_or_double_click_on_the_empty_tab_strip_opens_a_tab() {
    let mut app = app_at(r"C:\work", &[("a.txt", false)]);
    let (x, y) = empty_strip(&app);
    press(&mut app, x, y, MouseButton::Middle, 1);
    assert_eq!(app.tabs.len(), 2);
    harness::frame(&mut app);
    app.drain_effects().for_each(drop);
    let (x, y) = empty_strip(&app);
    press(&mut app, x, y, MouseButton::Left, 1);
    assert_eq!(app.tabs.len(), 2);
    press(&mut app, x, y, MouseButton::Left, 2);
    assert_eq!(app.tabs.len(), 3);
    assert!(!app.drain_effects().any(|e| matches!(e, Effect::Window(WindowCmd::ToggleMaximize))));
}

#[test]
fn a_double_click_on_the_caption_outside_the_strip_still_maximizes() {
    let mut app = app_at(r"C:\work", &[("a.txt", false)]);
    let y = 4.0;
    let x = (0..app.layout.title.x as usize).map(|x| x as f32).find(|&x| app.hit(x, y) == Hit::None && app.in_title(y)).expect("free caption room above the sidebar");
    press(&mut app, x, y, MouseButton::Left, 2);
    assert_eq!(app.tabs.len(), 1);
    assert!(app.drain_effects().any(|e| matches!(e, Effect::Window(WindowCmd::ToggleMaximize))));
}

#[test]
fn alt_double_click_inspects_and_ctrl_double_click_opens_a_folder_window() {
    let mut app = app_at(r"C:\work", &[("docs", true), ("a.txt", false)]);
    let row = |app: &App, name: &str| {
        let v = app.tab().view.as_ref().unwrap();
        let i = (0..v.listing().len() as u32).find(|&i| v.listing().name(i) == name.as_bytes()).unwrap();
        v.row_of(i).unwrap()
    };
    let at = |app: &App, r: usize| {
        let l = &app.layout;
        (l.cols[0].x + 40.0, l.list.y + (r as f32 + 0.5) * l.row_h)
    };
    let (x, y) = at(&app, row(&app, "a.txt"));
    hold(&mut app, ALT);
    press(&mut app, x, y, MouseButton::Left, 1);
    press(&mut app, x, y, MouseButton::Left, 2);
    hold(&mut app, NO);
    let fx: Vec<Effect> = app.drain_effects().collect();
    assert!(fx.iter().any(|e| matches!(e, Effect::Properties(p) if p.len() == 1 && p[0].file_name() == b"a.txt")), "Alt+double click shows properties");
    let (x, y) = at(&app, row(&app, "docs"));
    hold(&mut app, CTRL);
    press(&mut app, x, y, MouseButton::Left, 1);
    press(&mut app, x, y, MouseButton::Left, 2);
    hold(&mut app, NO);
    assert!(app.drain_effects().any(|e| matches!(e, Effect::NewWindow(r) if r[0].folder == loc(r"C:\work\docs"))));
    assert_eq!(app.tab().location(), &loc(r"C:\work"));
    let v = app.tab().view.as_ref().unwrap();
    assert_eq!(v.selected().map(|i| v.listing().name(i).to_vec()).collect::<Vec<_>>(), [b"docs".to_vec()]);
}
