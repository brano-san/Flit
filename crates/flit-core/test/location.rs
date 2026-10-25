use super::*;

fn wsl(distro: &str, path: &[u8]) -> Location {
    Location::Wsl { distro: distro.into(), path: FsPath::parse_exact(path).unwrap() }
}

#[test]
fn unc_forms_map_to_wsl() {
    let want = wsl("Ubuntu", b"/home/user/src");
    assert_eq!(Location::parse(r"\\wsl.localhost\Ubuntu\home\user\src").unwrap(), want);
    assert_eq!(Location::parse(r"\\wsl$\Ubuntu\home\user\src").unwrap(), want);
    assert_eq!(Location::parse(r"\\WSL.LOCALHOST\Ubuntu\home\user\src\").unwrap(), want);
    assert_eq!(Location::parse("wsl://Ubuntu/home/user/src").unwrap(), want);
    assert_eq!(Location::parse(r"\\wsl$\Ubuntu").unwrap(), wsl("Ubuntu", b"/"));
    assert_eq!(Location::parse("wsl://Debian").unwrap(), wsl("Debian", b"/"));
    assert!(matches!(Location::parse(r"\\nas\share\x").unwrap(), Location::Local(_)));
    assert!(Location::parse("wsl:///etc").is_none(), "distro required");
}

#[test]
fn wsl_to_windows_and_back() {
    let loc = wsl("Ubuntu-22.04", b"/home/u/My Docs");
    let win = loc.to_windows().unwrap();
    assert_eq!(win.as_bytes(), br"\\wsl.localhost\Ubuntu-22.04\home\u\My Docs");
    assert_eq!(Location::from_local(win), loc);
    assert_eq!(wsl("U", b"/").to_windows().unwrap().as_bytes(), br"\\wsl.localhost\U\");
}

#[test]
fn linux_names_with_windows_forbidden_chars() {
    let loc = wsl("Ubuntu", b"/tmp/a:b\\c?d*e|f");
    assert_eq!(loc.file_name(), b"a:b\\c?d*e|f");
    let win = loc.to_windows().unwrap();
    let shown = name::display(win.file_name()).into_owned();
    assert_eq!(shown, "a\u{F03A}b\u{F05C}c\u{F03F}d\u{F02A}e\u{F07C}f");
    assert_eq!(Location::from_local(win), loc, "round trip through the 9P view");
    let raw = wsl("Ubuntu", b"/tmp/bad\xFF\xFEname");
    assert_eq!(raw.file_name(), b"bad\xFF\xFEname");
    assert!(raw.to_windows().is_none());
    assert_eq!(Location::parse_exact(&raw.to_bytes()).unwrap(), raw, "canonical form is lossless");
}

#[test]
fn navigation() {
    let loc = wsl("Ubuntu", b"/home/u");
    assert_eq!(loc.parent().unwrap(), wsl("Ubuntu", b"/home"));
    assert_eq!(wsl("Ubuntu", b"/").parent().unwrap(), Location::computer());
    assert_eq!(loc.join(b"x:y"), wsl("Ubuntu", b"/home/u/x:y"));
    let labels: Vec<String> = loc.crumbs().into_iter().map(|c| c.0).collect();
    assert_eq!(labels, ["This PC", "Ubuntu (WSL)", "home", "u"]);
    assert_eq!(loc.provider(), ProviderId::Wsl("Ubuntu".into()));
    assert_eq!(Location::parse(r"C:\x").unwrap().provider(), ProviderId::Local);
    assert_eq!(loc.display(), "wsl://Ubuntu/home/u");
}

#[test]
fn serde_roundtrip_and_validation() {
    let locs = [Location::computer(), Location::parse(r"C:\Users").unwrap(), wsl("Ubuntu", b"/a\xFFb"), Location::parse(r"\\srv\s\d").unwrap()];
    for l in locs {
        let bytes = postcard::to_allocvec(&l).unwrap();
        assert_eq!(postcard::from_bytes::<Location>(&bytes).unwrap(), l);
    }
    let forged = postcard::to_allocvec(&b"relative\\x".to_vec()).unwrap();
    assert!(postcard::from_bytes::<FsPath>(&forged).is_err());
}
