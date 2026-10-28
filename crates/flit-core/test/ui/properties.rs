use super::*;
use crate::input::{Event, Key, Mods};
use crate::listing::{DirSnapshot, Listing, Meta};
use crate::location::Location;
use crate::preview::PreviewData;
use crate::provider::ProviderEvent;
use crate::sort::{self, CaseMode, SortKeys, ViewSpec};
use crate::ui::Persisted;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn app_at(path: &str) -> App {
    App::new(Persisted { path: Location::parse(path).unwrap(), ..Default::default() }, Vec::new(), 0, false, 1.0, (1000, 700), Instant::now())
}

fn deliver(app: &mut App, items: &[(&str, u32, u64)]) {
    let t = app.tab();
    let mut listing = Listing::default();
    let mut keys = SortKeys::new(CaseMode::Insensitive);
    for &(n, attrs, size) in items {
        let i = listing.push(n.as_bytes(), Meta { attrs, size, modified: 133_000_000_000_000_000, created: 132_000_000_000_000_000 });
        keys.push(listing.name(i));
    }
    let mut order = Vec::new();
    sort::sort(&listing, &keys, ViewSpec::default(), &mut order);
    let snap = DirSnapshot { gen: t.gen, path: t.location().path().clone(), listing, keys, order, view: ViewSpec::default(), done: true, error: None, elapsed: Duration::ZERO };
    t.slot.publish(Arc::new(snap));
    app.poll();
}

fn select(app: &mut App, names: &[&str]) {
    let v = app.tabs[app.cur].view.as_mut().unwrap();
    v.clear_selection();
    for n in names {
        let i = v.find(n.as_bytes()).unwrap();
        v.set_selected(i, true);
    }
}

fn value(app: &App, label: S) -> Option<String> {
    app.jobs.form.as_ref()?.controls.iter().find_map(|c| match c {
        Control::Value { label: l, value } if l == label.t() => Some(value.clone()),
        _ => None,
    })
}

fn measure(app: &mut App) -> (OpId, Vec<(FsPath, Option<u64>)>) {
    let found: Vec<_> = app.drain_effects().filter_map(|e| if let Effect::Provider { req: ProviderRequest::Measure { id, items, .. }, .. } = e { Some((id, items)) } else { None }).collect();
    let [one] = found.try_into().unwrap();
    one
}

#[test]
fn texts_name_counts_and_attributes() {
    assert_eq!(attributes_text(attr::READONLY | attr::HIDDEN | attr::DIR), format!("{}, {}", S::PropertiesReadOnly.t(), S::PropertiesHidden.t()));
    assert_eq!(attributes_text(0), "");
    assert!(contains_text(3, 1).starts_with('3'));
}

#[test]
fn a_folder_counts_in_the_background_and_esc_closes() {
    let mut app = app_at(r"C:\work");
    deliver(&mut app, &[("docs", attr::DIR | attr::READONLY, 0), ("a.txt", 0, 5)]);
    select(&mut app, &["docs"]);
    app.run_action("item.properties", Instant::now());
    assert_eq!(app.asking(), Some("properties"));
    let (id, items) = measure(&mut app);
    assert_eq!(items, vec![(FsPath::parse(r"C:\work\docs").unwrap(), None)]);
    assert_eq!(value(&app, S::PropertiesName).as_deref(), Some("docs"));
    assert_eq!(value(&app, S::PropertiesLocation).as_deref(), Some(r"C:\work"));
    assert_eq!(value(&app, S::PropertiesAttributes).as_deref(), Some(S::PropertiesReadOnly.t()));
    assert_eq!(value(&app, S::PropertiesSize).as_deref(), Some(S::PropertiesCounting.t()));
    assert!(value(&app, S::PropertiesUnpacked).is_none());
    let tab = app.tab().id;
    app.on_provider_event(ProviderEvent::Measured { tab, id: id + 1, totals: Totals { files: 9, ..Totals::default() }, file_system: None, done: true }, Instant::now());
    assert_eq!(value(&app, S::PropertiesContains).as_deref(), Some(S::PropertiesCounting.t()));
    app.on_provider_event(ProviderEvent::Measured { tab, id, totals: Totals { files: 4, folders: 2, size: 2048, on_disk: 4096, ..Totals::default() }, file_system: Some("NTFS".into()), done: true }, Instant::now());
    assert_eq!(value(&app, S::PropertiesContains), Some(contains_text(4, 2)));
    assert!(value(&app, S::PropertiesSize).unwrap().contains("2"));
    assert!(value(&app, S::PropertiesOnDisk).unwrap().contains("4"));
    assert_eq!(value(&app, S::PropertiesFileSystem).as_deref(), Some("NTFS"));
    let Some(Form { kind: Kind::Properties(m), .. }) = &app.jobs.form else { panic!() };
    let ctl = m.ctl.clone();
    app.on_event(&Event::Key { key: Key::Escape, mods: Mods::default(), pressed: true }, Instant::now());
    assert_eq!(app.asking(), None);
    assert!(ctl.is_cancelled());
}

