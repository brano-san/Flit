use super::*;
use crate::input::{Event, Mods};
use crate::location::Location;
use crate::provider::{ProviderEvent, ProviderRequest};
use crate::ui::{Drive, Effect, Persisted};
use crate::FsPath;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Instant;

fn login_ask(flags: u32) -> MountAsk {
    MountAsk::Login(LoginAsk { server: "nas".into(), message: "Password required for share docs on nas".into(), user: "ann".into(), domain: "WORK".into(), flags })
}

fn asked(a: &mut App, ask: MountAsk) -> (u64, Receiver<MountReply>) {
    let (answer, rx) = MountAnswer::new();
    let id = answer.id();
    let tab = a.tab().id;
    a.on_provider_event(ProviderEvent::MountAsk { tab, ask, answer }, Instant::now());
    (id, rx)
}

fn labels(f: &Form) -> Vec<String> {
    f.controls
        .iter()
        .map(|c| match c {
            Control::Text { label, field, .. } if field.secret => format!("secret:{label}"),
            Control::Text { label, field, .. } => format!("text:{label}={}", field.text()),
            Control::Check { label, .. } => format!("check:{label}"),
            Control::Choice { label, options, .. } => format!("choice:{label}={}", options.join("|")),
            Control::Line(t) => format!("line:{t}"),
            _ => "other".into(),
        })
        .collect()
}

fn index(f: &Form, prefix: &str) -> usize {
    labels(f).iter().position(|l| l.starts_with(prefix)).unwrap_or_else(|| panic!("no {prefix} in {:?}", labels(f)))
}

#[test]
fn an_sftp_password_request_shows_only_a_password_and_the_remember_choice() {
    let mut a = app();
    asked(&mut a, login_ask(ASK_PASSWORD | ASK_SAVING));
    let f = a.jobs.form.as_ref().expect("the login form is open");
    assert_eq!(f.title, crate::tr!(S::SidebarDriveLoginTitle, server = "nas"));
    let remember = format!("choice:{}={}|{}", S::SidebarDriveLoginRemember.t(), S::SidebarDriveLoginForget.t(), S::SidebarDriveLoginSession.t());
    assert_eq!(labels(f), ["line:Password required for share docs on nas".to_owned(), format!("secret:{}", S::SidebarDriveLoginPassword.t()), remember]);
    assert_eq!(f.choice(2), 0, "not remembering is the default");
    assert_eq!(f.focus, 1, "typing goes straight into the password");
    assert_eq!(f.buttons, [S::SidebarDriveLoginConnect.t(), S::WidgetsCancel.t()]);
}

#[test]
fn an_smb_request_asks_for_user_domain_and_password_and_offers_anonymous_access() {
    let mut a = app();
    asked(&mut a, login_ask(ASK_PASSWORD | ASK_USER | ASK_DOMAIN | ASK_ANONYMOUS));
    let f = a.jobs.form.as_ref().unwrap();
    assert_eq!(
        labels(f)[1..],
        [format!("text:{}=ann", S::SidebarDriveLoginUser.t()), format!("text:{}=WORK", S::SidebarDriveLoginDomain.t()), format!("secret:{}", S::SidebarDriveLoginPassword.t()), format!("check:{}", S::SidebarDriveLoginAnonymous.t())]
    );
    assert!(!labels(f).iter().any(|l| l.starts_with("choice:")), "no remember choice when gvfs cannot save");
}

