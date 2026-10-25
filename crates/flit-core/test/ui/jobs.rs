use super::*;
use crate::input::{Event, MouseButton};
use crate::listing::Meta;
use crate::provider::{OpReport, ProviderEvent, ProviderRequest};
use crate::text::tests::Mono;
use crate::ui::{Effect, Persisted};
use std::sync::mpsc;
use std::thread::JoinHandle;

const NO: Mods = Mods { ctrl: false, shift: false, alt: false };

fn t0() -> Instant {
    Instant::now()
}

fn p(s: &str) -> FsPath {
    FsPath::parse(s).unwrap()
}

fn app_at(path: &str) -> App {
    App::new(Persisted { path: Location::parse(path).unwrap(), ..Default::default() }, Vec::new(), 0, false, 1.0, (1000, 600), t0())
}

fn started(app: &mut App) -> Option<(OpId, OpCtl)> {
    app.drain_effects().find_map(|e| match e {
        Effect::Provider { req: ProviderRequest::Op { id, ctl, .. }, .. } => Some((id, ctl)),
        _ => None,
    })
}

fn paste(app: &mut App) -> (OpId, OpCtl) {
    app.paste();
    started(app).unwrap()
}

fn click(app: &mut App, pick: impl Fn(&App) -> Rect) {
    let r = pick(app);
    let (x, y) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
    app.on_event(&Event::MouseMove { x, y }, t0());
    app.on_event(&Event::MouseDown { x, y, button: MouseButton::Left, clicks: 1 }, t0());
    app.on_event(&Event::MouseUp { x, y, button: MouseButton::Left }, t0());
}

fn key(app: &mut App, key: Key) {
    app.on_event(&Event::Key { key, mods: NO, pressed: true }, t0());
}

fn finish(app: &mut App, id: OpId, error: Option<&str>) {
    let tab = app.tab().id;
    let report = OpReport { op: FileOp::Paste { dest: p(r"C:\x") }, items: Vec::new() };
    app.on_provider_event(ProviderEvent::OpDone { tab, id, error: error.map(str::to_owned), report: Some(report) }, t0());
}

fn conflict(app: &mut App, id: OpId) {
    let tab = app.tab().id;
    let conflict = Conflict { source: p(r"C:\a\f.txt"), target: p(r"C:\x\f.txt"), from: Meta { size: 3, ..Default::default() }, to: Meta { size: 5, ..Default::default() } };
    app.on_provider_event(ProviderEvent::OpConflict { tab, id, conflict }, t0());
}

fn asked(app: &mut App, id: OpId, ctl: &OpCtl) -> JoinHandle<Option<Resolve>> {
    let (tx, rx) = mpsc::channel();
    let ctl = ctl.clone();
    let waiting = std::thread::spawn(move || ctl.resolve(|| tx.send(()).unwrap()));
    rx.recv().unwrap();
    conflict(app, id);
    waiting
}

fn states(app: &App) -> Vec<JobState> {
    app.job_states().into_iter().map(|(_, s, _)| s).collect()
}

fn unscaled(v: f32) -> f32 {
    v
}

#[test]
fn conflicts_are_answered_in_flits_own_dialog_with_the_keyboard() {
    let mut app = app_at(r"C:\x");
    let (id, ctl) = paste(&mut app);
    let waiting = asked(&mut app, id, &ctl);
    assert_eq!((app.asking(), app.jobs.sel), (Some("conflict"), 1), "keep both is the default button");
    assert!(app.job_states()[0].0.starts_with("Pasting into"));
    key(&mut app, Key::Char('R'));
    assert_eq!((waiting.join().unwrap(), app.asking()), (Some(Resolve::Replace), None));

    let waiting = asked(&mut app, id, &ctl);
    key(&mut app, Key::Char('A'));
    key(&mut app, Key::Enter);
    assert_eq!(waiting.join().unwrap(), Some(Resolve::KeepBoth));
    assert_eq!(ctl.resolve(|| panic!("asked again")), Some(Resolve::KeepBoth), "the answer applies to all remaining conflicts");

    let (id, ctl) = paste(&mut app);
    let waiting = asked(&mut app, id, &ctl);
    key(&mut app, Key::Escape);
    assert_eq!((waiting.join().unwrap(), ctl.is_cancelled()), (None, true));
    finish(&mut app, id, Some("The operation was cancelled"));
    assert_eq!(states(&app)[1], JobState::Cancelled);
}

