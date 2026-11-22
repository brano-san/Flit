use super::*;
use std::time::Duration;
use crate::input::{Event, HitArea, Mods};
use crate::listing::{attr, DirSnapshot, Listing, Meta};
use crate::path::FsPath;
use crate::provider::ProviderRequest;
use crate::sort::{self, CaseMode, SortKeys};
use crate::ui::{drop_effect, DragEvent, MenuItem, Persisted, Spot, WindowCmd};
use std::sync::Arc;

const NO: Mods = Mods { ctrl: false, shift: false, alt: false };
const CTRL: Mods = Mods { ctrl: true, shift: false, alt: false };
const CTRL_ALT: Mods = Mods { ctrl: true, shift: false, alt: true };

fn loc(s: &str) -> Location {
    Location::parse(s).unwrap()
}

fn app_with(paths: &[&str]) -> App {
    let routes = paths.iter().map(|p| Route { folder: loc(p), select: None }).collect();
    let mut app = App::new(Persisted::default(), routes, 0, false, 1.0, (2000, 600), Instant::now());
    app.drain_effects().for_each(drop);
    app
}

fn names(app: &App) -> Vec<String> {
    app.tabs.items.iter().map(|t| t.location().display().into_owned()).collect()
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
    app.drain_effects().for_each(drop);
}

fn ev(app: &mut App, e: Event) {
    app.on_event(&e, Instant::now());
}

fn key(app: &mut App, key: Key, mods: Mods) {
    ev(app, Event::Key { key, mods, pressed: true });
}

fn tab_xy(app: &App, i: usize) -> (f32, f32) {
    let r = app.layout.tabs[i].0;
    (r.x + 30.0, r.y + r.h / 2.0)
}

fn row_xy(app: &App, row: usize) -> (f32, f32) {
    let l = &app.layout;
    (l.cols[0].x + 200.0, l.list.y + (row as f32 + 0.5) * l.row_h)
}

fn menu_labels(app: &mut App) -> Vec<String> {
    let menu = app.drain_effects().find_map(|e| if let Effect::ShellMenu(m) = e { Some(m.menu) } else { None }).unwrap();
    menu.into_iter().filter_map(|i| if let MenuItem::Command { label, .. } = i { Some(label) } else { None }).collect()
}

fn gray_labels(app: &mut App) -> Vec<String> {
    let menu = app.drain_effects().find_map(|e| if let Effect::ShellMenu(m) = e { Some(m.menu) } else { None }).unwrap();
    menu.into_iter().filter_map(|i| if let MenuItem::Command { label, disabled: true, .. } = i { Some(label) } else { None }).collect()
}

fn listed(app: &mut App) -> Vec<String> {
    app.drain_effects().filter_map(|e| if let Effect::Provider { req: ProviderRequest::List { path, .. }, .. } = e { Some(path.display().into_owned()) } else { None }).collect()
}

#[test]
fn mru_tab_cycle_keeps_the_same_order_until_control_is_released() {
    let mut app = app_with(&[r"C:\a", r"C:\b", r"C:\c", r"C:\d"]);
    for i in [2, 0, 1] {
        app.select_tab(i);
    }
    key(&mut app, Key::Tab, CTRL);
    assert_eq!(app.tabs.cur, 0);
    ev(&mut app, Event::Key { key: Key::Tab, mods: CTRL, pressed: false });
    key(&mut app, Key::Tab, CTRL);
    assert_eq!(app.tabs.cur, 2);
    key(&mut app, Key::Tab, Mods { shift: true, ..CTRL });
    assert_eq!(app.tabs.cur, 0);
    ev(&mut app, Event::Key { key: Key::Other, mods: NO, pressed: false });
    key(&mut app, Key::Tab, CTRL);
    assert_eq!(app.tabs.cur, 1);
}

#[test]
fn mru_tab_cycle_can_be_disabled_and_control_page_keys_use_the_tab_strip() {
    use crate::ui::settings::{self, Value, SETTINGS};
    let mut app = app_with(&[r"C:\a", r"C:\b", r"C:\c", r"C:\d"]);
    app.set_caps(crate::caps::Caps { unix_paths: true });
    for i in [2, 0, 1] {
        app.select_tab(i);
    }
    key(&mut app, Key::PageDown, CTRL);
    assert_eq!(app.tabs.cur, 2);
    key(&mut app, Key::PageUp, CTRL);
    assert_eq!(app.tabs.cur, 1);
    let row = settings::index("general.mru_tabs").unwrap();
    assert!(SETTINGS[row].set(&mut app, Value::Bool(false)));
    key(&mut app, Key::Tab, CTRL);
    assert_eq!(app.tabs.cur, 2);
}

#[test]
fn tab_menu_closes_other_left_and_right_tabs_and_remembers_them() {
    let mut app = app_with(&[r"C:\a", r"C:\b", r"C:\c", r"C:\d", r"C:\e"]);
    let (x, y) = tab_xy(&app, 2);
    ev(&mut app, Event::MouseDown { x, y, button: MouseButton::Right, clicks: 1 });
    ev(&mut app, Event::MouseUp { x, y, button: MouseButton::Right });
    assert_eq!(app.tabs.cur, 2);
    let labels = menu_labels(&mut app);
    assert_eq!(labels, ["Shift Tab Left", "Shift Tab Right", "Duplicate Tab", "Duplicate Tab To New Window", "Close Tab", "Close Other Tabs", "Close Left Tabs", "Close Right Tabs", "Close All Tabs", "Move tab to a new window", "Reopen closed tab"]);
    assert_eq!(app.menu_owner, Some(Hit::Tab(2)));
    let (x0, y0) = tab_xy(&app, 0);
    ev(&mut app, Event::MouseDown { x: x0, y: y0, button: MouseButton::Right, clicks: 1 });
    ev(&mut app, Event::MouseUp { x: x0, y: y0, button: MouseButton::Right });
    let gray = gray_labels(&mut app);
    assert_eq!(gray, ["Shift Tab Left", "Close Left Tabs", "Reopen closed tab"]);
    app.select_tab(2);
    app.run_action("tab.close_right", Instant::now());
    assert_eq!(names(&app), [r"C:\a", r"C:\b", r"C:\c"]);
    app.run_action("tab.close_left", Instant::now());
    assert_eq!(names(&app), [r"C:\c"]);
    assert_eq!(app.tabs.cur, 0);
    app.run_action("tab.reopen", Instant::now());
    assert_eq!(names(&app), [r"C:\c", r"C:\b"]);
    app.run_action("tab.close_others", Instant::now());
    assert_eq!(names(&app), [r"C:\b"]);
    app.drain_effects().for_each(drop);
    key(&mut app, Key::Char('W'), Mods { ctrl: true, shift: true, alt: false });
    assert!(app.drain_effects().any(|e| matches!(e, Effect::Window(WindowCmd::Close))));
}

#[test]
fn dragging_a_tab_reorders_and_esc_puts_it_back() {
    let mut app = app_with(&[r"C:\a", r"C:\b", r"C:\c"]);
    let (x0, y) = tab_xy(&app, 0);
    let (x2, _) = tab_xy(&app, 2);
    let r2 = app.layout.tabs[2].0;
    ev(&mut app, Event::MouseDown { x: x0, y, button: MouseButton::Left, clicks: 1 });
    ev(&mut app, Event::MouseMove { x: r2.x + r2.w * 0.75, y });
    assert_eq!(names(&app), [r"C:\b", r"C:\c", r"C:\a"]);
    assert_eq!(app.tabs.cur, 2);
    ev(&mut app, Event::MouseUp { x: x2, y, button: MouseButton::Left });
    assert_eq!(app.drag, Drag::None);
    let (x1, _) = tab_xy(&app, 1);
    ev(&mut app, Event::MouseDown { x: x2, y, button: MouseButton::Left, clicks: 1 });
    ev(&mut app, Event::MouseMove { x: x1 - 40.0, y });
    assert_eq!(names(&app), [r"C:\b", r"C:\a", r"C:\c"]);
    key(&mut app, Key::Escape, NO);
    assert_eq!(names(&app), [r"C:\b", r"C:\c", r"C:\a"]);
    assert_eq!(app.tabs.cur, 2);
    ev(&mut app, Event::MouseUp { x: x1, y, button: MouseButton::Left });
    assert!(!app.drain_effects().any(|e| matches!(e, Effect::MoveTab { .. })));
}

#[test]
fn dragging_outside_the_strip_switches_to_split_preview_and_returning_or_cancelling_cleans_it_up() {
    let mut app = app_with(&[r"C:\a", r"C:\b"]);
    let (x, y) = tab_xy(&app, 0);
    ev(&mut app, Event::MouseDown { x, y, button: MouseButton::Left, clicks: 1 });
    ev(&mut app, Event::MouseMove { x: x + 10.0, y });
    assert!(app.dragged_tab().is_some() && app.insert_mark().is_some());
    let below = app.layout.title.bottom() + 1.0;
    ev(&mut app, Event::MouseMove { x, y: below });
    assert!(app.tabs.docking.is_some(), "split preview starts immediately below the strip");
    assert!(app.dragged_tab().is_none() && app.insert_mark().is_none());
    ev(&mut app, Event::MouseMove { x, y });
    assert!(app.tabs.docking.is_none() && app.dragged_tab().is_some());
    key(&mut app, Key::Escape, NO);
    assert!(app.tabs.docking.is_none() && app.dragged_tab().is_none() && app.insert_mark().is_none());
    assert!(app.tabs.strip.hold.is_none());
}

