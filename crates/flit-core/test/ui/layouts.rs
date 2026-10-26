use super::*;
use crate::input::{Event, MouseButton};
use crate::listing::{attr, DirSnapshot, Listing, Meta};
use crate::location::Location;
use crate::provider::ProviderRequest;
use crate::sort::{self, CaseMode, SortKeys};
use crate::text::tests::Mono;
use crate::text::Gfx;
use crate::render::kind;
use crate::ui::{Bitmap, Effect, IconKind, IconRequest, IconResult, Persisted};
use std::sync::Arc;
use std::time::Duration;

fn loc(s: &str) -> Location {
    Location::parse(s).unwrap()
}

fn app_with(persisted: Persisted) -> App {
    App::new(persisted, Vec::new(), 0, false, 1.0, (1000, 600), Instant::now())
}

fn app_at(path: &str) -> App {
    app_with(Persisted { path: loc(path), ..Default::default() })
}

fn frame(app: &mut App) {
    app.frame(&mut Gfx::new(Mono, 1024), Instant::now());
}

fn publish(app: &mut App, names: &[String], done: bool) {
    let t = app.tab();
    let mut listing = Listing::default();
    let mut keys = SortKeys::new(CaseMode::Insensitive);
    for n in names {
        let dir = !n.contains('.');
        let i = listing.push(n.as_bytes(), Meta { attrs: if dir { attr::DIR } else { 0 }, ..Default::default() });
        keys.push(listing.name(i));
    }
    let view = t.view_spec;
    let mut order = Vec::new();
    sort::sort(&listing, &keys, view, &mut order);
    let snap = DirSnapshot { gen: t.gen, path: t.location().path().clone(), listing, keys, order, view, done, error: None, elapsed: Duration::ZERO };
    t.slot.publish(Arc::new(snap));
    app.poll();
    frame(app);
}

fn deliver(app: &mut App, names: &[&str]) {
    publish(app, &names.iter().map(|s| s.to_string()).collect::<Vec<_>>(), true);
}

fn files(n: usize, ext: &str) -> Vec<String> {
    (0..n).map(|k| format!("f{k:05}.{ext}")).collect()
}

fn key(app: &mut App, key: Key, mods: Mods) {
    app.on_event(&Event::Key { key, mods, pressed: true }, Instant::now());
    app.on_event(&Event::Key { key, mods: Mods::default(), pressed: false }, Instant::now());
}

const CTRL: Mods = Mods { ctrl: true, shift: false, alt: false };
const CTRL_SHIFT: Mods = Mods { ctrl: true, shift: true, alt: false };
const NO: Mods = Mods { ctrl: false, shift: false, alt: false };

fn focused(app: &App) -> Option<String> {
    let v = app.tab().view.as_ref()?;
    Some(String::from_utf8_lossy(v.listing().name(v.focus?)).into_owned())
}

fn selected(app: &App) -> Vec<String> {
    let v = app.tab().view.as_ref().unwrap();
    v.selected().map(|i| String::from_utf8_lossy(v.listing().name(i)).into_owned()).collect()
}

fn center_of(app: &App, row: usize) -> (f32, f32) {
    let r = app.item_parts(app.row_rect(row).unwrap()).body;
    (r.x + r.w / 2.0, r.y + r.h / 2.0)
}

fn click(app: &mut App, (x, y): (f32, f32)) {
    app.on_event(&Event::MouseDown { x, y, button: MouseButton::Left, clicks: 1 }, Instant::now());
    app.on_event(&Event::MouseUp { x, y, button: MouseButton::Left }, Instant::now());
}

#[test]
fn ctrl_shift_digits_switch_layouts_and_each_folder_remembers_its_own() {
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &["a.txt", "b.txt"]);
    let order = [('5', LayoutMode::Grid), ('2', LayoutMode::List), ('4', LayoutMode::Columns), ('3', LayoutMode::Cards), ('1', LayoutMode::Details)];
    for (digit, mode) in order {
        key(&mut app, Key::Char(digit), CTRL_SHIFT);
        assert_eq!((app.tab().mode, app.tab().look, app.layout.mode), (mode, mode, mode), "Ctrl+Shift+{digit}");
        assert_eq!(app.folder_views().get(folder_key(&loc(r"C:\x"))).unwrap().layout, mode);
    }
    key(&mut app, Key::Char('5'), CTRL_SHIFT);
    assert!(app.layout.header.h == 0.0 && app.layout.flow.per > 1);
    app.run_action("nav.up", Instant::now());
    deliver(&mut app, &["x"]);
    assert_eq!(app.tab().look, LayoutMode::Details, "C:\\ has no view of its own");
    app.go_history(false, Instant::now());
    deliver(&mut app, &["a.txt", "b.txt"]);
    assert_eq!(app.tab().look, LayoutMode::Grid);
}

