use super::*;
use crate::input::{Event, Mods};
use crate::listing::{DirSnapshot, Meta};
use crate::provider::ProviderRequest;
use crate::sort::{CaseMode, SortKeys};
use crate::text::tests::Mono;
use crate::ui::{Persisted, SearchKey, SearchRequest};

fn loc(s: &str) -> Location {
    Location::parse(s).unwrap()
}

fn app_at(path: &str) -> App {
    App::new(Persisted { path: loc(path), ..Default::default() }, Vec::new(), 0, false, 1.0, (1000, 600), Instant::now())
}

fn frame(app: &mut App) {
    app.frame(&mut Gfx::new(Mono, 1024), Instant::now());
}

fn deliver_with(app: &mut App, items: &[(&str, u64, bool)], error: Option<FsError>) -> Vec<Effect> {
    let t = app.tab();
    let mut listing = Listing::default();
    let mut keys = SortKeys::new(CaseMode::Insensitive);
    for &(n, size, dir) in items {
        let i = listing.push(n.as_bytes(), Meta { size, attrs: if dir { crate::listing::attr::DIR } else { 0 }, ..Default::default() });
        keys.push(listing.name(i));
    }
    let view = t.view_spec;
    let mut order = Vec::new();
    sort::sort(&listing, &keys, view, &mut order);
    let snap = DirSnapshot { gen: t.gen, path: t.location().path().clone(), listing, keys, order, view, done: true, error, elapsed: Duration::ZERO };
    t.slot.publish(Arc::new(snap));
    app.poll();
    frame(app);
    app.drain_effects().collect()
}

fn deliver(app: &mut App, items: &[(&str, u64, bool)]) {
    deliver_with(app, items, None);
}

fn resort_now(app: &mut App) {
    let job = app.drain_effects().filter_map(|e| if let Effect::Resort { tab, gen, view, snap } = e { Some((tab, gen, view, snap)) } else { None }).next_back();
    let (tab, gen, view, snap) = job.expect("resort requested");
    let mut order = Vec::new();
    sort::sort(&snap.listing, &snap.keys, view, &mut order);
    app.on_resorted(tab, gen, view, snap.listing.len(), order);
}

fn run(app: &mut App, id: &str) {
    assert!(app.run_action(id, Instant::now()), "{id}");
}

fn press_key(app: &mut App, key: crate::input::Key, mods: Mods) {
    app.on_event(&Event::Key { key, mods, pressed: true }, Instant::now());
    app.on_event(&Event::Key { key, mods: Mods::default(), pressed: false }, Instant::now());
}

fn line_y(app: &App, line: usize) -> f32 {
    app.layout.list.y + (line as f32 + 0.5) * app.layout.row_h
}

fn click_line(app: &mut App, dx: f32, line: usize, button: MouseButton) {
    let (x, y) = (app.layout.cols[0].x + dx, line_y(app, line));
    click(app, x, y, button);
}

fn click(app: &mut App, x: f32, y: f32, button: MouseButton) {
    app.on_event(&Event::MouseDown { x, y, button, clicks: 1 }, Instant::now());
    app.on_event(&Event::MouseUp { x, y, button }, Instant::now());
}

fn listed(app: &App, of: impl Fn(&DirView) -> Vec<u32>) -> Vec<String> {
    let v = app.tab().view.as_ref().unwrap();
    of(v).into_iter().map(|i| String::from_utf8_lossy(v.listing().name(i)).into_owned()).collect()
}

fn names(app: &App) -> Vec<String> {
    listed(app, |v| v.rows().to_vec())
}

fn selected(app: &App) -> Vec<String> {
    listed(app, |v| v.selected().collect())
}

fn menu_of(app: &mut App) -> ShellMenu {
    app.drain_effects().find_map(|e| if let Effect::ShellMenu(m) = e { Some(m) } else { None }).expect("menu shown")
}

fn commands(items: &[MenuItem]) -> Vec<(String, bool)> {
    items
        .iter()
        .flat_map(|i| match i {
            MenuItem::Command { label, checked, .. } => vec![(label.clone(), *checked)],
            MenuItem::Submenu { items, .. } => commands(items),
            MenuItem::Header(_) | MenuItem::Separator => Vec::new(),
        })
        .collect()
}

