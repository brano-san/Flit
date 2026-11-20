use super::*;
use flit_core::sort::{GroupField, GroupSpec, SortSpec};
use flit_core::ui::{folder_key, Columns};

const GIT_LEN: usize = 35;
const DESCRIPTIONS_LEN: usize = 1;
const QUICK_LEN: usize = 4;
const AFTER_WORD: usize = GIT_LEN + DESCRIPTIONS_LEN + QUICK_LEN;
const WORD_LEN: usize = 1 + AFTER_WORD;

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
    let nav = NavState { pinned: vec![loc(r"C:\work"), loc("wsl://Ubuntu/home")], sidebar_w: 300.0, sidebar: false, files: Vec::new() };
    let placement = Placement { x: 10, y: 20, width: 800, height: 600, maximized: true };
    let mut persisted = Persisted { path: loc(r"D:\x"), show_hidden: true, ..Default::default() };
    persisted.view.sort = SortSpec { field: SortField::Size, descending: true, ..Default::default() };
    persisted.view.columns.widths[Column::Type as usize] = 77.0;
    let s = Saved::new(persisted, Some(placement)).with_nav(nav.clone());
    let bytes = older_encode(&s);
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
    persisted.view.zoom = 30;
    persisted.view.sizes = Sizes([0, 4, 3, 2, 11]);
    persisted.prefs.defaults.layout = LayoutMode::Adaptive;
    persisted.prefs.thumbnails = false;
    persisted.prefs.thumb_cache_mb = 1200;
    let pics = folder_key(&loc(r"C:\Pics"));
    persisted.folders.put(folder_key(&loc(r"C:\Plain")), FolderView::default());
    persisted.folders.put(pics, FolderView { layout: LayoutMode::Columns, sizes: Sizes([2, 1, 1, 4, 5]), ..Default::default() });
    let s = Saved::new(persisted, None);
    let bytes = older_encode(&s);
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
        quick: QuickPrefs::default(),
    };
    let pane = PaneState { visible: true, details: true, right_w: 410.0, bottom_h: 300.0, volume: 0.25 };
    let s = Saved::new(Persisted::default(), None).with_pane(pane).with_side(side.clone());
    let bytes = older_encode(&s);
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
    s.nav = NavState { pinned: vec![loc(r"C:\a"), loc(r"C:\b"), loc(r"C:\a")], sidebar_w: f32::INFINITY, sidebar: true, files: Vec::new() };
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
    let bytes = older_encode(&s);
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
    let bytes = older_encode(&s);
    assert_eq!(Saved::decode(&bytes).unwrap().pane, pane);
    let older = older_encode(&Saved::new(Persisted::default(), None));
    let trailing = [postcard::to_stdvec(&PaneState::default()), postcard::to_stdvec(&NavPrefs::default()), postcard::to_stdvec(&Defaults::default()), postcard::to_stdvec(&Tools::default()), postcard::to_stdvec(&SplitPrefs::default())].map(|b| b.unwrap().len()).iter().sum::<usize>() + WORD_LEN;
    let without = &older[..older.len() - trailing];
    assert_eq!(Saved::decode(without).unwrap().pane, PaneState::default());
    let broken = Saved { pane: PaneState { right_w: -1.0, volume: f32::NAN, ..pane }, ..s }.sanitized();
    assert_eq!((broken.pane.right_w, broken.pane.volume), (PaneState::default().right_w, PaneState::default().volume));
}