#[test]
fn the_old_layout_stays_until_the_new_folder_arrives() {
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &["a.txt"]);
    key(&mut app, Key::Char('5'), CTRL_SHIFT);
    app.navigate(loc(r"C:\y"), Instant::now());
    frame(&mut app);
    assert_eq!((app.tab().mode, app.layout.mode), (LayoutMode::Details, LayoutMode::Grid), "the Grid listing of C:\\x is still on screen");
    deliver(&mut app, &["b.txt"]);
    assert_eq!(app.layout.mode, LayoutMode::Details);
}

#[test]
fn ctrl_plus_minus_and_ctrl_wheel_resize_items_and_ctrl_numpad_plus_fits_columns() {
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &["a.txt"]);
    assert_eq!(app.layout.row_h, 24.0);
    key(&mut app, Key::Char('+'), CTRL);
    assert_eq!((app.tab().sizes.level(LayoutMode::Details), app.layout.row_h), (2, 28.0));
    key(&mut app, Key::Char('+'), CTRL_SHIFT);
    assert_eq!(app.layout.row_h, 32.0);
    for _ in 0..6 {
        key(&mut app, Key::Char('-'), CTRL);
    }
    assert_eq!((app.tab().sizes.level(LayoutMode::Details), app.layout.row_h, app.layout.icon), (0, 20.0, 16.0));
    key(&mut app, Key::NumPlus, CTRL);
    assert!(app.tab().fit_pending);
    assert_eq!(app.tab().sizes.level(LayoutMode::Details), 0, "Ctrl+Num+ fits columns instead of resizing");
    key(&mut app, Key::Char('5'), CTRL_SHIFT);
    let icon = app.layout.icon;
    app.on_event(&Event::Key { key: Key::Other, mods: CTRL, pressed: true }, Instant::now());
    app.on_event(&Event::Wheel { dx: 0.0, dy: 1.0 }, Instant::now());
    assert!(app.layout.icon > icon);
    for _ in 0..20 {
        app.on_event(&Event::Wheel { dx: 0.0, dy: 1.0 }, Instant::now());
    }
    assert_eq!(app.tab().sizes.level(LayoutMode::Grid), 11);
    assert_eq!(app.tab().sizes.level(LayoutMode::Details), 0, "every layout keeps its own size");
    let v = app.folder_views().get(folder_key(&loc(r"C:\x"))).unwrap();
    assert_eq!((v.sizes.level(LayoutMode::Grid), v.sizes.level(LayoutMode::Details)), (11, 0));
}

#[test]
fn grid_tiles_take_clicks_gaps_clear_the_selection_and_arrows_move_by_tile_and_band() {
    let mut app = app_at(r"C:\x");
    let names = files(30, "txt");
    publish(&mut app, &names, true);
    key(&mut app, Key::Char('5'), CTRL_SHIFT);
    frame(&mut app);
    let per = app.layout.flow.per;
    assert!(per >= 3, "{per} tiles per band");
    let at = center_of(&app, 1);
    click(&mut app, at);
    assert_eq!(selected(&app), [names[1].clone()]);
    key(&mut app, Key::Right, NO);
    assert_eq!(focused(&app).unwrap(), names[2]);
    key(&mut app, Key::Down, NO);
    assert_eq!(focused(&app).unwrap(), names[2 + per]);
    key(&mut app, Key::Up, NO);
    key(&mut app, Key::Left, NO);
    assert_eq!(focused(&app).unwrap(), names[1]);
    let cell = app.row_rect(2).unwrap();
    assert_eq!(app.hit(cell.x + 1.0, cell.y + 1.0), Hit::ListEmpty);
    click(&mut app, (cell.x + 1.0, cell.y + 1.0));
    assert!(selected(&app).is_empty());
    key(&mut app, Key::End, NO);
    assert_eq!(focused(&app).unwrap(), names[29]);
    assert!(app.tab().scroll > 0.0);
    let last = app.row_rect(29).unwrap();
    assert!(last.bottom() <= app.layout.list.bottom() + 0.5, "the focused tile is revealed");
}

