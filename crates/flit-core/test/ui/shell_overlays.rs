use super::*;
use crate::listing::{attr, ChangeKind, DirChange, Meta};
use crate::render::kind;
use crate::text::tests::Mono;
use crate::text::Gfx;
use crate::ui::{Bitmap, Effect, IconResult, LayoutMode, OverlayBatch, OverlayReply, OVERLAY_BATCH};

fn listed(dir: &str, items: &[(&str, u32, i64)]) -> App {
    let mut app = app_at(dir);
    app.set_shell_overlays(true);
    publish_meta(&mut app, items);
    app.drain_effects().for_each(drop);
    app
}

fn publish_meta(app: &mut App, items: &[(&str, u32, i64)]) {
    let t = app.tab();
    let mut listing = Listing::default();
    let mut keys = SortKeys::new(sort::CaseMode::Insensitive);
    for &(n, attrs, modified) in items {
        let i = listing.push(n.as_bytes(), Meta { attrs, size: 10, modified, ..Default::default() });
        keys.push(listing.name(i));
    }
    let mut order = Vec::new();
    sort::sort(&listing, &keys, ViewSpec::default(), &mut order);
    t.slot.publish(Arc::new(DirSnapshot { gen: t.gen, path: t.location().path().clone(), listing, keys, order, view: ViewSpec::default(), done: true, error: None, elapsed: Duration::from_millis(1) }));
    app.poll();
}

fn bitmap(v: u8) -> Bitmap {
    Bitmap { width: 2, height: 2, rgba: vec![v; 16] }
}

fn frame(app: &mut App, gfx: &mut Gfx<Mono>) -> Vec<OverlayBatch> {
    app.frame(gfx, Instant::now());
    let mut batches = Vec::new();
    for e in app.drain_effects().collect::<Vec<_>>() {
        match e {
            Effect::Overlays(b) => batches.push(b),
            Effect::Icon(r) => app.on_icon(gfx, r.key, IconResult::Ready(bitmap(200))),
            _ => {}
        }
    }
    batches
}

fn names(batches: &[OverlayBatch]) -> Vec<String> {
    let mut out: Vec<String> = batches.iter().flat_map(|b| &b.items).map(|i| String::from_utf8_lossy(i.path.file_name()).into_owned()).collect();
    out.sort();
    out
}

fn answer(app: &mut App, gfx: &mut Gfx<Mono>, batch: &OverlayBatch, slot_of: impl Fn(&str) -> Option<u8>, image: bool) {
    let found: Vec<(u64, Option<u8>)> = batch.items.iter().map(|i| (i.key, slot_of(&String::from_utf8_lossy(i.path.file_name())))).collect();
    let mut slots: Vec<u8> = found.iter().filter_map(|f| f.1).chain(batch.fetch.iter().copied()).filter(|s| !batch.have.contains(s)).collect();
    slots.dedup();
    let images = if image { slots.into_iter().map(|s| (s, bitmap(255))).collect() } else { Vec::new() };
    app.on_overlays(gfx, OverlayReply { id: batch.id, size: batch.size, found, skipped: Vec::new(), images });
}

fn stacked_icons(gfx: &Gfx<Mono>) -> usize {
    let images: Vec<[f32; 4]> = gfx.dl.instances.iter().filter(|i| i.kind == kind::IMAGE).map(|i| i.rect).collect();
    images.iter().enumerate().filter(|(k, r)| images[..*k].contains(r)).count()
}

const FILES: &[(&str, u32, i64)] = &[("a.txt", 0, 5), ("b.txt", 0, 5), ("src", attr::DIR, 5)];

#[test]
fn a_file_the_shell_marks_shows_its_overlay_over_the_shared_type_icon_and_unmarked_ones_do_not() {
    for mode in [LayoutMode::Details, LayoutMode::List, LayoutMode::Grid, LayoutMode::Cards, LayoutMode::Tree, LayoutMode::Columns] {
        let mut app = listed(r"C:\work", FILES);
        app.set_layout(mode);
        let mut gfx = Gfx::new(Mono, 1024);
        let batches = frame(&mut app, &mut gfx);
        assert_eq!(names(&batches), ["a.txt", "b.txt", "src"], "{mode:?}");
        assert!(batches.iter().flat_map(|b| &b.items).all(|i| i.path.parent().unwrap().as_bytes() == br"C:\work"));
        for b in &batches {
            answer(&mut app, &mut gfx, b, |n| (n == "a.txt").then_some(3), true);
        }
        assert!(frame(&mut app, &mut gfx).is_empty(), "{mode:?}: answers are cached");
        assert_eq!(stacked_icons(&gfx), 1, "{mode:?}: only a.txt has a second image drawn over its icon");
    }
}

#[test]
fn without_overlays_or_outside_local_drives_nothing_is_asked_or_drawn() {
    let mut off = listed(r"C:\work", FILES);
    off.set_shell_overlays(false);
    let mut gfx = Gfx::new(Mono, 1024);
    assert!(frame(&mut off, &mut gfx).is_empty());
    assert_eq!(stacked_icons(&gfx), 0);
    for dir in ["/tmp/work", r"\\server\share\work"] {
        let mut app = listed(dir, FILES);
        assert!(frame(&mut app, &mut Gfx::new(Mono, 1024)).is_empty(), "{dir}");
    }
    let mut app = listed(r"C:\work", &[("drive", attr::VOLUME, 0), ("cloud.txt", attr::NOT_LOCAL, 0), ("data.bin:zone", attr::STREAM, 0), ("plain.txt", 0, 0)]);
    assert_eq!(names(&frame(&mut app, &mut Gfx::new(Mono, 1024))), ["plain.txt"]);
    let mut none = listed(r"C:\work", FILES);
    let mut gfx = Gfx::new(Mono, 1024);
    for b in frame(&mut none, &mut gfx) {
        answer(&mut none, &mut gfx, &b, |_| None, true);
    }
    assert!(frame(&mut none, &mut gfx).is_empty());
    assert_eq!(stacked_icons(&gfx), 0, "no overlay index leaves the plain type icon");
}

