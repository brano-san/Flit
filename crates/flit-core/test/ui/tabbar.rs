use super::*;
use std::time::Duration;
use crate::input::{Event, Mods};
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
    app.tabs.iter().map(|t| t.location().display().into_owned()).collect()
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
    app.drain_effects()
        .filter_map(|e| if let Effect::Provider { req: ProviderRequest::List { path, .. }, .. } = e { Some(path.display().into_owned()) } else { None })
        .collect()
}

#[test]
fn tab_menu_closes_other_left_and_right_tabs_and_remembers_them() {
    let mut app = app_with(&[r"C:\a", r"C:\b", r"C:\c", r"C:\d", r"C:\e"]);
    let (x, y) = tab_xy(&app, 2);
    ev(&mut app, Event::MouseDown { x, y, button: MouseButton::Right, clicks: 1 });
    ev(&mut app, Event::MouseUp { x, y, button: MouseButton::Right });
    assert_eq!(app.cur, 2);
    let labels = menu_labels(&mut app);
    assert_eq!(
        labels,
        ["Shift Tab Left", "Shift Tab Right", "Duplicate Tab", "Duplicate Tab To New Window", "Close Tab", "Close Other Tabs", "Close Left Tabs", "Close Right Tabs", "Close All Tabs", "Move tab to a new window", "Reopen closed tab"]
    );
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
    assert_eq!(app.cur, 0);
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
    assert_eq!(app.cur, 2);
    ev(&mut app, Event::MouseUp { x: x2, y, button: MouseButton::Left });
    assert_eq!(app.drag, Drag::None);
    let (x1, _) = tab_xy(&app, 1);
    ev(&mut app, Event::MouseDown { x: x2, y, button: MouseButton::Left, clicks: 1 });
    ev(&mut app, Event::MouseMove { x: x1 - 40.0, y });
    assert_eq!(names(&app), [r"C:\b", r"C:\a", r"C:\c"]);
    key(&mut app, Key::Escape, NO);
    assert_eq!(names(&app), [r"C:\b", r"C:\c", r"C:\a"]);
    assert_eq!(app.cur, 2);
    ev(&mut app, Event::MouseUp { x: x1, y, button: MouseButton::Left });
    assert!(!app.drain_effects().any(|e| matches!(e, Effect::MoveTab { .. })));
}

