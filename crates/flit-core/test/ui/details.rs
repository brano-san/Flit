use super::*;
use crate::input::{Event, Mods};
use crate::listing::{DirSnapshot, Meta};
use crate::provider::{ProviderEvent, ProviderRequest};
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
    app.animations = false;
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
        ("Folders", true),
        ("Files", true),
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
    app.on_event(&Event::MenuCommand(11), Instant::now());
    assert_eq!(app.columns(), [Column::Name, Column::Type, Column::Modified, Column::Folders, Column::Files, Column::Size, Column::Created]);
    assert_eq!(app.layout.columns, app.columns());
    let h = app.layout.cols[1];
    click(&mut app, h.x + 20.0, h.y + h.h / 2.0, MouseButton::Right);
    menu_of(&mut app);
    app.on_event(&Event::MenuCommand(1), Instant::now());
    assert_eq!(app.columns(), [Column::Name, Column::Modified, Column::Type, Column::Folders, Column::Files, Column::Size, Column::Created], "Shift Column Right");
    let t = app.layout.cols[2];
    app.on_event(&Event::MouseDown { x: t.x + 20.0, y: t.y + 5.0, button: MouseButton::Middle, clicks: 1 }, Instant::now());
    app.on_event(&Event::MouseUp { x: t.x + 20.0, y: t.y + 5.0, button: MouseButton::Middle }, Instant::now());
    assert_eq!(app.columns(), [Column::Name, Column::Modified, Column::Folders, Column::Files, Column::Size, Column::Created], "a middle click closes the column");
    click(&mut app, h.x + 20.0, h.y + h.h / 2.0, MouseButton::Right);
    menu_of(&mut app);
    app.on_event(&Event::MenuCommand(5), Instant::now());
    assert_eq!(app.columns(), [Column::Name, Column::Type, Column::Modified, Column::Folders, Column::Files, Column::Size], "Restore Default Columns");
    run(&mut app, "view.column.created");
    run(&mut app, "view.column.type");
    run(&mut app, "view.columns.fit");
    frame(&mut app);
    let w = app.tab().columns.widths;
    assert_eq!([Column::Modified, Column::Created, Column::Size].map(|c| w[c as usize]), [170.0, 160.0, 80.0]);
    let k = app.layout.columns.iter().position(|&c| c == Column::Size).unwrap();
    let displayed = app.layout.cols[k].w / app.content_px();
    let other_widths: Vec<_> = app.layout.columns.iter().zip(&app.layout.cols).filter(|(c, _)| **c != Column::Name && **c != Column::Size).map(|(&c, r)| (c, r.w / app.content_px())).collect();
    let expected = (displayed + 25.0).min(app.layout.list.w / app.content_px() - crate::ui::NAME_MIN_W - other_widths.iter().map(|(_, w)| w).sum::<f32>());
    let (sx, sy) = (app.layout.cols[k].x, h.y + h.h / 2.0);
    app.on_event(&Event::MouseDown { x: sx, y: sy, button: MouseButton::Left, clicks: 1 }, Instant::now());
    app.on_event(&Event::MouseMove { x: sx - 25.0, y: sy }, Instant::now());
    app.on_event(&Event::MouseUp { x: sx - 25.0, y: sy, button: MouseButton::Left }, Instant::now());
    assert_eq!(app.tab().columns.width(Column::Size), expected);
    for (col, width) in other_widths {
        assert_eq!(app.tab().columns.width(col), width);
    }
    app.navigate(Location::recycle_bin(), Instant::now());
    deliver(&mut app, &[]);
    assert_eq!(app.columns(), BIN_COLUMNS);
    assert_eq!(app.tab().columns.width(Column::Size), 90.0);
    app.go_history(false, Instant::now());
    assert_eq!((app.tab().columns.width(Column::Size), app.tab().columns.shown(Column::Type)), (expected, false));
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
    app.prefs.checkboxes = false;
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
    app.animations = false;
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
fn hidden_statistics_columns_cancel_work_and_new_entries_do_not_recount_their_neighbors() {
    let mut app = app_at(r"C:\x");
    run(&mut app, "view.auto_sizes");
    let fx = deliver_with(&mut app, &[("old", 0, true)], None);
    let token = fx.iter().find_map(|e| match e { Effect::Provider { req: ProviderRequest::DirSizes { cancel, .. }, .. } => Some(cancel.clone()), _ => None }).unwrap();
    let gen = app.tab().gen;
    for c in [Column::Size, Column::Folders, Column::Files] {
        if app.tab().columns.shown(c) { app.toggle_column(c); }
    }
    assert!(!token.is_current(gen));
    assert!(!app.tab().sizing(0));
    app.drain_effects().for_each(drop);
    app.compute_dir_sizes();
    assert!(!app.drain_effects().any(|e| matches!(e, Effect::Provider { req: ProviderRequest::DirSizes { .. }, .. })));
    app.tab_mut().view.as_mut().unwrap().set_dir_stats(0, crate::listing::DirStats { size: 42, ..Default::default() });
    app.toggle_column(Column::Size);
    app.drain_effects().for_each(drop);
    app.tab_mut().view.as_mut().unwrap().apply(&crate::listing::DirChange { name: b"new".to_vec(), kind: crate::listing::ChangeKind::Added, meta: Some(Meta { attrs: attr::DIR, ..Default::default() }) });
    app.compute_dir_sizes();
    let wanted: Vec<_> = app.drain_effects().filter_map(|e| match e { Effect::Provider { req: ProviderRequest::DirSizes { items, .. }, .. } => Some(items), _ => None }).flatten().collect();
    assert_eq!(wanted, [(1, b"new".to_vec())]);
    assert!(app.tab().sizing(1));
    assert_eq!(app.tab().view.as_ref().unwrap().listing().entries[0].size, 42);
}

