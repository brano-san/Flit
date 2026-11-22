use super::*;
use crate::input::{Event, MouseButton};
use crate::listing::Meta;
use crate::provider::{OpProgress, OpReport, ProviderEvent, ProviderRequest};
use crate::text::tests::Mono;
use crate::ui::{Effect, Persisted, WindowCmd};
use std::sync::mpsc;
use std::thread::JoinHandle;

const NO: Mods = Mods { ctrl: false, shift: false, alt: false };
const SHIFT: Mods = Mods { ctrl: false, shift: true, alt: false };

fn t0() -> Instant {
    Instant::now()
}

fn p(s: &str) -> FsPath {
    FsPath::parse(s).unwrap()
}

fn app_at(path: &str) -> App {
    App::new(Persisted { path: Location::parse(path).unwrap(), ..Default::default() }, Vec::new(), 0, false, 1.0, (1000, 600), t0())
}

#[test]
fn symbolic_link_privilege_retry_is_explicit_and_its_translations_fit_the_first_button() {
    let original = crate::i18n::lang();
    for lang in crate::i18n::Lang::ALL {
        crate::i18n::set_lang(lang);
        let mut app = app_at(r"C:\x");
        app.on_event(&Event::Resized { width: 400, height: 400 }, t0());
        let (id, ctl) = paste(&mut app);
        let tab = app.tab().id;
        app.on_provider_event(ProviderEvent::OpFailure { tab, id, failure: Failure { path: p(r"C:\source.txt"), error: S::ErrorsEngineSymlinkPrivilege.en().into(), fatal: false } }, t0());
        let mut gfx = Gfx::new(Mono, 1024);
        app.frame(&mut gfx, t0());
        assert_eq!(app.dialog_buttons()[0], S::MenuLinksRetryAdmin.t());
        let (dialog, buttons) = app.dialog_rects();
        assert!(buttons[0].w > buttons[1].w && buttons[0].y < buttons[1].y);
        assert!(buttons.iter().all(|b| b.x >= dialog.x && b.right() <= dialog.right() && b.bottom() <= 400.0));
        let font = FontStyle::new(FontFamily::Ui, 13.0, false);
        let rows = crate::ui::settings_page::wrap(&mut gfx, S::MenuLinksRetryAdmin.t(), font, buttons[0].w - 16.0);
        assert!(rows.len() <= 2 && rows.iter().all(|line| gfx.measure(line, font) <= buttons[0].w - 16.0));
        let answered = ctl.clone();
        let (ready, waiting) = mpsc::channel();
        let waiter = std::thread::spawn(move || answered.recover(false, || { ready.send(()).unwrap(); }));
        waiting.recv_timeout(Duration::from_secs(1)).unwrap();
        key(&mut app, Key::Home);
        key(&mut app, Key::Enter);
        assert_eq!(waiter.join().unwrap(), Some(Recover::Retry));
    }
    crate::i18n::set_lang(original);
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

fn shift(app: &mut App, key: Key) {
    app.on_event(&Event::Key { key, mods: SHIFT, pressed: true }, t0());
}

fn letter(app: &mut App, vk: char, typed: char, mods: Mods) {
    app.on_event(&Event::Key { key: Key::Char(vk), mods, pressed: true }, t0());
    app.on_event(&Event::Char(typed), t0());
}

fn finish(app: &mut App, id: OpId, error: Option<&str>) {
    let tab = app.tab().id;
    let report = OpReport { op: FileOp::Paste { dest: p(r"C:\x") }, items: Vec::new() };
    app.on_provider_event(ProviderEvent::OpDone { tab, id, error: error.map(str::to_owned), report: Some(report) }, t0());
}

fn sizes(from: u64, to: u64) -> Conflict {
    Conflict { source: p(r"C:\a\f.txt"), target: p(r"C:\x\f.txt"), from: Meta { size: from, ..Default::default() }, to: Meta { size: to, ..Default::default() } }
}

fn conflict(app: &mut App, id: OpId) {
    let tab = app.tab().id;
    app.on_provider_event(ProviderEvent::OpConflict { tab, id, conflict: sizes(3, 5) }, t0());
}

fn asked_about(app: &mut App, id: OpId, ctl: &OpCtl, conflict: Conflict) -> (JoinHandle<Option<Resolve>>, mpsc::Receiver<()>) {
    let (tx, rx) = mpsc::channel();
    let (ctl, c) = (ctl.clone(), conflict.clone());
    let waiting = std::thread::spawn(move || ctl.resolve(&c, || tx.send(()).unwrap()));
    rx.recv().unwrap();
    app.on_provider_event(ProviderEvent::OpConflict { tab: app.tab().id, id, conflict }, t0());
    (waiting, rx)
}

fn asked(app: &mut App, id: OpId, ctl: &OpCtl) -> JoinHandle<Option<Resolve>> {
    asked_about(app, id, ctl, sizes(3, 5)).0
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
    letter(&mut app, 'R', 'r', NO);
    assert_eq!((waiting.join().unwrap(), app.asking()), (Some(Resolve::Replace), None));

    let waiting = asked(&mut app, id, &ctl);
    letter(&mut app, 'D', 'D', SHIFT);
    key(&mut app, Key::Enter);
    assert_eq!(waiting.join().unwrap(), Some(Resolve::KeepBoth));
    assert_eq!(ctl.resolve(&sizes(1, 2), || panic!("asked again")), Some(Resolve::KeepBoth), "the answer applies to all remaining conflicts");

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
    assert_eq!(ctl.resolve(&sizes(1, 2), || panic!("asked again")), Some(Resolve::Skip));
    let mut g = Gfx::new(Mono, 2048);
    let (id, ctl) = paste(&mut app);
    let waiting = asked(&mut app, id, &ctl);
    app.open_search();
    app.frame(&mut g, t0());
    assert!(!g.dl.instances.is_empty());
    click(&mut app, |a| *a.jobs.r.buttons.last().unwrap());
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
    assert_eq!((app.jobs.menu, app.job_menu_items(0)), (Some(0), vec!["Open destination", "Pause", "Cancel"]));
    click(&mut app, |a| a.jobs.r.menu[2]);
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
fn fine_touchpad_deltas_over_the_operations_panel_add_up_to_whole_rows() {
    let mut app = app_at(r"C:\x");
    app.run_action("ops.show", t0());
    for _ in 0..8 {
        paste(&mut app);
    }
    app.frame(&mut Gfx::new(Mono, 2048), t0());
    let r = app.jobs.r.panel;
    app.on_event(&Event::MouseMove { x: r.x + r.w / 2.0, y: r.y + r.h / 2.0 }, t0());
    let (t, ms) = (t0(), Duration::from_millis);
    let wheel = |app: &mut App, dy: f32, at: u64| {
        app.on_event(&Event::Wheel { dx: 0.0, dy }, t + ms(at));
        app.jobs.first
    };
    assert_eq!([wheel(&mut app, -0.25, 0), wheel(&mut app, -0.25, 10), wheel(&mut app, -0.25, 20)], [0, 0, 0], "a quarter notch moves nothing yet");
    assert_eq!(wheel(&mut app, -0.25, 30), 1, "four quarters make one row");
    assert_eq!(wheel(&mut app, -1.0, 40), 2, "a mouse notch still moves one row");
    assert_eq!(wheel(&mut app, -2.0, 50), 3, "the list stops at its end");
    assert_eq!(wheel(&mut app, 0.5, 60), 3);
    assert_eq!(wheel(&mut app, 0.5, 70), 2, "two half notches back make one row back");
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
    assert_eq!(ctl.resolve(&sizes(1, 2), || panic!("asked despite a default action")), Some(Resolve::Replace));
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
        let (c, asked) = (ctl.clone(), conflict.clone());
        let waiting = std::thread::spawn(move || c.resolve(&asked, || tx.send(()).unwrap()));
        rx.recv().unwrap();
        app.on_provider_event(ProviderEvent::OpConflict { tab: app.tab().id, id, conflict }, t0());
        waiting
    };
    let (id, ctl) = paste(&mut app);
    let waiting = ask(&mut app, &ctl, id, crate::limits::over_limit(&p(r"C:\a\big.iso"), &p(r"E:\big.iso"), 5 << 30, crate::limits::FAT_MAX_FILE));
    assert_eq!((app.asking(), app.jobs.sel, app.dialog_buttons()), (Some("too_large"), 0, vec!["Skip", "Cancel"]));
    letter(&mut app, 'A', 'a', NO);
    key(&mut app, Key::Enter);
    assert_eq!((waiting.join().unwrap(), ctl.policy(), app.asking()), (Some(Resolve::Skip), None, None));

    let waiting = ask(&mut app, &ctl, id, crate::limits::short_of_space(&p(r"E:\"), 10, 3));
    assert_eq!((app.asking(), app.jobs.sel, app.dialog_buttons()), (Some("too_large"), 0, vec!["Copy anyway", "Retry", "Cancel"]));
    click(&mut app, |a| a.jobs.r.buttons[1]);
    assert_eq!((waiting.join().unwrap(), ctl.policy()), (Some(crate::limits::ROOM_RETRY), None), "retry measures the free space again and sets no policy");
    let waiting = ask(&mut app, &ctl, id, crate::limits::short_of_space(&p(r"E:\"), 10, 3));
    key(&mut app, Key::Enter);
    assert_eq!((waiting.join().unwrap(), ctl.policy()), (Some(crate::limits::ROOM_ANYWAY), None));
    let waiting = ask(&mut app, &ctl, id, crate::limits::short_of_space(&p(r"E:\"), 10, 3));
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
    app.job_done(quick, None, false, Instant::now());
    assert_eq!(app.tab().stats.op, None);
    let (slow, _) = paste(&mut app);
    app.job_done(slow, None, false, Instant::now() + Duration::from_millis(1500));
    assert!(app.tab().stats.op.is_some_and(|d| d >= Duration::from_millis(1400)));
    assert_eq!(app.bottom_tip(crate::ui::bottom::BottomHit::Elapsed), S::TooltipElapsedOp);
}

#[test]
fn user_cancelled_operations_are_shown_as_cancelled_without_a_code() {
    for e in ["(0x80270000)", "Cancelled by the user (0x800704C7)", "(0x80004004)", "The operation was cancelled (0xFFFFFFFF)", "Deletion was cancelled"] {
        assert!(cancelled_error(e), "{e}");
    }
    assert!(!cancelled_error("Access is denied (0x80070005)"));
    let mut app = app_at(r"C:\x");
    let (id, _) = paste(&mut app);
    finish(&mut app, id, Some("(0x80270000)"));
    let (other, _) = paste(&mut app);
    finish(&mut app, other, Some("Access is denied (0x80070005)"));
    assert_eq!(states(&app), [JobState::Cancelled, JobState::Failed("Access is denied (0x80070005)".into())]);
    assert_eq!(app.jobs.list[0].status(), "Cancelled");
    assert_eq!(app.tab().error.as_deref(), Some("Access is denied (0x80070005)"), "only the real failure reaches the status bar");
}

#[test]
fn the_jobs_panel_slides_open_and_fades_out_when_closed() {
    let mut app = app_at(r"C:\x");
    let (_id, _ctl) = paste(&mut app);
    click(&mut app, |a| a.layout.ops_button);
    assert!(app.jobs_open());
    let mut g = crate::text::Gfx::new(crate::text::tests::Mono, 1024);
    let t = Instant::now();
    assert!(app.frame(&mut g, t), "it slides in");
    app.frame(&mut g, t + Duration::from_millis(400));
    click(&mut app, |a| a.layout.ops_button);
    assert!(!app.jobs_open());
    app.frame(&mut g, t + Duration::from_millis(410));
    assert_eq!(app.ghosts.fading_count(), 1, "and fades out");
    app.frame(&mut g, t + Duration::from_millis(700));
    assert!(app.ghosts.fading_count() == 0);
    app.animations = false;
    click(&mut app, |a| a.layout.ops_button);
    app.frame(&mut g, t + Duration::from_millis(710));
    click(&mut app, |a| a.layout.ops_button);
    app.frame(&mut g, t + Duration::from_millis(720));
    assert!(app.ghosts.fading_count() == 0, "without Animations nothing lingers");
}

#[test]
fn the_operations_button_shows_a_warning_after_a_failure_and_the_list_marks_the_failed_row() {
    let mut app = app_at(r"C:\x");
    let mut g = Gfx::new(Mono, 1024);
    let (id, _) = paste(&mut app);
    assert_eq!(app.jobs.outcome(), Outcome::None);
    finish(&mut app, id, Some("Access is denied"));
    assert_eq!(app.jobs.outcome(), Outcome::Failed);
    app.frame(&mut g, t0());
    let error = app.look.palette.base.error.0;
    let b = app.layout.ops_button;
    let red_in = |g: &Gfx<Mono>, r: Rect| g.dl.instances.iter().filter(|i| i.color == error && r.contains(i.rect[0] + i.rect[2] / 2.0, i.rect[1] + i.rect[3] / 2.0)).count();
    assert!(red_in(&g, b) >= 2, "a red warning glyph and the unseen badge");
    click(&mut app, |a| a.layout.ops_button);
    for s in 1..3 {
        app.frame(&mut g, t0() + Duration::from_secs(s));
    }
    let row = app.jobs.r.rows[0];
    assert!(red_in(&g, row) >= 3, "the failed row has a red stripe, a warning glyph and a red status");
    let (id, _) = paste(&mut app);
    finish(&mut app, id, None);
    assert_eq!(app.jobs.outcome(), Outcome::Done, "the latest outcome decides the icon");
}

fn failed(app: &mut App, id: OpId, ctl: &OpCtl, fatal: bool) -> JoinHandle<Option<Recover>> {
    let (tx, rx) = mpsc::channel();
    let c = ctl.clone();
    let waiting = std::thread::spawn(move || c.recover(fatal, || tx.send(()).unwrap()));
    rx.recv().unwrap();
    let failure = Failure { path: p(r"C:\a\locked.txt"), error: "The process cannot access the file because it is being used by another process, so Flit waits for your answer.".into(), fatal };
    app.on_provider_event(ProviderEvent::OpFailure { tab: app.tab().id, id, failure }, t0());
    waiting
}

#[test]
fn a_failing_item_asks_to_retry_skip_skip_all_or_cancel() {
    let mut app = app_at(r"C:\x");
    let mut g = Gfx::new(Mono, 2048);
    let (id, ctl) = paste(&mut app);
    let waiting = failed(&mut app, id, &ctl, false);
    assert_eq!((app.asking(), app.jobs.sel, app.dialog_buttons()), (Some("failure"), 0, vec!["Retry", "Skip", "Skip all", "Cancel"]));
    let (title, lines) = app.dialog_text().unwrap();
    assert_eq!(title, "Cannot copy \u{201C}locked.txt\u{201D}");
    assert!(lines[0].starts_with("The process cannot access") && lines[1] == r"In C:\a", "{lines:?}");
    assert_eq!(app.jobs.list[0].status(), "Problem");
    app.frame(&mut g, t0());
    key(&mut app, Key::Enter);
    assert_eq!((waiting.join().unwrap(), app.asking()), (Some(Recover::Retry), None), "Retry is the default button");
    assert!(!app.jobs.list[0].asking && app.jobs.list[0].status() != "Problem");

    let waiting = failed(&mut app, id, &ctl, false);
    click(&mut app, |a| a.jobs.r.buttons[1]);
    assert_eq!((waiting.join().unwrap(), ctl.skips_failures()), (Some(Recover::Skip), false));
    let waiting = failed(&mut app, id, &ctl, false);
    click(&mut app, |a| a.jobs.r.buttons[2]);
    assert_eq!((waiting.join().unwrap(), ctl.skips_failures()), (Some(Recover::Skip), true));
    assert_eq!(ctl.recover(false, || panic!("asked after Skip all")), Some(Recover::Skip));

    let waiting = failed(&mut app, id, &ctl, true);
    assert_eq!((app.asking(), app.dialog_buttons()), (Some("full"), vec!["Retry", "Cancel"]), "a full disk is asked even after Skip all");
    assert_eq!(app.dialog_text().unwrap().0, "The destination is full");
    app.frame(&mut g, t0());
    key(&mut app, Key::Escape);
    assert_eq!((waiting.join().unwrap(), ctl.is_cancelled()), (None, true));
    finish(&mut app, id, Some("The operation was cancelled"));
    assert_eq!(states(&app), [JobState::Cancelled]);
}

#[test]
fn a_finished_job_drops_its_failure_question() {
    let mut app = app_at(r"C:\x");
    let (id, ctl) = paste(&mut app);
    let waiting = failed(&mut app, id, &ctl, false);
    ctl.cancel();
    finish(&mut app, id, Some("cancelled"));
    assert_eq!((waiting.join().unwrap(), app.asking()), (None, None));
}

fn closes(app: &mut App) -> bool {
    app.drain_effects().any(|e| matches!(e, Effect::Window(crate::ui::WindowCmd::Close)))
}

#[test]
fn closing_a_window_with_running_operations_asks_and_cancels_them_only_when_told() {
    let mut app = app_at(r"C:\x");
    app.on_event(&Event::CloseRequested, t0());
    assert!(closes(&mut app) && app.confirm.is_none(), "nothing runs, so the window closes at once");
    let (first, a) = paste(&mut app);
    let (_, b) = paste(&mut app);
    app.on_event(&Event::CloseRequested, t0());
    let c = app.confirm.clone().expect("closing asks first");
    assert_eq!((c.question.as_str(), c.yes.as_str()), ("2 operations are still running", "Cancel and close"));
    assert!(!closes(&mut app) && !a.is_cancelled());
    click(&mut app, |a| a.confirm.as_ref().unwrap().buttons[1]);
    let commands: Vec<_> = app.drain_effects().filter_map(|e| if let Effect::Window(cmd) = e { Some(cmd) } else { None }).collect();
    assert_eq!(commands, [WindowCmd::KeepOpen]);
    assert!(app.confirm.is_none() && !a.is_cancelled() && !b.is_cancelled(), "Keep window open leaves everything running");

    finish(&mut app, first, None);
    app.run_action("window.close", t0());
    assert_eq!(app.confirm.as_ref().map(|c| c.question.as_str()), Some("1 operation is still running"), "finished operations do not count");
    click(&mut app, |a| a.confirm.as_ref().unwrap().buttons[0]);
    assert!(b.is_cancelled() && closes(&mut app), "Cancel and close cancels what runs and closes");
}

#[test]
fn running_jobs_keep_the_system_awake() {
    let mut app = app_at(r"C:\x");
    assert!(!app.keeps_awake());
    let (id, _) = paste(&mut app);
    assert!(app.keeps_awake() && app.running_jobs() == 1);
    finish(&mut app, id, Some("Access is denied"));
    assert!(!app.keeps_awake() && app.running_jobs() == 0);
}

#[test]
fn items_the_trash_refuses_can_be_deleted_permanently_instead() {
    let mut app = app_at("/media/usb");
    app.caps = crate::caps::Caps { unix_paths: true };
    app.drain_effects().for_each(drop);
    let all = vec![p("/media/usb/a"), p("/media/usb/b"), p("/media/usb/c")];
    app.start_confirmed(ProviderId::Local, FileOp::Trash(all.clone()));
    let (id, _) = started(&mut app).unwrap();
    let tab = app.tab().id;
    let refused = vec![p("/media/usb/b"), p("/media/usb/c")];
    app.on_provider_event(ProviderEvent::Untrashable { tab, id, items: refused.clone() }, t0());
    let report = OpReport { op: FileOp::Trash(all), items: vec![(p("/media/usb/a"), p("/home/u/.local/share/Trash/files/a"))] };
    app.on_provider_event(ProviderEvent::OpDone { tab, id, error: Some("2 items failed. Cannot move /media/usb/b to the Trash: Permission denied".into()), report: Some(report) }, t0());
    assert_eq!(app.asking(), Some("untrashable"));
    let (title, lines) = app.dialog_text().unwrap();
    assert_eq!((title.as_str(), lines[0].as_str(), lines[1].as_str()), ("Delete permanently?", "2 items can't be moved to the Trash.", "This cannot be undone."));
    assert_eq!(app.dialog_buttons(), ["Delete permanently", "Cancel"]);
    assert_eq!(app.jobs.sel, 0);
    key(&mut app, Key::Enter);
    let deletes: Vec<FileOp> = app.drain_effects().filter_map(|e| match e {
        crate::ui::Effect::Provider { provider: ProviderId::Local, req: ProviderRequest::Op { op, .. } } => Some(op),
        _ => None,
    }).collect();
    assert_eq!(deletes, [FileOp::Delete(refused)], "exactly the refused items are deleted");
    assert_eq!(app.asking(), None);

    app.start_confirmed(ProviderId::Local, FileOp::Trash(vec![p("/media/usb/d")]));
    let (id, ctl) = started(&mut app).unwrap();
    app.on_provider_event(ProviderEvent::Untrashable { tab, id, items: vec![p("/media/usb/d")] }, t0());
    assert_eq!(app.dialog_text().unwrap().1[0], "“d” can't be moved to the Trash.");
    key(&mut app, Key::Escape);
    assert!(started(&mut app).is_none() && app.asking().is_none(), "cancel keeps the item");
    ctl.cancel();
    app.on_provider_event(ProviderEvent::Untrashable { tab, id, items: vec![p("/media/usb/d")] }, t0());
    assert_eq!(app.asking(), None, "a cancelled operation does not ask");
}

#[test]
fn an_unfocused_window_flashes_once_when_a_question_arrives() {
    let mut app = app_at(r"C:\x");
    app.drain_effects().for_each(drop);
    app.on_event(&Event::Focus(false), t0());
    app.ask_confirm(FileOp::Trash(vec![p(r"C:\x\a.txt")]));
    assert_eq!(app.drain_effects().filter(|e| matches!(e, Effect::Window(WindowCmd::Attention))).count(), 1);
    app.ask_extension_change("txt".into());
    assert_eq!(app.drain_effects().filter(|e| matches!(e, Effect::Window(WindowCmd::Attention))).count(), 0, "a follow-up question does not flash again");
    app.drain_effects().for_each(drop);
    app.on_event(&Event::Focus(true), t0());
    app.empty_bin();
    assert!(!app.drain_effects().any(|e| matches!(e, Effect::Window(WindowCmd::Attention))), "a focused window does not flash");
}

#[test]
fn the_taskbar_progress_averages_running_jobs_and_pauses_for_a_question() {
    let mut app = app_at(r"C:\x");
    assert_eq!(app.overall_progress(), None);
    let (id, _) = paste(&mut app);
    assert_eq!(app.overall_progress(), Some((0.0, false)));
    app.job_progress(id, OpProgress { bytes: 50, total_bytes: 100, ..Default::default() }, t0());
    assert_eq!(app.overall_progress().map(|(f, _)| (f * 100.0) as u32), Some(50));
    app.ask_confirm(FileOp::Trash(vec![p(r"C:\x\a.txt")]));
    assert_eq!(app.overall_progress().map(|(_, waiting)| waiting), Some(true));
    finish(&mut app, id, None);
    assert_eq!(app.overall_progress(), None);
}

#[test]
fn a_long_operation_notifies_an_unfocused_window_when_it_finishes() {
    let mut app = app_at(r"C:\x");
    app.drain_effects().for_each(drop);
    let (id, _) = paste(&mut app);
    app.on_event(&Event::Focus(false), t0());
    app.drain_effects().for_each(drop);
    app.op_done(1, id, None, None, t0() + Duration::from_secs(11));
    let notes: Vec<String> = app.drain_effects().filter_map(|e| if let Effect::Notify(text) = e { Some(text) } else { None }).collect();
    assert_eq!(notes.len(), 1);
    let (id, _) = paste(&mut app);
    app.drain_effects().for_each(drop);
    app.op_done(1, id, Some("disk full".into()), None, t0() + Duration::from_secs(12));
    assert!(app.drain_effects().any(|e| matches!(e, Effect::Notify(_))), "an error is announced too");
    let (id, _) = paste(&mut app);
    app.drain_effects().for_each(drop);
    app.on_event(&Event::Focus(true), t0());
    app.op_done(1, id, None, None, t0() + Duration::from_secs(31));
    assert!(!app.drain_effects().any(|e| matches!(e, Effect::Notify(_))), "a focused window says nothing");
    let (id, _) = paste(&mut app);
    app.drain_effects().for_each(drop);
    app.on_event(&Event::Focus(false), t0());
    app.op_done(1, id, None, None, t0() + Duration::from_secs(3));
    assert!(!app.drain_effects().any(|e| matches!(e, Effect::Notify(_))), "a short operation says nothing");
}

#[test]
fn dialog_buttons_answer_by_localized_first_letters_and_shift_tab_walks_back() {
    let mut app = app_at(r"C:\x");
    let (id, ctl) = paste(&mut app);
    let waiting = asked(&mut app, id, &ctl);
    assert_eq!(app.dialog_buttons(), ["Replace", "Keep both", "Skip", "Skip identical", "Replace older", "Cancel"]);
    key(&mut app, Key::Tab);
    assert_eq!(app.jobs.sel, 2, "Tab advances");
    shift(&mut app, Key::Tab);
    assert_eq!(app.jobs.sel, 1, "Shift+Tab walks back");
    letter(&mut app, 'S', 's', NO);
    assert_eq!(waiting.join().unwrap(), Some(Resolve::Skip), "the first letter of the localized button answers, in either case");
    crate::i18n::set_lang(crate::i18n::Lang::Ru);
    let (id, ctl) = paste(&mut app);
    let waiting = asked(&mut app, id, &ctl);
    let letters: Vec<Option<char>> = app.dialog_buttons().iter().map(|l| first_letter(l)).collect();
    assert!(letters.iter().all(|l| l.is_some()) && letters[0] != letters[1], "{letters:?}");
    let ru = letters[1].unwrap().to_lowercase().next().unwrap();
    assert!(!ru.is_ascii(), "the Russian label starts with a Cyrillic letter");
    letter(&mut app, 'S', 'ы', NO);
    assert_eq!(app.asking(), Some("conflict"), "the Latin key code alone answers nothing on a Russian layout");
    letter(&mut app, 'J', ru, NO);
    assert_eq!(waiting.join().unwrap(), Some(Resolve::KeepBoth), "the Russian answer works by the letter the layout types");
    crate::i18n::set_lang(crate::i18n::Lang::En);
}

fn button(app: &App, label: &str) -> usize {
    app.dialog_buttons().iter().position(|b| *b == label).unwrap_or_else(|| panic!("no {label} in {:?}", app.dialog_buttons()))
}

#[test]
fn replace_older_answers_this_conflict_by_date_and_decides_the_rest_without_asking() {
    let mut app = app_at(r"C:\x");
    let (id, ctl) = paste(&mut app);
    let waiting = asked(&mut app, id, &ctl);
    let k = button(&app, "Replace older");
    click(&mut app, |a| a.jobs.r.buttons[k]);
    assert_eq!((waiting.join().unwrap(), ctl.policy(), app.asking()), (Some(Resolve::Skip), Some(Resolve::ReplaceOlder), None), "the new file is not newer, so it is skipped");
    let mut newer = sizes(3, 5);
    newer.from.modified = 600_000_000;
    assert_eq!(ctl.resolve(&newer, || panic!("asked again")), Some(Resolve::Replace));
    assert_eq!(ctl.resolve(&sizes(3, 5), || panic!("asked again")), Some(Resolve::Skip));
}

#[test]
fn skip_identical_skips_same_files_from_now_on_and_still_asks_about_a_different_one() {
    let mut app = app_at(r"C:\x");
    let (id, ctl) = paste(&mut app);
    let (waiting, again) = asked_about(&mut app, id, &ctl, sizes(3, 5));
    let k = button(&app, "Skip identical");
    click(&mut app, |a| a.jobs.r.buttons[k]);
    again.recv().unwrap();
    conflict(&mut app, id);
    assert_eq!(app.dialog_buttons(), ["Replace", "Keep both", "Skip", "Replace older", "Cancel"], "a different file is asked about again, without the choice already made");
    assert_eq!(app.jobs.sel, 1);
    key(&mut app, Key::Enter);
    assert_eq!(waiting.join().unwrap(), Some(Resolve::KeepBoth));
    assert_eq!(ctl.resolve(&sizes(4, 4), || panic!("asked about an identical file")), Some(Resolve::Skip));
    let (waiting, _) = asked_about(&mut app, id, &ctl, sizes(4, 9));
    letter(&mut app, 'S', 's', NO);
    assert_eq!((waiting.join().unwrap(), ctl.policy()), (Some(Resolve::Skip), Some(Resolve::SkipSame)), "S still means Skip for this one file");
}

#[test]
fn the_conflict_dialog_says_when_both_files_look_the_same() {
    let mut app = app_at(r"C:\x");
    let (id, ctl) = paste(&mut app);
    let same = "Both have the same size and modification date.";
    let waiting = asked_about(&mut app, id, &ctl, sizes(4, 4)).0;
    assert!(app.dialog_text().unwrap().1.iter().any(|l| l == same));
    key(&mut app, Key::Escape);
    waiting.join().unwrap();
    let (id, ctl) = paste(&mut app);
    let waiting = asked(&mut app, id, &ctl);
    assert!(!app.dialog_text().unwrap().1.iter().any(|l| l == same));
    key(&mut app, Key::Escape);
    waiting.join().unwrap();
}

#[test]
fn conflict_buttons_never_overlap_even_in_a_narrow_window() {
    for width in [1000, 420, 300] {
        let mut app = app_at(r"C:\x");
        app.on_event(&Event::Resized { width, height: 600 }, t0());
        let (id, ctl) = paste(&mut app);
        let waiting = asked(&mut app, id, &ctl);
        let (d, b, check) = (app.jobs.r.dialog, app.jobs.r.buttons.clone(), app.jobs.r.check);
        let apart = |a: &Rect, c: &Rect| a.right() <= c.x || c.right() <= a.x || a.bottom() <= c.y || c.bottom() <= a.y;
        for (i, r) in b.iter().enumerate() {
            assert!(r.x >= d.x && r.right() <= d.right() && r.y >= d.y && r.bottom() <= d.bottom() && r.w > 0.0, "{width}: button {i} {r:?} leaves {d:?}");
            assert!(apart(r, &check), "{width}: button {i} covers the checkbox");
            assert!(b[i + 1..].iter().all(|o| apart(r, o)), "{width}: button {i} overlaps");
        }
        assert!(b[3].bottom() <= b[0].y, "{width}: the choices for all conflicts sit above the answers");
        key(&mut app, Key::Escape);
        waiting.join().unwrap();
    }
}

#[test]
fn one_conflict_review_applies_file_and_folder_choices_independently() {
    let mut app = app_at(r"C:\x");
    let (id, ctl) = paste(&mut app);
    let mut leaf = sizes(15, 25);
    let mut folder = sizes(0, 0);
    leaf.target = FsPath::parse(r"C:\x\file").unwrap();
    folder.target = FsPath::parse(r"C:\x\folder").unwrap();
    folder.from.attrs = crate::listing::attr::DIR;
    folder.to.attrs = crate::listing::attr::DIR;
    let conflicts = vec![leaf, folder];
    let tab = app.tab().id;
    app.on_provider_event(ProviderEvent::OpConflicts { tab, id, conflicts: conflicts.clone() }, t0());
    for width in [1000, 420, 300] {
        app.on_event(&Event::Resized { width, height: 450 }, t0());
        let mut draw = Gfx::new(Mono, 2048);
        app.frame(&mut draw, t0());
        let form = app.jobs.form.as_ref().unwrap();
        assert!(form.button_rects.iter().all(|r| r.x >= form.rect.x && r.right() <= form.rect.right() && r.w > 0.0));
    }
    let choose = |app: &mut App, row: usize, value: usize| {
        let Control::Choice { sel, .. } = &mut app.jobs.form.as_mut().unwrap().controls[row] else { panic!("expected a conflict choice") };
        *sel = value;
        app.form_changed(row);
    };
    choose(&mut app, 0, 2);
    choose(&mut app, 1, 1);
    let form = app.jobs.form.as_ref().unwrap();
    assert_eq!((form.choice(2), form.choice(3)), (1, 0));
    choose(&mut app, 2, 3);
    assert!(app.jobs.form.as_ref().unwrap().note.contains("15 B"));
    app.form_submit(0);
    assert!(app.jobs.form.as_ref().unwrap().error);
    choose(&mut app, 2, 1);

    let (ready, shown) = std::sync::mpsc::channel();
    let observer = ctl.clone();
    let worker = std::thread::spawn(move || observer.confirm_conflicts(conflicts, |_| { ready.send(()).unwrap(); }));
    shown.recv_timeout(Duration::from_secs(2)).unwrap();
    app.form_submit(0);
    assert_eq!(worker.join().unwrap(), Some(vec![Resolve::Skip, Resolve::Replace]));
    assert!(app.jobs.form.is_none());
}

#[test]
fn closing_a_conflict_review_cancels_the_transfer() {
    let mut app = app_at(r"C:\x");
    let (id, ctl) = paste(&mut app);
    app.on_provider_event(ProviderEvent::OpConflicts { tab: app.tab().id, id, conflicts: vec![sizes(10, 20)] }, t0());
    key(&mut app, Key::Escape);
    assert!(ctl.is_cancelled());
    assert!(app.jobs.form.is_none());
}

#[test]
fn speed_limit_from_the_operations_panel_changes_only_that_copy() {
    let mut app = app_at(r"C:\x");
    app.run_action("ops.show", t0());
    let (id, ctl) = paste(&mut app);
    ctl.set_copy_limit_available(true);
    let (_, other) = paste(&mut app);
    let row = app.jobs.list.iter().position(|j| j.id == id).map(|k| app.jobs.row_of(k)).unwrap();
    let set_rate = |app: &mut App, value: &str| {
        click(app, |a| Jobs::more_rect(a.jobs.r.rows[row], unscaled));
        let option = app.job_menu_items(row).iter().position(|&s| s == "Limit speed…").expect("running copy has a speed limit");
        click(app, |a| a.jobs.r.menu[option]);
        app.jobs.form.as_mut().unwrap().set_text(0, value);
        app.form_submit(0);
    };
    set_rate(&mut app, "7");
    assert_eq!(ctl.copy_limit(), 7);
    assert_eq!(other.copy_limit(), 0);
    set_rate(&mut app, "0");
    assert_eq!(ctl.copy_limit(), 0);
    finish(&mut app, id, None);
    assert!(!app.job_menu_items(row).contains(&"Limit speed…"));
}

#[test]
fn a_running_operation_pauses_and_resumes_from_its_row_or_its_menu() {
    let mut app = app_at(r"C:\x");
    app.run_action("ops.show", t0());
    let (first, ctl) = paste(&mut app);
    ctl.set_copy_limit_available(true);
    let (_, other) = paste(&mut app);
    let tab = app.tab().id;
    let progress = OpProgress { stage: Stage::Running, bytes: 50, total_bytes: 200, items: 1, total_items: 4 };
    app.on_provider_event(ProviderEvent::OpProgress { tab, id: first, progress }, t0());
    let row = app.jobs.list.iter().position(|j| j.id == first).map(|k| app.jobs.row_of(k)).unwrap();
    let mut g = Gfx::new(Mono, 2048);
    app.frame(&mut g, t0());
    assert!(app.keeps_awake());
    click(&mut app, |a| Jobs::pause_rect(a.jobs.r.rows[row], unscaled));
    assert!(ctl.is_paused() && !other.is_paused(), "only that operation pauses");
    let j = app.jobs.list.iter().find(|j| j.id == first).unwrap();
    assert_eq!(j.status(), "Paused");
    assert!(j.detail(false).starts_with("50 B of 200 B"));
    assert_eq!(app.overall_progress().map(|p| p.1), Some(true), "the taskbar shows the paused state");
    app.frame(&mut g, t0());
    click(&mut app, |a| Jobs::more_rect(a.jobs.r.rows[row], unscaled));
    assert_eq!(app.job_menu_items(row), ["Open destination", "Resume", "Cancel", "Limit speed…"]);
    click(&mut app, |a| a.jobs.r.menu[1]);
    assert!(!ctl.is_paused() && !ctl.is_cancelled());
    app.pause_job(row);
    finish(&mut app, first, None);
    app.frame(&mut g, t0());
    let done = app.jobs.list.iter().position(|j| j.id == first).map(|k| app.jobs.row_of(k)).unwrap();
    let r = Jobs::pause_rect(app.jobs.r.rows[done], unscaled);
    assert_ne!(app.jobs_hit(r.x + 2.0, r.y + 2.0), Some(Hit::OpsPause(done)), "a finished operation has nothing to pause");
    assert!(!app.job_menu_items(done).contains(&"Resume"));
}
#[test]
fn review_for_a_missing_job_releases_the_waiting_operation() {
    let mut app = app_at(r"C:\x");
    let (id, ctl) = paste(&mut app);
    app.jobs.list.clear();
    let (ready, started) = mpsc::channel();
    let observer = ctl.clone();
    let waiting = std::thread::spawn(move || observer.confirm_conflicts(vec![sizes(1, 2)], |_| ready.send(()).unwrap()));
    started.recv_timeout(Duration::from_secs(2)).unwrap();
    app.job_conflicts(id, vec![sizes(1, 2)]);
    assert!(ctl.is_cancelled());
    assert_eq!(waiting.join().unwrap(), None);
    assert!(app.jobs.form.is_none());
}

#[test]
fn a_speed_limit_form_preserves_an_open_conflict_review() {
    let mut app = app_at(r"C:\x");
    let (id, ctl) = paste(&mut app);
    ctl.set_copy_limit_available(true);
    app.job_conflicts(id, vec![sizes(1, 2)]);
    let option = app.job_menu(0).iter().position(|&s| s == S::UiJobCopyLimit).unwrap();
    app.jobs.menu = Some(0);
    app.run_job_menu(option, t0());
    assert!(matches!(app.jobs.form.as_ref().unwrap().kind, super::super::archive::Kind::CopyLimit(_)));
    app.jobs.form.as_mut().unwrap().set_text(0, "3");
    app.form_submit(0);
    assert!(matches!(app.jobs.form.as_ref().unwrap().kind, super::super::archive::Kind::ConflictTable { .. }));
    assert_eq!(ctl.copy_limit(), 3);
    assert!(!ctl.is_cancelled());
}

#[test]
fn a_transfer_outside_the_flit_engine_has_no_speed_limit_controls() {
    let mut app = app_at(r"C:\x");
    let (_, ctl) = paste(&mut app);
    ctl.set_copy_limit(7);
    assert!(!app.job_menu(0).contains(&S::UiJobCopyLimit));
    assert!(!app.jobs.list[0].detail(false).contains("7"));
    ctl.set_copy_limit_available(true);
    assert!(app.job_menu(0).contains(&S::UiJobCopyLimit));
    assert!(app.jobs.list[0].detail(false).contains("7"));
    ctl.set_copy_limit_available(false);
    assert!(!app.job_menu(0).contains(&S::UiJobCopyLimit));
    assert!(!app.jobs.list[0].detail(false).contains("7"));
}

#[test]
fn folder_review_defaults_to_merge_and_hides_children_when_the_folder_is_renamed() {
    let mut app = app_at(r"C:\x");
    let (id, _) = paste(&mut app);
    let folder = Conflict {
        source: p(r"C:\in\tree"), target: p(r"C:\out\tree"),
        from: Meta { attrs: crate::listing::attr::DIR, ..Default::default() },
        to: Meta { attrs: crate::listing::attr::DIR, ..Default::default() },
    };
    let file = Conflict { source: folder.source.join(b"file"), target: folder.target.join(b"file"), ..sizes(1, 2) };
    app.job_conflicts(id, vec![folder, file, sizes(1, 2)]);
    let form = app.jobs.form.as_mut().unwrap();
    let Control::Choice { sel, options, .. } = &mut form.controls[2] else { panic!("expected a folder choice") };
    assert_eq!(options[*sel], "Merge");
    assert!(!options.iter().any(|s| s == "Replace"));
    let choose = |app: &mut App, index: usize, choice: usize| {
        let Control::Choice { sel, .. } = &mut app.jobs.form.as_mut().unwrap().controls[index] else { panic!("expected a choice") };
        *sel = choice;
        app.form_changed(index);
    };
    choose(&mut app, 3, 1);
    choose(&mut app, 2, 2);
    assert_eq!(app.jobs.form.as_ref().unwrap().controls.len(), 4);
    choose(&mut app, 2, 0);
    let form = app.jobs.form.as_ref().unwrap();
    assert_eq!(form.controls.len(), 5);
    assert_eq!(form.choice(3), 1);
    app.form_submit(0);
    assert!(app.jobs.form.is_none());
}
