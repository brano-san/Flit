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

const CTRL_ALT: Mods = Mods { ctrl: true, shift: false, alt: true };

#[test]
fn toggles_and_reserves_the_right_or_bottom_edge() {
    let mut wide = app((1200, 700));
    let list_w = wide.layout.list.w;
    press(&mut wide, Key::Char('I'), CTRL_ALT);
    assert!(wide.pane_state().visible);
    let pane = wide.preview.r.pane;
    assert_eq!(pane.right(), 1200.0);
    assert_eq!(wide.layout.list.w, list_w - pane.w);
    let mut tall = app((700, 1200));
    press(&mut tall, Key::Char('I'), CTRL_ALT);
    let pane = tall.preview.r.pane;
    assert_eq!(pane.bottom(), tall.layout.status.y);
    assert!(tall.layout.list.bottom() <= pane.y);
    press(&mut tall, Key::Char('I'), CTRL_ALT);
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
fn details_tab_and_many_items() {
    let mut a = app((1200, 700));
    press(&mut a, Key::Char('P'), Mods { shift: true, alt: true, ctrl: false });
    assert!(a.pane_state().visible && a.pane_state().details);
    select(&mut a, "c.docx");
    frame(&mut a);
    let r = requests(&mut a).pop().unwrap();
    assert!(r.details);
    a.on_preview(r.gen, PreviewData::Props(vec![preview::PropGroup { title: "Document".into(), rows: vec![("Pages".into(), "3".into())] }]));
    frame(&mut a);
    assert!(a.preview.extent > 0.0);
    press(&mut a, Key::Char('A'), Mods { ctrl: true, ..Mods::default() });
    frame(&mut a);
    assert!(requests(&mut a).is_empty());
    assert_eq!(a.preview.subject.as_ref().unwrap().many, Some((4, 300)));
    let b = a.preview.r.button;
    a.on_event(&Event::MouseDown { x: b.x + 2.0, y: b.y + 2.0, button: crate::input::MouseButton::Left, clicks: 1 }, Instant::now());
    a.on_event(&Event::MouseUp { x: b.x + 2.0, y: b.y + 2.0, button: crate::input::MouseButton::Left }, Instant::now());
    assert!(a.drain_effects().any(|e| matches!(e, Effect::Properties(p) if p.len() == 4)));
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
    assert!(matches!(&a.preview.content, Content::Basic(Some(m)) if m == "no handler"));
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
    a.preview_mouse_down(PaneHit::Volume, v.right(), v.y);
    assert_eq!(a.pane_state().volume, 1.0);
    select(&mut a, "a.rs");
    frame(&mut a);
    assert!(a.drain_effects().any(|e| matches!(e, Effect::Host(HostCmd::Close))));
}

#[test]
fn space_quick_views_or_peeks_with_the_pane() {
    let mut a = app((1200, 700));
    select(&mut a, "a.rs");
    a.on_event(&Event::Char(' '), Instant::now());
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
    let lay = layout_doc(&mut g, &d, 95.0, 1.0, &mut widths);
    let lines: Vec<String> = lay.lines.iter().map(|l| lay.pieces[l.pieces.start as usize..l.pieces.end as usize].iter().map(|p| &d.text[p.start as usize..p.end as usize]).collect()).collect();
    assert_eq!(lines, ["aaaa bbbb ", "cccc", "verylongw", "ordthatbr", "eaks"]);
    let code = preview::text_doc("fn main() { let x = 1; }", Some(preview::syntax::by_ext("rs").unwrap()));
    assert_eq!(layout_doc(&mut g, &code, 50.0, 1.0, &mut widths).lines.len(), 1);
    let md = preview::markdown_doc("# Title\n\nbody text here");
    let lay = layout_doc(&mut g, &md, 400.0, 1.0, &mut widths);
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
