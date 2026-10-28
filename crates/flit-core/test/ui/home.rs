use super::*;
use crate::input::{Event, MouseButton};
use crate::provider::ProviderRequest;
use crate::text::tests::Mono;
use crate::ui::{Drive, MenuItem, Persisted, Places, ShellMenu};
use std::sync::Arc;

fn loc(s: &str) -> Location {
    Location::parse(s).unwrap()
}

fn p(s: &str) -> FsPath {
    FsPath::parse(s).unwrap()
}

fn recent(name: &str, target: &str, dir: bool, at: i64) -> Recent {
    Recent { name: name.into(), target: p(target), dir, at }
}

fn places() -> Arc<Places> {
    Arc::new(Places {
        folders: vec![(KnownFolder::Home, loc(r"C:\Users\u")), (KnownFolder::Downloads, loc(r"C:\Users\u\Downloads"))],
        drives: vec![Drive { path: p(r"C:\"), label: "Local Disk (C:)".into(), total: 1000, free: 250, removable: false, unmounted: false, kind: None, remote: None }],
        recent: vec![recent("report.docx", r"C:\Users\u\Documents\report.docx", false, 300), recent("proj", r"C:\work\proj", true, 200), recent("old.txt", r"C:\old.txt", false, 100)],
        ..Default::default()
    })
}

fn home_app() -> App {
    let mut app = App::new(Persisted { path: Location::home_page(), ..Default::default() }, Vec::new(), 0, false, 1.0, (1000, 700), Instant::now());
    app.set_places(places());
    app.set_pinned(vec![loc(r"C:\work")]);
    app.drain_effects().for_each(drop);
    app
}

fn cards(app: &App) -> Vec<String> {
    app.nav.home.cards.iter().map(|(_, c)| match c {
        Card::Folder { label, .. } => format!("folder {label}"),
        Card::Drive(d) => format!("drive {d}"),
        Card::Tag(k) => format!("tag {}", app.nav.sidebar.tags.defs[*k].name),
        Card::Recent(r) => format!("recent {}", app.nav.sidebar.places.recent[*r].name),
    }).collect()
}

fn click(r: Rect, app: &mut App, button: MouseButton) {
    let (x, y) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
    app.on_event(&Event::MouseMove { x, y }, Instant::now());
    app.on_event(&Event::MouseDown { x, y, button, clicks: 1 }, Instant::now());
    app.on_event(&Event::MouseUp { x, y, button }, Instant::now());
}

fn card_rect(app: &App, name: &str) -> Rect {
    let k = cards(app).iter().position(|c| c.ends_with(name)).unwrap_or_else(|| panic!("no card {name}"));
    app.nav.home.cards[k].0
}

fn menu_of(app: &mut App) -> ShellMenu {
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

fn run(app: &mut App, menu: &ShellMenu, label: &str) {
    let all = labels(&menu.menu);
    let k = all.iter().position(|c| c.trim_end_matches(" *") == label).unwrap_or_else(|| panic!("{label} not in {all:?}"));
    app.on_event(&Event::MenuCommand(k), Instant::now());
}

#[test]
fn home_is_its_own_location_with_widgets_and_no_listing() {
    let mut app = App::new(Persisted::default(), Vec::new(), 0, false, 1.0, (1000, 700), Instant::now());
    app.set_places(places());
    app.drain_effects().for_each(drop);
    app.go_home_page(false, Instant::now());
    assert!(app.on_home());
    assert_eq!(app.current_location().display(), "Home");
    assert_eq!(app.tab().crumbs.iter().map(|c| c.0.as_str()).collect::<Vec<_>>(), ["Home"]);
    let fx: Vec<Effect> = app.drain_effects().collect();
    assert!(fx.iter().any(|e| matches!(e, Effect::RefreshPlaces)), "opening Home refreshes the recent files");
    assert!(!fx.iter().any(|e| matches!(e, Effect::Provider { req: ProviderRequest::List { .. }, .. })), "Home lists nothing");
    app.frame(&mut Gfx::new(Mono, 1024), Instant::now());
    assert_eq!(cards(&app), ["folder Downloads", "drive 0", "recent report.docx", "recent proj", "recent old.txt"]);
    assert_eq!(app.nav.home.titles.iter().map(|t| t.1).collect::<Vec<_>>(), Widget::ALL);
    let (r, _) = app.nav.home.cards[0];
    assert_eq!(app.hit(r.x + 2.0, r.y + 2.0), Hit::HomeItem(0));
    assert_eq!(Location::parse("shell:::{F874310E-B6B7-47DC-BC84-B9E6B38F5903}"), Some(Location::home_page()));
    let full = app.nav.home.area;
    app.set_pane_state(crate::ui::PaneState { visible: true, ..Default::default() });
    app.frame(&mut Gfx::new(Mono, 1024), Instant::now());
    assert_eq!(app.nav.home.area, full, "the info pane stays out of the Home page");
    app.open_address();
    assert_eq!(app.nav.palette.as_ref().unwrap().field.text(), "");
}

#[test]
fn quick_access_shows_pinned_folders_first_and_cards_open_where_asked() {
    let mut app = home_app();
    app.frame(&mut Gfx::new(Mono, 1024), Instant::now());
    assert_eq!(&cards(&app)[..2], ["folder work", "folder Downloads"]);
    click(card_rect(&app, "Downloads"), &mut app, MouseButton::Middle);
    assert_eq!(app.tabs().len(), 2, "middle click opens a new tab");
    app.select_tab(0);
    click(card_rect(&app, "report.docx"), &mut app, MouseButton::Left);
    assert!(app.drain_effects().any(|e| matches!(e, Effect::Provider { req: ProviderRequest::Open { path, .. }, .. } if path == p(r"C:\Users\u\Documents\report.docx"))));
    assert!(app.on_home(), "opening a recent file stays on Home");
    click(card_rect(&app, "proj"), &mut app, MouseButton::Left);
    assert_eq!(app.current_location(), &loc(r"C:\work\proj"));
}

#[test]
fn widgets_toggle_from_the_page_menu_and_are_remembered() {
    let mut app = home_app();
    let empty = Rect::new(app.nav.home.area.right() - 30.0, app.nav.home.area.bottom() - 30.0, 4.0, 4.0);
    click(empty, &mut app, MouseButton::Right);
    let menu = menu_of(&mut app);
    assert!(!menu.shell);
    assert_eq!(labels(&menu.menu), ["Split Right", "Split Bottom", "Quick access *", "Drives *", "Tags *", "Recent files *", "Clear recent files"]);
    run(&mut app, &menu, "Drives");
    assert!(!cards(&app).iter().any(|c| c.starts_with("drive")));
    assert_eq!(app.nav_prefs().widgets_off, ["drives"]);
    let mut other = App::new(Persisted { path: Location::home_page(), ..Default::default() }, Vec::new(), 0, false, 1.0, (1000, 700), Instant::now());
    other.set_places(places());
    other.set_nav_prefs(app.nav_prefs());
    assert!(!cards(&other).iter().any(|c| c.starts_with("drive")));
    assert!(other.run_action("home.drives", Instant::now()));
    assert!(cards(&other).iter().any(|c| c.starts_with("drive")));
}

#[test]
fn recent_entries_hide_in_flit_until_they_are_used_again() {
    let mut app = home_app();
    click(card_rect(&app, "old.txt"), &mut app, MouseButton::Right);
    let menu = menu_of(&mut app);
    assert!(menu.shell, "a recent file also offers the shell menu");
    assert_eq!(menu.items, [p(r"C:\old.txt")]);
    assert_eq!(labels(&menu.menu), ["Open", "Open file location", "Copy path", "Remove from the list", "Clear the list", "Properties"]);
    run(&mut app, &menu, "Remove from the list");
    assert!(!cards(&app).contains(&"recent old.txt".to_owned()));
    let mut again = (*places()).clone();
    again.recent[2].at = 400;
    app.set_places(Arc::new(again));
    assert!(cards(&app).contains(&"recent old.txt".to_owned()), "opened again after removal");
    assert!(app.run_action("home.clear_recent", Instant::now()));
    assert!(!cards(&app).iter().any(|c| c.starts_with("recent")));
    assert!(app.nav.home.notes.iter().any(|n| n.1.t() == "No recent files" && !n.2));
    let mut fresh = (*places()).clone();
    fresh.recent.push(recent("new.txt", r"C:\new.txt", false, 500));
    app.set_places(Arc::new(fresh));
    assert_eq!(cards(&app).iter().filter(|c| c.starts_with("recent")).collect::<Vec<_>>(), ["recent new.txt"]);
    let p = app.nav_prefs();
    assert_eq!(p.recent_cleared, 400);
    assert!(p.recent_hidden.is_empty());
}

#[test]
fn tag_cards_show_their_items_and_offer_the_tag_menu() {
    let mut app = home_app();
    assert!(app.nav.home.notes.iter().any(|n| n.1.t().starts_with("No tags yet")));
    let mut store = crate::tags::TagStore { tags: Vec::new(), files: crate::hash::FxHashMap::default() };
    store.add("Work", crate::tags::COLORS[1].1).unwrap();
    store.add("Long name", crate::tags::COLORS[2].1).unwrap();
    app.set_tags(Arc::new(crate::tags::Tags::new(1, &store, &crate::hash::FxHashMap::default())));
    assert_eq!(cards(&app).iter().filter(|c| c.starts_with("tag")).collect::<Vec<_>>(), ["tag Work", "tag Long name"]);
    click(card_rect(&app, "Long name"), &mut app, MouseButton::Right);
    let menu = menu_of(&mut app);
    assert!(!menu.shell);
    assert_eq!(labels(&menu.menu), ["Show items with this tag", "Open all tagged items", "New tag", "Manage tags"]);
    click(card_rect(&app, "Long name"), &mut app, MouseButton::Left);
    let query = app.drain_effects().find_map(|e| if let Effect::Search(crate::ui::SearchRequest::Query { text, .. }) = e { Some(text) } else { None });
    assert_eq!(query.as_deref(), Some("tag:\"Long name\""));
    assert!(app.run_action("home.tags", Instant::now()));
    assert!(!cards(&app).iter().any(|c| c.starts_with("tag")));
}

#[test]
fn turned_off_recent_files_show_a_warning() {
    let mut app = home_app();
    app.set_places(Arc::new(Places { recent_off: true, ..(*places()).clone() }));
    assert!(app.nav.home.notes.iter().any(|n| n.2 && n.1.t().contains("turned off")));
}

#[test]
#[ignore]
fn bench_home_and_sidebar_frames() {
    let mut places = (*places()).clone();
    places.recent = (0..24).map(|k| recent(&format!("file {k}.docx"), &format!(r"C:\Users\u\Documents\file {k}.docx"), false, 1000 - k)).collect();
    places.drives = (0..8).map(|k| Drive { path: p(&format!("{}:\\", (b'C' + k) as char)), label: format!("Disk {k}"), total: 1000, free: 300, removable: k > 5, unmounted: false, kind: None, remote: None }).collect();
    let mut app = App::new(Persisted::default(), Vec::new(), 0, false, 1.0, (1400, 900), Instant::now());
    app.set_places(Arc::new(places));
    app.set_pinned((0..12).map(|k| loc(&format!(r"C:\work\p{k}"))).collect());
    let mut gfx = Gfx::new(Mono, 1024);
    app.frame(&mut gfx, Instant::now());
    let t = Instant::now();
    app.go_home_page(false, Instant::now());
    app.frame(&mut gfx, Instant::now());
    let open = t.elapsed();
    let mut times = Vec::new();
    for k in 0..400 {
        app.on_event(&Event::MouseMove { x: 400.0 + (k % 50) as f32 * 8.0, y: 200.0 + (k % 40) as f32 * 12.0 }, Instant::now());
        let t = Instant::now();
        app.frame(&mut gfx, Instant::now());
        times.push(t.elapsed());
    }
    times.sort();
    let ms = |d: std::time::Duration| d.as_secs_f64() * 1e3;
    eprintln!("BENCH open Home, click to finished frame {:.3} ms; hover frames median {:.3} ms p99 {:.3} ms ({} cards)", ms(open), ms(times[200]), ms(times[396]), app.nav.home.cards.len());
    let t = Instant::now();
    app.toggle_section(crate::ui::Section::Storage);
    app.frame(&mut gfx, Instant::now());
    eprintln!("BENCH collapse a sidebar section to finished frame {:.3} ms", ms(t.elapsed()));
}

#[test]
fn the_palette_lists_only_commands_that_apply_here() {
    let mut app = home_app();
    let live = |app: &App| {
        let p = app.nav.palette.as_ref().unwrap();
        let c = p.commands.as_ref().unwrap();
        p.titles.iter().cloned().zip(c.live.iter().copied()).collect::<Vec<(String, bool)>>()
    };
    let on = |rows: &[(String, bool)], title: &str| rows.iter().any(|(t, l)| t == title && *l);
    let off = |rows: &[(String, bool)], title: &str| rows.iter().any(|(t, l)| t == title && !*l);
    app.open_palette();
    let home = live(&app);
    assert!(on(&home, "Show or hide Recent files on the Home page"));
    assert!(off(&home, "Copy") && off(&home, "Empty Trash Items"), "commands that do not apply here are listed gray");
    let k = home.iter().position(|(t, _)| t == "Go to This PC").unwrap();
    let p = app.nav.palette.as_ref().unwrap();
    let row = p.commands.as_ref().unwrap().lines.iter().position(|l| matches!(l, crate::ui::palette::Line::Item(n, ..) if *n == k)).unwrap();
    app.run_palette_item(row, Instant::now());
    assert_eq!(app.current_location(), &Location::computer(), "palette rows map back to their actions");
    app.navigate(loc(r"C:\x"), Instant::now());
    app.open_palette();
    let folder = live(&app);
    assert!(on(&folder, "Copy"));
    assert!(off(&folder, "Show or hide Recent files on the Home page"));
    assert!(off(&folder, "Group search results by folder"));
}

fn asked(app: &mut App, size: u32) -> Vec<(String, crate::ui::IconKind)> {
    app.drain_effects().filter_map(|e| if let Effect::Icon(r) = e { (r.size == size).then(|| (r.path.display().into_owned(), r.kind)) } else { None }).collect()
}

fn kinds_in(g: &Gfx<Mono>, r: Rect, kind: u32) -> usize {
    g.dl.instances.iter().filter(|i| i.kind == kind && r.contains(i.rect[0] + i.rect[2] / 2.0, i.rect[1] + i.rect[3] / 2.0)).count()
}

#[test]
fn home_cards_ask_the_system_for_place_drive_and_file_icons() {
    use crate::ui::IconKind::{Place, Volume};
    let mut app = home_app();
    let mut g = Gfx::new(Mono, 1024);
    app.frame(&mut g, Instant::now());
    let cards = asked(&mut app, 32);
    for want in [(r"C:\work", Place), (r"C:\Users\u\Downloads", Place), (r"C:\", Volume)] {
        assert!(cards.contains(&(want.0.to_owned(), want.1)), "{want:?} not in {cards:?}");
    }
    assert_eq!(cards.len(), 3, "{cards:?}");
    let rows = app.icons.len();
    app.frame(&mut g, Instant::now());
    assert!(asked(&mut app, 32).is_empty(), "pending card icons are asked once");
    assert_eq!(app.icons.len(), rows);
}

#[test]
fn recent_rows_ask_for_file_and_folder_icons() {
    use crate::ui::IconKind::{File, Place};
    let mut app = home_app();
    app.frame(&mut Gfx::new(Mono, 1024), Instant::now());
    let rows = asked(&mut app, 20);
    for want in [(r"C:\Users\u\Documents\report.docx", File), (r"C:\work\proj", Place), (r"C:\old.txt", File)] {
        assert!(rows.contains(&(want.0.to_owned(), want.1)), "{want:?} not in {rows:?}");
    }
}

#[test]
fn card_glyphs_show_only_until_the_system_icon_arrives() {
    let mut app = home_app();
    let mut g = Gfx::new(Mono, 1024);
    app.frame(&mut g, Instant::now());
    let r = card_rect(&app, "Downloads");
    let ir = Rect::new(r.x, r.y, 44.0, r.h);
    assert_eq!(kinds_in(&g, ir, crate::render::kind::GLYPH), 1, "the folder glyph stands in while loading");
    assert_eq!(kinds_in(&g, ir, crate::render::kind::IMAGE), 0);
    let key = app.drain_effects().find_map(|e| if let Effect::Icon(q) = e { (q.size == 32 && q.path.display() == r"C:\Users\u\Downloads").then_some(q.key) } else { None }).expect("asked");
    app.on_icon(&mut g, key, crate::ui::IconResult::Ready(crate::ui::Bitmap { width: 32, height: 32, rgba: vec![255; 32 * 32 * 4] }));
    app.frame(&mut g, Instant::now());
    assert_eq!(kinds_in(&g, ir, crate::render::kind::IMAGE), 1, "the system icon replaces the glyph");
    assert_eq!(kinds_in(&g, ir, crate::render::kind::GLYPH), 0);
}
