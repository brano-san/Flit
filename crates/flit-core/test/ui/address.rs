use super::*;
use crate::input::{Event, MouseButton};
use crate::listing::{attr, DirChange, FsError, Listing, Meta};
use crate::sort::{self, CaseMode, SortKeys, ViewSpec};
use crate::text::tests::Mono;
use crate::ui::{MenuItem, Persisted};
use crate::open::Open;

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

fn submit(app: &mut App, text: &str) -> Vec<Effect> {
    key(app, Key::Char('L'), CTRL);
    app.edit.set(text);
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

#[test]
fn suggestions_come_from_subfolders_and_typed_paths() {
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[("alpha", true), ("beta", true), ("apple.txt", false)], None);
    app.share_typed(Arc::new(Mutex::new(vec![r"C:\x\alpha\deep".into(), r"D:\elsewhere".into()])));
    key(&mut app, Key::Char('L'), CTRL);
    app.layout();
    assert!(app.addr.items.is_empty());
    app.edit.set(r"C:\x\a");
    app.layout();
    assert_eq!(app.addr.items, [(r"C:\x\alpha\deep".to_owned(), true), (r"C:\x\alpha\".to_owned(), false)]);
    assert_eq!(app.addr.rects.len(), 2);
    let r = app.addr.rects[1];
    assert_eq!(app.hit(r.x + 20.0, r.y + 5.0), Hit::Suggestion(1));
    key(&mut app, Key::Down, NO);
    key(&mut app, Key::Down, NO);
    assert_eq!(app.addr.sel, Some(1));
    key(&mut app, Key::Enter, NO);
    let fx: Vec<Effect> = app.drain_effects().collect();
    assert!(fx.iter().any(|e| matches!(e, Effect::Open(o) if o.paths == [br"C:\x\alpha\".to_vec()])));
    assert!(app.addr.items.is_empty() || !app.editing);

    key(&mut app, Key::Char('L'), CTRL);
    app.layout();
    app.edit.set(r"D:\data\s");
    app.layout();
    assert!(app.addr.items.is_empty());
    let p = app.addr.probe.as_ref().unwrap();
    assert_eq!(p.loc, loc(r"D:\data"));
    p.slot.publish(snapshot(p.gen, &p.loc, &[("src", true), ("docs", true), ("setup.exe", false)], None));
    app.poll();
    app.layout();
    assert_eq!(app.addr.items, [(r"D:\data\src\".to_owned(), false)]);
    key(&mut app, Key::Tab, NO);
    assert_eq!(app.edit.text, r"D:\data\src\");
    key(&mut app, Key::Escape, NO);
    assert!(app.editing);
    key(&mut app, Key::Escape, NO);
    assert!(!app.editing);
}

fn type_address(app: &mut App, text: &str) {
    key(app, Key::Char('L'), CTRL);
    app.layout();
    app.edit.set(text);
    app.layout();
}

fn answer_probe(app: &mut App, expect: &str, items: &[(&str, bool)], error: Option<FsErrorKind>) {
    let p = app.addr.probe.as_ref().expect("a probe");
    assert_eq!(p.loc, loc(expect));
    p.slot.publish(snapshot(p.gen, &p.loc, items, error));
    app.poll();
    app.layout();
}

fn suggested(app: &App) -> Vec<&str> {
    app.addr.items.iter().map(|(t, _)| t.as_str()).collect()
}

#[test]
fn a_missing_folder_suggests_close_names_from_its_nearest_existing_parent() {
    let mut app = app_at(r"D:\x");
    type_address(&mut app, r"C:\User\");
    assert!(app.addr.items.is_empty());
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
    assert_eq!(suggested(&app), [r"C:\Users\brano\Documents\"]);

    app.share_typed(Arc::new(Mutex::new(vec![r"C:\Users\brano\Documents\Work".into(), r"C:\Windows".into()])));
    type_address(&mut app, r"C:\Usrs\brano\Docs");
    assert_eq!(suggested(&app), [r"C:\Users\brano\Documents\Work"]);
}

#[test]
fn suggestions_rank_prefix_then_recent_then_closeness_then_length() {
    let mut app = app_at(r"C:\x");
    deliver(&mut app, &[("dxo", true), ("redo", true), ("dotfiles_long", true), ("Downloads", true), ("docs", true), ("dog.txt", false), ("zzz", true)], None);
    app.share_typed(Arc::new(Mutex::new(vec![r"C:\x\Downloads".into()])));
    type_address(&mut app, r"C:\x\do");
    assert_eq!(suggested(&app), [r"C:\x\Downloads", r"C:\x\docs\", r"C:\x\dotfiles_long\", r"C:\x\redo\", r"C:\x\dxo\"]);
    assert!(app.addr.items[0].1);
}

#[test]
fn up_to_25_suggestions_scroll_ten_at_a_time() {
    let mut app = app_at(r"C:\x");
    let names: Vec<String> = (0..30).map(|k| format!("d{k:02}")).collect();
    let items: Vec<(&str, bool)> = names.iter().map(|n| (n.as_str(), true)).collect();
    deliver(&mut app, &items, None);
    type_address(&mut app, r"C:\x\d");
    frame(&mut app);
    assert_eq!(app.addr.items.len(), 25);
    assert_eq!(app.addr.rects.len(), 10);
    let r = app.addr.rects[0];
    assert_eq!(app.hit(r.x + 5.0, r.y + 5.0), Hit::Suggestion(0));
    app.suggestions_wheel(-1.0);
    frame(&mut app);
    assert_eq!(app.hit(r.x + 5.0, r.y + 5.0), Hit::Suggestion(3));
    app.suggestions_wheel(-100.0);
    frame(&mut app);
    assert_eq!(app.hit(r.x + 5.0, r.y + 5.0), Hit::Suggestion(15));
    app.suggestions_wheel(100.0);
    for _ in 0..12 {
        app.address_key(Key::Down, Mods::default());
    }
    frame(&mut app);
    assert_eq!(app.hit(r.x + 5.0, r.y + 5.0), Hit::Suggestion(2));
}

#[test]
fn chevrons_list_subfolders_and_the_ellipsis_lists_hidden_crumbs() {
    let mut app = app_at(r"C:\x\y");
    deliver(&mut app, &[("inner", true), ("note.txt", false)], None);
    frame(&mut app);
    let n = app.tab().crumbs.len();
    assert_eq!(app.addr.chevrons.len(), n);
    assert_eq!(app.addr.chevrons[n - 1].w, 0.0, "the last crumb in the bar has no chevron after it");
    let c = app.addr.chevrons[n - 2];
    assert!(c.w > 0.0);
    assert_eq!(app.hit(c.x + c.w / 2.0, c.y + c.h / 2.0), Hit::CrumbMenu(n - 2));
    app.crumb_menu(n - 1);
    assert_eq!(menu(&mut app), [("inner".to_owned(), false)]);
    app.crumb_menu(n - 2);
    let p = &app.addr.menu.as_ref().unwrap().0;
    assert_eq!(p.loc, loc(r"C:\x"));
    p.slot.publish(snapshot(p.gen, &p.loc, &[("y", true), ("z", true), ("x.txt", false)], None));
    app.drain_effects().for_each(drop);
    app.poll();
    assert_eq!(menu(&mut app), [("y".to_owned(), true), ("z".to_owned(), false)]);
    app.on_event(&Event::MenuCommand(1), Instant::now());
    assert_eq!(app.tab().location(), &loc(r"C:\x\z"));

    let mut deep = app_at(r"C:\one\two\three\four\five\six\seven\eight\nine\ten");
    deep.on_event(&Event::Resized { width: 520, height: 600 }, Instant::now());
    frame(&mut deep);
    let o = deep.addr.overflow;
    assert!(o.w > 0.0);
    assert_eq!(deep.hit(o.x + o.w / 2.0, o.y + o.h / 2.0), Hit::CrumbOverflow);
    deep.overflow_menu();
    let hidden = menu(&mut deep);
    assert_eq!(hidden.first().map(|h| h.0.as_str()), Some("This PC"));
    deep.on_event(&Event::MenuCommand(0), Instant::now());
    assert_eq!(deep.tab().location(), &Location::computer());
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
    app.drain_effects().for_each(drop);
    app.crumb_menu(app.tab().crumbs.len() - 1);
    assert_eq!(menu(&mut app), [("c".to_owned(), true), ("d".to_owned(), false)]);
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
    assert_eq!(app.edit.text, r"C:\a\b");
    assert_eq!(app.current_location(), &loc(r"C:\a\b"));
    assert_eq!(app.window_title(), "C:\\a\\b \u{2014} Flit");
}

#[test]
fn the_dimmed_tail_collapses_before_real_crumbs_overflow() {
    let mut app = app_at(r"C:\one\two\a_rather_long_folder_name\another_rather_long_folder_name\yet_another_long_one");
    for _ in 0..3 {
        app.go_up(Instant::now());
    }
    assert_eq!(ghosts(&app).len(), 3);
    app.on_event(&Event::Resized { width: 560, height: 600 }, Instant::now());
    frame(&mut app);
    let real = app.tab().crumbs.len();
    assert_eq!(app.addr.overflow.w, 0.0, "real crumbs stay in the bar");
    assert!(app.layout.crumbs[..real].iter().all(|r| r.w > 0.0));
    assert_eq!(app.layout.crumbs[real + 2].w, 0.0, "the deepest dimmed crumb goes first");
    app.on_event(&Event::Resized { width: 1400, height: 600 }, Instant::now());
    frame(&mut app);
    assert!(app.layout.crumbs.iter().all(|r| r.w > 0.0));
}

#[test]
fn folder_suggestions_show_system_icons_and_history_keeps_its_glyph() {
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
