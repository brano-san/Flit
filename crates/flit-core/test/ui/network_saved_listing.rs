use super::*;
use crate::ui::SavedServer;

const SFTP: &str = "sftp://Ann@192.168.1.7:2222/share";

fn configure(a: &mut App, uri: &str, name: Option<&str>) {
    let mut prefs = a.nav_prefs();
    prefs.saved_servers = vec![SavedServer { address: uri.into(), name: name.map(str::to_owned) }];
    a.set_nav_prefs(prefs);
}

#[test]
fn saved_servers_are_visible_before_discovery_and_keep_their_own_group() {
    let mut a = linux();
    configure(&mut a, SFTP, Some("Zebra"));
    in_network(&mut a);
    a.poll();
    assert_eq!(rows(&a), ["Zebra"]);
    let found = [("SFTP://Ann@192.168.1.7:2222/share/", "Duplicate"), (WEB_DAV, "Alpha")];
    publish(&mut a, false, &found, None);
    assert_eq!(rows(&a), ["Zebra", "Alpha"]);
    let view = a.tab().view.as_ref().unwrap();
    assert_eq!(view.groups().iter().map(|g| g.count).collect::<Vec<_>>(), [1, 1]);
    assert_eq!(a.tab().entry_location(view.rows()[0]), network_location(SFTP));
    publish(&mut a, true, &found, None);
    assert_eq!(rows(&a), ["Zebra", "Alpha"]);
    configure(&mut a, SFTP, None);
    assert_eq!(rows(&a), ["Ann@192.168.1.7", "Alpha"]);
    let mut prefs = a.nav_prefs();
    prefs.saved_servers.clear();
    a.set_nav_prefs(prefs);
    assert_eq!(rows(&a), ["Alpha", "Duplicate"]);
    assert!(a.tab().view.as_ref().unwrap().groups().is_empty());
    assert!(launches(&mut a).is_empty());
}

#[test]
fn a_saved_server_can_be_opened_before_discovery_but_history_does_not_reconnect() {
    let mut a = linux();
    configure(&mut a, SFTP, None);
    in_network(&mut a);
    a.poll();
    let at = coords(&mut a, "Ann@192.168.1.7");
    down(&mut a, at, MouseButton::Left, 2);
    let first = mount_started(&mut a, SFTP);
    let answer = password_for(&mut a, first);
    a.form_submit(1);
    assert!(matches!(answer.try_recv(), Ok(MountReply::Cancel)));
    mount_finished(&mut a, first, None);
    a.go_history(false, Instant::now());
    a.poll();
    assert_eq!(rows(&a), ["Ann@192.168.1.7"]);
    assert!(launches(&mut a).is_empty());
    a.go_history(true, Instant::now());
    a.poll();
    assert_eq!(a.current_location(), &network_location(SFTP));
    assert!(launches(&mut a).is_empty());
    let mut restored = App::new(Persisted { path: Location::from_local(FsPath::network()), ..Default::default() }, Vec::new(), 0, false, 1.0, (1000, 700), Instant::now());
    restored.set_caps(crate::caps::Caps { unix_paths: true });
    restored.set_nav_prefs(a.nav_prefs());
    restored.restore_tabs(&[Location::from_local(FsPath::network()), network_location(SFTP)], 0);
    restored.poll();
    assert_eq!(rows(&restored), ["Ann@192.168.1.7"]);
    restored.select_tab(1);
    restored.poll();
    assert_eq!(restored.current_location(), &network_location(SFTP));
    assert!(launches(&mut restored).is_empty());
    assert!(restored.jobs.form.is_none());
}

#[test]
fn saved_servers_survive_missing_discovery_and_windows_ignores_the_list() {
    let mut a = linux();
    configure(&mut a, SFTP, None);
    in_network(&mut a);
    publish(&mut a, true, &[], Some(crate::listing::FsError::no_network_browser()));
    assert_eq!(rows(&a), ["Ann@192.168.1.7"]);
    assert!(a.tab().error.is_some());
    let mut win = app();
    win.drain_effects().for_each(drop);
    configure(&mut win, SFTP, None);
    in_network(&mut win);
    publish(&mut win, true, &[("WIN", "WIN")], None);
    assert_eq!(rows(&win), ["WIN"]);
    assert!(win.tab().view.as_ref().unwrap().groups().is_empty());
}