#[test]
fn a_tab_torn_off_the_strip_moves_to_another_window() {
    let mut app = app_with(&[r"C:\a", r"C:\b"]);
    app.run_action("nav.up", Instant::now());
    let (x, y) = tab_xy(&app, 1);
    ev(&mut app, Event::MouseDown { x, y, button: MouseButton::Left, clicks: 1 });
    ev(&mut app, Event::MouseMove { x, y: 300.0 });
    assert!(matches!(app.drag, Drag::Tab { torn: false, .. }) && app.tabs.docking.is_some(), "inside the window the tab docks");
    ev(&mut app, Event::MouseMove { x, y: -30.0 });
    assert!(matches!(app.drag, Drag::Tab { torn: true, .. }) && app.tabs.docking.is_none());
    app.drain_effects().for_each(drop);
    ev(&mut app, Event::MouseUp { x, y: -30.0, button: MouseButton::Left });
    let moved = app.drain_effects().find_map(|e| if let Effect::MoveTab { tab, at, last } = e { Some((tab, at, last)) } else { None }).unwrap();
    assert_eq!(moved.0.location(), &loc(r"C:\"));
    assert_eq!((moved.1, moved.2), (Some((x, -30.0)), false));
    assert_eq!(names(&app), [r"C:\a"]);

    let mut target = app_with(&[r"C:\x", r"C:\y"]);
    let r = target.layout.tabs[1].0;
    let base = target.tabs.next_tab;
    target.insert_tab(moved.0, Some((r.x + 5.0, 10.0)), Instant::now());
    assert_eq!(names(&target), [r"C:\x", r"C:\", r"C:\y"]);
    assert_eq!(target.tabs.cur, 1);
    assert_eq!(target.tab().id, base);
    assert!(target.tab().history.can_back());
    assert_eq!(listed(&mut target), [r"C:\"]);

    let (x, y) = tab_xy(&app, 0);
    ev(&mut app, Event::MouseDown { x, y, button: MouseButton::Left, clicks: 1 });
    ev(&mut app, Event::MouseMove { x, y: -30.0 });
    ev(&mut app, Event::MouseUp { x, y: -30.0, button: MouseButton::Left });
    let back = app.drain_effects().find_map(|e| if let Effect::MoveTab { tab, last: true, .. } = e { Some(tab) } else { None }).unwrap();
    assert_eq!(names(&app), [r"C:\a"]);
    app.adopt_tab(back, Instant::now());
    assert_eq!(names(&app), [r"C:\a"]);
    assert_eq!(listed(&mut app), [r"C:\a"]);
    app.run_action("tab.move_to_window", Instant::now());
    assert!(!app.drain_effects().any(|e| matches!(e, Effect::MoveTab { .. })));
}

fn tab_over(app: &mut App) -> Vec<Option<(f32, f32)>> {
    app.drain_effects().filter_map(|e| if let Effect::TabOver(at) = e { Some(at) } else { None }).collect()
}

#[test]
fn a_tab_dragged_out_of_the_window_reports_where_it_is_until_it_comes_back_or_is_dropped() {
    let mut app = app_with(&[r"C:\a", r"C:\b", r"C:\c"]);
    let (x, y) = tab_xy(&app, 1);
    ev(&mut app, Event::MouseDown { x, y, button: MouseButton::Left, clicks: 1 });
    ev(&mut app, Event::MouseMove { x: x + 40.0, y });
    assert_eq!(tab_over(&mut app), [], "on its own strip the tab reports nothing");
    ev(&mut app, Event::MouseMove { x, y: -30.0 });
    ev(&mut app, Event::MouseMove { x: -80.0, y: -30.0 });
    assert_eq!(tab_over(&mut app), [Some((x, -30.0)), Some((-80.0, -30.0))], "outside the window every move is reported");
    ev(&mut app, Event::MouseMove { x, y });
    ev(&mut app, Event::MouseMove { x: x + 10.0, y });
    assert_eq!(tab_over(&mut app), [None], "back over its window the mark is taken away once");
    ev(&mut app, Event::MouseMove { x, y: -30.0 });
    key(&mut app, Key::Escape, NO);
    assert_eq!(tab_over(&mut app), [Some((x, -30.0)), None], "Esc takes the mark away");
    ev(&mut app, Event::MouseUp { x, y: -30.0, button: MouseButton::Left });
    assert_eq!(tab_over(&mut app), []);
}

#[test]
fn the_window_under_a_dragged_tab_marks_where_it_will_land() {
    let mut app = app_with(&[r"C:\a", r"C:\b"]);
    let (x, y) = tab_xy(&app, 1);
    ev(&mut app, Event::MouseDown { x, y, button: MouseButton::Left, clicks: 1 });
    ev(&mut app, Event::MouseMove { x, y: -30.0 });
    ev(&mut app, Event::MouseUp { x, y: -30.0, button: MouseButton::Left });
    let moved = app.drain_effects().find_map(|e| if let Effect::MoveTab { tab, .. } = e { Some(tab) } else { None }).unwrap();

    let mut target = app_with(&[r"C:\x", r"C:\y", r"C:\z"]);
    target.layout();
    assert_eq!(target.insert_mark(), None, "no mark without a dragged tab");
    let [t0, t1, _] = [0, 1, 2].map(|i| target.layout.tabs[i].0);
    let at = (t1.x + 10.0, t1.y + 200.0);
    assert!(target.tab_incoming(Some(at)));
    assert!(!target.tab_incoming(Some((at.0 + 1.0, at.1))), "moving within one gap changes nothing");
    let mark = target.insert_mark().expect("a mark");
    assert!(mark.x < t1.x && mark.right() > t1.x && mark.x > t0.x + t0.w / 2.0, "the mark stands between the first and second tab: {mark:?}");
    let mut g = Gfx::new(crate::text::tests::Mono, 1024);
    target.frame(&mut g, Instant::now());
    let accent = target.look.palette.base.accent.0;
    assert!(g.dl.instances.iter().any(|i| i.rect == [mark.x, mark.y, mark.w, mark.h] && i.color == accent), "the mark is drawn");
    assert!(target.tab_incoming(Some((t0.x + 2.0, at.1))));
    assert_eq!(target.insert_mark().map(|r| r.right() > t0.x && r.x < t0.x), Some(true), "before the first tab");
    assert!(target.tab_incoming(Some(at)));
    target.insert_tab(moved, Some(at), Instant::now());
    assert_eq!(names(&target), [r"C:\x", r"C:\b", r"C:\y", r"C:\z"], "the tab lands where the mark stood");
    assert_eq!(target.insert_mark(), None, "and the mark goes");
    assert!(!target.tab_incoming(None));
}

#[test]
fn the_mark_shows_only_in_the_panel_under_the_pointer() {
    let mut target = app_with(&[r"C:\x", r"C:\y"]);
    key(&mut target, Key::Char('S'), Mods { ctrl: true, shift: true, alt: false });
    target.animations = false;
    target.layout();
    let right = target.tabs.panels.active;
    let left = target.tabs.panels.list.iter().find(|p| p.id != right).unwrap().id;
    let r = target.layout.tabs[0].0;
    assert!(target.tab_incoming(Some((r.right() - 2.0, r.y + 300.0))));
    assert!(target.insert_mark().is_some(), "the right panel shows the mark");
    assert_eq!(target.visit(left, |a| a.insert_mark()), None, "the left panel does not");
    let l = target.tabs.panels.list.iter().find(|p| p.id == left).unwrap().layout.tabs[0].0;
    assert!(target.tab_incoming(Some((l.x + 5.0, l.y + 300.0))));
    assert_eq!(target.insert_mark(), None);
    assert!(target.visit(left, |a| a.insert_mark()).is_some());
    assert!(target.tab_incoming(None));
    assert_eq!(target.visit(left, |a| a.insert_mark()), None, "leaving the window clears it");
}

#[test]
fn wheel_over_the_strip_switches_tabs_in_the_chosen_direction() {
    let mut app = app_with(&[r"C:\a", r"C:\b", r"C:\c"]);
    app.select_tab(0);
    let (x, y) = tab_xy(&app, 1);
    ev(&mut app, Event::MouseMove { x, y });
    ev(&mut app, Event::Wheel { dx: 0.0, dy: -1.0 });
    assert_eq!(app.tabs.cur, 1);
    ev(&mut app, Event::Wheel { dx: 0.0, dy: -0.5 });
    ev(&mut app, Event::Wheel { dx: 0.0, dy: -0.5 });
    assert_eq!(app.tabs.cur, 2);
    ev(&mut app, Event::Wheel { dx: 0.0, dy: -3.0 });
    assert_eq!(app.tabs.cur, 2);
    app.tab_wheel_reversed = true;
    ev(&mut app, Event::Wheel { dx: 0.0, dy: 1.0 });
    assert_eq!(app.tabs.cur, 2);
    ev(&mut app, Event::Wheel { dx: 0.0, dy: -2.0 });
    assert_eq!(app.tabs.cur, 0);
    let (x, y) = row_xy(&app, 3);
    ev(&mut app, Event::MouseMove { x, y });
    ev(&mut app, Event::Wheel { dx: 0.0, dy: -1.0 });
    assert_eq!(app.tabs.cur, 0);
}

#[test]
fn restored_tabs_stay_asleep_until_shown() {
    let mut app = app_with(&[r"C:\b"]);
    let rev = app.session_rev();
    app.restore_tabs(&[loc(r"C:\a"), loc(r"C:\b"), loc(r"C:\c")], 1);
    assert!(app.session_rev() > rev);
    assert_eq!(names(&app), [r"C:\a", r"C:\b", r"C:\c"]);
    assert_eq!(app.tabs.cur, 1);
    assert!(listed(&mut app).is_empty());
    assert_eq!(app.tabs.items[0].gen, 0);
    assert_eq!(app.tabs.items[2].title(), "c");
    assert_eq!(app.session(), WindowSession { tabs: vec![loc(r"C:\a"), loc(r"C:\b"), loc(r"C:\c")], cur: 1, panels: None, trails: vec![loc(r"C:\a"), loc(r"C:\b"), loc(r"C:\c")] });
    key(&mut app, Key::Char('3'), CTRL);
    assert_eq!(listed(&mut app), [r"C:\c"]);
    assert_eq!(app.window_title(), r"C:\c — Flit");
    key(&mut app, Key::Char('2'), CTRL);
    assert!(listed(&mut app).is_empty());
}

#[test]
fn hovering_a_tab_shows_its_full_path_after_a_pause() {
    let mut app = app_with(&[r"C:\a\deep", r"C:\b"]);
    let start = Instant::now();
    let (x, y) = tab_xy(&app, 0);
    let delay = crate::ui::tooltip::DELAY;
    app.on_event(&Event::MouseMove { x, y }, start);
    assert_eq!(app.tooltip(), None);
    assert!(app.drain_effects().any(|e| matches!(e, Effect::Timer(_))));
    app.on_event(&Event::Timer, start + delay);
    assert_eq!(app.tooltip().map(|t| (t.line, t.command)), Some((r"C:\a\deep".to_owned(), None)));
    let (x, y) = row_xy(&app, 2);
    app.on_event(&Event::MouseMove { x, y }, start + delay);
    assert_eq!(app.tooltip(), None);
}

#[test]
fn several_selected_items_open_together_and_folders_go_to_new_tabs() {
    let mut app = app_with(&[r"C:\x"]);
    deliver(&mut app, &[("a", true), ("b", true), ("f.txt", false)]);
    key(&mut app, Key::Char('A'), CTRL);
    key(&mut app, Key::Enter, NO);
    let fx: Vec<Effect> = app.drain_effects().collect();
    assert!(fx.iter().any(|e| matches!(e, Effect::Provider { req: ProviderRequest::Open { path, .. }, .. } if path.display() == r"C:\x\f.txt")));
    assert_eq!(names(&app), [r"C:\x", r"C:\x\a", r"C:\x\b"]);

    app.select_tab(0);
    let v = app.tabs.items[0].view.as_mut().unwrap();
    v.clear_selection();
    let a = v.find(b"a").unwrap();
    v.set_selected(a, true);
    key(&mut app, Key::Enter, CTRL);
    assert_eq!(names(&app).len(), 4);
    app.select_tab(0);
    app.drain_effects().for_each(drop);
    app.run_action("item.open_window", Instant::now());
    assert!(app.drain_effects().any(|e| matches!(e, Effect::NewWindow(r) if r[0].folder == loc(r"C:\x\a"))));
}

#[test]
fn middle_click_opens_a_folder_in_a_new_tab_and_double_click_on_empty_space_goes_up() {
    let mut app = app_with(&[r"C:\x"]);
    deliver(&mut app, &[("a", true), ("f.txt", false)]);
    let (x, y) = row_xy(&app, 0);
    ev(&mut app, Event::MouseDown { x, y, button: MouseButton::Middle, clicks: 1 });
    ev(&mut app, Event::MouseUp { x, y, button: MouseButton::Middle });
    assert_eq!(names(&app), [r"C:\x", r"C:\x\a"]);
    app.select_tab(0);

    let (x, y) = row_xy(&app, 8);
    app.double_click_up = false;
    ev(&mut app, Event::MouseDown { x, y, button: MouseButton::Left, clicks: 2 });
    ev(&mut app, Event::MouseUp { x, y, button: MouseButton::Left });
    assert_eq!(app.tab().location(), &loc(r"C:\x"));
    app.double_click_up = true;
    ev(&mut app, Event::MouseDown { x, y, button: MouseButton::Left, clicks: 2 });
    ev(&mut app, Event::MouseUp { x, y, button: MouseButton::Left });
    assert_eq!(app.tab().location(), &loc(r"C:\"));
    deliver(&mut app, &[("x", true), ("f.txt", false)]);
    let (x, y) = row_xy(&app, 1);
    ev(&mut app, Event::MouseDown { x, y, button: MouseButton::Middle, clicks: 1 });
    ev(&mut app, Event::MouseUp { x, y, button: MouseButton::Middle });
    assert_eq!(names(&app).len(), 2);
    assert!(app.autoscrolling());
}

#[test]
fn folders_open_in_new_tabs_when_the_flag_is_on() {
    let mut app = app_with(&[r"C:\x"]);
    deliver(&mut app, &[("a", true)]);
    app.folders_in_new_tab = true;
    key(&mut app, Key::Down, NO);
    key(&mut app, Key::Enter, NO);
    assert_eq!(names(&app), [r"C:\x", r"C:\x\a"]);
}

#[test]
fn dropping_onto_the_new_tab_button_opens_new_tabs() {
    let mut app = app_with(&[r"C:\x"]);
    let r = app.layout.new_tab;
    let (x, y) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
    let paths = vec![FsPath::parse(r"D:\docs").unwrap()];
    let effect = app.drag(&DragEvent::Enter { paths, x, y, mods: NO, allowed: drop_effect::COPY | drop_effect::MOVE, right: false }, Instant::now());
    assert_eq!(effect, drop_effect::COPY);
    assert_eq!(app.drop_spot(), Some(Spot::NewTab));
    assert_eq!(app.drop_caption(), Some(("Open %1 in a new tab".to_owned(), "docs".to_owned())));
    app.drag(&DragEvent::Drop { x, y, mods: NO, allowed: drop_effect::COPY | drop_effect::MOVE }, Instant::now());
    assert_eq!(names(&app), [r"C:\x", r"D:\docs"]);
}

#[test]
fn tab_actions_button_follows_its_flag() {
    let mut app = app_with(&[r"C:\x", r"C:\y"]);
    let r = app.layout.actions;
    assert!(r.w > 0.0 && r.right() <= app.tabs.panels.caption[0].x);
    ev(&mut app, Event::MouseDown { x: r.x + 5.0, y: r.y + 5.0, button: MouseButton::Left, clicks: 1 });
    ev(&mut app, Event::MouseUp { x: r.x + 5.0, y: r.y + 5.0, button: MouseButton::Left });
    assert!(matches!(app.nav.palette.as_ref().map(|p| &p.pick), Some(crate::ui::palette::Pick::Tabs)), "the button opens Search Tabs");
    app.tab_actions_button = false;
    app.layout();
    assert_eq!(app.layout.actions.w, 0.0);
}

#[test]
fn window_modes_have_shortcuts() {
    let mut app = app_with(&[r"C:\x"]);
    key(&mut app, Key::F(11), NO);
    key(&mut app, Key::F(12), NO);
    key(&mut app, Key::Up, CTRL_ALT);
    key(&mut app, Key::Down, CTRL_ALT);
    let cmds: Vec<WindowCmd> = app.drain_effects().filter_map(|e| if let Effect::Window(c) = e { Some(c) } else { None }).collect();
    assert_eq!(cmds, [WindowCmd::Fullscreen, WindowCmd::Compact(None), WindowCmd::Compact(Some(true)), WindowCmd::Compact(Some(false))]);
}

fn tab_icons(app: &mut App) -> Vec<(String, crate::ui::IconKind, u64)> {
    app.drain_effects().filter_map(|e| if let Effect::Icon(r) = e { (r.size == 16).then(|| (r.path.display().into_owned(), r.kind, r.key)) } else { None }).collect()
}

#[test]
fn tabs_show_the_system_icon_of_their_location() {
    use crate::ui::IconKind::{Folder, Place, Volume};
    use crate::ui::{Drive, KnownFolder, Places};
    let mut app = app_with(&[r"C:\Users\u\Downloads", r"C:\", r"C:\work\sub", "wsl://Debian/", r"C:\work\pinned"]);
    let drive = Drive { path: FsPath::parse(r"C:\").unwrap(), label: "Local Disk (C:)".into(), total: 10, free: 5, file_system: String::new(), removable: false, unmounted: false, kind: None, remote: None };
    app.set_places(Arc::new(Places { folders: vec![(KnownFolder::Downloads, loc(r"C:\Users\u\Downloads"))], drives: vec![drive], ..Default::default() }));
    app.set_pinned(vec![loc(r"C:\work\pinned")]);
    app.open_in_tabs([Location::computer(), Location::recycle_bin(), Location::home_page()].into_iter().map(|folder| Route { folder, select: None }).collect(), Instant::now());
    app.drain_effects().for_each(drop);
    let mut g = crate::text::Gfx::new(crate::text::tests::Mono, 1024);
    app.frame(&mut g, Instant::now());
    let asked: Vec<(String, crate::ui::IconKind)> = tab_icons(&mut app).into_iter().map(|(p, k, _)| (p, k)).collect();
    for want in [
        (r"C:\Users\u\Downloads".to_owned(), Place),
        (r"C:\".to_owned(), Volume),
        (r"C:\work\sub".to_owned(), Folder),
        (r"\\wsl.localhost\Debian\".to_owned(), Volume),
        (r"C:\work\pinned".to_owned(), Place),
        (FsPath::computer().display().into_owned(), Place),
        (FsPath::recycle_bin().display().into_owned(), Place),
    ] {
        assert!(asked.contains(&want), "{want:?} not in {asked:?}");
    }
    assert_eq!(asked.len(), 7, "the Flit home page keeps its own glyph: {asked:?}");
    app.frame(&mut g, Instant::now());
    assert!(tab_icons(&mut app).is_empty(), "pending tab icons are asked once");
}

#[test]
fn ordinary_folder_tabs_share_one_cached_folder_icon_that_replaces_the_glyph() {
    let mut app = app_with(&[r"C:\x\one", r"C:\x\two"]);
    let mut g = crate::text::Gfx::new(crate::text::tests::Mono, 1024);
    app.frame(&mut g, Instant::now());
    let folders: Vec<u64> = tab_icons(&mut app).into_iter().filter(|(_, k, _)| *k == crate::ui::IconKind::Folder).map(|(_, _, key)| key).collect();
    assert_eq!(folders.len(), 1, "one cached folder icon for every ordinary folder tab");
    let tab = app.layout.tabs[1].0;
    let inside = |g: &crate::text::Gfx<crate::text::tests::Mono>, kind| g.dl.instances.iter().filter(|i| i.kind == kind && i.rect[0] >= tab.x && i.rect[0] < tab.x + 28.0 && i.rect[1] >= tab.y && i.rect[1] < tab.bottom()).count();
    assert_eq!(inside(&g, crate::render::kind::GLYPH), 1, "the glyph stands in while loading");
    app.on_icon(&mut g, folders[0], crate::ui::IconResult::Ready(crate::ui::Bitmap { width: 16, height: 16, rgba: vec![255; 16 * 16 * 4] }));
    app.frame(&mut g, Instant::now());
    assert_eq!(inside(&g, crate::render::kind::IMAGE), 1);
    assert_eq!(inside(&g, crate::render::kind::GLYPH), 0, "no glyph once the icon is here");
}

fn drop_on(app: &mut App, from: usize, pick: impl Fn(&crate::ui::dock::Zone) -> bool) {
    let (x, y) = tab_xy(app, from);
    ev(app, Event::MouseDown { x, y, button: MouseButton::Left, clicks: 1 });
    let r = app.panel_rect();
    ev(app, Event::MouseMove { x: r.x + r.w / 2.0, y: r.y + r.h / 2.0 });
    let z = *app.tabs.docking.as_ref().unwrap().zones.iter().find(|z| pick(z)).expect("zone");
    let (zx, zy) = (z.button.x + 2.0, z.button.y + 2.0);
    ev(app, Event::MouseMove { x: zx, y: zy });
    assert_eq!(app.tabs.docking.as_ref().unwrap().hot.map(|k| app.tabs.docking.as_ref().unwrap().zones[k].dock), Some(z.dock));
    ev(app, Event::MouseUp { x: zx, y: zy, button: MouseButton::Left });
    assert!(app.tabs.docking.is_none());
    let mut g = crate::text::Gfx::new(crate::text::tests::Mono, 1024);
    app.frame(&mut g, Instant::now());
}

#[test]
fn dropping_a_tab_on_a_zone_splits_moves_or_copies_it() {
    use crate::ui::dock::Dock;
    let mut app = app_with(&[r"C:\a", r"C:\b"]);
    let first = app.tabs.panels.active;
    drop_on(&mut app, 1, |z| matches!(z.dock, Dock::Split { down: false, before: false, .. }));
    assert_eq!(app.panel_count(), 2);
    assert_ne!(app.tabs.panels.active, first);
    assert_eq!(names(&app), [r"C:\b"]);
    let second = app.tabs.panels.active;
    assert!(app.panel_rect().x > 300.0, "the new panel is on the right");
    let (x, y) = tab_xy(&app, 0);
    ev(&mut app, Event::MouseDown { x, y, button: MouseButton::Left, clicks: 1 });
    let r = app.panel_rect();
    ev(&mut app, Event::MouseMove { x: r.x + r.w / 2.0, y: r.y + r.h / 2.0 });
    let own = |app: &App| app.tabs.docking.as_ref().unwrap().zones.iter().any(|z| matches!(z.dock, Dock::Split { panel, .. } | Dock::Into(panel) if panel == second));
    assert!(!own(&app), "the only tab of a panel is not offered its own panel");
    ev(&mut app, Event::Key { key: Key::Other, mods: CTRL, pressed: true });
    assert!(own(&app), "a copy may split its own panel, offered as soon as Ctrl goes down");
    ev(&mut app, Event::Key { key: Key::Other, mods: NO, pressed: false });
    assert!(!own(&app));
    key(&mut app, Key::Escape, NO);
    assert_eq!(app.panel_count(), 2);
    app.activate_panel(first);
    assert_eq!(names(&app), [r"C:\a"]);
    let r = app.tabs.panels.list.iter().find(|p| p.id == second).unwrap().rect;
    let (x, y) = tab_xy(&app, 0);
    ev(&mut app, Event::MouseDown { x, y, button: MouseButton::Left, clicks: 1 });
    ev(&mut app, Event::MouseMove { x: r.x + r.w / 2.0, y: r.y + r.h / 2.0 });
    let z = *app.tabs.docking.as_ref().unwrap().zones.iter().find(|z| z.dock == Dock::Into(second)).unwrap();
    ev(&mut app, Event::MouseMove { x: z.button.x + 2.0, y: z.button.y + 2.0 });
    ev(&mut app, Event::Key { key: Key::Other, mods: Mods { ctrl: true, shift: false, alt: false }, pressed: true });
    ev(&mut app, Event::MouseUp { x: z.button.x + 2.0, y: z.button.y + 2.0, button: MouseButton::Left });
    assert_eq!(app.panel_count(), 2, "a copy leaves the source panel");
    assert_eq!(app.tabs.panels.active, second);
    assert_eq!(names(&app), [r"C:\b", r"C:\a"]);
    assert_eq!(app.tabs.cur, 1);
    ev(&mut app, Event::Key { key: Key::Other, mods: NO, pressed: false });
    app.activate_panel(first);
    let r = app.tabs.panels.list.iter().find(|p| p.id == second).unwrap().rect;
    let (x, y) = tab_xy(&app, 0);
    ev(&mut app, Event::MouseDown { x, y, button: MouseButton::Left, clicks: 1 });
    ev(&mut app, Event::MouseMove { x: r.x + r.w / 2.0, y: r.y + r.h / 2.0 });
    let z = *app.tabs.docking.as_ref().unwrap().zones.iter().find(|z| z.dock == Dock::Into(second)).unwrap();
    ev(&mut app, Event::MouseMove { x: z.button.x + 2.0, y: z.button.y + 2.0 });
    ev(&mut app, Event::MouseUp { x: z.button.x + 2.0, y: z.button.y + 2.0, button: MouseButton::Left });
    assert_eq!(app.panel_count(), 1, "moving the last tab away closes its panel");
    assert_eq!(names(&app), [r"C:\b", r"C:\a", r"C:\a"]);
    let (x, y) = tab_xy(&app, 0);
    ev(&mut app, Event::MouseDown { x, y, button: MouseButton::Left, clicks: 1 });
    let left = app.tabs.panels.left;
    ev(&mut app, Event::MouseMove { x: left + 5.0, y: 400.0 });
    let edge = app.tabs.docking.as_ref().unwrap().zones.iter().find(|z| z.dock == Dock::Window { before: true }).copied();
    assert!(edge.is_some(), "the window edge offers a column");
    key(&mut app, Key::Escape, NO);
    assert!(app.tabs.docking.is_none() && app.drag == Drag::None);
}

#[test]
fn a_tab_close_button_and_the_new_tab_button_share_one_centre_line() {
    let a = crate::ui::harness::app(&["a.txt"]);
    let (_, close) = a.layout.tabs[0];
    let add = a.layout.new_tab;
    assert!((close.y + close.h / 2.0 - (add.y + add.h / 2.0)).abs() <= 0.5, "{close:?} {add:?}");
}

#[test]
fn a_new_tab_slides_in_and_the_strip_settles_without_more_frames() {
    let mut a = crate::ui::harness::app(&["a.txt"]);
    let mut g = Gfx::new(crate::text::tests::Mono, 1024);
    let t0 = Instant::now();
    assert!(!a.frame(&mut g, t0), "restored tabs do not animate");
    a.open_new_tab(t0);
    let t1 = Instant::now();
    assert!(a.frame(&mut g, t1), "the new tab slides in");
    assert!(a.tabs.strip.arrival(a.tabs.items[a.tabs.items.len() - 1].id, t1 + std::time::Duration::from_millis(20)) < 1.0);
    assert!(!a.frame(&mut g, t1 + std::time::Duration::from_millis(300)));
    a.animations = false;
    a.open_new_tab(t1);
    assert!(!a.frame(&mut g, t1 + std::time::Duration::from_millis(310)), "without Animations it appears at once");
}

#[test]
fn a_new_tab_of_the_open_folder_shows_its_rows_at_once_and_refreshes_behind_them() {
    let mut app = app_with(&[r"C:\a"]);
    deliver(&mut app, &[("x.txt", false), ("y.txt", false), ("sub", true)]);
    app.open_new_tab(Instant::now());
    assert_eq!(app.tabs.items.len(), 2);
    assert_eq!(app.tabs.cur, 1);
    assert_eq!(app.tab().view.as_ref().map(|v| v.rows().len()), Some(3), "the new tab draws the listing it duplicates on its first frame");
    assert_eq!(listed(&mut app), [r"C:\a"], "and still reads the folder in the background");
    deliver(&mut app, &[("x.txt", false), ("y.txt", false), ("sub", true), ("z.txt", false)]);
    assert_eq!(app.tab().view.as_ref().map(|v| v.rows().len()), Some(4));
    app.select_tab(0);
    app.dispatch(vec![Route { folder: loc(r"C:\b"), select: None }], crate::open::OpenTarget::NewTab, Instant::now());
    assert!(app.tab().view.is_none(), "another folder starts empty as before");
}

#[test]
fn a_second_right_click_on_the_tab_keeps_its_menu_open_and_only_that_tab_is_lit() {
    let mut app = app_with(&[r"C:\a", r"C:\b"]);
    let mut g = crate::text::Gfx::new(crate::text::tests::Mono, 1024);
    app.frame(&mut g, Instant::now());
    let (x, y) = tab_xy(&app, 1);
    let right = |app: &mut App| {
        ev(app, Event::MouseMove { x, y });
        ev(app, Event::MouseDown { x, y, button: crate::input::MouseButton::Right, clicks: 1 });
        ev(app, Event::MouseUp { x, y, button: crate::input::MouseButton::Right });
        let shown: Vec<Effect> = app.drain_effects().collect();
        for e in shown {
            if let Effect::ShellMenu(m) = e {
                ev(app, Event::ShowMenu { items: m.menu, x: m.x, y: m.y, launches: Vec::new(), kind: m.kind, shell: Vec::new(), pending: false });
            }
        }
    };
    right(&mut app);
    app.frame(&mut g, Instant::now());
    assert!(app.menu_open_from(Hit::Tab(1)));
    let opened = app.own_menu.as_ref().map(|m| m.shown);
    right(&mut app);
    assert!(app.menu_open_from(Hit::Tab(1)), "the menu stays open");
    assert_eq!(app.own_menu.as_ref().map(|m| m.shown), opened, "it is the same menu, not closed and opened again");
    assert!(!app.menu_open_from(Hit::Tab(0)));
    key(&mut app, Key::Escape, Mods::default());
    key(&mut app, Key::Char('S'), Mods { ctrl: true, shift: true, alt: false });
    app.frame(&mut g, Instant::now());
    app.activate_panel(0);
    let (x, y) = tab_xy(&app, 0);
    ev(&mut app, Event::MouseMove { x, y });
    ev(&mut app, Event::MouseDown { x, y, button: crate::input::MouseButton::Right, clicks: 1 });
    ev(&mut app, Event::MouseUp { x, y, button: crate::input::MouseButton::Right });
    let m = app.drain_effects().find_map(|e| if let Effect::ShellMenu(m) = e { Some(m) } else { None }).unwrap();
    ev(&mut app, Event::ShowMenu { items: m.menu, x: m.x, y: m.y, launches: Vec::new(), kind: m.kind, shell: Vec::new(), pending: false });
    assert!(app.menu_open_from(Hit::Tab(0)));
    let other = app.parked_ids()[0];
    assert!(!app.visit(other, |a| a.menu_open_from(Hit::Tab(0))), "the first tab of the other panel is not lit");
}

#[test]
fn reopening_a_closed_tab_restores_its_history() {
    let mut app = app_with(&[r"C:\a", r"C:\b"]);
    deliver(&mut app, &[("x", false)]);
    app.navigate(loc(r"C:\a\one"), Instant::now());
    deliver(&mut app, &[("y", false)]);
    app.navigate(loc(r"C:\a\one\two"), Instant::now());
    deliver(&mut app, &[("z", false)]);
    assert_eq!(app.tab().history.back_list().len(), 2);
    key(&mut app, Key::Char('W'), CTRL);
    assert_eq!(app.tabs().len(), 1);
    assert_eq!(app.tabs.closed.last().map(|c| c.location()), Some(&loc(r"C:\a\one\two")));
    assert_eq!(app.tabs.closed.last().map(|c| c.history.back_list().len()), Some(2), "the trail is remembered");
    app.run_action("tab.reopen", Instant::now());
    assert_eq!(app.tabs().len(), 2);
    assert_eq!(*app.tab().location(), loc(r"C:\a\one\two"), "the tab reopens at its last folder");
    assert_eq!(app.tab().history.back_list().len(), 2, "the back trail comes back with it");
    assert!(app.tab().history.can_back(), "Back walks the restored trail");
    key(&mut app, Key::Backspace, NO);
    assert_eq!(*app.tab().location(), loc(r"C:\a\one"));
}

fn numbered(n: usize) -> Vec<String> {
    (0..n).map(|i| format!("f{i:03}")).collect()
}

fn deliver_files(app: &mut App, names: &[String]) {
    let items: Vec<(&str, bool)> = names.iter().map(|n| (n.as_str(), false)).collect();
    deliver(app, &items);
}

#[test]
fn reopening_a_closed_tab_brings_back_forward_history_scroll_and_selection() {
    let files = numbered(300);
    let mut app = app_with(&[r"C:\a", r"C:\b"]);
    app.select_tab(0);
    deliver(&mut app, &[("one", true)]);
    app.navigate(loc(r"C:\a\one"), Instant::now());
    deliver_files(&mut app, &files);
    app.navigate(loc(r"C:\a\one\two"), Instant::now());
    deliver(&mut app, &[("z", false)]);
    app.run_action("nav.back", Instant::now());
    deliver_files(&mut app, &files);
    app.layout();
    assert!(app.max_scroll() > 500.0);
    let t = app.tab_mut();
    (t.scroll, t.scroll_target) = (480.0, 480.0);
    let v = t.view.as_mut().unwrap();
    let i = v.find(b"f150").unwrap();
    v.clear_selection();
    v.set_selected(i, true);
    v.focus = Some(i);
    key(&mut app, Key::Char('W'), CTRL);
    assert_eq!(app.tabs().len(), 1);
    app.run_action("tab.reopen", Instant::now());
    assert_eq!(*app.tab().location(), loc(r"C:\a\one"));
    assert_eq!(app.tab().history.forward_list(), [loc(r"C:\a\one\two")], "Forward comes back with the tab");
    assert_eq!(app.tab().history.back_list(), [loc(r"C:\a")]);
    deliver_files(&mut app, &files);
    app.layout();
    assert_eq!((app.tab().scroll, app.tab().scroll_target), (480.0, 480.0), "the list is scrolled where it was");
    assert_eq!(app.tab().focused_name().as_deref(), Some(&b"f150"[..]), "and the same item is focused");
    app.run_action("nav.forward", Instant::now());
    assert_eq!(*app.tab().location(), loc(r"C:\a\one\two"));
}

#[test]
fn each_reopened_tab_gets_its_own_history_also_from_the_tab_list() {
    let mut app = app_with(&[r"C:\a", r"C:\b", r"C:\c"]);
    app.select_tab(0);
    app.navigate(loc(r"C:\a\x"), Instant::now());
    app.select_tab(1);
    app.navigate(loc(r"C:\b\y"), Instant::now());
    app.navigate(loc(r"C:\b\y\z"), Instant::now());
    key(&mut app, Key::Char('W'), CTRL);
    app.select_tab(0);
    key(&mut app, Key::Char('W'), CTRL);
    assert_eq!(names(&app), [r"C:\c"]);
    app.run_action("tab.new", Instant::now());
    app.select_tab(0);
    app.run_action("tab.search", Instant::now());
    assert!(app.tab_row_closed(2));
    app.choose_tab_row(2, Instant::now());
    assert_eq!((app.tabs.cur, names(&app)), (1, vec![r"C:\c".to_owned(), r"C:\a\x".to_owned(), r"C:\c".to_owned()]), "the latest closed tab opens next to the current one");
    assert_eq!(app.tab().history.back_list(), [loc(r"C:\a")]);
    assert!(app.tabs()[2].history.back_list().is_empty(), "the last tab keeps its own empty history");
    assert_eq!(app.tabs.closed.len(), 1);
    app.run_action("tab.reopen", Instant::now());
    assert_eq!(*app.tab().location(), loc(r"C:\b\y\z"));
    assert_eq!(app.tab().history.back_list(), [loc(r"C:\b"), loc(r"C:\b\y")], "the other closed tab kept its own trail");
    assert!(app.tabs.closed.is_empty());
    app.run_action("tab.reopen", Instant::now());
    assert_eq!(app.tabs().len(), 4, "nothing more to reopen");
}

#[test]
fn a_new_tab_opens_next_to_the_current_one() {
    let mut app = app_with(&[r"C:\a", r"C:\b", r"C:\c"]);
    deliver(&mut app, &[("x", false)]);
    app.select_tab(1);
    key(&mut app, Key::Char('T'), CTRL);
    assert_eq!(app.tabs().len(), 4);
    assert_eq!(app.tabs.cur, 2, "the new tab takes the place right of the current one");
    assert_eq!(names(&app), [r"C:\a", r"C:\b", r"C:\b", r"C:\c"], "the tabs after it shift right");
    app.select_tab(0);
    key(&mut app, Key::Char('T'), CTRL);
    assert_eq!(names(&app), [r"C:\a", r"C:\a", r"C:\b", r"C:\b", r"C:\c"]);
    assert_eq!(app.tabs.cur, 1);
}

#[test]
fn right_click_on_the_empty_strip_opens_new_tab_and_reopen() {
    let mut app = app_with(&[r"C:\a", r"C:\b"]);
    deliver(&mut app, &[("x", false)]);
    let t = app.layout.title;
    let y = t.y + t.h / 2.0;
    let mut at = None;
    for k in (0..t.w as usize).rev() {
        let cx = t.x + k as f32;
        if app.hit(cx, y) == crate::ui::Hit::None && !app.layout.new_tab.contains(cx, y) {
            at = Some((cx, y));
            break;
        }
    }
    let (x, y) = at.expect("empty strip room");
    app.on_event(&Event::MouseDown { x, y, button: MouseButton::Right, clicks: 1 }, Instant::now());
    app.on_event(&Event::MouseUp { x, y, button: MouseButton::Right }, Instant::now());
    let labels: Vec<String> = app.drain_effects().find_map(|e| if let Effect::ShellMenu(m) = e { Some(m.menu.into_iter().filter_map(|i| if let MenuItem::Command { label, .. } = i { Some(label) } else { None }).collect()) } else { None }).expect("a strip menu");
    assert_eq!(labels, ["Open New Tab", "Reopen closed tab"]);
    let tabs = app.tabs().len();
    app.on_event(&Event::MenuCommand(0), Instant::now());
    assert_eq!(app.tabs().len(), tabs + 1, "New tab from the strip menu opens one");
}

fn many(n: usize, width: u32) -> App {
    let paths: Vec<String> = (0..n).map(|i| format!(r"C:\t{i:02}")).collect();
    let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
    let mut app = app_with(&refs);
    ev(&mut app, Event::Resized { width, height: 600 });
    app
}

fn shown(app: &App) -> Vec<usize> {
    app.layout.tabs.iter().enumerate().filter(|(_, (r, _))| r.w > 0.0).map(|(i, _)| i).collect()
}

fn arrow_click(app: &mut App, right: bool) {
    let r = app.layout.tab_arrows[usize::from(right)];
    let (x, y) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
    ev(app, Event::MouseDown { x, y, button: MouseButton::Left, clicks: 1 });
    ev(app, Event::MouseUp { x, y, button: MouseButton::Left });
    app.layout();
}

fn arrows_enabled(app: &mut App) -> Vec<(String, bool)> {
    let tree = app.accessibility();
    tree.nodes.iter().filter(|n| n.name.starts_with("Scroll tabs")).map(|n| (n.name.clone(), n.enabled)).collect()
}

#[test]
fn tabs_shrink_first_and_only_then_hide_behind_arrows_that_scroll_the_strip() {
    let app = many(3, 2000);
    assert_eq!(shown(&app), [0, 1, 2]);
    assert!(app.layout.tabs.iter().all(|(r, _)| r.w == app.s(crate::ui::TAB_W)), "a few tabs keep the full width");
    assert_eq!(app.layout.tab_arrows, [Rect::default(); 2], "no arrows while everything fits");

    let app = many(12, 2000);
    assert_eq!(shown(&app).len(), 12, "twelve tabs still fit by getting narrower");
    let w = app.layout.tabs[0].0.w;
    assert!(w < app.s(crate::ui::TAB_W) && w >= app.s(crate::ui::TAB_MIN_W), "shrunk to {w}");
    assert_eq!(app.layout.tab_arrows, [Rect::default(); 2]);

    let mut app = many(30, 2000);
    app.select_tab(0);
    app.layout();
    let visible = shown(&app);
    assert!(visible.len() < 30 && visible[0] == 0, "{visible:?}");
    assert!(visible.windows(2).all(|p| p[1] == p[0] + 1), "the visible tabs are one run");
    let [left, right] = app.layout.tab_arrows;
    let first = app.layout.tabs[visible[0]].0;
    let last = app.layout.tabs[*visible.last().unwrap()].0;
    assert!(left.w > 0.0 && left.right() <= first.x && last.right() <= right.x && right.right() <= app.layout.new_tab.x, "arrows frame the tabs and the new tab button follows");
    assert_eq!(arrows_enabled(&mut app), [("Scroll tabs left".to_owned(), false), ("Scroll tabs right".to_owned(), true)]);
    assert_eq!(app.hit_test(left.x + 2.0, left.y + 2.0), HitArea::Client, "an arrow is a button, not window caption");
    let hidden = 30 - visible.len();
    assert_eq!(app.tip_text(Hit::TabScroll(true)).unwrap().details, [format!("{hidden} tabs are hidden on this side.")]);

    arrow_click(&mut app, true);
    assert_eq!(shown(&app)[0], 1, "the strip moves by one tab");
    assert_eq!(app.tabs.cur, 0, "scrolling does not switch tabs");
    app.layout();
    assert_eq!(shown(&app)[0], 1, "the next layout keeps the scrolled strip");
    assert_eq!(app.tip_text(Hit::TabScroll(false)).unwrap().details, ["1 tab is hidden on this side."]);
    for _ in 0..40 {
        arrow_click(&mut app, true);
    }
    assert_eq!(shown(&app).last(), Some(&29), "the strip stops at the last tab");
    assert_eq!(arrows_enabled(&mut app), [("Scroll tabs left".to_owned(), true), ("Scroll tabs right".to_owned(), false)]);
    assert!(app.tip_text(Hit::TabScroll(true)).unwrap().details.is_empty());
    for _ in 0..40 {
        arrow_click(&mut app, false);
    }
    assert_eq!(shown(&app)[0], 0, "and back at the first one");
}

#[test]
fn the_current_tab_is_scrolled_into_view_whenever_it_changes() {
    let mut app = many(30, 2000);
    app.select_tab(0);
    app.layout();
    arrow_click(&mut app, true);
    arrow_click(&mut app, true);
    key(&mut app, Key::Tab, CTRL);
    app.layout();
    assert_eq!(app.tabs.cur, 29);
    assert!(shown(&app).contains(&29), "Ctrl+Tab brings the most recently used tab back into view");

    app.select_tab(29);
    app.layout();
    assert_eq!(shown(&app).last(), Some(&29));
    app.select_tab(28);
    app.layout();
    assert_eq!(shown(&app).last(), Some(&29), "a visible tab is selected without moving the strip");

    app.select_tab(29);
    app.run_action("tab.new", Instant::now());
    app.layout();
    assert_eq!(app.tabs.cur, 30);
    assert!(shown(&app).contains(&30), "a new tab is in view");

    app.select_tab(3);
    app.layout();
    ev(&mut app, Event::Resized { width: 1100, height: 600 });
    assert!(shown(&app).contains(&3), "a resize keeps the current tab in view");
    app.run_action("tab.close", Instant::now());
    app.layout();
    assert!(shown(&app).contains(&app.tabs.cur), "closing the current tab shows the next current one");

    app.enter_focus(crate::ui::Focus::TabBar);
    key(&mut app, Key::End, NO);
    key(&mut app, Key::Left, NO);
    app.layout();
    let kb = app.kb;
    assert!(kb > shown(&app)[0] && shown(&app).contains(&kb), "the keyboard focus scrolls the strip to its tab");
    key(&mut app, Key::Escape, NO);
    app.layout();
    assert!(shown(&app).contains(&app.tabs.cur), "leaving the strip brings the current tab back");

    let mut restored = app_with(&[r"C:\a"]);
    let locs: Vec<Location> = (0..30).map(|i| loc(&format!(r"C:\r{i:02}"))).collect();
    restored.restore_tabs(&locs, 25);
    restored.layout();
    assert!(shown(&restored).contains(&25), "a restored session shows its current tab");
}

#[test]
fn scrolling_one_panel_strip_leaves_the_other_alone_and_its_arrows_work_while_parked() {
    let mut app = many(30, 2000);
    app.animations = false;
    app.select_tab(0);
    key(&mut app, Key::Char('S'), Mods { ctrl: true, shift: true, alt: false });
    assert_eq!(app.tabs.panels.active, 1);
    app.activate_panel(0);
    app.layout();
    arrow_click(&mut app, true);
    arrow_click(&mut app, true);
    let first = shown(&app)[0];
    app.activate_panel(1);
    for _ in 0..25 {
        app.run_action("tab.new", Instant::now());
    }
    app.layout();
    assert!(app.layout.tab_arrows[0].w > 0.0, "the other panel overflows on its own");
    arrow_click(&mut app, false);
    let parked = &app.tabs.panels.list.iter().find(|p| p.id == 0).unwrap().layout;
    assert_eq!(parked.tabs.iter().position(|(r, _)| r.w > 0.0), Some(first), "the parked strip keeps its place");
    let r = parked.tab_arrows[1];
    let (x, y) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
    assert_eq!(app.hit_test(x, y), HitArea::Client);
    ev(&mut app, Event::MouseDown { x, y, button: MouseButton::Left, clicks: 1 });
    ev(&mut app, Event::MouseUp { x, y, button: MouseButton::Left });
    app.layout();
    assert_eq!(app.tabs.panels.active, 0);
    assert_eq!(shown(&app)[0], first + 1, "a parked panel's arrow scrolls that panel");
}

fn centre(r: Rect) -> (f32, f32) {
    (r.x + r.w / 2.0, r.y + r.h / 2.0)
}

fn timer(app: &mut App, at: Instant) {
    app.on_event(&Event::Timer, at);
    app.layout();
}

fn asks_timer(app: &mut App) -> bool {
    app.drain_effects().any(|e| matches!(e, Effect::Timer(_)))
}

#[test]
fn dragging_a_tab_onto_an_arrow_scrolls_the_strip_step_by_step_and_esc_brings_it_back() {
    let mut app = many(30, 2000);
    app.select_tab(4);
    app.layout();
    let (fit, s0) = (shown(&app).len(), shown(&app)[0]);
    let (x, y) = tab_xy(&app, 4);
    let right = centre(app.layout.tab_arrows[1]);
    ev(&mut app, Event::MouseDown { x, y, button: MouseButton::Left, clicks: 1 });
    ev(&mut app, Event::MouseMove { x: right.0, y });
    app.layout();
    assert_eq!(shown(&app)[0], s0 + 1, "the strip moves by one tab, not to the end");
    assert_eq!((app.tabs.cur, names(&app)[s0 + fit].as_str()), (s0 + fit, r"C:\t04"), "the dragged tab rides at the right edge");
    assert!(shown(&app).contains(&app.tabs.cur), "the dragged tab stays in view");
    ev(&mut app, Event::MouseMove { x: right.0 + 1.0, y });
    app.layout();
    assert_eq!(shown(&app)[0], s0 + 1, "moving over the arrow does not add steps");
    let t = Instant::now();
    timer(&mut app, t + Duration::from_millis(100));
    assert_eq!(shown(&app)[0], s0 + 1, "it waits before repeating");
    timer(&mut app, t + super::HOLD_DELAY);
    assert_eq!((shown(&app)[0], app.tabs.cur), (s0 + 2, s0 + fit + 1), "then steps while it stays over the arrow");
    timer(&mut app, t + super::HOLD_DELAY + super::HOLD_REPEAT);
    assert_eq!((shown(&app)[0], app.tabs.cur), (s0 + 3, s0 + fit + 2));
    let left = centre(app.layout.tab_arrows[0]);
    ev(&mut app, Event::MouseMove { x: left.0, y });
    app.layout();
    assert_eq!((shown(&app)[0], app.tabs.cur, names(&app)[s0 + 2].as_str()), (s0 + 2, s0 + 2, r"C:\t04"), "the left arrow steps back with the tab at its edge");
    key(&mut app, Key::Escape, NO);
    app.layout();
    assert_eq!((app.tabs.cur, names(&app)[4].as_str()), (4, r"C:\t04"));
    assert!(shown(&app).contains(&4));
    let before = shown(&app)[0];
    timer(&mut app, t + Duration::from_secs(5));
    assert_eq!(shown(&app)[0], before, "a cancelled drag stops the steps");
    ev(&mut app, Event::MouseUp { x, y, button: MouseButton::Left });
}

#[test]
fn a_held_arrow_repeats_until_released_or_left_and_a_click_steps_once() {
    let mut app = many(30, 2000);
    app.select_tab(0);
    app.layout();
    let (x, y) = centre(app.layout.tab_arrows[1]);
    let t = Instant::now();
    app.on_event(&Event::MouseDown { x, y, button: MouseButton::Left, clicks: 1 }, t);
    app.layout();
    assert_eq!(shown(&app)[0], 1, "pressing steps at once");
    assert!(asks_timer(&mut app));
    timer(&mut app, t + Duration::from_millis(200));
    assert_eq!(shown(&app)[0], 1);
    timer(&mut app, t + super::HOLD_DELAY);
    assert_eq!(shown(&app)[0], 2, "holding repeats after a pause");
    let step = t + super::HOLD_DELAY + super::HOLD_REPEAT;
    timer(&mut app, step);
    assert_eq!(shown(&app)[0], 3);
    let (ox, oy) = tab_xy(&app, 10);
    app.on_event(&Event::MouseMove { x: ox, y: oy }, step);
    timer(&mut app, step + super::HOLD_REPEAT);
    assert_eq!(shown(&app)[0], 3, "off the arrow it waits");
    app.on_event(&Event::MouseMove { x, y }, step);
    timer(&mut app, step + super::HOLD_REPEAT * 2);
    assert_eq!(shown(&app)[0], 4, "back over it the steps go on");
    app.on_event(&Event::MouseUp { x, y, button: MouseButton::Left }, step);
    app.layout();
    assert_eq!(shown(&app)[0], 4, "releasing adds no step");
    app.drain_effects().for_each(drop);
    timer(&mut app, step + super::HOLD_REPEAT * 3);
    assert_eq!(shown(&app)[0], 4, "and stops the repeat");
    assert!(!asks_timer(&mut app), "a released arrow asks for no more timers");
    assert_eq!(app.tabs.cur, 0, "scrolling never switches tabs");

    arrow_click(&mut app, true);
    assert_eq!(shown(&app)[0], 5, "a click is one step");
    app.on_event(&Event::MouseDown { x, y, button: MouseButton::Left, clicks: 1 }, t);
    let mut at = t + super::HOLD_DELAY;
    for _ in 0..40 {
        timer(&mut app, at);
        at += super::HOLD_REPEAT;
    }
    assert_eq!(shown(&app).last(), Some(&29));
    app.drain_effects().for_each(drop);
    timer(&mut app, at);
    assert!(!asks_timer(&mut app), "at the end no more frames or timers are asked for");
    app.on_event(&Event::MouseUp { x, y, button: MouseButton::Left }, at);
}

#[test]
fn a_file_dragged_over_an_arrow_scrolls_the_strip_but_not_over_a_tab() {
    let mut app = many(30, 2000);
    app.select_tab(0);
    app.layout();
    let all = drop_effect::COPY | drop_effect::MOVE;
    let (x, y) = centre(app.layout.tab_arrows[1]);
    let paths = vec![FsPath::parse(r"D:\docs\a.txt").unwrap()];
    let t = Instant::now();
    app.drag(&DragEvent::Enter { paths, x, y, mods: NO, allowed: all, right: false }, t);
    app.layout();
    assert_eq!(shown(&app)[0], 1, "the strip moves as the file arrives over the arrow");
    app.drag(&DragEvent::Over { x, y, mods: NO, allowed: all }, t + Duration::from_millis(50));
    app.layout();
    assert_eq!(shown(&app)[0], 1, "repeated drag-over events do not race ahead");
    timer(&mut app, t + super::HOLD_DELAY);
    assert_eq!(shown(&app)[0], 2, "the strip keeps scrolling while the file stays there");
    let (tx, ty) = tab_xy(&app, 5);
    app.drag(&DragEvent::Over { x: tx, y: ty, mods: NO, allowed: all }, t + super::HOLD_DELAY);
    timer(&mut app, t + super::HOLD_DELAY + super::HOLD_REPEAT);
    assert_eq!(shown(&app)[0], 2, "over a tab the strip stays");
    assert_eq!(app.drop_spot(), Some(Spot::Tab(5)), "and the tab is the drop target");
    app.drag(&DragEvent::Over { x, y, mods: NO, allowed: all }, t + Duration::from_secs(1));
    app.layout();
    assert_eq!(shown(&app)[0], 3);
    app.drag(&DragEvent::Leave, t + Duration::from_secs(1));
    timer(&mut app, t + Duration::from_secs(2));
    assert_eq!(shown(&app)[0], 3, "leaving the window stops the scrolling");
    assert_eq!(names(&app).len(), 30, "nothing was opened");
}

#[test]
fn a_narrow_window_keeps_the_arrows_and_new_tab_clear_of_the_window_buttons() {
    for (width, arrows) in [(520, false), (640, true), (900, true), (1300, true)] {
        let mut app = many(20, width);
        app.select_tab(10);
        app.layout();
        let l = &app.layout;
        let limit = if l.actions.w > 0.0 { l.actions.x } else { app.tabs.panels.caption[0].x };
        assert_eq!(l.tab_arrows[0].w > 0.0, arrows, "{width}: arrows only where a tab still fits between them");
        assert!((!arrows || l.tab_arrows[0].x >= l.title.x) && l.new_tab.right() <= limit, "{width}: {:?} {:?} {limit}", l.tab_arrows, l.new_tab);
        assert!(l.tabs.iter().all(|(r, _)| r.w == 0.0 || !arrows || (r.x >= l.tab_arrows[0].right() && r.right() <= l.tab_arrows[1].x)), "{width}: tabs stay between the arrows");
        assert!(shown(&app).contains(&10), "{width}: the current tab is in view");
    }
}

fn strip_end(l: &crate::ui::Layout) -> f32 {
    let tabs = l.tabs.iter().map(|(r, _)| r.right());
    tabs.chain([l.new_tab.right(), l.actions.right(), l.tab_arrows[1].right()]).fold(0.0, f32::max)
}

#[test]
fn while_a_new_panel_opens_the_left_strip_stays_clear_of_the_window_buttons() {
    let mut app = many(30, 2000);
    app.animations = true;
    let mut g = Gfx::new(crate::text::tests::Mono, 1024);
    let t = Instant::now();
    app.frame(&mut g, t);
    let caption = app.tabs.panels.caption[0].x;
    assert!(strip_end(&app.layout) <= caption, "one panel ends its strip before the window buttons");
    key(&mut app, Key::Char('S'), Mods { ctrl: true, shift: true, alt: false });
    assert!(app.tabs.panels.growing.is_some(), "the new panel grows in");
    for k in [0, 16, 50, 100, 400] {
        app.frame(&mut g, ms(t, k));
        let left = app.tabs.panels.list.iter().find(|p| p.id == 0).unwrap();
        assert!(k > 0 || left.rect.right() > caption, "at first the left panel still reaches under the buttons");
        assert!(strip_end(&left.layout) <= caption, "{k} ms: the left strip ends at {} past the buttons at {caption}", strip_end(&left.layout));
    }
    assert!(app.tabs.panels.growing.is_none());
    let left = app.tabs.panels.list.iter().find(|p| p.id == 0).unwrap();
    let s4 = app.s(4.0);
    assert_eq!(left.layout.actions.right(), left.rect.right() - s4, "once the panel stops short of the buttons its strip uses its whole width");
    assert!(strip_end(&app.layout) <= caption, "the right panel keeps clear of the buttons");
    app.close_panel();
    let t = Instant::now();
    for k in [0, 16, 50, 400] {
        app.frame(&mut g, ms(t, k));
        assert!(strip_end(&app.layout) <= caption, "{k} ms into closing: the strip ends at {}", strip_end(&app.layout));
    }
}

fn outlines(app: &App, g: &Gfx<crate::text::tests::Mono>, title: Rect) -> Vec<(Rect, u32)> {
    let sep = app.look.palette.base.separator.0;
    let mut v: Vec<(Rect, u32)> =
        g.dl.instances
            .iter()
            .filter(|i| i.kind == crate::render::kind::SOLID && i.border > 0.0 && i.rect[2] > 20.0 && i.color & 0x00FF_FFFF == sep & 0x00FF_FFFF)
            .filter(|i| i.rect[1] >= title.y && i.rect[1] < title.bottom() && i.rect[0] >= title.x && i.rect[0] < title.right())
            .map(|i| (Rect::new(i.rect[0], i.rect[1], i.rect[2], i.rect[3]), i.color >> 24))
            .collect();
    v.sort_by(|a, b| a.0.x.total_cmp(&b.0.x));
    v
}

fn ms(t: Instant, n: u64) -> Instant {
    t + Duration::from_millis(n)
}

#[test]
fn a_closed_tab_shrinks_and_fades_while_its_right_neighbour_slides_into_its_place() {
    let mut app = app_with(&[r"C:\a", r"C:\b", r"C:\c"]);
    let mut g = Gfx::new(crate::text::tests::Mono, 1024);
    let t0 = Instant::now();
    app.select_tab(1);
    app.frame(&mut g, ms(t0, 1000));
    let (old1, old2) = (app.layout.tabs[1].0, app.layout.tabs[2].0);
    let full = app.look.palette.base.separator.0 >> 24;
    key(&mut app, Key::Char('W'), CTRL);
    assert_eq!(names(&app), [r"C:\a", r"C:\c"]);
    let t1 = ms(t0, 2000);
    assert!(app.frame(&mut g, t1), "the close animates");
    let title = app.layout.title;
    let o = outlines(&app, &g, title);
    assert_eq!(o.iter().map(|o| (o.0.x, o.1)).collect::<Vec<_>>(), [(old1.x, full), (old2.x, full)], "at first the closed tab is where it was and its neighbour has not moved");
    assert_eq!(app.tabs.strip.ghost_count(app.tabs.panels.here), 1);

    assert!(app.frame(&mut g, ms(t1, 60)));
    let o = outlines(&app, &g, title);
    assert_eq!(o.len(), 2, "{o:?}");
    let ghost = o.iter().find(|o| o.1 < full).expect("a fading ghost");
    let live = o.iter().find(|o| o.1 == full).expect("the new current tab");
    assert!(ghost.0.w < old1.w && ghost.0.x == old1.x, "the ghost shrinks towards its left edge: {ghost:?}");
    assert!(live.0.x > old1.x && live.0.x < old2.x, "the neighbour is on its way: {live:?}");

    assert!(!app.frame(&mut g, ms(t1, 200)), "the strip settles and asks for no more frames");
    let o = outlines(&app, &g, title);
    assert_eq!(o.iter().map(|o| (o.0.x, o.1)).collect::<Vec<_>>(), [(old1.x, full)]);
    assert_eq!(app.tabs.strip.ghost_count(app.tabs.panels.here), 0);
}

#[test]
fn without_animations_a_closed_tab_goes_at_once_and_no_frames_are_asked_for() {
    let mut app = app_with(&[r"C:\a", r"C:\b", r"C:\c"]);
    app.animations = false;
    let mut g = Gfx::new(crate::text::tests::Mono, 1024);
    let t0 = Instant::now();
    app.select_tab(1);
    app.frame(&mut g, ms(t0, 1000));
    let old1 = app.layout.tabs[1].0;
    key(&mut app, Key::Char('W'), CTRL);
    assert!(!app.frame(&mut g, ms(t0, 2000)));
    let o = outlines(&app, &g, app.layout.title);
    assert_eq!(o.iter().map(|o| o.0.x).collect::<Vec<_>>(), [old1.x], "the neighbour is in place on the first frame");
    assert_eq!(app.tabs.strip.ghost_count(app.tabs.panels.here), 0);
    assert!(!app.motion);
}

#[test]
fn during_the_close_animation_clicks_land_on_the_final_layout() {
    let mut app = app_with(&[r"C:\a", r"C:\b", r"C:\c", r"C:\d"]);
    let mut g = Gfx::new(crate::text::tests::Mono, 1024);
    let t0 = Instant::now();
    app.select_tab(3);
    app.frame(&mut g, ms(t0, 1000));
    let (x, y) = tab_xy(&app, 1);
    ev(&mut app, Event::MouseDown { x, y, button: MouseButton::Middle, clicks: 1 });
    ev(&mut app, Event::MouseUp { x, y, button: MouseButton::Middle });
    assert_eq!(names(&app), [r"C:\a", r"C:\c", r"C:\d"]);
    let t1 = ms(t0, 2000);
    app.frame(&mut g, t1);
    assert!(app.frame(&mut g, ms(t1, 50)), "still moving");
    assert_eq!(app.tabs.strip.ghost_count(app.tabs.panels.here), 1, "a middle click animates too");
    let r = app.layout.tabs[1].0;
    let (x, y) = (r.x + 4.0, r.y + r.h / 2.0);
    ev(&mut app, Event::MouseDown { x, y, button: MouseButton::Left, clicks: 1 });
    ev(&mut app, Event::MouseUp { x, y, button: MouseButton::Left });
    assert_eq!(app.tabs.cur, 1, "the click where C now stands selects it, the ghost of B is not hit");
    assert_eq!(names(&app), [r"C:\a", r"C:\c", r"C:\d"]);
    assert_eq!(app.drag, Drag::None);
}

#[test]
fn quick_closes_each_leave_a_ghost_and_slides_go_on_from_where_tabs_are_drawn() {
    let mut app = app_with(&[r"C:\a", r"C:\b", r"C:\c", r"C:\d"]);
    let mut g = Gfx::new(crate::text::tests::Mono, 1024);
    let t0 = Instant::now();
    app.select_tab(1);
    app.frame(&mut g, ms(t0, 1000));
    let (old1, old3) = (app.layout.tabs[1].0, app.layout.tabs[3].0);
    let full = app.look.palette.base.separator.0 >> 24;
    key(&mut app, Key::Char('W'), CTRL);
    let t1 = ms(t0, 2000);
    app.frame(&mut g, t1);
    app.frame(&mut g, ms(t1, 30));
    let title = app.layout.title;
    let c_at = outlines(&app, &g, title).into_iter().find(|o| o.1 == full).unwrap().0.x;
    key(&mut app, Key::Char('W'), CTRL);
    assert_eq!(names(&app), [r"C:\a", r"C:\d"]);
    assert!(app.frame(&mut g, ms(t1, 40)));
    assert_eq!(app.tabs.strip.ghost_count(app.tabs.panels.here), 2, "both closed tabs fade on their own");
    let o = outlines(&app, &g, title);
    assert_eq!(o.len(), 3, "{o:?}");
    assert!(o.iter().any(|o| o.0.x == c_at && o.1 == full), "C's ghost starts where C was drawn: {o:?} {c_at}");
    assert!(o.iter().any(|o| o.0.x == old1.x && o.1 < full), "B's ghost keeps fading");
    let d = o.iter().filter(|o| o.1 == full).map(|o| o.0.x).fold(f32::MIN, f32::max);
    assert!(d < old3.x && d > old1.x, "D goes on from where it was drawn, not from its old place: {d} {old3:?}");
    assert!(!app.frame(&mut g, ms(t1, 400)));
    assert_eq!(app.tabs.strip.ghost_count(app.tabs.panels.here), 0);
}

#[test]
fn closing_a_tab_in_one_panel_leaves_the_other_panel_strip_still() {
    let mut app = app_with(&[r"C:\a", r"C:\b"]);
    let mut g = Gfx::new(crate::text::tests::Mono, 1024);
    key(&mut app, Key::Char('S'), Mods { ctrl: true, shift: true, alt: false });
    let (first, second) = (app.parked_ids()[0], app.tabs.panels.active);
    app.run_action("tab.new", Instant::now());
    app.run_action("tab.new", Instant::now());
    app.select_tab(1);
    let t0 = Instant::now();
    app.frame(&mut g, ms(t0, 1000));
    let other = app.tabs.panels.list.iter().find(|p| p.id == first).unwrap().layout.title;
    let before = outlines(&app, &g, other);
    assert_eq!(before.len(), 1);
    let old1 = app.layout.tabs[1].0;
    key(&mut app, Key::Char('W'), CTRL);
    let t1 = ms(t0, 2000);
    app.frame(&mut g, t1);
    assert!(app.frame(&mut g, ms(t1, 60)));
    assert_eq!(outlines(&app, &g, other), before, "the other panel's tabs do not move or fade");
    assert_eq!(app.tabs.strip.ghost_count(first), 0);
    assert_eq!(app.tabs.strip.ghost_count(second), 1);
    assert!(outlines(&app, &g, app.layout.title).iter().any(|o| o.0.x == old1.x && o.0.w < old1.w), "the closed tab fades in its own panel");
}

#[test]
fn closing_a_tab_of_an_overflowing_strip_animates_between_the_arrows() {
    let mut app = many(30, 2000);
    let mut g = Gfx::new(crate::text::tests::Mono, 1024);
    app.select_tab(2);
    let t0 = Instant::now();
    app.frame(&mut g, ms(t0, 1000));
    let [left, right] = app.layout.tab_arrows;
    assert!(left.w > 0.0);
    let (old2, old3) = (app.layout.tabs[2].0, app.layout.tabs[3].0);
    key(&mut app, Key::Char('W'), CTRL);
    let t1 = ms(t0, 2000);
    app.frame(&mut g, t1);
    assert!(app.frame(&mut g, ms(t1, 60)));
    assert_eq!(app.layout.tab_arrows, [left, right], "the arrows stay put");
    let o = outlines(&app, &g, app.layout.title);
    assert_eq!(o.len(), 2, "{o:?}");
    assert!(o.iter().all(|o| o.0.x >= left.right() && o.0.right() <= right.x), "{o:?}");
    assert!(o.iter().any(|o| o.0.x > old2.x && o.0.x < old3.x), "the next tab slides in: {o:?}");
    assert!(!app.frame(&mut g, ms(t1, 300)));
    assert_eq!(outlines(&app, &g, app.layout.title).iter().map(|o| o.0.x).collect::<Vec<_>>(), [old2.x]);
}

fn drawn(app: &App, i: usize) -> Rect {
    app.tabs.strip.drawn(app.tabs.panels.here, crate::ui::tabbar::Part::Tab(app.tabs.items[i].id)).expect("drawn")
}

fn plus_drawn(app: &App) -> Rect {
    app.tabs.strip.drawn(app.tabs.panels.here, crate::ui::tabbar::Part::Plus).expect("drawn")
}

#[test]
fn a_new_tab_pushes_the_tabs_to_its_right_aside_and_clicks_use_the_final_places() {
    let mut app = app_with(&[r"C:\a", r"C:\b", r"C:\c"]);
    let mut g = Gfx::new(crate::text::tests::Mono, 1024);
    let t0 = Instant::now();
    app.select_tab(0);
    app.frame(&mut g, ms(t0, 1000));
    let (old_b, old_c, old_plus) = (app.layout.tabs[1].0, app.layout.tabs[2].0, app.layout.new_tab);
    key(&mut app, Key::Char('T'), CTRL);
    assert_eq!((app.tabs.cur, names(&app)), (1, vec![r"C:\a".to_owned(), r"C:\a".to_owned(), r"C:\b".to_owned(), r"C:\c".to_owned()]));
    let t1 = ms(t0, 2000);
    assert!(app.frame(&mut g, t1));
    assert_eq!((drawn(&app, 2).x, drawn(&app, 3).x, plus_drawn(&app).x), (old_b.x, old_c.x, old_plus.x), "at first the tabs on the right stay where they were");

    assert!(app.frame(&mut g, ms(t1, 60)));
    let (b, c, plus) = (drawn(&app, 2), drawn(&app, 3), plus_drawn(&app));
    assert!(b.x > old_b.x && b.x < app.layout.tabs[2].0.x, "B is on its way right: {b:?}");
    assert!(c.x > old_c.x && c.x < app.layout.tabs[3].0.x, "C is on its way right: {c:?}");
    assert!(plus.x > old_plus.x && plus.x < app.layout.new_tab.x, "and so is the new tab button: {plus:?}");
    let r = app.layout.tabs[3].0;
    let (x, y) = (r.right() - 3.0, r.y + r.h / 2.0);
    assert!(!c.contains(x, y), "the click below lands where C is not drawn yet");
    ev(&mut app, Event::MouseDown { x, y, button: MouseButton::Left, clicks: 1 });
    ev(&mut app, Event::MouseUp { x, y, button: MouseButton::Left });
    assert_eq!(app.tabs.cur, 3, "a click on C's final place selects C");

    assert!(!app.frame(&mut g, ms(t1, 300)), "the strip settles");
    assert_eq!((drawn(&app, 2), drawn(&app, 3)), (app.layout.tabs[2].0, app.layout.tabs[3].0));
}

#[test]
fn without_animations_a_new_tab_moves_its_neighbours_at_once() {
    let mut app = app_with(&[r"C:\a", r"C:\b", r"C:\c"]);
    app.animations = false;
    let mut g = Gfx::new(crate::text::tests::Mono, 1024);
    let t0 = Instant::now();
    app.select_tab(0);
    app.frame(&mut g, ms(t0, 1000));
    key(&mut app, Key::Char('T'), CTRL);
    assert!(!app.frame(&mut g, ms(t0, 2000)), "no frames are asked for");
    assert!(!app.motion);
    assert_eq!(app.tabs.strip.drawn(app.tabs.panels.here, crate::ui::tabbar::Part::Tab(app.tabs.items[2].id)), None, "nothing is remembered for animation");
}

#[test]
fn dragging_a_tab_follows_the_pointer_while_neighbours_slide_into_the_insertion_slot() {
    let mut app = app_with(&[r"C:\a", r"C:\b", r"C:\c"]);
    let mut g = Gfx::new(crate::text::tests::Mono, 1024);
    let t0 = Instant::now();
    app.frame(&mut g, ms(t0, 1000));
    let (x0, y) = tab_xy(&app, 0);
    let r2 = app.layout.tabs[2].0;
    ev(&mut app, Event::MouseDown { x: x0, y, button: MouseButton::Left, clicks: 1 });
    ev(&mut app, Event::MouseMove { x: r2.x + r2.w * 0.75, y });
    assert_eq!(names(&app), [r"C:\b", r"C:\c", r"C:\a"]);
    assert!(app.frame(&mut g, ms(t0, 2000)));
    assert_eq!(drawn(&app, app.tabs.cur).x, r2.x + r2.w * 0.75 - (x0 - app.layout.tabs[0].0.x), "the grabbed point stays under the cursor");
    assert_ne!(drawn(&app, 0), app.layout.tabs[0].0, "neighbours animate to their new slots");
    assert!(app.insert_mark().is_some());
    app.frame(&mut g, ms(t0, 2070));
    let middle = drawn(&app, 0);
    assert!(middle.x > app.layout.tabs[0].0.x && middle.x < app.layout.tabs[1].0.x);
    assert_eq!(app.tabs.strip.ghost_count(app.tabs.panels.here), 0);
    ev(&mut app, Event::MouseUp { x: r2.x + r2.w * 0.75, y, button: MouseButton::Left });
    assert_eq!(app.drag, Drag::None);
    assert_eq!(app.insert_mark(), None);
    app.frame(&mut g, ms(t0, 2200));
    app.frame(&mut g, ms(t0, 2400));
    assert_eq!(drawn(&app, app.tabs.cur), app.layout.tabs[app.tabs.cur].0);
}

#[test]
fn scrolling_an_overflowing_strip_slides_the_tabs_and_brings_the_next_one_in_from_the_edge() {
    let mut app = many(30, 2000);
    let mut g = Gfx::new(crate::text::tests::Mono, 1024);
    app.select_tab(0);
    let t0 = Instant::now();
    app.frame(&mut g, ms(t0, 1000));
    let old: Vec<Rect> = app.layout.tabs.iter().map(|t| t.0).collect();
    let last = shown(&app).last().copied().unwrap();
    arrow_click(&mut app, true);
    assert_eq!(shown(&app)[0], 1);
    let t1 = ms(t0, 2000);
    assert!(app.frame(&mut g, t1));
    assert_eq!(drawn(&app, 1).x, old[1].x, "at first nothing jumps");
    assert!(app.frame(&mut g, ms(t1, 60)));
    let one = drawn(&app, 1);
    assert!(one.x < old[1].x && one.x > old[0].x, "the strip slides left: {one:?}");
    let gone = drawn(&app, 0);
    assert!(gone.w > 0.0 && gone.x < old[0].x, "the first tab slides out to the left: {gone:?}");
    let next = drawn(&app, last + 1);
    let target = app.layout.tabs[last + 1].0;
    assert!(next.w == target.w && next.x > target.x, "the next tab comes in from the right edge: {next:?} {target:?}");
    let [left, right] = app.layout.tab_arrows;
    let clip = |i: &crate::render::Instance| i.clip[0] >= left.right() - 0.5 && i.clip[2] <= right.x + 0.5;
    let title = app.layout.title;
    let lane: Vec<_> = g.dl.instances.iter().filter(|i| i.kind == crate::render::kind::GLYPH && i.rect[1] >= title.y && i.rect[1] < title.bottom() && i.rect[0] > left.right() && i.rect[0] < right.x).collect();
    assert!(!lane.is_empty() && lane.iter().all(|i| clip(i)), "moving tabs are clipped to the lane between the arrows");
    assert!(!app.frame(&mut g, ms(t1, 300)));
    assert_eq!(drawn(&app, 1), app.layout.tabs[1].0);
    assert_eq!(drawn(&app, 0).w, 0.0);
}
