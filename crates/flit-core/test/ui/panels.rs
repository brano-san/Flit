use super::*;
use crate::input::{CursorIcon, HitArea, Key, Mods, MouseButton};
use crate::listing::{attr, DirSnapshot, Listing, Meta};
use crate::path::FsPath;
use crate::provider::ProviderRequest;
use crate::sort::{self, CaseMode, SortKeys};
use crate::text::tests::Mono;
use crate::ui::harness::frame;
use crate::ui::TAB_W;
use crate::ui::{drop_effect, MenuItem, Persisted, WindowSession};
use std::sync::Arc;
use std::time::Duration;

const NO: Mods = Mods { ctrl: false, shift: false, alt: false };
const CTRL: Mods = Mods { ctrl: true, shift: false, alt: false };
const SHIFT: Mods = Mods { ctrl: false, shift: true, alt: false };
const CTRL_SHIFT: Mods = Mods { ctrl: true, shift: true, alt: false };

fn t0() -> Instant {
    Instant::now()
}

fn loc(s: &str) -> Location {
    Location::parse(s).unwrap()
}

fn app_at(path: &str) -> App {
    let mut app = App::new(Persisted { path: loc(path), ..Default::default() }, Vec::new(), 0, false, 1.0, (1200, 800), t0());
    app.drain_effects().for_each(drop);
    app
}

fn publish(t: &Tab, items: &[(&str, bool)]) {
    let mut listing = Listing::default();
    let mut keys = SortKeys::new(CaseMode::Insensitive);
    for &(n, dir) in items {
        let i = listing.push(n.as_bytes(), Meta { attrs: if dir { attr::DIR } else { 0 }, ..Default::default() });
        keys.push(listing.name(i));
    }
    let mut order = Vec::new();
    sort::sort(&listing, &keys, t.view_spec, &mut order);
    let snap = DirSnapshot { gen: t.gen, path: t.location().path().clone(), listing, keys, order, view: t.view_spec, done: true, error: None, elapsed: Duration::ZERO };
    t.slot.publish(Arc::new(snap));
}

fn deliver(app: &mut App, items: &[(&str, bool)]) {
    publish(app.tab(), items);
    app.poll();
}

fn key(app: &mut App, key: Key, mods: Mods) {
    app.on_event(&Event::Key { key, mods, pressed: true }, t0());
    app.on_event(&Event::Key { key, mods: NO, pressed: false }, t0());
}

fn ev(app: &mut App, e: Event) {
    app.on_event(&e, t0());
}

fn listed(app: &mut App) -> Vec<(TabId, Location)> {
    app.drain_effects().filter_map(|e| if let Effect::Provider { req: ProviderRequest::List { tab, path, .. }, .. } = e { Some((tab, Location::Local(path))) } else { None }).collect()
}

fn tree(n: &Node) -> String {
    match n {
        Node::Leaf(id) => id.to_string(),
        Node::Group { down, kids } => format!("({})", kids.iter().map(|(k, _)| tree(k)).collect::<Vec<_>>().join(if *down { "/" } else { "|" })),
    }
}

fn shape(app: &App) -> String {
    tree(&app.panels.root)
}

fn rect(app: &App, id: u32) -> Rect {
    app.panels.list.iter().find(|p| p.id == id).unwrap().rect
}

fn layout_of(app: &App, id: u32) -> &Layout {
    if id == app.panels.here { &app.layout } else { &app.panels.list.iter().find(|p| p.id == id).unwrap().layout }
}

fn row_in(l: &Layout, row: usize) -> (f32, f32) {
    (l.cols[0].x + 60.0, l.list.y + (row as f32 + 0.5) * l.row_h)
}

fn three(path: &str) -> App {
    let mut app = app_at(path);
    key(&mut app, Key::Char('S'), CTRL_SHIFT);
    key(&mut app, Key::Char('S'), CTRL);
    app.drain_effects().for_each(drop);
    app
}

fn labels(items: &[MenuItem]) -> Vec<String> {
    items
        .iter()
        .flat_map(|i| match i {
            MenuItem::Command { label, .. } => vec![label.clone()],
            MenuItem::Submenu { items, .. } => labels(items),
            MenuItem::Header(_) | MenuItem::Separator => Vec::new(),
        })
        .collect()
}

