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

fn denied() -> crate::listing::FsError {
    crate::listing::FsError { kind: crate::listing::FsErrorKind::AccessDenied, code: 0 }
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
fn opening_an_smb_server_or_a_share_mounts_it_through_connect_to_server() {
    let mut a = linux();
    in_network(&mut a);
    publish(&mut a, true, &SERVERS, None);
    a.open_entry(entry(&a, "BRANO"), Instant::now());
    let effects: Vec<Effect> = a.drain_effects().collect();
    assert!(!effects.iter().any(|e| matches!(e, Effect::Provider { req: ProviderRequest::List { .. }, .. })), "the server is not listed before it is mounted");
    let launched: Vec<&Launch> = effects.iter().filter_map(|e| if let Effect::Provider { req: ProviderRequest::Launch { launch, .. }, .. } = e { Some(launch) } else { None }).collect();
    assert_eq!(launched, [&Launch::ConnectServer("smb://brano-2.local:445/".into())]);
    a.open_entry(entry(&a, "Файлы на сервере"), Instant::now());
    assert_eq!(launches(&mut a), [Launch::ConnectServer("sftp://brano-2.local:22/".into())]);
    let server = FsPath::network_server(b"smb://brano-2.local:445/").unwrap();
    let tab = a.tab().id;
    a.on_provider_event(ProviderEvent::Navigate { tab, path: server.clone() }, Instant::now());
    assert_eq!(listed(&mut a), [server], "after the server is mounted its shares are listed");
    publish(&mut a, true, &[("smb://brano-2.local/%D0%9E%D0%B1%D1%89%D0%B8%D0%B5%20%D0%B4%D0%BE%D0%BA%D1%83%D0%BC%D0%B5%D0%BD%D1%82%D1%8B", "Общие документы"), ("smb://brano-2.local/private", "private")], None);
    a.open_entry(entry(&a, "private"), Instant::now());
    assert_eq!(launches(&mut a), [Launch::ConnectServer("smb://brano-2.local/private".into())], "a share is mounted by the same connect path that asks for passwords");
    let mounted = FsPath::parse_exact(b"/run/user/1000/gvfs/smb-share:server=brano-2.local,share=private").unwrap();
    a.on_provider_event(ProviderEvent::Navigate { tab, path: mounted.clone() }, Instant::now());
    assert_eq!(a.tab().location(), &Location::Local(mounted.clone()));
    assert_eq!(listed(&mut a), [mounted]);
}

#[test]
fn enter_on_several_selected_servers_connects_only_one() {
    let mut a = linux();
    in_network(&mut a);
    publish(&mut a, true, &SERVERS, None);
    assert!(a.run_action("select.all", Instant::now()));
    a.drain_effects().for_each(drop);
    a.on_event(&Event::Key { key: crate::input::Key::Enter, mods: Mods::default(), pressed: true }, Instant::now());
    let effects: Vec<Effect> = a.drain_effects().collect();
    let connects = effects.iter().filter(|e| matches!(e, Effect::Provider { req: ProviderRequest::Launch { launch: Launch::ConnectServer(_), .. }, .. })).count();
    assert_eq!(connects, 1);
    assert!(!effects.iter().any(|e| matches!(e, Effect::Provider { req: ProviderRequest::List { .. }, .. })), "no tab tries to list an address as a folder");
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
fn without_gvfs_and_avahi_the_network_names_the_package_to_install() {
    let mut a = linux();
    in_network(&mut a);
    publish(&mut a, true, &[], Some(crate::listing::FsError::no_network_browser(1)));
    let shown = a.tab().view.as_ref().unwrap().snap().error.clone().expect("the error is shown");
    assert_eq!(shown.kind, crate::listing::FsErrorKind::Unavailable);
    assert_eq!(shown.to_string(), crate::tr!(S::ErrorsFsNoNetworkBrowser, package = "gvfs-smb", avahi = "avahi-tools"));
    assert_eq!(crate::listing::FsError::no_network_browser(0).to_string(), crate::tr!(S::ErrorsFsNoNetworkBrowser, package = "gvfs-backends", avahi = "avahi-utils"));
    assert_eq!(crate::listing::FsError::no_network_browser(99).network_package(), Some("gvfs-backend-samba"));
    assert_eq!(crate::listing::FsError::not_responding().network_package(), None);
    assert!(launches(&mut a).is_empty(), "a missing tool does not start a connection");
}

#[test]
fn a_server_that_needs_a_password_opens_the_login_once_per_visit() {
    let mut a = linux();
    let server = FsPath::network_server(b"smb://nas/").unwrap();
    a.navigate(Location::Local(server), Instant::now());
    a.drain_effects().for_each(drop);
    publish(&mut a, true, &[], Some(denied()));
    assert_eq!(launches(&mut a), [Launch::ConnectServer("smb://nas/".into())], "gvfs asks for the password in the login window");
    assert!(a.run_action("nav.refresh", Instant::now()));
    a.drain_effects().for_each(drop);
    publish(&mut a, true, &[], Some(denied()));
    assert!(launches(&mut a).is_empty(), "a refused login is not asked again in a loop");
    assert!(a.run_action("nav.refresh", Instant::now()));
    a.drain_effects().for_each(drop);
    publish(&mut a, true, &[("smb://nas/docs", "docs")], None);
    assert!(a.run_action("nav.refresh", Instant::now()));
    a.drain_effects().for_each(drop);
    publish(&mut a, true, &[], Some(denied()));
    assert_eq!(launches(&mut a), [Launch::ConnectServer("smb://nas/".into())], "after a successful visit the login is offered again");
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
