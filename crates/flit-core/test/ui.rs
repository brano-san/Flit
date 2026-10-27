use super::*;
use crate::listing::{ChangeKind, DirChange, Listing, Meta};
use crate::open::parse_args;
use crate::provider::{FileOp, GenToken};
use crate::sort::{self, SortKeys};
use crate::text::tests::Mono;

fn t0() -> Instant {
    Instant::now()
}

fn loc(s: &str) -> Location {
    Location::parse(s).unwrap()
}

fn snapshot(gen: u64, path: &FsPath, items: &[(&str, bool)]) -> Arc<DirSnapshot> {
    let items: Vec<(&str, u32)> = items.iter().map(|&(n, dir)| (n, if dir { attr::DIR } else { 0 })).collect();
    snapshot_with(gen, path, ViewSpec::default(), true, &items)
}

fn snapshot_with(gen: u64, path: &FsPath, view: ViewSpec, done: bool, items: &[(&str, u32)]) -> Arc<DirSnapshot> {
    let mut listing = Listing::default();
    let mut keys = SortKeys::new(CaseMode::Insensitive);
    for &(n, attrs) in items {
        let i = listing.push(n.as_bytes(), Meta { attrs, size: 10, ..Default::default() });
        keys.push(listing.name(i));
    }
    let mut order = Vec::new();
    sort::sort(&listing, &keys, view, &mut order);
    Arc::new(DirSnapshot { gen, path: path.clone(), listing, keys, order, view, done, error: None, elapsed: Duration::from_millis(1) })
}

fn publish(app: &mut App, view: ViewSpec, done: bool, items: &[(&str, u32)]) {
    let t = app.tab();
    t.slot.publish(snapshot_with(t.gen, t.location().path(), view, done, items));
    app.poll();
}

fn trashed(app: &mut App) -> Option<Vec<FsPath>> {
    app.drain_effects().find_map(|e| match e {
        Effect::Provider { req: ProviderRequest::Op { op: FileOp::Trash(p), .. }, .. } => Some(p),
        _ => None,
    })
}

fn resort_now(app: &mut App) {
    let job = app.drain_effects().find_map(|e| if let Effect::Resort { tab, gen, view, snap } = e { Some((tab, gen, view, snap)) } else { None });
    let (tab, gen, view, snap) = job.expect("resort requested");
    let mut order = Vec::new();
    sort::sort(&snap.listing, &snap.keys, view, &mut order);
    app.on_resorted(tab, gen, view, snap.listing.len(), order);
}

fn deliver(app: &mut App, items: &[(&str, bool)]) {
    let t = app.tab();
    t.slot.publish(snapshot(t.gen, t.location().path(), items));
    app.poll();
}

fn app_at(path: &str) -> App {
    let state = Persisted { path: loc(path), ..Default::default() };
    App::new(state, Vec::new(), 0, false, 1.0, (1000, 600), t0())
}

fn listed(app: &mut App) -> Vec<(TabId, Location)> {
    app.drain_effects()
        .filter_map(|e| match e {
            Effect::Provider { provider, req: ProviderRequest::List { tab, path, .. } } => Some((
                tab,
                match provider {
                    ProviderId::Local => Location::Local(path),
                    ProviderId::Wsl(distro) => Location::Wsl { distro, path },
                },
            )),
            _ => None,
        })
        .collect()
}

fn key(app: &mut App, key: Key, mods: Mods) {
    app.on_event(&Event::Key { key, mods, pressed: true }, t0());
}

const NO: Mods = Mods { ctrl: false, shift: false, alt: false };
const SHIFT: Mods = Mods { ctrl: false, shift: true, alt: false };
const CTRL: Mods = Mods { ctrl: true, shift: false, alt: false };

fn selected_names(app: &App) -> Vec<String> {
    let v = app.tab().view.as_ref().unwrap();
    v.selected().map(|i| String::from_utf8_lossy(v.listing().name(i)).into_owned()).collect()
}

fn click(app: &mut App, r: Rect) {
    let (x, y) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
    app.on_event(&Event::MouseDown { x, y, button: MouseButton::Left, clicks: 1 }, t0());
    app.on_event(&Event::MouseUp { x, y, button: MouseButton::Left }, t0());
}

