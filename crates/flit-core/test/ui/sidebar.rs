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
            ("This PC", G_DRIVE, Section::Pinned),
            ("Debian (WSL)", G_LINUX, Section::Pinned),
            ("C:", G_DRIVE, Section::Pinned),
            ("proj", G_FOLDER, Section::Pinned),
            ("u", G_FOLDER, Section::Pinned),
            ("Music", G_MUSIC, Section::Folders),
            ("Disk (C:)", G_DRIVE, Section::Drives),
            ("Debian", G_LINUX, Section::Wsl),
        ]
    );
    assert_eq!(sb.items[6].usage, Some(0.0));
    assert!(sb.is_pinned(&loc(r"C:\")));
    assert!(!sb.is_pinned(&loc(r"C:\Users\u\Music")));
}

#[test]
fn known_folders_map_to_shell_names() {
    let names: Vec<&str> = KnownFolder::ALL.iter().map(|k| k.shell_name()).collect();
    assert_eq!(names, ["shell:Profile", "shell:Desktop", "shell:Downloads", "shell:Personal", "shell:My Pictures", "shell:My Music", "shell:My Video"]);
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
    assert_eq!(sb.rects.len(), 24);
    assert!(sb.content_h > sb.rect.h);
    let first = sb.rects[0].y;
    app.hover = Hit::Side(0);
    app.on_event(&crate::input::Event::Wheel { dx: 0.0, dy: -2.0 }, Instant::now());
    assert!(app.nav.sidebar.rects[0].y < first, "the wheel scrolls the sidebar, not the list");
    app.on_event(&crate::input::Event::Wheel { dx: 0.0, dy: -1000.0 }, Instant::now());
    let sb = &app.nav.sidebar;
    assert!((sb.rects.last().unwrap().bottom() + 6.0 + (28.0 + 8.0) - sb.rect.bottom()).abs() < 1.0, "clamped at the end, above the Settings button");
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
            MenuItem::Separator => Vec::new(),
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

#[test]
fn sections_collapse_hide_and_come_back() {
    let mut app = app_with_places();
    click(header(&app, Section::Drives), &mut app, MouseButton::Left);
    assert_eq!(side_labels(&app), ["a", "b", "Recycle Bin", "Debian"]);
    assert_eq!(app.nav_prefs().collapsed, ["drives"]);
    assert_eq!(app.nav.sidebar.rects.len(), app.nav.sidebar.items.len());
    click(header(&app, Section::Wsl), &mut app, MouseButton::Right);
    let m = menu(&mut app);
    assert!(!m.shell);
    assert_eq!(labels(&m.menu), ["Collapse", "Hide this section", "Pinned *", "Folders *", "Libraries *", "Drives *", "WSL *", "Tags *", "Compact sidebar"]);
    run(&mut app, &m, "Hide this section");
    assert!(!app.nav.sidebar.headers.iter().any(|h| h.1 == Section::Wsl));
    assert_eq!(side_labels(&app), ["a", "b", "Recycle Bin"]);
    click(header(&app, Section::Folders), &mut app, MouseButton::Right);
    let m = menu(&mut app);
    assert!(labels(&m.menu).contains(&"WSL".to_owned()));
    run(&mut app, &m, "WSL");
    assert!(app.nav.sidebar.headers.iter().any(|h| h.1 == Section::Wsl));
    click(header(&app, Section::Wsl), &mut app, MouseButton::Right);
    let m = menu(&mut app);
    run(&mut app, &m, "Hide this section");
    assert!(app.run_action("sidebar.sections", Instant::now()));
    assert!(app.nav_prefs().collapsed.is_empty() && app.nav_prefs().hidden.is_empty());
    assert_eq!(side_labels(&app).len(), 6);
}

#[test]
fn compact_mode_shows_icons_only_and_is_a_core_flag() {
    let mut app = app_with_places();
    let full = app.layout.list.x;
    assert!(app.run_action("sidebar.compact", Instant::now()));
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
    assert_eq!(labels(&m.menu), ["Open", "Open in new tab", "Open in new window", "Split vertically", "Split horizontally", "Copy path", "Open in terminal", "Pin to sidebar", "Eject", "Properties"]);
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
    assert_eq!(labels(&m.menu), ["Open", "Open in new tab", "Open in new window", "Split vertically", "Split horizontally", "Restore all items", "Empty Recycle Bin"]);
}

#[test]
fn dragging_folders_onto_pinned_pins_them_and_collapsed_sections_spring_open() {
    let mut app = app_with_places();
    let pinned = header(&app, Section::Pinned);
    let (x, y) = (pinned.x + 10.0, pinned.y + pinned.h / 2.0);
    let paths = vec![FsPath::parse(r"D:\new").unwrap(), FsPath::parse(r"D:\file.txt").unwrap()];
    let (t, all, none) = (Instant::now(), 7, Mods::default());
    app.drag(&DragEvent::Enter { paths: paths.clone(), x, y, mods: none, allowed: all }, t);
    assert_eq!(app.drop_caption(), Some(("Pin to %1".to_owned(), "Sidebar".to_owned())));
    assert_eq!(app.drop_spot(), Some(Spot::SideHeader(Section::Pinned)));
    assert_eq!(app.drag(&DragEvent::Drop { x, y, mods: none, allowed: all }, t), 4);
    assert!(app.drain_effects().any(|e| matches!(e, Effect::PinFolders(p) if p == paths)));
    app.pin_locations(vec![loc(r"D:\new"), loc(r"C:\a")]);
    assert_eq!(app.nav_state().pinned, [loc(r"C:\a"), loc(r"C:\b"), loc(r"D:\new")]);
    app.pin_locations(vec![loc(r"C:\a")]);
    assert!(app.tab().error.is_some(), "nothing new to pin says so");
    app.toggle_section(Section::Drives);
    let drives = header(&app, Section::Drives);
    let (x, y) = (drives.x + 10.0, drives.y + drives.h / 2.0);
    app.drag(&DragEvent::Enter { paths, x, y, mods: none, allowed: all }, t);
    assert_eq!(app.drag(&DragEvent::Over { x, y, mods: none, allowed: all }, t), 0);
    app.drag(&DragEvent::Over { x, y, mods: none, allowed: all }, t + Duration::from_millis(300));
    assert!(app.nav.sidebar.is_collapsed(Section::Drives));
    app.drag(&DragEvent::Over { x, y, mods: none, allowed: all }, t + Duration::from_millis(600));
    assert!(!app.nav.sidebar.is_collapsed(Section::Drives), "half a second of hovering expands the section");
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
    let asked: Vec<(Vec<u8>, IconKind)> = app.drain_effects().filter_map(|e| if let Effect::Icon(r) = e { Some((r.path.as_bytes().to_vec(), r.kind)) } else { None }).collect();
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
    assert_eq!(asked.len(), 7, "{asked:?}");
    assert!(app.icons.values().all(|s| *s == crate::ui::IconState::Pending));
}