#[test]
fn the_old_dual_pane_record_stays_ninth_and_is_ignored_so_later_records_still_load() {
    let tools = Tools { streams: true, ..Tools::new(vec!["ext:txt".into()]) };
    let mut persisted = Persisted::default();
    persisted.archive.open_after = true;
    persisted.prefs.show_streams = true;
    let nav = NavState { pinned: vec![loc(r"C:\work")], sidebar_w: 280.0, sidebar: true, files: Vec::new() };
    let side = NavPrefs { compact: true, ..Default::default() };
    let pane = PaneState { visible: true, ..Default::default() };
    let s = Saved::new(persisted, None).with_nav(nav.clone()).with_pane(pane).with_side(side.clone()).with_tools(tools.clone());
    let bytes = older_encode(&s);
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
fn the_retired_toolbar_record_still_rides_eighth_so_old_files_keep_their_search_history() {
    use flit_core::ui::MAX_HISTORY;
    let toolbar = RetiredToolbar { visible: false, sets: [vec![RetiredTool::Button("nav.refresh".into(), RetiredShow::Both), RetiredTool::Separator], vec![RetiredTool::Button("bin.empty".into(), RetiredShow::Label)], Vec::new()] };
    let old = Tools { toolbar, searches: vec!["report".into(), "ext:pdf".into()], streams: true };
    let mut persisted = Persisted::default();
    persisted.archive.open_after = true;
    persisted.prefs.show_streams = true;
    let s = Saved::new(persisted, None).with_tools(old.clone());
    let bytes = older_encode(&s);
    let back = Saved::decode(&bytes).unwrap();
    let kept = Tools { streams: true, ..Tools::new(old.searches.clone()) };
    assert_eq!((&back.tools, &back.persisted.archive, back.persisted.prefs.show_streams), (&kept, &s.persisted.archive, true), "the old toolbar is read and dropped");
    assert_eq!(Saved::decode(&back.encode().unwrap()).unwrap().tools, kept);
    let bytes = &bytes[..bytes.len() - postcard::to_stdvec(&SplitPrefs::default()).unwrap().len() - WORD_LEN];
    let stored = postcard::to_stdvec(&old).unwrap();
    assert!(bytes.ends_with(&stored));
    let master = &bytes[..bytes.len() - stored.len()];
    let older = Saved::decode(master).unwrap();
    assert_eq!(older.tools, Tools::default());
    assert_eq!(older.persisted, Persisted { prefs: ViewPrefs { show_streams: false, ..s.persisted.prefs.clone() }, ..s.persisted.clone() });
    let junk = Tools { searches: (0..100).map(|k| format!("q{k}")).collect(), ..old };
    let fixed = Saved { tools: junk, ..s }.sanitized();
    assert_eq!(fixed.tools.searches.len(), MAX_HISTORY);
}

#[test]
fn seven_z_word_size_rides_tenth_and_older_files_keep_automatic() {
    let mut persisted = Persisted::default();
    persisted.archive.compression.word = 64;
    let s = Saved::new(persisted, None);
    let bytes = older_encode(&s);
    assert_eq!(Saved::decode(&bytes).unwrap().persisted.archive.compression.word, 64);
    let stored = postcard::to_stdvec(&64u32).unwrap();
    assert!(bytes[..bytes.len() - AFTER_WORD].ends_with(&stored));
    let older = &bytes[..bytes.len() - AFTER_WORD - stored.len()];
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
    let bytes = older_encode(&s);
    std::fs::write(&f, &bytes).unwrap();
    let back = load_from(&f).unwrap();
    assert_eq!((back.persisted.path, back.persisted.show_hidden), (loc(r"D:\kept"), true));
    let mut newer = s.clone();
    newer.version = VERSION + 1;
    let newer = older_encode(&newer);
    std::fs::write(&f, &newer).unwrap();
    assert!(load_from(&f).is_none());
    assert_eq!(std::fs::read(crate::schema::aside(&f, crate::schema::NEWER)).unwrap(), newer);
    std::fs::write(&f, [VERSION as u8, 0xFF]).unwrap();
    assert!(load_from(&f).is_none());
    assert!(crate::schema::aside(&f, crate::schema::BAD).exists());
}

#[test]
fn the_palette_description_switch_rides_last_and_older_files_show_descriptions() {
    let mut persisted = Persisted::default();
    persisted.prefs.command_descriptions = false;
    let bytes = older_encode(&Saved::new(persisted, None));
    assert_eq!(bytes[bytes.len() - QUICK_LEN - 1], 0);
    assert!(!Saved::decode(&bytes).unwrap().persisted.prefs.command_descriptions);
    let older = &bytes[..bytes.len() - QUICK_LEN - DESCRIPTIONS_LEN];
    assert!(Saved::decode(older).unwrap().persisted.prefs.command_descriptions);
}

#[test]
fn quick_access_order_paths_visits_and_deep_views_ride_last() {
    let persisted = Persisted { path: loc(r"D:\x"), ..Default::default() };
    let deep = folder_key(&loc(r"D:\x"));
    persisted.folders.put(deep, FolderView::default());
    persisted.folders.set_deep(vec![deep, 42]);
    let side = NavPrefs { quick: QuickPrefs { order: vec!["drives".into(), "pinned".into()], show_paths: true, visits: vec![(loc(r"D:\x"), 7)], ..Default::default() }, ..Default::default() };
    let bytes = older_encode(&Saved::new(persisted, None).with_side(side.clone()));
    let back = Saved::decode(&bytes).unwrap();
    assert_eq!(back.side.quick, side.quick);
    assert_eq!(back.persisted.folders.deep_keys(), [deep], "keys without a saved view are dropped");
    let record = postcard::to_stdvec(&QuickRecord { quick: side.quick.clone(), deep: vec![deep] }).unwrap().len();
    let older = Saved::decode(&bytes[..bytes.len() - record]).unwrap();
    assert_eq!(older.side.quick, QuickPrefs::default(), "older files start with the default order");
}

fn older_encode(s: &Saved) -> Vec<u8> {
    let bytes = s.encode().unwrap();
    let p = &s.persisted;
    let folders = p.folders.entries();
    let z = ZoomRecord { view: zoomed(&p.view), defaults: zoomed(&p.prefs.defaults), folders: folders.iter().map(|(_, v)| zoomed(v)).collect(), retain_filter: p.prefs.retain_filter };
    let tail = postcard::to_stdvec(&z).unwrap().len() + postcard::to_stdvec(&s.nav.files).unwrap().len() + wide_len(s);
    bytes[..bytes.len() - tail].to_vec()
}

fn wide_len(s: &Saved) -> usize {
    let p = &s.persisted;
    let folders = p.folders.entries();
    postcard::to_stdvec(&WideRecord { view: wide(&p.view), defaults: wide(&p.prefs.defaults), folders: folders.iter().map(|(_, v)| wide(v)).collect() }).unwrap().len() + 1
        + postcard::to_stdvec(&IntegrationRecord { git: p.prefs.git_integration, places: s.side.quick.places.clone(), places_hidden: s.side.quick.places_hidden.clone() }).unwrap().len()
        + 2
        + attributes_len(s)
}

fn attributes_len(s: &Saved) -> usize {
    let p = &s.persisted;
    let folders = p.folders.entries();
    postcard::to_stdvec(&AttributesRecord { view: attributed(&p.view), defaults: attributed(&p.prefs.defaults), folders: folders.iter().map(|(_, v)| attributed(v)).collect() }).unwrap().len() + newer_len(s)
}

fn newer_len(s: &Saved) -> usize {
    shells_len(s) + hash_record_len(s)
}

fn hash_record_len(s: &Saved) -> usize {
    let p = &s.persisted;
    postcard::to_stdvec(&GitHashRecord { view: hash_width(&p.view), defaults: hash_width(&p.prefs.defaults), folders: p.folders.entries().iter().map(|(k, v)| (*k, hash_width(v))).collect() }).unwrap().len()
}

fn shells_len(s: &Saved) -> usize {
    let p = &s.persisted;
    let folders = p.folders.entries();
    postcard::to_stdvec(&ShellRecord { view: shelled(&p.view), defaults: shelled(&p.prefs.defaults), folders: folders.iter().map(|(_, v)| shelled(v)).collect() }).unwrap().len()
}

fn classic() -> Saved {
    let mut p = Persisted { path: loc(r"D:\media"), ..Default::default() };
    p.view.sort = SortSpec { field: SortField::Size, descending: true, ..Default::default() };
    p.view.group = GroupSpec { field: GroupField::Type, ..Default::default() };
    p.view.columns.toggle(Column::Attributes);
    p.view.columns.toggle(Column::ImageSize);
    p.view.columns.set_width(Column::Version, 123.0);
    p.view.columns.shift(Column::Size, &[Column::Name, Column::Type, Column::Modified, Column::Size], false);
    p.prefs.defaults.sort.field = SortField::Modified;
    p.prefs.defaults.columns.toggle(Column::Duration);
    p.folders.put(folder_key(&loc(r"D:\media\x")), FolderView { group: GroupSpec { field: GroupField::Size, ..Default::default() }, ..Default::default() });
    Saved::new(p, None)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|k| u8::from_str_radix(&s[k..k + 2], 16).unwrap()).collect()
}