#[test]
fn statistics_switches_compute_only_visible_requested_data_and_share_one_scan() {
    use crate::ui::settings::{index, SETTINGS};
    let ids = ["files.auto_sizes", "files.count_folders", "files.count_files"];
    let columns = [Column::Size, Column::Folders, Column::Files];
    for mask in 0..8 {
        let mut app = app_at(r"C:\x");
        for (k, id) in ids.iter().enumerate() {
            SETTINGS[index(id).unwrap()].set(&mut app, crate::ui::Value::Bool(mask & (1 << k) != 0));
        }
        let fx = deliver_with(&mut app, &[("src", 0, true)], None);
        assert_eq!(fx.iter().filter(|e| matches!(e, Effect::Provider { req: ProviderRequest::DirSizes { .. }, .. })).count(), usize::from(mask != 0));
        app.tab_mut().view.as_mut().unwrap().set_dir_stats(0, crate::listing::DirStats { size: 42, folders: 2, files: 3, partial: false });
        let arena = Bump::new();
        for (k, col) in columns.iter().enumerate() {
            let text = app.cell_text(&arena, app.tab().view.as_ref().unwrap().listing(), 0, *col);
            assert_eq!(!text.is_empty(), mask & (1 << k) != 0, "mask={mask}, column={col:?}");
        }
        for col in columns {
            if app.tab().columns.shown(col) { app.toggle_column(col); }
        }
        assert!(!app.folder_stats_visible());
    }
}

