use super::*;
use crate::caps::Caps;
use crate::listing::{DirSnapshot, Listing, Meta};
use crate::location::Location;
use crate::sort::{self, CaseMode, SortKeys};
use crate::text::tests::Mono;
use crate::text::Gfx;
use crate::ui::{Effect, Persisted};
use std::sync::Arc;

fn list(path: &str, n: usize, caps: Caps) -> (App, Instant) {
    let t0 = Instant::now();
    let mut app = App::new(Persisted { path: Location::parse(path).unwrap(), ..Default::default() }, Vec::new(), 0, false, 1.0, (1000, 600), t0);
    app.set_caps(caps);
    let t = app.tab();
    let mut listing = Listing::default();
    let mut keys = SortKeys::new(CaseMode::Insensitive);
    for k in 0..n {
        let i = listing.push(format!("f{k:03}.txt").as_bytes(), Meta { size: 1, ..Default::default() });
        keys.push(listing.name(i));
    }
    let view = t.view_spec;
    let mut order = Vec::new();
    sort::sort(&listing, &keys, view, &mut order);
    let snap = DirSnapshot { gen: t.gen, path: t.location().path().clone(), listing, keys, order, view, done: true, error: None, elapsed: Duration::ZERO };
    t.slot.publish(Arc::new(snap));
    app.poll();
    app.frame(&mut Gfx::new(Mono, 1024), t0);
    app.drain_effects().for_each(drop);
    (app, t0)
}

fn linux() -> Caps {
    Caps { unix_paths: true }
}

fn selected(app: &App) -> Vec<u32> {
    app.tab().view.as_ref().map_or_else(Vec::new, |v| v.selected().collect())
}

fn menus(app: &mut App) -> usize {
    app.drain_effects().filter(|e| matches!(e, Effect::ShellMenu(_))).count()
}

fn spot(app: &App) -> (f32, f32) {
    (app.layout.cols[0].x + 40.0, app.layout.list.y + 3.5 * app.layout.row_h)
}

fn hold(app: &mut App, (x, y): (f32, f32), at: Instant) {
    app.on_event(&Event::Pointer { touch: true }, at);
    app.on_event(&Event::MouseDown { x, y, button: MouseButton::Left, clicks: 1 }, at);
    app.on_event(&Event::MouseMove { x: x + 2.0, y: y + 2.0 }, at + Duration::from_millis(200));
    app.on_event(&Event::Timer, at + HOLD + Duration::from_millis(20));
}

#[test]
fn holding_a_finger_on_linux_opens_the_context_menu_once_and_the_lift_does_nothing_more() {
    let (mut app, t0) = list("/home/u", 50, linux());
    let at = spot(&app);
    let t = t0 + Duration::from_secs(1);
    hold(&mut app, at, t);
    assert_eq!(menus(&mut app), 1, "a held finger opens the item menu");
    let picked = selected(&app);
    assert_eq!(picked.len(), 1, "for the item under the finger");
    app.on_event(&Event::Timer, t + Duration::from_secs(2));
    app.on_event(&Event::MouseUp { x: at.0 + 2.0, y: at.1 + 2.0, button: MouseButton::Left }, t + Duration::from_secs(2));
    assert_eq!(menus(&mut app), 0, "lifting the finger opens nothing more");
    assert_eq!(selected(&app), picked);
    assert!(app.ops_idle() && app.marquee.is_none() && app.drag == Drag::None);
}