const CLASSIC_FROM_0_7: &str = "0208443a5c6d656469610301000000164300000c430000b442000000005c43010003010002000140000000000000164300000c430000b44200005c430000164300001643000001000000000140000000000000164300000c430000b44200005c4300001643000016430001000100010001000187b7ff829ebdb8e3cf010000000003000140000000000000164300000c430000b44200005c4300001643000016430001010101050001010101050100010101010501f40300000000000000000000cdcc4c3f0000000000000006000000000000000400010203000000000000000000000000c8420000824300000c4300001643000000c8420000824300000c430000164301000000c8420000824300000c430000164301000000000000f10000b442000016430000dc420000dc420000dc420000b4420000dc42000405020308010a06090b070c0e0d0000f10000b442000016430000dc420000dc420000dc420000b4420000dc42000405020108030a06090b070c0e0d010000f10000b442000016430000dc420000dc420000dc420000b4420000dc42000405020108030a06090b070c0e0d00000000b442000016430000dc420000dc420000dc420000b4420000dc420000b4420004050203080f010a06090b070c0e0d0000b442000016430000dc420000dc420000dc420000b4420000dc420000b4420004050201080f030a06090b070c0e0d010000b442000016430000dc420000dc420000dc420000b4420000dc420000b4420004050201080f030a06090b070c0e0d010000000000f1070000b442000016430000dc420000dc420000dc420000b4420000dc420000b4420000b4420000dc420000c842000020430000f6420004050203080f010a06090b070c0e0d1011121314f1090000b442000016430000dc420000dc420000dc420000b4420000dc420000b4420000b4420000dc420000c842000020430000dc420004050201080f030a06090b070c0e0d101112131401f1010000b442000016430000dc420000dc420000dc420000b4420000dc420000b4420000b4420000dc420000c842000020430000dc420004050201080f030a06090b070c0e0d1011121314";