#[test]
fn navigation_flow() {
    let mut app = app_at(r"C:\work");
    assert_eq!(listed(&mut app), [(1, loc(r"C:\work"))]);
    deliver(&mut app, &[("src", true), ("a.txt", false)]);
    key(&mut app, Key::Down, NO);
    assert_eq!(selected_names(&app), ["src"]);
    key(&mut app, Key::Enter, NO);
    assert_eq!(listed(&mut app), [(1, loc(r"C:\work\src"))]);
    let stale = app.tab().gen - 1;
    app.tab().slot.publish(snapshot(stale, &FsPath::parse(r"C:\work").unwrap(), &[("zzz", false)]));
    app.poll();
    assert!(app.tab().view.as_ref().unwrap().snap().gen != app.tab().gen);
    deliver(&mut app, &[("main.rs", false)]);
    key(&mut app, Key::Backspace, NO);
    assert_eq!(listed(&mut app), [(1, loc(r"C:\work"))]);
    deliver(&mut app, &[("src", true), ("a.txt", false)]);
    assert_eq!(selected_names(&app), ["src"]);
    key(&mut app, Key::Up, Mods { alt: true, ..NO });
    assert_eq!(listed(&mut app), [(1, loc(r"C:\"))]);
    deliver(&mut app, &[("work", true), ("Windows", true)]);
    assert_eq!(selected_names(&app), ["work"]);
}

#[test]
fn keyboard_selection_and_type_ahead() {
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[("apple", false), ("banana", false), ("blueberry", false), ("cherry", false)]);
    key(&mut app, Key::Home, NO);
    key(&mut app, Key::Down, SHIFT);
    key(&mut app, Key::Down, SHIFT);
    assert_eq!(selected_names(&app), ["apple", "banana", "blueberry"]);
    key(&mut app, Key::End, NO);
    assert_eq!(selected_names(&app), ["cherry"]);
    app.on_event(&Event::Char('b'), t0());
    assert_eq!(selected_names(&app), ["banana"]);
    app.on_event(&Event::Char('b'), t0());
    assert_eq!(selected_names(&app), ["blueberry"]);
    app.on_event(&Event::Char('c'), t0() + Duration::from_secs(2));
    assert_eq!(selected_names(&app), ["cherry"]);
    key(&mut app, Key::Char('A'), CTRL);
    assert_eq!(app.tab().view.as_ref().unwrap().selected_count(), 4);
}

#[test]
fn sort_click_resorts_and_diffs_apply() {
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[("a", false), ("c", false)]);
    app.drain_effects().for_each(drop);
    let size_header = app.layout.cols[3];
    click(&mut app, size_header);
    let resort = app.drain_effects().find_map(|e| if let Effect::Resort { tab, gen, view, snap } = e { Some((tab, gen, view, snap)) } else { None });
    let (tab, gen, view, snap) = resort.expect("resort requested");
    assert_eq!(view.sort.field, SortField::Size);
    let mut order = Vec::new();
    sort::sort(&snap.listing, &snap.keys, view, &mut order);
    app.on_resorted(tab, gen, view, snap.listing.len(), order);
    let added = DirChange { kind: ChangeKind::Added, name: b"b".to_vec(), meta: Some(Meta { size: 1, ..Default::default() }) };
    app.on_provider_event(ProviderEvent::Changes { tab, gen, changes: vec![added] }, t0());
    let mut g = Gfx::new(Mono, 1024);
    app.frame(&mut g, t0());
    let v = app.tab().view.as_ref().unwrap();
    let names: Vec<_> = v.rows().iter().map(|&i| v.listing().name(i).to_vec()).collect();
    assert_eq!(names, [b"b".to_vec(), b"a".to_vec(), b"c".to_vec()]);
}

#[test]
fn frame_draws_and_requests_icons() {
    let mut app = app_at(r"C:\x");
    let names: Vec<String> = (0..1000).map(|i| format!("file{i}.txt")).collect();
    let items: Vec<(&str, bool)> = names.iter().map(|n| (n.as_str(), false)).collect();
    deliver(&mut app, &items);
    app.drain_effects().for_each(drop);
    let mut g = Gfx::new(Mono, 2048);
    app.frame(&mut g, t0());
    assert!(!g.dl.instances.is_empty());
    let effects: Vec<Effect> = app.drain_effects().collect();
    let icons = effects.iter().filter(|e| matches!(e, Effect::Icon(r) if r.kind == IconKind::Type)).count();
    let types = effects.iter().filter(|e| matches!(e, Effect::TypeName(_))).count();
    assert_eq!(icons, 1, "one icon request per extension");
    assert_eq!(types, 1);
    let (_, first, last) = app.visible_rows();
    assert_eq!(first, 0);
    assert!(last > 10 && last < 30, "only visible rows: {last}");
    app.frame(&mut g, t0());
    assert_eq!(app.drain_effects().count(), 0);
}

#[test]
fn this_pc_asks_for_distro_icons_by_their_share_root() {
    let mut app = App::new(Persisted { path: Location::computer(), ..Default::default() }, Vec::new(), 0, false, 1.0, (1000, 600), t0());
    publish(&mut app, ViewSpec::default(), true, &[("C:", attr::DIR | attr::VOLUME), ("Ubuntu-22.04", attr::DIR | attr::VOLUME)]);
    app.drain_effects().for_each(drop);
    app.frame(&mut Gfx::new(Mono, 1024), t0());
    let mut asked: Vec<(Vec<u8>, IconKind)> = app.drain_effects().filter_map(|e| if let Effect::Icon(r) = e { (r.kind == IconKind::Volume).then_some((r.path.as_bytes().to_vec(), r.kind)) } else { None }).collect();
    asked.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(asked, [(br"C:\".to_vec(), IconKind::Volume), (br"\\wsl.localhost\Ubuntu-22.04\".to_vec(), IconKind::Volume)]);
}

#[test]
fn caption_hit_test() {
    let app = app_at(r"C:\x");
    let l = &app.layout;
    assert_eq!(app.hit_test(l.caption[2].x + 2.0, 5.0), HitArea::Close);
    assert_eq!(app.hit_test(l.caption[1].x + 2.0, 5.0), HitArea::Maximize);
    assert_eq!(app.hit_test(l.new_tab.right() + 20.0, 10.0), HitArea::Caption);
    assert_eq!(app.hit_test(l.tabs[0].0.x + 10.0, l.tabs[0].0.y + 10.0), HitArea::Client);
    assert_eq!(app.hit_test(l.address.x + 20.0, l.address.y + 5.0), HitArea::Client);
    assert_eq!(app.hit_test(300.0, 300.0), HitArea::Client);
}

#[test]
fn address_bar_edit_and_submit() {
    let mut app = app_at(r"C:\x");
    app.drain_effects().for_each(drop);
    key(&mut app, Key::Char('L'), CTRL);
    for c in r"D:\data".chars() {
        app.on_event(&Event::Char(c), t0());
    }
    key(&mut app, Key::Enter, NO);
    run_opens(&mut app);
    assert_eq!(listed(&mut app), [(1, loc(r"D:\data"))]);
    for (typed, expected) in [(r"\\wsl.localhost\Ubuntu\home", "wsl://Ubuntu/home"), ("shell:Downloads", r"C:\Users\u\Downloads")] {
        key(&mut app, Key::Char('L'), CTRL);
        for c in typed.chars() {
            app.on_event(&Event::Char(c), t0());
        }
        key(&mut app, Key::Enter, NO);
        run_opens(&mut app);
        assert_eq!(listed(&mut app), [(1, loc(expected))]);
    }
}

fn run_opens(app: &mut App) {
    let opens: Vec<Open> = app.drain_effects().filter_map(|e| if let Effect::Open(o) = e { Some(o) } else { None }).collect();
    for o in opens {
        app.open(&o, &|s| s.eq_ignore_ascii_case("shell:Downloads").then(|| loc(r"C:\Users\u\Downloads")), t0());
    }
}

#[test]
fn tabs_are_independent() {
    let mut app = app_at(r"C:\a");
    deliver(&mut app, &[("one", false), ("two", false)]);
    key(&mut app, Key::Down, NO);
    app.drain_effects().for_each(drop);
    key(&mut app, Key::Char('T'), CTRL);
    assert_eq!(app.tabs().len(), 2);
    assert_eq!(listed(&mut app), [(2, loc(r"C:\a"))]);
    deliver(&mut app, &[("one", false), ("two", false)]);
    key(&mut app, Key::End, NO);
    assert_eq!(selected_names(&app), ["two"]);
    key(&mut app, Key::Up, Mods { alt: true, ..NO });
    assert_eq!(listed(&mut app), [(2, loc(r"C:\"))]);
    key(&mut app, Key::Tab, CTRL);
    assert_eq!(app.tab().id, 1);
    assert_eq!(app.current_location(), &loc(r"C:\a"));
    assert_eq!(selected_names(&app), ["one"]);
    assert!(!app.tab().history.can_back());
    let t2 = &app.tabs()[1];
    t2.slot.publish(snapshot(t2.gen, t2.location().path(), &[("Windows", true)]));
    assert!(!app.poll(), "background tab changed, active one did not");
    assert_eq!(app.tabs()[1].view.as_ref().unwrap().listing().len(), 1);
    let (id2, gen2) = (app.tabs()[1].id, app.tabs()[1].gen);
    app.on_provider_event(ProviderEvent::DirSize { tab: id2, gen: gen2 - 1, idx: 0, size: 99 }, t0());
    assert_ne!(app.tabs()[1].view.as_ref().unwrap().listing().entries[0].size, 99);
}

#[test]
fn closing_tabs_stops_watches_and_last_closes_window() {
    let mut app = app_at(r"C:\a");
    key(&mut app, Key::Char('T'), CTRL);
    app.drain_effects().for_each(drop);
    key(&mut app, Key::Char('W'), CTRL);
    let fx: Vec<Effect> = app.drain_effects().collect();
    assert!(fx.iter().any(|e| matches!(e, Effect::Provider { req: ProviderRequest::Watch { tab: 2, path: None, .. }, .. })));
    assert_eq!(app.tabs().len(), 1);
    let r = app.layout.tabs[0].0;
    app.on_event(&Event::MouseDown { x: r.x + 10.0, y: r.y + 10.0, button: MouseButton::Middle, clicks: 1 }, t0());
    assert!(app.drain_effects().any(|e| matches!(e, Effect::Window(WindowCmd::Close))));
}

#[test]
fn external_open_routes_through_dispatch() {
    let mut app = app_at(r"C:\a");
    app.drain_effects().for_each(drop);
    let args = |a: &[&str]| a.iter().map(|s| s.as_bytes().to_vec()).collect::<Vec<_>>();
    app.open(&parse_args(&args(&[r"/select,C:\x\b.txt"]), None), &|_| None, t0());
    assert_eq!(app.tabs().len(), 2);
    assert_eq!(listed(&mut app), [(2, loc(r"C:\x"))]);
    deliver(&mut app, &[("a.txt", false), ("b.txt", false)]);
    assert_eq!(selected_names(&app), ["b.txt"]);
    app.open(&parse_args(&args(&["--current-tab", r"\\wsl$\Ubuntu\etc"]), None), &|_| None, t0());
    let fx: Vec<Effect> = app.drain_effects().collect();
    assert!(fx.iter().any(|e| matches!(e, Effect::Provider { provider: ProviderId::Local, req: ProviderRequest::Watch { tab: 2, path: None, .. } })));
    assert!(fx.iter().any(|e| matches!(e, Effect::Provider { provider: ProviderId::Wsl(d), req: ProviderRequest::List { tab: 2, .. } } if d == "Ubuntu")));
    assert_eq!(app.tabs().len(), 2);
    app.open(&parse_args(&args(&["--new-window", r"D:\"]), None), &|_| None, t0());
    assert!(app.drain_effects().any(|e| matches!(e, Effect::NewWindow(r) if r[0].folder == loc(r"D:\"))));
    app.open(&parse_args(&args(&["shell:Nope"]), None), &|_| None, t0());
    assert_eq!(app.tabs().len(), 2);
    assert!(app.tab().error.is_some());
}

#[test]
fn start_routes_open_one_tab_each() {
    let start = vec![Route { folder: loc(r"C:\a"), select: None }, Route { folder: loc("wsl://Debian/"), select: None }];
    let mut app = App::new(Persisted::default(), start, 0, false, 1.0, (1000, 600), t0());
    assert_eq!(listed(&mut app), [(1, loc(r"C:\a")), (2, loc("wsl://Debian/"))]);
    assert_eq!(app.tab().id, 2);
}

#[test]
fn background_tab_diff_overflow_rescans() {
    let mut app = app_at(r"C:\a");
    deliver(&mut app, &[("one", false)]);
    let (id, gen) = (app.tab().id, app.tab().gen);
    key(&mut app, Key::Char('T'), CTRL);
    app.drain_effects().for_each(drop);
    let change = DirChange { kind: ChangeKind::Removed, name: b"x".to_vec(), meta: None };
    app.on_provider_event(ProviderEvent::Changes { tab: id, gen, changes: vec![change; MAX_PENDING_DIFFS + 1] }, t0());
    assert!(app.tabs()[0].pending_diffs.is_empty());
    assert_eq!(listed(&mut app), [(id, loc(r"C:\a"))]);
}

const CTRL_SHIFT: Mods = Mods { ctrl: true, shift: true, alt: false };

#[test]
fn actions_before_the_new_listing_target_the_shown_folder() {
    let mut app = app_at(r"C:\work");
    deliver(&mut app, &[("src", true), ("a.txt", false)]);
    key(&mut app, Key::Down, NO);
    key(&mut app, Key::Enter, NO);
    deliver(&mut app, &[("main.rs", false), ("sub", true)]);
    key(&mut app, Key::End, NO);
    app.drain_effects().for_each(drop);
    key(&mut app, Key::Up, Mods { alt: true, ..NO });
    assert_eq!(listed(&mut app), [(1, loc(r"C:\work"))]);
    key(&mut app, Key::Delete, NO);
    assert_eq!(trashed(&mut app).unwrap(), [FsPath::parse(r"C:\work\src\main.rs").unwrap()]);
    key(&mut app, Key::Char('C'), CTRL_SHIFT);
    assert!(app.drain_effects().any(|e| matches!(e, Effect::SetClipboard(s) if s == r"C:\work\src\main.rs")));
    key(&mut app, Key::Char('S'), Mods { ctrl: true, shift: false, alt: true });
    assert!(!app.drain_effects().any(|e| matches!(e, Effect::Provider { req: ProviderRequest::DirSizes { .. }, .. })));
    let (id, gen) = (app.tab().id, app.tab().gen);
    app.on_provider_event(ProviderEvent::DirSize { tab: id, gen, idx: 0, size: 99 }, t0());
    assert_ne!(app.tab().view.as_ref().unwrap().listing().entries[0].size, 99);
    key(&mut app, Key::Enter, NO);
    assert!(app.drain_effects().any(|e| matches!(e, Effect::Provider { req: ProviderRequest::Open { path, .. }, .. } if path == FsPath::parse(r"C:\work\src\main.rs").unwrap())));
}

#[test]
fn hidden_entries_leave_the_selection() {
    let shown = ViewSpec { show_hidden: true, ..Default::default() };
    let mut app = App::new(Persisted { path: loc(r"C:\x"), show_hidden: true, ..Default::default() }, Vec::new(), 0, false, 1.0, (1000, 600), t0());
    publish(&mut app, shown, true, &[(".git", attr::DIR | attr::HIDDEN), ("a.txt", 0), ("b.txt", 0)]);
    key(&mut app, Key::Char('A'), CTRL);
    assert_eq!(app.tab().view.as_ref().unwrap().selected_count(), 3);
    app.drain_effects().for_each(drop);
    key(&mut app, Key::Char('H'), CTRL);
    resort_now(&mut app);
    assert_eq!(selected_names(&app), ["a.txt", "b.txt"]);
    assert_eq!(app.tab().view.as_ref().unwrap().selected_count(), 2);
    key(&mut app, Key::Delete, NO);
    assert_eq!(trashed(&mut app).unwrap(), [FsPath::parse(r"C:\x\a.txt").unwrap(), FsPath::parse(r"C:\x\b.txt").unwrap()]);
}

#[test]
fn pending_select_scans_partial_snapshots_and_skips_invisible() {
    let mut app = app_at(r"C:\a");
    let args = |a: &[&str]| a.iter().map(|s| s.as_bytes().to_vec()).collect::<Vec<_>>();
    app.open(&parse_args(&args(&[r"/select,C:\y\b.txt"]), None), &|_| None, t0());
    publish(&mut app, ViewSpec::default(), false, &[("a.txt", 0)]);
    assert!(selected_names(&app).is_empty());
    publish(&mut app, ViewSpec::default(), false, &[("a.txt", 0), ("b.txt", 0)]);
    assert_eq!(selected_names(&app), ["b.txt"]);
    app.open(&parse_args(&args(&[r"/select,C:\z\.git"]), None), &|_| None, t0());
    publish(&mut app, ViewSpec::default(), true, &[(".git", attr::DIR | attr::HIDDEN), ("x", 0)]);
    let v = app.tab().view.as_ref().unwrap();
    assert_eq!((v.selected_count(), v.focus), (0, None));
}

#[test]
fn delete_needs_a_visible_selection() {
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[("a.txt", false)]);
    key(&mut app, Key::Down, NO);
    key(&mut app, Key::Escape, NO);
    assert!(app.tab().view.as_ref().unwrap().focus.is_some());
    app.drain_effects().for_each(drop);
    key(&mut app, Key::Delete, NO);
    assert_eq!(trashed(&mut app), None);
}

#[test]
fn altgr_characters_reach_type_ahead() {
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[("zz", false), ("ąb", false)]);
    let altgr = Mods { ctrl: true, shift: false, alt: true };
    key(&mut app, Key::Char('A'), altgr);
    assert!(selected_names(&app).is_empty());
    app.on_event(&Event::Char('ą'), t0());
    assert_eq!(selected_names(&app), ["ąb"]);
    key(&mut app, Key::Char('Z'), CTRL);
    app.on_event(&Event::Char('z'), t0());
    assert_eq!(selected_names(&app), ["ąb"]);
}

#[test]
fn trash_on_unwatched_locations_reloads() {
    let start = [r"wsl://Ubuntu/home", r"\\srv\share\d", r"C:\x"].map(|p| Route { folder: loc(p), select: None }).to_vec();
    let mut app = App::new(Persisted::default(), start, 0, false, 1.0, (1000, 600), t0());
    app.drain_effects().for_each(drop);
    for (id, path, reloads) in [(1, "wsl://Ubuntu/home", true), (2, r"\\srv\share\d", true), (3, r"C:\x", false)] {
        app.on_provider_event(ProviderEvent::OpDone { tab: id, id: 0, error: None, report: None }, t0());
        assert_eq!(listed(&mut app), if reloads { vec![(id, loc(path))] } else { vec![] });
    }
}

#[test]
fn open_caps_the_number_of_routes() {
    let mut app = app_at(r"C:\a");
    let paths = (0..100).map(|i| format!(r"C:\d{i}").into_bytes()).collect();
    app.open(&Open { paths, target: OpenTarget::NewTab, ..Default::default() }, &|_| None, t0());
    assert_eq!(app.tabs().len(), 1 + MAX_ROUTES);
    deliver(&mut app, &[("f", false)]);
    assert!(app.tab().error.as_deref().is_some_and(|e| e.contains("100")));
    let start = (0..40).map(|i| Route { folder: loc(&format!(r"C:\s{i}")), select: None }).collect();
    let app = App::new(Persisted::default(), start, 0, false, 1.0, (1000, 600), t0());
    assert_eq!(app.tabs().len(), MAX_ROUTES);
    assert!(app.tab().error.is_some());
}

#[test]
fn atlas_overflow_mid_frame_redraws_before_presenting() {
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[("a.txt", false)]);
    let mut clean = Gfx::new(Mono, 1024);
    app.frame(&mut clean, t0());
    let glyphs = clean.uploads.rects.len() as u32;
    let size = (64..).find(|s| (s / 7) * (s / 9) >= 2 * glyphs).unwrap();
    let mut g = Gfx::new(Mono, size);
    g.begin(Color::default());
    let filler: String = (0..(size / 7) * (size / 9) - glyphs / 2).map(|i| char::from_u32(0x4E00 + i).unwrap()).collect();
    g.text(0.0, 0.0, &filler, FontStyle::new(FontFamily::Ui, 99.0, false), Color::default(), f32::INFINITY);
    assert!(!g.atlas_reset);
    assert!(!app.frame(&mut g, t0()), "nothing left to redraw");
    assert!(!g.atlas_reset);
    assert_eq!(g.dl.instances.len(), clean.dl.instances.len());
}

#[test]
fn shutdown_cancels_jobs_and_stops_watches() {
    let start = vec![Route { folder: loc(r"C:\a"), select: None }, Route { folder: loc("wsl://Debian/"), select: None }];
    let mut app = App::new(Persisted::default(), start, 0, false, 1.0, (1000, 600), t0());
    app.drain_effects().for_each(drop);
    let tokens: Vec<(GenToken, u64)> = app.tabs().iter().map(|t| (t.cancel.clone(), t.gen)).collect();
    assert!(tokens.iter().all(|(c, g)| c.is_current(*g)));
    app.shutdown();
    let fx: Vec<Effect> = app.drain_effects().collect();
    assert!(fx.iter().any(|e| matches!(e, Effect::Provider { provider: ProviderId::Local, req: ProviderRequest::Watch { tab: 1, path: None, .. } })));
    assert!(fx.iter().any(|e| matches!(e, Effect::Provider { provider: ProviderId::Wsl(d), req: ProviderRequest::Watch { tab: 2, path: None, .. } } if d == "Debian")));
    assert!(tokens.iter().all(|(c, g)| !c.is_current(*g)));
}