#[test]
fn the_conflict_dialog_is_modal_and_works_with_the_mouse() {
    let mut app = app_at(r"C:\x");
    let (id, ctl) = paste(&mut app);
    let waiting = asked(&mut app, id, &ctl);
    let tabs = app.tabs().len();
    click(&mut app, |a| a.layout.new_tab);
    app.on_event(&Event::Char('x'), t0());
    assert_eq!((app.tabs().len(), app.asking()), (tabs, Some("conflict")), "the rest of the window waits for the answer");
    click(&mut app, |a| a.jobs.r.check);
    assert!(app.jobs.all);
    click(&mut app, |a| a.jobs.r.buttons[2]);
    assert_eq!(waiting.join().unwrap(), Some(Resolve::Skip));
    assert_eq!(ctl.resolve(|| panic!("asked again")), Some(Resolve::Skip));
    let mut g = Gfx::new(Mono, 2048);
    let (id, ctl) = paste(&mut app);
    let waiting = asked(&mut app, id, &ctl);
    app.open_search();
    app.frame(&mut g, t0());
    assert!(!g.dl.instances.is_empty());
    click(&mut app, |a| a.jobs.r.buttons[3]);
    assert_eq!((waiting.join().unwrap(), ctl.is_cancelled()), (None, true), "the dialog answers over the search view too");
}

#[test]
fn the_operations_panel_shows_progress_results_and_cancels() {
    let mut app = app_at(r"C:\x");
    let before = app.layout.address.w;
    assert_eq!(app.layout.ops_button.w, 0.0, "the button hides while nothing runs");
    let (id, ctl) = paste(&mut app);
    assert!(app.layout.ops_button.w > 0.0 && app.layout.address.w < before);
    click(&mut app, |a| a.layout.ops_button);
    assert!(app.jobs_open());
    let tab = app.tab().id;
    let progress = OpProgress { stage: Stage::Running, bytes: 50, total_bytes: 200, items: 1, total_items: 4 };
    app.on_provider_event(ProviderEvent::OpProgress { tab, id, progress }, t0());
    let j = &app.jobs.list[0];
    assert_eq!((j.fraction(), j.status()), (0.25, "25%".to_owned()));
    assert!(j.detail().starts_with("50 B of 200 B") && j.detail().ends_with("1 of 4 items"), "{}", j.detail());
    let mut g = Gfx::new(Mono, 2048);
    app.frame(&mut g, t0());
    click(&mut app, |a| Jobs::cancel_rect(a.jobs.r.rows[0], unscaled));
    assert!(ctl.is_cancelled() && app.jobs_open());
    assert_eq!(app.jobs.list[0].detail(), "Cancelling…");
    finish(&mut app, id, Some("The operation was cancelled"));
    let (failed, _) = paste(&mut app);
    finish(&mut app, failed, Some("Access is denied"));
    let (fine, _) = paste(&mut app);
    finish(&mut app, fine, None);
    assert_eq!(states(&app), [JobState::Cancelled, JobState::Failed("Access is denied".into()), JobState::Done]);
    assert!(!app.jobs.unseen_failure, "a failure shown in the open panel is seen");
    app.frame(&mut g, t0());

    key(&mut app, Key::Escape);
    let (late, _) = paste(&mut app);
    finish(&mut app, late, Some("boom"));
    assert!(!app.jobs_open() && app.jobs.unseen_failure && app.layout.ops_button.w > 0.0, "an unseen failure keeps the button");
    click(&mut app, |a| a.layout.ops_button);
    assert!(app.jobs_open() && !app.jobs.unseen_failure);
    click(&mut app, |a| a.jobs.r.clear);
    assert!(app.job_states().is_empty());
}

