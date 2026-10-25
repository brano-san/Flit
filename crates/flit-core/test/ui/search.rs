use super::*;
use crate::provider::ProviderRequest;
use crate::text::tests::Mono;
use crate::ui::Persisted;

fn t0() -> Instant {
    Instant::now()
}

fn app() -> App {
    let state = Persisted { path: Location::parse(r"C:\x").unwrap(), ..Default::default() };
    let mut app = App::new(state, Vec::new(), 0, false, 1.0, (1000, 600), t0());
    app.drain_effects().for_each(drop);
    app
}

const CTRL: Mods = Mods { ctrl: true, shift: false, alt: false };
const NO: Mods = Mods { ctrl: false, shift: false, alt: false };

fn key(app: &mut App, key: Key, mods: Mods) {
    app.on_event(&Event::Key { key, mods, pressed: true }, t0());
}

fn requests(app: &mut App) -> Vec<SearchRequest> {
    app.drain_effects().filter_map(|e| if let Effect::Search(r) = e { Some(r) } else { None }).collect()
}

fn gen_of(app: &App) -> u64 {
    app.search.gen
}

fn row(name: &str, dir: &str, dir_flag: bool) -> SearchRow {
    SearchRow { name: name.as_bytes().to_vec(), dir: dir.as_bytes().to_vec(), meta: Meta { size: 7, modified: 0, created: 0, attrs: if dir_flag { attr::DIR } else { 0 } } }
}

fn page(from: u64, n: u64) -> Vec<SearchRow> {
    (from..from + n).map(|i| row(&format!("f{i}.txt"), r"C:\data", false)).collect()
}

fn frame(app: &mut App) {
    let mut g = Gfx::new(Mono, 2048);
    app.frame(&mut g, t0());
}

#[test]
fn ctrl_e_opens_search_and_every_keystroke_is_a_new_query() {
    let mut app = app();
    key(&mut app, Key::Char('E'), CTRL);
    assert!(app.search_active());
    let fx: Vec<Effect> = app.drain_effects().collect();
    assert!(fx.iter().any(|e| matches!(e, Effect::Title(t) if t.starts_with("Search"))));
    assert!(fx.iter().any(|e| matches!(e, Effect::Search(SearchRequest::Query { gen: 1, text, .. }) if text.is_empty())));
    app.on_event(&Event::Char('a'), t0());
    app.on_event(&Event::Char('b'), t0());
    key(&mut app, Key::Backspace, NO);
    let texts: Vec<(u64, String)> = requests(&mut app).into_iter().filter_map(|r| if let SearchRequest::Query { gen, text, .. } = r { Some((gen, text)) } else { None }).collect();
    assert_eq!(texts, [(2, "a".to_string()), (3, "ab".to_string()), (4, "a".to_string())]);
    assert_eq!(app.ime_area(), Some(app.search_layout().field));
    app.on_paste("xyz\nignored");
    assert!(matches!(&requests(&mut app)[..], [SearchRequest::Query { text, .. }] if text == "axyz"));
}

#[test]
fn only_visible_pages_are_fetched_and_stale_results_are_dropped() {
    let mut app = app();
    app.open_search();
    app.on_event(&Event::Char('f'), t0());
    requests(&mut app);
    let gen = gen_of(&app);
    app.on_search_event(SearchEvent::Done { gen: gen - 1, total: 5 }, t0());
    assert_eq!(app.search.total, 0, "an older query does not count");
    frame(&mut app);
    assert!(requests(&mut app).is_empty(), "no rows before the total is known");
    app.on_search_event(SearchEvent::Done { gen, total: 1_000_000 }, t0());
    frame(&mut app);
    let first = requests(&mut app);
    assert_eq!(first, [SearchRequest::Rows { gen, from: 0, to: 2 * PAGE }]);
    app.on_search_event(SearchEvent::Rows { gen, from: 0, rows: page(0, 2 * PAGE) }, t0());
    frame(&mut app);
    assert!(requests(&mut app).is_empty(), "loaded pages are not fetched again");
    app.on_event(&Event::Wheel { dx: 0.0, dy: -1000.0 }, t0());
    frame(&mut app);
    let after = requests(&mut app);
    assert_eq!(after.len(), 1);
    let SearchRequest::Rows { from, to, .. } = after[0] else { panic!() };
    let (vis_from, vis_to) = app.search_range();
    assert!(from <= vis_from && to >= vis_to && to - from <= 4 * PAGE, "{from}..{to} covers {vis_from}..{vis_to}");
    app.on_search_event(SearchEvent::Rows { gen: gen - 1, from, rows: page(from, to - from) }, t0());
    assert!(!app.search.rows.contains_key(&from));
    key(&mut app, Key::End, CTRL);
    assert_eq!(app.search.focus, Some(999_999));
    frame(&mut app);
    assert!(matches!(&requests(&mut app)[..], [SearchRequest::Rows { to: 1_000_000, .. }]));
}

