use super::*;
use crate::input::{Event, Key, Mods};
use crate::listing::{DirSnapshot, Listing, Meta};
use crate::location::{Location, ProviderId};
use crate::provider::{AccessChange, Launch, OpReport, ProviderEvent, ProviderRequest};
use crate::sort::{self, CaseMode, SortKeys, ViewSpec};
use crate::ui::properties::Measuring;
use crate::ui::{Effect, Persisted};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn app_with(names: &[&str]) -> App {
    let mut app = App::new(Persisted { path: Location::parse("/home/u").unwrap(), ..Default::default() }, Vec::new(), 0, false, 1.0, (1000, 900), Instant::now());
    app.caps.unix_paths = true;
    let t = app.tab();
    let mut listing = Listing::default();
    let mut keys = SortKeys::new(CaseMode::Insensitive);
    for n in names {
        let i = listing.push(n.as_bytes(), Meta { attrs: 0, size: 1, ..Default::default() });
        keys.push(listing.name(i));
    }
    let mut order = Vec::new();
    sort::sort(&listing, &keys, ViewSpec::default(), &mut order);
    let snap = DirSnapshot { gen: t.gen, path: t.location().path().clone(), listing, keys, order, view: ViewSpec::default(), done: true, error: None, elapsed: Duration::ZERO };
    t.slot.publish(Arc::new(snap));
    app.poll();
    app.drain_effects().for_each(drop);
    app
}

fn key(app: &mut App, k: Key) {
    app.on_event(&Event::Key { key: k, mods: Mods::default(), pressed: true }, Instant::now());
}

fn p(s: &str) -> FsPath {
    FsPath::parse(s).unwrap()
}

fn ops(app: &mut App) -> Vec<(OpId, FileOp)> {
    app.drain_effects().filter_map(|e| if let Effect::Provider { req: ProviderRequest::Op { id, op, .. }, .. } = e { Some((id, op)) } else { None }).collect()
}

#[test]
fn toggling_group_write_and_execution_emits_one_access_change() {
    let mut app = app_with(&["tool"]);
    app.tab_mut().view.as_mut().unwrap().set_selected(0, true);
    app.properties_dialog();
    let id = app
        .drain_effects()
        .find_map(|e| {
            if let Effect::Provider { req: ProviderRequest::Access { id, items, .. }, .. } = e {
                assert_eq!(items, vec![p("/home/u/tool")]);
                Some(id)
            } else {
                None
            }
        })
        .expect("the dialog asks for the current access");
    let accounts = Accounts { users: vec![(1000, "u".into()), (0, "root".into())], groups: vec![(1000, "u".into()), (27, "sudo".into())] };
    let tab = app.tab().id;
    app.on_provider_event(ProviderEvent::Access { tab, id, items: vec![Some(Access { mode: Access::FILE | 0o644, uid: 1000, gid: 1000 })], accounts }, Instant::now());
    let f = app.dialogs.form.as_mut().unwrap();
    let Kind::Properties(Measuring { probe: Probe::Editing(e), .. }) = &f.kind else { panic!("no permissions section") };
    let (at, exec, group_name) = (e.at, e.exec_at().unwrap(), e.group_at());
    if let Control::Flags { items, .. } = &mut f.controls[at + 2] {
        items[1].1 = true;
    }
    f.set_text(group_name, "sudo");
    if let Control::Check { on, .. } = &mut f.controls[exec] {
        *on = true;
    }
    app.form_changed(exec);
    let f = app.dialogs.form.as_ref().unwrap();
    assert_eq!(f.flags(at + 3), vec![true, false, true], "others can read, so they may run it too");
    key(&mut app, Key::Enter);
    assert!(app.dialogs.form.is_none());
    let edit = AccessEdit { mode: ModeEdit { set: 0o131, clear: 0 }, group: Some(27), ..Default::default() };
    assert_eq!(ops(&mut app).into_iter().map(|(_, op)| op).collect::<Vec<_>>(), vec![FileOp::SetAccess { items: vec![p("/home/u/tool")], edit }]);
}

#[test]
fn an_unknown_owner_keeps_the_dialog_open() {
    let mut app = app_with(&["a"]);
    app.tab_mut().view.as_mut().unwrap().set_selected(0, true);
    app.properties_dialog();
    let id = app.drain_effects().find_map(|e| if let Effect::Provider { req: ProviderRequest::Access { id, .. }, .. } = e { Some(id) } else { None }).unwrap();
    let tab = app.tab().id;
    app.on_provider_event(ProviderEvent::Access { tab, id, items: vec![Some(Access { mode: Access::FILE | 0o600, uid: 1000, gid: 1000 })], accounts: Accounts::default() }, Instant::now());
    let f = app.dialogs.form.as_mut().unwrap();
    let Kind::Properties(Measuring { probe: Probe::Editing(e), .. }) = &f.kind else { panic!() };
    let owner = e.owner_at();
    f.set_text(owner, "nobody-here");
    key(&mut app, Key::Enter);
    assert!(app.dialogs.form.as_ref().is_some_and(|f| f.error));
    assert!(ops(&mut app).is_empty());
}