#[test]
fn checkbox_animation_starts_at_zero_and_finishes_once() {
    let now = Instant::now();
    let mut checks = Checks::default();
    checks.settle(vec![(1, false)], true, now);
    assert_eq!(checks.progress(1, now), 0.0);
    checks.settle(vec![(1, true)], true, now);
    let mut previous = 0.0;
    for ms in [20, 60, 100, 160, 300, 600] {
        let time = now + Duration::from_millis(ms);
        let progress = checks.progress(1, time);
        assert!(progress >= previous);
        previous = progress;
        checks.settle(vec![(1, true)], true, time);
    }
    assert_eq!(previous, 1.0);
    assert!(checks.fresh.is_empty());
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
    assert_eq!(commands(&menu), [("Group by folder".to_owned(), true)]);
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
    let mut app = app_at(r"C:\x");
    app.prefs.count_folders = true;
    app.prefs.count_files = true;
    let mut l = Listing::default();
    let d = l.push(b"dir", Meta { attrs: attr::DIR, ..Default::default() });
    let empty = l.push(b"none", Meta { attrs: attr::DIR, ..Default::default() });
    let f = l.push(b"f.txt", Meta { size: 250, ..Default::default() });
    l.set_stats(d, DirStats { size: 750, folders: 2, files: 10, partial: false });
    l.set_stats(empty, DirStats::default());
    l.volume = 10_000;
    let arena = Bump::new();
    let cell = |l: &Listing, i, c| app.cell_text(&arena, l, i, c).to_owned();
    assert_eq!([cell(&l, d, Column::Folders), cell(&l, empty, Column::Folders), cell(&l, f, Column::Folders)], ["2", "--", "--"]);
    assert_eq!([cell(&l, d, Column::Files), cell(&l, empty, Column::Files), cell(&l, f, Column::Files)], ["10", "--", "--"]);
    let only_files = l.push(b"docs", Meta { attrs: attr::DIR, ..Default::default() });
    l.set_stats(only_files, DirStats { size: 1, folders: 0, files: 3, partial: false });
    assert_eq!([cell(&l, only_files, Column::Folders), cell(&l, only_files, Column::Files)], ["--", "3"]);
    assert_eq!([Column::Folders, Column::Files].map(|c| c.title()), ["Folders", "Files"]);
    assert_eq!([Column::Folders, Column::Files].map(|c| c.field()), [Some(crate::sort::SortField::Folders), Some(crate::sort::SortField::Files)]);
    assert_eq!(cell(&l, f, Column::Volume), "2.5%");
    assert_eq!(cell(&l, f, Column::FolderUsage), "100.0%");
    let c = l.push(b"C:", Meta { attrs: attr::DIR | attr::VOLUME, size: 1000, ..Default::default() });
    l.push_drive_info(250, "NTFS");
    assert_eq!([cell(&l, c, Column::Percent), cell(&l, c, Column::FileSystem)], ["75.0%", "NTFS"]);
    assert!(!cell(&l, c, Column::Free).is_empty());
    assert_eq!(cell(&l, f, Column::Accessed), "", "no access time, no text");
}

#[test]
fn item_checkboxes_appear_only_on_selected_items_and_hovered_icons() {
    assert!(!crate::ui::details::shows_check(true, false, false));
    assert!(crate::ui::details::shows_check(true, true, false) && crate::ui::details::shows_check(true, false, true));
    assert!(!crate::ui::details::shows_check(false, true, true), "the setting turns them off");
    let mut a = crate::ui::harness::app(&["a.txt", "b.txt", "c.txt"]);
    a.prefs.checkboxes = true;
    let t0 = Instant::now();
    let mut g = Gfx::new(Mono, 1024);
    assert!(!a.frame(&mut g, t0));
    assert!(a.details.checks.seen.len() == 3 && a.details.checks.seen.iter().all(|s| !s.1), "no boxes while nothing is selected or hovered");
    a.toggle_row(0);
    assert!(a.frame(&mut g, t0 + Duration::from_millis(1)), "the box grows in over a few frames");
    assert!(a.details.checks.progress(a.tab().view.as_ref().unwrap().rows()[0], t0 + Duration::from_millis(2)) < 1.0);
    assert!(!a.frame(&mut g, t0 + Duration::from_millis(400)), "and then the list is idle again");
    let shown: Vec<bool> = a.details.checks.seen.iter().map(|s| s.1).collect();
    assert_eq!(shown, [true, false, false]);
    let icon = a.item_parts(a.row_rect(1).unwrap()).icon;
    a.on_event(&Event::MouseMove { x: icon.x + icon.w / 2.0, y: icon.y + icon.h / 2.0 }, t0);
    assert_eq!(a.details.checks.hover, Some(1));
    a.frame(&mut g, t0 + Duration::from_millis(500));
    assert_eq!(a.details.checks.seen.iter().filter(|s| s.1).count(), 2, "hovering the icon shows its box");
    let name = a.item_parts(a.row_rect(1).unwrap()).name;
    a.on_event(&Event::MouseMove { x: name.x + 20.0, y: name.y + name.h / 2.0 }, t0);
    assert_eq!(a.details.checks.hover, None, "the rest of the row does not");
    a.animations = false;
    a.toggle_row(2);
    assert!(!a.frame(&mut g, t0 + Duration::from_millis(600)), "with Animations off the box appears at once");
}

#[test]
fn a_selected_item_draws_its_box_instead_of_its_icon() {
    let mut a = crate::ui::harness::app(&["a.txt", "b.txt"]);
    a.prefs.checkboxes = true;
    a.animations = false;
    a.toggle_row(0);
    let mut g = Gfx::new(Mono, 1024);
    a.frame(&mut g, Instant::now());
    let glyphs_in = |g: &Gfx<Mono>, r: crate::render::Rect| g.dl.instances.iter().filter(|i| i.kind == crate::render::kind::GLYPH && r.contains(i.rect[0] + i.rect[2] / 2.0, i.rect[1] + i.rect[3] / 2.0)).count();
    let (boxed, plain) = (a.item_parts(a.row_rect(0).unwrap()).icon, a.item_parts(a.row_rect(1).unwrap()).icon);
    assert_eq!(glyphs_in(&g, boxed), 0, "no icon under the box");
    assert!(glyphs_in(&g, plain) > 0, "the other row keeps its icon");
}

