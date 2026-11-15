use super::*;
use crate::input::{Event, Key, Mods};
use crate::listing::{DirSnapshot, Listing, Meta};
use crate::location::Location;
use crate::sort::{self, CaseMode, SortKeys};
use crate::text::tests::Mono;
use crate::ui::Persisted;
use std::sync::Arc;
use std::time::Duration;

fn app(size: (u32, u32)) -> App {
    let state = Persisted { path: Location::parse(r"C:\work").unwrap(), ..Default::default() };
    let mut app = App::new(state, Vec::new(), 0, false, 1.0, size, Instant::now());
    let t = app.tab();
    let mut listing = Listing::default();
    let mut keys = SortKeys::new(CaseMode::Insensitive);
    for (n, attrs) in [("docs", attr::DIR), ("a.rs", 0), ("b.png", 0), ("c.docx", 0)] {
        let i = listing.push(n.as_bytes(), Meta { attrs, size: 100, modified: 1, ..Default::default() });
        keys.push(listing.name(i));
    }
    let view = t.view_spec;
    let mut order = Vec::new();
    sort::sort(&listing, &keys, view, &mut order);
    let snap = DirSnapshot { gen: t.gen, path: t.location().path().clone(), listing, keys, order, view, done: true, error: None, elapsed: Duration::from_millis(1) };
    t.slot.publish(Arc::new(snap));
    app.poll();
    app.drain_effects().for_each(drop);
    app
}

fn frame(app: &mut App) {
    app.frame(&mut Gfx::new(Mono, 2048), Instant::now());
}

fn press(app: &mut App, key: Key, mods: Mods) {
    app.on_event(&Event::Key { key, mods, pressed: true }, Instant::now());
}

fn select(app: &mut App, name: &str) {
    let v = app.tab_mut().view.as_mut().unwrap();
    let i = v.find(name.as_bytes()).unwrap();
    v.clear_selection();
    v.set_selected(i, true);
    v.focus = Some(i);
}

fn requests(app: &mut App) -> Vec<PreviewRequest> {
    app.drain_effects().filter_map(|e| if let Effect::Preview(r) = e { Some(r) } else { None }).collect()
}

const NO: Mods = Mods { ctrl: false, shift: false, alt: false };

#[test]
fn toggles_and_reserves_the_right_or_bottom_edge() {
    let mut wide = app((1200, 700));
    let list_w = wide.layout.list.w;
    press(&mut wide, Key::Space, NO);
    assert!(wide.pane_state().visible);
    let pane = wide.preview.r.pane;
    assert_eq!(pane.right(), 1200.0);
    assert_eq!(pane.bottom(), wide.layout.status.bottom());
    assert_eq!(wide.layout.status.right(), pane.x);
    assert!(wide.layout.list.w < list_w);
    let mut tall = app((700, 1200));
    press(&mut tall, Key::Space, NO);
    let pane = tall.preview.r.pane;
    assert_eq!(pane.bottom(), 1200.0);
    assert!(tall.layout.status.bottom() <= pane.y && tall.layout.list.bottom() <= tall.layout.status.y);
    press(&mut tall, Key::Space, NO);
    assert!(!tall.pane_state().visible);
    assert_eq!(tall.preview.r.pane, Rect::default());
}

#[test]
fn selection_requests_previews_and_drops_stale_results() {
    let mut a = app((1200, 700));
    a.set_pane_state(PaneState { visible: true, ..Default::default() });
    select(&mut a, "a.rs");
    frame(&mut a);
    let first = requests(&mut a).pop().expect("request for a.rs");
    assert_eq!(first.path, FsPath::parse(r"C:\work\a.rs").unwrap());
    assert!(matches!(first.kind, Kind::Code(_)) && !first.details);
    frame(&mut a);
    assert!(requests(&mut a).is_empty());
    select(&mut a, "b.png");
    frame(&mut a);
    let second = requests(&mut a).pop().expect("request for b.png");
    assert_eq!(second.kind, Kind::Image);
    assert!(!first.cancel.is_current(first.gen) && second.cancel.is_current(second.gen));
    a.on_preview(first.gen, PreviewData::Doc(preview::text_doc("stale", None)));
    assert!(matches!(a.preview.content, Content::Loading));
    a.on_preview(second.gen, PreviewData::Image(Bitmap { width: 4, height: 2, rgba: vec![255; 32] }));
    let (rev, img) = a.preview_image();
    assert!(rev > 0 && img.is_some_and(|b| b.width == 4));
    frame(&mut a);
    select(&mut a, "docs");
    frame(&mut a);
    assert_eq!(requests(&mut a).pop().unwrap().kind, Kind::Folder);
    assert!(a.preview_image().1.is_none());
    let mut f = preview::FolderSummary::default();
    f.add(b"x.txt", false, false, 5, 1);
    a.on_preview(a.preview.gen, PreviewData::Folder(f.finish(false)));
    frame(&mut a);
}

#[test]
fn properties_card_details_and_many_items() {
    let mut a = app((1200, 700));
    press(&mut a, Key::Space, NO);
    select(&mut a, "c.docx");
    frame(&mut a);
    assert!(requests(&mut a).iter().all(|r| !r.details));
    press(&mut a, Key::Char('I'), Mods { ctrl: true, ..Mods::default() });
    assert!(a.preview.look.card && a.pane_state().details);
    frame(&mut a);
    let r = requests(&mut a).into_iter().find(|r| r.details).expect("properties request");
    a.on_preview(r.gen, PreviewData::Props(vec![preview::PropGroup { title: "Document".into(), rows: vec![("Pages".into(), "3".into())] }]));
    assert_eq!(a.preview.props.len(), 1);
    frame(&mut a);
    assert!(a.preview.card_extent > 0.0);
    a.preview_click(PaneHit::CardClose);
    assert!(!a.preview.look.card);
    press(&mut a, Key::Char('A'), Mods { ctrl: true, ..Mods::default() });
    frame(&mut a);
    assert_eq!(a.preview.subject.as_ref().unwrap().many, Some((4, 300)));
    assert_eq!(a.preview.subject.as_ref().unwrap().title, "1 Folder, 3 Files");
    assert!(a.preview.types.len() >= 3);
    a.preview_click(PaneHit::Title);
    frame(&mut a);
    let b = a.preview.r.button;
    a.on_event(&Event::MouseDown { x: b.x + 2.0, y: b.y + 2.0, button: crate::input::MouseButton::Left, clicks: 1 }, Instant::now());
    a.on_event(&Event::MouseUp { x: b.x + 2.0, y: b.y + 2.0, button: crate::input::MouseButton::Left }, Instant::now());
    assert_eq!(a.asking(), Some("properties"));
    assert!(a.drain_effects().all(|e| !matches!(e, Effect::Properties(_))));
}