fn group_labels(app: &App) -> Vec<String> {
    let v = app.tab().view.as_ref().unwrap();
    let arena = Bump::new();
    v.groups().iter().map(|g| format!("{} {}", app.group_label(&arena, v, g), g.count)).collect()
}

#[test]
fn grouping_adds_counted_headers_that_collapse_on_click() {
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[("b.txt", 10, false), ("a.rs", 20_000, false), ("src", 0, true), ("c.TXT", 30, false)]);
    run(&mut app, "view.group.type");
    resort_now(&mut app);
    assert_eq!(group_labels(&app), ["File folder 1", "RS File 1", "TXT File 2"]);
    assert_eq!(names(&app), ["src", "a.rs", "b.txt", "c.TXT"]);
    assert_eq!(app.tab().view.as_ref().unwrap().lines(), 7);
    let x = app.layout.cols[0].x + 100.0;
    assert_eq!([0, 1, 4, 6].map(|n| app.hit(x, line_y(&app, n))), [Hit::Group(0), Hit::Row(0), Hit::Group(2), Hit::Row(3)]);
    click_line(&mut app, 100.0, 4, MouseButton::Left);
    assert_eq!(names(&app), ["src", "a.rs"]);
    assert_eq!(app.tab().view.as_ref().unwrap().lines(), 5);
    let c = app.tab().view.as_ref().unwrap().all_counts();
    assert_eq!(c.dirs + c.files, 4, "collapsed items still count");
    frame(&mut app);
    click_line(&mut app, 100.0, 4, MouseButton::Left);
    assert_eq!(names(&app).len(), 4);
    run(&mut app, "view.group.size");
    resort_now(&mut app);
    assert_eq!(group_labels(&app), ["Small (16 KB - 1 MB) 1", "Tiny (1 B - 16 KB) 2", "Unspecified 1"], "folders without a size come last");
    run(&mut app, "view.sort.descending");
    resort_now(&mut app);
    assert_eq!(group_labels(&app)[0], "Small (16 KB - 1 MB) 1", "the order of groups is fixed");
    run(&mut app, "view.group.modified");
    resort_now(&mut app);
    assert_eq!(group_labels(&app), ["Unspecified 4"]);
    run(&mut app, "view.group.name");
    resort_now(&mut app);
    assert_eq!(group_labels(&app), ["A - H 3", "Q - Z 1"]);
    run(&mut app, "view.group.none");
    resort_now(&mut app);
    assert!(app.tab().view.as_ref().unwrap().groups().is_empty());
}

#[test]
fn views_are_remembered_per_folder_and_new_folders_use_the_defaults() {
    let mut app = app_at(r"C:\a");
    app.set_view_mode(ViewMode::Folder);
    let now = Instant::now();
    run(&mut app, "view.sort.size");
    run(&mut app, "view.group.type");
    app.navigate(loc(r"C:\b"), now);
    assert_eq!((app.tab().view_spec.sort, app.tab().view_spec.group), (SortSpec::default(), GroupSpec::default()));
    run(&mut app, "view.sort.descending");
    app.drain_effects().for_each(drop);
    app.go_history(false, now);
    assert_eq!((app.tab().view_spec.sort.field, app.tab().view_spec.group.field), (SortField::Size, GroupField::Type));
    let listing_view = app.drain_effects().find_map(|e| match e {
        Effect::Provider { req: ProviderRequest::List { view, .. }, .. } => Some(view),
        _ => None,
    });
    assert_eq!(listing_view.map(|v| (v.sort.field, v.group.field)), Some((SortField::Size, GroupField::Type)), "the folder is listed with its own view");
    app.go_history(true, now);
    assert!(app.tab().view_spec.sort.descending);
    assert_eq!(app.persisted().folders.len(), 1, "only the folder with a saved view is remembered");
    run(&mut app, "view.set_default");
    app.navigate(loc(r"C:\c"), now);
    assert!(app.tab().view_spec.sort.descending, "new folders open with the default view");
    run(&mut app, "view.same_for_all");
    run(&mut app, "view.sort.type");
    app.navigate(loc(r"C:\a"), now);
    assert_eq!(app.tab().view_spec.sort.field, SortField::Type, "one view for every folder");
    assert_eq!(app.folder_views().get(folder_key(&loc(r"C:\a"))).unwrap().sort.field, SortField::Size, "the remembered view is kept");
    run(&mut app, "view.same_for_all");
    app.refresh(now);
    assert_eq!(app.tab().view_spec.sort.field, SortField::Size);
}

