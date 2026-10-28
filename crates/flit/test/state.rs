use super::*;
use flit_core::sort::{GroupField, GroupSpec, SortSpec};
use flit_core::ui::{folder_key, Columns};

const GIT_LEN: usize = 35;
const WORD_LEN: usize = 1 + GIT_LEN;

#[test]
fn repository_columns_ride_in_their_own_record() {
    let mut persisted = Persisted { path: loc(r"D:\x"), ..Default::default() };
    persisted.view.columns.toggle(Column::GitStatus);
    persisted.view.columns.set_width(Column::GitMessage, 333.0);
    persisted.prefs.defaults.columns.toggle(Column::GitDate);
    let s = Saved::new(persisted, None);
    let back = Saved::decode(&s.encode().unwrap()).unwrap();
    assert!(back.persisted.view.columns.shown(Column::GitStatus));
    assert_eq!(back.persisted.view.columns.width(Column::GitMessage), 333.0);
    assert!(back.persisted.prefs.defaults.columns.shown(Column::GitDate));
    assert!(!back.persisted.prefs.defaults.columns.shown(Column::GitStatus));
}

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
fn layouts_and_item_sizes_ride_in_their_own_record_so_older_builds_still_read_details() {
    let mut persisted = Persisted::default();
    persisted.view.layout = LayoutMode::Grid;
    persisted.view.sizes = Sizes([0, 4, 3, 2, 11]);
    persisted.prefs.defaults.layout = LayoutMode::Adaptive;
    persisted.prefs.thumbnails = false;
    persisted.prefs.thumb_cache_mb = 1200;
    let pics = folder_key(&loc(r"C:\Pics"));
    persisted.folders.put(folder_key(&loc(r"C:\Plain")), FolderView::default());
    persisted.folders.put(pics, FolderView { layout: LayoutMode::Columns, sizes: Sizes([2, 1, 1, 4, 5]), ..Default::default() });
    let s = Saved::new(persisted, None);
    let bytes = s.encode().unwrap();
    let back = Saved::decode(&bytes).unwrap().sanitized();
    assert_eq!(back.persisted.view, s.persisted.view);
    assert_eq!(back.persisted.prefs, s.persisted.prefs);
    assert_eq!(back.persisted.folders.entries(), s.persisted.folders.entries());
    let (_, tail) = postcard::take_from_bytes::<Head>(&bytes).unwrap();
    let (_, tail) = postcard::take_from_bytes::<NavState>(tail).unwrap();
    let old: ViewRecord = postcard::from_bytes(tail).unwrap();
    assert_eq!((old.view.layout, old.prefs.defaults.layout), (LayoutMode::Details, LayoutMode::Details));
    assert!(old.folders.iter().all(|(_, v)| v.layout == LayoutMode::Details));
    assert_eq!(old.view.columns, s.persisted.view.columns);
}

#[test]
fn out_of_range_sizes_and_cache_limits_fall_back() {
    let mut s = Saved::new(Persisted::default(), None);
    s.persisted.view.sizes = Sizes([7, 1, 9, 1, 40]);
    s.persisted.prefs.thumb_cache_mb = 1;
    let s = s.sanitized();
    assert_eq!(s.persisted.view.sizes, Sizes([Sizes::default().0[0], 1, Sizes::default().0[2], 1, Sizes::default().0[4]]));
    assert_eq!(s.persisted.prefs.thumb_cache_mb, flit_core::ui::THUMB_CACHE_MB.0);
}