#[test]
fn inspector_commands_change_the_look_and_layout() {
    let mut a = app((1200, 700));
    assert!(!a.available("inspector.wrap"));
    press(&mut a, Key::Space, NO);
    select(&mut a, "a.rs");
    frame(&mut a);
    let r = requests(&mut a).pop().unwrap();
    a.on_preview(r.gen, PreviewData::Doc(preview::text_doc("one
two  words
three", None)));
    frame(&mut a);
    for id in ["inspector.wrap", "inspector.line_numbers", "inspector.whitespace", "inspector.filter", "inspector.zoom_in", "inspector.layout"] {
        assert!(a.available(id), "{id}");
    }
    assert!(!a.available("inspector.grid") && !a.available("inspector.scale"));
    let (lines, spaces) = (a.preview.look.lines, a.preview.look.spaces);
    assert!(a.run_action("inspector.line_numbers", Instant::now()));
    assert!(a.run_action("inspector.whitespace", Instant::now()));
    assert_eq!((a.preview.look.lines, a.preview.look.spaces), (!lines, !spaces));
    let size = a.opts.inspector_size;
    assert!(a.run_action("inspector.zoom_in", Instant::now()));
    assert_eq!(a.opts.inspector_size, size + 1);
    assert!(a.run_action("inspector.zoom_out", Instant::now()));
    assert_eq!(a.opts.inspector_size, size);
    assert!(a.run_action("inspector.filter", Instant::now()));
    assert!(a.inspector_field_focused());
    a.on_event(&Event::Char('t'), Instant::now());
    a.on_event(&Event::Char('w'), Instant::now());
    frame(&mut a);
    assert_eq!(a.preview.doc.as_ref().map(|d| d.shown), Some(1));
    press(&mut a, Key::Escape, NO);
    assert!(!a.inspector_field_focused());
    assert!(a.inspector_right());
    assert!(a.run_action("inspector.layout", Instant::now()));
    assert_eq!(a.preview.look.at, InspectorAt::Below);
    frame(&mut a);
    assert!(!a.inspector_right() && a.preview.r.pane.y > a.layout.status.y);
    select(&mut a, "b.png");
    frame(&mut a);
    let r = requests(&mut a).pop().unwrap();
    a.on_preview(r.gen, PreviewData::Image(Bitmap { width: 40, height: 20, rgba: vec![255; 3200] }));
    frame(&mut a);
    assert!(a.available("inspector.grid") && a.available("inspector.scale") && !a.available("inspector.wrap"));
    assert_eq!(a.preview.look.scale, Scale::Fit);
    assert!(a.run_action("inspector.scale", Instant::now()));
    assert_eq!(a.preview.look.scale, Scale::Original);
    assert!(a.run_action("inspector.zoom_in", Instant::now()));
    assert_eq!(a.preview.look.scale, Scale::Custom);
    assert!(a.preview.look.zoom > 1.0);
    let grid = a.preview.look.grid;
    assert!(a.run_action("inspector.grid", Instant::now()));
    assert_eq!(a.preview.look.grid, !grid);
    assert_eq!(image_factor(Scale::Original, 3.0, (40.0, 20.0), (10.0, 10.0)), 1.0);
    assert_eq!(image_factor(Scale::Fit, 3.0, (40.0, 20.0), (20.0, 20.0)), 0.5);
    assert_eq!(image_factor(Scale::Fit, 3.0, (4.0, 2.0), (20.0, 20.0)), 1.0);
    assert_eq!(image_factor(Scale::Custom, 3.0, (4.0, 2.0), (20.0, 20.0)), 3.0);
    press(&mut a, Key::Char('W'), Mods { ctrl: true, ..Mods::default() });
}

#[test]
fn hosted_previews_follow_the_subject() {
    let mut a = app((1200, 700));
    a.set_pane_state(PaneState { visible: true, ..Default::default() });
    select(&mut a, "c.docx");
    frame(&mut a);
    let r = requests(&mut a).pop().unwrap();
    a.on_preview(r.gen, PreviewData::Handler("{84F66100-FF7C-4fb4-B0C0-02CD7FB668FE}".into()));
    let show = a.drain_effects().find_map(|e| if let Effect::Host(HostCmd::Show { gen, what, .. }) = e { Some((gen, what)) } else { None });
    assert_eq!(show, Some((r.gen, Hosted::Handler("{84F66100-FF7C-4fb4-B0C0-02CD7FB668FE}".into()))));
    frame(&mut a);
    let placed = a.drain_effects().find_map(|e| if let Effect::Host(HostCmd::Place(p)) = e { Some(p) } else { None });
    assert_eq!(placed, Some(Some(a.preview.r.body)));
    a.open_palette();
    frame(&mut a);
    assert!(a.drain_effects().any(|e| matches!(e, Effect::Host(HostCmd::Place(None)))));
    a.close_palette();
    a.on_event(&Event::Hosted(HostEvent::Failed { gen: r.gen, message: "no handler".into() }), Instant::now());
    assert!(matches!(&a.preview.content, Content::Basic(Some(preview::Note::Message(m))) if m == "no handler"));
    select(&mut a, "a.rs");
    frame(&mut a);
    assert!(!a.drain_effects().any(|e| matches!(e, Effect::Host(HostCmd::Close))));
}

#[test]
fn media_controls_send_commands() {
    let mut a = app((1200, 700));
    a.set_pane_state(PaneState { visible: true, volume: 0.5, ..Default::default() });
    select(&mut a, "b.png");
    frame(&mut a);
    let r = requests(&mut a).pop().unwrap();
    a.on_preview(r.gen, PreviewData::Media { video: true, poster: None });
    a.drain_effects().for_each(drop);
    a.on_event(&Event::Hosted(HostEvent::Media { gen: r.gen, position: 5.0, duration: 10.0, playing: false }), Instant::now());
    frame(&mut a);
    a.preview_click(PaneHit::Play);
    assert!(a.drain_effects().any(|e| matches!(e, Effect::Host(HostCmd::Media(MediaCmd::Play)))));
    frame(&mut a);
    assert!(a.drain_effects().any(|e| matches!(e, Effect::Host(HostCmd::Place(Some(_))))));
    let v = a.preview.r.volume;
    a.preview_mouse_down(PaneHit::Volume, (v.right(), v.y), MouseButton::Left, 1, Instant::now());
    assert_eq!(a.pane_state().volume, 1.0);
    select(&mut a, "a.rs");
    frame(&mut a);
    assert!(a.drain_effects().any(|e| matches!(e, Effect::Host(HostCmd::Close))));
}

#[test]
fn quick_view_opens_the_selection_or_peeks_with_the_pane() {
    let mut a = app((1200, 700));
    select(&mut a, "a.rs");
    a.on_event(&Event::Char(' '), Instant::now());
    assert!(!a.drain_effects().any(|e| matches!(e, Effect::QuickView(_))), "Space toggles the Inspector, not Quick View");
    assert!(a.run_action("item.quick_view", Instant::now()));
    let path = a.drain_effects().find_map(|e| if let Effect::QuickView(p) = e { Some(p) } else { None });
    assert_eq!(path, Some(FsPath::parse(r"C:\work\a.rs").unwrap()));
    a.on_quick_view(true);
    assert!(!a.pane_state().visible);
    a.on_quick_view(false);
    assert!(a.pane_state().visible && !a.pane_state().details);
    a.on_quick_view(false);
    assert!(!a.pane_state().visible);
    a.on_event(&Event::Char('b'), Instant::now());
    a.on_event(&Event::Char(' '), Instant::now());
    assert!(!a.drain_effects().any(|e| matches!(e, Effect::QuickView(_))));
}

#[test]
fn state_is_sanitized() {
    let s = PaneState { visible: true, details: true, right_w: f32::NAN, bottom_h: 5.0, volume: 3.0 }.sanitized();
    assert_eq!(s, PaneState { visible: true, details: true, ..Default::default() });
}

#[test]
fn documents_wrap_to_the_pane_width() {
    let mut g = Gfx::new(Mono, 1024);
    let mut widths = FxHashMap::default();
    let d = preview::text_doc("aaaa bbbb cccc\nverylongwordthatbreaks", None);
    let lay = layout_doc(&mut g, &d, 95.0, (1.0, 12.5), &mut widths, true, "");
    let lines: Vec<String> = lay.lines.iter().map(|l| lay.pieces[l.pieces.start as usize..l.pieces.end as usize].iter().map(|p| &d.text[p.start as usize..p.end as usize]).collect()).collect();
    assert_eq!(lines, ["aaaa bbbb ", "cccc", "verylongw", "ordthatbr", "eaks"]);
    let code = preview::text_doc("fn main() { let x = 1; }", Some(preview::syntax::by_ext("rs").unwrap()));
    assert_eq!(layout_doc(&mut g, &code, 50.0, (1.0, 12.5), &mut widths, false, "").lines.len(), 1);
    let lay = layout_doc(&mut g, &d, 95.0, (1.0, 12.5), &mut widths, false, "long");
    assert_eq!((lay.shown, lay.lines.len(), lay.lines[0].num), (1, 1, 2));
    let md = preview::markdown_doc("# Title\n\nbody text here");
    let lay = layout_doc(&mut g, &md, 400.0, (1.0, 12.5), &mut widths, true, "");
    assert_eq!(lay.lines.len(), 2);
    assert!(lay.lines[0].h > 0.0 && lay.height > lay.lines[1].y);
}

#[test]
fn details_tab_shows_and_edits_the_selection_tags() {
    use crate::tags::{TagRequest, TagStore, Tags, COLORS};
    let mut a = app((1200, 700));
    let mut store = TagStore { tags: Vec::new(), files: Default::default() };
    store.add("Work", COLORS[0].1).unwrap();
    store.add("Old", COLORS[1].1).unwrap();
    a.set_tags(Arc::new(Tags::new(1, &store, &Default::default())));
    a.set_pane_state(PaneState { visible: true, details: true, ..Default::default() });
    select(&mut a, "a.rs");
    frame(&mut a);
    let chips: Vec<PaneHit> = a.preview.chips.iter().map(|c| c.1).collect();
    assert_eq!(chips, [PaneHit::Tag(0), PaneHit::Tag(1), PaneHit::NewTag]);
    a.drain_effects().for_each(drop);
    let click = |a: &mut App, k: usize| {
        let r = a.preview.chips[k].0;
        a.on_event(&Event::MouseDown { x: r.x + 4.0, y: r.y + 4.0, button: crate::input::MouseButton::Left, clicks: 1 }, Instant::now());
        a.on_event(&Event::MouseUp { x: r.x + 4.0, y: r.y + 4.0, button: crate::input::MouseButton::Left }, Instant::now());
    };
    click(&mut a, 1);
    let set: Vec<TagRequest> = a.drain_effects().filter_map(|e| if let Effect::Tags(r) = e { Some(r) } else { None }).collect();
    assert_eq!(set, [TagRequest::Set { items: vec![FsPath::parse(r"C:\work\a.rs").unwrap()], tag: 1, on: true }]);
    click(&mut a, 2);
    assert!(a.nav.palette.is_some());
    a.nav.palette = None;
    a.tab_mut().view.as_mut().unwrap().clear_selection();
    frame(&mut a);
    assert!(a.preview.chips.is_empty());
}

#[test]
fn volumes_of_unknown_size_show_no_size_instead_of_zero_bytes() {
    let mut app = App::new(Persisted { path: Location::computer(), ..Default::default() }, Vec::new(), 0, false, 1.0, (1000, 600), Instant::now());
    let share = crate::ui::Drive { path: crate::path::FsPath::parse(r"Z:\").unwrap(), label: "Share (Z:)".into(), total: 0, free: 0, file_system: String::new(), removable: false, unmounted: false, kind: None, remote: Some(r"\\srv\s".into()) };
    let fixed = crate::ui::Drive { path: crate::path::FsPath::parse(r"C:\").unwrap(), label: "C".into(), total: 1000, free: 400, file_system: String::new(), removable: false, unmounted: false, kind: None, remote: None };
    app.set_places(Arc::new(crate::ui::Places { drives: vec![fixed, share], ..Default::default() }));
    let t = app.tab();
    let mut listing = Listing::default();
    let mut keys = SortKeys::new(CaseMode::Insensitive);
    for (n, attrs, size) in [("C:", attr::DIR | attr::VOLUME | attr::SIZE_KNOWN, 1000), ("Ubuntu-22.04", attr::DIR | attr::VOLUME, 0), ("Z:", attr::DIR | attr::VOLUME, 0)] {
        let i = listing.push(n.as_bytes(), Meta { attrs, size, ..Default::default() });
        keys.push(listing.name(i));
    }
    let view = t.view_spec;
    let mut order = Vec::new();
    sort::sort(&listing, &keys, view, &mut order);
    t.slot.publish(Arc::new(DirSnapshot { gen: t.gen, path: t.location().path().clone(), listing, keys, order, view, done: true, error: None, elapsed: Duration::from_millis(1) }));
    app.poll();
    let arena = bumpalo::Bump::new();
    let v = app.tab().view.as_ref().unwrap();
    let size = |name: &str| {
        let i = (0..3).find(|&i| v.listing().name(i) == name.as_bytes()).unwrap();
        app.cell_text(&arena, v.listing(), i, crate::ui::Column::Size).to_owned()
    };
    assert_eq!(size("Ubuntu-22.04"), "", "a WSL distro has no known size");
    assert_eq!(size("Z:"), "");
    assert!(!size("C:").is_empty());
    for (name, want) in [("C:", Some((1000, 400))), ("Z:", None), ("Ubuntu-22.04", None)] {
        app.tab_mut().view.as_mut().unwrap().clear_selection();
        select(&mut app, name);
        assert_eq!(app.subject().unwrap().drive, want, "{name}");
    }
}

#[test]
fn the_inspector_title_chevron_turns_with_the_card() {
    let mut a = app((1200, 700));
    press(&mut a, Key::Space, NO);
    select(&mut a, "c.docx");
    frame(&mut a);
    let turn = super::super::address::TURN;
    assert_eq!(a.preview.turn.value(Instant::now(), turn), 0.0);
    press(&mut a, Key::Char('I'), Mods { ctrl: true, ..Mods::default() });
    let t = Instant::now();
    a.frame(&mut Gfx::new(Mono, 2048), t);
    let mid = a.preview.turn.value(t + Duration::from_millis(50), turn);
    assert!(mid > 0.0 && mid < 90.0, "{mid}");
    a.frame(&mut Gfx::new(Mono, 2048), t + Duration::from_millis(400));
    assert_eq!(a.preview.turn.value(t + Duration::from_millis(400), turn), 90.0);
    a.animations = false;
    a.preview_click(PaneHit::CardClose);
    a.frame(&mut Gfx::new(Mono, 2048), t + Duration::from_millis(410));
    assert_eq!(a.preview.turn.value(t + Duration::from_millis(410), turn), 0.0, "without Animations it turns at once");
}

#[test]
fn the_inspector_opens_at_half_of_the_area_it_shares_until_the_divider_is_dragged() {
    let mut a = app((1200, 700));
    press(&mut a, Key::Space, NO);
    let half = |a: &App| {
        let (p, l) = (a.preview.r.pane, &a.layout);
        (p.w - (p.right() - l.header.x) / 2.0).abs() <= 1.0
    };
    assert!(half(&a), "{:?} {:?}", a.preview.r.pane, a.layout.header);
    a.on_event(&Event::Resized { width: 1500, height: 700 }, Instant::now());
    frame(&mut a);
    assert!(half(&a), "half of the new width after a resize");
    let split = a.preview.r.split;
    let (x, y) = (split.x + split.w / 2.0, split.y + 40.0);
    a.on_event(&Event::MouseMove { x, y }, Instant::now());
    a.on_event(&Event::MouseDown { x, y, button: crate::input::MouseButton::Left, clicks: 1 }, Instant::now());
    a.on_event(&Event::MouseMove { x: x - 100.0, y }, Instant::now());
    a.on_event(&Event::MouseUp { x: x - 100.0, y, button: crate::input::MouseButton::Left }, Instant::now());
    let dragged = a.pane_state().right_w;
    assert!(dragged != HALF && (dragged - a.preview.r.pane.w).abs() <= 1.0, "{dragged}");
    a.on_event(&Event::Resized { width: 1700, height: 700 }, Instant::now());
    frame(&mut a);
    assert_eq!(a.preview.r.pane.w, dragged, "a dragged size stays");
    let old = PaneState { right_w: 340.0, bottom_h: 260.0, ..Default::default() }.sanitized();
    assert_eq!((old.right_w, old.bottom_h), (HALF, HALF), "the old untouched defaults become half");
    let kept = PaneState { right_w: 410.0, bottom_h: 300.0, ..Default::default() }.sanitized();
    assert_eq!((kept.right_w, kept.bottom_h), (410.0, 300.0));
}

#[test]
fn an_inspector_below_resizes_from_its_own_edge_and_not_from_the_status_bar() {
    let mut a = app((1200, 700));
    press(&mut a, Key::Space, NO);
    a.preview.look.at = InspectorAt::Below;
    a.layout();
    frame(&mut a);
    let (pane, status, split) = (a.preview.r.pane, a.layout.status, a.preview.r.split);
    assert!(split.y < pane.y && split.bottom() > pane.y, "the grip straddles the inspector's top edge: {split:?} {pane:?}");
    assert!(split.y >= status.y + status.h / 2.0, "the grip is not on the status bar's top edge: {split:?} {status:?}");
    let x = pane.x + pane.w / 2.0;
    assert_ne!(a.hit(x, status.y + 1.0), Hit::Preview(PaneHit::Split), "the top of the status bar does not resize the inspector");
    let y = pane.y;
    assert_eq!(a.hit(x, y), Hit::Preview(PaneHit::Split));
    let before = pane.h;
    a.on_event(&Event::MouseMove { x, y }, Instant::now());
    a.on_event(&Event::MouseDown { x, y, button: crate::input::MouseButton::Left, clicks: 1 }, Instant::now());
    a.on_event(&Event::MouseMove { x, y: y - 60.0 }, Instant::now());
    a.on_event(&Event::MouseUp { x, y: y - 60.0, button: crate::input::MouseButton::Left }, Instant::now());
    assert!((a.preview.r.pane.h - before - 60.0).abs() <= 1.0, "{} -> {}", before, a.preview.r.pane.h);
}

#[test]
fn online_only_files_show_basic_info_until_the_download_button_is_clicked() {
    use crate::listing::{ChangeKind, DirChange};
    let mut a = app((1200, 700));
    a.set_pane_state(PaneState { visible: true, ..Default::default() });
    let change = |attrs| DirChange { kind: ChangeKind::Modified, name: b"a.rs".to_vec(), meta: Some(Meta { attrs, size: 100, modified: 1, ..Default::default() }) };
    a.tab_mut().view.as_mut().unwrap().apply(&change(attr::ONLINE_ONLY));
    select(&mut a, "a.rs");
    frame(&mut a);
    frame(&mut a);
    assert!(requests(&mut a).is_empty(), "selecting an online-only file reads nothing");
    assert!(matches!(a.preview.content, Content::Basic(Some(Note::OnlineOnly))));
    let r = a.preview_rect(PaneHit::Download).expect("a download button");
    let (x, y) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
    assert!(matches!(a.preview_hit(x, y), Some(Hit::Preview(PaneHit::Download))));
    a.on_event(&Event::MouseMove { x, y }, Instant::now());
    a.on_event(&Event::MouseDown { x, y, button: crate::input::MouseButton::Left, clicks: 1 }, Instant::now());
    a.on_event(&Event::MouseUp { x, y, button: crate::input::MouseButton::Left }, Instant::now());
    frame(&mut a);
    let asked = requests(&mut a).pop().expect("the click previews the file");
    assert_eq!((asked.path, asked.details), (FsPath::parse(r"C:\work\a.rs").unwrap(), false));
    assert!(matches!(a.preview.content, Content::Loading));
    select(&mut a, "b.png");
    frame(&mut a);
    assert_eq!(requests(&mut a).len(), 1);
    a.tab_mut().view.as_mut().unwrap().apply(&change(attr::PINNED));
    select(&mut a, "a.rs");
    frame(&mut a);
    assert_eq!(requests(&mut a).len(), 1, "a file kept on the device previews at once");
}

#[test]
fn only_files_away_from_the_device_wait_for_a_download() {
    assert!(preview::waits_for_download(attr::ONLINE_ONLY));
    assert!(preview::waits_for_download(attr::OFFLINE | attr::READONLY));
    assert!(!preview::waits_for_download(0));
    assert!(!preview::waits_for_download(attr::PINNED));
    assert!(!preview::waits_for_download(attr::DIR | attr::ONLINE_ONLY), "a folder preview lists names only");
    assert!(!preview::waits_for_download(attr::VOLUME | attr::OFFLINE));
}

#[test]
fn the_wheel_scrolls_unwrapped_text_and_large_images_sideways() {
    let mut a = app((1200, 700));
    press(&mut a, Key::Space, NO);
    select(&mut a, "a.rs");
    frame(&mut a);
    let r = requests(&mut a).pop().unwrap();
    a.on_preview(r.gen, PreviewData::Doc(preview::text_doc(&"word ".repeat(400), None)));
    a.preview.look.wrap = Some(false);
    frame(&mut a);
    assert!(a.preview.hmax > 0.0);
    let b = a.preview.r.body;
    a.on_event(&Event::MouseMove { x: b.x + 20.0, y: b.y + 10.0 }, Instant::now());
    assert_eq!(a.hover, Hit::Preview(PaneHit::Body));
    a.on_event(&Event::Wheel { dx: 1.0, dy: 0.0 }, Instant::now());
    let right = a.preview.hscroll;
    assert!(right > 0.0 && a.preview.scroll == 0.0);
    let shift = Mods { shift: true, ..NO };
    a.on_event(&Event::Key { key: Key::Other, mods: shift, pressed: false }, Instant::now());
    a.on_event(&Event::Wheel { dx: 0.0, dy: -1.0 }, Instant::now());
    assert!(a.preview.hscroll > right, "Shift+wheel scrolls sideways");
    let size = a.opts.inspector_size;
    let ctrl = Mods { ctrl: true, ..NO };
    a.on_event(&Event::Key { key: Key::Other, mods: ctrl, pressed: false }, Instant::now());
    a.on_event(&Event::Wheel { dx: -1.0, dy: 0.0 }, Instant::now());
    assert_eq!(a.opts.inspector_size, size, "a sideways Ctrl+wheel does not resize the text");
    a.on_event(&Event::Key { key: Key::Other, mods: NO, pressed: false }, Instant::now());
    a.preview.look.wrap = Some(true);
    frame(&mut a);
    assert_eq!((a.preview.hmax, a.preview.hscroll), (0.0, 0.0));
    select(&mut a, "b.png");
    frame(&mut a);
    let r = requests(&mut a).pop().unwrap();
    a.on_preview(r.gen, PreviewData::Image(Bitmap { width: 4000, height: 20, rgba: vec![255; 320_000] }));
    a.preview.look.scale = Scale::Original;
    frame(&mut a);
    assert!(a.preview.hmax > 0.0);
    a.on_event(&Event::Wheel { dx: 2.0, dy: 0.0 }, Instant::now());
    assert!(a.preview.hscroll > 0.0);
}

const CTRL: Mods = Mods { ctrl: true, shift: false, alt: false };

fn text_preview(size: (u32, u32), text: &str) -> App {
    let mut a = app(size);
    press(&mut a, Key::Space, NO);
    select(&mut a, "a.rs");
    frame(&mut a);
    let r = requests(&mut a).pop().unwrap();
    a.on_preview(r.gen, PreviewData::Doc(preview::text_doc(text, None)));
    frame(&mut a);
    a.drain_effects().for_each(drop);
    a
}

fn spot_xy(a: &App, line: usize, col: usize) -> (f32, f32) {
    let b = a.preview.r.body;
    let l = &a.preview.doc.as_ref().unwrap().lines[line];
    (b.x + a.preview.r.gutter + col as f32 * 10.0 + 2.0 - a.preview.hscroll, b.y + l.y + l.h / 2.0 - a.preview.scroll)
}

fn mouse(a: &mut App, (x, y): (f32, f32), button: MouseButton, clicks: u8) {
    a.on_event(&Event::MouseDown { x, y, button, clicks }, Instant::now());
    a.on_event(&Event::MouseUp { x, y, button }, Instant::now());
}

fn drag(a: &mut App, from: (f32, f32), to: (f32, f32)) {
    a.on_event(&Event::MouseDown { x: from.0, y: from.1, button: MouseButton::Left, clicks: 1 }, Instant::now());
    a.on_event(&Event::MouseMove { x: to.0, y: to.1 }, Instant::now());
    a.on_event(&Event::MouseUp { x: to.0, y: to.1, button: MouseButton::Left }, Instant::now());
}

fn drag_spots(a: &mut App, from: (usize, usize), to: (usize, usize)) {
    let (f, t) = (spot_xy(a, from.0, from.1), spot_xy(a, to.0, to.1));
    drag(a, f, t);
}

fn click_spot(a: &mut App, at: (usize, usize), button: MouseButton, clicks: u8) {
    let xy = spot_xy(a, at.0, at.1);
    mouse(a, xy, button, clicks);
}

fn copied(a: &mut App, key: Key, mods: Mods) -> (Vec<String>, bool) {
    a.drain_effects().for_each(drop);
    press(a, key, mods);
    let fx: Vec<Effect> = a.drain_effects().collect();
    let text = fx.iter().filter_map(|e| if let Effect::SetClipboard(s) = e { Some(s.clone()) } else { None }).collect();
    (text, fx.iter().any(|e| matches!(e, Effect::SetClipboardFiles { .. })))
}

fn shown_menu(a: &mut App) -> Vec<(String, bool)> {
    let m = a.drain_effects().find_map(|e| if let Effect::ShellMenu(m) = e { Some(m.menu) } else { None }).expect("a menu");
    m.into_iter().filter_map(|i| if let crate::ui::MenuItem::Command { label, disabled, .. } = i { Some((label, disabled)) } else { None }).collect()
}

#[test]
fn dragging_across_the_text_preview_selects_text_that_ctrl_c_copies_instead_of_the_files() {
    let mut a = text_preview((1200, 700), "alpha beta\ngamma delta");
    let lb = if a.caps.unix_paths { "\n" } else { "\r\n" };
    drag_spots(&mut a, (0, 6), (1, 5));
    assert_eq!(a.focus, Focus::Preview);
    frame(&mut a);
    assert_eq!(copied(&mut a, Key::Char('C'), CTRL), (vec![format!("beta{lb}gamma")], false));
    assert_eq!(copied(&mut a, Key::Insert, CTRL), (vec![format!("beta{lb}gamma")], false));
    drag_spots(&mut a, (1, 5), (0, 6));
    assert_eq!(copied(&mut a, Key::Char('C'), CTRL).0, [format!("beta{lb}gamma")], "dragging backwards selects the same text");
}

#[test]
fn ctrl_c_in_the_file_list_still_copies_files_while_the_preview_holds_a_selection() {
    let mut a = text_preview((1200, 700), "alpha beta\ngamma delta");
    drag_spots(&mut a, (0, 0), (0, 5));
    a.focus = Focus::List;
    assert_eq!(copied(&mut a, Key::Char('C'), CTRL), (Vec::new(), true));
    press(&mut a, Key::Char('A'), CTRL);
    assert!(a.tab().view.as_ref().unwrap().selected_count() > 1, "Ctrl+A in the list selects every file");
    let mut fresh = text_preview((1200, 700), "alpha");
    assert_eq!(copied(&mut fresh, Key::Char('C'), CTRL), (Vec::new(), true), "nothing clicked in the preview, so the list keeps Ctrl+C");
}

#[test]
fn double_click_takes_a_word_triple_click_the_line_shift_click_extends_and_ctrl_a_takes_all() {
    let mut a = text_preview((1200, 700), "let snake_case = 42;\nnext line\n\nlast");
    let lb = if a.caps.unix_paths { "\n" } else { "\r\n" };
    let at = spot_xy(&a, 0, 7);
    mouse(&mut a, at, MouseButton::Left, 1);
    mouse(&mut a, at, MouseButton::Left, 2);
    assert_eq!(copied(&mut a, Key::Char('C'), CTRL).0, ["snake_case"]);
    mouse(&mut a, at, MouseButton::Left, 1);
    assert_eq!(copied(&mut a, Key::Char('C'), CTRL).0, ["let snake_case = 42;"]);
    click_spot(&mut a, (1, 0), MouseButton::Left, 1);
    a.mods.shift = true;
    click_spot(&mut a, (3, 2), MouseButton::Left, 1);
    a.mods.shift = false;
    assert_eq!(copied(&mut a, Key::Char('C'), CTRL).0, [format!("next line{lb}{lb}la")]);
    let (text, files) = copied(&mut a, Key::Char('A'), CTRL);
    assert!(text.is_empty() && !files);
    assert_eq!(copied(&mut a, Key::Char('C'), CTRL).0, [format!("let snake_case = 42;{lb}next line{lb}{lb}last")]);
    assert_eq!(a.tab().view.as_ref().unwrap().selected_count(), 1, "Ctrl+A in the preview leaves the file selection alone");
}

#[test]
fn the_preview_context_menu_copies_and_selects_all_text() {
    let mut a = text_preview((1200, 700), "one two");
    click_spot(&mut a, (0, 1), MouseButton::Right, 1);
    assert_eq!(a.focus, Focus::Preview);
    assert_eq!(shown_menu(&mut a), [(S::MenuCopy.t().to_owned(), true), (S::MenuFieldSelectAll.t().to_owned(), false)]);
    a.on_event(&Event::MenuCommand(1), Instant::now());
    a.on_event(&Event::MenuCommand(0), Instant::now());
    let fx: Vec<Effect> = a.drain_effects().collect();
    assert!(fx.iter().any(|e| matches!(e, Effect::SetClipboard(s) if s == "one two")));
    assert!(!fx.iter().any(|e| matches!(e, Effect::SetClipboardFiles { .. })));
    click_spot(&mut a, (0, 1), MouseButton::Right, 1);
    assert_eq!(shown_menu(&mut a)[0], (S::MenuCopy.t().to_owned(), false), "a right click keeps the selection");
}

#[test]
fn selection_follows_the_sideways_scroll_of_unwrapped_text() {
    let line = (0..300).map(|k| char::from(b'a' + (k % 26) as u8)).collect::<String>();
    let mut a = text_preview((1200, 700), &line);
    a.preview.look.wrap = Some(false);
    frame(&mut a);
    assert!(a.preview.hmax > 500.0);
    a.preview.hscroll = 500.0;
    frame(&mut a);
    let b = a.preview.r.body;
    let x0 = b.x + a.preview.r.gutter;
    let y = spot_xy(&a, 0, 0).1;
    drag(&mut a, (x0 + 2.0, y), (x0 + 32.0, y));
    assert_eq!(copied(&mut a, Key::Char('C'), CTRL).0, [line[50..53].to_owned()]);
    drag(&mut a, (x0 + 2.0, y), (b.right() + 400.0, y));
    assert!(a.preview.hscroll > 500.0, "dragging past the edge scrolls sideways");
    a.preview.look.wrap = Some(true);
    frame(&mut a);
    assert_eq!(a.preview.hmax, 0.0);
}

#[test]
fn a_long_text_copies_only_on_request_and_empty_or_binary_previews_copy_nothing() {
    let long = "x".repeat(preview::TEXT_CHARS + 50);
    let mut a = text_preview((1200, 700), &long);
    a.preview.look.wrap = Some(false);
    frame(&mut a);
    click_spot(&mut a, (0, 0), MouseButton::Left, 1);
    press(&mut a, Key::Char('A'), CTRL);
    for _ in 0..3 {
        frame(&mut a);
    }
    assert!(!a.drain_effects().any(|e| matches!(e, Effect::SetClipboard(_))), "frames never touch the clipboard");
    assert_eq!(copied(&mut a, Key::Char('C'), CTRL).0.iter().map(String::len).collect::<Vec<_>>(), [preview::TEXT_CHARS]);
    let mut empty = text_preview((1200, 700), "");
    click_spot(&mut empty, (0, 3), MouseButton::Left, 2);
    press(&mut empty, Key::Char('A'), CTRL);
    assert_eq!(copied(&mut empty, Key::Char('C'), CTRL), (Vec::new(), false));
    select(&mut empty, "c.docx");
    frame(&mut empty);
    let r = requests(&mut empty).pop().unwrap();
    empty.on_preview(r.gen, PreviewData::Basic { poster: None, note: None });
    frame(&mut empty);
    assert_eq!(empty.focus, Focus::Preview);
    assert_eq!(copied(&mut empty, Key::Char('C'), CTRL), (Vec::new(), false), "Ctrl+C in a focused preview never copies the files");
}

#[test]
fn a_narrow_pane_still_selects_and_a_new_subject_drops_the_selection() {
    let mut a = text_preview((420, 260), "alpha beta gamma delta epsilon");
    drag_spots(&mut a, (0, 0), (0, 2));
    assert_eq!(copied(&mut a, Key::Char('C'), CTRL).0, ["al"]);
    select(&mut a, "c.docx");
    frame(&mut a);
    let r = requests(&mut a).pop().unwrap();
    a.on_preview(r.gen, PreviewData::Doc(preview::text_doc("other", None)));
    frame(&mut a);
    assert_eq!(copied(&mut a, Key::Char('C'), CTRL), (Vec::new(), false));
}

#[test]
fn images_copy_as_images_from_ctrl_c_and_the_context_menu() {
    let mut a = app((1200, 700));
    press(&mut a, Key::Space, NO);
    select(&mut a, "b.png");
    frame(&mut a);
    let r = requests(&mut a).pop().unwrap();
    let image = Bitmap { width: 2, height: 1, rgba: vec![255, 0, 0, 255, 0, 0, 0, 0] };
    a.on_preview(r.gen, PreviewData::Image(image.clone()));
    frame(&mut a);
    a.drain_effects().for_each(drop);
    let images = |a: &mut App| a.drain_effects().filter_map(|e| if let Effect::SetClipboardImage(b) = e { Some((b.width, b.height, b.rgba)) } else { None }).collect::<Vec<_>>();
    press(&mut a, Key::Char('C'), CTRL);
    assert!(images(&mut a).is_empty(), "the list keeps Ctrl+C until the image is clicked");
    let b = a.preview.r.body;
    mouse(&mut a, (b.x + b.w / 2.0, b.y + b.h / 2.0), MouseButton::Right, 1);
    assert_eq!(shown_menu(&mut a), [(S::PreviewCopyImage.t().to_owned(), false)]);
    a.on_event(&Event::MenuCommand(0), Instant::now());
    assert_eq!(images(&mut a), [(2, 1, image.rgba.clone())]);
    let (text, files) = copied(&mut a, Key::Char('C'), CTRL);
    assert!(text.is_empty() && !files);
    press(&mut a, Key::Char('C'), CTRL);
    assert_eq!(images(&mut a).len(), 1);
}

#[test]
fn a_shrunk_preview_image_is_copied_at_full_size() {
    let mut a = app((1200, 700));
    press(&mut a, Key::Space, NO);
    select(&mut a, "b.png");
    frame(&mut a);
    let r = requests(&mut a).pop().unwrap();
    let side = a.preview.image_px;
    let shrunk = Bitmap { width: side, height: 1, rgba: vec![9; side as usize * 4] };
    a.on_preview(r.gen, PreviewData::Image(shrunk.clone()));
    frame(&mut a);
    a.drain_effects().for_each(drop);
    let b = a.preview.r.body;
    mouse(&mut a, (b.x + b.w / 2.0, b.y + b.h / 2.0), MouseButton::Left, 1);
    a.drain_effects().for_each(drop);
    press(&mut a, Key::Char('C'), CTRL);
    let effects: Vec<Effect> = a.drain_effects().collect();
    assert!(!effects.iter().any(|e| matches!(e, Effect::SetClipboardImage(_))), "the shrunk copy is not what lands on the clipboard");
    let full = effects.iter().find_map(|e| if let Effect::Preview(q) = e { Some(q.clone()) } else { None }).expect("the file is decoded again at full size");
    assert_eq!((full.px, full.path.clone(), full.details), (crate::preview::COPY_PX, r.path.clone(), false));
    assert!(full.cancel.is_current(full.gen));
    let big = Bitmap { width: side * 3, height: 2, rgba: vec![7; side as usize * 24] };
    a.on_preview(full.gen, PreviewData::Image(big.clone()));
    let copied: Vec<(u32, u32)> = a.drain_effects().filter_map(|e| if let Effect::SetClipboardImage(b) = e { Some((b.width, b.height)) } else { None }).collect();
    assert_eq!(copied, [(side * 3, 2)]);
    assert_eq!(a.preview.image.as_ref().map(|b| b.width), Some(side), "the pane keeps showing its own copy");
    press(&mut a, Key::Char('C'), CTRL);
    let again = requests(&mut a).pop().unwrap();
    a.on_preview(again.gen, PreviewData::Basic { poster: None, note: None });
    let copied: Vec<(u32, u32)> = a.drain_effects().filter_map(|e| if let Effect::SetClipboardImage(b) = e { Some((b.width, b.height)) } else { None }).collect();
    assert_eq!(copied, [(side, 1)], "when the file cannot be decoded again the shown image is copied");
    assert!(matches!(a.preview.content, Content::Image));
}

#[test]
fn copied_text_follows_paragraphs_words_and_the_line_filter() {
    let d = preview::text_doc("one two\n\nthree", None);
    assert_eq!(para_span(&d, 1), (7, 7));
    assert_eq!(para_span(&d, 2), (7, 12));
    assert_eq!(para_span(&d, 9), (12, 12));
    let all = (Spot::default(), Spot { para: 2, at: 12 });
    assert_eq!(picked_text(&d, all, "", "\n"), "one two\n\nthree");
    assert_eq!(picked_text(&d, all, "t", "\n"), "one two\nthree", "hidden lines stay out of the copy");
    assert_eq!(picked_text(&d, (Spot { para: 0, at: 3 }, Spot { para: 0, at: 3 }), "", "\n"), "");
    assert_eq!(picked_text(&preview::text_doc("", None), all, "", "\n"), "");
    assert_eq!(word_at("let snake_case = 42;", 7), 4..14);
    assert_eq!(word_at("a  b", 2), 1..3);
    assert_eq!(word_at("x.y", 1), 0..1);
    assert_eq!(word_at("", 0), 0..0);
    assert_eq!(word_at("été", 1), 0..5);
}

#[test]
fn the_selection_is_painted_under_the_picked_characters_and_dims_without_focus() {
    let mut a = text_preview((1200, 700), "alpha beta\ngamma");
    drag_spots(&mut a, (0, 6), (1, 0));
    let painted = |a: &mut App, color: u32| {
        let mut g = Gfx::new(Mono, 2048);
        a.frame(&mut g, Instant::now());
        g.dl.instances.iter().filter(|i| i.color == color).map(|i| (i.rect[0], i.rect[2])).collect::<Vec<_>>()
    };
    let x0 = a.preview.r.body.x + a.preview.r.gutter;
    let lit = a.theme().text_selection.0;
    let line_h = a.preview.doc.as_ref().unwrap().lines[0].h;
    assert_eq!(painted(&mut a, lit), [(x0 + 60.0, 40.0 + (line_h * 0.3).round())], "the line break shows as a short tail");
    a.focus = Focus::List;
    let dim = a.theme().selection_inactive.0;
    assert!(painted(&mut a, dim).contains(&(x0 + 60.0, 40.0 + (line_h * 0.3).round())));
}

#[test]
fn shift_arrows_extend_the_preview_selection_like_a_text_editor() {
    const SHIFT: Mods = Mods { ctrl: false, shift: true, alt: false };
    let mut a = text_preview((1200, 700), "one two\nthree\nfour");
    click_spot(&mut a, (0, 4), MouseButton::Left, 1);
    for _ in 0..3 {
        press(&mut a, Key::Right, SHIFT);
    }
    assert_eq!(copied(&mut a, Key::Char('C'), CTRL).0, ["two"]);
    press(&mut a, Key::Down, SHIFT);
    assert_eq!(copied(&mut a, Key::Char('C'), CTRL).0, ["two\r\nthree"], "Shift+Down runs to the same column on the next line, here its end");
    press(&mut a, Key::Left, SHIFT);
    press(&mut a, Key::Home, SHIFT);
    assert_eq!(copied(&mut a, Key::Char('C'), CTRL).0, ["two\r\n"]);
    press(&mut a, Key::End, Mods { ctrl: true, shift: true, alt: false });
    assert_eq!(copied(&mut a, Key::Char('C'), CTRL).0, ["two\r\nthree\r\nfour"]);
    press(&mut a, Key::Right, SHIFT);
    assert_eq!(copied(&mut a, Key::Char('C'), CTRL).0, ["two\r\nthree\r\nfour"], "the end of the text stops the caret");
    press(&mut a, Key::Home, Mods { ctrl: true, shift: true, alt: false });
    press(&mut a, Key::Up, SHIFT);
    assert_eq!(copied(&mut a, Key::Char('C'), CTRL).0, ["one "], "the selection flips to the anchor's left");
    let before = a.preview.pick.range();
    press(&mut a, Key::Right, NO);
    assert_eq!(a.preview.pick.range(), before, "an arrow without Shift does not touch the selection");
}

#[test]
fn a_hosted_preview_released_for_an_operation_hides_and_comes_back_when_it_ends() {
    let mut a = app((1200, 700));
    a.set_pane_state(PaneState { visible: true, ..Default::default() });
    select(&mut a, "c.docx");
    frame(&mut a);
    let r = requests(&mut a).pop().unwrap();
    a.on_preview(r.gen, PreviewData::Handler("{84F66100-FF7C-4fb4-B0C0-02CD7FB668FE}".into()));
    frame(&mut a);
    a.drain_effects().for_each(drop);
    a.on_event(&Event::Hosted(HostEvent::Released { gen: r.gen + 1 }), Instant::now());
    assert!(matches!(a.preview.content, Content::Handler) && !a.drain_effects().any(|e| matches!(e, Effect::Host(_))), "another preview's release changes nothing");
    a.on_event(&Event::Hosted(HostEvent::Released { gen: r.gen }), Instant::now());
    assert!(matches!(a.preview.content, Content::Basic(None)));
    assert!(a.drain_effects().any(|e| matches!(e, Effect::Host(HostCmd::Close))), "the preview window leaves the pane");
    frame(&mut a);
    assert!(requests(&mut a).is_empty(), "nothing reopens the file while the operation runs");
    let tab = a.tab().id;
    a.op_done(tab, 77, Some("The process cannot access the file".into()), None, Instant::now());
    frame(&mut a);
    assert_eq!(requests(&mut a).len(), 1, "the file is still there, so its preview comes back");
}
