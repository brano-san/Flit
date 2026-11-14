use super::*;
use crate::input::{CursorIcon, Event};
use crate::listing::{attr, ChangeKind, DirChange, DirSnapshot, Listing, Meta};
use crate::path::FsPath;
use crate::provider::{FileOp, ProviderEvent, ProviderRequest, TabId};
use crate::sort::{self, CaseMode, SortKeys};
use crate::text::tests::Mono;
use crate::text::Gfx;
use crate::ui::{Drag, Drive, Effect, KnownFolder, Persisted, Places};
use std::sync::Arc;
use std::time::Duration;

const NO: Mods = Mods { ctrl: false, shift: false, alt: false };
const CTRL: Mods = Mods { ctrl: true, shift: false, alt: false };
const CTRL_SHIFT: Mods = Mods { ctrl: true, shift: true, alt: false };

fn t0() -> Instant {
    Instant::now()
}

fn loc(s: &str) -> Location {
    Location::parse(s).unwrap()
}

fn app_at(path: &str) -> App {
    App::new(Persisted { path: loc(path), ..Default::default() }, Vec::new(), 0, false, 1.0, (1000, 600), t0())
}

fn deliver(app: &mut App, items: &[(&str, u32)]) {
    let t = app.tab();
    let mut listing = Listing::default();
    let mut keys = SortKeys::new(CaseMode::Insensitive);
    for &(n, attrs) in items {
        let i = listing.push(n.as_bytes(), Meta { attrs, size: 10, ..Default::default() });
        keys.push(listing.name(i));
    }
    let view = t.view_spec;
    let mut order = Vec::new();
    sort::sort(&listing, &keys, view, &mut order);
    let snap = DirSnapshot { gen: t.gen, path: t.location().path().clone(), listing, keys, order, view, done: true, error: None, elapsed: Duration::from_millis(1) };
    t.slot.publish(Arc::new(snap));
    app.poll();
}

fn files(app: &mut App, names: &[&str]) {
    let items: Vec<(&str, u32)> = names.iter().map(|&n| (n, 0)).collect();
    deliver(app, &items);
}

fn key(app: &mut App, k: Key, mods: Mods) {
    app.on_event(&Event::Key { key: k, mods, pressed: true }, t0());
}

fn hold(app: &mut App, mods: Mods) {
    app.on_event(&Event::Key { key: Key::Other, mods, pressed: false }, t0());
}

fn typed(app: &mut App, s: &str) {
    for c in s.chars() {
        app.on_event(&Event::Char(c), t0());
    }
}

fn press(app: &mut App, r: Rect, button: MouseButton) {
    let (x, y) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
    app.on_event(&Event::MouseMove { x, y }, t0());
    app.on_event(&Event::MouseDown { x, y, button, clicks: 1 }, t0());
    app.on_event(&Event::MouseUp { x, y, button }, t0());
}

fn rows(app: &App) -> Vec<String> {
    let v = app.tab().view.as_ref().unwrap();
    v.rows().iter().map(|&i| String::from_utf8_lossy(v.listing().name(i)).into_owned()).collect()
}

fn selected(app: &App) -> Vec<String> {
    let v = app.tab().view.as_ref().unwrap();
    v.selected().map(|i| String::from_utf8_lossy(v.listing().name(i)).into_owned()).collect()
}

fn focused(app: &App) -> Option<String> {
    let v = app.tab().view.as_ref().unwrap();
    v.focus.map(|i| String::from_utf8_lossy(v.listing().name(i)).into_owned())
}

fn listed(app: &mut App) -> Vec<(TabId, Location)> {
    app.drain_effects()
        .filter_map(|e| match e {
            Effect::Provider { provider, req: ProviderRequest::List { tab, path, .. } } => Some((
                tab,
                match provider {
                    crate::location::ProviderId::Local => Location::Local(path),
                    crate::location::ProviderId::Wsl(distro) => Location::Wsl { distro, path },
                },
            )),
            _ => None,
        })
        .collect()
}

