use super::*;
use crate::input::{Event, Key, Mods, MouseButton};
use crate::location::Location;
use crate::provider::{ProviderEvent, ProviderRequest};
use crate::ui::{Drive, Effect, Persisted};
use crate::FsPath;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Instant;

#[test]
fn connection_failure_hides_passwords_from_the_status_line() {
    let mut a = linux();
    let tab = a.tab().id;
    a.on_provider_event(ProviderEvent::OpDone {
        tab, id: 0,
        error: Some("Cannot connect to sftp://ann:secret@nas/docs (password=also-secret&port=22)".into()),
        report: None,
    }, Instant::now());
    assert_eq!(a.tab().error.as_deref(), Some("Cannot connect to sftp://ann:***@nas/docs (password=***&port=22)"));
}

#[test]
fn connection_history_masks_the_address_but_the_request_uses_the_password() {
    let mut a = linux();
    let now = Instant::now();
    let uri = "smb://ann:secret@nas/";
    a.submit_address(uri, now);
    assert_eq!(a.addr.typed.lock().unwrap().as_slice(), ["smb://ann@nas"]);
    a.connect_server(uri);
    assert_eq!(launches(&mut a), [Launch::ConnectServer(uri.into())]);
    let loc = Location::from_local(FsPath::network_server(uri.as_bytes()).unwrap());
    assert!(loc.display().contains("smb://ann:***@nas/"));
    assert_eq!(loc.crumbs().last().unwrap().0, "smb://ann:***@nas/");
    assert!(loc.to_bytes().ends_with(uri.as_bytes()));
}

#[test]
fn connect_dialog_offers_an_unchecked_network_checkbox_only_on_linux() {
    let mut a = linux();
    a.run_action("drive.connect_server", Instant::now());
    let f = a.jobs.form.as_ref().expect("Linux opens the connection form");
    assert!(matches!(f.controls.get(1), Some(Control::Check { on: false, .. })));
    let mut a = app();
    a.run_action("drive.connect_server", Instant::now());
    assert!(a.jobs.form.is_none());
    assert!(matches!(a.nav.palette.as_ref().map(|p| &p.pick), Some(Pick::Server)));
}

#[test]
fn the_connect_dialog_shows_the_address_example_from_the_start_and_turns_it_red_only_for_a_bad_address() {
    let mut a = linux();
    a.run_action("drive.connect_server", Instant::now());
    let f = a.jobs.form.as_ref().unwrap();
    assert_eq!((f.note.as_str(), f.error), (S::SidebarDriveServerPrompt.t(), false), "the example is a hint before anything is typed");
    a.jobs.form.as_mut().unwrap().set_text(0, "192.168.1.35");
    a.form_submit(0);
    let f = a.jobs.form.as_ref().expect("a bare address keeps the dialog open");
    assert_eq!((f.note.as_str(), f.error), (S::SidebarDriveServerPrompt.t(), true));
    let mut a = linux();
    a.run_action("drive.connect_server", Instant::now());
    a.jobs.form.as_mut().unwrap().set_text(0, "sftp://ann@192.168.1.35");
    a.form_submit(0);
    assert!(a.jobs.form.is_none(), "a full address still connects at once");
}

#[test]
fn saved_addresses_preserve_user_port_and_path_without_credentials() {
    for scheme in SCHEMES {
        assert_eq!(saved_address(&format!("{scheme}://Ann:secret@NAS:2222/Shared/")), Some(format!("{scheme}://Ann@nas:2222/Shared")));
    }
    let ipv6 = saved_address("SFTP://ann%40work@[FD00::A]:2222/docs/").unwrap();
    assert_eq!(ipv6, "sftp://ann%40work@[fd00::a]:2222/docs");
    assert_eq!(server_label(&ipv6), "ann%40work@[fd00::a]");
    assert_eq!(saved_address("SFTP://Анна@СЕРВЕР.рф/Файлы/").as_deref(), Some("sftp://Анна@сервер.рф/Файлы"));
}