#[test]
fn the_folder_store_is_bounded_and_keeps_recent_folders() {
    let store = FolderViews::default();
    for k in 0..FOLDER_VIEWS_CAP as u64 {
        store.put(k, FolderView::default());
    }
    assert!(store.get(0).is_some());
    store.put(u64::MAX, FolderView::default());
    assert!(store.len() <= FOLDER_VIEWS_CAP * 3 / 4 + 1);
    assert!(store.get(0).is_some() && store.get(u64::MAX).is_some() && store.get(1).is_none(), "the least recently used folders go first");
    assert_eq!(folder_key(&loc(r"C:\Docs")), folder_key(&loc(r"c:\docs")));
    assert_ne!(folder_key(&loc(r"C:\Docs")), folder_key(&loc("wsl://Ubuntu/Docs")));
}

#[test]
fn the_header_menu_shows_hides_and_fits_columns_and_the_bin_keeps_its_own() {
    let mut app = app_at(r"C:\x");
    app.set_view_mode(ViewMode::Folder);
    deliver(&mut app, &[("a.txt", 10, false)]);
    let h = app.layout.cols[1];
    click(&mut app, h.x + 20.0, h.y + h.h / 2.0, MouseButton::Right);
    let menu = menu_of(&mut app);
    assert!(!menu.shell && menu.items.is_empty());
    let want = [
        ("Shift Column Left", false),
        ("Shift Column Right", false),
        ("Size Column To Fit", false),
        ("Close Column", false),
        ("Size All Columns To Fit", false),
        ("Restore Default Columns", false),
        ("Name", true),
        ("Type", true),
        ("Size", true),
        ("Total Items", true),
        ("Date Created", false),
        ("Date Modified", true),
        ("Date Accessed", false),
        ("Volume Usage", false),
        ("Folder Usage", false),
        ("Tags", false),
        ("Always size columns to fit", false),
    ];
    assert_eq!(commands(&menu.menu), want.map(|(l, c)| (l.to_owned(), c)));
    let disabled: Vec<bool> = menu.menu.iter().filter_map(|m| if let MenuItem::Command { disabled, .. } = m { Some(*disabled) } else { None }).collect();
    assert_eq!((disabled[0], disabled[1], disabled[5], disabled[6]), (true, false, true, true), "Type cannot move left of Name, defaults cannot be restored, Name always shows");
    app.on_event(&Event::MenuCommand(10), Instant::now());
    assert_eq!(app.columns(), [Column::Name, Column::Type, Column::Modified, Column::Items, Column::Size, Column::Created]);
    assert_eq!(app.layout.columns, app.columns());
    let h = app.layout.cols[1];
    click(&mut app, h.x + 20.0, h.y + h.h / 2.0, MouseButton::Right);
    menu_of(&mut app);
    app.on_event(&Event::MenuCommand(1), Instant::now());
    assert_eq!(app.columns(), [Column::Name, Column::Modified, Column::Type, Column::Items, Column::Size, Column::Created], "Shift Column Right");
    let t = app.layout.cols[2];
    app.on_event(&Event::MouseDown { x: t.x + 20.0, y: t.y + 5.0, button: MouseButton::Middle, clicks: 1 }, Instant::now());
    app.on_event(&Event::MouseUp { x: t.x + 20.0, y: t.y + 5.0, button: MouseButton::Middle }, Instant::now());
    assert_eq!(app.columns(), [Column::Name, Column::Modified, Column::Items, Column::Size, Column::Created], "a middle click closes the column");
    click(&mut app, h.x + 20.0, h.y + h.h / 2.0, MouseButton::Right);
    menu_of(&mut app);
    app.on_event(&Event::MenuCommand(5), Instant::now());
    assert_eq!(app.columns(), [Column::Name, Column::Type, Column::Modified, Column::Items, Column::Size], "Restore Default Columns");
    run(&mut app, "view.column.created");
    run(&mut app, "view.column.type");
    run(&mut app, "view.columns.fit");
    frame(&mut app);
    let w = app.tab().columns.widths;
    assert_eq!([Column::Modified, Column::Created, Column::Size].map(|c| w[c as usize]), [170.0, 160.0, 80.0]);
    let k = app.layout.columns.iter().position(|&c| c == Column::Size).unwrap();
    let (sx, sy) = (app.layout.cols[k].x, h.y + h.h / 2.0);
    app.on_event(&Event::MouseDown { x: sx, y: sy, button: MouseButton::Left, clicks: 1 }, Instant::now());
    app.on_event(&Event::MouseMove { x: sx - 25.0, y: sy }, Instant::now());
    app.on_event(&Event::MouseUp { x: sx - 25.0, y: sy, button: MouseButton::Left }, Instant::now());
    assert_eq!(app.tab().columns.width(Column::Size), 105.0);
    app.navigate(Location::recycle_bin(), Instant::now());
    deliver(&mut app, &[]);
    assert_eq!(app.columns(), BIN_COLUMNS);
    assert_eq!(app.tab().columns.width(Column::Size), 90.0);
    app.go_history(false, Instant::now());
    assert_eq!((app.tab().columns.width(Column::Size), app.tab().columns.shown(Column::Type)), (105.0, false));
}

