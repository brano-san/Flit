use super::*;
use crate::input::{CursorIcon, MouseButton};
use crate::listing::{attr, DirSnapshot, Listing, Meta};
use crate::path::FsPath;
use crate::provider::ProviderRequest;
use crate::sort::{self, CaseMode, SortKeys};
use crate::text::tests::Mono;
use crate::ui::{drop_effect, MenuItem, Persisted};
use std::sync::Arc;
use std::time::Duration;

const NO: Mods = Mods { ctrl: false, shift: false, alt: false };
const CTRL: Mods = Mods { ctrl: true, shift: false, alt: false };
const CTRL_SHIFT: Mods = Mods { ctrl: true, shift: true, alt: false };
const ALT_SHIFT: Mods = Mods { ctrl: false, shift: true, alt: true };
const CTRL_ALT: Mods = Mods { ctrl: true, shift: false, alt: true };

fn t0() -> Instant {
    Instant::now()
}

fn loc(s: &str) -> Location {
    Location::parse(s).unwrap()
}

fn app_sized(path: &str, width: u32) -> App {
    let mut app = App::new(Persisted { path: loc(path), ..Default::default() }, Vec::new(), 0, false, 1.0, (width, 600), t0());
    app.drain_effects().for_each(drop);
    app
}

fn app_at(path: &str) -> App {
    app_sized(path, 1000)
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

fn deliver_other(app: &mut App, items: &[(&str, bool)]) {
    publish(app.other_pane().unwrap(), items);
    app.poll();
}

fn key(app: &mut App, key: Key, mods: Mods) {
    app.on_event(&Event::Key { key, mods, pressed: true }, t0());
}

fn ev(app: &mut App, e: Event) {
    app.on_event(&e, t0());
}

fn listed(app: &mut App) -> Vec<(TabId, Location)> {
    app.drain_effects().filter_map(|e| if let Effect::Provider { req: ProviderRequest::List { tab, path, .. }, .. } = e { Some((tab, Location::Local(path))) } else { None }).collect()
}

fn labels(items: &[MenuItem]) -> Vec<String> {
    items
        .iter()
        .flat_map(|i| match i {
            MenuItem::Command { label, checked, .. } => vec![format!("{label}{}", if *checked { " *" } else { "" })],
            MenuItem::Submenu { items, .. } => labels(items),
            MenuItem::Separator => Vec::new(),
        })
        .collect()
}

fn menu(app: &mut App) -> Vec<String> {
    let m = app.drain_effects().find_map(|e| if let Effect::ShellMenu(m) = e { Some(m.menu) } else { None }).expect("menu shown");
    labels(&m)
}

fn row_in(l: &Layout, row: usize) -> (f32, f32) {
    (l.cols[0].x + 60.0, l.list.y + (row as f32 + 0.5) * l.row_h)
}

fn split_loaded(path: &str) -> (App, TabId, TabId) {
    let mut app = app_at(path);
    deliver(&mut app, &[("sub", true), ("a.txt", false), ("b.txt", false)]);
    let left = app.tab().id;
    key(&mut app, Key::Char('V'), ALT_SHIFT);
    ev(&mut app, Event::Key { key: Key::Char('V'), mods: NO, pressed: false });
    deliver(&mut app, &[("sub", true), ("a.txt", false), ("b.txt", false)]);
    let right = app.tab().id;
    app.drain_effects().for_each(drop);
    (app, left, right)
}

#[test]
fn splitting_opens_this_folder_on_the_right_and_moves_the_focus_there() {
    let mut app = app_at(r"C:\a");
    deliver(&mut app, &[("sub", true)]);
    let left = app.tab().id;
    key(&mut app, Key::Char('V'), ALT_SHIFT);
    let right = app.tab().id;
    assert_ne!(left, right);
    assert_eq!(app.tab().location(), &loc(r"C:\a"));
    assert_eq!(app.other_pane().map(|t| t.id), Some(left));
    assert_eq!(app.other_pane().unwrap().rows(), 1, "the first pane keeps its listing");
    assert_eq!(listed(&mut app), [(right, loc(r"C:\a"))]);
    assert!(app.layout.pane.x > app.alt.layout.pane.right(), "the new pane is on the right");
    assert!(app.layout.splitter.w > 0.0 && app.layout.splitter.x >= app.alt.layout.pane.right());
    key(&mut app, Key::Right, CTRL_SHIFT);
    assert_eq!(app.tab().id, left);
    assert!(app.drain_effects().any(|e| matches!(e, Effect::Title(_))));
    assert!(app.layout.pane.right() < app.alt.layout.pane.x);
    key(&mut app, Key::Char('H'), ALT_SHIFT);
    assert!(app.layout.pane.bottom() <= app.alt.layout.pane.y, "the panes now stand one above the other");
    assert_eq!(app.tab().id, left, "arranging keeps the focus");
    let s = app.split_sessions();
    assert_eq!(s[0].as_ref().map(|s| (s.dir, s.second)), Some((SplitDir::Horizontal, false)));
}

#[test]
fn a_click_in_the_other_pane_focuses_it_and_lands_on_its_row() {
    let (mut app, left, right) = split_loaded(r"C:\a");
    let (x, y) = row_in(&app.alt.layout, 2);
    ev(&mut app, Event::MouseMove { x, y });
    ev(&mut app, Event::MouseDown { x, y, button: MouseButton::Left, clicks: 1 });
    ev(&mut app, Event::MouseUp { x, y, button: MouseButton::Left });
    assert_eq!(app.tab().id, left);
    assert_eq!(app.other_pane().unwrap().id, right);
    let v = app.tab().view.as_ref().unwrap();
    assert_eq!(v.selected().map(|i| v.listing().name(i).to_vec()).collect::<Vec<_>>(), [b"b.txt".to_vec()]);
    assert!(app.other_pane().unwrap().view.as_ref().unwrap().selected_count() == 0);
}

#[test]
fn folders_open_in_the_other_pane_from_the_keyboard() {
    let mut app = app_at(r"C:\a");
    deliver(&mut app, &[("sub", true), ("a.txt", false)]);
    let first = app.tab().id;
    key(&mut app, Key::Down, NO);
    key(&mut app, Key::Enter, CTRL_SHIFT);
    assert_eq!(app.tab().location(), &loc(r"C:\a\sub"));
    assert_eq!(app.other_pane().map(|t| (t.id, t.location().clone())), Some((first, loc(r"C:\a"))));
    deliver(&mut app, &[("deep", true)]);
    key(&mut app, Key::Right, CTRL_SHIFT);
    assert_eq!(app.tab().id, first);
    key(&mut app, Key::Enter, CTRL_SHIFT);
    assert_eq!(app.tab().location(), &loc(r"C:\a\sub"), "the selected folder replaces the folder of the other pane");
    assert_ne!(app.tab().id, first);
    app.run_action("pane.focus", t0());
    app.run_action("pane.here_other", t0());
    assert_eq!(app.tab().location(), &loc(r"C:\a"));
    key(&mut app, Key::Left, Mods { alt: true, ..NO });
    assert_eq!(app.tab().location(), &loc(r"C:\a\sub"), "each pane keeps its own history");
    assert_eq!(app.other_pane().unwrap().location(), &loc(r"C:\a"));
}

#[test]
fn closing_and_toggling_panes_keeps_the_right_one() {
    let (mut app, left, right) = split_loaded(r"C:\a");
    key(&mut app, Key::Char('W'), CTRL_ALT);
    assert!(app.other_pane().is_none());
    assert_eq!(app.tab().id, left, "closing the active pane leaves the other one");
    assert!(app.drain_effects().any(|e| matches!(e, Effect::Provider { req: ProviderRequest::Watch { tab, path: None, .. }, .. } if tab == right)));
    app.run_action("pane.close", t0());
    assert_eq!(app.tab().error.as_deref(), Some(ONE_PANE));
    key(&mut app, Key::Char('S'), CTRL_SHIFT);
    let added = app.tab().id;
    assert_eq!(app.other_pane().map(|t| t.id), Some(left));
    key(&mut app, Key::Char('S'), CTRL_SHIFT);
    assert!(app.other_pane().is_none());
    assert_eq!(app.tab().id, added, "toggling off closes the pane without the focus");
}

#[test]
fn narrow_windows_hide_the_second_pane_and_refuse_a_new_one() {
    let mut app = app_sized(r"C:\a", 700);
    key(&mut app, Key::Char('V'), ALT_SHIFT);
    assert!(app.other_pane().is_none());
    assert_eq!(app.tab().error.as_deref(), Some(TOO_NARROW));
    ev(&mut app, Event::Resized { width: 1000, height: 600 });
    key(&mut app, Key::Char('V'), ALT_SHIFT);
    assert!(app.split_shown());
    let full = app.layout.content;
    ev(&mut app, Event::Resized { width: 740, height: 600 });
    assert!(app.other_pane().is_some() && !app.split_shown());
    assert_eq!(app.layout.pane.w, app.layout.content.w, "the active pane takes the whole width");
    assert_eq!(app.layout.splitter, Rect::default());
    assert!(full.w > app.layout.content.w);
    ev(&mut app, Event::Resized { width: 1000, height: 600 });
    assert!(app.split_shown());
}

#[test]
fn listings_and_changes_reach_the_pane_without_the_focus() {
    let mut app = app_at(r"C:\a");
    key(&mut app, Key::Char('V'), ALT_SHIFT);
    deliver(&mut app, &[("x", false)]);
    deliver_other(&mut app, &[("one", false), ("two", false)]);
    let other = app.other_pane().unwrap();
    let (id, gen) = (other.id, other.gen);
    assert_eq!((other.rows(), app.tab().rows()), (2, 1));
    app.on_provider_event(ProviderEvent::Changes { tab: id, gen, changes: Vec::new() }, t0());
    app.drain_effects().for_each(drop);
    app.on_provider_event(ProviderEvent::Rescan { tab: id, gen }, t0());
    assert_eq!(listed(&mut app), [(id, loc(r"C:\a"))]);
    deliver_other(&mut app, &[("one", false), ("two", false), ("three", false)]);
    assert_eq!(app.other_pane().unwrap().rows(), 3);
    let mut g = Gfx::new(Mono, 2048);
    app.frame(&mut g, t0());
    assert_eq!(app.other_visible_rows().map(|(g, first, _)| (g, first)), Some((app.other_pane().unwrap().gen, 0)));
    assert_eq!(app.visible_rows().0, app.tab().gen);
    let both = g.dl.instances.len();
    app.run_action("pane.close", t0());
    app.frame(&mut g, t0());
    assert!(g.dl.instances.len() < both, "the second pane is drawn");
    assert_eq!(app.other_visible_rows(), None);
}

#[test]
fn sessions_restore_panes_per_tab() {
    let mut app = app_at(r"C:\a");
    key(&mut app, Key::Char('H'), ALT_SHIFT);
    app.on_event(&Event::Key { key: Key::Char('T'), mods: CTRL, pressed: true }, t0());
    let s = app.session();
    assert_eq!(s.splits.len(), 2);
    assert_eq!(s.splits[0].as_ref().map(|p| (p.other.clone(), p.dir, p.second)), Some((loc(r"C:\a"), SplitDir::Horizontal, true)));
    assert!(s.splits[1].is_none());
    let mut back = app_at(r"C:\a");
    back.restore_tabs(&s.tabs, 0);
    back.drain_effects().for_each(drop);
    back.restore_splits(&s.splits);
    assert_eq!(back.split_sessions(), s.splits);
    assert_eq!(listed(&mut back).len(), 1, "the visible tab loads its second pane at once");
    back.restore_splits(&[]);
    assert!(back.other_pane().is_some(), "a session from an older build leaves the panes alone");
    back.restore_splits(&[None, None]);
    assert!(back.other_pane().is_none());
}

#[test]
fn menus_offer_splits_arrangement_and_the_other_pane() {
    let mut app = app_at(r"C:\a");
    deliver(&mut app, &[("sub", true)]);
    app.tab_actions_menu();
    let m = menu(&mut app);
    assert!(m.contains(&"Split vertically".into()) && m.contains(&"Split horizontally".into()) && m.contains(&"Open new tabs with two panes".into()));
    key(&mut app, Key::Down, NO);
    app.context_menu();
    let m = menu(&mut app);
    let tab = m.iter().position(|l| l == "Open in new tab").unwrap();
    assert_eq!(m[tab + 2..tab + 4], ["Split vertically".to_owned(), "Split horizontally".to_owned()]);
    app.run_action("pane.new_horizontal", t0());
    assert_eq!(app.tab().location(), &loc(r"C:\a\sub"));
    app.tab_actions_menu();
    let m = menu(&mut app);
    assert!(m.contains(&"Arrange one above the other *".into()) && m.contains(&"Arrange side by side".into()) && m.contains(&"Close pane".into()));
    app.run_action("pane.focus", t0());
    app.context_menu();
    let m = menu(&mut app);
    assert!(m.contains(&"Open in other pane".into()));
    app.on_event(&Event::Key { key: Key::Escape, mods: NO, pressed: true }, t0());
    app.context_menu();
    let m = menu(&mut app);
    assert!(m.contains(&"Open in other pane".into()) && m.contains(&"Close pane".into()), "{m:?}");
}

#[test]
fn the_splitter_drags_and_a_double_click_evens_the_panes() {
    let (mut app, ..) = split_loaded(r"C:\a");
    let bar = app.layout.splitter;
    let (x, y) = (bar.x + bar.w / 2.0, bar.y + 200.0);
    ev(&mut app, Event::MouseMove { x, y });
    assert!(app.drain_effects().any(|e| matches!(e, Effect::Cursor(CursorIcon::ResizeHorizontal))));
    let rev = app.session_rev();
    ev(&mut app, Event::MouseDown { x, y, button: MouseButton::Left, clicks: 1 });
    ev(&mut app, Event::MouseMove { x: x - 100.0, y });
    ev(&mut app, Event::MouseUp { x: x - 100.0, y, button: MouseButton::Left });
    let ratio = app.split_sessions()[0].as_ref().unwrap().ratio;
    assert!((ratio - (0.5 - 100.0 / (app.layout.content.w - 5.0))).abs() < 0.01, "{ratio}");
    assert!(app.session_rev() > rev);
    assert!(app.alt.layout.pane.w < app.layout.pane.w);
    let bar = app.layout.splitter;
    let (x, y) = (bar.x + bar.w / 2.0, bar.y + 200.0);
    ev(&mut app, Event::MouseDown { x, y, button: MouseButton::Left, clicks: 2 });
    ev(&mut app, Event::MouseUp { x, y, button: MouseButton::Left });
    assert_eq!(app.split_sessions()[0].as_ref().unwrap().ratio, 0.5);
}

#[test]
fn the_wheel_scrolls_the_other_pane_without_moving_the_focus() {
    let mut app = app_at(r"C:\a");
    key(&mut app, Key::Char('V'), ALT_SHIFT);
    let names: Vec<String> = (0..300).map(|i| format!("f{i}.txt")).collect();
    let items: Vec<(&str, bool)> = names.iter().map(|n| (n.as_str(), false)).collect();
    deliver(&mut app, &items);
    deliver_other(&mut app, &items);
    let right = app.tab().id;
    let (x, y) = row_in(&app.alt.layout, 3);
    ev(&mut app, Event::MouseMove { x, y });
    ev(&mut app, Event::Wheel { dx: 0.0, dy: -3.0 });
    assert_eq!(app.tab().id, right);
    assert!(app.other_pane().unwrap().scroll_target > 0.0);
    assert_eq!(app.tab().scroll_target, 0.0);
}

#[test]
fn new_tabs_open_with_two_panes_when_asked() {
    let mut app = app_at(r"C:\a");
    app.run_action("pane.dual_default", t0());
    assert!(app.persisted().split.dual);
    key(&mut app, Key::Char('T'), CTRL);
    assert_eq!(app.tabs().len(), 2);
    assert_eq!(app.other_pane().map(|t| t.location().clone()), Some(loc(r"C:\a")));
    assert_eq!(app.split_sessions()[1].as_ref().map(|s| s.second), Some(false), "the first pane keeps the focus");
    app.run_action("pane.default_direction", t0());
    assert_eq!(app.persisted().split.dir, SplitDir::Horizontal);
}

#[test]
fn dragging_files_over_the_other_pane_focuses_it_for_the_drop() {
    let (mut app, left, _) = split_loaded(r"C:\a");
    let (x, y) = row_in(&app.alt.layout, 0);
    let paths = vec![FsPath::parse(r"D:\in\f.txt").unwrap()];
    let effect = app.drag(&DragEvent::Enter { paths, x, y, mods: NO, allowed: drop_effect::COPY | drop_effect::MOVE }, t0());
    assert_eq!(app.tab().id, left);
    assert_ne!(effect, drop_effect::NONE);
}

#[test]
fn moved_tabs_take_their_second_pane_along() {
    let (mut app, ..) = split_loaded(r"C:\a");
    app.on_event(&Event::Key { key: Key::Char('T'), mods: CTRL, pressed: true }, t0());
    app.select_tab(0);
    app.move_tab_to_window();
    let moved = app.drain_effects().find_map(|e| if let Effect::MoveTab { tab, .. } = e { Some(tab) } else { None }).unwrap();
    let mut other = App::new(Persisted { path: loc(r"C:\z"), ..Default::default() }, Vec::new(), 100, false, 1.0, (1000, 600), t0());
    other.drain_effects().for_each(drop);
    other.insert_tab(moved, None, t0());
    let ids: Vec<TabId> = listed(&mut other).into_iter().map(|(id, _)| id).collect();
    assert_eq!(ids.len(), 2);
    assert!(ids.iter().all(|&id| id > 100), "both panes get ids of the new window");
    assert!(other.other_pane().is_some());
}