#[test]
fn checking_the_box_saves_only_a_successfully_connected_address_and_only_once() {
    let mut a = linux();
    let tab = a.tab().id;
    let path = FsPath::parse("/run/user/1000/gvfs/sftp:host=nas,user=Ann").unwrap();
    let address = "sftp://Ann@nas/docs";
    for (input, save, success) in [(address, false, true), (address, true, false), ("SFTP://Ann:secret@NAS/docs/", true, true), (address, true, true)] {
        a.run_action("drive.connect_server", Instant::now());
        let form = a.jobs.form.as_mut().unwrap();
        form.set_text(0, input);
        let Control::Check { on, .. } = &mut form.controls[1] else { panic!("missing checkbox") };
        *on = save;
        a.form_submit(0);
        let request = launches(&mut a);
        assert_eq!(request.len(), 1);
        let event = match &request[0] {
            Launch::ConnectServer(uri) if !save && success && uri == address => ProviderEvent::Navigate { tab, path: path.clone() },
            Launch::ConnectAndSaveServer(uri) if save && uri == address && success => ProviderEvent::SavedServerConnected { tab, address: uri.clone(), path: path.clone() },
            Launch::ConnectAndSaveServer(uri) if save && uri == address => ProviderEvent::OpDone { tab, id: 0, error: Some("offline".into()), report: None },
            unexpected => panic!("{unexpected:?}"),
        };
        a.on_provider_event(event, Instant::now());
        if save && success {
            assert_eq!(a.nav_prefs().saved_servers, [SavedServer { address: address.into(), name: None }]);
        } else {
            assert!(a.nav_prefs().saved_servers.is_empty());
        }
    }
    a.drain_effects().for_each(drop);
    in_network(&mut a);
    publish(&mut a, true, &SERVERS, None);
    assert_eq!(rows(&a)[0], "Ann@nas");
}

#[path = "network_menu.rs"]
mod menu;

fn login_ask(flags: u32) -> MountAsk {
    MountAsk::Login(LoginAsk { server: "nas".into(), message: "Password required for share docs on nas".into(), user: "ann".into(), domain: "WORK".into(), flags })
}

