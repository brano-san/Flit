use super::*;
use crate::location::Location;
use crate::provider::ProviderRequest;
use crate::ui::{Drive, Effect, Persisted};
use crate::FsPath;
use std::time::Instant;

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
    assert!(!a.run_action("drive.connect_server", Instant::now()));
    a.caps.unix_paths = true;
    assert!(!a.run_action("drive.map", Instant::now()));
    a.connect_server("smb://nas/share");
    a.connect_server("nas");
    assert_eq!(launches(&mut a), [Launch::ConnectServer("smb://nas/share".into())]);
}

#[test]
fn network_goes_to_the_network_place() {
    let mut a = app();
    assert!(a.run_action("nav.network", Instant::now()));
    assert_eq!(a.tab().location(), &Location::Local(FsPath::network()));
}

#[test]
fn a_disconnected_mapped_drive_says_so() {
    let mut d = Drive { path: FsPath::parse(r"Z:\").unwrap(), label: r"share (\\nas) (Z:)".into(), total: 0, free: 0, removable: false, unmounted: true, kind: None, remote: Some(r"\\nas\share".into()) };
    assert!(d.offline());
    assert_eq!(d.title(), format!("share (\\\\nas) (Z:) \u{2014} {}", S::SidebarDriveOffline.t()));
    d.unmounted = false;
    assert_eq!(d.title(), r"share (\\nas) (Z:)");
}