#[test]
fn a_tab_torn_off_the_strip_moves_to_another_window() {
    let mut app = app_with(&[r"C:\a", r"C:\b"]);
    app.run_action("nav.up", Instant::now());
    let (x, y) = tab_xy(&app, 1);
    ev(&mut app, Event::MouseDown { x, y, button: MouseButton::Left, clicks: 1 });
    ev(&mut app, Event::MouseMove { x, y: 300.0 });
    assert!(matches!(app.drag, Drag::Tab { torn: false, .. }) && app.docking.is_some(), "inside the window the tab docks");
    ev(&mut app, Event::MouseMove { x, y: -30.0 });
    assert!(matches!(app.drag, Drag::Tab { torn: true, .. }) && app.docking.is_none());
    app.drain_effects().for_each(drop);
    ev(&mut app, Event::MouseUp { x, y: -30.0, button: MouseButton::Left });
    let moved = app.drain_effects().find_map(|e| if let Effect::MoveTab { tab, at, last } = e { Some((tab, at, last)) } else { None }).unwrap();
    assert_eq!(moved.0.location(), &loc(r"C:\"));
    assert_eq!((moved.1, moved.2), (Some((x, -30.0)), false));
    assert_eq!(names(&app), [r"C:\a"]);

    let mut target = app_with(&[r"C:\x", r"C:\y"]);
    let r = target.layout.tabs[1].0;
    let base = target.next_tab;
    target.insert_tab(moved.0, Some((r.x + 5.0, 10.0)), Instant::now());
    assert_eq!(names(&target), [r"C:\x", r"C:\", r"C:\y"]);
    assert_eq!(target.cur, 1);
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

#[test]
fn wheel_over_the_strip_switches_tabs_in_the_chosen_direction() {
    let mut app = app_with(&[r"C:\a", r"C:\b", r"C:\c"]);
    app.select_tab(0);
    let (x, y) = tab_xy(&app, 1);
    ev(&mut app, Event::MouseMove { x, y });
    ev(&mut app, Event::Wheel { dx: 0.0, dy: -1.0 });
    assert_eq!(app.cur, 1);
    ev(&mut app, Event::Wheel { dx: 0.0, dy: -0.5 });
    ev(&mut app, Event::Wheel { dx: 0.0, dy: -0.5 });
    assert_eq!(app.cur, 2);
    ev(&mut app, Event::Wheel { dx: 0.0, dy: -3.0 });
    assert_eq!(app.cur, 2);
    app.tab_wheel_reversed = true;
    ev(&mut app, Event::Wheel { dx: 0.0, dy: 1.0 });
    assert_eq!(app.cur, 2);
    ev(&mut app, Event::Wheel { dx: 0.0, dy: -2.0 });
    assert_eq!(app.cur, 0);
    let (x, y) = row_xy(&app, 3);
    ev(&mut app, Event::MouseMove { x, y });
    ev(&mut app, Event::Wheel { dx: 0.0, dy: -1.0 });
    assert_eq!(app.cur, 0);
}

#[test]
fn restored_tabs_stay_asleep_until_shown() {
    let mut app = app_with(&[r"C:\b"]);
    let rev = app.session_rev();
    app.restore_tabs(&[loc(r"C:\a"), loc(r"C:\b"), loc(r"C:\c")], 1);
    assert!(app.session_rev() > rev);
    assert_eq!(names(&app), [r"C:\a", r"C:\b", r"C:\c"]);
    assert_eq!(app.cur, 1);
    assert!(listed(&mut app).is_empty());
    assert_eq!(app.tabs[0].gen, 0);
    assert_eq!(app.tabs[2].title(), "c");
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
    let v = app.tabs[0].view.as_mut().unwrap();
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
    let effect = app.drag(&DragEvent::Enter { paths, x, y, mods: NO, allowed: drop_effect::COPY | drop_effect::MOVE }, Instant::now());
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
    assert!(r.w > 0.0 && r.right() <= app.panels.caption[0].x);
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
    let drive = Drive { path: FsPath::parse(r"C:\").unwrap(), label: "Local Disk (C:)".into(), total: 10, free: 5, removable: false, unmounted: false, kind: None, remote: None };
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
    let z = *app.docking.as_ref().unwrap().zones.iter().find(|z| pick(z)).expect("zone");
    let (zx, zy) = (z.button.x + 2.0, z.button.y + 2.0);
    ev(app, Event::MouseMove { x: zx, y: zy });
    assert_eq!(app.docking.as_ref().unwrap().hot.map(|k| app.docking.as_ref().unwrap().zones[k].dock), Some(z.dock));
    ev(app, Event::MouseUp { x: zx, y: zy, button: MouseButton::Left });
    assert!(app.docking.is_none());
    let mut g = crate::text::Gfx::new(crate::text::tests::Mono, 1024);
    app.frame(&mut g, Instant::now());
}

#[test]
fn dropping_a_tab_on_a_zone_splits_moves_or_copies_it() {
    use crate::ui::dock::Dock;
    let mut app = app_with(&[r"C:\a", r"C:\b"]);
    let first = app.panels.active;
    drop_on(&mut app, 1, |z| matches!(z.dock, Dock::Split { down: false, before: false, .. }));
    assert_eq!(app.panel_count(), 2);
    assert_ne!(app.panels.active, first);
    assert_eq!(names(&app), [r"C:\b"]);
    let second = app.panels.active;
    assert!(app.panel_rect().x > 300.0, "the new panel is on the right");
    drop_on(&mut app, 0, |z| matches!(z.dock, Dock::Split { down: false, before: true, .. }));
    assert_eq!(app.panel_count(), 2, "the only tab of a panel cannot split its own panel");
    app.activate_panel(first);
    assert_eq!(names(&app), [r"C:\a"]);
    let r = app.panels.list.iter().find(|p| p.id == second).unwrap().rect;
    let (x, y) = tab_xy(&app, 0);
    ev(&mut app, Event::MouseDown { x, y, button: MouseButton::Left, clicks: 1 });
    ev(&mut app, Event::MouseMove { x: r.x + r.w / 2.0, y: r.y + r.h / 2.0 });
    let z = *app.docking.as_ref().unwrap().zones.iter().find(|z| z.dock == Dock::Into(second)).unwrap();
    ev(&mut app, Event::MouseMove { x: z.button.x + 2.0, y: z.button.y + 2.0 });
    ev(&mut app, Event::Key { key: Key::Other, mods: Mods { ctrl: true, shift: false, alt: false }, pressed: true });
    ev(&mut app, Event::MouseUp { x: z.button.x + 2.0, y: z.button.y + 2.0, button: MouseButton::Left });
    assert_eq!(app.panel_count(), 2, "a copy leaves the source panel");
    assert_eq!(app.panels.active, second);
    assert_eq!(names(&app), [r"C:\b", r"C:\a"]);
    assert_eq!(app.cur, 1);
    ev(&mut app, Event::Key { key: Key::Other, mods: NO, pressed: false });
    app.activate_panel(first);
    let r = app.panels.list.iter().find(|p| p.id == second).unwrap().rect;
    let (x, y) = tab_xy(&app, 0);
    ev(&mut app, Event::MouseDown { x, y, button: MouseButton::Left, clicks: 1 });
    ev(&mut app, Event::MouseMove { x: r.x + r.w / 2.0, y: r.y + r.h / 2.0 });
    let z = *app.docking.as_ref().unwrap().zones.iter().find(|z| z.dock == Dock::Into(second)).unwrap();
    ev(&mut app, Event::MouseMove { x: z.button.x + 2.0, y: z.button.y + 2.0 });
    ev(&mut app, Event::MouseUp { x: z.button.x + 2.0, y: z.button.y + 2.0, button: MouseButton::Left });
    assert_eq!(app.panel_count(), 1, "moving the last tab away closes its panel");
    assert_eq!(names(&app), [r"C:\b", r"C:\a", r"C:\a"]);
    let (x, y) = tab_xy(&app, 0);
    ev(&mut app, Event::MouseDown { x, y, button: MouseButton::Left, clicks: 1 });
    let left = app.panels.left;
    ev(&mut app, Event::MouseMove { x: left + 5.0, y: 400.0 });
    let edge = app.docking.as_ref().unwrap().zones.iter().find(|z| z.dock == Dock::Window { before: true }).copied();
    assert!(edge.is_some(), "the window edge offers a column");
    key(&mut app, Key::Escape, NO);
    assert!(app.docking.is_none() && app.drag == Drag::None);
}