fn asked(a: &mut App, ask: MountAsk) -> (u64, Receiver<MountReply>) {
    let (answer, rx) = MountAnswer::new();
    let id = answer.id();
    let tab = a.tab().id;
    a.on_provider_event(ProviderEvent::MountAsk { tab, gen: None, ask, answer }, Instant::now());
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
    a.ask_unlock(FsPath::parse("/dev/sdd2").unwrap(), "Encrypted volume", false);
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
    a.ask_unlock(FsPath::parse("/dev/sdd2").unwrap(), "Encrypted volume", false);
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
fn an_empty_network_place_checks_once_and_the_banner_opens_sharing_settings() {
    let mut a = app();
    let now = Instant::now();
    assert!(a.run_action("nav.network", now));
    a.drain_effects().for_each(drop);
    publish(&mut a, false, &[], None);
    assert!(!a.drain_effects().any(|e| matches!(e, Effect::NetworkDiscovery { .. })));
    publish(&mut a, true, &[], None);
    let checks: Vec<_> = a.drain_effects().filter_map(|e| match e {
        Effect::NetworkDiscovery { tab, gen } => Some((tab, gen)),
        _ => None,
    }).collect();
    assert_eq!(checks, [(a.tab().id, a.tab().gen)]);
    let (tab, gen) = checks[0];
    a.on_provider_event(ProviderEvent::NetworkDiscovery { tab, gen: gen + 1, warning: Some(NetworkWarning::WindowsDisabled) }, now);
    assert!(a.network_settings_rect().is_none());
    a.on_provider_event(ProviderEvent::NetworkDiscovery { tab, gen, warning: Some(NetworkWarning::WindowsDisabled) }, now);
    let r = a.network_settings_rect().unwrap();
    let (x, y) = (r.x + r.w * 0.5, r.y + r.h * 0.5);
    assert_eq!(a.hit(x, y), crate::ui::Hit::NetworkSettings);
    a.activate(crate::ui::Hit::NetworkSettings, now);
    assert_eq!(launches(&mut a), [Launch::Run { program: "ms-settings:network-advancedsharing".into(), args: String::new(), dir: None }]);
    publish(&mut a, true, &[], None);
    assert!(!a.drain_effects().any(|e| matches!(e, Effect::NetworkDiscovery { .. })));
}

#[test]
fn a_nonempty_network_and_an_enabled_firewall_do_not_show_the_banner() {
    let mut a = app();
    let now = Instant::now();
    a.run_action("nav.network", now);
    a.drain_effects().for_each(drop);
    publish(&mut a, true, &[("server", "Server")], None);
    assert!(!a.drain_effects().any(|e| matches!(e, Effect::NetworkDiscovery { .. })));
    publish(&mut a, true, &[], None);
    let (tab, gen) = (a.tab().id, a.tab().gen);
    a.on_provider_event(ProviderEvent::NetworkDiscovery { tab, gen, warning: None }, now);
    assert!(a.discovery_warning().is_none());
    assert!(a.network_settings_rect().is_none());
}

#[test]
fn refresh_rechecks_discovery_and_drops_a_warning_from_an_earlier_load() {
    let mut a = app();
    let now = Instant::now();
    assert!(a.run_action("nav.network", now));
    publish(&mut a, true, &[], None);
    let (tab, gen) = (a.tab().id, a.tab().gen);
    a.on_provider_event(ProviderEvent::NetworkDiscovery { tab, gen, warning: Some(NetworkWarning::WindowsDisabled) }, now);
    assert!(a.network_settings_rect().is_some());
    a.activate(crate::ui::Hit::NetworkSettings, now);
    launches(&mut a);

    assert!(a.run_action("nav.refresh", now));
    publish(&mut a, false, &[], None);
    assert!(!a.drain_effects().any(|e| matches!(e, Effect::NetworkDiscovery { .. })));
    publish(&mut a, true, &[], None);
    let current = a.tab().gen;
    assert_ne!(gen, current);
    assert!(a.drain_effects().any(|e| matches!(e, Effect::NetworkDiscovery { tab: t, gen: g } if (t, g) == (tab, current))));
    a.on_provider_event(ProviderEvent::NetworkDiscovery { tab, gen, warning: Some(NetworkWarning::WindowsDisabled) }, now);
    assert!(a.network_settings_rect().is_none());
    a.on_provider_event(ProviderEvent::NetworkDiscovery { tab, gen: current, warning: None }, now);
    assert!(a.network_settings_rect().is_none());
}

#[test]
fn a_disconnected_mapped_drive_says_so() {
    let mut d = Drive { path: FsPath::parse(r"Z:\").unwrap(), label: r"share (\\nas) (Z:)".into(), total: 0, free: 0, file_system: String::new(), removable: false, unmounted: true, kind: None, remote: Some(r"\\nas\share".into()) };
    assert!(d.offline());
    assert_eq!(d.title(), format!("share (\\\\nas) (Z:) \u{2014} {}", S::SidebarDriveOffline.t()));
    d.unmounted = false;
    assert_eq!(d.title(), r"share (\\nas) (Z:)");
}

const SAMPLE_SOURCE: &str = "gio list network:/// and smb://brano-2.local:445/ on WSL Ubuntu 22.04, gvfs 1.48.2, samba 4.15, avahi 0.8";

fn linux() -> App {
    let mut a = app();
    a.set_caps(crate::caps::Caps { unix_paths: true });
    a.drain_effects().for_each(drop);
    a
}

fn listed(a: &mut App) -> Vec<FsPath> {
    a.drain_effects().filter_map(|e| if let Effect::Provider { req: ProviderRequest::List { path, .. }, .. } = e { Some(path) } else { None }).collect()
}

fn publish(a: &mut App, done: bool, items: &[(&str, &str)], error: Option<crate::listing::FsError>) {
    let mut listing = crate::listing::Listing::default();
    let mut keys = crate::sort::SortKeys::new(crate::sort::CaseMode::Insensitive);
    for &(uri, label) in items {
        let i = listing.push(uri.as_bytes(), crate::listing::Meta { attrs: crate::listing::attr::DIR, ..Default::default() });
        listing.set_label(i, label);
        keys.push(listing.label(i));
    }
    let view = crate::sort::ViewSpec::default();
    let mut order = Vec::new();
    crate::sort::sort(&listing, &keys, view, &mut order);
    let t = a.tab();
    let snap = crate::listing::DirSnapshot { gen: t.gen, path: t.location().path().clone(), listing, keys, order, view, done, error, elapsed: std::time::Duration::from_millis(1) };
    t.slot.publish(std::sync::Arc::new(snap));
    a.poll();
}

fn rows(a: &App) -> Vec<String> {
    let v = a.tab().view.as_ref().expect("a listing is shown");
    v.rows().iter().map(|&i| String::from_utf8_lossy(v.listing().label(i)).into_owned()).collect()
}

fn entry(a: &App, label: &str) -> u32 {
    let l = a.tab().view.as_ref().unwrap().listing();
    (0..l.len() as u32).find(|&i| l.label(i) == label.as_bytes()).unwrap_or_else(|| panic!("{label} is not listed"))
}

fn in_network(a: &mut App) {
    assert!(a.run_action("nav.network", Instant::now()), "Go to Network is offered");
    assert_eq!(listed(a), [FsPath::network()], "opening Network asks the platform to look for computers");
}

const SERVERS: [(&str, &str); 4] = [
    ("sftp://brano-2.local:22/", "Файлы на сервере"),
    ("smb://brano-2.local:445/", "BRANO"),
    ("dav+sd://Team%20DAV._webdav._tcp.local", "Team DAV"),
    ("nfs://brano-2.local:2049/export", "nfs export"),
];

const SMB: &str = "smb://brano-2.local:445/";
const SHARE: &str = "smb://brano-2.local/private";
const WEB_DAV: &str = "dav+sd://Team%20DAV._webdav._tcp.local";

fn denied() -> crate::listing::FsError {
    crate::listing::FsError { kind: crate::listing::FsErrorKind::AccessDenied, code: 0 }
}

fn network_location(uri: &str) -> Location {
    Location::from_local(FsPath::network_server(uri.as_bytes()).unwrap())
}

fn displayed_network() -> App {
    let mut a = linux();
    in_network(&mut a);
    publish(&mut a, true, &SERVERS, None);
    a.drain_effects().for_each(drop);
    a
}

fn mount_started(a: &mut App, uri: &str) -> (u64, u64) {
    let starts: Vec<_> = a.drain_effects().filter_map(|effect| match effect {
        Effect::Provider { req: ProviderRequest::Launch { tab, launch: Launch::ConnectNetwork { uri, gen } }, .. } => Some((tab, gen, uri)),
        Effect::Provider { req: ProviderRequest::List { path, .. }, .. } if path.is_network() => panic!("listed {} before mounting", path.display()),
        _ => None,
    }).collect();
    assert_eq!(starts.len(), 1, "{starts:?}");
    let (tab, gen, actual) = &starts[0];
    assert_eq!(actual, uri);
    let t = a.tabs().iter().find(|t| t.id == *tab).unwrap();
    assert_eq!(t.location(), &network_location(uri));
    (*tab, *gen)
}

fn mount_finished(a: &mut App, (tab, gen): (u64, u64), path: Option<FsPath>) {
    a.on_provider_event(ProviderEvent::NetworkConnected { tab, gen, result: Ok(path) }, Instant::now());
}

fn coords(a: &mut App, name: &str) -> (f32, f32) {
    a.layout();
    let row = a.tab().view.as_ref().unwrap().row_of(entry(a, name)).unwrap();
    let cell = a.row_rect(row).unwrap();
    let (x, y) = (cell.x + 80.0, cell.y + cell.h / 2.0);
    assert_eq!(a.hit(x, y), super::super::Hit::Row(row));
    assert!(!a.on_checkbox(x, y));
    (x, y)
}

fn down(a: &mut App, (x, y): (f32, f32), button: MouseButton, clicks: u8) {
    a.on_event(&Event::MouseDown { x, y, button, clicks }, Instant::now());
}

fn select(a: &mut App, label: &str) {
    let i = entry(a, label);
    let v = a.tab_mut().view.as_mut().unwrap();
    v.clear_selection();
    v.focus = Some(i);
    v.set_selected(i, true);
}

fn window_request(a: &mut App, uri: &str) -> App {
    let routes = a.drain_effects().find_map(|e| if let Effect::NewNetworkWindow(routes) = e { Some(routes) } else { None }).expect("a new window connects the selected address");
    assert_eq!(routes[0].folder, network_location(uri));
    let mut win = App::new(Persisted::default(), routes, 100, false, 1.0, (1000, 700), Instant::now());
    win.set_caps(a.caps);
    win.activate_network_tabs();
    win
}

fn password_for(a: &mut App, (tab, gen): (u64, u64)) -> Receiver<MountReply> {
    let (answer, rx) = MountAnswer::new();
    a.on_provider_event(ProviderEvent::MountAsk { tab, gen: Some(gen), ask: login_ask(ASK_PASSWORD), answer }, Instant::now());
    rx
}

#[test]
fn linux_network_lists_computers_as_they_are_found_and_f5_looks_again() {
    let mut a = linux();
    in_network(&mut a);
    assert_eq!(a.tab().location(), &Location::Local(FsPath::network()));
    publish(&mut a, false, &SERVERS[1..2], None);
    assert_eq!(rows(&a), ["BRANO"], "{SAMPLE_SOURCE}: the first server shows while the search goes on");
    publish(&mut a, true, &SERVERS, None);
    assert_eq!(rows(&a), ["BRANO", "nfs export", "Team DAV", "Файлы на сервере"]);
    a.on_event(&Event::Key { key: crate::input::Key::F(5), mods: Mods::default(), pressed: true }, Instant::now());
    assert_eq!(listed(&mut a), [FsPath::network()], "F5 searches the network again");
}

#[test]
fn typing_a_server_address_explicitly_connects_and_opens_its_list() {
    let mut a = linux();
    a.submit_address(SMB, Instant::now());
    let cmd = a.drain_effects().find_map(|e| if let Effect::Open(cmd) = e { Some(cmd) } else { None }).unwrap();
    a.open(&cmd, &|_| None, Instant::now());
    assert_eq!(launches(&mut a), [Launch::ConnectServer(SMB.into())]);
    let path = network_location(SMB).path().clone();
    let tab = a.tab().id;
    a.on_provider_event(ProviderEvent::Navigate { tab, path: path.clone() }, Instant::now());
    assert_eq!(a.current_location().path(), &path);
    assert_eq!(listed(&mut a), [path]);
}

#[test]
fn double_clicking_a_computer_lists_shares_and_a_share_opens_its_mounted_folder() {
    let mut a = displayed_network();
    let at = coords(&mut a, "BRANO");
    down(&mut a, at, MouseButton::Left, 2);
    let started = mount_started(&mut a, SMB);
    let server = network_location(SMB);
    mount_finished(&mut a, started, Some(server.path().clone()));
    assert_eq!(listed(&mut a), [server.path().clone()]);
    publish(&mut a, true, &[(SHARE, "private")], None);
    let at = coords(&mut a, "private");
    down(&mut a, at, MouseButton::Left, 2);
    let started = mount_started(&mut a, SHARE);
    let mounted = FsPath::parse_exact(b"/run/user/1000/gvfs/smb-share:server=brano-2.local,share=private").unwrap();
    mount_finished(&mut a, started, Some(mounted.clone()));
    assert_eq!(a.current_location().path(), &mounted);
    assert_eq!(listed(&mut a), [mounted]);
}

#[test]
fn enter_on_several_selected_servers_connects_only_one() {
    let mut a = displayed_network();
    assert!(a.run_action("select.all", Instant::now()));
    let v = a.tab().view.as_ref().unwrap();
    let i = v.focus.or_else(|| v.selected().next()).unwrap();
    let uri = std::str::from_utf8(v.listing().name(i)).unwrap().to_owned();
    a.on_event(&Event::Key { key: Key::Enter, mods: Mods::default(), pressed: true }, Instant::now());
    mount_started(&mut a, &uri);
}

#[test]
fn open_in_new_tab_keeps_the_original_location_and_routes_to_the_right_mount() {
    for (label, uri, destination) in [("BRANO", SMB, network_location(SMB).path().clone()), ("Team DAV", WEB_DAV, FsPath::parse("/run/user/1000/gvfs/dav:host=team").unwrap())] {
        let mut a = displayed_network();
        select(&mut a, label);
        assert!(a.run_action("item.open_tab", Instant::now()));
        let started = mount_started(&mut a, uri);
        mount_finished(&mut a, started, Some(destination.clone()));
        assert_eq!(a.tabs()[0].location().path(), &FsPath::network());
        assert_eq!(a.tabs()[1].location().path(), &destination);
    }
}

#[test]
fn open_in_new_window_connects_the_address_in_the_new_window() {
    let mut a = displayed_network();
    select(&mut a, "Team DAV");
    assert!(a.run_action("item.open_window", Instant::now()));
    let mut win = window_request(&mut a, WEB_DAV);
    let started = mount_started(&mut win, WEB_DAV);
    let mounted = FsPath::parse("/run/user/1000/gvfs/dav:host=team").unwrap();
    mount_finished(&mut win, started, Some(mounted.clone()));
    assert_eq!(win.current_location().path(), &mounted);
    assert_eq!(a.current_location().path(), &FsPath::network());
}

#[test]
fn ctrl_double_click_connects_in_the_new_window() {
    let mut a = displayed_network();
    let at = coords(&mut a, "BRANO");
    a.on_event(&Event::Key { key: Key::Other, mods: Mods { ctrl: true, ..Default::default() }, pressed: false }, Instant::now());
    down(&mut a, at, MouseButton::Left, 2);
    let mut win = window_request(&mut a, SMB);
    let started = mount_started(&mut win, SMB);
    let server = network_location(SMB);
    mount_finished(&mut win, started, Some(server.path().clone()));
    assert_eq!(win.current_location(), &server);
    assert_eq!(listed(&mut win), [server.path().clone()]);
}

#[test]
fn dragging_a_network_service_to_the_tab_strip_opens_the_mounted_path() {
    let mut a = displayed_network();
    let at = coords(&mut a, "nfs export");
    let plus = a.layout.new_tab;
    let (x, y) = (plus.x + plus.w / 2.0, plus.y + plus.h / 2.0);
    down(&mut a, at, MouseButton::Left, 1);
    a.on_event(&Event::MouseMove { x, y }, Instant::now());
    a.on_event(&Event::MouseUp { x, y, button: MouseButton::Left }, Instant::now());
    let started = mount_started(&mut a, "nfs://brano-2.local:2049/export");
    let mounted = FsPath::parse("/run/user/1000/gvfs/nfs:host=brano-2.local").unwrap();
    mount_finished(&mut a, started, Some(mounted.clone()));
    assert_eq!(a.tabs()[1].location().path(), &mounted);
}

#[test]
fn an_empty_network_says_nothing_was_found_and_how_to_search_again() {
    let mut a = linux();
    in_network(&mut a);
    publish(&mut a, true, &[], None);
    assert!(rows(&a).is_empty());
    assert_eq!(a.empty_text(a.tab().location(), false), S::UiEmptyNetwork);
    assert!(S::UiEmptyNetwork.en().contains("F5"));
    assert_eq!(a.empty_text(&Location::Local(FsPath::home_page()), false), S::UiEmptyFolder, "ordinary empty folders keep their text");
}

#[test]
fn network_without_browsers_points_to_missing_tools_in_settings() {
    let mut a = linux();
    in_network(&mut a);
    publish(&mut a, true, &[], Some(crate::listing::FsError::no_network_browser()));
    let shown = a.tab().view.as_ref().unwrap().snap().error.clone().expect("the error is shown");
    assert_eq!(shown.to_string(), S::ErrorsFsNoNetworkBrowser.t());
    assert!(S::ErrorsFsNoNetworkBrowser.en().contains("Settings"));
    assert!(launches(&mut a).is_empty());
}

#[test]
fn back_and_forward_reuse_cached_shares_and_reject_a_late_password_request() {
    let mut a = displayed_network();
    a.open_entry(entry(&a, "BRANO"), Instant::now());
    let started = mount_started(&mut a, SMB);
    let server = network_location(SMB);
    mount_finished(&mut a, started, Some(server.path().clone()));
    assert_eq!(listed(&mut a), [server.path().clone()]);
    publish(&mut a, true, &[(SHARE, "private")], None);
    a.go_history(false, Instant::now());
    assert_eq!(listed(&mut a), [FsPath::network()]);
    publish(&mut a, true, &SERVERS, None);
    a.go_history(true, Instant::now());
    a.poll();
    assert_eq!(a.current_location(), &server);
    assert_eq!(rows(&a), ["private"]);
    let effects: Vec<_> = a.drain_effects().collect();
    assert!(!effects.iter().any(|e| matches!(e, Effect::Provider { req: ProviderRequest::List { .. } | ProviderRequest::Launch { .. }, .. })));
    let late = password_for(&mut a, started);
    assert!(matches!(late.try_recv(), Ok(MountReply::Cancel)));
    assert!(a.jobs.form.is_none());
}

#[test]
fn restoring_a_server_tab_shows_an_empty_page_that_offers_to_connect() {
    let server = network_location(SMB);
    let mut a = App::new(Persisted { path: server.clone(), ..Default::default() }, Vec::new(), 0, false, 1.0, (1000, 700), Instant::now());
    a.set_caps(crate::caps::Caps { unix_paths: true });
    a.restore_tabs(&[server.clone(), network_location("smb://other/")], 0);
    for k in 0..2 {
        a.select_tab(k);
        a.poll();
        assert!(rows(&a).is_empty());
        assert_eq!(a.empty_text(a.current_location(), false), S::UiEmptyNetworkConnect);
        assert!(!a.drain_effects().any(|e| matches!(e, Effect::Provider { req: ProviderRequest::List { .. } | ProviderRequest::Launch { .. }, .. })));
        assert!(a.jobs.form.is_none());
    }
}

#[test]
fn clicking_a_server_offers_login_and_f5_after_cancel_offers_it_again() {
    let mut a = displayed_network();
    let at = coords(&mut a, "BRANO");
    down(&mut a, at, MouseButton::Left, 2);
    let first = mount_started(&mut a, SMB);
    let rx = password_for(&mut a, first);
    assert!(a.jobs.form.is_some());
    a.form_submit(1);
    assert!(matches!(rx.try_recv(), Ok(MountReply::Cancel)));
    mount_finished(&mut a, first, None);
    a.poll();
    assert!(a.jobs.form.is_none());
    assert!(a.run_action("nav.refresh", Instant::now()));
    let second = mount_started(&mut a, SMB);
    assert_ne!(first, second);
    let _reply = password_for(&mut a, second);
    assert!(a.jobs.form.is_some());
}

#[test]
fn windows_network_still_browses_servers_as_folders() {
    let mut a = app();
    a.drain_effects().for_each(drop);
    in_network(&mut a);
    publish(&mut a, true, &[("SRV", "SRV")], None);
    a.open_entry(entry(&a, "SRV"), Instant::now());
    let effects: Vec<Effect> = a.drain_effects().collect();
    assert!(!effects.iter().any(|e| matches!(e, Effect::Provider { req: ProviderRequest::Launch { .. }, .. })), "Windows does not mount servers to open them");
    let lists: Vec<&FsPath> = effects.iter().filter_map(|e| if let Effect::Provider { req: ProviderRequest::List { path, .. }, .. } = e { Some(path) } else { None }).collect();
    assert_eq!(lists, [&FsPath::network_server(b"SRV").unwrap()]);
    publish(&mut a, true, &[], None);
    assert_eq!(a.empty_text(a.tab().location(), false), S::UiEmptyFolder, "Windows keeps its empty folder text");
    publish(&mut a, true, &[], Some(denied()));
    assert!(launches(&mut a).is_empty(), "Windows asks for passwords on its own");
}

#[test]
fn the_linux_network_address_uses_a_native_location() {
    let mut a = linux();
    a.submit_address(crate::path::NETWORK_URI, Instant::now());
    let effect = a.drain_effects().find_map(|e| if let Effect::Open(cmd) = e { Some(cmd) } else { None }).expect("the address is opened in Flit");
    assert_eq!(effect.paths, [crate::path::NETWORK_URI.as_bytes()]);
    assert_eq!(crate::open::resolve_for(&effect.paths[0], None, &|_| None, true), Ok(Location::Local(FsPath::network())));
    in_network(&mut a);
    assert_eq!(a.tab().crumbs.last().unwrap().0, S::PathNetwork.t());
    a.open_address();
    assert_eq!(a.nav.palette.as_ref().unwrap().field.text(), crate::path::NETWORK_URI);
}

#[path = "network_saved_listing.rs"]
mod saved_listing;
