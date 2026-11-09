use super::*;
use crate::input::{Event, Key, Mods};
use crate::location::{Location, ProviderId};
use crate::provider::{FileOp, OpCtl};
use crate::ui::settings::{index, SETTINGS};
use crate::ui::{Persisted, Value};
use crate::FsPath;
use std::time::Instant;

fn app() -> App {
    let mut app = App::new(Persisted { path: Location::parse(r"C:\Work").unwrap(), ..Default::default() }, Vec::new(), 0, false, 1.0, (1000, 600), Instant::now());
    app.animations = false;
    app.drain_effects().for_each(drop);
    app
}

fn release(version: &str) -> Release {
    Release { version: version.into(), notes: "## What's Changed\n\n\n* Faster copies\n".into(), url: "https://example.com/a.zip".into(), signature: "QUJD".into() }
}

fn found(manual: bool, version: Option<&str>) -> UpdateEvent {
    UpdateEvent::Checked { manual, found: Ok(version.map(release)) }
}

fn requests(app: &mut App) -> Vec<UpdateRequest> {
    app.drain_effects().filter_map(|e| if let Effect::Update(r) = e { Some(r) } else { None }).collect()
}

fn status(app: &App) -> Option<String> {
    app.tab().error.clone()
}

fn stage(app: &App) -> Option<Stage> {
    match &app.jobs.form.as_ref()?.kind {
        Kind::Update(u) => Some(u.stage),
        _ => None,
    }
}

fn key(app: &mut App, key: Key) {
    app.on_event(&Event::Key { key, mods: Mods::default(), pressed: true }, Instant::now());
}

#[test]
fn start_up_check_is_quiet_unless_there_is_a_new_version() {
    let mut a = app();
    a.on_update(found(false, None));
    a.on_update(UpdateEvent::Checked { manual: false, found: Err("offline".into()) });
    assert_eq!(status(&a), None);
    assert!(a.jobs.form.is_none());
    a.on_update(found(false, Some("9.0.0")));
    let f = a.jobs.form.as_ref().unwrap();
    assert_eq!(f.title, crate::tr!(S::UpdateDialogTitle, version = "9.0.0"));
    assert_eq!(f.buttons, vec![S::UpdateDialogInstall.t(), S::UpdateDialogSkip.t(), S::UpdateDialogLater.t()]);
    assert!(matches!(f.controls.last(), Some(Control::Line(l)) if l == "• Faster copies"));
    assert_eq!(f.focus, f.controls.len(), "Enter installs");
}

#[test]
fn manual_check_reports_up_to_date_and_errors() {
    let mut a = app();
    a.run_action("app.check_updates", Instant::now());
    assert_eq!(requests(&mut a), vec![UpdateRequest::Check { manual: true }]);
    assert_eq!(status(&a).as_deref(), Some(S::UpdateStatusChecking.t()));
    a.on_update(found(true, None));
    assert_eq!(status(&a), Some(crate::tr!(S::UpdateStatusUpToDate, version = CURRENT)));
    a.on_update(UpdateEvent::Checked { manual: true, found: Err("no route to host".into()) });
    assert!(status(&a).unwrap().contains("no route to host"));
}

#[test]
fn skipped_version_is_remembered_and_only_the_start_up_check_honours_it() {
    let mut a = app();
    a.on_update(found(false, Some("9.0.0")));
    a.form_submit(1);
    assert!(a.jobs.form.is_none());
    let i = index("updates.skipped").unwrap();
    assert_eq!(SETTINGS[i].get(&a), Value::Strings(vec!["9.0.0".into()]));
    a.on_update(found(false, Some("9.0.0")));
    assert!(a.jobs.form.is_none(), "start-up check stays quiet about a skipped version");
    a.on_update(found(false, Some("9.0.1")));
    assert!(a.jobs.form.is_some(), "a newer one is offered again");
    a.form_submit(2);
    a.on_update(found(true, Some("9.0.0")));
    assert!(a.jobs.form.is_some(), "Check for updates still offers it");
}

