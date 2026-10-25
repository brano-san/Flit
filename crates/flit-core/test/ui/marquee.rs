use super::*;
use crate::input::{Event, Key, Mods, MouseButton};
use crate::listing::{DirSnapshot, Listing, Meta};
use crate::location::Location;
use crate::sort::{self, CaseMode, SortKeys, ViewSpec};
use crate::text::tests::Mono;
use crate::ui::{Effect, Persisted};
use std::sync::Arc;
use std::time::{Duration, Instant};

const NO: Mods = Mods { ctrl: false, shift: false, alt: false };
const CTRL: Mods = Mods { ctrl: true, shift: false, alt: false };

fn app_with(names: &[String]) -> App {
    let state = Persisted { path: Location::parse(r"C:\m").unwrap(), ..Default::default() };
    let mut app = App::new(state, Vec::new(), 0, false, 1.0, (1000, 600), Instant::now());
    let t = app.tab();
    let mut listing = Listing::default();
    let mut keys = SortKeys::new(CaseMode::Insensitive);
    for n in names {
        let i = listing.push(n.as_bytes(), Meta { attrs: 0, size: 1, ..Default::default() });
        keys.push(listing.name(i));
    }
    let mut order = Vec::new();
    sort::sort(&listing, &keys, ViewSpec::default(), &mut order);
    let snap = DirSnapshot { gen: t.gen, path: t.location().path().clone(), listing, keys, order, view: ViewSpec::default(), done: true, error: None, elapsed: Duration::ZERO };
    t.slot.publish(Arc::new(snap));
    app.poll();
    app.drain_effects().for_each(drop);
    app
}

fn five() -> App {
    app_with(&["a", "b", "c", "d", "e"].map(String::from))
}

fn row_y(app: &App, row: f32) -> f32 {
    app.layout.list.y + (row + 0.5) * app.layout.row_h
}

fn detail_x(app: &App) -> f32 {
    app.layout.cols[1].x + 10.0
}

fn press(app: &mut App, x: f32, y: f32, mods: Mods) {
    app.on_event(&Event::Key { key: Key::Other, mods, pressed: false }, Instant::now());
    app.on_event(&Event::MouseDown { x, y, button: MouseButton::Left, clicks: 1 }, Instant::now());
}

fn drag_to(app: &mut App, x: f32, y: f32) {
    app.on_event(&Event::MouseMove { x, y }, Instant::now());
}

fn release(app: &mut App, x: f32, y: f32) {
    app.on_event(&Event::MouseUp { x, y, button: MouseButton::Left }, Instant::now());
}

fn select(app: &mut App, names: &[&str]) {
    let v = app.tabs[app.cur].view.as_mut().unwrap();
    v.clear_selection();
    for n in names {
        let i = v.find(n.as_bytes()).unwrap();
        v.set_selected(i, true);
    }
}

fn selected(app: &App) -> Vec<String> {
    let v = app.tab().view.as_ref().unwrap();
    v.selected().map(|i| String::from_utf8_lossy(v.listing().name(i)).into_owned()).collect()
}

fn name_of(app: &App, i: Option<u32>) -> String {
    String::from_utf8_lossy(app.tab().view.as_ref().unwrap().listing().name(i.unwrap())).into_owned()
}

#[test]
fn dragging_from_empty_space_selects_the_rows_it_crosses() {
    let mut app = five();
    select(&mut app, &["a"]);
    let (x, y) = (detail_x(&app), row_y(&app, 6.0));
    press(&mut app, x, y, NO);
    assert!(selected(&app).is_empty());
    let y = row_y(&app, 2.0);
    drag_to(&mut app, x, y);
    assert_eq!(app.drag, Drag::Marquee);
    assert_eq!(selected(&app), ["c", "d", "e"]);
    let y = row_y(&app, 4.0);
    drag_to(&mut app, x, y);
    assert_eq!(selected(&app), ["e"], "shrinking the band drops the rows it leaves");
    let y = row_y(&app, 4.0);
    release(&mut app, x, y);
    assert_eq!(selected(&app), ["e"]);
    assert!(app.marquee.is_none() && app.drag == Drag::None);
}

#[test]
fn ctrl_marquee_toggles_against_the_existing_selection() {
    let mut app = five();
    select(&mut app, &["a", "c"]);
    let x = detail_x(&app);
    let y = row_y(&app, 6.0);
    press(&mut app, x, y, CTRL);
    let y = row_y(&app, 1.0);
    drag_to(&mut app, x, y);
    assert_eq!(selected(&app), ["a", "b", "d", "e"]);
}

#[test]
fn pressing_a_detail_column_selects_a_range_instead_of_dragging_the_file() {
    let mut app = five();
    let x = detail_x(&app);
    let y = row_y(&app, 1.0);
    press(&mut app, x, y, NO);
    assert_eq!(selected(&app), ["b"]);
    let y = row_y(&app, 3.0);
    drag_to(&mut app, x, y);
    assert_eq!(selected(&app), ["b", "c", "d"]);
    assert!(!app.drain_effects().any(|e| matches!(e, Effect::StartDrag(_))));
    let v = app.tab().view.as_ref().unwrap();
    let (anchor, focus) = (v.anchor, v.focus);
    assert_eq!((name_of(&app, anchor), name_of(&app, focus)), ("b".into(), "d".into()));
}

#[test]
fn a_click_on_a_detail_column_without_moving_selects_one_row() {
    let mut app = five();
    select(&mut app, &["a", "c"]);
    let (x, y) = (detail_x(&app), row_y(&app, 3.0));
    press(&mut app, x, y, NO);
    drag_to(&mut app, x + 1.0, y + 1.0);
    release(&mut app, x + 1.0, y + 1.0);
    assert_eq!(selected(&app), ["d"]);
    assert_eq!(app.drag, Drag::None);
}

#[test]
fn holding_the_pointer_below_the_list_scrolls_and_extends_the_band() {
    let names: Vec<String> = (0..200).map(|i| format!("f{i:03}")).collect();
    let mut app = app_with(&names);
    let mut g = Gfx::new(Mono, 1024);
    let now = Instant::now();
    app.frame(&mut g, now);
    let x = detail_x(&app);
    let y = row_y(&app, 0.0);
    press(&mut app, x, y, NO);
    let below = app.layout.list.bottom() + 40.0;
    drag_to(&mut app, x, below);
    let visible = selected(&app).len();
    assert!(app.frame(&mut g, now + Duration::from_millis(50)));
    assert!(app.frame(&mut g, now + Duration::from_millis(100)));
    assert!(app.tab().scroll > 0.0);
    let grown = selected(&app);
    assert!(grown.len() > visible, "{} rows after autoscroll, {visible} before", grown.len());
    assert_eq!(grown[0], "f000");
    release(&mut app, x, below);
    assert!(app.marquee.is_none());
}