#[test]
fn a_program_without_execute_permission_asks_then_allows_and_runs_it() {
    let mut app = app_with(&["Tool.AppImage"]);
    let (tab, path) = (app.tab().id, p("/home/u/Tool.AppImage"));
    app.on_provider_event(ProviderEvent::NotExecutable { tab, path: path.clone() }, Instant::now());
    assert_eq!(app.asking(), Some("allow_run"));
    key(&mut app, Key::Enter);
    let [(id, op)] = ops(&mut app).try_into().unwrap();
    assert_eq!(op, FileOp::SetAccess { items: vec![path.clone()], edit: AccessEdit { mode: ModeEdit { set: 0o100, clear: 0 }, ..Default::default() } });
    let change = AccessChange { item: path.clone(), from: Access { mode: Access::FILE | 0o644, uid: 1000, gid: 1000 }, to: Access { mode: Access::FILE | 0o744, uid: 1000, gid: 1000 } };
    let report = OpReport::new(FileOp::Access(vec![change]), vec![(path.clone(), path.clone())]);
    app.on_provider_event(ProviderEvent::OpDone { tab, id, error: None, report: Some(report) }, Instant::now());
    assert!(app.drain_effects().any(|e| matches!(e, Effect::Provider { provider: ProviderId::Local, req: ProviderRequest::Open { path: opened, .. } } if opened == path)));
}

#[test]
fn cancelling_the_run_question_changes_nothing() {
    let mut app = app_with(&["Tool.AppImage"]);
    let tab = app.tab().id;
    app.on_provider_event(ProviderEvent::NotExecutable { tab, path: p("/home/u/Tool.AppImage") }, Instant::now());
    key(&mut app, Key::Escape);
    assert_eq!(app.asking(), None);
    assert!(ops(&mut app).is_empty());
}

#[test]
fn folders_in_the_selection_get_separate_masks_for_enclosed_files_and_folders() {
    let mut app = app_with(&["dir", "dir/note"]);
    app.tab_mut().view.as_mut().unwrap().set_selected(0, true);
    app.properties_dialog();
    let id = app.drain_effects().find_map(|e| if let Effect::Provider { req: ProviderRequest::Access { id, .. }, .. } = e { Some(id) } else { None }).unwrap();
    let accounts = Accounts { users: vec![(1000, "u".into())], groups: vec![(1000, "u".into())] };
    let tab = app.tab().id;
    app.on_provider_event(ProviderEvent::Access { tab, id, items: vec![Some(Access { mode: Access::DIR | 0o750, uid: 1000, gid: 1000 })], accounts }, Instant::now());
    let f = app.dialogs.form.as_mut().unwrap();
    let Kind::Properties(Measuring { probe: Probe::Editing(e), .. }) = &f.kind else { panic!("no permissions section") };
    let (files_at, folders_at) = (e.enclosed_files_at().expect("enclosed masks are shown"), e.enclosed_folders_at().unwrap());
    assert_eq!(f.flags(files_at + 1), vec![true, true, false], "enclosed files default to the folder rights without the execute bits");
    assert_eq!(f.flags(folders_at + 1), vec![true, true, true], "enclosed folders start like the folder itself");
    if let Control::Flags { items, .. } = &mut f.controls[files_at + 1] {
        items[2].1 = true;
    }
    if let Control::Flags { items, .. } = &mut f.controls[folders_at + 2] {
        items[1].1 = true;
    }
    key(&mut app, Key::Enter);
    let edit = AccessEdit { mode: ModeEdit::between(0o750, 0o750), owner: None, group: None, enclosed: Some(Enclosed { files: ModeEdit::between(0o600, 0o700), folders: ModeEdit::between(0o750, 0o770) }) };
    assert_eq!(ops(&mut app).into_iter().map(|(_, op)| op).collect::<Vec<_>>(), vec![FileOp::SetAccess { items: vec![p("/home/u/dir")], edit }], "each mask is edited independently");
}