#[test]
fn the_recycle_bin_background_menu_offers_view_commands_without_shell_items() {
    let mut app = app_at("shell:RecycleBinFolder");
    deliver(&mut app, &[("old", 0, true)]);
    click_line(&mut app, 100.0, 6, MouseButton::Right);
    let menu = menu_of(&mut app);
    assert!(!menu.shell);
    let labels: Vec<String> = commands(&menu.menu).into_iter().map(|c| c.0).collect();
    for want in ["Original location", "Date deleted", "Show files first", "Refresh", "Empty Recycle Bin", "Restore all items", "Properties"] {
        assert!(labels.iter().any(|l| l == want), "{want} missing from {labels:?}");
    }
    assert!(!labels.iter().any(|l| l == "Paste"), "{labels:?}");
}

#[test]
fn selection_commands_checkboxes_and_hover_select() {
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[("a", 1, false), ("b", 1, false), ("c", 1, false)]);
    app.run_action("select.invert", Instant::now());
    assert_eq!(selected(&app), ["a", "b", "c"]);
    let x = app.layout.cols[0].x + 100.0;
    click_line(&mut app, 100.0, 0, MouseButton::Left);
    app.run_action("select.invert", Instant::now());
    assert_eq!(selected(&app), ["b", "c"]);
    press_key(&mut app, crate::input::Key::Escape, Mods::default());
    assert!(selected(&app).is_empty());
    run(&mut app, "view.checkboxes");
    frame(&mut app);
    click_line(&mut app, 15.0, 0, MouseButton::Left);
    click_line(&mut app, 15.0, 2, MouseButton::Left);
    assert_eq!(selected(&app), ["a", "c"]);
    click_line(&mut app, 15.0, 0, MouseButton::Left);
    assert_eq!(selected(&app), ["c"]);
    run(&mut app, "view.checkboxes");
    run(&mut app, "view.hover_select");
    app.drain_effects().for_each(drop);
    let now = Instant::now();
    app.on_event(&Event::MouseMove { x, y: line_y(&app, 1) }, now);
    assert!(app.drain_effects().any(|e| matches!(e, Effect::Timer(d) if d == Duration::from_millis(600))));
    app.on_event(&Event::Timer, now + Duration::from_millis(300));
    assert_eq!(selected(&app), ["c"]);
    assert!(app.drain_effects().any(|e| matches!(e, Effect::Timer(d) if d == Duration::from_millis(300))), "an early timer is armed again");
    app.on_event(&Event::Timer, now + Duration::from_millis(600));
    assert_eq!(selected(&app), ["b"]);
}