#[test]
fn a_state_without_the_new_columns_writes_every_older_record_byte_for_byte_as_before() {
    let s = classic();
    let bytes = s.encode().unwrap();
    let old = unhex(CLASSIC_FROM_0_7);
    assert_eq!(hex(&bytes[..bytes.len() - newer_len(&s)]), hex(&old), "the records older builds read are unchanged");
    let back = Saved::decode(&old).unwrap().sanitized().persisted;
    let want = s.persisted.clone();
    assert_eq!((back.view.sort, back.view.group, back.view.columns), (want.view.sort, want.view.group, want.view.columns), "a file from the older build opens with its settings");
    assert_eq!(back.prefs.defaults.columns, want.prefs.defaults.columns);
    assert_eq!(back.folders.entries(), want.folders.entries());
}

#[test]
fn shell_columns_sorting_and_grouping_survive_a_restart_and_older_builds_read_name_order_without_groups() {
    let mut p = Persisted { path: loc(r"D:\media"), ..Default::default() };
    let kinds = [Column::Name, Column::Type, Column::Size, Column::Company, Column::Duration];
    p.view.columns.toggle(Column::Company);
    p.view.columns.toggle(Column::Duration);
    p.view.columns.set_width(Column::Company, 222.0);
    p.view.columns.shift(Column::Company, &kinds, false);
    p.view.columns.shift(Column::Company, &kinds, false);
    p.view.sort = SortSpec { field: SortField::Duration, descending: true, ..Default::default() };
    p.view.group = GroupSpec { field: GroupField::Artist, ..Default::default() };
    p.prefs.defaults.sort.field = SortField::Version;
    p.prefs.defaults.group.field = GroupField::Dimensions;
    let key = folder_key(&loc(r"D:\media\clips"));
    p.folders.put(key, FolderView { sort: SortSpec { field: SortField::Artist, ..Default::default() }, group: GroupSpec { field: GroupField::Duration, ..Default::default() }, ..Default::default() });
    let moved = p.view.columns.ordered(&kinds);
    assert_eq!(moved[2], Column::Company);
    let s = Saved::new(p, None);
    let bytes = s.encode().unwrap();
    let back = Saved::decode(&bytes).unwrap().sanitized().persisted;
    assert_eq!((back.view.sort.field, back.view.sort.descending, back.view.group.field), (SortField::Duration, true, GroupField::Artist));
    assert!(back.view.columns.shown(Column::Company) && back.view.columns.shown(Column::Duration));
    assert_eq!((back.view.columns.width(Column::Company), back.view.columns.ordered(&kinds)), (222.0, moved));
    assert_eq!((back.prefs.defaults.sort.field, back.prefs.defaults.group.field), (SortField::Version, GroupField::Dimensions));
    assert_eq!(back.folders.get(key).map(|f| (f.sort.field, f.group.field)), Some((SortField::Artist, GroupField::Duration)));
    let older = Saved::decode(&bytes[..bytes.len() - newer_len(&s)]).unwrap().sanitized().persisted;
    assert_eq!((older.view.sort.field, older.view.sort.descending, older.view.group.field), (SortField::Name, true, GroupField::None), "builds without these sorts fall back to names");
    assert_eq!((older.prefs.defaults.sort.field, older.prefs.defaults.group.field), (SortField::Name, GroupField::None));
    assert_eq!(older.folders.get(key).map(|f| (f.sort.field, f.group.field)), Some((SortField::Name, GroupField::None)));
    assert!(older.view.columns.shown(Column::Duration), "the four older shell columns keep their switch");
    assert!(!older.view.columns.shown(Column::Company));
    let (_, wide_order) = wide(&s.persisted.view);
    let (_, _, _, _, zoom_order) = zoomed(&s.persisted.view);
    let (extra, _, order) = attributed(&s.persisted.view);
    assert!(wide_order.iter().chain(&zoom_order).chain(&order).all(|&id| id < 21), "older builds never see an id they do not know");
    let mut ids = order.to_vec();
    ids.sort_unstable();
    assert_eq!(ids, (0..21).collect::<Vec<u8>>(), "older builds get a whole column order and keep it");
    assert!(extra < 1 << 13);
    let legacy_views = postcard::to_stdvec(&legacy(s.persisted.view)).unwrap();
    assert_eq!(postcard::from_bytes::<FolderView>(&legacy_views).ok().map(|v| (v.sort.field, v.group.field)), Some((SortField::Name, GroupField::None)));
}