#[test]
fn files_only_selection_has_no_enclosed_masks_and_the_run_check() {
    let mut app = app_with(&["tool"]);
    app.tab_mut().view.as_mut().unwrap().set_selected(0, true);
    app.properties_dialog();
    let id = app.drain_effects().find_map(|e| if let Effect::Provider { req: ProviderRequest::Access { id, .. }, .. } = e { Some(id) } else { None }).unwrap();
    let accounts = Accounts::default();
    let tab = app.tab().id;
    app.on_provider_event(ProviderEvent::Access { tab, id, items: vec![Some(Access { mode: Access::FILE | 0o644, uid: 1000, gid: 1000 })], accounts }, Instant::now());
    let f = app.dialogs.form.as_ref().unwrap();
    let Kind::Properties(Measuring { probe: Probe::Editing(e), .. }) = &f.kind else { panic!("no permissions section") };
    assert!(e.enclosed_files_at().is_none() && e.enclosed_folders_at().is_none(), "a plain file shows no enclosed masks");
    assert!(e.exec_at().is_some(), "a plain file keeps the execute checkbox");
}

#[test]
fn the_run_question_can_run_the_program_in_a_terminal_instead() {
    let mut app = app_with(&["Tool.AppImage"]);
    let (tab, path) = (app.tab().id, p("/home/u/Tool.AppImage"));
    app.on_provider_event(ProviderEvent::NotExecutable { tab, path: path.clone() }, Instant::now());
    assert_eq!(app.asking(), Some("allow_run"));
    let f = app.dialogs.form.as_ref().unwrap();
    assert_eq!(f.buttons, ["Allow and run", "Run in terminal", "Cancel"]);
    key(&mut app, Key::Right);
    key(&mut app, Key::Enter);
    let [(id, op)] = ops(&mut app).try_into().unwrap();
    assert_eq!(op, FileOp::SetAccess { items: vec![path.clone()], edit: AccessEdit { mode: ModeEdit { set: 0o100, clear: 0 }, ..Default::default() } });
    let change = AccessChange { item: path.clone(), from: Access { mode: Access::FILE | 0o644, uid: 1000, gid: 1000 }, to: Access { mode: Access::FILE | 0o744, uid: 1000, gid: 1000 } };
    let report = OpReport::new(FileOp::Access(vec![change]), vec![(path.clone(), path.clone())]);
    app.on_provider_event(ProviderEvent::OpDone { tab, id, error: None, report: Some(report) }, Instant::now());
    assert!(app.drain_effects().any(|e| matches!(&e, Effect::Provider { req: ProviderRequest::Launch { launch, .. }, .. } if *launch == Launch::RunInTerminal { program: path.clone() })), "the terminal variant launches in a terminal, not the normal open");
}

#[test]
fn running_as_root_on_linux_warns_about_root_rather_than_windows_drag_and_drop() {
    let mut app = app_with(&["a"]);
    app.warn_elevated();
    let shown = app.tab().error.clone().unwrap();
    assert!(shown.contains("root") && !shown.contains("Windows"), "{shown}");
    app.caps.unix_paths = false;
    app.warn_elevated();
    let shown = app.tab().error.clone().unwrap();
    assert!(shown.contains("administrator") && shown.contains("Windows"), "Windows keeps its own notice: {shown}");
}

fn launched(app: &mut App) -> Vec<Launch> {
    app.drain_effects().filter_map(|e| if let Effect::Provider { req: ProviderRequest::Launch { launch, .. }, .. } = e { Some(launch) } else { None }).collect()
}

#[test]
fn an_executable_script_asks_to_run_it_in_a_terminal_or_open_it() {
    let path = p("/home/u/build.sh");
    let answers = [(0, Launch::Execute(path.clone())), (1, Launch::RunInTerminal { program: path.clone() }), (2, Launch::Display(path.clone()))];
    for (rights, expected) in answers {
        let mut app = app_with(&["build.sh"]);
        let tab = app.tab().id;
        app.on_provider_event(ProviderEvent::OpenAsk { tab, path: path.clone(), ask: OpenAsk::Script }, Instant::now());
        assert_eq!(app.asking(), Some("run_script"));
        assert_eq!(app.dialogs.form.as_ref().unwrap().buttons, ["Run", "Run in terminal", "Open", "Cancel"]);
        for _ in 0..rights {
            key(&mut app, Key::Right);
        }
        key(&mut app, Key::Enter);
        assert_eq!(launched(&mut app), [expected]);
        assert!(app.dialogs.form.is_none());
    }
}

#[test]
fn cancelling_the_script_question_runs_nothing_and_a_second_question_waits_for_the_first() {
    let mut app = app_with(&["a.sh", "b.sh"]);
    let tab = app.tab().id;
    app.on_provider_event(ProviderEvent::OpenAsk { tab, path: p("/home/u/a.sh"), ask: OpenAsk::Script }, Instant::now());
    app.on_provider_event(ProviderEvent::OpenAsk { tab, path: p("/home/u/b.sh"), ask: OpenAsk::Script }, Instant::now());
    assert!(matches!(&app.dialogs.form.as_ref().unwrap().kind, Kind::RunScript { path, .. } if *path == p("/home/u/a.sh")));
    key(&mut app, Key::Escape);
    assert!(launched(&mut app).is_empty() && app.dialogs.form.is_none());
}