#[test]
fn marquee_in_grid_selects_the_tiles_the_band_touches() {
    let mut app = app_at(r"C:\x");
    let names = files(12, "txt");
    publish(&mut app, &names, true);
    key(&mut app, Key::Char('5'), CTRL_SHIFT);
    frame(&mut app);
    let per = app.layout.flow.per;
    let a = app.row_rect(1).unwrap();
    let b = app.item_parts(app.row_rect(per + 2).unwrap()).body;
    let start = (a.x + a.w / 2.0, a.y + 1.0);
    assert_eq!(app.hit(start.0, start.1), Hit::ListEmpty);
    app.on_event(&Event::MouseDown { x: start.0, y: start.1, button: MouseButton::Left, clicks: 1 }, Instant::now());
    app.on_event(&Event::MouseMove { x: b.x + 4.0, y: b.y + 4.0 }, Instant::now());
    assert_eq!(selected(&app), [1, 2, per + 1, per + 2].map(|k| names[k].clone()));
    app.on_event(&Event::MouseUp { x: b.x + 4.0, y: b.y + 4.0, button: MouseButton::Left }, Instant::now());
}

#[test]
fn list_flows_down_columns_and_scrolls_sideways() {
    let mut app = app_at(r"C:\x");
    let names = files(300, "txt");
    publish(&mut app, &names, true);
    key(&mut app, Key::Char('2'), CTRL_SHIFT);
    frame(&mut app);
    let l = &app.layout;
    let per = l.flow.per;
    assert!(l.flow.across && l.track.y >= l.list.bottom() && l.thumb.w > 0.0);
    assert_eq!(per, (l.list.h / l.row_h).floor() as usize);
    let at = center_of(&app, 0);
    click(&mut app, at);
    key(&mut app, Key::Right, NO);
    assert_eq!(focused(&app).unwrap(), names[per]);
    key(&mut app, Key::Down, NO);
    assert_eq!(focused(&app).unwrap(), names[per + 1]);
    key(&mut app, Key::End, NO);
    assert!(app.tab().scroll > 0.0);
    let r = app.row_rect(299).unwrap();
    assert!(r.right() <= app.layout.list.right() + 0.5 && r.x >= app.layout.list.x);
    key(&mut app, Key::Home, NO);
    app.on_event(&Event::Wheel { dx: 0.0, dy: -1.0 }, Instant::now());
    assert_eq!(app.tab().scroll_target, app.layout.flow.cell.0);
}

#[test]
fn columns_layout_arrows_go_to_the_parent_and_into_folders() {
    let mut app = app_at(r"C:\x\sub");
    deliver(&mut app, &["inner", "z.txt"]);
    key(&mut app, Key::Char('4'), CTRL_SHIFT);
    app.drain_effects().for_each(drop);
    key(&mut app, Key::Down, NO);
    assert_eq!(focused(&app).unwrap(), "inner");
    key(&mut app, Key::Right, NO);
    assert_eq!(app.current_location(), &loc(r"C:\x\sub\inner"));
    deliver(&mut app, &["deep.txt"]);
    assert_eq!(app.layout.mode, LayoutMode::Columns, "the parent's view is inherited");
    key(&mut app, Key::Left, NO);
    assert_eq!(app.current_location(), &loc(r"C:\x\sub"));
    let listed = app.drain_effects().any(|e| matches!(e, Effect::Provider { req: ProviderRequest::List { path, .. }, .. } if path == *loc(r"C:\x\sub").path()));
    assert!(listed);
    deliver(&mut app, &["inner", "z.txt"]);
    assert_eq!(focused(&app).unwrap(), "inner", "going up selects the folder we came from");
}