#[test]
fn a_cut_or_unknown_shell_record_keeps_every_older_setting() {
    let mut s = classic();
    s.persisted.view.sort.field = SortField::Duration;
    s.persisted.view.columns.toggle(Column::Title);
    let bytes = s.encode().unwrap();
    let cut = Saved::decode(&bytes[..bytes.len() - hash_record_len(&s) - 3]).unwrap().sanitized().persisted;
    assert!(cut.view.columns.shown(Column::Attributes) && cut.view.columns.shown(Column::ImageSize));
    assert_eq!(cut.view.columns.width(Column::Version), 123.0);
    assert_eq!((cut.view.sort.field, cut.view.group.field), (SortField::Name, GroupField::Type));
    let mut future = bytes[..bytes.len() - newer_len(&s)].to_vec();
    let mut record = shelled(&s.persisted.view);
    (record.3, record.4) = (200, 200);
    record.0 |= 1 << 30;
    record.1.push(77.0);
    record.2.push(99);
    future.extend(postcard::to_stdvec(&ShellRecord { view: record.clone(), defaults: record, folders: Vec::new() }).unwrap());
    let read = Saved::decode(&future).unwrap().sanitized().persisted;
    assert_eq!((read.view.sort.field, read.view.group.field), (SortField::Name, GroupField::Type), "sorts this build does not know fall back");
    assert!(read.view.columns.shown(Column::Title) && read.view.columns.shown(Column::Attributes));
    assert_eq!(read.view.columns.width(Column::Version), 123.0);
}

