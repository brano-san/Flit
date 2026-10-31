use super::*;
use crate::input::{Event, Key, Mods, MouseButton};
use crate::sort::Folders;
use crate::ui::{harness, Effect, LayoutMode};
use std::time::{Duration, Instant};

fn bar(app: &App, b: BottomHit) -> Option<Rect> {
    app.layout.bar.iter().find(|x| x.0 == b).map(|x| x.1)
}

fn click(app: &mut App, b: BottomHit) {
    let r = bar(app, b).unwrap_or_else(|| panic!("{b:?} is on the bar"));
    let (x, y) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
    app.on_event(&Event::MouseMove { x, y }, Instant::now());
    app.on_event(&Event::MouseDown { x, y, button: MouseButton::Left, clicks: 1 }, Instant::now());
    app.on_event(&Event::MouseUp { x, y, button: MouseButton::Left }, Instant::now());
    harness::frame(app);
}

fn resort(app: &mut App) {
    let job = app.drain_effects().filter_map(|e| if let Effect::Resort { tab, gen, view, snap } = e { Some((tab, gen, view, snap)) } else { None }).next_back();
    if let Some((tab, gen, view, snap)) = job {
        let mut order = Vec::new();
        crate::sort::sort(&snap.listing, &snap.keys, view, &mut order);
        app.on_resorted(tab, gen, view, snap.listing.len(), order);
    }
    harness::frame(app);
}

#[test]
fn the_bottom_bar_holds_the_filter_counters_order_layout_and_zoom() {
    let app = harness::app(&["a.txt", "b.txt", "c.png"]);
    for b in [BottomHit::Subfolders, BottomHit::Retain, BottomHit::Folders, BottomHit::Files, BottomHit::Order, BottomHit::Layout, BottomHit::Zoom] {
        assert!(bar(&app, b).is_some_and(|r| app.layout.status.contains(r.x + 1.0, r.y + 1.0)), "{b:?}");
    }
    assert!(bar(&app, BottomHit::Elapsed).is_none(), "fast folders show no time");
    let order: Vec<f32> = [BottomHit::Folders, BottomHit::Files, BottomHit::Order, BottomHit::Layout, BottomHit::Zoom].iter().map(|&b| bar(&app, b).unwrap().x).collect();
    assert!(order.windows(2).all(|w| w[0] < w[1]), "counters, order, layout and zoom run left to right");
    assert!(app.nav.filter.field.right() < bar(&app, BottomHit::Subfolders).unwrap().x);
    assert_eq!(app.filter_placeholder(), "Filter 3 files...");
    assert_eq!(app.hit(app.nav.filter.field.x + 30.0, app.nav.filter.field.y + 4.0), Hit::FilterField);
}

#[test]
fn counters_hide_folders_and_files_and_show_what_is_left() {
    let mut app = harness::app(&["a.txt", "b.txt", "c.png"]);
    click(&mut app, BottomHit::Files);
    resort(&mut app);
    assert!(app.tab().view_spec.hide_files);
    assert!(app.tab().view.as_ref().unwrap().rows().is_empty());
    assert_eq!(app.bottom_tip(BottomHit::Files), S::TooltipFilesHidden);
    click(&mut app, BottomHit::Files);
    resort(&mut app);
    assert_eq!(app.tab().view.as_ref().unwrap().rows().len(), 3);
    assert_eq!(app.bottom_tip(BottomHit::Folders), S::TooltipFoldersShown);
}

#[test]
fn the_order_button_cycles_folders_first_last_and_mixed() {
    let mut app = harness::app(&["a.txt"]);
    let mut seen = Vec::new();
    for _ in 0..3 {
        click(&mut app, BottomHit::Order);
        seen.push(app.tab().view_spec.sort.folders);
    }
    assert_eq!(seen, [Folders::Last, Folders::Mixed, Folders::First]);
    assert_eq!(app.bottom_tip(BottomHit::Order), S::TooltipOrderFirst);
}

#[test]
fn the_zoom_block_scrolls_through_layouts_and_the_layout_button_opens_the_picker() {
    let mut app = harness::app(&["a.txt"]);
    let z = bar(&app, BottomHit::Zoom).unwrap();
    app.on_event(&Event::MouseMove { x: z.x + 5.0, y: z.y + 5.0 }, Instant::now());
    for _ in 0..3 {
        app.on_event(&Event::Wheel { dx: 0.0, dy: 1.0 }, Instant::now());
    }
    assert_eq!((app.tab().mode, app.tab().zoom), (LayoutMode::Grid, 30));
    harness::frame(&mut app);
    app.drain_effects().for_each(drop);
    click(&mut app, BottomHit::Layout);
    let menu = app.drain_effects().find_map(|e| if let Effect::ShellMenu(m) = e { Some(m.menu) } else { None }).expect("the picker opens");
    let labels: Vec<String> = menu.iter().filter_map(|m| if let crate::ui::MenuItem::Command { label, .. } = m { Some(label.clone()) } else { None }).collect();
    assert_eq!(&labels[..6], ["Details", "Tree", "List", "M Icons", "L Icons", "XL Icons"]);
}