fn menu(app: &mut App) -> Vec<String> {
    let m = app.drain_effects().find_map(|e| if let Effect::ShellMenu(m) = e { Some(m.menu) } else { None }).expect("menu shown");
    labels(&m)
}

#[test]
fn split_right_and_bottom_build_a_tree_and_move_the_focus_to_the_new_panel() {
    let mut app = app_at(r"C:\a");
    deliver(&mut app, &[("sub", true)]);
    let first = app.tab().id;
    key(&mut app, Key::Char('S'), CTRL_SHIFT);
    assert_eq!(shape(&app), "(0|1)");
    assert_eq!(app.panels.active, 1);
    assert_ne!(app.tab().id, first);
    assert_eq!(app.tab().location(), &loc(r"C:\a"), "the new panel opens the current folder");
    assert_eq!(app.parked_tabs().map(|t| (t.id, t.rows())).collect::<Vec<_>>(), [(first, 1)], "the first panel keeps its listing");
    assert_eq!(listed(&mut app), [(app.tab().id, loc(r"C:\a"))]);
    assert!(rect(&app, 1).x > rect(&app, 0).right(), "the new panel is on the right");
    key(&mut app, Key::Char('S'), CTRL);
    assert_eq!(shape(&app), "(0|(1/2))");
    assert!(rect(&app, 2).y > rect(&app, 1).bottom() - 1.0 && rect(&app, 2).x == rect(&app, 1).x);
    key(&mut app, Key::Char('S'), CTRL);
    assert_eq!(shape(&app), "(0|(1/2/3))", "a split along the group adds a sibling right after the panel");
    key(&mut app, Key::Char('S'), CTRL_SHIFT);
    assert_eq!(shape(&app), "(0|(1/2/(3|4)))");
    app.activate_panel(0);
    key(&mut app, Key::Char('S'), CTRL_SHIFT);
    assert_eq!(shape(&app), "(0|5|(1/2/(3|4)))");
    assert_eq!(app.panel_count(), 6);
}

#[test]
fn every_panel_has_its_own_tab_row_and_only_the_active_one_a_status_bar() {
    let mut app = three(r"C:\a");
    frame(&mut app);
    let (a, b, c) = (layout_of(&app, 0).clone(), layout_of(&app, 1).clone(), layout_of(&app, 2).clone());
    assert_eq!((a.title.y, b.title.y), (0.0, 0.0), "the top panels' tab rows are the title row");
    assert_eq!(c.title.y, rect(&app, 2).y);
    assert!(a.tabs[0].0.w > 0.0 && b.tabs[0].0.w > 0.0 && c.tabs[0].0.w > 0.0);
    assert_eq!(a.tabs[0].0.w, app.s(TAB_W), "tabs have one fixed width");
    let cap = app.panels.caption;
    assert!(cap[0].x >= b.actions.right() && cap[2].right() == 1200.0 && b.title.contains(cap[1].x + 1.0, 5.0), "the captions end the top right panel's row");
    assert!(a.actions.right() <= rect(&app, 0).right());
    assert_eq!(app.panels.active, 2);
    assert!(app.layout.status.h > 0.0 && a.status.h == 0.0 && b.status.h == 0.0);
    assert_eq!(app.layout.status.bottom(), 800.0);
    let side = app.nav.sidebar.rect;
    assert_eq!((side.y, side.bottom()), (app.s(TAB_H), 800.0), "Quick Access runs from below the title row to the bottom");
    assert!(app.panels.menu.right() <= side.right() && app.panels.quick.right() <= side.right(), "the menu and the toggle sit above Quick Access");
    assert_eq!(rect(&app, 0).x, side.right());
    assert_eq!(app.hit_test(b.new_tab.right() + 30.0, 10.0), HitArea::Caption);
    assert_eq!(app.hit_test(a.new_tab.right() + 30.0, 10.0), HitArea::Caption);
    assert_eq!(app.hit_test(a.tabs[0].0.x + 20.0, 10.0), HitArea::Client, "a tab of an inactive top panel is not a drag area");
    assert_eq!(app.hit_test((app.panels.menu.right() + app.panels.quick.x) / 2.0, 10.0), HitArea::Caption, "so is the space between the menu and the toggle");
    assert_eq!(app.hit_test(app.panels.menu.x + 5.0, 10.0), HitArea::Client);
    assert_eq!(app.hit_test(c.new_tab.right() + 30.0, c.title.y + 10.0), HitArea::Client, "a lower panel's tab row is not a drag area");
    assert_eq!(app.hit_test(cap[2].x + 5.0, 5.0), HitArea::Close);
    key(&mut app, Key::Char('B'), CTRL);
    assert_eq!(app.nav.sidebar.rect.w, 0.0);
    assert_eq!(rect(&app, 0).x, 0.0);
    assert!(layout_of(&app, 0).tabs[0].0.x > app.panels.quick.right(), "tabs start after the menu and the toggle");
}