fn serve_pane(app: &mut App, effects: &[Effect], path: &str, names: &[&str]) -> bool {
    let Some((gen, out, view)) = effects.iter().find_map(|e| match e {
        Effect::Provider { req: ProviderRequest::List { tab: 0, gen, path: p, view, out, .. }, .. } if *p == *loc(path).path() => Some((*gen, out.clone(), *view)),
        _ => None,
    }) else {
        return false;
    };
    let mut listing = Listing::default();
    let mut keys = SortKeys::new(CaseMode::Insensitive);
    for n in names {
        let i = listing.push(n.as_bytes(), Meta { attrs: if n.contains('.') { 0 } else { attr::DIR }, ..Default::default() });
        keys.push(listing.name(i));
    }
    let mut order = Vec::new();
    sort::sort(&listing, &keys, view, &mut order);
    out.publish(Arc::new(DirSnapshot { gen, path: loc(path).path().clone(), listing, keys, order, view, done: true, error: None, elapsed: Duration::ZERO }));
    app.poll();
    frame(app);
    true
}

#[test]
fn columns_show_parent_panes_that_reuse_the_old_listing_and_navigate_on_click() {
    let mut app = app_at(r"C:\x\sub");
    deliver(&mut app, &["inner", "z.txt"]);
    key(&mut app, Key::Char('4'), CTRL_SHIFT);
    let effects: Vec<Effect> = app.drain_effects().collect();
    assert_eq!(app.layout.panes.len(), 2);
    assert!(serve_pane(&mut app, &effects, r"C:\x", &["other", "sub"]));
    assert!(app.layout.list.x > app.layout.panes[1].right() - 1.0);
    let other = app.layout.panes[1];
    assert_eq!(app.hit(other.x + 40.0, other.y + app.layout.row_h * 0.5), Hit::Pane(1, Some(0)));
    key(&mut app, Key::Down, NO);
    key(&mut app, Key::Right, NO);
    let effects: Vec<Effect> = app.drain_effects().collect();
    deliver(&mut app, &["deep.txt"]);
    assert!(!serve_pane(&mut app, &effects, r"C:\x\sub", &[]), "the listing of C:\\x\\sub moves into a pane");
    let t = app.tab();
    assert_eq!(t.panes.iter().map(|p| p.loc.clone()).collect::<Vec<_>>(), [loc(r"C:\x"), loc(r"C:\x\sub")]);
    assert!(t.panes.iter().all(|p| p.view.is_some()));
    let pane = app.layout.panes[0];
    let y = pane.y + app.layout.row_h * 0.5;
    click(&mut app, (pane.x + 40.0, y));
    assert_eq!(app.current_location(), &loc(r"C:\x\other"));
}

#[test]
fn adaptive_shows_media_folders_as_a_grid_and_keeps_its_first_choice_while_loading() {
    let mut app = app_at(r"C:\pics");
    let media = files(40, "jpg");
    publish(&mut app, &media[..10], false);
    key(&mut app, Key::Char('6'), CTRL_SHIFT);
    assert_eq!((app.tab().mode, app.tab().look), (LayoutMode::Adaptive, LayoutMode::Grid));
    let mut mixed = media.clone();
    mixed.extend(files(200, "txt"));
    publish(&mut app, &mixed, true);
    assert_eq!(app.tab().look, LayoutMode::Grid, "no jump once the listing grows");
    app.refresh(Instant::now());
    publish(&mut app, &mixed[..10], false);
    assert_eq!(app.tab().look, LayoutMode::Details, "the next visit starts from the final decision");
}

#[test]
fn adaptive_follows_the_desktop_ini_folder_type() {
    let mut persisted = Persisted { path: loc(r"C:\docs"), ..Default::default() };
    persisted.prefs.defaults.layout = LayoutMode::Adaptive;
    let mut app = app_with(persisted);
    let t = app.tab();
    assert!(t.slot.wants_kind(t.gen));
    t.slot.set_kind(t.gen, FolderKind::Pictures);
    deliver(&mut app, &["a.txt", "b.txt"]);
    assert_eq!(app.tab().look, LayoutMode::Grid);
}

#[test]
fn adaptive_is_refused_while_all_folders_share_one_view() {
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &["a.txt"]);
    app.run_action("view.same_for_all", Instant::now());
    key(&mut app, Key::Char('6'), CTRL_SHIFT);
    assert_eq!(app.tab().mode, LayoutMode::Details);
    assert_eq!(app.tab().error.as_deref(), Some(S::LayoutsAdaptiveShared.t()));
}