#[test]
fn a_middle_click_scrolls_with_the_mouse_until_the_next_click() {
    let mut app = app_at(r"C:\x");
    let items: Vec<String> = (0..300).map(|i| format!("f{i:03}")).collect();
    let items: Vec<(&str, u64, bool)> = items.iter().map(|n| (n.as_str(), 1, false)).collect();
    deliver(&mut app, &items);
    let (x, y) = (app.layout.cols[0].x + 100.0, line_y(&app, 3));
    click(&mut app, x, y, MouseButton::Middle);
    assert!(app.details.autoscroll.is_some() && app.cursor == CursorIcon::Scroll);
    app.on_event(&Event::MouseMove { x, y: y + 112.0 }, Instant::now());
    let mut g = Gfx::new(Mono, 1024);
    let t = Instant::now();
    app.frame(&mut g, t);
    assert!(app.frame(&mut g, t + Duration::from_millis(100)), "autoscrolling keeps frames coming");
    assert!(app.tab().scroll > 50.0, "{}", app.tab().scroll);
    click(&mut app, x, y, MouseButton::Left);
    assert!(app.details.autoscroll.is_none() && app.cursor == CursorIcon::Default);
    assert!(selected(&app).is_empty(), "the click that ends autoscrolling selects nothing");
}

#[test]
fn display_switches_hide_dotfiles_extensions_and_the_status_bar_and_split_the_count() {
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[(".env", 1, false), ("notes.txt", 1, false), ("src", 0, true)]);
    run(&mut app, "view.dotfiles");
    resort_now(&mut app);
    assert_eq!(names(&app), ["src", "notes.txt"]);
    run(&mut app, "view.system");
    resort_now(&mut app);
    assert!(app.tab().view_spec.show_system && app.tab().view_spec.hide_dotfiles);
    run(&mut app, "view.extensions");
    let v = app.tab().view.as_ref().unwrap();
    let shown: Vec<String> = v.rows().iter().map(|&i| app.shown_name(v.listing(), i).into_owned()).collect();
    assert_eq!(shown, ["src", "notes"]);
    let c = app.tab().view.as_ref().unwrap().shown_counts();
    assert_eq!((c.dirs, c.files), (1, 1));
    run(&mut app, "view.status_bar");
    frame(&mut app);
    assert_eq!((app.layout.status.h, app.layout.list.bottom()), (0.0, 600.0));
    app.status("Something went wrong");
    frame(&mut app);
    assert!(app.layout.status.h > 0.0, "errors still show");
}

#[test]
fn unavailable_folders_explain_themselves_and_auto_sizes_measure_loaded_folders() {
    let mut app = app_at(r"C:\x");
    run(&mut app, "view.auto_sizes");
    let fx = deliver_with(&mut app, &[("src", 0, true), ("a.txt", 1, false)], None);
    assert!(fx.iter().any(|e| matches!(e, Effect::Provider { req: ProviderRequest::DirSizes { items, .. }, .. } if items.len() == 1)));
    app.navigate(loc(r"C:\gone"), Instant::now());
    let fx = deliver_with(&mut app, &[], Some(FsError { kind: FsErrorKind::NotFound, code: 3 }));
    assert!(!fx.iter().any(|e| matches!(e, Effect::Provider { req: ProviderRequest::DirSizes { .. }, .. })));
    assert_eq!(app.tab().error.as_deref(), Some("Path not found (0x00000003)"));
}

#[test]
fn search_results_can_be_grouped_by_folder() {
    let mut app = app_at(r"C:\x");
    app.open_search();
    app.drain_effects().for_each(drop);
    run(&mut app, "search.group_folder");
    assert!(app.search.group_folder);
    let query = app.drain_effects().find_map(|e| if let Effect::Search(SearchRequest::Query { sort, .. }) = e { Some(sort) } else { None });
    assert_eq!(query.map(|s| s.key), Some(SearchKey::Folder));
    let menu = app.build_search_menu(false);
    assert_eq!(commands(&menu), [("Group by folder".to_owned(), true)]);
    let menu = app.build_search_menu(true);
    assert_eq!(commands(&menu), [("Open file location".to_owned(), false), ("Group by folder".to_owned(), true)]);
}