#[test]
fn ten_thousand_files_ask_only_for_visible_rows_in_bounded_batches() {
    let names_: Vec<String> = (0..10_000).map(|k| format!("f{k:05}.txt")).collect();
    let items: Vec<(&str, u32, i64)> = names_.iter().map(|n| (n.as_str(), 0, 1)).collect();
    let mut app = listed(r"C:\big", &items);
    let mut gfx = Gfx::new(Mono, 1024);
    let first = frame(&mut app, &mut gfx);
    let asked = names(&first);
    assert!(!asked.is_empty() && asked.len() < 100, "{}", asked.len());
    assert!(first.iter().all(|b| b.items.len() <= OVERLAY_BATCH));
    assert!(asked.iter().all(|n| n.as_str() < "f00100.txt"));
    assert_eq!(app.visible_overlays().len(), asked.len());
    for _ in 0..200 {
        app.on_event(&crate::input::Event::Wheel { dx: 0.0, dy: -3.0 }, Instant::now());
    }
    let later = names(&frame(&mut app, &mut gfx));
    assert!(!later.is_empty() && later.len() < 100);
    assert!(later.iter().all(|n| !asked.contains(n)), "rows that scrolled away are not asked again");
    assert!(app.visible_overlays().len() < 100);
}

#[test]
fn a_watched_change_or_a_new_modification_time_asks_the_shell_again_and_late_answers_are_dropped() {
    let mut app = listed(r"C:\work", FILES);
    let mut gfx = Gfx::new(Mono, 1024);
    let first = frame(&mut app, &mut gfx);
    for b in &first {
        answer(&mut app, &mut gfx, b, |_| None, true);
    }
    let (tab, gen) = (app.tab().id, app.tab().gen);
    let change = DirChange { kind: ChangeKind::Modified, name: b"a.txt".to_vec(), meta: Some(Meta { size: 10, modified: 5, ..Default::default() }), label: Default::default() };
    app.on_provider_event(ProviderEvent::Changes { tab, gen, changes: vec![change] }, Instant::now());
    let again = frame(&mut app, &mut gfx);
    assert_eq!(names(&again), ["a.txt"], "same modification time, but the watcher saw a change");
    app.on_provider_event(ProviderEvent::Changes { tab, gen, changes: vec![DirChange { kind: ChangeKind::Modified, name: b"a.txt".to_vec(), meta: Some(Meta { size: 10, modified: 5, ..Default::default() }), label: Default::default() }] }, Instant::now());
    answer(&mut app, &mut gfx, &again[0], |_| Some(4), true);
    assert_eq!(names(&frame(&mut app, &mut gfx)), ["a.txt"], "an answer to a request made before the change is dropped");
    assert_eq!(stacked_icons(&gfx), 0);
    publish_meta(&mut app, &[("a.txt", 0, 5), ("b.txt", 0, 9), ("src", attr::DIR, 5)]);
    let b = frame(&mut app, &mut gfx);
    assert!(names(&b).contains(&"b.txt".to_owned()), "a new modification time is a new cache entry");
    for b in &b {
        answer(&mut app, &mut gfx, b, |_| None, true);
    }
    assert!(frame(&mut app, &mut gfx).is_empty());
    app.refresh(Instant::now());
    publish_meta(&mut app, &[("a.txt", 0, 5), ("b.txt", 0, 9), ("src", attr::DIR, 5)]);
    assert_eq!(names(&frame(&mut app, &mut gfx)), ["a.txt", "b.txt", "src"], "refreshing the folder asks for every overlay again");
}

#[test]
fn skipped_rows_are_asked_again_and_a_missing_image_is_fetched_once() {
    let mut app = listed(r"C:\work", FILES);
    let mut gfx = Gfx::new(Mono, 1024);
    let first = frame(&mut app, &mut gfx);
    let b = &first[0];
    let skipped: Vec<u64> = b.items.iter().filter(|i| i.path.file_name() == b"b.txt").map(|i| i.key).collect();
    let found = b.items.iter().filter(|i| i.path.file_name() != b"b.txt").map(|i| (i.key, (i.path.file_name() == b"a.txt").then_some(2))).collect();
    app.on_overlays(&mut gfx, OverlayReply { id: b.id, size: b.size, found, skipped, images: Vec::new() });
    let next = frame(&mut app, &mut gfx);
    assert_eq!(names(&next), ["b.txt"]);
    assert_eq!(next.iter().flat_map(|b| b.fetch.clone()).collect::<Vec<_>>(), [2], "the overlay image that never arrived is fetched");
    for b in &next {
        app.on_overlays(&mut gfx, OverlayReply { id: b.id, size: b.size, found: b.items.iter().map(|i| (i.key, None)).collect(), skipped: Vec::new(), images: Vec::new() });
    }
    assert!(frame(&mut app, &mut gfx).is_empty(), "an image the shell cannot give is not asked for on every frame");
    assert_eq!(stacked_icons(&gfx), 0);
}

#[test]
fn an_overlay_slot_outside_the_shell_range_is_ignored() {
    let mut app = listed(r"C:\work", FILES);
    let mut gfx = Gfx::new(Mono, 1024);
    for b in frame(&mut app, &mut gfx) {
        answer(&mut app, &mut gfx, &b, |_| Some(16), true);
    }
    assert!(frame(&mut app, &mut gfx).is_empty());
    assert_eq!(stacked_icons(&gfx), 0);
}