#[test]
fn the_subfolders_and_retain_toggles_switch_and_the_slow_folders_show_their_time() {
    let mut app = harness::app(&["a.txt"]);
    app.drain_effects().for_each(drop);
    click(&mut app, BottomHit::Subfolders);
    assert!(app.tab().subfolders);
    assert!(app.drain_effects().any(|e| matches!(e, Effect::Provider { req: crate::provider::ProviderRequest::List { view, .. }, .. } if view.deep)));
    click(&mut app, BottomHit::Retain);
    assert!(app.prefs.retain_filter && app.tab().retain_filter);
    assert_eq!(app.bottom_tip(BottomHit::Retain), S::TooltipRetainFilter);
    app.tab_mut().stats.done = Some(Duration::from_millis(1500));
    harness::frame(&mut app);
    assert!(bar(&app, BottomHit::Elapsed).is_some());
    app.on_event(&Event::Key { key: Key::Char('F'), mods: Mods { ctrl: true, ..Mods::default() }, pressed: true }, Instant::now());
    assert_eq!(app.focus, crate::ui::Focus::Filter);
}

#[test]
fn every_bottom_bar_element_shows_a_tooltip() {
    let mut app = harness::app(&["a.txt", "b.txt", "c.png"]);
    let mut hits: Vec<Hit> = app.layout.bar.iter().map(|x| Hit::Bottom(x.0)).collect();
    hits.push(Hit::FilterField);
    assert!(hits.len() >= 8, "{hits:?}");
    let t0 = Instant::now();
    for h in hits {
        app.hover = h;
        app.tip_hover(h, t0);
        app.tip_timer(t0 + Duration::from_secs(2));
        assert!(app.tooltip().is_some(), "{h:?} has a tooltip");
        app.tip_hide();
    }
    app.hover = Hit::Bottom(BottomHit::Folders);
    app.tip_hover(app.hover, t0);
    app.tip_timer(t0 + Duration::from_secs(2));
    assert!(app.tooltip().is_some_and(|t| t.command.is_some()), "a counter names its command too");
}

#[test]
fn the_bar_is_two_pieces_with_the_list_between_and_the_filter_piece_widens_on_focus() {
    let mut app = harness::app(&["a.txt", "b.txt", "c.png"]);
    let [left, right] = app.layout.bar_pieces;
    let status = app.layout.status;
    assert_eq!(left.x, status.x);
    assert_eq!(right.right(), status.right());
    assert!(left.right() + 100.0 < right.x, "the list shows between {left:?} and {right:?}");
    assert!(left.right() >= bar(&app, BottomHit::Retain).unwrap().right());
    assert!(right.x <= bar(&app, BottomHit::Folders).unwrap().x);
    let f = app.nav.filter.field;
    app.on_event(&Event::MouseMove { x: f.x + 30.0, y: f.y + 4.0 }, Instant::now());
    app.on_event(&Event::MouseDown { x: f.x + 30.0, y: f.y + 4.0, button: MouseButton::Left, clicks: 1 }, Instant::now());
    app.on_event(&Event::MouseUp { x: f.x + 30.0, y: f.y + 4.0, button: MouseButton::Left }, Instant::now());
    harness::frame(&mut app);
    assert!(app.layout.bar_pieces[0].right() > left.right(), "focus widens the filter piece");
    assert!(app.nav.filter.close.w > 0.0, "and shows its close button");
}

#[test]
fn a_hidden_counter_is_marked_in_its_own_colour() {
    let mut app = harness::app(&["a.txt", "b.txt"]);
    click(&mut app, BottomHit::Files);
    resort(&mut app);
    let r = bar(&app, BottomHit::Files).unwrap();
    let mut g = crate::text::Gfx::new(crate::text::tests::Mono, 1024);
    app.frame(&mut g, Instant::now());
    let red = crate::theme::DARK.error.0;
    assert!(!g.dl.instances.iter().any(|i| i.color == red && r.contains(i.rect[0], i.rect[1])), "no red cross on the counter");
}