#[test]
fn enter_opens_and_ctrl_enter_reveals_in_the_folder() {
    let mut app = app();
    app.open_search();
    requests(&mut app);
    let gen = gen_of(&app);
    app.on_search_event(SearchEvent::Done { gen, total: 2 }, t0());
    app.on_search_event(SearchEvent::Rows { gen, from: 0, rows: vec![row("a.txt", r"C:\data", false), row("sub", r"C:\data", true)] }, t0());
    key(&mut app, Key::Down, NO);
    key(&mut app, Key::Enter, NO);
    let opened = app.drain_effects().find_map(|e| match e {
        Effect::Provider { req: ProviderRequest::Open { path, .. }, .. } => Some(path),
        _ => None,
    });
    assert_eq!(opened, Some(crate::FsPath::parse(r"C:\data\a.txt").unwrap()));
    assert!(app.search_active());
    key(&mut app, Key::Enter, CTRL);
    assert!(!app.search_active());
    assert_eq!(app.current_location(), &Location::parse(r"C:\data").unwrap());
    let t = app.tab();
    assert_eq!(t.loading.as_ref().and_then(|l| l.select.clone()), Some(b"a.txt".to_vec()));
    app.open_search();
    key(&mut app, Key::Down, NO);
    key(&mut app, Key::Down, NO);
    key(&mut app, Key::Enter, NO);
    assert!(!app.search_active());
    assert_eq!(app.current_location(), &Location::parse(r"C:\data\sub").unwrap());
}

#[test]
fn header_clicks_switch_the_order_and_escape_closes() {
    let mut app = app();
    app.open_search();
    requests(&mut app);
    let size = app.search_layout().cols[2];
    let click = |app: &mut App| {
        let (x, y) = (size.x + size.w / 2.0, size.y + size.h / 2.0);
        app.on_event(&Event::MouseDown { x, y, button: MouseButton::Left, clicks: 1 }, t0());
        app.on_event(&Event::MouseUp { x, y, button: MouseButton::Left }, t0());
    };
    click(&mut app);
    assert!(matches!(&requests(&mut app)[..], [SearchRequest::Query { sort: SearchSort { key: SearchKey::Size, descending: false }, .. }]));
    click(&mut app);
    assert!(matches!(&requests(&mut app)[..], [SearchRequest::Query { sort: SearchSort { key: SearchKey::Size, descending: true }, .. }]));
    key(&mut app, Key::Escape, NO);
    assert!(!app.search_active());
    assert!(app.drain_effects().any(|e| matches!(e, Effect::Title(t) if t.ends_with("— Flit") && !t.starts_with("Search"))));
    key(&mut app, Key::Char('E'), CTRL);
    assert!(requests(&mut app).is_empty(), "reopening keeps the previous query and results");
    key(&mut app, Key::Char('T'), CTRL);
    assert!(!app.search_active());
    assert_eq!(app.tabs().len(), 2);
}

#[test]
fn status_tracks_the_backend_and_numbers_are_grouped() {
    let mut app = app();
    app.open_search();
    let volumes = vec![IndexVolume { id: "C:".into(), files: 1234, state: IndexState::Scanning(0.5) }];
    app.on_search_event(SearchEvent::Status { backend: SearchBackend::Fallback, volumes: volumes.clone(), note: Some("down".into()) }, t0());
    assert_eq!((app.search.backend, &app.search.volumes), (SearchBackend::Fallback, &volumes));
    app.on_search_event(SearchEvent::Error { message: "invalid regex".into() }, t0());
    frame(&mut app);
    assert_eq!(app.search.error.as_deref(), Some("invalid regex"));
    assert_eq!(thousands(0), "0");
    assert_eq!(thousands(999), "999");
    assert_eq!(thousands(1234567), "1,234,567");
}