#[test]
fn clicks_tab_and_directions_select_panels() {
    let mut app = three(r"C:\a");
    frame(&mut app);
    key(&mut app, Key::Tab, NO);
    assert_eq!(app.panels.active, 0, "Tab wraps around the panels");
    assert!(app.drain_effects().any(|e| matches!(e, Effect::Title(_))));
    key(&mut app, Key::Tab, SHIFT);
    assert_eq!(app.panels.active, 2);
    key(&mut app, Key::Tab, SHIFT);
    assert_eq!(app.panels.active, 1);
    app.run_action("panel.below", t0());
    assert_eq!(app.panels.active, 2);
    app.run_action("panel.left", t0());
    assert_eq!(app.panels.active, 0);
    app.run_action("panel.left", t0());
    assert_eq!(app.panels.active, 0, "nothing on the left");
    app.run_action("panel.right", t0());
    assert_eq!(app.panels.active, 1, "the closest panel on the right overlapping most");
    app.run_action("panel.above", t0());
    assert_eq!(app.panels.active, 1);
    let r = rect(&app, 2);
    let (x, y) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
    ev(&mut app, Event::MouseDown { x, y, button: MouseButton::Left, clicks: 1 });
    ev(&mut app, Event::MouseUp { x, y, button: MouseButton::Left });
    assert_eq!(app.panels.active, 2);
    app.focus = Focus::Nav;
    key(&mut app, Key::Tab, NO);
    assert_eq!(app.panels.active, 2, "Tab outside the list keeps moving the keyboard focus");
    assert_ne!(app.focus, Focus::Nav);
}

#[test]
fn a_click_in_an_inactive_panel_activates_it_and_lands_on_its_row() {
    let mut app = app_at(r"C:\a");
    deliver(&mut app, &[("sub", true), ("a.txt", false), ("b.txt", false)]);
    key(&mut app, Key::Char('S'), CTRL_SHIFT);
    deliver(&mut app, &[("sub", true), ("a.txt", false), ("b.txt", false)]);
    frame(&mut app);
    let (left, right) = (app.parked_tabs().next().unwrap().id, app.tab().id);
    let (x, y) = row_in(layout_of(&app, 0), 2);
    ev(&mut app, Event::MouseMove { x, y });
    ev(&mut app, Event::MouseDown { x, y, button: MouseButton::Left, clicks: 1 });
    ev(&mut app, Event::MouseUp { x, y, button: MouseButton::Left });
    assert_eq!(app.tab().id, left);
    assert_eq!(app.parked_tabs().next().unwrap().id, right);
    let v = app.tab().view.as_ref().unwrap();
    assert_eq!(v.selected().map(|i| v.listing().name(i).to_vec()).collect::<Vec<_>>(), [b"b.txt".to_vec()]);
    let tab = layout_of(&app, 1).tabs[0].0;
    ev(&mut app, Event::MouseMove { x: tab.x + 20.0, y: tab.y + 10.0 });
    assert_eq!(app.panels.hover, Some((1, Hit::Tab(0))), "tabs of an inactive panel react to the pointer");
}