#[test]
fn sizes_fall_back_per_layout_when_out_of_range() {
    assert_eq!(Sizes([9, 0, 4, 2, 11]).sanitized(), Sizes([1, 0, 1, 2, 11]));
    assert!(is_media(b"JPG") && is_media(b"mkv") && !is_media(b"pdf") && has_thumbnail(b"pdf") && !has_thumbnail(b"txt"));
}

fn thumbs_asked(app: &mut App) -> Vec<IconRequest> {
    app.drain_effects().filter_map(|e| if let Effect::Thumbnail(r) = e { Some(r) } else { None }).collect()
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

#[test]
#[ignore]
fn bench_layouts_on_100k() {
    let mut app = app_at(r"C:\big");
    let mut gfx = Gfx::new(Mono, 1024);
    publish(&mut app, &files(100_000, "jpg"), true);
    app.frame(&mut gfx, Instant::now());
    for (digit, mode) in [('2', LayoutMode::List), ('3', LayoutMode::Cards), ('4', LayoutMode::Columns), ('5', LayoutMode::Grid), ('6', LayoutMode::Adaptive), ('1', LayoutMode::Details)] {
        let t = Instant::now();
        key(&mut app, Key::Char(digit), CTRL_SHIFT);
        app.frame(&mut gfx, Instant::now());
        eprintln!("BENCH switch to {:<9} on 100k entries, key to finished frame {:>7.3} ms", mode.title(), ms(t.elapsed()));
        let _ = app.drain_effects().count();
    }
    key(&mut app, Key::Char('5'), CTRL_SHIFT);
    let fake = Bitmap { width: 128, height: 96, rgba: vec![200; 128 * 96 * 4] };
    let mut times = Vec::new();
    for _ in 0..600 {
        for r in thumbs_asked(&mut app) {
            app.on_thumb(&mut gfx, r.key, IconResult::Ready(fake.clone()));
        }
        gfx.thumbs.uploads.clear();
        app.on_event(&Event::Wheel { dx: 0.0, dy: -1.0 }, Instant::now());
        let t = Instant::now();
        app.frame(&mut gfx, Instant::now());
        times.push(t.elapsed());
    }
    times.sort();
    let at = |q: f64| ms(times[((times.len() - 1) as f64 * q) as usize]);
    eprintln!("BENCH Grid scroll over 100k thumbnails, frame CPU  median {:.3} ms  p99 {:.3} ms  max {:.3} ms  ({} thumbnails in the atlas, {} quads, {} on screen, scrolled to {})", at(0.5), at(0.99), at(1.0), gfx.thumbs.len(), gfx.dl.instances.len(), thumb_quads(&gfx), app.tab().scroll);
}

fn thumb_quads(gfx: &Gfx<Mono>) -> usize {
    gfx.dl.instances.iter().filter(|q| q.kind == kind::THUMB).count()
}

#[test]
fn grid_asks_for_each_thumbnail_once_and_draws_it_while_details_never_asks() {
    let mut app = app_at(r"C:\pics");
    let mut gfx = Gfx::new(Mono, 1024);
    deliver(&mut app, &["a.jpg", "b.txt", "c"]);
    app.frame(&mut gfx, Instant::now());
    assert!(thumbs_asked(&mut app).is_empty());
    key(&mut app, Key::Char('5'), CTRL_SHIFT);
    app.frame(&mut gfx, Instant::now());
    let asked = thumbs_asked(&mut app);
    assert_eq!(asked.len(), 1);
    assert!(asked[0].kind == IconKind::Thumbnail && asked[0].path == *loc(r"C:\pics\a.jpg").path());
    app.on_thumb(&mut gfx, asked[0].key, IconResult::Ready(Bitmap { width: 40, height: 30, rgba: vec![255; 4800] }));
    app.frame(&mut gfx, Instant::now());
    assert!(thumbs_asked(&mut app).is_empty());
    assert_eq!((thumb_quads(&gfx), gfx.thumbs.uploads.rects.len()), (1, 1));
    app.run_action("view.thumbnails", Instant::now());
    app.frame(&mut gfx, Instant::now());
    assert_eq!(thumb_quads(&gfx), 0, "the toggle turns thumbnails off");
}