#[test]
fn a_short_tap_or_a_moving_finger_never_opens_the_menu_and_windows_holds_stay_native() {
    let (mut app, t0) = list("/home/u", 50, linux());
    let at = spot(&app);
    let t = t0 + Duration::from_secs(1);
    app.touch_tap(at, t);
    app.on_event(&Event::Timer, t + HOLD + Duration::from_millis(20));
    assert_eq!(menus(&mut app), 0, "a tap that ended before the hold time is a click");
    assert_eq!(selected(&app).len(), 1);
    let t = t + Duration::from_secs(5);
    app.on_event(&Event::Pointer { touch: true }, t);
    app.on_event(&Event::MouseDown { x: at.0, y: at.1, button: MouseButton::Left, clicks: 1 }, t);
    app.on_event(&Event::MouseMove { x: at.0, y: at.1 - 60.0 }, t + Duration::from_millis(100));
    app.on_event(&Event::Timer, t + HOLD + Duration::from_millis(20));
    assert_eq!(menus(&mut app), 0, "a finger that pans does not open the menu");
    app.on_event(&Event::MouseUp { x: at.0, y: at.1 - 60.0, button: MouseButton::Left }, t + HOLD + Duration::from_millis(30));

    let (mut win, t0) = list(r"C:\x", 50, Caps::default());
    let at = spot(&win);
    hold(&mut win, at, t0 + Duration::from_secs(1));
    assert_eq!(menus(&mut win), 0, "Windows turns a held finger into a right click itself");
}

#[test]
fn a_second_finger_cancels_the_tap_and_the_glide_without_clicking() {
    let (mut app, t0) = list(r"C:\x", 200, Caps::default());
    let (x, y) = spot(&app);
    let t = t0 + Duration::from_secs(1);
    app.on_event(&Event::Pointer { touch: true }, t);
    app.on_event(&Event::MouseDown { x, y, button: MouseButton::Left, clicks: 1 }, t);
    assert_eq!(selected(&app).len(), 1, "the touch selects at once like a press");
    app.on_event(&Event::TouchCancel, t + Duration::from_millis(30));
    assert!(selected(&app).is_empty(), "a second finger takes the press back");
    app.on_event(&Event::Pinch(0.3), t + Duration::from_millis(40));
    assert_eq!(app.tab().scroll, 0.0);
    assert!(app.ops_idle() && app.marquee.is_none() && app.pressed == Hit::None);

    let end = app.touch_swipe((x, y + 200.0), (x, y), t + Duration::from_secs(1));
    let scrolled = app.tab().scroll;
    assert!(scrolled > 150.0);
    app.on_event(&Event::TouchCancel, end + Duration::from_millis(5));
    let mut g = Gfx::new(Mono, 1024);
    app.frame(&mut g, end + Duration::from_millis(16));
    app.frame(&mut g, end + Duration::from_millis(100));
    assert_eq!(app.tab().scroll, scrolled, "a second finger stops the glide");
}

#[test]
fn a_touch_pan_leaves_the_wheel_and_mouse_drags_exactly_as_they_were() {
    let (mut app, t0) = list(r"C:\x", 200, Caps::default());
    let (x, y) = spot(&app);
    let end = app.touch_swipe((x, y + 200.0), (x, y), t0 + Duration::from_secs(1));
    assert!(app.tab().scroll > 150.0, "the list pans");
    let t = end + Duration::from_secs(10);
    app.on_event(&Event::MouseMove { x, y }, t);
    let before = app.tab().scroll_target;
    app.on_event(&Event::Wheel { dx: 0.0, dy: -1.0 }, t);
    assert_eq!(app.tab().scroll_target - before, app.wheel_step(), "one notch still moves one wheel step");
    let bx = app.layout.cols[1].x + 20.0;
    app.on_event(&Event::Pointer { touch: false }, t);
    app.on_event(&Event::MouseDown { x: bx, y, button: MouseButton::Left, clicks: 1 }, t);
    let top = app.tab().scroll_target;
    app.on_event(&Event::MouseMove { x: bx, y: y + 150.0 }, t);
    app.on_event(&Event::MouseUp { x: bx, y: y + 150.0, button: MouseButton::Left }, t);
    assert_eq!(app.tab().scroll_target, top, "a mouse drag draws a band instead of panning");
    assert!(selected(&app).len() > 3);
}