#[test]
fn folders_open_in_new_panels_from_the_keyboard_and_menus() {
    let mut app = app_at(r"C:\a");
    deliver(&mut app, &[("sub", true), ("a.txt", false)]);
    key(&mut app, Key::Down, NO);
    key(&mut app, Key::Enter, CTRL_SHIFT);
    assert_eq!(app.tab().location(), &loc(r"C:\a\sub"));
    assert_eq!(shape(&app), "(0|1)");
    app.activate_panel(0);
    app.context_menu();
    let m = menu(&mut app);
    assert!(m.contains(&"Open In Right Split".into()) && m.contains(&"Open In Bottom Split".into()), "{m:?}");
    app.run_action("panel.open_bottom", t0());
    assert_eq!(shape(&app), "((0/2)|1)");
    assert_eq!(app.tab().location(), &loc(r"C:\a\sub"));
    app.panel_quick_menu();
    let m = menu(&mut app);
    assert!(m.contains(&"Split Right".into()) && m.contains(&"Split Bottom".into()), "{m:?}");
    let hide = &crate::ui::SETTINGS[crate::ui::settings::index("general.menu_panes").unwrap()];
    assert!(hide.set(&mut app, crate::ui::Value::Bool(false)));
    key(&mut app, Key::Escape, NO);
    app.context_menu();
    let m = menu(&mut app);
    assert!(!m.contains(&"Split Right".into()), "the setting hides the panel commands: {m:?}");
}

#[test]
fn closing_the_last_tab_closes_the_panel_and_the_last_panel_closes_the_window() {
    let mut app = three(r"C:\a");
    let gone = app.tab().id;
    key(&mut app, Key::Char('T'), CTRL);
    assert_eq!(app.tabs().len(), 2);
    key(&mut app, Key::Char('W'), CTRL);
    assert_eq!(shape(&app), "(0|(1/2))", "a panel with tabs left stays");
    key(&mut app, Key::Char('W'), CTRL);
    assert_eq!(shape(&app), "(0|1)", "the group of one collapses");
    assert_eq!(app.panels.active, 1);
    assert!(app.drain_effects().any(|e| matches!(e, Effect::Provider { req: ProviderRequest::Watch { tab, path: None, .. }, .. } if tab == gone)));
    key(&mut app, Key::Char('S'), CTRL);
    key(&mut app, Key::Char('T'), CTRL);
    key(&mut app, Key::Char('W'), CTRL_SHIFT);
    assert_eq!(shape(&app), "(0|1)", "Close All Tabs closes the panel with all its tabs");
    assert!(app.nav.closed.len() >= 2);
    app.activate_panel(0);
    key(&mut app, Key::Char('W'), CTRL);
    assert_eq!(shape(&app), "1");
    assert!(!app.drain_effects().any(|e| matches!(e, Effect::Window(WindowCmd::Close))));
    key(&mut app, Key::Char('W'), CTRL);
    assert!(app.drain_effects().any(|e| matches!(e, Effect::Window(WindowCmd::Close))));
}

#[test]
fn separators_drag_with_a_minimum_and_a_double_click_centres_them() {
    let mut app = app_at(r"C:\a");
    key(&mut app, Key::Char('S'), CTRL_SHIFT);
    frame(&mut app);
    let bar = app.panels.seps[0].rect;
    let (x, y) = (bar.x, 400.0);
    ev(&mut app, Event::MouseMove { x, y });
    assert!(app.drain_effects().any(|e| matches!(e, Effect::Cursor(CursorIcon::ResizeHorizontal))));
    let rev = app.session_rev();
    ev(&mut app, Event::MouseDown { x, y, button: MouseButton::Left, clicks: 1 });
    ev(&mut app, Event::MouseMove { x: x - 100.0, y });
    assert!((rect(&app, 0).w - (bar.x - rect(&app, 0).x - 100.0)).abs() <= 1.0);
    ev(&mut app, Event::MouseMove { x: 0.0, y });
    assert_eq!(rect(&app, 0).w, app.s(75.0), "a panel keeps its minimum size");
    ev(&mut app, Event::MouseUp { x: 0.0, y, button: MouseButton::Left });
    assert!(app.session_rev() > rev);
    let bar = app.panels.seps[0].rect;
    ev(&mut app, Event::MouseDown { x: bar.x, y, button: MouseButton::Left, clicks: 2 });
    ev(&mut app, Event::MouseUp { x: bar.x, y, button: MouseButton::Left });
    assert!((rect(&app, 0).w - rect(&app, 1).w).abs() <= 1.0, "{} vs {}", rect(&app, 0).w, rect(&app, 1).w);
}

