use super::*;
use crate::input::{Event, MouseButton};
use crate::text::tests::Mono;
use crate::ui::{DragEvent, MenuItem, Persisted, Spot};
use std::time::Duration;

fn loc(s: &str) -> Location {
    Location::parse(s).unwrap()
}

fn drive(letter: char) -> Drive {
    Drive { path: FsPath::parse(&format!("{letter}:\\")).unwrap(), label: format!("Disk ({letter}:)"), total: 100, free: 100, removable: false, unmounted: false, kind: None, remote: None }
}

#[test]
fn pinned_items_get_labels_and_glyphs() {
    let mut sb = Sidebar::default();
    sb.state.pinned = vec![Location::computer(), loc("wsl://Debian/"), loc(r"C:\"), loc(r"C:\work\proj"), loc("wsl://Debian/home/u")];
    sb.places = Arc::new(Places { folders: vec![(KnownFolder::Music, loc(r"C:\Users\u\Music"))], drives: vec![drive('C')], wsl: vec!["Debian".into()], ..Default::default() });
    sb.rebuild();
    let got: Vec<(&str, &str, Section)> = sb.items.iter().map(|it| (it.label.as_str(), it.glyph, it.section)).collect();
    assert_eq!(
        got,
        [
            ("This PC", G_DRIVE, Section::Bookmarks),
            ("Debian (WSL)", G_LINUX, Section::Bookmarks),
            ("C:", G_DRIVE, Section::Bookmarks),
            ("proj", G_FOLDER, Section::Bookmarks),
            ("u", G_FOLDER, Section::Bookmarks),
            ("Disk (C:)", G_DRIVE, Section::Storage),
            ("This PC", G_DRIVE, Section::Places),
            ("Music", G_MUSIC, Section::Places),
            ("Home", G_HOME, Section::Places),
            ("Debian", G_LINUX, Section::Wsl),
        ]
    );
    assert_eq!(sb.items[5].usage, Some(0.0));
    assert!(sb.is_pinned(&loc(r"C:\")));
    assert!(!sb.is_pinned(&loc(r"C:\Users\u\Music")));
}

#[test]
fn known_folders_map_to_shell_names() {
    let names: Vec<&str> = KnownFolder::ALL.iter().map(|k| k.shell_name()).collect();
    assert_eq!(names, ["shell:Profile", "shell:Desktop", "shell:Downloads", "shell:Personal", "shell:My Pictures", "shell:My Music", "shell:My Video", "shell:Favorites"]);
    assert_eq!(KnownFolder::ALL.map(|k| k.label())[0], "Home");
}

#[test]
fn layout_shows_the_pin_hint_and_scrolls_long_lists() {
    let mut app = App::new(Persisted::default(), Vec::new(), 0, false, 1.0, (900, 400), Instant::now());
    assert!(app.nav.sidebar.hint.is_some());
    assert!(app.nav.sidebar.rects.is_empty());
    let drives = ('C'..='Z').map(drive).collect();
    app.set_places(Arc::new(Places { drives, ..Default::default() }));
    let sb = &app.nav.sidebar;
    assert_eq!(sb.rects.len(), 26, "24 drives, This PC and Home");
    assert!(sb.content_h > sb.rect.h);
    let first = sb.rects[0].y;
    app.hover = Hit::Side(0);
    app.on_event(&crate::input::Event::Wheel { dx: 0.0, dy: -2.0 }, Instant::now());
    assert!(app.nav.sidebar.rects[0].y < first, "the wheel scrolls the sidebar, not the list");
    app.on_event(&crate::input::Event::Wheel { dx: 0.0, dy: -1000.0 }, Instant::now());
    let sb = &app.nav.sidebar;
    let end = sb.rects.last().unwrap().bottom();
    assert!(end <= sb.list.bottom() + 0.5 && end >= sb.list.bottom() - SECTION_GAP - 1.0, "clamped at the end of the list, above the Settings button: {end} {:?}", sb.list);
    assert!(sb.rects.last().unwrap().bottom() < sb.settings.y);
    assert!(matches!(app.sidebar_hit(sb.rects[23].x + 5.0, sb.rects[23].y + 5.0), Some(Hit::Side(23))));
    assert!(app.sidebar_hit(sb.rect.right() + 50.0, sb.rect.y + 5.0).is_none());
}

#[test]
fn opening_an_unmounted_volume_asks_the_platform_to_mount_it() {
    let mut app = App::new(Persisted::default(), Vec::new(), 0, false, 1.0, (1000, 700), Instant::now());
    let dev = FsPath::parse("/dev/sdb1").unwrap();
    app.set_places(Arc::new(Places { drives: vec![Drive { path: dev.clone(), label: "STICK".into(), total: 0, free: 0, removable: true, unmounted: true, kind: None, remote: None }], ..Default::default() }));
    app.drain_effects().for_each(drop);
    let i = app.nav.sidebar.items.iter().position(|it| it.loc.path() == &dev).unwrap();
    app.open_side_item(i, false, Instant::now());
    let mounts: Vec<_> = app.drain_effects().filter_map(|e| match e {
        Effect::Provider { req: ProviderRequest::Launch { launch: Launch::Mount(d), .. }, .. } => Some(d),
        _ => None,
    }).collect();
    assert_eq!(mounts, [dev]);
}

fn app_with_places() -> App {
    let mut app = App::new(Persisted::default(), Vec::new(), 0, false, 1.0, (1000, 700), Instant::now());
    let usb = Drive { removable: true, label: "USB (E:)".into(), path: FsPath::parse(r"E:\").unwrap(), total: 10, free: 5, unmounted: false, kind: None, remote: None };
    app.set_places(Arc::new(Places { drives: vec![drive('C'), usb], wsl: vec!["Debian".into()], recycle_bin: true, ..Default::default() }));
    app.set_pinned(vec![loc(r"C:\a"), loc(r"C:\b")]);
    app.drain_effects().for_each(drop);
    app
}

fn click(r: Rect, app: &mut App, button: MouseButton) {
    let (x, y) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
    app.on_event(&Event::MouseMove { x, y }, Instant::now());
    app.on_event(&Event::MouseDown { x, y, button, clicks: 1 }, Instant::now());
    app.on_event(&Event::MouseUp { x, y, button }, Instant::now());
}

fn header(app: &App, s: Section) -> Rect {
    app.nav.sidebar.headers.iter().find(|h| h.1 == s).map(|h| h.0).unwrap_or_else(|| panic!("no header {s:?}"))
}

fn menu(app: &mut App) -> ShellMenu {
    app.drain_effects().find_map(|e| if let Effect::ShellMenu(m) = e { Some(m) } else { None }).expect("menu shown")
}

fn labels(items: &[MenuItem]) -> Vec<String> {
    items
        .iter()
        .flat_map(|i| match i {
            MenuItem::Command { label, checked, .. } => vec![format!("{label}{}", if *checked { " *" } else { "" })],
            MenuItem::Submenu { items, .. } => labels(items),
            MenuItem::Header(_) | MenuItem::Separator => Vec::new(),
        })
        .collect()
}

fn run(app: &mut App, m: &ShellMenu, label: &str) {
    let all = labels(&m.menu);
    let k = all.iter().position(|c| c.trim_end_matches(" *") == label).unwrap_or_else(|| panic!("{label} not in {all:?}"));
    app.on_event(&Event::MenuCommand(k), Instant::now());
}

fn side_labels(app: &App) -> Vec<&str> {
    app.nav.sidebar.items.iter().map(|it| it.label.as_str()).collect()
}

fn empty_area(app: &App) -> Rect {
    let list = app.nav.sidebar.list;
    Rect::new(list.x, list.bottom() - 12.0, list.w, 8.0)
}

fn disabled(items: &[MenuItem]) -> Vec<String> {
    items.iter().filter_map(|i| if let MenuItem::Command { label, disabled: true, .. } = i { Some(label.clone()) } else { None }).collect()
}

#[test]
fn headers_fold_and_the_empty_area_menu_hides_sections() {
    let mut app = app_with_places();
    assert_eq!(side_labels(&app), ["a", "b", "Disk (C:)", "USB (E:)", "This PC", "Recycle Bin", "Home", "Debian"]);
    click(header(&app, Section::Storage), &mut app, MouseButton::Left);
    assert_eq!(side_labels(&app), ["a", "b", "This PC", "Recycle Bin", "Home", "Debian"]);
    assert_eq!(app.nav_prefs().collapsed, ["drives"]);
    assert_eq!(app.nav.sidebar.rects.len(), app.nav.sidebar.items.len());
    click(empty_area(&app), &mut app, MouseButton::Right);
    let m = menu(&mut app);
    assert!(!m.shell);
    let all = labels(&m.menu);
    for want in ["Recents *", "Bookmarks *", "Storage *", "Places *", "Libraries *", "WSL *", "Tags *", "Compact sidebar"] {
        assert!(all.contains(&want.to_owned()), "{want} not in {all:?}");
    }
    run(&mut app, &m, "WSL");
    assert!(!app.nav.sidebar.headers.iter().any(|h| h.1 == Section::Wsl));
    assert!(app.run_action("quick_access.sections", Instant::now()));
    assert!(app.nav_prefs().collapsed.is_empty() && app.nav_prefs().hidden.is_empty());
    assert_eq!(side_labels(&app).len(), 8);
}

fn stays(items: &[MenuItem]) -> Vec<String> {
    items
        .iter()
        .flat_map(|i| match i {
            MenuItem::Command { label, stay: true, .. } => vec![label.clone()],
            MenuItem::Submenu { items, .. } => stays(items),
            _ => Vec::new(),
        })
        .collect()
}

#[test]
fn section_toggles_keep_the_menu_open_and_update_the_sidebar_behind_it() {
    let mut app = app_with_places();
    click(empty_area(&app), &mut app, MouseButton::Right);
    let m = menu(&mut app);
    assert_eq!(stays(&m.menu), ["Recents", "Bookmarks", "Storage", "Places", "Libraries", "WSL", "Tags", "Compact sidebar"], "only the show/hide toggles stay open");
    app.on_event(&Event::ShowMenu { items: m.menu, x: 300.0, y: 100.0, launches: Vec::new() }, Instant::now());
    let mut g = Gfx::new(Mono, 1024);
    app.frame(&mut g, Instant::now());
    let key = |app: &mut App, key: Key| app.on_event(&Event::Key { key, mods: Mods::default(), pressed: true }, Instant::now());
    key(&mut app, Key::End);
    key(&mut app, Key::Enter);
    assert!(app.own_menu_open(), "toggling Compact leaves the menu open");
    assert!(app.nav_prefs().compact);
    app.frame(&mut g, Instant::now());
    for k in [Key::Up, Key::Right, Key::Down, Key::Down, Key::Enter] {
        key(&mut app, k);
    }
    assert!(app.own_menu_open(), "hiding a section leaves the menu open");
    assert!(app.nav.sidebar.is_hidden(Section::Storage));
    assert!(!side_labels(&app).contains(&"Disk (C:)"), "the sidebar behind the menu follows");
    key(&mut app, Key::Enter);
    assert!(app.own_menu_open() && !app.nav.sidebar.is_hidden(Section::Storage), "the same row toggles back");
    key(&mut app, Key::Escape);
    key(&mut app, Key::Up);
    key(&mut app, Key::Enter);
    assert!(!app.own_menu_open(), "ordinary commands still close the menu");
}

#[test]
fn the_header_menu_folds_shifts_shows_paths_and_clears() {
    let mut app = app_with_places();
    click(header(&app, Section::Bookmarks), &mut app, MouseButton::Right);
    let m = menu(&mut app);
    assert_eq!(labels(&m.menu), ["Collapse All", "Expand All", "Show Paths", "Shift Group Up", "Shift Group Down", "Restore Groups Order", "Clear Bookmarks"]);
    assert_eq!(disabled(&m.menu), ["Expand All", "Shift Group Up", "Restore Groups Order"], "Recents is empty, so Bookmarks is the first group shown");
    run(&mut app, &m, "Shift Group Down");
    assert_eq!(app.nav_prefs().order()[..4], [Section::Recents, Section::Storage, Section::Bookmarks, Section::Places]);
    assert_eq!(side_labels(&app)[..3], ["Disk (C:)", "USB (E:)", "a"]);
    click(header(&app, Section::Storage), &mut app, MouseButton::Right);
    let m = menu(&mut app);
    assert!(!disabled(&m.menu).contains(&"Restore Groups Order".to_owned()));
    assert!(!labels(&m.menu).iter().any(|l| l.starts_with("Clear")), "Storage has no last group");
    run(&mut app, &m, "Restore Groups Order");
    assert_eq!(app.nav_prefs().order(), Section::ALL);
    click(header(&app, Section::Places), &mut app, MouseButton::Right);
    let m = menu(&mut app);
    run(&mut app, &m, "Collapse All");
    assert!(side_labels(&app).is_empty());
    click(header(&app, Section::Places), &mut app, MouseButton::Right);
    let m = menu(&mut app);
    run(&mut app, &m, "Expand All");
    assert_eq!(side_labels(&app).len(), 8);
    click(header(&app, Section::Places), &mut app, MouseButton::Right);
    let m = menu(&mut app);
    run(&mut app, &m, "Show Paths");
    assert!(app.nav_prefs().quick.show_paths);
    assert_eq!(app.nav.sidebar.items[0].path, r"C:\a");
    click(header(&app, Section::Bookmarks), &mut app, MouseButton::Right);
    let m = menu(&mut app);
    run(&mut app, &m, "Clear Bookmarks");
    assert!(app.nav_state().pinned.is_empty());
    assert!(app.run_action("bookmarks.clear", Instant::now()));
}

#[test]
fn recent_files_and_folders_have_their_own_capped_group() {
    let mut app = app_with_places();
    let recent = |k: i64, dir: bool| Recent { name: format!("r{k}"), target: FsPath::parse(&format!(r"C:\r\r{k}")).unwrap(), dir, at: 100 - k };
    let mut places = (*app.nav.sidebar.places).clone();
    places.recent = (0..14).map(|k| recent(k, k % 2 == 0)).collect();
    app.set_places(Arc::new(places));
    app.drain_effects().for_each(drop);
    let labels: Vec<String> = side_labels(&app).into_iter().map(str::to_owned).collect();
    assert_eq!(labels[..3], ["r0", "r1", "r2"], "Recents come first");
    assert_eq!(app.nav.sidebar.items.iter().filter(|it| it.section == Section::Recents).count(), 10);
    assert!(app.nav.sidebar.headers.iter().any(|h| h.1 == Section::Recents));
    app.frame(&mut Gfx::new(Mono, 1024), Instant::now());
    let asked: Vec<(Vec<u8>, IconKind)> = app.drain_effects().filter_map(|e| if let Effect::Icon(r) = e { Some((r.path.as_bytes().to_vec(), r.kind)) } else { None }).collect();
    assert!(asked.contains(&(br"C:\r\r0".to_vec(), IconKind::Place)) && asked.contains(&(br"C:\r\r1".to_vec(), IconKind::File)), "{asked:?}");
    let file = labels.iter().position(|l| *l == "r1").unwrap();
    app.open_side_item(file, false, Instant::now());
    assert!(app.drain_effects().any(|e| matches!(e, Effect::Provider { req: ProviderRequest::Open { path, .. }, .. } if path == FsPath::parse(r"C:\r\r1").unwrap())));
    let a = labels.iter().position(|l| *l == "a").unwrap();
    assert!(!app.nav.sidebar.removable(file) && app.nav.sidebar.removable(a), "only bookmarks have a remove button");
    click(app.nav.sidebar.rects[file], &mut app, MouseButton::Right);
    let m = menu(&mut app);
    run(&mut app, &m, "Remove from the list");
    assert!(!side_labels(&app).contains(&"r1"));
    assert_eq!(app.nav.sidebar.items.iter().filter(|it| it.section == Section::Recents).count(), 10, "the next entry moves up");
    let folder = side_labels(&app).iter().position(|l| *l == "r0").unwrap();
    app.open_side_item(folder, false, Instant::now());
    assert_eq!(app.current_location(), &loc(r"C:\r\r0"));
    click(header(&app, Section::Recents), &mut app, MouseButton::Right);
    let m = menu(&mut app);
    run(&mut app, &m, "Clear Recents");
    assert!(!app.nav.sidebar.items.iter().any(|it| it.section == Section::Recents));
}

#[test]
fn section_header_labels_and_chevrons_sit_in_the_middle_of_their_hover_rect() {
    for scale in [1.0, 1.25, 1.5, 1.75, 2.0] {
        let mut app = App::new(Persisted::default(), Vec::new(), 0, false, scale, (1200, 2000), Instant::now());
        app.set_places(Arc::new(Places { drives: vec![drive('C')], wsl: vec!["Debian".into()], recycle_bin: true, ..Default::default() }));
        let mut g = Gfx::new(Mono, 1024);
        let m = g.metrics(app.header_font());
        assert!(app.nav.sidebar.headers.len() >= 4, "{scale}");
        for &(r, section) in &app.nav.sidebar.headers {
            let mid = r.y + r.h / 2.0;
            let (icon, _, baseline, chevron) = header_parts(r, m.ascent, m.descent, scale);
            let text_mid = baseline + (m.descent - m.ascent) / 2.0;
            assert!((text_mid - mid).abs() <= 0.5, "{section:?} at {scale}: label centre {text_mid} vs {mid}");
            assert_eq!((chevron.y, chevron.h), (r.y, r.h), "{section:?} at {scale}: the chevron shares the hover rect");
            assert_eq!((icon.y, icon.h), (r.y, r.h), "{section:?} at {scale}: the header glyph shares the hover rect");
        }
    }
}

#[test]
fn compact_mode_shows_icons_only_and_is_a_core_flag() {
    let mut app = app_with_places();
    let full = app.layout.list.x;
    assert!(app.run_action("quick_access.compact", Instant::now()));
    assert!(app.nav_prefs().compact);
    assert_eq!(app.layout.list.x, COMPACT_W);
    assert!(app.layout.list.x < full);
    assert_eq!(app.nav.sidebar.splitter, Rect::default(), "no splitter in compact mode");
    let r = app.nav.sidebar.rects[0];
    assert!(matches!(app.sidebar_hit(r.right() - 3.0, r.y + 3.0), Some(Hit::Side(0))), "no pin button in compact mode");
    app.frame(&mut Gfx::new(Mono, 1024), Instant::now());
    click(r, &mut app, MouseButton::Left);
    assert_eq!(app.current_location(), &loc(r"C:\a"));
}

#[test]
fn item_menus_offer_place_commands() {
    let mut app = app_with_places();
    let usb = side_labels(&app).iter().position(|l| *l == "USB (E:)").unwrap();
    click(app.nav.sidebar.rects[usb], &mut app, MouseButton::Right);
    let m = menu(&mut app);
    assert!(m.shell, "items keep Show more options");
    assert_eq!(m.items, [FsPath::parse(r"E:\").unwrap()]);
    assert_eq!(labels(&m.menu), ["Open", "Open in new tab", "Open in new window", "Open In Right Split", "Open In Bottom Split", "Copy path", "Open in terminal", "Pin to sidebar", "Eject", "Properties"]);
    run(&mut app, &m, "Eject");
    assert!(app.drain_effects().any(|e| matches!(e, Effect::Provider { req: ProviderRequest::Launch { launch: Launch::Eject(d), .. }, .. } if d == FsPath::parse(r"E:\").unwrap())));
    click(app.nav.sidebar.rects[0], &mut app, MouseButton::Right);
    let m = menu(&mut app);
    assert!(labels(&m.menu).contains(&"Move down".to_owned()) && !labels(&m.menu).contains(&"Move up".to_owned()));
    run(&mut app, &m, "Move down");
    assert_eq!(app.nav_state().pinned, [loc(r"C:\b"), loc(r"C:\a")]);
    assert!(app.drain_effects().any(|e| matches!(e, Effect::Pinned(p) if p.len() == 2)));
    click(app.nav.sidebar.rects[1], &mut app, MouseButton::Right);
    let m = menu(&mut app);
    run(&mut app, &m, "Open in new window");
    assert!(app.drain_effects().any(|e| matches!(e, Effect::NewWindow(r) if r[0].folder == loc(r"C:\a"))));
    let bin = side_labels(&app).iter().position(|l| *l == "Recycle Bin").unwrap();
    click(app.nav.sidebar.rects[bin], &mut app, MouseButton::Right);
    let m = menu(&mut app);
    assert!(!m.shell);
    assert_eq!(labels(&m.menu), ["Open", "Open in new tab", "Open in new window", "Open In Right Split", "Open In Bottom Split", "Restore all items", "Empty Recycle Bin"]);
}

#[test]
fn dragging_folders_onto_pinned_pins_them_and_collapsed_sections_spring_open() {
    let mut app = app_with_places();
    let pinned = header(&app, Section::Bookmarks);
    let (x, y) = (pinned.x + 10.0, pinned.y + pinned.h / 2.0);
    let paths = vec![FsPath::parse(r"D:\new").unwrap(), FsPath::parse(r"D:\file.txt").unwrap()];
    let (t, all, none) = (Instant::now(), 7, Mods::default());
    app.drag(&DragEvent::Enter { paths: paths.clone(), x, y, mods: none, allowed: all }, t);
    assert_eq!(app.drop_caption(), Some(("Pin to %1".to_owned(), "Sidebar".to_owned())));
    assert_eq!(app.drop_spot(), Some(Spot::SideHeader(Section::Bookmarks)));
    assert_eq!(app.drag(&DragEvent::Drop { x, y, mods: none, allowed: all }, t), 4);
    assert!(app.drain_effects().any(|e| matches!(e, Effect::PinFolders(p) if p == paths)));
    app.pin_locations(vec![loc(r"D:\new"), loc(r"C:\a")]);
    assert_eq!(app.nav_state().pinned, [loc(r"C:\a"), loc(r"C:\b"), loc(r"D:\new")]);
    app.pin_locations(vec![loc(r"C:\a")]);
    assert!(app.tab().error.is_some(), "nothing new to pin says so");
    app.toggle_section(Section::Storage);
    let drives = header(&app, Section::Storage);
    let (x, y) = (drives.x + 10.0, drives.y + drives.h / 2.0);
    app.drag(&DragEvent::Enter { paths, x, y, mods: none, allowed: all }, t);
    assert_eq!(app.drag(&DragEvent::Over { x, y, mods: none, allowed: all }, t), 0);
    app.drag(&DragEvent::Over { x, y, mods: none, allowed: all }, t + Duration::from_millis(300));
    assert!(app.nav.sidebar.is_collapsed(Section::Storage));
    app.drag(&DragEvent::Over { x, y, mods: none, allowed: all }, t + Duration::from_millis(600));
    assert!(!app.nav.sidebar.is_collapsed(Section::Storage), "half a second of hovering expands the section");
}

#[test]
fn nav_prefs_are_sanitized() {
    let p = NavPrefs { collapsed: vec!["x".into(); 1000], recent_hidden: (0..1000).map(|k| (FsPath::parse(r"C:\a").unwrap(), k)).collect(), ..Default::default() }.sanitized();
    assert_eq!(p.collapsed.len(), MAX_LIST);
    assert_eq!(p.recent_hidden.len(), MAX_LIST);
    assert_eq!(p.recent_hidden[0].1, 1000 - MAX_LIST as i64, "the newest are kept");
}

#[test]
fn distros_in_the_sidebar_show_their_own_icons() {
    let mut app = app_with_places();
    app.frame(&mut Gfx::new(Mono, 1024), Instant::now());
    let asked: Vec<(Vec<u8>, IconKind)> = app.drain_effects().filter_map(|e| if let Effect::Icon(r) = e { Some((r.path.as_bytes().to_vec(), r.kind)) } else { None }).collect();
    assert!(asked.contains(&(br"\\wsl.localhost\Debian\".to_vec(), IconKind::Volume)), "{asked:?}");
    app.frame(&mut Gfx::new(Mono, 1024), Instant::now());
    assert!(!app.drain_effects().any(|e| matches!(e, Effect::Icon(r) if r.kind == IconKind::Volume)), "a pending icon is asked once");
}

#[test]
fn places_ask_the_system_for_their_icons_and_keep_glyphs_until_then() {
    let mut app = app_with_places();
    let offline = Drive { path: FsPath::parse(r"Z:\").unwrap(), label: "Share (Z:)".into(), total: 0, free: 0, removable: false, unmounted: true, kind: None, remote: Some(r"\\srv\share".into()) };
    let downloads = loc(r"C:\Users\u\Downloads");
    let mut places = (*app.nav.sidebar.places).clone();
    places.folders = vec![(KnownFolder::Downloads, downloads.clone())];
    places.drives.push(offline);
    app.set_places(Arc::new(places));
    let mut store = crate::tags::TagStore { tags: Vec::new(), files: Default::default() };
    store.add("Red", 0xFF0000).unwrap();
    app.set_tags(Arc::new(Tags::new(1, &store, &Default::default())));
    app.drain_effects().for_each(drop);
    app.frame(&mut Gfx::new(Mono, 1024), Instant::now());
    let asked: Vec<(Vec<u8>, IconKind)> = app.drain_effects().filter_map(|e| if let Effect::Icon(r) = e { (r.size == 20).then_some((r.path.as_bytes().to_vec(), r.kind)) } else { None }).collect();
    for want in [
        (br"C:\a".to_vec(), IconKind::Place),
        (downloads.path().as_bytes().to_vec(), IconKind::Place),
        (FsPath::recycle_bin().as_bytes().to_vec(), IconKind::Place),
        (br"C:\".to_vec(), IconKind::Volume),
        (br"E:\".to_vec(), IconKind::Volume),
        (br"\\wsl.localhost\Debian\".to_vec(), IconKind::Volume),
    ] {
        assert!(asked.contains(&want), "{want:?} not in {asked:?}");
    }
    assert!(!asked.iter().any(|(p, _)| p == br"Z:\"), "an offline share is not touched");
    assert_eq!(asked.len(), 8, "{asked:?}");
    assert!(app.icons.values().all(|s| *s == crate::ui::IconState::Pending));
}

#[test]
fn every_section_header_has_its_own_glyph_before_the_title() {
    let glyphs: Vec<&str> = Section::ALL.iter().map(|s| s.glyph()).collect();
    let mut unique = glyphs.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), Section::ALL.len(), "{glyphs:?}");
    let app = app_with_places();
    for &(r, section) in &app.nav.sidebar.headers {
        let (icon, title_x, _, chevron) = header_parts(r, 10.0, 3.0, 1.0);
        assert!(r.x < icon.x && icon.right() <= title_x && title_x < chevron.x, "{section:?}");
    }
}

#[test]
fn items_are_indented_so_their_icons_line_up_with_the_header_titles() {
    for scale in [1.0, 1.5, 2.0] {
        let mut app = App::new(Persisted::default(), Vec::new(), 0, false, scale, (1200, 900), Instant::now());
        app.set_places(Arc::new(Places { drives: vec![drive('C')], recycle_bin: true, ..Default::default() }));
        let sb = &app.nav.sidebar;
        let (header_icon, title_x, _, _) = header_parts(sb.headers[0].0, 10.0, 3.0, scale);
        for &r in &sb.rects {
            let (icon, text_x) = item_parts(r, scale);
            let glyph_left = icon.x + (icon.w - (16.0 * scale).round()) / 2.0;
            assert!(glyph_left > header_icon.right(), "{scale}: items sit right of the header glyph");
            assert!((glyph_left - title_x).abs() <= 2.0 * scale, "{scale}: icon {glyph_left} vs title {title_x}");
            assert!(text_x > icon.right() - 1.0);
            assert_eq!(r.x, sb.headers[0].0.x, "hit rects stay full width");
        }
    }
}

#[test]
fn drive_icons_center_on_the_whole_row_with_its_bar_and_other_icons_on_the_label() {
    for scale in [1.0, 1.5] {
        let mut app = App::new(Persisted::default(), Vec::new(), 0, false, scale, (1200, 900), Instant::now());
        app.set_places(Arc::new(Places { drives: vec![drive('C')], recycle_bin: true, ..Default::default() }));
        let sb = &app.nav.sidebar;
        let tall = sb.rects.iter().copied().find(|r| r.h > (28.0 * scale).round()).expect("a drive row with a bar");
        let icon = item_icon(tall, scale, true);
        assert_eq!(icon.y + icon.h / 2.0, tall.y + tall.h / 2.0, "{scale}: the drive icon sits between its label and its bar");
        let plain = sb.rects.iter().copied().find(|r| r.h <= (28.0 * scale).round()).unwrap();
        assert_eq!(item_icon(plain, scale, false), item_parts(plain, scale).0);
    }
}

fn drive_bars(app: &App) -> Vec<(Rect, Rect)> {
    let sb = &app.nav.sidebar;
    sb.items.iter().zip(&sb.rects).filter(|(it, _)| it.usage.is_some()).map(|(_, &r)| (r, app.drive_bar(r))).collect()
}

#[test]
fn drive_bars_share_the_widest_label_width_and_ignore_the_sidebar_width() {
    let mut app = app_with_places();
    let mut g = Gfx::new(Mono, 1024);
    let font = app.ui_font(13.0);
    let widest = app.nav.sidebar.items.iter().filter(|it| it.usage.is_some()).map(|it| g.measure(&it.label, font)).fold(0.0, f32::max);
    assert!(widest > 0.0);
    let mut widths = Vec::new();
    for w in [240.0, 420.0] {
        let mut st = app.nav_state();
        st.sidebar_w = w;
        app.set_nav_state(st);
        app.frame(&mut g, Instant::now());
        assert_eq!(app.nav.sidebar.bar_w, widest.ceil() + BAR_MARGIN);
        let bars = drive_bars(&app);
        assert_eq!(bars.len(), 2);
        for (r, b) in &bars {
            assert_eq!(b.x, item_parts(*r, 1.0).1, "the bar starts under the label");
            assert_eq!(b.w, bars[0].1.w, "every drive bar has the same width");
        }
        widths.push(bars[0].1.w);
    }
    assert_eq!(widths, [widest.ceil() + BAR_MARGIN; 2], "a wider sidebar does not stretch the bars");
}

#[test]
fn drives_set_the_minimum_sidebar_width() {
    let mut app = App::new(Persisted::default(), Vec::new(), 0, false, 1.0, (1200, 900), Instant::now());
    let mut g = Gfx::new(Mono, 1024);
    app.frame(&mut g, Instant::now());
    assert_eq!(app.nav.sidebar.min_w(), SIDEBAR_MIN_W, "the floor holds without drives");
    let mut st = app.nav_state();
    st.sidebar_w = SIDEBAR_MIN_W;
    app.set_nav_state(st);
    let long = Drive { label: "A very long volume label for bars (D:)".into(), ..drive('D') };
    app.set_places(Arc::new(Places { drives: vec![drive('C'), long], ..Default::default() }));
    app.frame(&mut g, Instant::now());
    let min = app.nav.sidebar.min_w();
    assert!(min > SIDEBAR_MIN_W && min == (78.0 + app.nav.sidebar.bar_w).ceil(), "{min}");
    assert_eq!(app.nav_state().sidebar_w, min, "a narrower sidebar grows to fit the bars");
    for (r, bar) in drive_bars(&app) {
        assert_eq!(bar.w, app.nav.sidebar.bar_w, "the bar fits uncut");
        assert!(bar.right() <= r.right() - 12.0 + 0.01);
    }
    app.begin_splitter_drag(min);
    app.drag_splitter(0.0, min, min);
    assert_eq!(app.nav_state().sidebar_w, min, "the splitter stops at the drive minimum");
}

fn type_text(app: &mut App, text: &str) {
    for c in text.chars() {
        app.on_event(&Event::Char(c), Instant::now());
    }
}

#[test]
fn alt_f_filters_quick_access_with_marks_and_escape_clears_it() {
    let mut app = app_with_places();
    let alt = Mods { ctrl: false, shift: false, alt: true };
    app.on_event(&Event::Key { key: Key::Char('F'), mods: alt, pressed: true }, Instant::now());
    assert_eq!(app.focus, Focus::Sidebar);
    assert!(app.nav.sidebar.typing);
    type_text(&mut app, "dsk");
    assert_eq!(side_labels(&app), ["Disk (C:)"]);
    assert_eq!(app.nav.sidebar.items[0].marks, [0..1, 2..4]);
    assert!(app.nav.sidebar.headers.iter().all(|h| h.1 == Section::Storage), "groups without matches are not shown");
    frame(&mut app);
    let c = app.nav.sidebar.clear;
    assert!(c.w > 0.0);
    assert_eq!(app.hit(c.x + c.w / 2.0, c.y + c.h / 2.0), Hit::QuickClear);
    click(header(&app, Section::Storage), &mut app, MouseButton::Left);
    assert_eq!(side_labels(&app), ["Disk (C:)"], "a folded group still shows its matches while filtering");
    app.on_event(&Event::Key { key: Key::Escape, mods: Mods::default(), pressed: true }, Instant::now());
    assert!(app.nav.sidebar.filter.text().is_empty());
    assert!(!side_labels(&app).contains(&"Disk (C:)"), "the fold applies again once the filter is gone");
    let f = app.nav.sidebar.field;
    assert_eq!(app.hit(f.x + f.w / 2.0, f.y + f.h / 2.0), Hit::QuickField);
    click(f, &mut app, MouseButton::Left);
    assert!(app.nav.sidebar.typing && app.focus == Focus::Sidebar);
    type_text(&mut app, "zz");
    assert!(side_labels(&app).is_empty());
    click(app.nav.sidebar.clear, &mut app, MouseButton::Left);
    assert!(app.nav.sidebar.filter.text().is_empty());
}

#[test]
fn short_paths_drop_the_home_parent_and_trailing_separators() {
    let home = loc(r"C:\Users\brano");
    assert_eq!(short_path(&loc(r"C:\Users\brano\Downloads"), Some(&home)), r"brano\Downloads");
    assert_eq!(short_path(&loc(r"c:\users\brano"), Some(&home)), "brano");
    assert_eq!(short_path(&loc(r"C:\"), Some(&home)), "C:");
    assert_eq!(short_path(&loc(r"D:\work"), Some(&home)), r"D:\work");
    assert_eq!(short_path(&loc(r"C:\Usersx\b"), Some(&home)), r"C:\Usersx\b");
    assert_eq!(short_path(&Location::computer(), Some(&home)), "");
    assert_eq!(short_path(&Location::recycle_bin(), None), "");
}

#[test]
fn bookmarks_have_a_trash_button_that_asks_first() {
    let mut app = app_with_places();
    frame(&mut app);
    let r = app.nav.sidebar.rects[0];
    app.on_event(&Event::MouseMove { x: r.x + 5.0, y: r.y + 5.0 }, Instant::now());
    let pin = app.nav.sidebar.pin_rect(r, app.scale);
    assert_eq!(app.hit(pin.x + pin.w / 2.0, pin.y + pin.h / 2.0), Hit::SidePin(0));
    assert_eq!(app.tip_text(Hit::SidePin(0)).unwrap().line, "Remove this bookmark.");
    click(pin, &mut app, MouseButton::Left);
    assert_eq!(app.nav_state().pinned.len(), 2, "removing asks first");
    app.on_event(&Event::Key { key: Key::Enter, mods: Mods::default(), pressed: true }, Instant::now());
    assert_eq!(app.nav_state().pinned, [loc(r"C:\b")]);
    let drive = side_labels(&app).iter().position(|l| *l == "Disk (C:)").unwrap();
    assert!(!app.nav.sidebar.removable(drive));
}

#[test]
fn visits_are_counted_and_the_five_most_visited_are_offered() {
    let mut app = App::new(Persisted { path: loc(r"C:\start"), ..Default::default() }, Vec::new(), 0, false, 1.0, (1000, 700), Instant::now());
    for (path, times) in [(r"C:\a", 3), (r"C:\b", 1), (r"C:\c", 5), (r"C:\d", 2), (r"C:\e", 4), (r"C:\f", 6)] {
        for _ in 0..times {
            app.navigate(loc(path), Instant::now());
            app.navigate(loc(r"C:\start"), Instant::now());
        }
    }
    app.go_home_page(false, Instant::now());
    assert!(!app.nav_prefs().quick.visits.iter().any(|(l, _)| l.is_home_page()), "the Home page is not a folder visit");
    let top: Vec<String> = app.nav_prefs().most_visited().iter().map(|l| l.display().into_owned()).collect();
    assert_eq!(top, [r"C:\f", r"C:\c", r"C:\e", r"C:\a", r"C:\d"]);
    app.drain_effects().for_each(drop);
    let r = app.layout.nav[3];
    click(r, &mut app, MouseButton::Left);
    let m = menu(&mut app);
    assert_eq!(labels(&m.menu), ["f", "c", "e", "a", "d"]);
    run(&mut app, &m, "c");
    assert_eq!(app.current_location(), &loc(r"C:\c"));
}

#[test]
fn quick_prefs_are_sanitized() {
    let mut p = NavPrefs::default();
    p.quick.order = (0..100).map(|k| format!("x{k}")).collect();
    p.quick.visits = (0..200).map(|k| (loc(&format!(r"C:\v{k}")), 1)).collect();
    let p = p.sanitized();
    assert_eq!(p.quick.order.len(), 100, "short lists are kept as they are");
    assert!(p.quick.visits.len() <= 64);
    assert_eq!(p.order(), Section::ALL);
}

fn frame(app: &mut App) {
    app.frame(&mut Gfx::new(Mono, 1024), Instant::now());
}
