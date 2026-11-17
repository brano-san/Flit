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

#[test]
fn linux_background_properties_keep_selection_and_measure_the_current_folder() {
    let mut app = app_at("/home/user/folder");
    app.caps.unix_paths = true;
    deliver(&mut app, &[("selected.txt", 0, 5)]);
    app.tab_mut().view.as_mut().unwrap().set_selected(0, true);
    let path = app.current_location().path().clone();
    app.drain_effects().for_each(drop);
    app.properties_for(Some(path.clone()));
    assert!(app.jobs.form.is_some());
    assert!(app.tab().view.as_ref().unwrap().is_selected(0));
    assert!(app.drain_effects().any(|e| matches!(e, Effect::Provider { req: ProviderRequest::Measure { items, .. }, .. } if items.iter().any(|(p, _)| p == &path))));
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

fn flags(app: &App) -> Option<Vec<(String, bool)>> {
    app.jobs.form.as_ref()?.controls.iter().find_map(|c| match c {
        Control::Flags { label, items, .. } if label == S::PropertiesAttributes.t() => Some(items.clone()),
        _ => None,
    })
}

fn set_flags(app: &mut App, on: &[bool]) {
    let f = app.jobs.form.as_mut().unwrap();
    let k = f.controls.iter().position(|c| matches!(c, Control::Flags { .. })).expect("no attribute flags");
    let Control::Flags { items, .. } = &mut f.controls[k] else { unreachable!() };
    for ((_, now), &want) in items.iter_mut().zip(on) {
        *now = want;
    }
    app.form_changed(k);
}

fn check_unblock(app: &mut App) {
    let f = app.jobs.form.as_mut().unwrap();
    let k = f.controls.iter().position(|c| matches!(c, Control::Check { label, .. } if label == S::PropertiesUnblock.t())).expect("no unblock box");
    if let Control::Check { on, .. } = &mut f.controls[k] {
        *on = true;
    }
    app.form_changed(k);
}

fn file_ops(app: &mut App) -> Vec<FileOp> {
    app.drain_effects().filter_map(|e| if let Effect::Provider { req: ProviderRequest::Op { op, .. }, .. } = e { Some(op) } else { None }).collect()
}

fn props_reply(app: &mut App, groups: Vec<PropGroup>) {
    let req = app.drain_effects().find_map(|e| if let Effect::Preview(r) = e { Some(r) } else { None }).unwrap();
    app.on_preview(req.gen, PreviewData::Props(groups));
}

fn security() -> PropGroup {
    PropGroup { title: S::PropertiesSecurity.t().into(), rows: vec![(S::PropertiesSecurity.t().into(), S::PropertiesBlocked.t().into())] }
}

#[test]
fn a_file_shows_read_only_and_hidden_boxes_and_ok_changes_only_what_was_ticked() {
    let mut app = app_at(r"C:\work");
    deliver(&mut app, &[("a.txt", attr::READONLY | attr::COMPRESSED, 5), ("b.txt", 0, 7)]);
    select(&mut app, &["a.txt"]);
    app.properties_dialog();
    assert_eq!(flags(&app), Some(vec![(S::PropertiesReadOnly.t().to_owned(), true), (S::PropertiesHidden.t().to_owned(), false)]));
    assert!(app.jobs.form.as_ref().unwrap().controls.iter().any(|c| matches!(c, Control::Value { value, .. } if value == S::PropertiesCompressed.t())), "other attributes are still listed");
    assert_eq!(app.jobs.form.as_ref().unwrap().buttons, [S::ArchiveClose.t()], "nothing changed yet, so only Close is offered");
    app.drain_effects().for_each(drop);
    set_flags(&mut app, &[false, true]);
    assert_eq!(app.jobs.form.as_ref().unwrap().buttons, [S::ArchiveOk.t(), S::WidgetsCancel.t()]);
    app.form_submit(0);
    assert_eq!(file_ops(&mut app), [FileOp::SetAttributes { items: vec![FsPath::parse(r"C:\work\a.txt").unwrap()], set: attr::HIDDEN, clear: attr::READONLY, unblock: false }]);
    assert_eq!(app.asking(), None);
}

#[test]
fn ok_without_changes_or_cancel_after_changes_runs_nothing_and_mixed_boxes_stay_untouched() {
    let mut app = app_at(r"C:\work");
    deliver(&mut app, &[("a.txt", attr::HIDDEN, 5), ("b.txt", 0, 7)]);
    select(&mut app, &["a.txt", "b.txt"]);
    app.properties_dialog();
    assert_eq!(flags(&app), Some(vec![(S::PropertiesReadOnly.t().to_owned(), false), (S::PropertiesHidden.t().to_owned(), false)]), "hidden on only one of two items is shown unticked");
    app.drain_effects().for_each(drop);
    app.form_submit(0);
    assert!(file_ops(&mut app).is_empty());
    assert_eq!(app.asking(), None);
    app.properties_dialog();
    app.drain_effects().for_each(drop);
    set_flags(&mut app, &[true, true]);
    app.form_submit(1);
    assert!(file_ops(&mut app).is_empty());
    assert_eq!(app.asking(), None);
}

#[test]
fn a_downloaded_file_offers_unblock_and_ok_removes_the_mark_with_the_attribute_change() {
    let mut app = app_at(r"C:\Downloads");
    deliver(&mut app, &[("setup.exe", 0, 5)]);
    select(&mut app, &["setup.exe"]);
    app.properties_dialog();
    props_reply(&mut app, vec![PropGroup { title: "Origin".into(), rows: vec![("Authors".into(), "Ann".into())] }, security()]);
    assert_eq!(value(&app, S::PropertiesSecurity).as_deref(), Some(S::PropertiesBlocked.t()));
    assert!(app.jobs.form.as_ref().unwrap().controls.iter().any(|c| matches!(c, Control::Value { label, .. } if label == "Authors")));
    check_unblock(&mut app);
    app.form_submit(0);
    assert_eq!(file_ops(&mut app), [FileOp::SetAttributes { items: vec![FsPath::parse(r"C:\Downloads\setup.exe").unwrap()], set: 0, clear: 0, unblock: true }]);
}

#[test]
fn unblock_is_not_offered_without_the_mark_and_unix_items_keep_plain_attributes() {
    let mut app = app_at(r"C:\Downloads");
    deliver(&mut app, &[("notes.txt", attr::READONLY, 5)]);
    select(&mut app, &["notes.txt"]);
    app.properties_dialog();
    props_reply(&mut app, vec![PropGroup { title: "Origin".into(), rows: vec![("Authors".into(), "Ann".into())] }]);
    assert!(!app.jobs.form.as_ref().unwrap().controls.iter().any(|c| matches!(c, Control::Check { .. })));
    let mut app = app_at("/home/user/folder");
    app.caps.unix_paths = true;
    deliver(&mut app, &[("notes.txt", attr::READONLY, 5)]);
    select(&mut app, &["notes.txt"]);
    app.properties_dialog();
    assert_eq!(flags(&app), None);
    assert_eq!(value(&app, S::PropertiesAttributes).as_deref(), Some(S::PropertiesReadOnly.t()));
    assert_eq!(app.jobs.form.as_ref().unwrap().buttons.len(), 1);
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
    assert_eq!(flags(&app), Some(vec![(S::PropertiesHidden.t().to_owned(), false)]), "a folder's read-only bit is not offered, as in Explorer");
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
fn an_open_asking_for_properties_shows_them_once_the_item_is_selected() {
    let mut app = app_at(r"C:\work");
    deliver(&mut app, &[("b.txt", 0, 7)]);
    let open = crate::open::Open { select: vec![br"C:\other\a.txt".to_vec()], target: crate::open::OpenTarget::NewTab, properties: true, ..Default::default() };
    app.open(&open, &|_| None, Instant::now());
    assert_eq!(app.asking(), None);
    deliver(&mut app, &[("a.txt", 0, 5), ("c.txt", 0, 1)]);
    assert_eq!(app.asking(), Some("properties"));
    assert_eq!(value(&app, S::PropertiesName).as_deref(), Some("a.txt"));
    app.jobs.form = None;
    deliver(&mut app, &[("a.txt", 0, 5), ("c.txt", 0, 1)]);
    assert_eq!(app.asking(), None);
    app.open(&crate::open::Open { properties: false, ..open }, &|_| None, Instant::now());
    deliver(&mut app, &[("a.txt", 0, 5)]);
    assert_eq!(app.asking(), None);
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

#[test]
fn a_recent_file_from_the_home_page_shows_as_a_file_on_unix_paths() {
    let mut app = app_at("/home/user/folder");
    app.caps.unix_paths = true;
    let path = FsPath::parse("/home/user/notes.txt").unwrap();
    app.drain_effects().for_each(drop);
    app.place_command(crate::ui::sidebar::PlaceCmd::FileProperties(path.clone()), Instant::now());
    assert_eq!(value(&app, S::PropertiesType).as_deref(), Some("TXT File"));
    assert_eq!(value(&app, S::PropertiesSize).as_deref(), Some(S::PropertiesCounting.t()), "the size is measured, not taken as zero");
    assert_eq!(value(&app, S::PropertiesContains), None, "a file contains nothing");
    assert!(app.drain_effects().any(|e| matches!(e, Effect::Provider { req: ProviderRequest::Measure { items, .. }, .. } if items == vec![(path.clone(), None)])));
    let mut app = app_at(r"C:\x");
    app.drain_effects().for_each(drop);
    let path = FsPath::parse(r"C:\x\notes.txt").unwrap();
    app.place_command(crate::ui::sidebar::PlaceCmd::FileProperties(path.clone()), Instant::now());
    assert!(app.jobs.form.is_none());
    assert!(app.drain_effects().any(|e| matches!(e, Effect::Properties(p) if p == vec![path.clone()])));
}

#[test]
fn sidebar_properties_open_the_core_dialog_on_unix_paths_only() {
    let mut app = app_at("/home/user/folder");
    app.caps.unix_paths = true;
    let path = FsPath::parse("/home/user/other").unwrap();
    app.drain_effects().for_each(drop);
    app.place_command(crate::ui::sidebar::PlaceCmd::Properties(path.clone()), Instant::now());
    assert!(app.jobs.form.is_some());
    assert!(app.drain_effects().any(|e| matches!(e, Effect::Provider { req: ProviderRequest::Measure { items, .. }, .. } if items.iter().any(|(p, _)| p == &path))));
}

#[test]
fn long_properties_stay_in_the_window_and_scroll_below_a_fixed_header() {
    let mut app = app_at(r"C:\work");
    deliver(&mut app, &[("a.txt", 0, 5)]);
    select(&mut app, &["a.txt"]);
    app.properties_dialog();
    props_reply(&mut app, vec![PropGroup { title: "Details".into(), rows: (0..80).map(|i| (format!("Property {i}"), "value".into())).collect() }]);
    app.on_event(&Event::Resized { width: 420, height: 320 }, Instant::now());
    let f = app.jobs.form.as_ref().unwrap();
    assert!(f.rect.y >= 0.0 && f.rect.bottom() <= 320.0);
    assert!(f.rect.x >= 0.0 && f.rect.right() <= 420.0);
    assert!(f.document.max() > 0.0);
    let buttons = f.button_rects.clone();
    app.form_key(Key::End, Mods::default());
    let f = app.jobs.form.as_ref().unwrap();
    assert!(f.document.scroll.target > 0.0);
    assert_eq!(f.button_rects, buttons);
}

#[test]
fn outside_click_closes_unchanged_properties_but_keeps_edits() {
    let mut app = app_at(r"C:\work");
    deliver(&mut app, &[("a.txt", 0, 5)]);
    select(&mut app, &["a.txt"]);
    app.properties_dialog();
    let ctl = match &app.jobs.form.as_ref().unwrap().kind { Kind::Properties(m) => m.ctl.clone(), _ => unreachable!() };
    let r = app.jobs.form.as_ref().unwrap().rect;
    app.form_down(r.x + 2.0, r.y + 2.0, crate::input::MouseButton::Left, 1, Instant::now());
    assert!(app.jobs.form.is_some(), "inside click keeps the dialog open");
    app.drain_effects().for_each(drop);
    set_flags(&mut app, &[true, true]);
    app.form_down(1.0, 100.0, crate::input::MouseButton::Left, 1, Instant::now());
    assert!(app.jobs.form.is_some(), "a stray click never discards edited attributes");
    assert!(file_ops(&mut app).is_empty());
    set_flags(&mut app, &[false, false]);
    app.form_down(1.0, 100.0, crate::input::MouseButton::Left, 1, Instant::now());
    assert!(app.jobs.form.is_none());
    assert!(ctl.is_cancelled());
    assert!(file_ops(&mut app).is_empty());
}

#[test]
fn header_close_is_accessible_and_cancels_pending_attribute_changes() {
    let mut app = app_at(r"C:\work");
    deliver(&mut app, &[("a.txt", 0, 5)]);
    select(&mut app, &["a.txt"]);
    app.properties_dialog();
    set_flags(&mut app, &[true, true]);
    let f = app.jobs.form.as_ref().unwrap();
    let k = f.controls.len() + f.buttons.len();
    let close = f.close_rect;
    assert_eq!(app.form_hit(close.x + 1.0, close.y + 1.0), Some(crate::ui::Hit::Form(k)));
    let mut tree = crate::a11y::Tree::default();
    let mut targets = Vec::new();
    app.form_a11y(&mut tree, &mut targets);
    assert!(targets.iter().any(|(_, h)| *h == crate::ui::Hit::Form(k)));
    app.drain_effects().for_each(drop);
    app.form_a11y_invoke(k);
    assert!(app.jobs.form.is_none());
    assert!(file_ops(&mut app).is_empty());
}

#[test]
fn async_dates_update_existing_rows_without_duplicates_or_shifting_measurement_targets() {
    let mut app = app_at("/home/user/folder");
    app.caps.unix_paths = true;
    deliver(&mut app, &[("notes.txt", 0, 5)]);
    select(&mut app, &["notes.txt"]);
    app.properties_dialog();
    let (id, indices) = match &app.jobs.form.as_ref().unwrap().kind {
        Kind::Properties(m) => (m.id, (m.rows.size, m.rows.on_disk, m.rows.accessed, m.rows.file_system)),
        _ => unreachable!(),
    };
    props_reply(&mut app, vec![PropGroup { title: S::PropertiesDates.t().into(), rows: vec![
        (S::PropertiesCreated.t().into(), "native creation date".into()),
        (S::PropertiesAccessed.t().into(), "native access date".into()),
        (S::PropertiesChanged.t().into(), "old metadata date".into()),
        (S::PropertiesChanged.t().into(), "native metadata date".into()),
    ] }]);
    for (label, expected) in [(S::PropertiesCreated, "native creation date"), (S::PropertiesAccessed, "native access date"), (S::PropertiesChanged, "native metadata date")] {
        let rows: Vec<_> = app.jobs.form.as_ref().unwrap().controls.iter().filter_map(|c| match c {
            Control::Value { label: shown, value } if shown == label.t() => Some(value.as_str()),
            _ => None,
        }).collect();
        assert_eq!(rows, [expected], "each date has one updated row");
    }
    let Kind::Properties(m) = &app.jobs.form.as_ref().unwrap().kind else { unreachable!() };
    assert_eq!((m.rows.size, m.rows.on_disk, m.rows.accessed, m.rows.file_system), indices);
    let accessed = 133_000_000_000_000_000;
    app.measured(id, Totals { size: 42, on_disk: 4096, accessed, ..Default::default() }, Some("ext4".into()), true);
    assert_eq!(value(&app, S::PropertiesSize), Some(app.bytes_text(42)));
    assert_eq!(value(&app, S::PropertiesOnDisk), Some(app.bytes_text(4096)));
    assert_eq!(value(&app, S::PropertiesAccessed), Some(app.date_text(accessed)));
    assert_eq!(value(&app, S::PropertiesFileSystem).as_deref(), Some("ext4"));
    assert_eq!(value(&app, S::PropertiesCreated).as_deref(), Some("native creation date"));
    assert_eq!(value(&app, S::PropertiesChanged).as_deref(), Some("native metadata date"));
}

#[test]
fn unchanged_properties_offer_one_close_and_ok_with_cancel_appear_only_while_something_is_changed() {
    let mut app = app_at(r"C:\work");
    deliver(&mut app, &[("a.txt", 0, 5)]);
    select(&mut app, &["a.txt"]);
    app.properties_dialog();
    app.layout();
    let f = app.jobs.form.as_ref().unwrap();
    assert_eq!(f.buttons, [S::ArchiveClose.t()]);
    assert_eq!(f.button_rects, [f.close_rect], "the only way out is the header close, not a second button below");
    set_flags(&mut app, &[true, false]);
    let f = app.jobs.form.as_ref().unwrap();
    assert_eq!(f.buttons, [S::ArchiveOk.t(), S::WidgetsCancel.t()]);
    assert!(f.button_rects.iter().all(|r| *r != f.close_rect && r.y > f.close_rect.y), "OK and Cancel sit at the bottom");
    set_flags(&mut app, &[false, false]);
    assert_eq!(app.jobs.form.as_ref().unwrap().buttons, [S::ArchiveClose.t()], "undoing the change brings back the single Close");
    app.drain_effects().for_each(drop);
    app.form_key(Key::Enter, Mods::default());
    assert!(app.jobs.form.is_none());
    assert!(file_ops(&mut app).is_empty(), "closing an unchanged dialog changes nothing");
}