#[test]
fn even_siblings_and_even_all_reset_the_sizes() {
    let mut app = three(r"C:\a");
    app.set_sep(1, 100.0);
    app.set_sep(0, 300.0);
    assert!((rect(&app, 1).h - 100.0).abs() <= 1.0 && (rect(&app, 0).w - 300.0).abs() <= 1.0);
    app.run_action("panel.even_siblings", t0());
    assert!((rect(&app, 1).h - rect(&app, 2).h).abs() <= 1.0);
    assert!((rect(&app, 0).w - 300.0).abs() <= 1.0, "only the group of the active panel");
    app.set_sep(1, 100.0);
    app.run_action("panel.even_all", t0());
    assert!((rect(&app, 1).h - rect(&app, 2).h).abs() <= 1.0 && (rect(&app, 0).w - rect(&app, 1).w).abs() <= 1.0);
}

#[test]
fn shift_wheel_moves_the_separator_of_the_innermost_group_of_the_panel() {
    let mut app = three(r"C:\a");
    frame(&mut app);
    let sizes = |a: &App| (rect(a, 0).w, rect(a, 1).h);
    let wheel = |a: &mut App, id: u32, dy: f32| {
        let r = rect(a, id);
        ev(a, Event::MouseMove { x: r.x + r.w / 2.0, y: r.y + r.h / 2.0 });
        a.mods = SHIFT;
        ev(a, Event::Wheel { dx: 0.0, dy });
        a.mods = NO;
    };
    let before = sizes(&app);
    wheel(&mut app, 0, -1.0);
    assert!(sizes(&app).0 > before.0 && sizes(&app).1 == before.1, "over A the A|B separator moves");
    let before = sizes(&app);
    wheel(&mut app, 1, -1.0);
    assert!(sizes(&app).1 > before.1 && sizes(&app).0 == before.0, "over B the B/C separator moves");
    let before = sizes(&app);
    wheel(&mut app, 2, 1.0);
    assert!(sizes(&app).1 < before.1 && sizes(&app).0 == before.0, "over C too");
    assert_eq!(app.panels.active, 2, "the wheel does not change the active panel");
}

#[test]
fn listings_and_events_reach_tabs_of_inactive_panels() {
    let mut app = app_at(r"C:\a");
    key(&mut app, Key::Char('S'), CTRL_SHIFT);
    key(&mut app, Key::Char('S'), CTRL);
    deliver(&mut app, &[("x", false)]);
    let parked: Vec<TabId> = app.parked_tabs().map(|t| t.id).collect();
    assert_eq!(parked.len(), 2);
    for t in app.parked_tabs() {
        publish(t, &[("one", false), ("two", false)]);
    }
    app.poll();
    assert!(app.parked_tabs().all(|t| t.rows() == 2) && app.tab().rows() == 1);
    let (id, gen) = app.parked_tabs().map(|t| (t.id, t.gen)).next().unwrap();
    app.drain_effects().for_each(drop);
    app.on_provider_event(ProviderEvent::Rescan { tab: id, gen }, t0());
    assert_eq!(listed(&mut app), [(id, loc(r"C:\a"))]);
    assert!(!app.drain_effects().any(|e| matches!(e, Effect::Title(_))));
    let t = app.parked_tabs().find(|t| t.id == id).unwrap();
    publish(t, &[("one", false), ("two", false), ("three", false)]);
    app.poll();
    assert_eq!(app.parked_tabs().find(|t| t.id == id).unwrap().rows(), 3);
    let mut g = Gfx::new(Mono, 2048);
    app.frame(&mut g, t0());
    let gens: Vec<u64> = app.parked_tabs().map(|t| t.gen).collect();
    assert_eq!(app.parked_rows().map(|r| r.0).collect::<Vec<_>>(), gens);
    let all = g.dl.instances.len();
    app.run_action("tab.close_all", t0());
    app.run_action("tab.close_all", t0());
    app.frame(&mut g, t0());
    assert!(g.dl.instances.len() < all, "inactive panels are drawn");
    assert_eq!(app.parked_rows().count(), 0);
}