#[test]
fn alternate_data_streams_show_only_after_the_toggle_which_relists_every_tab() {
    let mut l = Listing::default();
    l.push(b"a.txt", Meta::default());
    l.push(b"a.txt:meta", Meta { attrs: crate::listing::attr::STREAM, ..Default::default() });
    let shown = |v: &sort::ViewSpec| (0..2).filter(|&i| sort::visible(&l, i, v)).count();
    assert_eq!(shown(&sort::ViewSpec::default()), 1);
    assert_eq!(shown(&sort::ViewSpec { show_streams: true, ..Default::default() }), 2);
    let mut app = app_at(r"C:\a");
    deliver(&mut app, &[("a.txt", 1, false)]);
    run(&mut app, "view.streams");
    assert!(app.prefs.show_streams && app.tab().view_spec.show_streams);
    let relisted = app.drain_effects().find_map(|e| match e {
        Effect::Provider { req: ProviderRequest::List { view, .. }, .. } => Some(view),
        _ => None,
    });
    assert!(relisted.is_some_and(|v| v.show_streams), "turning streams on lists the folder again with streams");
    run(&mut app, "view.streams");
    assert!(!app.tab().view_spec.show_streams);
    assert!(!app.drain_effects().any(|e| matches!(e, Effect::Provider { req: ProviderRequest::List { .. }, .. })), "turning them off only filters");
}

#[test]
fn alternate_data_stream_rows_are_never_handed_to_file_operations() {
    let mut app = app_at(r"C:\a");
    run(&mut app, "view.streams");
    let t = app.tab();
    let mut listing = Listing::default();
    let mut keys = SortKeys::new(CaseMode::Insensitive);
    for (name, attrs) in [("a.txt", 0), ("a.txt:Zone.Identifier", crate::listing::attr::STREAM)] {
        let i = listing.push(name.as_bytes(), Meta { attrs, ..Default::default() });
        keys.push(listing.name(i));
    }
    let view = t.view_spec;
    let mut order = Vec::new();
    sort::sort(&listing, &keys, view, &mut order);
    let snap = DirSnapshot { gen: t.gen, path: t.location().path().clone(), listing, keys, order, view, done: true, error: None, elapsed: Duration::ZERO };
    t.slot.publish(Arc::new(snap));
    app.poll();
    frame(&mut app);
    app.drain_effects().for_each(drop);
    let v = app.tab_mut().view.as_mut().unwrap();
    v.clear_selection();
    v.set_selected(1, true);
    v.focus = Some(1);
    for id in ["item.trash", "item.delete", "file.copy", "file.cut", "file.rename"] {
        app.tab_mut().error = None;
        run(&mut app, id);
        assert!(!app.drain_effects().any(|e| matches!(e, Effect::Provider { req: ProviderRequest::Op { .. }, .. } | Effect::SetClipboardFiles { .. })), "{id}");
        assert_eq!(app.tab().error.as_deref(), Some("Alternate data streams are shown for viewing only"), "{id}");
    }
}

#[test]
fn folder_views_are_keyed_by_case_only_on_unix_paths() {
    let key = |s: &str| folder_key(&Location::parse(s).unwrap());
    assert_eq!(key(r"C:\Users\A"), key(r"c:\users\a"));
    assert_ne!(key("/home/A"), key("/home/a"));
    assert_eq!(key("/home/a"), key("/home/a"));
}

#[test]
fn saved_views_follow_none_this_folder_and_subfolders() {
    let now = Instant::now();
    let mut app = app_at(r"C:\p");
    assert_eq!(app.view_mode(), ViewMode::None);
    run(&mut app, "view.sort.size");
    app.navigate(loc(r"C:\q"), now);
    assert_eq!(app.tab().view_spec.sort.field, SortField::Size, "without a saved view the change follows to other folders");
    assert!(app.persisted().folders.is_empty());

    app.navigate(loc(r"C:\p"), now);
    app.set_view_mode(ViewMode::Subfolders);
    run(&mut app, "view.sort.type");
    app.navigate(loc(r"C:\p\deep\er"), now);
    assert_eq!(app.tab().view_spec.sort.field, SortField::Type, "subfolders use the saved view");
    assert_eq!(app.view_mode(), ViewMode::None, "the radio shows only the folder's own mode");
    run(&mut app, "view.sort.name");
    app.navigate(loc(r"C:\p"), now);
    assert_eq!(app.tab().view_spec.sort.field, SortField::Name, "a change below updates the saved view");
    app.navigate(loc(r"C:\q"), now);
    assert_eq!(app.tab().view_spec.sort.field, SortField::Size, "other folders keep the defaults");

    app.navigate(loc(r"C:\p\deep"), now);
    app.set_view_mode(ViewMode::Folder);
    run(&mut app, "view.sort.modified");
    app.navigate(loc(r"C:\p\deep\er"), now);
    assert_eq!(app.tab().view_spec.sort.field, SortField::Name, "This folder does not reach its subfolders");

    app.navigate(loc(r"C:\p"), now);
    app.run_action("folder.cycle_view", now);
    assert_eq!(app.view_mode(), ViewMode::None);
    app.run_action("folder.cycle_view", now);
    assert_eq!(app.view_mode(), ViewMode::Folder);
    app.run_action("views.clear", now);
    assert!(app.persisted().folders.is_empty());
    assert_eq!(app.view_mode(), ViewMode::None);
}

