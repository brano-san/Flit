use super::*;
use crate::listing::{ChangeKind, DirChange, Listing, Meta};
use crate::open::parse_args;
use crate::sort::{self, SortKeys};
use crate::text::tests::Mono;

fn t0() -> Instant {
    Instant::now()
}

fn loc(s: &str) -> Location {
    Location::parse(s).unwrap()
}

fn snapshot(gen: u64, path: &FsPath, items: &[(&str, bool)]) -> Arc<DirSnapshot> {
    let mut listing = Listing::default();
    let mut keys = SortKeys::new(CaseMode::Insensitive);
    for &(n, dir) in items {
        let i = listing.push(n.as_bytes(), Meta { attrs: if dir { attr::DIR } else { 0 }, size: 10, ..Default::default() });
        keys.push(listing.name(i));
    }
    let view = ViewSpec::default();
    let mut order = Vec::new();
    sort::sort(&listing, &keys, view, &mut order);
    Arc::new(DirSnapshot { gen, path: path.clone(), listing, keys, order, view, done: true, error: None, elapsed: Duration::from_millis(1) })
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
    let icons = effects.iter().filter(|e| matches!(e, Effect::Icon(_))).count();
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
    assert_eq!(listed(&mut app), [(1, loc(r"D:\data"))]);
    key(&mut app, Key::Char('L'), CTRL);
    for c in r"\\wsl.localhost\Ubuntu\home".chars() {
        app.on_event(&Event::Char(c), t0());
    }
    key(&mut app, Key::Enter, NO);
    assert_eq!(listed(&mut app), [(1, loc("wsl://Ubuntu/home"))]);
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