#[test]
fn the_wheel_scrolls_an_inactive_panel_without_activating_it() {
    let mut app = app_at(r"C:\a");
    key(&mut app, Key::Char('S'), CTRL_SHIFT);
    let names: Vec<String> = (0..300).map(|i| format!("f{i}.txt")).collect();
    let items: Vec<(&str, bool)> = names.iter().map(|n| (n.as_str(), false)).collect();
    deliver(&mut app, &items);
    publish(app.parked_tabs().next().unwrap(), &items);
    app.poll();
    frame(&mut app);
    let (x, y) = row_in(layout_of(&app, 0), 3);
    ev(&mut app, Event::MouseMove { x, y });
    ev(&mut app, Event::Wheel { dx: 0.0, dy: -3.0 });
    assert_eq!(app.panels.active, 1);
    assert!(app.parked_tabs().next().unwrap().scroll_target > 0.0);
    assert_eq!(app.tab().scroll_target, 0.0);
}

#[test]
fn dragging_files_over_an_inactive_panel_activates_it_for_the_drop() {
    let mut app = app_at(r"C:\a");
    deliver(&mut app, &[("a.txt", false)]);
    key(&mut app, Key::Char('S'), CTRL_SHIFT);
    deliver(&mut app, &[("a.txt", false)]);
    frame(&mut app);
    let (x, y) = row_in(layout_of(&app, 0), 0);
    let paths = vec![FsPath::parse(r"D:\in\f.txt").unwrap()];
    let effect = app.drag(&DragEvent::Enter { paths, x, y, mods: NO, allowed: drop_effect::COPY | drop_effect::MOVE }, t0());
    assert_eq!(app.panels.active, 0);
    assert_ne!(effect, drop_effect::NONE);
}

#[test]
fn sessions_keep_the_panel_tree_and_old_sessions_load_into_one_panel() {
    let mut app = three(r"C:\a");
    key(&mut app, Key::Char('T'), CTRL);
    app.activate_panel(1);
    app.set_sep(0, 400.0);
    let s = app.session();
    assert_eq!(s.tabs.len(), 4);
    assert_eq!(s.cur, 1);
    let p = s.panels.clone().unwrap();
    assert_eq!(p.active, 1);
    let Shape::Group { down: false, kids } = &p.root else { panic!("{:?}", p.root) };
    assert!(matches!(&kids[1].0, Shape::Group { down: true, kids } if kids[1].0 == Shape::Leaf { tabs: 2, cur: 1 }));
    let mut back = app_at(r"C:\a");
    back.restore_tabs(&s.tabs, s.cur);
    back.restore_trails(&s.trails);
    back.drain_effects().for_each(drop);
    back.restore_panels(&p);
    assert_eq!(shape(&back), "(1|(2/3))");
    assert_eq!(back.session(), s);
    assert_eq!(listed(&mut back).len(), 2, "the shown tabs of the other panels load at once");
    assert!((rect(&back, 1).w - 400.0).abs() <= 1.0);
    let mut old = app_at(r"C:\a");
    old.restore_tabs(&s.tabs, s.cur);
    assert_eq!((old.panel_count(), old.tabs().len()), (1, 4), "a session without panels puts every tab into one panel");
    assert_eq!(old.session(), WindowSession { panels: None, ..s.clone() });
    old.restore_panels(&PanelsSession { root: Shape::Leaf { tabs: 4, cur: 0 }, active: 0 });
    assert_eq!(old.panel_count(), 1);
    old.restore_panels(&PanelsSession { root: Shape::Group { down: false, kids: vec![(Shape::Leaf { tabs: 1, cur: 0 }, 1.0), (Shape::Leaf { tabs: 1, cur: 0 }, 1.0)] }, active: 0 });
    assert_eq!(old.panel_count(), 1, "a tree that does not match the tabs is ignored");
}

#[test]
fn a_tab_moved_out_of_a_panel_with_one_tab_closes_the_panel() {
    let mut app = app_at(r"C:\a");
    key(&mut app, Key::Char('S'), CTRL_SHIFT);
    app.move_tab_to_window();
    assert!(app.drain_effects().any(|e| matches!(e, Effect::MoveTab { last: false, .. })));
    assert_eq!(app.panel_count(), 1);
    app.move_tab_to_window();
    assert!(!app.drain_effects().any(|e| matches!(e, Effect::MoveTab { .. })), "the only tab of the window stays");
}
