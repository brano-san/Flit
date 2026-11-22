use super::*;

#[test]
fn identifies_the_installed_package_and_shows_the_matching_upgrade_command() {
    let exe = Path::new("/usr/bin/flit");
    let deb = detect(exe, None, None, |manager, path| {
        assert_eq!((manager, path), (Probe::Dpkg, exe));
        Some("flit:amd64: /usr/bin/flit\n".to_owned())
    });
    assert_eq!(deb, Some(ManagedInstallation::Package("sudo apt install --only-upgrade flit:amd64".into())));

    let rpm = detect(exe, None, None, |manager, _| match manager {
        Probe::Dpkg => None,
        Probe::Rpm => Some("flit-nightly\n".to_owned()),
    });
    assert_eq!(rpm, Some(ManagedInstallation::Package("sudo dnf upgrade flit-nightly".into())));
}

#[test]
fn appimage_is_detected_before_package_probes_and_bad_owners_leave_portable_updates() {
    let exe = Path::new("/tmp/Flit/bin/flit");
    let appimage = detect(exe, Some(OsStr::new("/tmp/Flit.AppImage")), Some(OsStr::new("/tmp/Flit")), |_, _| panic!("AppImage is not owned by dpkg or rpm"));
    assert_eq!(appimage, Some(ManagedInstallation::AppImage));

    let portable = detect(exe, Some(OsStr::new("")), Some(OsStr::new("/tmp/Flit")), |manager, _| match manager {
        Probe::Dpkg => Some("flit: /usr/bin/flit\n".to_owned()),
        Probe::Rpm => Some("-invalid; shell".to_owned()),
    });
    assert_eq!(portable, None);
}

#[test]
fn inherited_appimage_variables_do_not_disable_portable_updates() {
    let image = Some(OsStr::new("/home/user/Terminal.AppImage"));
    let dir = Some(OsStr::new("/tmp/.mount_Terminal"));
    for (exe, appdir) in [
        ("/opt/flit/bin/flit", dir),
        ("/tmp/.mount_Terminal-copy/flit", dir),
        ("/tmp/.mount_Terminal/bin/flit", None),
        ("/opt/flit/bin/flit", Some(OsStr::new("/"))),
        ("/tmp/.mount_Terminal/bin/flit", Some(OsStr::new("/tmp/../tmp/.mount_Terminal"))),
    ] {
        assert_eq!(detect(Path::new(exe), image, appdir, |_, _| None), None, "{exe}");
    }
}