#[test]
fn sidebar_and_home_preferences_ride_sixth_after_the_pane_so_older_files_still_load() {
    let side = NavPrefs {
        compact: true,
        collapsed: vec!["drives".into()],
        hidden: vec!["wsl".into()],
        widgets_off: vec!["recent".into()],
        recent_hidden: vec![(flit_core::FsPath::parse(r"C:\a.txt").unwrap(), 42)],
        recent_cleared: 7,
    };
    let pane = PaneState { visible: true, details: true, right_w: 410.0, bottom_h: 300.0, volume: 0.25 };
    let s = Saved::new(Persisted::default(), None).with_pane(pane).with_side(side.clone());
    let bytes = s.encode().unwrap();
    let back = Saved::decode(&bytes).unwrap().sanitized();
    assert_eq!((back.pane, &back.side), (pane, &side));
    let trailing = postcard::to_stdvec(&side).unwrap().len() + postcard::to_stdvec(&s.persisted.archive).unwrap().len() + postcard::to_stdvec(&s.tools).unwrap().len() + postcard::to_stdvec(&SplitPrefs::default()).unwrap().len() + WORD_LEN;
    let master_layout = &bytes[..bytes.len() - trailing];
    let (_, tail) = postcard::take_from_bytes::<Head>(master_layout).unwrap();
    let (_, tail) = postcard::take_from_bytes::<NavState>(tail).unwrap();
    let (_, tail) = postcard::take_from_bytes::<ViewRecord>(tail).unwrap();
    let (_, tail) = postcard::take_from_bytes::<LayoutRecord>(tail).unwrap();
    assert_eq!(postcard::from_bytes::<PaneState>(tail).unwrap(), pane, "the pane stays the fifth record");
    let older = Saved::decode(master_layout).unwrap();
    assert_eq!((older.pane, &older.side), (pane, &NavPrefs::default()));
    assert_eq!(older.persisted, Persisted { prefs: ViewPrefs { show_streams: false, ..s.persisted.prefs.clone() }, ..s.persisted.clone() });
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

#[test]
fn archive_defaults_ride_seventh_and_never_keep_a_password() {
    use flit_core::archive::{Compression, Defaults, Format, Secret};
    use flit_core::digest::HashKind;
    let mut persisted = Persisted::default();
    let compression = Compression { format: Format::SevenZ, level: 9, threads: 4, password: Some(Secret("x".into())), ..Default::default() };
    persisted.archive = Defaults { compression, history: vec![r"C:\out".into()], open_after: true, hashes: vec![HashKind::Sha512] };
    let s = Saved::new(persisted, None);
    let bytes = s.encode().unwrap();
    let back = Saved::decode(&bytes).unwrap();
    let want = Defaults { compression: Compression { password: None, ..s.persisted.archive.compression.clone() }, ..s.persisted.archive.clone() };
    assert_eq!(back.persisted.archive, want);
    assert_eq!(back.persisted.prefs, s.persisted.prefs);
    let stored = postcard::to_stdvec(&want).unwrap();
    let bytes = &bytes[..bytes.len() - postcard::to_stdvec(&Tools::default()).unwrap().len() - postcard::to_stdvec(&SplitPrefs::default()).unwrap().len() - WORD_LEN];
    assert!(bytes.ends_with(&stored));
    let without = &bytes[..bytes.len() - stored.len()];
    assert_eq!(Saved::decode(without).unwrap().persisted.archive, Defaults::default());
}

#[test]
fn pane_state_rides_after_layouts_and_older_files_keep_the_default() {
    let pane = PaneState { visible: true, details: true, right_w: 410.0, bottom_h: 300.0, volume: 0.25 };
    let s = Saved::new(Persisted::default(), None).with_pane(pane);
    let bytes = s.encode().unwrap();
    assert_eq!(Saved::decode(&bytes).unwrap().pane, pane);
    let older = Saved::new(Persisted::default(), None).encode().unwrap();
    let trailing = [postcard::to_stdvec(&PaneState::default()), postcard::to_stdvec(&NavPrefs::default()), postcard::to_stdvec(&Defaults::default()), postcard::to_stdvec(&Tools::default()), postcard::to_stdvec(&SplitPrefs::default())].map(|b| b.unwrap().len()).iter().sum::<usize>() + WORD_LEN;
    let without = &older[..older.len() - trailing];
    assert_eq!(Saved::decode(without).unwrap().pane, PaneState::default());
    let broken = Saved { pane: PaneState { right_w: -1.0, volume: f32::NAN, ..pane }, ..s }.sanitized();
    assert_eq!((broken.pane.right_w, broken.pane.volume), (PaneState::default().right_w, PaneState::default().volume));
}

#[test]
fn the_old_dual_pane_record_stays_ninth_and_is_ignored_so_later_records_still_load() {
    use flit_core::ui::{Show, ToolItem, ToolbarState};
    let mut toolbar = ToolbarState::default();
    toolbar.sets[0] = vec![ToolItem::Button("nav.refresh".into(), Show::Icon)];
    let tools = Tools { toolbar, searches: vec!["ext:txt".into()], streams: true };
    let mut persisted = Persisted::default();
    persisted.archive.open_after = true;
    persisted.prefs.show_streams = true;
    let nav = NavState { pinned: vec![loc(r"C:\work")], sidebar_w: 280.0, sidebar: true };
    let side = NavPrefs { compact: true, ..Default::default() };
    let pane = PaneState { visible: true, ..Default::default() };
    let s = Saved::new(persisted, None).with_nav(nav.clone()).with_pane(pane).with_side(side.clone()).with_tools(tools.clone());
    let bytes = s.encode().unwrap();
    let back = Saved::decode(&bytes).unwrap();
    assert_eq!((&back.persisted, &back.nav, back.pane, &back.side, &back.tools), (&s.persisted, &nav, pane, &side, &tools));
    let (head, word) = bytes.split_at(bytes.len() - WORD_LEN);
    let stored = postcard::to_stdvec(&SplitPrefs::default()).unwrap();
    assert!(head.ends_with(&stored));
    let master = &head[..head.len() - stored.len()];
    let mut dual = master.to_vec();
    dual.extend(postcard::to_stdvec(&SplitPrefs { dual: true, dir: SplitDir::Horizontal }).unwrap());
    dual.extend(word);
    assert_eq!(Saved::decode(&dual).unwrap().persisted, s.persisted, "a file with two panes per tab keeps every later record");
    let (_, tail) = postcard::take_from_bytes::<Head>(master).unwrap();
    let (_, tail) = postcard::take_from_bytes::<NavState>(tail).unwrap();
    let (_, tail) = postcard::take_from_bytes::<ViewRecord>(tail).unwrap();
    let (_, tail) = postcard::take_from_bytes::<LayoutRecord>(tail).unwrap();
    let (_, tail) = postcard::take_from_bytes::<PaneState>(tail).unwrap();
    let (_, tail) = postcard::take_from_bytes::<NavPrefs>(tail).unwrap();
    let (_, tail) = postcard::take_from_bytes::<Defaults>(tail).unwrap();
    let (_, tail) = postcard::take_from_bytes::<Tools>(tail).unwrap();
    assert!(tail.is_empty(), "master writes eight records");
    let older = Saved::decode(master).unwrap();
    assert_eq!((older.pane, &older.side, &older.tools), (pane, &side, &tools));
}

#[test]
fn toolbar_and_search_history_ride_eighth_so_files_from_master_still_load() {
    use flit_core::ui::{Show, ToolItem, ToolbarState, MAX_HISTORY};
    let mut toolbar = ToolbarState { visible: false, ..Default::default() };
    toolbar.sets[0] = vec![ToolItem::Button("nav.refresh".into(), Show::Both), ToolItem::Separator];
    let tools = Tools { toolbar, searches: vec!["report".into(), "ext:pdf".into()], streams: true };
    let mut persisted = Persisted::default();
    persisted.archive.open_after = true;
    persisted.prefs.show_streams = true;
    let s = Saved::new(persisted, None).with_tools(tools.clone());
    let bytes = s.encode().unwrap();
    let back = Saved::decode(&bytes).unwrap();
    assert_eq!((&back.tools, &back.persisted.archive, back.persisted.prefs.show_streams), (&tools, &s.persisted.archive, true));
    let bytes = &bytes[..bytes.len() - postcard::to_stdvec(&SplitPrefs::default()).unwrap().len() - WORD_LEN];
    let stored = postcard::to_stdvec(&tools).unwrap();
    assert!(bytes.ends_with(&stored));
    let master = &bytes[..bytes.len() - stored.len()];
    let older = Saved::decode(master).unwrap();
    assert_eq!(older.tools, Tools::default());
    assert!(older.tools.toolbar.visible);
    assert_eq!(older.persisted, Persisted { prefs: ViewPrefs { show_streams: false, ..s.persisted.prefs.clone() }, ..s.persisted.clone() });
    let mut junk = tools;
    junk.toolbar.sets[1].push(ToolItem::Button("gone.button".into(), Show::Icon));
    junk.searches = (0..100).map(|k| format!("q{k}")).collect();
    let fixed = Saved { tools: junk, ..s }.sanitized();
    assert_eq!(fixed.tools.searches.len(), MAX_HISTORY);
    assert!(!fixed.tools.toolbar.sets[1].contains(&ToolItem::Button("gone.button".into(), Show::Icon)));
}

#[test]
fn seven_z_word_size_rides_tenth_and_older_files_keep_automatic() {
    let mut persisted = Persisted::default();
    persisted.archive.compression.word = 64;
    let s = Saved::new(persisted, None);
    let bytes = s.encode().unwrap();
    assert_eq!(Saved::decode(&bytes).unwrap().persisted.archive.compression.word, 64);
    let stored = postcard::to_stdvec(&64u32).unwrap();
    assert!(bytes[..bytes.len() - GIT_LEN].ends_with(&stored));
    let older = &bytes[..bytes.len() - GIT_LEN - stored.len()];
    assert_eq!(Saved::decode(older).unwrap().persisted.archive.compression.word, 0);
    let mut junk = older.to_vec();
    junk.extend(postcard::to_stdvec(&65u32).unwrap());
    assert_eq!(Saved::decode(&junk).unwrap().persisted.archive.compression.word, 0);
}

#[test]
fn state_files_round_trip_and_newer_or_broken_ones_are_set_aside() {
    let d = crate::schema::tests::Dir::new("state");
    let f = d.0.join("state.bin");
    let s = Saved::new(Persisted { path: loc(r"D:\kept"), show_hidden: true, ..Default::default() }, None);
    let bytes = s.encode().unwrap();
    std::fs::write(&f, &bytes).unwrap();
    let back = load_from(&f).unwrap();
    assert_eq!((back.persisted.path, back.persisted.show_hidden), (loc(r"D:\kept"), true));
    let mut newer = s.clone();
    newer.version = VERSION + 1;
    let newer = newer.encode().unwrap();
    std::fs::write(&f, &newer).unwrap();
    assert!(load_from(&f).is_none());
    assert_eq!(std::fs::read(crate::schema::aside(&f, crate::schema::NEWER)).unwrap(), newer);
    std::fs::write(&f, [VERSION as u8, 0xFF]).unwrap();
    assert!(load_from(&f).is_none());
    assert!(crate::schema::aside(&f, crate::schema::BAD).exists());
}