#[test]
fn commit_hash_columns_keep_their_visibility_width_and_sort_without_changing_old_records() {
    let mut persisted = Persisted { path: loc(r"D:\x"), ..Default::default() };
    persisted.view.columns.toggle(Column::GitHash);
    persisted.view.columns.set_width(Column::GitHash, 190.0);
    persisted.view.sort.field = SortField::GitHash;
    persisted.prefs.defaults.columns.set_width(Column::GitHash, 180.0);
    let key = folder_key(&loc(r"D:\other"));
    let mut folder = FolderView::default();
    folder.columns.toggle(Column::GitHash);
    folder.columns.set_width(Column::GitHash, 170.0);
    persisted.folders.put(key, folder);
    let saved = Saved::new(persisted, None);
    let bytes = saved.encode().unwrap();
    let back = Saved::decode(&bytes).unwrap();
    assert!(back.persisted.view.columns.shown(Column::GitHash));
    assert_eq!(back.persisted.view.columns.width(Column::GitHash), 190.0);
    assert_eq!(back.persisted.view.sort.field, SortField::GitHash);
    assert_eq!(back.persisted.prefs.defaults.columns.width(Column::GitHash), 180.0);
    assert!(!back.persisted.prefs.defaults.columns.shown(Column::GitHash));
    assert_eq!(back.persisted.folders.get(key).unwrap().columns.width(Column::GitHash), 170.0);
    let old = Saved::new(Persisted::default(), None);
    let old_bytes = old.encode().unwrap();
    let old_back = Saved::decode(&old_bytes[..old_bytes.len() - hash_record_len(&old)]).unwrap();
    assert!(!old_back.persisted.view.columns.shown(Column::GitHash));
    assert_eq!(old_back.persisted.view.columns.width(Column::GitHash), 130.0);
    let older = Saved::decode(&bytes[..bytes.len() - newer_len(&saved)]).unwrap();
    assert_eq!(older.persisted.view.sort.field, SortField::Name, "builds without the hash sort read name order instead of failing");
}

#[test]
fn the_attributes_column_keeps_its_place_width_and_switch_and_older_files_hide_it() {
    let mut persisted = Persisted { path: loc(r"D:\x"), ..Default::default() };
    let kinds = [Column::Name, Column::Type, Column::Modified, Column::Size, Column::Attributes];
    assert!(!persisted.view.columns.shown(Column::Attributes), "Explorer hides Attributes by default");
    persisted.view.columns.toggle(Column::Attributes);
    persisted.view.columns.set_width(Column::Attributes, 64.0);
    persisted.view.columns.shift(Column::Attributes, &kinds, false);
    persisted.view.columns.shift(Column::Attributes, &kinds, false);
    let moved = persisted.view.columns.ordered(&kinds);
    assert_eq!(moved[2], Column::Attributes);
    let s = Saved::new(persisted, None);
    let bytes = s.encode().unwrap();
    let v = Saved::decode(&bytes).unwrap().sanitized().persisted.view;
    assert!(v.columns.shown(Column::Attributes));
    assert_eq!((v.columns.width(Column::Attributes), v.columns.ordered(&kinds)), (64.0, moved));
    let older = Saved::decode(&bytes[..bytes.len() - attributes_len(&s)]).unwrap().sanitized().persisted.view;
    assert!(!older.columns.shown(Column::Attributes));
    assert_eq!(older.columns.ordered(&kinds).last(), Some(&Column::Attributes), "{:?}", older.columns.ordered(&kinds));
    let (_, wide_order) = wide(&s.persisted.view);
    assert!(!wide_order.contains(&(Column::Attributes as u8)), "older builds never see an id they do not know");
}