#[test]
fn clicking_icons_of_several_items_keeps_each_selected() {
    let mut a = crate::ui::harness::app(&["a.txt", "b.txt", "c.txt"]);
    a.prefs.checkboxes = true;
    let mut g = Gfx::new(Mono, 1024);
    let t0 = Instant::now();
    a.frame(&mut g, t0);
    let click_icon = |a: &mut App, row: usize, at: Instant| {
        let icon = a.item_parts(a.row_rect(row).unwrap()).icon;
        let (x, y) = (icon.x + icon.w / 2.0, icon.y + icon.h / 2.0);
        a.on_event(&Event::MouseMove { x, y }, at);
        a.on_event(&Event::MouseDown { x, y, button: MouseButton::Left, clicks: 1 }, at);
        a.on_event(&Event::MouseUp { x, y, button: MouseButton::Left }, at);
    };
    click_icon(&mut a, 0, t0);
    a.frame(&mut g, t0 + Duration::from_millis(50));
    click_icon(&mut a, 1, t0 + Duration::from_millis(900));
    a.frame(&mut g, t0 + Duration::from_millis(950));
    click_icon(&mut a, 2, t0 + Duration::from_millis(1900));
    let v = a.tab().view.as_ref().unwrap();
    assert_eq!(v.selected_count(), 3, "each icon click adds its item");
    click_icon(&mut a, 1, t0 + Duration::from_millis(2900));
    let v = a.tab().view.as_ref().unwrap();
    assert!(v.is_selected(v.rows()[0]) && !v.is_selected(v.rows()[1]) && v.is_selected(v.rows()[2]), "a second click on a box clears only that item");
}

#[test]
fn the_sort_arrow_turns_over_on_a_new_direction_and_fades_in_on_a_new_column() {
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[("a.txt", 10, false)]);
    frame(&mut app);
    let turn = crate::ui::address::TURN;
    let from = if app.tab().view_spec.sort.descending { 90.0 } else { -90.0 };
    app.tab_mut().view_spec.sort.descending ^= true;
    let t = Instant::now();
    assert!(app.frame(&mut Gfx::new(Mono, 1024), t));
    let mid = app.tab().motion.sort_turn.value(t + std::time::Duration::from_millis(50), turn);
    assert!(mid.abs() < 90.0 && mid != 0.0, "{mid}");
    app.frame(&mut Gfx::new(Mono, 1024), t + std::time::Duration::from_millis(400));
    assert_eq!(app.tab().motion.sort_turn.value(t + std::time::Duration::from_millis(400), turn), -from);
    app.tab_mut().view_spec.sort.field = crate::sort::SortField::Size;
    let t = t + std::time::Duration::from_millis(500);
    app.frame(&mut Gfx::new(Mono, 1024), t);
    assert!(app.tab().motion.sort_born.is_some(), "a new sort column fades its arrow in");
    app.frame(&mut Gfx::new(Mono, 1024), t + std::time::Duration::from_millis(400));
    assert!(app.tab().motion.sort_born.is_none());
    app.animations = false;
    app.tab_mut().view_spec.sort.descending ^= true;
    app.frame(&mut Gfx::new(Mono, 1024), t + std::time::Duration::from_millis(410));
    assert_eq!(app.tab().motion.sort_turn.value(t + std::time::Duration::from_millis(410), turn), from, "without Animations it flips at once");
}

#[test]
fn a_narrow_pane_squeezes_the_columns_to_fit_its_width() {
    let mut app = App::new(Persisted { path: loc(r"C:\x"), ..Default::default() }, Vec::new(), 0, false, 1.0, (560, 600), Instant::now());
    app.animations = false;
    let mut cols = app.tab().columns;
    cols.toggle(Column::Created);
    app.tab_mut().columns = cols;
    deliver(&mut app, &[("a.txt", 10, false)]);
    frame(&mut app);
    let (list, last) = (app.layout.list, *app.layout.cols.last().unwrap());
    assert!(app.layout.cols.len() > 3, "{:?}", app.layout.columns);
    assert!(last.right() <= list.right() + 0.5, "the table ends at {} past the pane edge {}: {:?} {:?}", last.right(), list.right(), app.layout.columns, app.layout.cols);
}

