use super::*;
use flit_core::sort::{GroupField, GroupSpec, SortSpec};
use flit_core::ui::{folder_key, Columns};

#[test]
fn corrupted_widths_and_placement_fall_back_to_defaults() {
    let mut s = Saved::new(Persisted::default(), Some(Placement { x: 0, y: 0, width: -5, height: 300, maximized: false }));
    s.persisted.view.columns.widths[1] = f32::NAN;
    s.persisted.view.columns.widths[2] = -10.0;
    s.persisted.view.columns.widths[3] = 120.0;
    s.persisted.prefs.defaults.columns.widths[4] = 1e9;
    let s = s.sanitized();
    let d = Columns::default().widths;
    assert_eq!(s.persisted.view.columns.widths[1..4], [d[1], d[2], 120.0]);
    assert_eq!(s.persisted.prefs.defaults.columns.widths[4], d[4]);
    assert!(s.placement().is_none());
}

fn loc(s: &str) -> Location {
    Location::parse(s).unwrap()
}

#[test]
fn nav_state_and_views_ride_after_the_stage_one_record() {
    let nav = NavState { pinned: vec![loc(r"C:\work"), loc("wsl://Ubuntu/home")], sidebar_w: 300.0, sidebar: false };
    let placement = Placement { x: 10, y: 20, width: 800, height: 600, maximized: true };
    let mut persisted = Persisted { path: loc(r"D:\x"), show_hidden: true, ..Default::default() };
    persisted.view.sort = SortSpec { field: SortField::Size, descending: true, ..Default::default() };
    persisted.view.columns.widths[Column::Type as usize] = 77.0;
    let s = Saved::new(persisted, Some(placement)).with_nav(nav.clone());
    let bytes = s.encode().unwrap();
    let stage_one = postcard::to_stdvec(&s.head()).unwrap();
    assert!(bytes.len() > stage_one.len() && bytes.starts_with(&stage_one));
    let back = Saved::decode(&bytes).unwrap();
    assert_eq!((&back.persisted, &back.nav, back.placement()), (&s.persisted, &nav, Some(placement)));
    let upgraded = Saved::decode(&stage_one).unwrap();
    assert_eq!((&upgraded.persisted, upgraded.placement()), (&s.persisted, Some(placement)));
    assert_eq!(upgraded.nav, NavState::default());
    let read_by_stage_one: Head = postcard::from_bytes(&bytes).unwrap();
    assert_eq!(read_by_stage_one, s.head());
}

#[test]
fn folder_views_and_preferences_survive_a_restart_and_created_sorts_by_name_for_older_builds() {
    let mut persisted = Persisted::default();
    persisted.view.sort.field = SortField::Created;
    persisted.prefs.same_for_all = true;
    persisted.prefs.show_extensions = false;
    let docs = folder_key(&loc(r"C:\Docs"));
    persisted.folders.put(docs, FolderView { group: GroupSpec { field: GroupField::Modified, ..Default::default() }, ..Default::default() });
    persisted.folders.put(folder_key(&loc(r"C:\Other")), FolderView::default());
    let s = Saved::new(persisted, None);
    assert_eq!(s.head().sort, SortField::Name);
    let mut tagged = s.clone();
    tagged.persisted.view.sort.field = SortField::Tag;
    assert_eq!(tagged.head().sort, SortField::Name, "older builds do not know tag sorting");
    let back = Saved::decode(&s.encode().unwrap()).unwrap().sanitized();
    assert_eq!(back.persisted.view.sort.field, SortField::Created);
    assert_eq!(back.persisted.prefs, s.persisted.prefs);
    assert_eq!(back.persisted.folders.entries(), s.persisted.folders.entries());
    assert_eq!(back.persisted.folders.get(docs).unwrap().group.field, GroupField::Modified);
}

#[test]
fn corrupted_nav_state_falls_back() {
    let mut s = Saved::new(Persisted::default(), None);
    s.nav = NavState { pinned: vec![loc(r"C:\a"), loc(r"C:\b"), loc(r"C:\a")], sidebar_w: f32::INFINITY, sidebar: true };
    let s = s.sanitized();
    assert_eq!(s.nav.pinned, [loc(r"C:\a"), loc(r"C:\b")]);
    assert_eq!(s.nav.sidebar_w, NavState::default().sidebar_w);
    let mut bytes = postcard::to_stdvec(&s.head()).unwrap();
    bytes.extend([0xFF, 0xFF, 0xFF]);
    assert_eq!(Saved::decode(&bytes).unwrap().nav, NavState::default());
}