#[test]
fn special_places_keep_their_own_view_without_asking() {
    let now = Instant::now();
    let mut app = app_at(r"C:\x");
    app.navigate(Location::computer(), now);
    run(&mut app, "view.sort.size");
    app.navigate(loc(r"C:\x"), now);
    assert_eq!(app.tab().view_spec.sort.field, SortField::Name);
    app.navigate(Location::computer(), now);
    assert_eq!(app.tab().view_spec.sort.field, SortField::Size);
}

#[test]
fn the_saved_view_menu_has_a_header_three_modes_and_set_as_default() {
    let mut app = app_at(r"C:\x");
    frame(&mut app);
    let r = app.layout.view;
    assert_eq!(app.hit(r.x + r.w / 2.0, r.y + r.h / 2.0), Hit::ViewMode);
    app.drain_effects().for_each(drop);
    app.view_mode_menu();
    let m = menu_of(&mut app);
    assert!(matches!(&m.menu[0], MenuItem::Header(h) if h == "Saved view settings"));
    assert_eq!(commands(&m.menu), [("None".to_owned(), true), ("This folder".to_owned(), false), ("This folder and subfolders".to_owned(), false), ("Set as default".to_owned(), false)]);
    app.on_event(&Event::MenuCommand(1), Instant::now());
    assert_eq!(app.view_mode(), ViewMode::Folder);
    assert_eq!(app.tip_text(Hit::ViewMode).unwrap().line, "View settings are saved for this folder.");
    app.view_mode_menu();
    let m = menu_of(&mut app);
    assert!(m.menu.iter().any(|i| matches!(i, MenuItem::Command { label, disabled: true, .. } if label == "Set as default")), "Set as default is off for This folder");
}

#[test]
fn new_columns_show_item_counts_usage_and_drive_space() {
    use crate::listing::{attr, DirStats, Listing};
    let app = app_at(r"C:\x");
    let mut l = Listing::default();
    let d = l.push(b"dir", Meta { attrs: attr::DIR, ..Default::default() });
    let empty = l.push(b"none", Meta { attrs: attr::DIR, ..Default::default() });
    let f = l.push(b"f.txt", Meta { size: 250, ..Default::default() });
    l.set_stats(d, DirStats { size: 750, folders: 2, files: 10, partial: false });
    l.set_stats(empty, DirStats::default());
    l.volume = 10_000;
    let arena = Bump::new();
    let cell = |l: &Listing, i, c| app.cell_text(&arena, l, i, c).to_owned();
    assert_eq!([cell(&l, d, Column::Items), cell(&l, empty, Column::Items), cell(&l, f, Column::Items)], ["12", "--", "--"]);
    assert_eq!(cell(&l, f, Column::Volume), "2.5%");
    assert_eq!(cell(&l, f, Column::FolderUsage), "100.0%");
    let c = l.push(b"C:", Meta { attrs: attr::DIR | attr::VOLUME, size: 1000, ..Default::default() });
    l.push_drive_info(250, "NTFS");
    assert_eq!([cell(&l, c, Column::Percent), cell(&l, c, Column::FileSystem)], ["75.0%", "NTFS"]);
    assert!(!cell(&l, c, Column::Free).is_empty());
    assert_eq!(cell(&l, f, Column::Accessed), "", "no access time, no text");
}