#[test]
fn the_more_menu_opens_the_destination_or_removes_the_entry() {
    let mut app = app_at(r"C:\x");
    app.run_action("ops.show", t0());
    let (id, ctl) = paste(&mut app);
    click(&mut app, |a| Jobs::more_rect(a.jobs.r.rows[0], unscaled));
    assert_eq!((app.jobs.menu, app.job_menu_items(0)), (Some(0), vec!["Open destination", "Cancel"]));
    click(&mut app, |a| a.jobs.r.menu[1]);
    assert!(ctl.is_cancelled());
    finish(&mut app, id, None);
    click(&mut app, |a| Jobs::more_rect(a.jobs.r.rows[0], unscaled));
    assert_eq!(app.job_menu_items(0), ["Open destination", "Remove from the list"]);
    let tabs = app.tabs().len();
    click(&mut app, |a| a.jobs.r.menu[0]);
    assert_eq!((app.tabs().len(), app.current_location().clone()), (tabs + 1, Location::parse(r"C:\x").unwrap()));
    app.run_action("ops.show", t0());
    click(&mut app, |a| Jobs::more_rect(a.jobs.r.rows[0], unscaled));
    click(&mut app, |a| a.jobs.r.menu[1]);
    assert!(app.job_states().is_empty());
}

#[test]
fn permanent_deletion_waits_for_the_confirmation_button() {
    let mut app = app_at(r"C:\x");
    app.empty_bin();
    assert_eq!(app.asking(), Some("confirm"));
    click(&mut app, |a| a.jobs.r.buttons[1]);
    assert!(app.asking().is_none() && started(&mut app).is_none());
    app.empty_bin();
    click(&mut app, |a| a.jobs.r.buttons[0]);
    assert!(started(&mut app).is_some());
    assert_eq!(app.job_states()[0].0, "Emptying the Recycle Bin");
}

#[test]
fn flags_keep_the_button_and_answer_conflicts_by_default() {
    let mut app = app_at(r"C:\x");
    app.ops_button_always = true;
    app.layout();
    assert!(app.layout.ops_button.w > 0.0);
    app.conflict_default = Some(Resolve::Replace);
    let (_, ctl) = paste(&mut app);
    assert_eq!(ctl.resolve(|| panic!("asked despite a default action")), Some(Resolve::Replace));
}

#[test]
fn closing_the_window_cancels_its_operations() {
    let mut app = app_at(r"C:\x");
    let (_, ctl) = paste(&mut app);
    app.shutdown();
    assert!(ctl.is_cancelled());
}

#[test]
fn titles_name_the_operation_and_its_items() {
    let items = vec![p(r"C:\a\one.txt"), p(r"C:\a\two.txt")];
    assert_eq!(job_title(&FileOp::Copy { items: items[..1].to_vec(), dest: p(r"D:\b") }).unwrap(), "Copying \u{201C}one.txt\u{201D} to \u{201C}b\u{201D}");
    assert_eq!(job_title(&FileOp::Trash(items.clone())).unwrap(), "Moving 2 items to the Recycle Bin");
    assert_eq!(job_title(&FileOp::MoveTo(vec![(items[0].clone(), p(r"C:\a\x.txt")), (items[1].clone(), p(r"C:\a\y.txt"))])).unwrap(), "Renaming 2 items");
    assert_eq!(job_title(&FileOp::Flatten(vec![p(r"C:\a")])).unwrap(), "Flattening \u{201C}a\u{201D}");
    assert!(job_title(&FileOp::Rename { item: items[0].clone(), name: b"z".to_vec() }).is_none());
    assert_eq!(job_dest(&FileOp::Trash(items)), Some(Location::recycle_bin()));
}