#[test]
fn a_shown_or_hidden_column_resizes_smoothly_and_its_neighbours_slide() {
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[("a.txt", 10, false)]);
    frame(&mut app);
    let width = |app: &App| app.layout.columns.iter().position(|&c| c == Column::Created).map_or(0.0, |k| app.layout.cols[k].w);
    let toggle = |app: &mut App| {
        let mut cols = app.tab().columns;
        cols.toggle(Column::Created);
        app.tab_mut().columns = cols;
    };
    toggle(&mut app);
    let t = Instant::now();
    app.frame(&mut Gfx::new(Mono, 1024), t);
    assert_eq!(width(&app), 0.0, "it starts closed");
    app.frame(&mut Gfx::new(Mono, 1024), t + std::time::Duration::from_millis(50));
    let mid = width(&app);
    app.frame(&mut Gfx::new(Mono, 1024), t + std::time::Duration::from_millis(400));
    let full = width(&app);
    assert!(mid > 0.0 && mid < full, "{mid} {full}");
    toggle(&mut app);
    let t = t + std::time::Duration::from_millis(400);
    app.frame(&mut Gfx::new(Mono, 1024), t);
    app.frame(&mut Gfx::new(Mono, 1024), t + std::time::Duration::from_millis(50));
    let closing = width(&app);
    assert!(closing > 0.0 && closing < full, "{closing}");
    app.frame(&mut Gfx::new(Mono, 1024), t + std::time::Duration::from_millis(400));
    assert!(!app.layout.columns.contains(&Column::Created));
    app.animations = false;
    toggle(&mut app);
    app.frame(&mut Gfx::new(Mono, 1024), t + std::time::Duration::from_millis(410));
    assert_eq!(width(&app), full, "without Animations the column is there at once");
}

#[test]
fn a_selected_tile_keeps_its_picture_and_shows_its_box_in_the_corner() {
    let mut a = crate::ui::harness::app(&["a.jpg", "b.jpg"]);
    a.prefs.checkboxes = true;
    a.animations = false;
    a.run_action("view.layout.grid", Instant::now());
    a.toggle_row(0);
    let mut g = Gfx::new(Mono, 1024);
    a.frame(&mut g, Instant::now());
    let glyphs_in = |g: &Gfx<Mono>, r: crate::render::Rect| g.dl.instances.iter().filter(|i| i.kind == crate::render::kind::GLYPH && r.contains(i.rect[0] + i.rect[2] / 2.0, i.rect[1] + i.rect[3] / 2.0)).count();
    let p = a.item_parts(a.row_rect(0).unwrap());
    assert!(glyphs_in(&g, p.icon) > 0, "the picture or icon of a selected tile stays visible");
    assert!(p.check.x < p.icon.x && p.check.y < p.icon.y && p.body.contains(p.check.x, p.check.y), "the box sits in the tile's corner, off the picture's centre");
    let other = a.item_parts(a.row_rect(1).unwrap());
    a.on_event(&Event::MouseMove { x: other.name.x + 4.0, y: other.name.y + 4.0 }, Instant::now());
    assert_eq!(a.details.checks.hover, Some(1), "hovering a tile shows its box");
}