#[test]
fn folder_statistics_are_independent_and_migrate_the_old_switch() {
    for mask in 0..8 {
        let mut saved = Saved::new(Persisted::default(), None);
        let prefs = &mut saved.persisted.prefs;
        prefs.auto_sizes = mask & 1 != 0;
        prefs.count_folders = mask & 2 != 0;
        prefs.count_files = mask & 4 != 0;
        let bytes = saved.encode().unwrap();
        let p = Saved::decode(&bytes).unwrap().persisted.prefs;
        assert_eq!((p.auto_sizes, p.count_folders, p.count_files), (mask & 1 != 0, mask & 2 != 0, mask & 4 != 0));
        let legacy = Saved::decode(&bytes[..bytes.len() - 2 - attributes_len(&saved)]).unwrap().persisted.prefs;
        assert_eq!((legacy.auto_sizes, legacy.count_folders, legacy.count_files), (mask & 1 != 0, mask & 1 != 0, mask & 1 != 0));
    }
}

#[test]
fn integration_and_places_roundtrip_without_changing_older_records() {
    let mut saved = Saved::new(Persisted { path: loc(r"D:\x"), ..Default::default() }, None);
    saved.persisted.prefs.git_integration = true;
    saved.side.quick.places = vec![loc(r"C:\Users\u\Downloads"), loc(r"C:\Users\u")];
    saved.side.quick.places_hidden = vec![Location::computer()];
    saved.side.quick.show_paths = true;
    let bytes = saved.encode().unwrap();
    let restored = Saved::decode(&bytes).unwrap();
    assert!(restored.persisted.prefs.git_integration);
    assert_eq!(restored.side.quick, saved.side.quick);
    let len = postcard::to_stdvec(&IntegrationRecord { git: true, places: saved.side.quick.places.clone(), places_hidden: saved.side.quick.places_hidden.clone() }).unwrap().len();
    let older = Saved::decode(&bytes[..bytes.len() - len - 2 - attributes_len(&saved)]).unwrap();
    assert!(!older.persisted.prefs.git_integration);
    assert!(older.side.quick.places.is_empty());
    assert!(older.side.quick.show_paths);
}

#[test]
fn bookmarked_files_ride_last_and_older_files_have_none() {
    let nav = NavState { pinned: vec![loc(r"C:\work"), loc(r"C:\work\notes.txt")], files: vec![loc(r"C:\work\notes.txt")], ..NavState::default() };
    let s = Saved::new(Persisted { path: loc(r"D:\x"), ..Default::default() }, None).with_nav(nav.clone());
    let bytes = s.encode().unwrap();
    assert_eq!(Saved::decode(&bytes).unwrap().nav, nav);
    let older = Saved::decode(&bytes[..bytes.len() - wide_len(&s) - postcard::to_stdvec(&nav.files).unwrap().len()]).unwrap();
    assert_eq!((older.nav.pinned, older.nav.files), (nav.pinned, Vec::new()));
}

#[test]
fn zoom_subfolders_and_new_columns_ride_last_and_older_files_keep_defaults() {
    let mut persisted = Persisted { path: loc(r"D:\x"), ..Default::default() };
    persisted.view.layout = LayoutMode::Grid;
    persisted.view.zoom = 75;
    persisted.view.subfolders = true;
    persisted.view.columns.toggle(Column::Accessed);
    persisted.view.columns.set_width(Column::Folders, 200.0);
    persisted.prefs.retain_filter = true;
    let key = folder_key(&loc(r"D:\x"));
    persisted.folders.put(key, FolderView { layout: LayoutMode::Tree, ..Default::default() });
    let s = Saved::new(persisted, None);
    let back = Saved::decode(&s.encode().unwrap()).unwrap().sanitized();
    let v = back.persisted.view;
    assert_eq!((v.layout, v.zoom, v.subfolders), (LayoutMode::Grid, 75, true));
    assert!(v.columns.shown(Column::Accessed));
    assert_eq!(v.columns.width(Column::Folders), 200.0);
    assert!(back.persisted.prefs.retain_filter);
    assert_eq!(back.persisted.folders.get(key).map(|f| (f.layout, f.zoom)), Some((LayoutMode::Tree, 10)));
    let older = Saved::decode(&older_encode(&s)).unwrap().sanitized();
    let o = older.persisted.view;
    assert_eq!((o.layout, o.zoom, o.subfolders), (LayoutMode::Grid, 30, false), "older files open icons at the medium size");
    assert!(!o.columns.shown(Column::Accessed) && !older.persisted.prefs.retain_filter);
}

