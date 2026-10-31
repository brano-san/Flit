use super::*;
use crate::input::{Event, Key, Mods, MouseButton};
use crate::listing::{attr, DirChange, FsError, Listing, Meta};
use crate::sort::{self, CaseMode, SortKeys, ViewSpec};
use crate::text::tests::Mono;
use crate::ui::{MenuItem, Persisted};
use crate::open::Open;
use crate::ui::goto::{Kind as GoKind, Line as GoLine};
use crate::ui::palette::Pick;
use crate::ui::sidebar::{Drive, KnownFolder, Places, Section};

const NO: Mods = Mods { ctrl: false, shift: false, alt: false };
const CTRL: Mods = Mods { ctrl: true, shift: false, alt: false };

fn loc(s: &str) -> Location {
    Location::parse(s).unwrap()
}

fn app_at(path: &str) -> App {
    let mut app = App::new(Persisted { path: loc(path), ..Default::default() }, Vec::new(), 0, false, 1.0, (1000, 600), Instant::now());
    app.drain_effects().for_each(drop);
    app
}

fn snapshot(gen: u64, path: &Location, items: &[(&str, bool)], error: Option<FsErrorKind>) -> Arc<DirSnapshot> {
    let mut listing = Listing::default();
    let mut keys = SortKeys::new(CaseMode::Insensitive);
    for &(n, dir) in items {
        let i = listing.push(n.as_bytes(), Meta { attrs: if dir { attr::DIR } else { 0 }, ..Default::default() });
        keys.push(listing.name(i));
    }
    let mut order = Vec::new();
    sort::sort(&listing, &keys, ViewSpec::default(), &mut order);
    let error = error.map(|kind| FsError { kind, code: 2 });
    Arc::new(DirSnapshot { gen, path: path.path().clone(), listing, keys, order, view: ViewSpec::default(), done: true, error, elapsed: Duration::ZERO })
}

fn deliver(app: &mut App, items: &[(&str, bool)], error: Option<FsErrorKind>) {
    let t = app.tab();
    t.slot.publish(snapshot(t.gen, t.location(), items, error));
    app.poll();
}

fn key(app: &mut App, key: Key, mods: Mods) {
    app.on_event(&Event::Key { key, mods, pressed: true }, Instant::now());
}

fn set_field(app: &mut App, text: &str) {
    app.nav.palette.as_mut().expect("an open window").field.edit.set(text);
    app.go_refresh();
    app.layout();
}

fn submit(app: &mut App, text: &str) -> Vec<Effect> {
    key(app, Key::Char('L'), CTRL);
    set_field(app, text);
    key(app, Key::Enter, NO);
    app.drain_effects().collect()
}

fn handle_open(app: &mut App, fx: Vec<Effect>) {
    let cmd: Open = fx.into_iter().find_map(|e| if let Effect::Open(c) = e { Some(c) } else { None }).expect("an open command");
    app.open(&cmd, &|_| None, Instant::now());
}

fn launches(app: &mut App) -> Vec<Launch> {
    app.drain_effects().filter_map(|e| if let Effect::Provider { req: ProviderRequest::Launch { launch, .. }, .. } = e { Some(launch) } else { None }).collect()
}

fn menu(app: &mut App) -> Vec<(String, bool)> {
    let m = app.drain_effects().find_map(|e| if let Effect::ShellMenu(m) = e { Some(m.menu) } else { None }).expect("a menu");
    m.into_iter().filter_map(|i| if let MenuItem::Command { label, checked, .. } = i { Some((label, checked)) } else { None }).collect()
}

fn frame(app: &mut App) {
    let mut g = Gfx::new(Mono, 1024);
    app.frame(&mut g, Instant::now());
}

#[test]
fn urls_open_in_their_program_and_are_remembered() {
    let mut app = app_at(r"C:\x");
    let fx = submit(&mut app, "  https://example.com/a?b=c ");
    assert!(!fx.iter().any(|e| matches!(e, Effect::Open(_))));
    let launched: Vec<Launch> = fx.into_iter().filter_map(|e| if let Effect::Provider { req: ProviderRequest::Launch { launch, .. }, .. } = e { Some(launch) } else { None }).collect();
    assert_eq!(launched, [Launch::Run { program: "https://example.com/a?b=c".into(), args: String::new(), dir: None }]);
    assert_eq!(app.addr.typed.lock().unwrap().first().map(String::as_str), Some("https://example.com/a?b=c"));
    for text in ["mailto:someone@example.com", "ms-settings:display"] {
        assert!(is_url(text), "{text}");
    }
    for text in [r"C:\x", "shell:Downloads", "wsl://Ubuntu/home", "file:///C:/x", "::{20D04FE0-3AEA-1069-A2D8-08002B30309D}", "notepad"] {
        assert!(!is_url(text), "{text}");
    }
}