#[test]
fn the_type_funnel_lights_on_hover_and_opens_its_menu_under_itself_right_aligned() {
    let mut a = crate::ui::harness::app(&["a.txt", "b.jpg"]);
    let k = a.layout.columns.iter().position(|&c| c == Column::Type).expect("a Type column");
    let f = a.funnel_rect(a.layout.cols[k]);
    let (x, y) = (f.x + f.w / 2.0, f.y + f.h / 2.0);
    a.on_event(&Event::MouseMove { x, y }, Instant::now());
    assert_eq!(a.head_hot, Some((k, true)));
    let mut g = Gfx::new(Mono, 1024);
    let t0 = Instant::now();
    assert!(a.frame(&mut g, t0), "the funnel's hover fades in");
    assert!(!a.frame(&mut g, t0 + Duration::from_millis(300)));
    assert_eq!(a.head_glow.alpha((k, true), t0 + Duration::from_millis(300)), 1.0);
    let x0 = a.layout.cols[k].x + 6.0;
    a.on_event(&Event::MouseMove { x: x0, y }, Instant::now());
    assert_eq!(a.head_hot, Some((k, false)), "the rest of the header lights as a column, not the funnel");
    a.on_event(&Event::MouseMove { x, y }, Instant::now());
    a.on_event(&Event::MouseDown { x, y, button: MouseButton::Left, clicks: 1 }, Instant::now());
    a.on_event(&Event::MouseUp { x, y, button: MouseButton::Left }, Instant::now());
    let m = a.drain_effects().find_map(|e| if let Effect::ShellMenu(m) = e { Some(m) } else { None }).expect("the types menu opens");
    assert_eq!((m.x, m.y), (f.x, f.bottom()));
    a.on_event(&Event::ShowMenu { items: m.menu, x: m.x, y: m.y, launches: Vec::new(), kind: m.kind, shell: Vec::new(), pending: false }, Instant::now());
    a.frame(&mut g, Instant::now());
    let panel = a.menu_panels()[0];
    assert!((panel.right() - f.right()).abs() <= 1.0, "{panel:?} {f:?}");
}

#[test]
fn chrome_hovers_fade_in_and_out_and_snap_without_animations() {
    let mut a = crate::ui::harness::app(&["a.txt"]);
    let mut g = Gfx::new(Mono, 1024);
    let t0 = Instant::now();
    a.frame(&mut g, t0);
    let r = a.layout.new_tab;
    a.on_event(&Event::MouseMove { x: r.x + r.w / 2.0, y: r.y + r.h / 2.0 }, t0);
    assert!(a.frame(&mut g, t0 + Duration::from_millis(1)));
    assert!(a.frame(&mut g, t0 + Duration::from_millis(40)));
    let mid = a.lit(Hit::NewTab);
    assert!(mid > 0.0 && mid < 1.0, "{mid}");
    assert!(matches!(a.button(Hit::NewTab, false, true), crate::ui::widgets::Button::Glow(_)));
    assert!(!a.frame(&mut g, t0 + Duration::from_millis(200)));
    assert_eq!(a.lit(Hit::NewTab), 1.0);
    a.on_event(&Event::MouseMove { x: 5.0, y: 500.0 }, t0 + Duration::from_millis(200));
    assert!(a.frame(&mut g, t0 + Duration::from_millis(201)), "and fades out");
    assert!(a.frame(&mut g, t0 + Duration::from_millis(240)));
    let out = a.lit(Hit::NewTab);
    assert!(out > 0.0 && out < 1.0);
    a.animations = false;
    a.on_event(&Event::MouseMove { x: r.x + r.w / 2.0, y: r.y + r.h / 2.0 }, t0 + Duration::from_millis(300));
    assert!(!a.frame(&mut g, t0 + Duration::from_millis(301)));
    assert_eq!(a.lit(Hit::NewTab), 1.0, "without Animations the hover is there at once");
}

#[test]
fn folder_cells_spin_while_their_sizes_load_and_settle_when_they_arrive() {
    let mut app = app_at(r"C:\x");
    run(&mut app, "view.auto_sizes");
    let fx = deliver_with(&mut app, &[("src", 0, true), ("a.txt", 1, false)], None);
    assert!(fx.iter().any(|e| matches!(e, Effect::Provider { req: ProviderRequest::DirSizes { .. }, .. })));
    let src = app.tab().view.as_ref().unwrap().rows()[0];
    assert!(app.tab().sizing(src));
    let mut g = Gfx::new(Mono, 1024);
    let row = app.row_rect(0).unwrap();
    let dots = |g: &Gfx<Mono>| g.dl.instances.iter().filter(|i| i.rect[1] >= row.y && i.rect[1] + i.rect[3] <= row.bottom() && i.rect[2] < 3.0 && i.rect[2] == i.rect[3]).count();
    assert!(app.frame(&mut g, Instant::now()), "a spinning cell asks for the next frame");
    let spinning = dots(&g);
    assert!(spinning >= 10, "the loading folder cells draw a spinner");
    let (tab, gen) = (app.tab().id, app.tab().gen);
    app.on_provider_event(ProviderEvent::DirSize { tab, gen, idx: src, stats: crate::listing::DirStats { size: 4096, folders: 2, files: 3, partial: false } }, Instant::now());
    assert!(!app.tab().sizing(src));
    app.frame(&mut g, Instant::now());
    assert!(dots(&g) + 10 <= spinning, "known sizes replace the spinner");
}