#[test]
fn the_folders_and_files_columns_keep_their_widths_and_places_and_older_files_put_files_after_folders() {
    let mut persisted = Persisted { path: loc(r"D:\x"), ..Default::default() };
    persisted.view.columns.set_width(Column::Files, 133.0);
    persisted.view.columns.shift(Column::Files, &flit_core::ui::COLUMNS, false);
    persisted.view.columns.shift(Column::Files, &flit_core::ui::COLUMNS, false);
    let moved = persisted.view.columns.ordered(&flit_core::ui::COLUMNS);
    let s = Saved::new(persisted, None);
    let bytes = s.encode().unwrap();
    let v = Saved::decode(&bytes).unwrap().sanitized().persisted.view;
    assert_eq!(v.columns.width(Column::Files), 133.0);
    assert_eq!(v.columns.ordered(&flit_core::ui::COLUMNS), moved);
    let older = Saved::decode(&bytes[..bytes.len() - wide_len(&s)]).unwrap().sanitized().persisted.view;
    let order = older.columns.ordered(&flit_core::ui::COLUMNS);
    let k = order.iter().position(|&c| c == Column::Folders).unwrap();
    assert_eq!(order[k + 1], Column::Files, "{order:?}");
    assert!(older.columns.shown(Column::Files));
}

#[test]
fn the_files_column_shows_by_default_while_files_saved_from_now_on_keep_their_choice() {
    assert!(flit_core::ui::FolderView::default().columns.shown(Column::Files), "a fresh view shows Files next to Folders");
    let mut persisted = Persisted { path: loc(r"D:\x"), ..Default::default() };
    persisted.view.columns.toggle(Column::Files);
    persisted.prefs.defaults.columns.toggle(Column::Files);
    let key = folder_key(&loc(r"D:\x\y"));
    let mut hidden_both = FolderView::default();
    hidden_both.columns.toggle(Column::Files);
    hidden_both.columns.toggle(Column::Folders);
    persisted.folders.put(key, hidden_both);
    let s = Saved::new(persisted, None);
    let bytes = s.encode().unwrap();
    let chosen = Saved::decode(&bytes).unwrap().sanitized().persisted;
    assert!(!chosen.view.columns.shown(Column::Files), "a choice saved by this build stays");
    let added = postcard::to_stdvec(&IntegrationRecord { git: false, places: Vec::new(), places_hidden: Vec::new() }).unwrap().len();
    let before = Saved::decode(&bytes[..bytes.len() - added - 3 - attributes_len(&s)]).unwrap().sanitized().persisted;
    assert!(before.view.columns.shown(Column::Files) && before.prefs.defaults.columns.shown(Column::Files), "a layout from before the split shows Files with Folders");
    let folder = before.folders.get(key).unwrap();
    assert!(!folder.columns.shown(Column::Folders) && !folder.columns.shown(Column::Files), "a folder that hid the counts keeps them hidden");
}

#[test]
fn saved_network_locations_use_home_on_linux_and_keep_the_shell_path_on_windows() {
    let network = flit_core::FsPath::network();
    let server = flit_core::FsPath::network_server(b"NAS").unwrap();
    let home = Location::from_local(crate::P::home());
    for path in [network, server] {
        let path = Location::from_local(path);
        let mut side = NavPrefs::default();
        let local = loc("/tmp");
        side.quick.visits = vec![(path.clone(), 1), (local.clone(), 2)];
        let state = Saved::new(Persisted { path: path.clone(), show_hidden: true, ..Default::default() }, None).with_side(side);
        let reopened = Saved::decode(&state.encode().unwrap()).unwrap();
        let expected = if cfg!(target_os = "linux") { &home } else { &path };
        assert_eq!(&reopened.persisted.path, expected);
        assert!(reopened.persisted.show_hidden);
        let visits = if cfg!(target_os = "linux") { vec![(local, 2)] } else { vec![(path, 1), (local, 2)] };
        assert_eq!(reopened.side.quick.visits, visits);
    }
}