#[test]
fn a_typed_command_runs_when_no_folder_has_that_name() {
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[("f.txt", false)], None);
    let fx = submit(&mut app, r#"notepad "my file.txt""#);
    handle_open(&mut app, fx);
    assert_eq!(app.tab().location(), &loc(r#"C:\x\notepad "my file.txt""#));
    assert!(launches(&mut app).is_empty());
    deliver(&mut app, &[], Some(FsErrorKind::NotFound));
    assert_eq!(app.tab().location(), &loc(r"C:\x"));
    assert!(!app.tab().history.can_back());
    assert_eq!(launches(&mut app), [Launch::Run { program: "notepad".into(), args: "\"my file.txt\"".into(), dir: Some(FsPath::parse(r"C:\x").unwrap()) }]);
    deliver(&mut app, &[], Some(FsErrorKind::NotFound));
    assert!(launches(&mut app).is_empty(), "a later failure never runs anything");

    let mut pc = app_at("This PC");
    let fx = submit(&mut pc, "calc");
    handle_open(&mut pc, fx);
    assert_eq!(launches(&mut pc), [Launch::Run { program: "calc".into(), args: String::new(), dir: None }]);
    assert_eq!(split_command(r#""C:\Program Files\x.exe" -a  b"#), (r"C:\Program Files\x.exe".to_owned(), "-a  b".to_owned()));
}

#[test]
fn a_typed_file_path_opens_the_file_and_stays_in_the_folder() {
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[("f.txt", false)], None);
    let fx = submit(&mut app, r"C:\x\f.txt");
    handle_open(&mut app, fx);
    app.drain_effects().for_each(drop);
    deliver(&mut app, &[], Some(FsErrorKind::NotADirectory));
    assert_eq!(app.tab().location(), &loc(r"C:\x"));
    let opened: Vec<String> = app.drain_effects().filter_map(|e| if let Effect::Provider { req: ProviderRequest::Open { path, .. }, .. } = e { Some(path.display().into_owned()) } else { None }).collect();
    assert_eq!(opened, [r"C:\x\f.txt"]);
}

#[test]
fn a_file_location_shows_its_folder_with_the_file_selected() {
    let mut app = app_at(r"C:\x\f.txt");
    deliver(&mut app, &[], Some(FsErrorKind::NotADirectory));
    assert_eq!(app.tab().location(), &loc(r"C:\x"));
    assert_eq!(app.tab().loading.as_ref().and_then(|l| l.select.clone()), Some(b"f.txt".to_vec()));
    assert!(!app.drain_effects().any(|e| matches!(e, Effect::Provider { req: ProviderRequest::Open { .. } | ProviderRequest::Launch { .. }, .. })));
}

fn go(app: &App) -> &crate::ui::goto::Go {
    app.nav.palette.as_ref().and_then(|p| p.go.as_ref()).expect("a go window")
}

fn shown(app: &App) -> Vec<String> {
    let g = go(app);
    g.lines.iter().filter_map(|l| if let GoLine::Row(k, _) = l { Some(g.rows[*k].label.clone()) } else { None }).collect()
}

fn selected(app: &App) -> Option<String> {
    let p = app.nav.palette.as_ref()?;
    let g = p.go.as_ref().filter(|g| g.picked)?;
    match g.lines.get(p.sel)? {
        GoLine::Row(k, _) => Some(g.rows[*k].label.clone()),
        GoLine::Header(s) => Some(format!("[{}]", s.id())),
    }
}

fn field(app: &App) -> &str {
    app.nav.palette.as_ref().expect("an open window").field.text()
}

fn type_address(app: &mut App, text: &str) {
    key(app, Key::Char('L'), CTRL);
    set_field(app, text);
}

fn answer_probe(app: &mut App, expect: &str, items: &[(&str, bool)], error: Option<FsErrorKind>) {
    let p = app.addr.probe.as_ref().expect("a probe");
    assert_eq!(p.loc, loc(expect));
    p.slot.publish(snapshot(p.gen, &p.loc, items, error));
    app.poll();
    app.layout();
}

fn suggested(app: &App) -> Vec<String> {
    let g = go(app);
    g.lines.iter().filter_map(|l| if let GoLine::Row(k, _) = l { Some(&g.rows[*k]) } else { None }).filter(|r| matches!(r.kind, GoKind::Typed(_) | GoKind::Suggested(_))).map(|r| r.label.clone()).collect()
}

const ALT: Mods = Mods { ctrl: false, shift: false, alt: true };

#[test]
fn the_address_window_lists_the_folder_then_typed_paths_and_tab_expands() {
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[("alpha", true), ("beta", true), ("apple.txt", false)], None);
    app.share_typed(Arc::new(Mutex::new(vec![r"C:\x\alpha\deep".into(), r"D:\elsewhere".into()])));
    key(&mut app, Key::Char('L'), CTRL);
    assert_eq!(app.nav.palette.as_ref().map(|p| p.pick.clone()), Some(Pick::GoTo));
    assert_eq!(field(&app), r"C:\x\");
    assert_eq!(shown(&app)[..3], ["alpha".to_owned(), "beta".to_owned(), "apple.txt".to_owned()]);
    assert_eq!(selected(&app), None, "a typed address is not replaced by a row until the arrows pick one");
    set_field(&mut app, r"C:\x\a");
    let rows = shown(&app);
    assert_eq!(rows[..2], ["alpha".to_owned(), "apple.txt".to_owned()]);
    assert!(rows.contains(&r"C:\x\alpha\deep".to_owned()), "{rows:?}");
    assert!(!rows.contains(&r"C:\x\alpha\".to_owned()), "a folder already listed is not suggested twice: {rows:?}");
    key(&mut app, Key::Down, NO);
    assert_eq!(selected(&app).as_deref(), Some("alpha"));
    key(&mut app, Key::Tab, NO);
    assert_eq!(field(&app), r"C:\x\alpha\");
    key(&mut app, Key::Escape, NO);
    assert!(!app.go_open());

    type_address(&mut app, r"C:\x\b");
    key(&mut app, Key::Down, NO);
    key(&mut app, Key::Enter, NO);
    assert!(!app.go_open());
    assert_eq!(app.tab().location(), &loc(r"C:\x\beta"));
}

#[test]
fn goto_starts_with_quick_access_groups_that_fold() {
    let mut app = app_at(r"C:\x");
    let c = Drive { path: FsPath::parse(r"C:\").unwrap(), label: "Disk (C:)".into(), total: 100, free: 25, removable: false, unmounted: false, kind: None, remote: None };
    app.set_places(Arc::new(Places { drives: vec![c], folders: vec![(KnownFolder::Downloads, loc(r"C:\Users\u\Downloads"))], ..Default::default() }));
    app.nav.sidebar.state.pinned = vec![loc(r"C:\work")];
    app.nav.sidebar.rebuild();
    key(&mut app, Key::Char('P'), CTRL);
    assert_eq!(field(&app), "");
    let heads: Vec<Section> = go(&app).lines.iter().filter_map(|l| if let GoLine::Header(s) = l { Some(*s) } else { None }).collect();
    assert_eq!(heads, [Section::Bookmarks, Section::Storage, Section::Places]);
    assert_eq!(selected(&app).as_deref(), Some("work"), "the first item is selected");
    frame(&mut app);
    assert_eq!(app.nav.palette.as_ref().unwrap().first, 0, "the first header stays in view");
    assert!(shown(&app).contains(&"Downloads".to_owned()));
    let storage = go(&app).lines.iter().position(|l| *l == GoLine::Header(Section::Storage)).unwrap();
    app.go_activate(storage, Instant::now());
    assert!(!shown(&app).contains(&"Disk (C:)".to_owned()), "a header folds its group");
    assert_eq!(selected(&app).as_deref(), Some("[drives]"));
    app.go_activate(storage, Instant::now());
    assert!(shown(&app).contains(&"Disk (C:)".to_owned()));
    set_field(&mut app, "dl");
    assert_eq!(shown(&app).first().map(String::as_str), Some("Downloads"), "typing without a separator filters Quick Access");
    set_field(&mut app, "");
    assert_eq!(selected(&app).as_deref(), Some("work"));
    key(&mut app, Key::Enter, NO);
    assert_eq!(app.tab().location(), &loc(r"C:\work"));
}

#[test]
fn goto_keys_walk_history_and_parents_and_select_in_the_panel() {
    let mut app = app_at(r"C:\a");
    app.navigate(loc(r"C:\b\c"), Instant::now());
    deliver(&mut app, &[("f.txt", false), ("g.txt", false)], None);
    app.drain_effects().for_each(drop);
    key(&mut app, Key::F(4), NO);
    assert!(app.go_open());
    key(&mut app, Key::Left, ALT);
    assert_eq!(app.tab().location(), &loc(r"C:\a"));
    assert_eq!(field(&app), r"C:\a\");
    app.on_event(&Event::MouseDown { x: 1.0, y: 300.0, button: MouseButton::Forward, clicks: 1 }, Instant::now());
    assert_eq!(app.tab().location(), &loc(r"C:\b\c"));
    assert_eq!(field(&app), r"C:\b\c\");
    deliver(&mut app, &[("f.txt", false), ("g.txt", false)], None);
    key(&mut app, Key::Up, ALT);
    assert_eq!(field(&app), r"C:\b\");
    assert_eq!(app.tab().location(), &loc(r"C:\b\c"), "Alt+Up only edits the field");
    set_field(&mut app, r"C:\b\c\g");
    key(&mut app, Key::Down, NO);
    assert_eq!(selected(&app).as_deref(), Some("g.txt"));
    key(&mut app, Key::Enter, CTRL);
    assert!(!app.go_open());
    assert_eq!(app.tab().location(), &loc(r"C:\b\c"));
    assert!(!app.drain_effects().any(|e| matches!(e, Effect::Provider { req: ProviderRequest::Open { .. }, .. })), "Ctrl+Enter selects instead of opening");
}

#[test]
fn a_crumb_drops_down_its_folder_with_the_next_crumb_marked() {
    let mut app = app_at(r"C:\x\y");
    deliver(&mut app, &[("inner", true), ("note.txt", false)], None);
    frame(&mut app);
    let n = app.tab().crumbs.len();
    let r = app.layout.crumb_chevrons[n - 2];
    assert!(r.w > 0.0);
    assert_eq!(app.hit(r.x + r.w / 2.0, r.y + r.h / 2.0), Hit::CrumbChevron(n - 2));
    click_at(&mut app, r);
    assert_eq!(app.nav.palette.as_ref().map(|p| p.pick.clone()), Some(Pick::Crumb(n - 2)));
    answer_probe(&mut app, r"C:\x", &[("y", true), ("z", true), ("x.txt", false)], None);
    assert_eq!(shown(&app), ["y", "z", "x.txt"]);
    assert_eq!(selected(&app).as_deref(), Some("y"), "the folder the path goes through is marked and selected");
    let p = app.nav.palette.as_ref().unwrap();
    assert_eq!(app.go_placeholder(p), "Filter 3 files...");
    assert!(p.rect.y >= r.bottom(), "the dropdown opens under its chevron");
    key(&mut app, Key::Down, NO);
    key(&mut app, Key::Enter, NO);
    assert_eq!(app.tab().location(), &loc(r"C:\x\z"));

    let mut keys = app_at(r"C:\x\y");
    deliver(&mut keys, &[("inner", true)], None);
    frame(&mut keys);
    key(&mut keys, Key::Down, ALT);
    let last = keys.tab().crumbs.len() - 1;
    assert_eq!(keys.nav.palette.as_ref().map(|p| p.pick.clone()), Some(Pick::Crumb(last)));
    assert_eq!(shown(&keys), ["inner"]);
    key(&mut keys, Key::Left, ALT);
    assert_eq!(keys.nav.palette.as_ref().map(|p| p.pick.clone()), Some(Pick::Crumb(last - 1)));
    key(&mut keys, Key::Right, ALT);
    assert_eq!(keys.nav.palette.as_ref().map(|p| p.pick.clone()), Some(Pick::Crumb(last)));
    key(&mut keys, Key::Right, ALT);
    assert_eq!(keys.nav.palette.as_ref().map(|p| p.pick.clone()), Some(Pick::Crumb(last)), "there is no crumb after the last one");
    assert_eq!(keys.tab().location(), &loc(r"C:\x\y"));
}

#[test]
fn a_crumb_click_goes_to_its_folder_without_a_dropdown() {
    let mut app = app_at(r"C:\x\y");
    deliver(&mut app, &[("inner", true)], None);
    frame(&mut app);
    let n = app.tab().crumbs.len();
    assert_eq!(app.layout.crumb_chevrons[n - 1].w, 0.0, "the current folder has no chevron of its own");
    assert!(app.layout.crumb_chevrons[..n - 1].iter().all(|c| c.w > 0.0), "only the separators between crumbs");
    let r = app.layout.crumbs[n - 1];
    click_at(&mut app, r);
    assert!(app.nav.palette.is_none(), "the current folder's crumb opens nothing");
    let r = app.layout.crumbs[n - 2];
    click_at(&mut app, r);
    assert!(app.nav.palette.is_none());
    assert_eq!(app.tab().location(), &loc(r"C:\x"));
}

#[test]
fn the_chevron_right_of_a_crumb_opens_that_crumb_and_closes_on_a_second_click() {
    let mut app = app_at(r"C:\x\y");
    deliver(&mut app, &[("inner", true)], None);
    frame(&mut app);
    let n = app.tab().crumbs.len();
    let (crumb, chev) = (app.layout.crumbs[n - 3], app.layout.crumb_chevrons[n - 3]);
    assert!(chev.x >= crumb.right() - 0.5 && chev.right() <= app.layout.crumbs[n - 2].x + 0.5, "the chevron sits between its crumb and the next one");
    click_at(&mut app, chev);
    assert_eq!(app.nav.palette.as_ref().map(|p| p.pick.clone()), Some(Pick::Crumb(n - 3)));
    assert_eq!(app.tab().location(), &loc(r"C:\x\y"));
    frame(&mut app);
    let chev = app.layout.crumb_chevrons[n - 3];
    click_at(&mut app, chev);
    assert!(app.nav.palette.is_none());
}

#[test]
fn an_open_chevron_turns_down_and_back_over_a_short_animation() {
    let t0 = Instant::now();
    let mut app = app_at(r"C:\x\y");
    deliver(&mut app, &[("inner", true)], None);
    app.frame(&mut Gfx::new(Mono, 1024), t0);
    let i = app.tab().crumbs.len() - 2;
    assert_eq!(app.addr.turn.angle(i, t0), 0.0);
    app.open_crumb(i);
    app.frame(&mut Gfx::new(Mono, 1024), t0);
    assert_eq!(app.addr.turn.angle(i, t0), 0.0, "it starts pointing right");
    assert!(app.motion);
    let mid = t0 + TURN / 2;
    app.frame(&mut Gfx::new(Mono, 1024), mid);
    let half = app.addr.turn.angle(i, mid);
    assert!(half > 0.0 && half < 90.0, "{half}");
    let end = t0 + TURN;
    app.frame(&mut Gfx::new(Mono, 1024), end);
    assert_eq!(app.addr.turn.angle(i, end), 90.0);
    assert!(!app.motion, "no redraws once it points down");
    app.close_palette();
    app.frame(&mut Gfx::new(Mono, 1024), end);
    assert!(app.motion);
    let back = end + TURN;
    app.frame(&mut Gfx::new(Mono, 1024), back);
    assert_eq!(app.addr.turn.angle(i, back), 0.0);
    assert!(!app.motion);
    app.animations = false;
    app.open_crumb(i);
    app.frame(&mut Gfx::new(Mono, 1024), back);
    assert_eq!(app.addr.turn.angle(i, back), 90.0, "without animations it turns at once");
    assert!(!app.motion);
}

#[test]
fn a_long_path_scrolls_with_arrows_and_keeps_the_current_folder_in_view() {
    let mut deep = app_at(r"C:\one\two\three\four\five\six\seven\eight\nine\ten");
    deep.on_event(&Event::Resized { width: 520, height: 600 }, Instant::now());
    frame(&mut deep);
    let [left, right] = deep.layout.crumb_arrows;
    assert!(left.w > 0.0 && right.w > 0.0);
    assert_eq!(deep.hit(left.x + left.w / 2.0, left.y + left.h / 2.0), Hit::CrumbScroll(false));
    let n = deep.tab().crumbs.len();
    assert!(deep.layout.crumbs[n - 1].w > 0.0, "the current folder is in view");
    assert_eq!(deep.layout.crumbs[0].w, 0.0);
    for _ in 0..20 {
        click_at(&mut deep, left);
        frame(&mut deep);
    }
    assert!(deep.layout.crumbs[0].w > 0.0, "scrolling left reaches the start");
    deep.navigate(loc(r"C:\one\two\three\four\five\six\seven\eight\nine"), Instant::now());
    frame(&mut deep);
    assert_eq!(deep.addr.shift, 0.0, "a new folder starts at the end again");

    let mut short = app_at(r"C:\x");
    frame(&mut short);
    assert_eq!(short.layout.crumb_arrows[0].w, 0.0);
}

#[test]
fn a_missing_folder_suggests_close_names_from_its_nearest_existing_parent() {
    let mut app = app_at(r"D:\x");
    type_address(&mut app, r"C:\User\");
    assert!(suggested(&app).is_empty());
    answer_probe(&mut app, r"C:\User", &[], Some(FsErrorKind::NotFound));
    answer_probe(&mut app, r"C:\", &[("Users", true), ("Windows", true), ("Program Files", true), ("pagefile.sys", false)], None);
    assert_eq!(suggested(&app), [r"C:\Users\"]);

    type_address(&mut app, r"C:\Usr\brno\Dcs");
    answer_probe(&mut app, r"C:\Usr\brno", &[], Some(FsErrorKind::NotFound));
    answer_probe(&mut app, r"C:\Usr", &[], Some(FsErrorKind::NotFound));
    answer_probe(&mut app, r"C:\", &[("Users", true), ("Windows", true)], None);
    assert_eq!(suggested(&app), [r"C:\Users\"]);
}

#[test]
fn a_typo_in_the_last_name_still_finds_the_folder() {
    let mut app = app_at(r"D:\x");
    type_address(&mut app, r"C:\Users\brano\Dcuments");
    answer_probe(&mut app, r"C:\Users\brano", &[("Desktop", true), ("Documents", true), ("Downloads", true), ("notes.txt", false)], None);
    assert!(shown(&app).contains(&r"C:\Users\brano\Documents\".to_owned()) || shown(&app).contains(&"Documents".to_owned()), "{:?}", shown(&app));

    app.share_typed(Arc::new(Mutex::new(vec![r"C:\Users\brano\Documents\Work".into(), r"C:\Windows".into()])));
    type_address(&mut app, r"C:\Usrs\brano\Docs");
    assert_eq!(suggested(&app), [r"C:\Users\brano\Documents\Work"]);
}

#[test]
fn suggestions_rank_prefix_then_recent_then_closeness_then_length_and_stop_at_25() {
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[("dxo", true), ("redo", true), ("dotfiles_long", true), ("Downloads", true), ("docs", true), ("dog.txt", false), ("zzz", true)], None);
    app.share_typed(Arc::new(Mutex::new(vec![r"C:\x\Downloads".into()])));
    let got = app.suggest(r"C:\x\do");
    assert_eq!(got.iter().map(|g| g.0.as_str()).collect::<Vec<_>>(), [r"C:\x\Downloads", r"C:\x\docs\", r"C:\x\dotfiles_long\", r"C:\x\redo\", r"C:\x\dxo\"]);
    assert!(got[0].1);

    let mut many = app_at(r"C:\x");
    let names: Vec<String> = (0..30).map(|k| format!("d{k:02}")).collect();
    let items: Vec<(&str, bool)> = names.iter().map(|n| (n.as_str(), true)).collect();
    deliver(&mut many, &items, None);
    assert_eq!(many.suggest(r"C:\x\d").len(), 25);
}

#[test]
fn back_and_forward_offer_their_history_on_right_click_and_long_press() {
    let mut app = app_at(r"C:\a");
    for p in [r"C:\b", r"C:\c"] {
        app.navigate(loc(p), Instant::now());
    }
    app.drain_effects().for_each(drop);
    let back = app.layout.nav[0];
    let (x, y) = (back.x + back.w / 2.0, back.y + back.h / 2.0);
    app.on_event(&Event::MouseDown { x, y, button: MouseButton::Right, clicks: 1 }, Instant::now());
    app.on_event(&Event::MouseUp { x, y, button: MouseButton::Right }, Instant::now());
    assert_eq!(menu(&mut app), [("b".to_owned(), false), ("a".to_owned(), false)]);
    app.on_event(&Event::MenuCommand(1), Instant::now());
    assert_eq!(app.tab().location(), &loc(r"C:\a"));
    let fwd = app.layout.nav[1];
    let (x, y) = (fwd.x + fwd.w / 2.0, fwd.y + fwd.h / 2.0);
    let start = Instant::now();
    app.drain_effects().for_each(drop);
    app.on_event(&Event::MouseMove { x, y }, start);
    app.on_event(&Event::MouseDown { x, y, button: MouseButton::Left, clicks: 1 }, start);
    app.on_event(&Event::Timer, start + LONG_PRESS);
    assert_eq!(menu(&mut app), [("b".to_owned(), false), ("c".to_owned(), false)]);
    app.on_event(&Event::MouseUp { x, y, button: MouseButton::Left }, start + LONG_PRESS);
    assert_eq!(app.tab().location(), &loc(r"C:\a"));
    app.on_event(&Event::MenuCommand(1), Instant::now());
    assert_eq!(app.tab().location(), &loc(r"C:\c"));
    assert!(!app.tab().history.can_forward());
}

#[test]
fn a_drive_without_media_offers_to_eject_it() {
    let mut app = app_at(r"E:\");
    deliver(&mut app, &[], Some(FsErrorKind::NoMedia));
    frame(&mut app);
    let r = app.eject_rect().unwrap();
    assert_eq!(app.hit(r.x + 5.0, r.y + 5.0), Hit::Eject);
    app.on_event(&Event::MouseDown { x: r.x + 5.0, y: r.y + 5.0, button: MouseButton::Left, clicks: 1 }, Instant::now());
    app.on_event(&Event::MouseUp { x: r.x + 5.0, y: r.y + 5.0, button: MouseButton::Left }, Instant::now());
    assert_eq!(launches(&mut app), [Launch::Eject(FsPath::parse(r"E:\").unwrap())]);

    let mut share = app_at(r"\\srv\share");
    deliver(&mut share, &[], Some(FsErrorKind::NoMedia));
    assert_eq!(share.eject_rect(), None);
    let mut busy = app_at(r"F:\");
    deliver(&mut busy, &[], Some(FsErrorKind::AccessDenied));
    assert_eq!(busy.eject_rect(), None);
}

#[test]
fn restoring_a_crashed_session_is_offered_and_never_forced() {
    let mut app = app_at(r"C:\x");
    app.offer_restore();
    assert_eq!(app.asking(), Some("restore"));
    key(&mut app, Key::Escape, NO);
    assert_eq!(app.asking(), None);
    assert!(!app.drain_effects().any(|e| matches!(e, Effect::RestoreSession)));
    app.offer_restore();
    key(&mut app, Key::Enter, NO);
    assert!(app.drain_effects().any(|e| matches!(e, Effect::RestoreSession)));
}

fn ghosts(app: &App) -> Vec<&str> {
    app.tab().ghosts.iter().map(|g| g.0.as_str()).collect()
}

fn click_at(app: &mut App, r: Rect) {
    let (x, y) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
    app.on_event(&Event::MouseMove { x, y }, Instant::now());
    app.on_event(&Event::MouseDown { x, y, button: MouseButton::Left, clicks: 1 }, Instant::now());
    app.on_event(&Event::MouseUp { x, y, button: MouseButton::Left }, Instant::now());
}

#[test]
fn going_up_keeps_the_deeper_folders_as_a_dimmed_tail() {
    let mut app = app_at(r"C:\a\b\c");
    assert!(ghosts(&app).is_empty());
    app.go_up(Instant::now());
    assert_eq!(app.tab().location(), &loc(r"C:\a\b"));
    assert_eq!(ghosts(&app), ["c"]);
    app.go_up(Instant::now());
    assert_eq!(ghosts(&app), ["b", "c"]);
    assert_eq!(app.tab().title(), "a");
    app.go_history(false, Instant::now());
    assert_eq!(app.tab().location(), &loc(r"C:\a\b"));
    assert_eq!(ghosts(&app), ["c"]);
    app.navigate(loc(r"C:\a"), Instant::now());
    app.navigate(loc(r"C:\a\b"), Instant::now());
    assert_eq!(ghosts(&app), ["c"], "going back down into the tail only moves the current marker");
    frame(&mut app);
    let real = app.tab().crumbs.len();
    assert_eq!(app.layout.crumbs.len(), real + 1);
    assert!(app.layout.crumbs[real].w > 0.0);
    assert_eq!(app.session().trails, [loc(r"C:\a\b\c")]);
}

#[test]
fn clicking_a_dimmed_crumb_navigates_into_it_through_history() {
    let mut app = app_at(r"C:\a\b\c");
    app.go_up(Instant::now());
    app.go_up(Instant::now());
    frame(&mut app);
    let real = app.tab().crumbs.len();
    let b = app.layout.crumbs[real];
    assert_eq!(app.hit(b.x + b.w / 2.0, b.y + b.h / 2.0), Hit::Crumb(real));
    click_at(&mut app, b);
    assert_eq!(app.tab().location(), &loc(r"C:\a\b"));
    assert_eq!(app.tab().history.back_list().last(), Some(&loc(r"C:\a")));
    assert_eq!(ghosts(&app), ["c"]);
    frame(&mut app);
    deliver(&mut app, &[("c", true), ("d", true)], None);
    app.open_crumb(app.tab().crumbs.len() - 1);
    assert_eq!(shown(&app), ["c", "d"]);
    assert_eq!(selected(&app).as_deref(), Some("c"), "a dimmed crumb is marked in its parent's dropdown");
}

#[test]
fn a_different_child_or_an_unrelated_place_replaces_the_tail() {
    let mut app = app_at(r"C:\a\b\c");
    app.go_up(Instant::now());
    app.go_up(Instant::now());
    app.navigate(loc(r"C:\a\x"), Instant::now());
    assert!(ghosts(&app).is_empty());
    app.go_up(Instant::now());
    assert_eq!(ghosts(&app), ["x"]);
    app.navigate(loc(r"D:\elsewhere"), Instant::now());
    app.navigate(loc(r"C:\a"), Instant::now());
    assert!(ghosts(&app).is_empty());
}

#[test]
fn a_dimmed_folder_that_is_gone_drops_with_everything_below_it() {
    let mut app = app_at(r"C:\a\b\c");
    app.go_up(Instant::now());
    app.go_up(Instant::now());
    deliver(&mut app, &[("B", true), ("other", true)], None);
    assert_eq!(ghosts(&app), ["b", "c"], "names match without case");
    let (tab, gen) = (app.tab().id, app.tab().gen);
    let gone = DirChange { kind: crate::listing::ChangeKind::Removed, name: b"B".to_vec(), meta: None };
    app.on_provider_event(crate::provider::ProviderEvent::Changes { tab, gen, changes: vec![gone] }, Instant::now());
    app.poll();
    assert!(ghosts(&app).is_empty());
    assert_eq!(app.tab().trail, loc(r"C:\a"));

    let mut file = app_at(r"C:\a\b");
    file.go_up(Instant::now());
    deliver(&mut file, &[("b", false)], None);
    assert!(ghosts(&file).is_empty(), "a file of that name is no folder to go back into");

    let mut denied = app_at(r"C:\a\b");
    denied.go_up(Instant::now());
    deliver(&mut denied, &[], Some(FsErrorKind::AccessDenied));
    assert_eq!(ghosts(&denied), ["b"], "an unreadable parent proves nothing");
}

#[test]
fn editing_and_copying_the_address_show_only_the_current_folder() {
    let mut app = app_at(r"C:\a\b\c");
    app.go_up(Instant::now());
    key(&mut app, Key::Char('L'), CTRL);
    assert_eq!(field(&app), r"C:\a\b\");
    assert_eq!(app.current_location(), &loc(r"C:\a\b"));
    assert_eq!(app.window_title(), "C:\\a\\b \u{2014} Flit");
}

#[test]
fn the_dimmed_tail_scrolls_out_before_the_current_folder() {
    let mut app = app_at(r"C:\one\two\a_rather_long_folder_name\another_rather_long_folder_name\yet_another_long_one");
    for _ in 0..3 {
        app.go_up(Instant::now());
    }
    assert_eq!(ghosts(&app).len(), 3);
    app.on_event(&Event::Resized { width: 780, height: 600 }, Instant::now());
    frame(&mut app);
    let real = app.tab().crumbs.len();
    assert!(app.layout.crumbs[real - 1].w > 0.0, "the current folder stays in the bar");
    assert_eq!(app.layout.crumbs[real + 2].w, 0.0, "the deepest dimmed crumb is scrolled out first");
    assert!(app.layout.crumb_arrows[1].w > 0.0);
    app.on_event(&Event::Resized { width: 1620, height: 600 }, Instant::now());
    frame(&mut app);
    assert!(app.layout.crumbs.iter().all(|r| r.w > 0.0));
}

#[test]
fn listed_folders_show_system_icons() {
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[("docs", true), ("Downloads", true)], None);
    app.share_typed(Arc::new(Mutex::new(vec![r"C:\x\Downloads".into()])));
    type_address(&mut app, r"C:\x\do");
    app.drain_effects().for_each(drop);
    frame(&mut app);
    let asked: Vec<(String, crate::ui::IconKind)> = app.drain_effects().filter_map(|e| if let Effect::Icon(r) = e { Some((r.path.display().into_owned(), r.kind)) } else { None }).collect();
    assert!(asked.contains(&(r"C:\x\docs\".to_owned(), crate::ui::IconKind::Folder)) || asked.contains(&(r"C:\x\docs".to_owned(), crate::ui::IconKind::Folder)), "{asked:?}");
    assert!(!asked.iter().any(|(p, _)| p.starts_with(r"C:\x\Downloads")), "typed history is not a folder icon: {asked:?}");
}

#[test]
fn goto_opened_before_quick_access_loads_shows_the_late_groups() {
    let mut app = app_at(r"C:\x");
    key(&mut app, Key::Char('P'), CTRL);
    frame(&mut app);
    app.set_pinned(vec![loc(r"C:\work")]);
    frame(&mut app);
    assert!(go(&app).lines.contains(&GoLine::Header(Section::Bookmarks)));
    assert_eq!(app.nav.palette.as_ref().unwrap().first, 0);
}

#[test]
fn compact_paths_start_at_the_home_folder_and_subfolders_mark_the_last_crumb() {
    let mut app = app_at(r"C:\Users\brano\Downloads\1");
    let mut places = (*app.nav.sidebar.places).clone();
    places.folders = vec![(KnownFolder::Home, loc(r"C:\Users\brano"))];
    app.set_places(Arc::new(places));
    app.frame(&mut Gfx::new(Mono, 1024), Instant::now());
    let shown = |app: &App| app.layout.crumbs.iter().map(|r| r.w > 0.0).collect::<Vec<_>>();
    assert_eq!(shown(&app), [false, false, false, true, true, true], "brano › Downloads › 1");
    app.set_compact_paths(false);
    app.frame(&mut Gfx::new(Mono, 1024), Instant::now());
    assert_eq!(shown(&app), [true; 6]);
    let before = app.layout.crumbs[5].w;
    app.tabs[app.cur].subfolders = true;
    app.frame(&mut Gfx::new(Mono, 1024), Instant::now());
    assert!(app.layout.crumbs[5].w > before, "the last crumb carries [...]");
}