#[test]
fn connecting_sends_the_typed_password_once_and_clears_the_field() {
    let mut a = app();
    let (_, rx) = asked(&mut a, login_ask(ASK_PASSWORD | ASK_USER | ASK_SAVING));
    let f = a.jobs.form.as_mut().unwrap();
    let (user, pass, remember) = (index(f, "text:"), index(f, "secret:"), index(f, "choice:"));
    f.set_text(user, " bob ");
    f.set_text(pass, "correct horse");
    if let Control::Choice { sel, .. } = &mut f.controls[remember] {
        *sel = 1;
    }
    a.form_submit(0);
    assert!(a.jobs.form.is_none());
    let reply = rx.try_recv().expect("an answer");
    assert!(!format!("{reply:?}").contains("horse"), "the password never shows in debug output");
    match reply {
        MountReply::Login { user, domain, password, anonymous, remember } => {
            assert_eq!((user.as_str(), domain.as_str(), password.as_str(), anonymous, remember), ("bob", "WORK", "correct horse", false, true));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(rx.try_recv().err(), Some(TryRecvError::Disconnected), "one answer per request");
}

#[test]
fn anonymous_access_sends_the_anonymous_flag() {
    let mut a = app();
    let (_, rx) = asked(&mut a, login_ask(ASK_PASSWORD | ASK_ANONYMOUS));
    let f = a.jobs.form.as_mut().unwrap();
    let k = index(f, "check:");
    if let Control::Check { on, .. } = &mut f.controls[k] {
        *on = true;
    }
    a.form_submit(0);
    assert!(matches!(rx.try_recv(), Ok(MountReply::Login { anonymous: true, remember: false, .. })));
}

#[test]
fn the_secret_field_is_wiped_with_its_undo_history() {
    let mut a = app();
    asked(&mut a, login_ask(ASK_PASSWORD));
    let f = a.jobs.form.as_mut().unwrap();
    let k = index(f, "secret:");
    if let Control::Text { field, .. } = &mut f.controls[k] {
        field.edit.insert("pa55");
        field.edit.insert(" more");
        assert!(field.edit.can_undo());
    }
    f.wipe_secrets();
    if let Control::Text { field, .. } = &mut f.controls[k] {
        assert!(field.text().is_empty() && !field.edit.can_undo());
    }
}

#[test]
fn cancel_and_escape_tell_gvfs_the_login_was_aborted() {
    let mut a = app();
    let (_, rx) = asked(&mut a, login_ask(ASK_PASSWORD));
    a.form_submit(1);
    assert!(a.jobs.form.is_none());
    assert!(matches!(rx.try_recv(), Ok(MountReply::Cancel)));
    let (_, rx) = asked(&mut a, login_ask(ASK_PASSWORD));
    a.on_event(&Event::Key { key: crate::input::Key::Escape, mods: Mods::default(), pressed: true }, Instant::now());
    assert!(a.jobs.form.is_none());
    assert!(matches!(rx.try_recv(), Ok(MountReply::Cancel)));
}

#[test]
fn a_request_while_another_dialog_is_open_is_dropped_instead_of_replacing_it() {
    let mut a = app();
    a.ask_unlock(FsPath::parse("/dev/sdd2").unwrap(), "Encrypted volume");
    let (_, rx) = asked(&mut a, login_ask(ASK_PASSWORD));
    assert!(matches!(a.jobs.form.as_ref().map(|f| &f.kind), Some(Kind::Unlock { .. })), "the dialog the user is typing in stays");
    assert_eq!(rx.try_recv().err(), Some(TryRecvError::Disconnected), "the platform is not left waiting");
}

#[test]
fn gvfs_withdrawing_a_request_closes_only_its_own_form() {
    let mut a = app();
    let (id, rx) = asked(&mut a, login_ask(ASK_PASSWORD));
    let tab = a.tab().id;
    a.on_provider_event(ProviderEvent::MountAskDone { tab, id: id + 1000 }, Instant::now());
    assert!(a.jobs.form.is_some(), "another request's withdrawal leaves this form open");
    a.on_provider_event(ProviderEvent::MountAskDone { tab, id }, Instant::now());
    assert!(a.jobs.form.is_none());
    assert_eq!(rx.try_recv().err(), Some(TryRecvError::Disconnected));
    a.ask_unlock(FsPath::parse("/dev/sdd2").unwrap(), "Encrypted volume");
    a.on_provider_event(ProviderEvent::MountAskDone { tab, id }, Instant::now());
    assert!(a.jobs.form.is_some(), "an unrelated dialog is not closed");
}

#[test]
fn a_host_key_question_offers_the_choices_in_their_order() {
    let mut a = app();
    let question = MountAsk::Question { server: "host".into(), message: "The identity of the remote computer is unknown.\nAre you sure you want to continue connecting?".into(), choices: vec!["Log In Anyway".into(), "Cancel Login".into()] };
    let (_, rx) = asked(&mut a, question);
    let f = a.jobs.form.as_mut().unwrap();
    assert_eq!(labels(f), ["line:The identity of the remote computer is unknown.".to_owned(), "line:Are you sure you want to continue connecting?".to_owned(), format!("choice:{}=Log In Anyway|Cancel Login", S::SidebarDriveLoginAnswer.t())]);
    assert_eq!(f.buttons, [S::SidebarDriveLoginContinue.t(), S::WidgetsCancel.t()]);
    if let Control::Choice { sel, .. } = &mut f.controls[2] {
        *sel = 1;
    }
    a.form_submit(0);
    assert!(matches!(rx.try_recv(), Ok(MountReply::Choice(1))));
}

#[test]
fn a_question_without_choices_is_cancelled_without_a_form() {
    let mut a = app();
    let (_, rx) = asked(&mut a, MountAsk::Question { server: "host".into(), message: "?".into(), choices: Vec::new() });
    assert!(a.jobs.form.is_none());
    assert!(matches!(rx.try_recv(), Ok(MountReply::Cancel)), "an empty list is never answered as choice 0");
}

#[test]
fn long_messages_wrap_into_lines() {
    let long = "word ".repeat(40);
    let lines = message_lines(&format!("{long}\n\n  second  "));
    assert!(lines.len() >= 3 && lines.iter().all(|l| l.chars().count() <= 72), "{lines:?}");
    assert_eq!(lines.last().map(String::as_str), Some("second"));
}

#[test]
fn the_login_form_fits_a_narrow_window_in_every_language() {
    let original = crate::i18n::lang();
    for lang in crate::i18n::Lang::ALL {
        crate::i18n::set_lang(lang);
        let mut a = App::new(Persisted::default(), Vec::new(), 0, false, 1.0, (360, 480), Instant::now());
        asked(&mut a, login_ask(ASK_PASSWORD | ASK_USER | ASK_DOMAIN | ASK_ANONYMOUS | ASK_SAVING));
        let mut g = crate::text::Gfx::new(crate::text::tests::Mono, 1024);
        a.frame(&mut g, Instant::now());
        let f = a.jobs.form.as_ref().unwrap();
        assert_eq!(f.button_rects.len(), 2);
        assert!(f.button_rects.iter().all(|b| b.x >= f.rect.x && b.right() <= f.rect.right() && b.bottom() <= 480.0 && b.w > 0.0), "{lang:?}: {:?} in {:?}", f.button_rects, f.rect);
        assert!(f.button_rects[0].right() <= f.button_rects[1].x);
    }
    crate::i18n::set_lang(original);
}

fn app() -> App {
    App::new(Persisted::default(), Vec::new(), 0, false, 1.0, (1000, 700), Instant::now())
}

fn launches(app: &mut App) -> Vec<Launch> {
    app.drain_effects()
        .filter_map(|e| match e {
            Effect::Provider { req: ProviderRequest::Launch { launch, .. }, .. } => Some(launch),
            _ => None,
        })
        .collect()
}

#[test]
fn server_addresses_need_a_known_scheme_and_a_host() {
    assert_eq!(server_uri(" smb://nas/share ").as_deref(), Some("smb://nas/share"));
    assert_eq!(server_uri("sftp://me@host:2222/home").as_deref(), Some("sftp://me@host:2222/home"));
    assert_eq!(server_uri("SMB://nas").as_deref(), Some("SMB://nas"));
    for bad in ["nas/share", "file:///etc", "smb://", "smb:///share", "sftp://me@/x", "smb://nas/a b", "http://nas", "-o://x"] {
        assert_eq!(server_uri(bad), None, "{bad}");
    }
}

#[test]
fn drive_actions_reach_the_shell_and_unix_hosts_hide_them() {
    let mut a = app();
    a.drain_effects().for_each(drop);
    assert!(a.run_action("drive.map", Instant::now()));
    assert!(a.run_action("drive.disconnect", Instant::now()));
    assert_eq!(launches(&mut a), [Launch::MapDrive(None), Launch::Disconnect(None)]);
    assert!(a.run_action("drive.connect_server", Instant::now()), "Windows connects to servers too");
    a.connect_server("smb://nas/share");
    a.connect_server(r" \\nas\share\docs ");
    a.connect_server("nas");
    assert_eq!(launches(&mut a), [Launch::ConnectServer("smb://nas/share".into()), Launch::ConnectServer(r"\\nas\share\docs".into())]);
    a.caps.unix_paths = true;
    assert!(!a.run_action("drive.map", Instant::now()));
    a.connect_server("smb://nas/share");
    a.connect_server("nas");
    a.connect_server(r"\\nas\share");
    assert_eq!(launches(&mut a), [Launch::ConnectServer("smb://nas/share".into())], "a Windows network path means nothing on Linux");
}

#[test]
fn network_goes_to_the_network_place() {
    let mut a = app();
    assert!(a.run_action("nav.network", Instant::now()));
    assert_eq!(a.tab().location(), &Location::Local(FsPath::network()));
}

#[test]
fn a_disconnected_mapped_drive_says_so() {
    let mut d = Drive { path: FsPath::parse(r"Z:\").unwrap(), label: r"share (\\nas) (Z:)".into(), total: 0, free: 0, file_system: String::new(), removable: false, unmounted: true, kind: None, remote: Some(r"\\nas\share".into()) };
    assert!(d.offline());
    assert_eq!(d.title(), format!("share (\\\\nas) (Z:) \u{2014} {}", S::SidebarDriveOffline.t()));
    d.unmounted = false;
    assert_eq!(d.title(), r"share (\\nas) (Z:)");
}
