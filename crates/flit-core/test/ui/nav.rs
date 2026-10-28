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
            Drive { path: FsPath::parse(r"C:\").unwrap(), label: "Local Disk (C:)".into(), total: 1000, free: 250, removable: false, unmounted: false, kind: None, remote: None },
            Drive { path: FsPath::parse(r"E:\").unwrap(), label: "USB Drive (E:)".into(), total: 0, free: 0, removable: true, unmounted: false, kind: None, remote: None },
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
    assert!(app.layout.header.y > header_y, "the filter bar pushes the list down");
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
    let shown = app.layout.list.x;
    assert_eq!(shown, app.nav_state().sidebar_w);
    key(&mut app, Key::Char('B'), CTRL);
    assert_eq!(app.layout.list.x, 0.0);
    assert!(!app.nav_state().sidebar);
    key(&mut app, Key::Char('B'), CTRL);
    assert_eq!(app.layout.list.x, shown);
    let sp = app.nav.sidebar.splitter;
    let (x, y) = (sp.x + sp.w / 2.0, sp.y + 40.0);
    app.on_event(&Event::MouseMove { x, y }, t0());
    assert_eq!(app.cursor, CursorIcon::ResizeHorizontal);
    app.on_event(&Event::MouseDown { x, y, button: MouseButton::Left, clicks: 1 }, t0());
    app.on_event(&Event::MouseMove { x: x + 60.0, y }, t0());
    app.on_event(&Event::MouseUp { x: x + 60.0, y, button: MouseButton::Left }, t0());
    assert_eq!(app.nav_state().sidebar_w, 280.0);
    assert_eq!(app.layout.list.x, 280.0);
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