#[test]
fn a_single_file_shows_the_shell_details_and_no_leftover_note() {
    let mut app = app_at(r"C:\work");
    deliver(&mut app, &[("a.txt", 0, 5), ("b.txt", 0, 7)]);
    select(&mut app, &["a.txt"]);
    app.properties_dialog();
    let effects: Vec<Effect> = app.drain_effects().collect();
    let req = effects.iter().find_map(|e| if let Effect::Preview(r) = e { Some(r.clone()) } else { None }).unwrap();
    assert!(req.details && req.path == FsPath::parse(r"C:\work\a.txt").unwrap());
    let id = effects.iter().find_map(|e| if let Effect::Provider { req: ProviderRequest::Measure { id, .. }, .. } = e { Some(*id) } else { None }).unwrap();
    let rows = vec![("Authors".to_string(), "Ann".to_string())];
    app.on_preview(req.gen + 1, PreviewData::Props(vec![PropGroup { title: "Origin".into(), rows: rows.clone() }]));
    assert!(app.jobs.form.as_ref().unwrap().controls.iter().all(|c| !matches!(c, Control::Value { label, .. } if label == "Authors")));
    app.on_preview(req.gen, PreviewData::Props(vec![PropGroup { title: "Origin".into(), rows }]));
    let f = app.jobs.form.as_ref().unwrap();
    assert!(f.controls.iter().any(|c| matches!(c, Control::Value { label, value } if label == "Authors" && value == "Ann")));
    let tab = app.tab().id;
    app.on_provider_event(ProviderEvent::Measured { tab, id, totals: Totals { size: 5, ..Totals::default() }, file_system: Some("NTFS".into()), done: true }, Instant::now());
    assert_eq!(app.jobs.form.as_ref().unwrap().note, "");
}

#[test]
fn an_archive_root_also_measures_its_unpacked_contents_and_shift_opens_the_shell() {
    let mut app = app_at(r"C:\work");
    deliver(&mut app, &[("pack.zip", 0, 300), ("b.txt", 0, 5)]);
    select(&mut app, &["pack.zip"]);
    app.properties_dialog();
    let (_, items) = measure(&mut app);
    let zip = FsPath::parse(r"C:\work\pack.zip").unwrap();
    assert_eq!(items, vec![(zip.clone(), Some(300)), (zip, None)]);
    assert!(value(&app, S::PropertiesUnpacked).is_some() && value(&app, S::PropertiesContains).is_some());
    app.jobs.form = None;
    select(&mut app, &["pack.zip", "b.txt"]);
    app.run_action("item.inspect", Instant::now());
    assert!(app.drain_effects().any(|e| matches!(e, Effect::Properties(p) if p.len() == 2)));
}