#[test]
fn running_file_operations_hold_the_install_back() {
    let mut a = app();
    let op = FileOp::Copy { items: vec![FsPath::parse(r"C:\Work\a.txt").unwrap()], dest: FsPath::parse(r"D:\").unwrap() };
    a.track_job(1, &ProviderId::Local, &op, &OpCtl::default());
    a.on_update(found(false, Some("9.0.0")));
    a.drain_effects().for_each(drop);
    key(&mut a, Key::Enter);
    assert_eq!(stage(&a), Some(Stage::Ask));
    assert!(requests(&mut a).is_empty(), "nothing downloads while a copy runs");
    let f = a.jobs.form.as_ref().unwrap();
    assert!(f.error && f.note == crate::trn!(P::UpdateDialogJobs, 1), "{}", f.note);
    a.job_done(1, None, false, Instant::now());
    key(&mut a, Key::Enter);
    assert_eq!(stage(&a), Some(Stage::Busy));
    assert_eq!(requests(&mut a), vec![UpdateRequest::Install(release("9.0.0"))]);
}

#[test]
fn download_shows_progress_and_can_be_cancelled() {
    let mut a = app();
    a.on_update(found(false, Some("9.0.0")));
    a.form_submit(0);
    assert_eq!(a.jobs.form.as_ref().unwrap().buttons, vec![S::WidgetsCancel.t()]);
    a.on_update(UpdateEvent::Progress { received: 42, total: Some(100) });
    let f = a.jobs.form.as_ref().unwrap();
    assert_eq!(f.note, crate::tr!(S::UpdateDialogDownloading, version = "9.0.0", percent = 42));
    assert_eq!(f.progress, Some(0.42));
    a.on_update(UpdateEvent::Progress { received: 50, total: None });
    assert_eq!(a.jobs.form.as_ref().unwrap().note, crate::tr!(S::UpdateDialogDownloadingPlain, version = "9.0.0"));
    a.on_update(UpdateEvent::Installing);
    assert_eq!(a.jobs.form.as_ref().unwrap().note, crate::tr!(S::UpdateDialogInstalling, version = "9.0.0"));
    a.drain_effects().for_each(drop);
    key(&mut a, Key::Escape);
    assert!(a.jobs.form.is_none());
    assert_eq!(requests(&mut a), vec![UpdateRequest::Cancel]);
}

#[test]
fn failure_returns_to_the_question_and_read_only_folder_points_to_the_releases() {
    let mut a = app();
    a.on_update(found(true, Some("9.0.0")));
    a.form_submit(0);
    a.on_update(UpdateEvent::Failed("disk full".into()));
    assert_eq!(stage(&a), Some(Stage::Ask));
    let f = a.jobs.form.as_ref().unwrap();
    assert!(f.error && f.note.contains("disk full") && f.progress.is_none());
    assert_eq!(f.buttons.len(), 3);
    a.form_submit(0);
    a.on_update(UpdateEvent::ReadOnly(r"C:\Program Files\Flit".into()));
    assert_eq!(stage(&a), Some(Stage::ReadOnly));
    assert_eq!(a.jobs.form.as_ref().unwrap().buttons, vec![S::UpdateDialogOpenReleases.t(), S::UpdateDialogLater.t()]);
    a.drain_effects().for_each(drop);
    a.form_submit(0);
    assert!(a.jobs.form.is_none());
    assert!(a.drain_effects().any(|e| matches!(e, Effect::Provider { req: crate::provider::ProviderRequest::Launch { launch: Launch::Run { program, .. }, .. }, .. } if program == RELEASES_URL)));
}

#[test]
fn late_events_without_the_dialog_go_to_the_status_line() {
    let mut a = app();
    a.on_update(UpdateEvent::Progress { received: 1, total: Some(2) });
    a.on_update(UpdateEvent::Failed("gone".into()));
    assert!(a.jobs.form.is_none());
    assert!(status(&a).unwrap().contains("gone"));
}

#[test]
fn start_up_setting_defaults_on_and_round_trips() {
    let mut a = app();
    assert!(a.checks_updates_on_start());
    let i = index(crate::ui::settings::CHECK_UPDATES).unwrap();
    assert!(SETTINGS[i].set(&mut a, Value::Bool(false)));
    assert!(!a.checks_updates_on_start());
}

#[test]
fn release_notes_are_trimmed_and_capped() {
    assert_eq!(note_lines("\n\nA\n\n\nB  \n\n"), vec!["A", "", "B"]);
    let long: String = (0..30).map(|k| format!("line {k}\n")).collect();
    let lines = note_lines(&long);
    assert_eq!(lines.len(), NOTE_LINES);
    assert_eq!(lines.last().map(String::as_str), Some("…"));
    assert!(note_lines("").is_empty());
    assert_eq!(note_lines("## What's Changed\n* Faster copies\n- **Bold** fix"), vec!["What's Changed", "• Faster copies", "• Bold fix"]);
}

#[test]
fn the_dialog_leaves_room_between_its_parts_and_keeps_buttons_to_their_text() {
    let mut a = app();
    a.on_update(found(true, Some("9.0.0")));
    a.frame(&mut crate::text::Gfx::new(crate::text::tests::Mono, 1024), Instant::now());
    let f = a.jobs.form.as_ref().unwrap();
    let values = f.controls.iter().position(|c| matches!(c, Control::Line(_))).unwrap();
    assert!(f.rows[values].y >= f.rows[values - 1].bottom() + 8.0, "release notes are set apart from the versions");
    let last = f.rows.last().unwrap();
    assert!(f.button_rects[0].y - last.bottom() >= 40.0, "the note and a gap sit between the rows and the buttons");
    assert!((f.rect.right() - 20.0 - f.button_rects.last().unwrap().right()).abs() < 1.0, "buttons end at the right edge");
    assert!(f.button_rects.windows(2).all(|w| (w[1].x - w[0].right() - 8.0).abs() < 1.0));
    assert!(f.button_rects.iter().map(|r| r.w).sum::<f32>() < f.rect.w - 80.0, "buttons are as wide as their text, not stretched across");
}
