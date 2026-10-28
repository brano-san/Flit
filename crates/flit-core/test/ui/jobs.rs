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
    let close = app.panels.caption[2];
    assert_eq!(app.hit_test(close.x + 4.0, close.y + 4.0), crate::input::HitArea::Close, "the window can still be closed or moved");
    assert_eq!(app.hit_test(app.layout.new_tab.right() + 40.0, 10.0), crate::input::HitArea::Caption);
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
    assert!(j.detail(false).starts_with("50 B of 200 B") && j.detail(false).ends_with("1 of 4 items"), "{}", j.detail(false));
    let mut g = Gfx::new(Mono, 2048);
    app.frame(&mut g, t0());
    click(&mut app, |a| Jobs::cancel_rect(a.jobs.r.rows[0], unscaled));
    assert!(ctl.is_cancelled() && app.jobs_open());
    assert_eq!(app.jobs.list[0].detail(false), "Cancelling…");
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
    assert_eq!(job_dest(&ProviderId::Local, &FileOp::Trash(items)), Some(Location::recycle_bin()));
    let wsl = ProviderId::Wsl("Ubuntu".into());
    assert_eq!(job_dest(&wsl, &FileOp::Trash(vec![p("/home/a")])), None);
    assert_eq!(job_dest(&wsl, &FileOp::Copy { items: vec![p("/home/a")], dest: p("/tmp") }), Some(Location::parse("wsl://Ubuntu/tmp").unwrap()));
}

#[test]
fn a_dialog_queued_behind_a_conflict_keeps_the_conflicts_default_button() {
    let mut app = app_at(r"C:\x");
    let (id, ctl) = paste(&mut app);
    let waiting = asked(&mut app, id, &ctl);
    app.ask_confirm(FileOp::Trash(vec![p(r"C:\x\a.txt")]));
    app.offer_restore();
    assert_eq!((app.asking(), app.jobs.sel), (Some("conflict"), 1));
    key(&mut app, Key::Enter);
    assert_eq!(waiting.join().unwrap(), Some(Resolve::KeepBoth));
    assert_eq!((app.asking(), app.jobs.sel), (Some("confirm"), 0));
}

#[test]
fn a_finished_job_drops_its_conflict_and_resets_the_default_button() {
    let mut app = app_at(r"C:\x");
    let (id, ctl) = paste(&mut app);
    let waiting = asked(&mut app, id, &ctl);
    app.offer_restore();
    key(&mut app, Key::Right);
    key(&mut app, Key::Right);
    ctl.cancel();
    finish(&mut app, id, Some("cancelled"));
    assert_eq!((app.asking(), app.jobs.sel), (Some("restore"), 0));
    drop(waiting);
}

#[test]
fn items_too_large_for_the_drive_are_skipped_or_cancel_without_setting_a_policy() {
    let mut app = app_at(r"C:\x");
    let ask = |app: &mut App, ctl: &OpCtl, id: OpId, conflict: Conflict| {
        let (tx, rx) = mpsc::channel();
        let c = ctl.clone();
        let waiting = std::thread::spawn(move || c.resolve(|| tx.send(()).unwrap()));
        rx.recv().unwrap();
        app.on_provider_event(ProviderEvent::OpConflict { tab: app.tab().id, id, conflict }, t0());
        waiting
    };
    let (id, ctl) = paste(&mut app);
    let waiting = ask(&mut app, &ctl, id, crate::limits::over_limit(&p(r"C:\a\big.iso"), &p(r"E:\big.iso"), 5 << 30, crate::limits::FAT_MAX_FILE));
    assert_eq!((app.asking(), app.jobs.sel, app.dialog_buttons()), (Some("too_large"), 0, vec!["Skip", "Cancel"]));
    key(&mut app, Key::Char('A'));
    key(&mut app, Key::Enter);
    assert_eq!((waiting.join().unwrap(), ctl.policy(), app.asking()), (Some(Resolve::Skip), None, None));

    let waiting = ask(&mut app, &ctl, id, crate::limits::short_of_space(&p(r"E:\"), 10, 3));
    assert_eq!((app.asking(), app.dialog_buttons()), (Some("too_large"), vec!["Copy anyway", "Cancel"]));
    key(&mut app, Key::Escape);
    assert_eq!((waiting.join().unwrap(), ctl.is_cancelled()), (None, true));
}

#[test]
fn a_crash_report_offers_its_folder_and_then_the_session() {
    let mut app = app_at(r"C:\x");
    app.offer_crash_report(true, p(r"C:\logs\flit-crash-1.dmp"));
    assert_eq!(app.dialog_buttons(), ["Restore", "Open folder", "Not now"]);
    let (title, lines) = app.dialog_text().unwrap();
    assert_eq!(title, "Restore the previous session?");
    assert!(lines[0].contains("flit-crash-1.dmp") && lines.len() == 2, "{lines:?}");
    app.answer(1);
    assert_eq!(app.tab().view_loc, Location::parse(r"C:\logs").unwrap());
    assert_eq!((app.asking(), app.dialog_buttons()), (Some("restore"), vec!["Restore", "Not now"]));
    app.answer(0);
    assert!(app.drain_effects().any(|e| matches!(e, Effect::RestoreSession)));
}

#[test]
fn a_crash_report_without_a_session_only_opens_the_folder() {
    let mut app = app_at(r"C:\x");
    app.offer_crash_report(false, p(r"C:\logs\flit-crash-1.dmp"));
    assert_eq!(app.dialog_buttons(), ["Open folder", "Not now"]);
    assert_eq!(app.dialog_text().unwrap().0, "Flit crashed last time");
    app.answer(1);
    assert_eq!(app.asking(), None);
    assert!(!app.drain_effects().any(|e| matches!(e, Effect::RestoreSession)));
}

#[test]
fn operations_longer_than_half_a_second_show_their_duration_in_the_status_block() {
    let mut app = app_at(r"C:\x");
    let (quick, _) = paste(&mut app);
    app.job_done(quick, None, Instant::now());
    assert_eq!(app.tab().stats.op, None);
    let (slow, _) = paste(&mut app);
    app.job_done(slow, None, Instant::now() + Duration::from_millis(1500));
    assert!(app.tab().stats.op.is_some_and(|d| d >= Duration::from_millis(1400)));
    assert_eq!(app.bottom_tip(crate::ui::bottom::BottomHit::Elapsed), S::TooltipElapsedOp);
}