#[test]
fn a_file_no_application_opens_offers_the_application_chooser() {
    let mut app = app_with(&["blob.xyz"]);
    let tab = app.tab().id;
    let apps = vec![crate::provider::AppChoice { name: "Hex".into(), program: p("/usr/share/applications/hex.desktop") }];
    app.on_provider_event(ProviderEvent::OpenAsk { tab, path: p("/home/u/blob.xyz"), ask: OpenAsk::NoApp { mime: "application/x-xyz".into(), apps } }, Instant::now());
    let palette = app.nav.palette.as_ref().expect("the chooser opens");
    assert_eq!(palette.titles.last().map(String::as_str), Some("Hex"));
    assert!(launched(&mut app).is_empty(), "nothing starts before a choice");
}

#[test]
fn an_untrusted_launcher_is_allowed_and_started_without_a_terminal_choice() {
    let mut app = app_with(&["Game.desktop"]);
    let (tab, path) = (app.tab().id, p("/home/u/Game.desktop"));
    app.on_provider_event(ProviderEvent::NotExecutable { tab, path: path.clone() }, Instant::now());
    assert_eq!(app.dialogs.form.as_ref().unwrap().buttons, ["Allow and run", "Cancel"]);
    key(&mut app, Key::Enter);
    let [(_, op)] = ops(&mut app).try_into().unwrap();
    assert_eq!(op, FileOp::SetAccess { items: vec![path], edit: AccessEdit { mode: ModeEdit { set: 0o100, clear: 0 }, ..Default::default() } });
}

#[test]
fn the_four_script_buttons_fit_a_narrow_window_without_overlapping() {
    let mut app = App::new(Persisted { path: Location::parse("/home/u").unwrap(), ..Default::default() }, Vec::new(), 0, false, 1.0, (360, 480), Instant::now());
    app.caps.unix_paths = true;
    let tab = app.tab().id;
    app.on_provider_event(ProviderEvent::OpenAsk { tab, path: p("/home/u/a-rather-long-script-name-for-a-narrow-window.sh"), ask: OpenAsk::Script }, Instant::now());
    let mut g = crate::text::Gfx::new(crate::text::tests::Mono, 1024);
    app.frame(&mut g, Instant::now());
    let f = app.dialogs.form.as_ref().unwrap();
    assert_eq!(f.button_rects.len(), 4);
    assert!(f.button_rects.iter().all(|b| b.x >= f.rect.x && b.right() <= f.rect.right() && b.w > 0.0), "{:?} in {:?}", f.button_rects, f.rect);
    assert!(f.button_rects.windows(2).all(|w| w[0].right() <= w[1].x), "{:?}", f.button_rects);
}

#[test]
fn permissions_show_ok_and_cancel_only_once_changed_and_a_stray_click_keeps_the_change() {
    let mut app = app_with(&["notes"]);
    app.tab_mut().view.as_mut().unwrap().set_selected(0, true);
    app.properties_dialog();
    let id = app.drain_effects().find_map(|e| if let Effect::Provider { req: ProviderRequest::Access { id, .. }, .. } = e { Some(id) } else { None }).unwrap();
    let tab = app.tab().id;
    let accounts = Accounts { users: vec![(1000, "u".into())], groups: vec![(1000, "u".into())] };
    app.on_provider_event(ProviderEvent::Access { tab, id, items: vec![Some(Access { mode: Access::FILE | 0o644, uid: 1000, gid: 1000 })], accounts }, Instant::now());
    app.layout();
    assert_eq!(app.dialogs.form.as_ref().unwrap().buttons, [S::ArchiveClose.t()], "reading the permissions alone changes nothing");
    let f = app.dialogs.form.as_mut().unwrap();
    let Kind::Properties(Measuring { probe: Probe::Editing(e), .. }) = &f.kind else { panic!("no permissions section") };
    let group = e.at + 2;
    if let Control::Flags { items, .. } = &mut f.controls[group] {
        items[1].1 = true;
    }
    app.form_changed(group);
    assert_eq!(app.dialogs.form.as_ref().unwrap().buttons, [S::ArchiveOk.t(), S::WidgetsCancel.t()]);
    app.drain_effects().for_each(drop);
    app.form_down(1.0, 100.0, crate::input::MouseButton::Left, 1, Instant::now());
    assert!(app.dialogs.form.is_some(), "a click beside the dialog does not throw away changed permissions");
    assert!(ops(&mut app).is_empty());
    key(&mut app, Key::Escape);
    assert!(app.dialogs.form.is_none());
    assert!(ops(&mut app).is_empty(), "Escape cancels the change");
}
