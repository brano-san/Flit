use super::*;
use crate::input::{Event, Key, Mods};
use crate::listing::{DirSnapshot, Listing, Meta};
use crate::location::{Location, ProviderId};
use crate::provider::{AccessChange, OpReport, ProviderEvent, ProviderRequest};
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
    let id = app.drain_effects().find_map(|e| if let Effect::Provider { req: ProviderRequest::Access { id, items, .. }, .. } = e { assert_eq!(items, vec![p("/home/u/tool")]); Some(id) } else { None }).expect("the dialog asks for the current access");
    let accounts = Accounts { users: vec![(1000, "u".into()), (0, "root".into())], groups: vec![(1000, "u".into()), (27, "sudo".into())] };
    let tab = app.tab().id;
    app.on_provider_event(ProviderEvent::Access { tab, id, items: vec![Some(Access { mode: Access::FILE | 0o644, uid: 1000, gid: 1000 })], accounts }, Instant::now());
    let f = app.jobs.form.as_mut().unwrap();
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
    let f = app.jobs.form.as_ref().unwrap();
    assert_eq!(f.flags(at + 3), vec![true, false, true], "others can read, so they may run it too");
    key(&mut app, Key::Enter);
    assert!(app.jobs.form.is_none());
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
    let f = app.jobs.form.as_mut().unwrap();
    let Kind::Properties(Measuring { probe: Probe::Editing(e), .. }) = &f.kind else { panic!() };
    let owner = e.owner_at();
    f.set_text(owner, "nobody-here");
    key(&mut app, Key::Enter);
    assert!(app.jobs.form.as_ref().is_some_and(|f| f.error));
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
    let report = OpReport { op: FileOp::Access(vec![change]), items: vec![(path.clone(), path.clone())] };
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