fn places() -> Arc<Places> {
    Arc::new(Places {
        folders: vec![(KnownFolder::Home, loc(r"C:\Users\u")), (KnownFolder::Downloads, loc(r"C:\Users\u\Downloads"))],
        drives: vec![
            Drive { path: FsPath::parse(r"C:\").unwrap(), label: "Local Disk (C:)".into(), total: 1000, free: 250, file_system: String::new(), removable: false, unmounted: false, kind: None, remote: None },
            Drive { path: FsPath::parse(r"E:\").unwrap(), label: "USB Drive (E:)".into(), total: 0, free: 0, file_system: String::new(), removable: true, unmounted: false, kind: None, remote: None },
        ],
        wsl: vec!["Ubuntu".into()],
        recycle_bin: true,
        ..Default::default()
    })
}

fn side_index(app: &App, label: &str) -> usize {
    app.nav.sidebar.items.iter().position(|it| it.label == label).unwrap_or_else(|| panic!("no sidebar item {label}"))
}

fn side_rect(app: &App, label: &str) -> Rect {
    app.nav.sidebar.rects[side_index(app, label)]
}

#[test]
fn filter_narrows_rows_selection_and_layout() {
    let mut app = app_at(r"C:\x");
    files(&mut app, &["apple.txt", "banana.rs", "cherry.txt", "date.rs"]);
    let header_y = app.layout.header.y;
    key(&mut app, Key::Char('A'), CTRL);
    key(&mut app, Key::Char('F'), CTRL);
    assert_eq!(app.focus, Focus::Filter);
    assert_eq!(app.layout.header.y, header_y, "the filter lives in the bottom bar");
    assert!(app.nav.filter.field.y >= app.layout.status.y && app.nav.filter.field.w > 0.0);
    typed(&mut app, ".RS");
    assert_eq!(rows(&app), ["banana.rs", "date.rs"]);
    let v = app.tab().view.as_ref().unwrap();
    assert_eq!((v.rows().len(), v.total_rows(), v.selected_count()), (2, 4, 2));
    app.drain_effects().for_each(drop);
    key(&mut app, Key::Delete, NO);
    let trashed: Vec<FsPath> = app
        .drain_effects()
        .filter_map(|e| match e {
            Effect::Provider { req: ProviderRequest::Op { op: FileOp::Trash(p), .. }, .. } => Some(p),
            _ => None,
        })
        .flatten()
        .collect();
    assert!(trashed.is_empty(), "Delete edits the filter text while the field has focus");
    assert_eq!(app.tab().filter.as_ref().unwrap().text(), ".RS");
    key(&mut app, Key::Backspace, NO);
    key(&mut app, Key::Backspace, NO);
    assert_eq!(rows(&app), ["apple.txt", "banana.rs", "cherry.txt", "date.rs"]);
    typed(&mut app, "t");
    assert_eq!(rows(&app), ["apple.txt", "cherry.txt"]);
    key(&mut app, Key::Enter, NO);
    assert_eq!(app.focus, Focus::List);
    key(&mut app, Key::Delete, NO);
    let trashed: Vec<FsPath> = app
        .drain_effects()
        .filter_map(|e| match e {
            Effect::Provider { req: ProviderRequest::Op { op: FileOp::Trash(p), .. }, .. } => Some(p),
            _ => None,
        })
        .flatten()
        .collect();
    assert_eq!(trashed, [FsPath::parse(r"C:\x\apple.txt").unwrap()], "only the visible, selected row");
    key(&mut app, Key::Char('F'), CTRL);
    key(&mut app, Key::Escape, NO);
    assert!(app.tab().filter.is_none());
    assert_eq!(app.layout.header.y, header_y);
    assert_eq!(rows(&app).len(), 4);
    assert_eq!(app.focus, Focus::List);
}

#[test]
fn filter_survives_refresh_and_watcher_but_not_navigation() {
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[("src", attr::DIR), ("notes.md", 0), ("main.rs", 0)]);
    key(&mut app, Key::Char('F'), CTRL);
    typed(&mut app, "s");
    assert_eq!(rows(&app), ["src", "main.rs", "notes.md"]);
    key(&mut app, Key::Backspace, NO);
    typed(&mut app, "rs");
    assert_eq!(rows(&app), ["main.rs"]);
    app.drain_effects().for_each(drop);
    key(&mut app, Key::F(5), NO);
    assert_eq!(listed(&mut app), [(1, loc(r"C:\x"))]);
    deliver(&mut app, &[("src", attr::DIR), ("notes.md", 0), ("main.rs", 0), ("lib.rs", 0)]);
    assert_eq!(rows(&app), ["lib.rs", "main.rs"]);
    let (tab, gen) = (app.tab().id, app.tab().gen);
    let add = |n: &str| DirChange { kind: ChangeKind::Added, name: n.into(), meta: Some(Meta::default()) };
    app.on_provider_event(ProviderEvent::Changes { tab, gen, changes: vec![add("build.rs"), add("README")] }, t0());
    app.frame(&mut Gfx::new(Mono, 1024), t0());
    assert_eq!(rows(&app), ["build.rs", "lib.rs", "main.rs"]);
    key(&mut app, Key::Up, Mods { alt: true, ..NO });
    assert!(app.tab().filter.is_none(), "navigation clears the filter");
    assert_eq!(listed(&mut app), [(1, loc(r"C:\"))]);
}

#[test]
fn filter_is_per_tab() {
    let mut app = app_at(r"C:\x");
    files(&mut app, &["a1", "b1"]);
    key(&mut app, Key::Char('F'), CTRL);
    typed(&mut app, "a");
    key(&mut app, Key::Char('T'), CTRL);
    files(&mut app, &["a1", "b1"]);
    assert!(app.tab().filter.is_none());
    assert_eq!(rows(&app).len(), 2);
    assert_eq!(app.focus, Focus::List);
    key(&mut app, Key::Tab, CTRL);
    assert_eq!(rows(&app), ["a1"]);
    app.on_paste("b\nignored");
    assert_eq!(rows(&app), ["a1"], "paste needs the field focused");
    key(&mut app, Key::Char('F'), CTRL);
    app.on_paste("b\nignored");
    assert_eq!(rows(&app), ["b1"]);
}

#[test]
fn sidebar_lists_places_and_opens_items() {
    let mut app = app_at(r"C:\x");
    app.set_places(places());
    let labels: Vec<&str> = app.nav.sidebar.items.iter().map(|it| it.label.as_str()).collect();
    assert_eq!(labels, ["Local Disk (C:)", "USB Drive (E:)", "This PC", "u", "Downloads", "Recycle Bin", "Home", "Ubuntu"]);
    assert_eq!(app.nav.sidebar.items[0].usage, Some(0.75));
    assert_eq!(app.nav.sidebar.items[1].usage, None);
    assert!(app.layout.list.x >= app.nav.sidebar.rect.right() - 1.0);
    app.drain_effects().for_each(drop);
    let r = side_rect(&app, "Downloads");
    press(&mut app, r, MouseButton::Left);
    assert_eq!(listed(&mut app), [(1, loc(r"C:\Users\u\Downloads"))]);
    let r = side_rect(&app, "Ubuntu");
    press(&mut app, r, MouseButton::Middle);
    assert_eq!(listed(&mut app), [(2, loc("wsl://Ubuntu/"))]);
    assert_eq!(app.tabs().len(), 2);
    hold(&mut app, CTRL);
    let r = side_rect(&app, "Local Disk (C:)");
    press(&mut app, r, MouseButton::Left);
    hold(&mut app, NO);
    assert_eq!(listed(&mut app), [(3, loc(r"C:\"))]);
    let here = app.current_location().clone();
    assert_eq!(app.nav.sidebar.items.iter().filter(|it| it.loc == here).count(), 1);
    app.on_event(&Event::DevicesChanged, t0());
    assert!(app.drain_effects().any(|e| matches!(e, Effect::RefreshPlaces)));
}

#[test]
fn a_drive_plugged_in_or_out_relists_this_pc_and_leaves_folders_alone() {
    let mut app = app_at(r"C:\work");
    app.drain_effects().for_each(drop);
    app.on_event(&Event::DevicesChanged, t0());
    assert_eq!(listed(&mut app), []);
    app.navigate(Location::Local(FsPath::computer()), t0());
    assert_eq!(listed(&mut app), [(app.tab().id, Location::Local(FsPath::computer()))]);
    app.on_event(&Event::DevicesChanged, t0());
    assert_eq!(listed(&mut app), [(app.tab().id, Location::Local(FsPath::computer()))]);
}

#[test]
fn pinning_from_keyboard_and_hover_button() {
    let mut app = app_at(r"C:\work\proj");
    app.set_places(places());
    app.drain_effects().for_each(drop);
    app.run_action("folder.bookmark", Instant::now());
    assert!(app.drain_effects().any(|e| matches!(e, Effect::Pinned(p) if p == [loc(r"C:\work\proj")])));
    assert_eq!(app.nav.sidebar.items[0].label, "proj");
    assert_eq!(app.nav_state().pinned, [loc(r"C:\work\proj")]);
    let downloads = side_rect(&app, "Downloads");
    let pin = app.nav.sidebar.pin_rect(downloads, app.scale);
    assert!(!matches!(app.hit(pin.x + pin.w / 2.0, pin.y + pin.h / 2.0), Hit::SidePin(_)), "only bookmarks have a remove button");
    app.pin_locations(vec![loc(r"C:\Users\u\Downloads")]);
    assert_eq!(app.nav_state().pinned, [loc(r"C:\work\proj"), loc(r"C:\Users\u\Downloads")]);
    assert!(listed(&mut app).is_empty(), "pinning does not navigate");
    let first = app.nav.sidebar.rects[0];
    let unpin = app.nav.sidebar.pin_rect(first, app.scale);
    press(&mut app, unpin, MouseButton::Left);
    assert_eq!(app.nav_state().pinned.len(), 2, "unpinning asks first");
    app.on_event(&Event::Key { key: Key::Enter, mods: Mods::default(), pressed: true }, Instant::now());
    assert_eq!(app.nav_state().pinned, [loc(r"C:\Users\u\Downloads")]);
    assert!(app.drain_effects().any(|e| matches!(e, Effect::Pinned(p) if p.len() == 1)));
    app.set_pinned(vec![loc(r"D:\shared")]);
    assert_eq!(app.nav.sidebar.items[0].label, "shared");
}

#[test]
fn pinned_folders_reorder_by_drag() {
    let mut app = app_at(r"C:\x");
    app.set_pinned(vec![loc(r"C:\a"), loc(r"C:\b"), loc(r"C:\c")]);
    app.drain_effects().for_each(drop);
    let (first, third) = (app.nav.sidebar.rects[0], app.nav.sidebar.rects[2]);
    let x = first.x + 20.0;
    app.on_event(&Event::MouseDown { x, y: first.y + 5.0, button: MouseButton::Left, clicks: 1 }, t0());
    app.on_event(&Event::MouseMove { x, y: first.y + 7.0 }, t0());
    assert_eq!(app.drag, Drag::None, "a small jitter is still a click");
    app.on_event(&Event::MouseMove { x, y: third.bottom() - 2.0 }, t0());
    assert_eq!(app.drag, Drag::PinMove { from: 0, to: 3 });
    app.frame(&mut Gfx::new(Mono, 1024), t0());
    app.on_event(&Event::MouseUp { x, y: third.bottom() - 2.0, button: MouseButton::Left }, t0());
    assert_eq!(app.nav_state().pinned, [loc(r"C:\b"), loc(r"C:\c"), loc(r"C:\a")]);
    let fx: Vec<Effect> = app.drain_effects().collect();
    assert!(fx.iter().any(|e| matches!(e, Effect::Pinned(p) if p.len() == 3)));
    assert!(!fx.iter().any(|e| matches!(e, Effect::Provider { req: ProviderRequest::List { .. }, .. })), "dropping does not navigate");
}

#[test]
fn pins_removed_by_another_window_mid_drag_are_harmless() {
    let mut app = app_at(r"C:\x");
    app.set_pinned(vec![loc(r"C:\a"), loc(r"C:\b")]);
    let first = app.nav.sidebar.rects[0];
    let x = first.x + 20.0;
    app.on_event(&Event::MouseDown { x, y: first.y + 5.0, button: MouseButton::Left, clicks: 1 }, t0());
    app.on_event(&Event::MouseMove { x, y: first.y + 40.0 }, t0());
    assert!(matches!(app.drag, Drag::PinMove { .. }));
    app.set_pinned(Vec::new());
    app.frame(&mut Gfx::new(Mono, 1024), t0());
    app.on_event(&Event::MouseUp { x, y: first.y + 40.0, button: MouseButton::Left }, t0());
    assert!(app.nav_state().pinned.is_empty());
}

#[test]
fn sidebar_toggle_splitter_and_persisted_width() {
    let mut app = app_at(r"C:\x");
    app.animations = false;
    let shown = app.layout.header.x;
    assert_eq!(shown, app.nav_state().sidebar_w);
    key(&mut app, Key::Char('B'), CTRL);
    assert_eq!(app.layout.header.x, 0.0);
    assert!(!app.nav_state().sidebar);
    key(&mut app, Key::Char('B'), CTRL);
    assert_eq!(app.layout.header.x, shown);
    let sp = app.nav.sidebar.splitter;
    let (x, y) = (sp.x + sp.w / 2.0, sp.y + 40.0);
    app.on_event(&Event::MouseMove { x, y }, t0());
    assert_eq!(app.cursor, CursorIcon::ResizeHorizontal);
    app.on_event(&Event::MouseDown { x, y, button: MouseButton::Left, clicks: 1 }, t0());
    app.on_event(&Event::MouseMove { x: x + 60.0, y }, t0());
    app.on_event(&Event::MouseUp { x: x + 60.0, y, button: MouseButton::Left }, t0());
    assert_eq!(app.nav_state().sidebar_w, 280.0);
    assert_eq!(app.layout.header.x, 280.0);
    app.on_event(&Event::MouseDown { x: 280.0, y, button: MouseButton::Left, clicks: 1 }, t0());
    app.on_event(&Event::MouseMove { x: 5000.0, y }, t0());
    app.on_event(&Event::MouseUp { x: 5000.0, y, button: MouseButton::Left }, t0());
    assert_eq!(app.nav_state().sidebar_w, crate::ui::sidebar::SIDEBAR_MAX_W);
    assert!(app.layout.list.w > 0.0);
    let mut state = app.nav_state();
    state.sidebar_w = f32::NAN;
    app.set_nav_state(state);
    assert_eq!(app.nav_state().sidebar_w, 220.0);
}

#[test]
fn keyboard_focus_moves_into_the_sidebar() {
    let mut app = app_at(r"C:\Users\u");
    app.set_places(places());
    files(&mut app, &["a"]);
    app.drain_effects().for_each(drop);
    key(&mut app, Key::F(6), NO);
    assert_eq!(app.focus, Focus::Sidebar);
    assert_eq!(app.nav.sidebar.cursor, side_index(&app, "u"), "starts at the current location");
    key(&mut app, Key::Down, NO);
    key(&mut app, Key::Enter, NO);
    assert_eq!(listed(&mut app), [(1, loc(r"C:\Users\u\Downloads"))]);
    assert_eq!(app.focus, Focus::List);
    key(&mut app, Key::F(6), NO);
    key(&mut app, Key::End, NO);
    key(&mut app, Key::Enter, CTRL);
    assert_eq!(listed(&mut app), [(2, loc("wsl://Ubuntu/"))]);
    key(&mut app, Key::F(6), NO);
    key(&mut app, Key::Escape, NO);
    assert_eq!(app.focus, Focus::List);
}

#[test]
fn palette_runs_commands_from_keyboard_and_mouse() {
    let mut app = app_at(r"C:\x");
    files(&mut app, &["a"]);
    key(&mut app, Key::Char('P'), CTRL_SHIFT);
    assert!(app.nav.palette.is_some());
    typed(&mut app, "new tab");
    key(&mut app, Key::Enter, NO);
    assert!(app.nav.palette.is_none());
    assert_eq!(app.tabs().len(), 2);
    files(&mut app, &["a"]);
    key(&mut app, Key::Char('P'), CTRL_SHIFT);
    typed(&mut app, "hidden");
    app.drain_effects().for_each(drop);
    key(&mut app, Key::Enter, NO);
    assert!(app.drain_effects().any(|e| matches!(e, Effect::Resort { view, .. } if view.show_hidden)));
    key(&mut app, Key::Char('P'), CTRL_SHIFT);
    typed(&mut app, "zzzz");
    assert!(app.nav.palette.as_ref().unwrap().matches.is_empty());
    key(&mut app, Key::Enter, NO);
    assert!(app.nav.palette.is_some(), "Enter without a match keeps the palette open");
    for _ in 0..4 {
        key(&mut app, Key::Backspace, NO);
    }
    typed(&mut app, "close tab");
    let row = app.nav.palette.as_ref().unwrap().rows[0];
    press(&mut app, row, MouseButton::Left);
    assert_eq!(app.tabs().len(), 1);
    key(&mut app, Key::Char('P'), CTRL_SHIFT);
    key(&mut app, Key::Down, NO);
    key(&mut app, Key::Up, NO);
    key(&mut app, Key::Up, NO);
    let p = app.nav.palette.as_ref().unwrap();
    assert_eq!(p.sel, p.matches.len() - 1, "selection wraps");
    press(&mut app, Rect::new(10.0, 500.0, 4.0, 4.0), MouseButton::Left);
    assert!(app.nav.palette.is_none(), "a click outside closes the palette");
    assert!(listed(&mut app).is_empty(), "and does not reach the list");
    key(&mut app, Key::Char('P'), CTRL_SHIFT);
    key(&mut app, Key::Escape, NO);
    assert!(app.nav.palette.is_none());
}

#[test]
fn registry_commands_reopen_tabs_and_open_windows() {
    let mut app = app_at(r"C:\a");
    key(&mut app, Key::Char('T'), CTRL);
    files(&mut app, &["x"]);
    key(&mut app, Key::Char('L'), CTRL);
    typed(&mut app, r"D:\b");
    key(&mut app, Key::Enter, NO);
    let opens: Vec<_> = app.drain_effects().filter_map(|e| if let Effect::Open(o) = e { Some(o) } else { None }).collect();
    app.open(&opens[0], &|_| None, t0());
    key(&mut app, Key::Char('W'), CTRL);
    assert_eq!(app.tabs().len(), 1);
    app.drain_effects().for_each(drop);
    key(&mut app, Key::Char('T'), CTRL_SHIFT);
    assert_eq!(app.tabs().len(), 2);
    assert_eq!(listed(&mut app), [(3, loc(r"D:\b"))]);
    key(&mut app, Key::Char('N'), CTRL);
    assert!(app.drain_effects().any(|e| matches!(e, Effect::NewWindow(r) if r[0].folder == loc(r"D:\b"))));
    files(&mut app, &["y"]);
    assert!(app.run_action("view.sort.size", t0()));
    assert!(app.drain_effects().any(|e| matches!(e, Effect::Resort { view, .. } if view.sort.field == crate::sort::SortField::Size)));
    assert!(!app.run_action("no.such.command", t0()));
}

#[test]
fn sidebar_frames_reuse_shaped_text() {
    let mut app = app_at(r"C:\x");
    app.set_places(places());
    key(&mut app, Key::Char('D'), CTRL);
    key(&mut app, Key::Char('F'), CTRL);
    files(&mut app, &["a.txt", "b.txt"]);
    let mut g = Gfx::new(Mono, 2048);
    app.frame(&mut g, t0());
    app.drain_effects().for_each(drop);
    g.uploads.clear();
    app.frame(&mut g, t0());
    assert!(g.uploads.rects.is_empty(), "nothing new to rasterize on an unchanged frame");
    assert_eq!(app.drain_effects().count(), 0);
    key(&mut app, Key::Char('P'), CTRL_SHIFT);
    app.frame(&mut g, t0());
    assert!(app.ime_area().is_some());
}

#[test]
fn opened_wsl_folders_take_the_registry_spelling() {
    let mut app = app_at(r"C:\x");
    app.set_places(places());
    app.drain_effects().for_each(drop);
    app.dispatch(vec![crate::open::Route { folder: loc(r"\\wsl$\UBUNTU\etc"), select: None }], crate::open::OpenTarget::NewTab, t0());
    assert_eq!(listed(&mut app), [(2, loc("wsl://Ubuntu/etc"))]);
    assert_eq!(app.tab().location(), &loc("wsl://Ubuntu/etc"));
}

#[test]
fn quick_access_slides_shut_and_open_with_ctrl_b() {
    let mut app = app_at(r"C:\x");
    let mut g = Gfx::new(Mono, 1024);
    app.frame(&mut g, Instant::now());
    let w = app.layout.header.x;
    key(&mut app, Key::Char('B'), CTRL);
    let t = Instant::now();
    app.frame(&mut g, t + Duration::from_millis(50));
    let mid = app.layout.header.x;
    assert!(mid > 0.0 && mid < w, "the panels follow the sidebar: {mid} {w}");
    app.frame(&mut g, t + Duration::from_millis(400));
    assert_eq!(app.layout.header.x, 0.0);
    key(&mut app, Key::Char('B'), CTRL);
    let t = Instant::now().max(t + Duration::from_millis(400));
    app.frame(&mut g, t + Duration::from_millis(50));
    let back = app.layout.header.x;
    assert!(back > 0.0 && back < w, "{back}");
    app.frame(&mut g, t + Duration::from_millis(400));
    assert_eq!(app.layout.header.x, w);
    app.animations = false;
    key(&mut app, Key::Char('B'), CTRL);
    assert_eq!(app.layout.header.x, 0.0, "without Animations it hides at once");
}

const SHIFT: Mods = Mods { ctrl: false, shift: true, alt: false };

fn redraw(app: &mut App) {
    app.frame(&mut Gfx::new(Mono, 1024), t0());
}

fn click_at(app: &mut App, (x, y): (f32, f32), clicks: u8, at: Instant) {
    app.on_event(&Event::MouseDown { x, y, button: MouseButton::Left, clicks }, at);
    app.on_event(&Event::MouseUp { x, y, button: MouseButton::Left }, at);
    redraw(app);
}

fn filter_selection(app: &App) -> String {
    let e = &app.tab().filter.as_ref().unwrap().edit;
    e.text[e.selection()].to_owned()
}

fn clipboard(app: &mut App) -> Vec<String> {
    app.drain_effects().filter_map(|e| if let Effect::SetClipboard(s) = e { Some(s) } else { None }).collect()
}

fn text_menu(app: &mut App) -> Vec<(String, bool)> {
    let menu = app.drain_effects().find_map(|e| if let Effect::ShellMenu(m) = e { Some(m.menu) } else { None }).expect("a menu");
    menu.into_iter().filter_map(|i| if let crate::ui::MenuItem::Command { label, disabled, .. } = i { Some((label, disabled)) } else { None }).collect()
}

fn filter_with(app: &mut App, text: &str) -> ((f32, f32), (f32, f32)) {
    key(app, Key::Char('F'), CTRL);
    typed(app, text);
    redraw(app);
    app.drain_effects().for_each(drop);
    let f = app.nav.filter.field;
    let y = f.y + f.h / 2.0;
    ((f.x + 25.0, y), (app.nav.filter.close.x - 6.0, y))
}

#[test]
fn shift_delete_in_the_filter_cuts_text_and_never_deletes_files() {
    let mut app = app_at(r"C:\x");
    files(&mut app, &["alpha.txt", "beta.txt"]);
    key(&mut app, Key::Char('A'), CTRL);
    key(&mut app, Key::Char('F'), CTRL);
    typed(&mut app, "a");
    app.drain_effects().for_each(drop);
    key(&mut app, Key::Char('A'), CTRL);
    key(&mut app, Key::Insert, CTRL);
    key(&mut app, Key::Delete, SHIFT);
    let effects: Vec<Effect> = app.drain_effects().collect();
    let copied: Vec<&str> = effects.iter().filter_map(|e| if let Effect::SetClipboard(s) = e { Some(s.as_str()) } else { None }).collect();
    assert_eq!(copied, ["a", "a"], "Ctrl+Insert copies, Shift+Delete cuts");
    assert!(!effects.iter().any(|e| matches!(e, Effect::Provider { req: ProviderRequest::Op { .. }, .. })));
    assert!(!app.jobs.modal(), "no permanent-delete prompt");
    assert_eq!(app.tab().filter.as_ref().unwrap().text(), "");
    key(&mut app, Key::Insert, SHIFT);
    assert!(app.drain_effects().any(|e| matches!(e, Effect::Paste)));
    app.on_event(&Event::Pasted("beta".into()), t0());
    assert_eq!(rows(&app), ["beta.txt"]);
}

#[test]
fn clicks_in_the_filter_pick_a_word_then_everything_and_dragging_extends() {
    let mut app = app_at(r"C:\x");
    files(&mut app, &["a"]);
    let (start, end) = filter_with(&mut app, "alpha beta");
    let t = t0();
    click_at(&mut app, end, 1, t);
    assert_eq!(filter_selection(&app), "");
    click_at(&mut app, end, 2, t);
    assert_eq!(filter_selection(&app), "beta", "a double click picks the word");
    click_at(&mut app, end, 1, t + Duration::from_millis(100));
    assert_eq!(filter_selection(&app), "alpha beta", "a quick third click picks everything");
    click_at(&mut app, end, 1, t + Duration::from_secs(2));
    assert_eq!(filter_selection(&app), "", "a later click only places the caret");
    hold(&mut app, SHIFT);
    click_at(&mut app, start, 1, t + Duration::from_secs(3));
    hold(&mut app, NO);
    assert_eq!(filter_selection(&app), "alpha beta", "Shift+click extends from the caret");
    click_at(&mut app, start, 1, t + Duration::from_secs(4));
    app.on_event(&Event::MouseDown { x: start.0, y: start.1, button: MouseButton::Left, clicks: 1 }, t + Duration::from_secs(5));
    redraw(&mut app);
    app.on_event(&Event::MouseMove { x: end.0 + 200.0, y: end.1 + 30.0 }, t + Duration::from_secs(5));
    redraw(&mut app);
    assert_eq!(filter_selection(&app), "alpha beta", "dragging selects, even past the field");
    app.on_event(&Event::MouseUp { x: end.0, y: end.1, button: MouseButton::Left }, t + Duration::from_secs(5));
    app.on_event(&Event::MouseMove { x: start.0, y: start.1 }, t + Duration::from_secs(5));
    redraw(&mut app);
    assert_eq!(filter_selection(&app), "alpha beta", "moving after the release changes nothing");
}

#[test]
fn the_filter_menu_edits_text_and_follows_the_selection() {
    let mut app = app_at(r"C:\x");
    files(&mut app, &["alpha beta", "gamma"]);
    let (start, _) = filter_with(&mut app, "alpha beta");
    app.on_event(&Event::MouseDown { x: start.0, y: start.1, button: MouseButton::Right, clicks: 1 }, t0());
    app.on_event(&Event::MouseUp { x: start.0, y: start.1, button: MouseButton::Right }, t0());
    let labels: Vec<&str> = vec!["Undo", "Cut", "Copy", "Paste", "Delete", "Select All"];
    assert_eq!(text_menu(&mut app), labels.iter().zip([false, true, true, false, true, false]).map(|(l, d)| (l.to_string(), d)).collect::<Vec<_>>());
    app.on_event(&Event::MenuCommand(5), t0());
    assert_eq!(filter_selection(&app), "alpha beta");
    key(&mut app, Key::Apps, NO);
    assert_eq!(text_menu(&mut app).iter().map(|(_, d)| *d).collect::<Vec<_>>(), [false, false, false, false, false, true], "the menu key opens it too");
    app.on_event(&Event::MenuCommand(1), t0());
    assert_eq!(clipboard(&mut app), ["alpha beta"]);
    assert_eq!(app.tab().filter.as_ref().unwrap().text(), "");
    app.on_event(&Event::MenuCommand(0), t0());
    assert_eq!(app.tab().filter.as_ref().unwrap().text(), "alpha beta");
    app.on_event(&Event::MenuCommand(3), t0());
    assert!(app.drain_effects().any(|e| matches!(e, Effect::Paste)));
    key(&mut app, Key::Enter, NO);
    key(&mut app, Key::Char('A'), CTRL);
    app.drain_effects().for_each(drop);
    for k in 0..6 {
        app.on_event(&Event::MenuCommand(k), t0());
    }
    let effects: Vec<Effect> = app.drain_effects().collect();
    assert!(effects.iter().all(|e| !matches!(e, Effect::Provider { req: ProviderRequest::Op { .. }, .. } | Effect::SetClipboardFiles { .. })), "with the list focused the menu commands touch no files");
    assert_eq!(app.tab().location(), &loc(r"C:\x"));
}

#[test]
fn the_address_bar_picks_words_too() {
    let mut app = app_at(r"C:\x");
    files(&mut app, &["a"]);
    key(&mut app, Key::Char('L'), CTRL);
    let Some(p) = app.nav.palette.as_mut() else { panic!("Ctrl+L opens the address bar") };
    p.field.edit.set(r"C:\Program Files");
    redraw(&mut app);
    let r = app.palette_field().unwrap();
    let at = (r.x + r.w / 2.0, r.y + r.h / 2.0);
    click_at(&mut app, at, 2, t0());
    let e = &app.nav.palette.as_ref().unwrap().field.edit;
    assert_eq!(&e.text[e.selection()], "Files");
}

#[test]
fn refresh_keeps_the_selection_focus_and_scroll() {
    let mut app = app_at(r"C:\x");
    let names: Vec<String> = (0..60).map(|i| format!("n{i:02}.txt")).collect();
    let items: Vec<(&str, u32)> = names.iter().map(|n| (n.as_str(), 0)).collect();
    deliver(&mut app, &items);
    key(&mut app, Key::Home, NO);
    key(&mut app, Key::Down, SHIFT);
    key(&mut app, Key::Down, SHIFT);
    assert_eq!(selected(&app), ["n00.txt", "n01.txt", "n02.txt"]);
    app.drain_effects().for_each(drop);
    let t = app.tab_mut();
    (t.scroll, t.scroll_target) = (120.0, 120.0);
    key(&mut app, Key::F(5), NO);
    assert_eq!(listed(&mut app), [(1, loc(r"C:\x"))]);
    let gone: Vec<(&str, u32)> = items.iter().copied().filter(|&(n, _)| n != "n01.txt").collect();
    deliver(&mut app, &gone);
    assert_eq!(selected(&app), ["n00.txt", "n02.txt"], "a file that vanished just drops out");
    assert_eq!(focused(&app).as_deref(), Some("n02.txt"));
    assert_eq!((app.tab().scroll, app.tab().scroll_target), (120.0, 120.0));
    redraw(&mut app);
    assert!((0.0..=app.max_scroll()).contains(&app.tab().scroll), "the restored scroll fits the folder");
}

#[test]
fn back_and_forward_bring_back_the_saved_view() {
    let mut app = app_at(r"C:\work");
    deliver(&mut app, &[("src", attr::DIR), ("a.txt", 0)]);
    key(&mut app, Key::Down, NO);
    key(&mut app, Key::Enter, NO);
    deliver(&mut app, &[("m.txt", 0), ("n.txt", 0), ("o.txt", 0), ("p.txt", 0)]);
    key(&mut app, Key::Home, NO);
    key(&mut app, Key::Down, SHIFT);
    assert_eq!(selected(&app), ["m.txt", "n.txt"]);
    let t = app.tab_mut();
    (t.scroll, t.scroll_target) = (90.0, 90.0);
    app.drain_effects().for_each(drop);
    key(&mut app, Key::Left, Mods { alt: true, ..NO });
    assert_eq!(listed(&mut app), [(1, loc(r"C:\work"))]);
    deliver(&mut app, &[("src", attr::DIR), ("a.txt", 0)]);
    assert_eq!(selected(&app), ["src"]);
    assert_eq!(focused(&app).as_deref(), Some("src"));
    key(&mut app, Key::Right, Mods { alt: true, ..NO });
    assert_eq!(listed(&mut app), [(1, loc(r"C:\work\src"))]);
    deliver(&mut app, &[("m.txt", 0), ("n.txt", 0), ("o.txt", 0), ("p.txt", 0)]);
    assert_eq!(selected(&app), ["m.txt", "n.txt"]);
    assert_eq!(focused(&app).as_deref(), Some("n.txt"));
    assert_eq!((app.tab().scroll, app.tab().scroll_target), (90.0, 90.0));
}

#[test]
fn back_brings_back_the_scroll_of_a_folder_left_with_nothing_selected() {
    let mut app = app_at(r"C:\work");
    deliver(&mut app, &[("a.txt", 0), ("b.txt", 0), ("c.txt", 0)]);
    assert_eq!(focused(&app), None);
    let t = app.tab_mut();
    (t.scroll, t.scroll_target) = (120.0, 120.0);
    app.drain_effects().for_each(drop);
    key(&mut app, Key::Up, Mods { alt: true, ..NO });
    assert_eq!(listed(&mut app), [(1, loc(r"C:\"))]);
    deliver(&mut app, &[("work", attr::DIR)]);
    key(&mut app, Key::Left, Mods { alt: true, ..NO });
    assert_eq!(listed(&mut app), [(1, loc(r"C:\work"))]);
    deliver(&mut app, &[("a.txt", 0), ("b.txt", 0), ("c.txt", 0)]);
    assert!(selected(&app).is_empty());
    assert_eq!((app.tab().scroll, app.tab().scroll_target), (120.0, 120.0));
}

#[test]
fn slash_and_tilde_in_the_list_open_the_address_bar_with_the_character_on_linux() {
    let later = |s: u64| t0() + Duration::from_secs(s);
    let mut app = app_at("/home/u");
    app.caps.unix_paths = true;
    app.opts.char_key = crate::ui::options::CharKey::Select;
    files(&mut app, &["a.txt", "a~b.txt", "b.txt"]);
    app.on_event(&Event::Char('a'), later(0));
    assert!(app.nav.palette.is_none(), "plain letters still go to type-ahead");
    app.on_event(&Event::Char('~'), later(0));
    assert!(app.nav.palette.is_none(), "a tilde in the middle of a typed name stays part of it");
    assert_eq!(selected(&app), ["a~b.txt"]);
    app.on_event(&Event::Char('/'), later(10));
    let Some(p) = app.nav.palette.as_ref() else { panic!("slash opens the address bar") };
    assert_eq!(p.field.text(), "/");
    key(&mut app, Key::Escape, NO);
    files(&mut app, &["a.txt", "b.txt"]);
    app.on_event(&Event::Char('~'), later(20));
    let Some(p) = app.nav.palette.as_ref() else { panic!("tilde opens the address bar") };
    assert_eq!(p.field.text(), "~");
}

#[test]
fn a_tilde_in_the_list_on_windows_finds_office_lock_files_by_typing() {
    let mut app = app_at(r"C:\x");
    app.opts.char_key = crate::ui::options::CharKey::Select;
    files(&mut app, &["a.txt", "~$report.docx"]);
    app.on_event(&Event::Char('~'), t0());
    assert!(app.nav.palette.is_none(), "Explorer has no slash or tilde shortcut");
    assert_eq!(selected(&app), ["~$report.docx"]);
}

fn right_click(app: &mut App, (x, y): (f32, f32)) {
    app.on_event(&Event::MouseDown { x, y, button: MouseButton::Right, clicks: 1 }, t0());
    app.on_event(&Event::MouseUp { x, y, button: MouseButton::Right }, t0());
}

fn drag_words(app: &mut App, from: (f32, f32), to: (f32, f32), at: Instant) {
    app.on_event(&Event::MouseDown { x: from.0, y: from.1, button: MouseButton::Left, clicks: 2 }, at);
    redraw(app);
    app.on_event(&Event::MouseMove { x: to.0, y: to.1 }, at);
    redraw(app);
    app.on_event(&Event::MouseUp { x: to.0, y: to.1, button: MouseButton::Left }, at);
}

fn picked(f: Option<&Field>) -> String {
    let e = &f.expect("the field").edit;
    e.text[e.selection()].to_owned()
}

fn no_menu(app: &mut App) -> bool {
    !app.drain_effects().any(|e| matches!(e, Effect::ShellMenu(_)))
}

#[test]
fn paste_in_the_field_menu_is_offered_only_while_the_clipboard_holds_text() {
    let mut app = app_at(r"C:\x");
    files(&mut app, &["alpha"]);
    let (start, _) = filter_with(&mut app, "alpha");
    let paste = |app: &mut App| {
        right_click(app, start);
        !text_menu(app)[3].1
    };
    assert!(paste(&mut app), "until the system reports anything Paste stays offered");
    app.on_event(&Event::ClipboardText(false), t0());
    assert!(!paste(&mut app), "an empty or non-text clipboard greys Paste out");
    right_click(&mut app, start);
    assert_eq!(text_menu(&mut app).iter().map(|(_, d)| *d).collect::<Vec<_>>(), [false, true, true, true, true, false], "the other entries do not change");
    key(&mut app, Key::Char('V'), CTRL);
    assert!(app.drain_effects().any(|e| matches!(e, Effect::Paste)), "Ctrl+V still asks the system");
    app.on_event(&Event::ClipboardText(true), t0());
    assert!(paste(&mut app));
}

#[test]
fn dragging_after_a_double_click_in_the_filter_extends_by_whole_words() {
    let mut app = app_at(r"C:\x");
    files(&mut app, &["a"]);
    let (start, end) = filter_with(&mut app, "alpha beta");
    drag_words(&mut app, end, start, t0());
    assert_eq!(filter_selection(&app), "alpha beta", "the word under the pointer is taken whole");
    drag_words(&mut app, start, (start.0 + 6.0, start.1), t0() + Duration::from_secs(2));
    assert_eq!(filter_selection(&app), "alpha", "within the first word nothing else is added");
    app.on_event(&Event::MouseDown { x: start.0, y: start.1, button: MouseButton::Left, clicks: 1 }, t0() + Duration::from_secs(4));
    redraw(&mut app);
    app.on_event(&Event::MouseMove { x: start.0 + 20.0, y: start.1 }, t0() + Duration::from_secs(4));
    redraw(&mut app);
    assert_eq!(filter_selection(&app), "al", "after a single click dragging still goes by characters");
}

#[test]
fn the_settings_search_picks_words_drags_and_offers_the_field_menu() {
    let mut app = app_at(r"C:\x");
    app.open_settings(None);
    typed(&mut app, "alpha beta");
    redraw(&mut app);
    let r = app.settings_ime_area().expect("typing goes to the search");
    let y = r.y + r.h / 2.0;
    let (start, end) = ((r.x + 32.0 + 25.0, y), (r.right() - 12.0, y));
    let t = t0();
    click_at(&mut app, end, 2, t);
    assert_eq!(picked(app.settings_search()), "beta");
    click_at(&mut app, end, 1, t + Duration::from_millis(100));
    assert_eq!(picked(app.settings_search()), "alpha beta", "a quick third click picks everything");
    drag_words(&mut app, end, start, t + Duration::from_secs(2));
    assert_eq!(picked(app.settings_search()), "alpha beta");
    click_at(&mut app, start, 1, t + Duration::from_secs(4));
    app.on_event(&Event::MouseDown { x: start.0, y, button: MouseButton::Left, clicks: 1 }, t + Duration::from_secs(5));
    app.on_event(&Event::MouseMove { x: end.0, y }, t + Duration::from_secs(5));
    redraw(&mut app);
    app.on_event(&Event::MouseUp { x: end.0, y, button: MouseButton::Left }, t + Duration::from_secs(5));
    assert_eq!(picked(app.settings_search()), "pha beta", "dragging selects");
    right_click(&mut app, start);
    assert_eq!(text_menu(&mut app).iter().map(|(_, d)| *d).collect::<Vec<_>>(), [false, false, false, false, false, false]);
    app.on_event(&Event::MenuCommand(1), t0());
    assert_eq!(clipboard(&mut app), ["pha beta"]);
    assert_eq!(app.settings_search().unwrap().text(), "al");
    app.on_event(&Event::MenuCommand(0), t0());
    assert_eq!(app.settings_search().unwrap().text(), "alpha beta");
    key(&mut app, Key::Apps, NO);
    assert_eq!(text_menu(&mut app).len(), 6, "the menu key opens it too");
    key(&mut app, Key::Enter, NO);
    key(&mut app, Key::Apps, NO);
    assert!(no_menu(&mut app), "with the list focused the menu key opens no field menu");
    app.on_event(&Event::MenuCommand(4), t0());
    assert_eq!(app.settings_search().unwrap().text(), "alpha beta", "a stale field command leaves the search alone");
}

#[test]
fn dialog_text_fields_pick_words_drag_and_offer_the_field_menu_but_secrets_stay_hidden() {
    use crate::ui::archive::Kind;
    use crate::ui::form::{Control, Form};
    let mut app = app_at(r"C:\x");
    app.open_form(Form::new(Kind::GitClone, "Clone", vec![Control::text("Address", "alpha beta", ""), Control::secret("Password", "")], vec!["OK", "Cancel"]));
    redraw(&mut app);
    let rows = app.jobs.form.as_ref().unwrap().rows.clone();
    let (y, y2) = (rows[0].y + rows[0].h / 2.0, rows[1].y + rows[1].h / 2.0);
    let (start, end) = ((rows[0].x + 2.0, y), (rows[0].right() - 12.0, y));
    let t = t0();
    click_at(&mut app, end, 2, t);
    assert_eq!(picked(app.form_text(0)), "beta");
    click_at(&mut app, end, 1, t + Duration::from_millis(100));
    assert_eq!(picked(app.form_text(0)), "alpha beta", "a quick third click picks everything");
    click_at(&mut app, end, 1, t + Duration::from_secs(2));
    drag_words(&mut app, end, start, t + Duration::from_secs(4));
    assert_eq!(picked(app.form_text(0)), "alpha beta", "dragging after a double click goes by words");
    right_click(&mut app, end);
    assert_eq!(text_menu(&mut app)[2], ("Copy".to_owned(), false));
    app.on_event(&Event::MenuCommand(2), t0());
    assert_eq!(clipboard(&mut app), ["alpha beta"]);
    click_at(&mut app, (rows[1].right() - 12.0, y2), 1, t + Duration::from_secs(6));
    typed(&mut app, "hunter2");
    key(&mut app, Key::Char('A'), CTRL);
    right_click(&mut app, (rows[1].right() - 12.0, y2));
    let menu = text_menu(&mut app);
    assert!(menu[1].1 && menu[2].1, "Cut and Copy stay off for a password");
    app.on_event(&Event::MenuCommand(2), t0());
    assert!(clipboard(&mut app).is_empty());
    assert_eq!(app.form_text(1).unwrap().text(), "hunter2");
    assert_eq!(app.form_text(0).unwrap().text(), "alpha beta", "the other field is untouched");
}

#[test]
fn the_inspector_filter_picks_words_drags_and_offers_the_field_menu() {
    let mut app = app_at(r"C:\x");
    files(&mut app, &["a.rs"]);
    key(&mut app, Key::Char('A'), CTRL);
    key(&mut app, Key::Space, NO);
    redraw(&mut app);
    let gen = app.drain_effects().filter_map(|e| if let Effect::Preview(r) = e { Some(r.gen) } else { None }).next_back().expect("a preview request");
    app.on_preview(gen, crate::preview::PreviewData::Doc(crate::preview::text_doc("alpha beta\ngamma", None)));
    assert!(app.run_action("inspector.filter", t0()));
    typed(&mut app, "alpha beta");
    redraw(&mut app);
    let r = app.inspector_filter_rect();
    let y = r.y + r.h / 2.0;
    let (start, end) = ((r.x + 22.0 + 25.0, y), (r.x + 22.0 + 95.0, y));
    let t = t0();
    click_at(&mut app, end, 2, t);
    assert_eq!(picked(app.inspector_filter()), "beta");
    click_at(&mut app, end, 1, t + Duration::from_millis(100));
    assert_eq!(picked(app.inspector_filter()), "alpha beta");
    click_at(&mut app, end, 1, t + Duration::from_secs(2));
    drag_words(&mut app, end, start, t + Duration::from_secs(4));
    assert_eq!(picked(app.inspector_filter()), "alpha beta");
    right_click(&mut app, start);
    assert_eq!(text_menu(&mut app).len(), 6);
    app.on_event(&Event::MenuCommand(1), t0());
    assert_eq!(clipboard(&mut app), ["alpha beta"]);
    assert_eq!(app.inspector_filter().unwrap().text(), "");
    assert_eq!(app.tab().location(), &loc(r"C:\x"), "cutting text never cuts files");
}
