use super::*;
use crate::input::Event;
use crate::listing::{attr, DirSnapshot, Listing, Meta};
use crate::sort::{self, CaseMode, SortKeys, ViewSpec};
use crate::text::tests::Mono;
use crate::provider::ProviderEvent;
use crate::ui::home::Recent;
use crate::ui::{Drive, KnownFolder, Persisted, Places};
use std::sync::Arc;

fn loc(s: &str) -> Location {
    Location::parse(s).unwrap()
}

fn app() -> App {
    let mut app = App::new(Persisted { path: loc(r"C:\start"), ..Default::default() }, Vec::new(), 0, false, 1.0, (1000, 700), Instant::now());
    let c = Drive { path: crate::path::FsPath::parse(r"C:\").unwrap(), label: "Disk (C:)".into(), total: 1 << 30, free: 1 << 29, removable: false, unmounted: false, kind: None, remote: None };
    let note = Recent { name: "note.txt".into(), target: crate::path::FsPath::parse(r"C:\docs\note.txt").unwrap(), dir: false, at: 5 };
    app.set_places(Arc::new(Places { drives: vec![c], folders: vec![(KnownFolder::Downloads, loc(r"C:\Users\u\Downloads"))], recent: vec![note], ..Default::default() }));
    app.frame(&mut Gfx::new(Mono, 1024), Instant::now());
    app.drain_effects().for_each(drop);
    app
}

fn hover(app: &mut App, label: &str, now: Instant) -> usize {
    let i = app.nav.sidebar.items.iter().position(|it| it.label == label).unwrap();
    let r = app.nav.sidebar.rects[i];
    app.on_event(&Event::MouseMove { x: r.x + 10.0, y: r.y + r.h / 2.0 }, now);
    i
}

fn answer(app: &mut App, items: &[(&str, bool, u64)]) {
    let p = app.quick.as_ref().and_then(|q| q.probe.as_ref()).expect("a parent listing");
    let mut listing = Listing::default();
    let mut keys = SortKeys::new(CaseMode::Insensitive);
    for &(n, dir, size) in items {
        let i = listing.push(n.as_bytes(), Meta { attrs: if dir { attr::DIR } else { 0 }, size, created: 133_000_000_000_000_000, ..Default::default() });
        keys.push(listing.name(i));
    }
    let mut order = Vec::new();
    sort::sort(&listing, &keys, ViewSpec::default(), &mut order);
    p.slot.publish(Arc::new(DirSnapshot { gen: p.gen, path: p.loc.path().clone(), listing, keys, order, view: ViewSpec::default(), done: true, error: None, elapsed: Duration::ZERO }));
    app.poll();
}

#[test]
fn a_folder_shows_its_properties_after_a_second() {
    let mut app = app();
    let t0 = Instant::now();
    hover(&mut app, "Downloads", t0);
    assert!(app.quick.as_ref().is_some_and(|q| q.shown.is_none()));
    app.on_event(&Event::Timer, t0 + QUICK_DELAY / 2);
    assert!(app.quick.as_ref().is_some_and(|q| q.shown.is_none()), "nothing before the delay");
    app.on_event(&Event::Timer, t0 + QUICK_DELAY);
    assert!(app.quick.as_ref().is_some_and(|q| q.shown.is_some()));
    let fx: Vec<Effect> = app.drain_effects().collect();
    let id = fx.iter().find_map(|e| if let Effect::Provider { req: ProviderRequest::Measure { id, items, .. }, .. } = e { Some((*id, items.clone())) } else { None }).expect("the folder is measured");
    assert_eq!(id.1, [(loc(r"C:\Users\u\Downloads").path().clone(), None)]);
    let rows = app.quick_rows();
    assert_eq!(rows.iter().map(|r| r.0.as_str()).collect::<Vec<_>>(), ["Subfolders:", "Subfiles:", "Size:"]);
    answer(&mut app, &[("Downloads", true, 0), ("other", true, 0)]);
    let totals = Totals { files: 69_800, folders: 11_900, size: 6_200_000_000, ..Default::default() };
    app.on_provider_event(ProviderEvent::Measured { tab: app.tab().id, id: id.0, totals, file_system: None, done: true }, Instant::now());
    let rows = app.quick_rows();
    assert_eq!(rows[0].0, "Created:");
    assert_eq!(rows[1], ("Subfolders:".to_owned(), "11.9K".to_owned()));
    assert_eq!(rows[2], ("Subfiles:".to_owned(), "69.8K".to_owned()));
    app.frame(&mut Gfx::new(Mono, 1024), Instant::now());
    app.on_event(&Event::MouseMove { x: 900.0, y: 400.0 }, Instant::now());
    assert!(app.quick.is_none(), "leaving the item hides it");
}

#[test]
fn a_recent_file_shows_its_type_system_properties_and_size() {
    let mut app = app();
    let t0 = Instant::now();
    hover(&mut app, "note.txt", t0);
    app.on_event(&Event::Timer, t0 + QUICK_DELAY);
    let fx: Vec<Effect> = app.drain_effects().collect();
    assert!(!fx.iter().any(|e| matches!(e, Effect::Provider { req: ProviderRequest::Measure { .. }, .. })), "files are not measured");
    let gen = fx.iter().find_map(|e| if let Effect::Preview(r) = e { r.details.then_some(r.gen) } else { None }).expect("system properties are asked for");
    answer(&mut app, &[("note.txt", false, 2048)]);
    let groups = vec![PropGroup { title: String::new(), rows: vec![("File description".into(), "Notes".into())] }];
    app.on_preview(gen, crate::preview::PreviewData::Props(groups));
    let rows: Vec<String> = app.quick_rows().into_iter().map(|r| r.0).collect();
    assert_eq!(rows, ["Type:", "File description", "Created:", "Size:"]);
}

#[test]
fn drives_show_their_size_and_tags_or_this_pc_show_nothing() {
    let mut app = app();
    let t0 = Instant::now();
    hover(&mut app, "Disk (C:)", t0);
    app.on_event(&Event::Timer, t0 + QUICK_DELAY);
    assert!(!app.drain_effects().any(|e| matches!(e, Effect::Provider { req: ProviderRequest::Measure { .. }, .. })));
    assert_eq!(app.quick_rows().iter().map(|r| r.0.as_str()).collect::<Vec<_>>(), ["Size:"]);
    hover(&mut app, "This PC", t0);
    assert!(app.quick.is_none());
    assert_eq!(compact_count(999), "999");
    assert_eq!(compact_count(1_500_000), "1.5M");
}
