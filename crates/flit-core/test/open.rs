use super::*;

fn args(a: &[&str]) -> Vec<Vec<u8>> {
    a.iter().map(|s| s.as_bytes().to_vec()).collect()
}

fn strs(v: &[Vec<u8>]) -> Vec<String> {
    v.iter().map(|b| String::from_utf8_lossy(b).into_owned()).collect()
}

#[test]
fn explorer_forms() {
    let o = parse_args(&args(&[r"C:\Windows"]), None);
    assert_eq!(strs(&o.paths), [r"C:\Windows"]);
    assert_eq!(o.target, OpenTarget::Default);

    let o = parse_args(&args(&[r"/select,C:\a b\c,d.txt"]), None);
    assert_eq!(strs(&o.select), [r"C:\a b\c,d.txt"]);
    assert!(o.paths.is_empty());

    let o = parse_args(&args(&["/n,/e,/select,C:\\x\\y.txt"]), None);
    assert_eq!(o.target, OpenTarget::NewWindow);
    assert_eq!(strs(&o.select), [r"C:\x\y.txt"]);

    let o = parse_args(&args(&["/e,", r"C:\Users"]), None);
    assert_eq!(strs(&o.paths), [r"C:\Users"]);

    let o = parse_args(&args(&["/root,\"C:\\Program Files\""]), None);
    assert_eq!(strs(&o.paths), [r"C:\Program Files"]);

    let o = parse_args(&args(&["/SELECT,", "/E,C:\\"]), None);
    assert!(o.select.is_empty(), "empty /select, ignored");

    let o = parse_args(&args(&["/select,", r"C:\a b\c.txt"]), None);
    assert_eq!(strs(&o.select), [r"C:\a b\c.txt"]);
    assert!(o.paths.is_empty());

    let o = parse_args(&args(&["shell:Downloads", "::{20D04FE0-3AEA-1069-A2D8-08002B30309D}"]), None);
    assert_eq!(strs(&o.paths), ["shell:Downloads", "::{20D04FE0-3AEA-1069-A2D8-08002B30309D}"]);

    let o = parse_args(&args(&["/idlist,:1234:5678", r"C:\"]), None);
    assert_eq!(strs(&o.paths), [r"C:\"]);

    let o = parse_args(&args(&["--new-window", "/home/user", "--current-tab"]), Some(b"/".to_vec()));
    assert_eq!(strs(&o.paths), ["/home/user"], "Linux paths are not switches");
    assert_eq!(o.target, OpenTarget::CurrentTab);
}

#[test]
fn tag_argument_takes_a_name_and_files() {
    let o = parse_args(&args(&["-Tag", "\"Long name\"", "a.txt", r"C:\b c.txt", "/select,x"]), Some(b"C:\\w".to_vec()));
    assert_eq!(o.tag.as_deref(), Some("Long name"));
    assert_eq!(strs(&o.paths), ["a.txt", r"C:\b c.txt", "/select,x"]);
    assert!(o.select.is_empty() && o.cwd.is_some());
    assert_eq!(parse_args(&args(&["--new-tab", "-TAG"]), None).tag.as_deref(), Some(""));
    assert_eq!(parse_args(&args(&["tag", "-Tagged"]), None).tag, None);
}

#[test]
fn resolution() {
    let none = |_: &str| None;
    let shell = |n: &str| (n.eq_ignore_ascii_case("shell:Downloads")).then(|| Location::parse(r"C:\Users\u\Downloads").unwrap());
    let cwd = Some(&br"D:\work"[..]);
    let r = |s: &str| resolve(s.as_bytes(), cwd, &shell);
    assert_eq!(r(r"C:\x").unwrap(), Location::parse(r"C:\x").unwrap());
    assert_eq!(r(r"src\lib").unwrap(), Location::parse(r"D:\work\src\lib").unwrap());
    assert_eq!(r(r"..\other").unwrap(), Location::parse(r"D:\other").unwrap());
    assert_eq!(r("::{20d04fe0-3aea-1069-a2d8-08002b30309d}").unwrap(), Location::computer());
    assert_eq!(r("shell:downloads").unwrap(), Location::parse(r"C:\Users\u\Downloads").unwrap());
    assert_eq!(r("shell:RecycleBinFolder").unwrap(), Location::recycle_bin());
    assert_eq!(r("::{645ff040-5081-101b-9f08-00aa002f954e}").unwrap(), Location::recycle_bin());
    assert!(matches!(r("shell:Fonts"), Err(ResolveError::UnknownShellLocation(_))));
    assert_eq!(r("file:///C:/My%20Files/x").unwrap(), Location::parse(r"C:\My Files\x").unwrap());
    assert!(r("file:///C:/Docs/%00x").is_err());
    assert_eq!(r(r"\\wsl$\Ubuntu\etc").unwrap(), Location::parse("wsl://Ubuntu/etc").unwrap());
    assert!(resolve(b"relative", None, &none).is_err());
}

#[test]
fn multibyte_paths_do_not_split_chars() {
    let none = |_: &str| None;
    for p in [r"C:\Пользователи", r"D:\Документы", r"C:\日本語", "shelЖ", "fileЖ:///"] {
        let expected = Location::parse(p).or_else(|| Location::parse(&format!(r"C:\{p}"))).unwrap();
        assert_eq!(resolve(p.as_bytes(), Some(br"C:\"), &none).unwrap(), expected);
    }
    let shell = |n: &str| (n == "shell:Загрузки").then(Location::computer);
    assert_eq!(resolve("shell:Загрузки".as_bytes(), None, &shell).unwrap(), Location::computer());
    assert!(resolve("filé:///x".as_bytes(), None, &none).is_err());
}

#[test]
fn windows_path_forms() {
    let none = |_: &str| None;
    let at = |cwd: &str, s: &str| resolve(s.as_bytes(), Some(cwd.as_bytes()), &none);
    let r = |s: &str| at(r"D:\work", s);
    assert_eq!(r(r"\Windows").unwrap(), Location::parse(r"D:\Windows").unwrap());
    assert_eq!(r("/Windows/System32").unwrap(), Location::parse(r"D:\Windows\System32").unwrap());
    assert_eq!(r("C:foo").unwrap(), Location::parse(r"C:\foo").unwrap());
    assert_eq!(r(r"d:foo\bar").unwrap(), Location::parse(r"D:\work\foo\bar").unwrap());
    assert_eq!(r("D:").unwrap(), Location::parse(r"D:\").unwrap());
    assert_eq!(at(r"\\srv\share\a\b", r"\x").unwrap(), Location::parse(r"\\srv\share\x").unwrap());
    assert_eq!(at(r"\\wsl.localhost\Ubuntu\home", "/etc").unwrap(), Location::parse("wsl://Ubuntu/etc").unwrap());
    assert_eq!(at("/home/u", "/tmp").unwrap(), Location::parse("/tmp").unwrap());
    assert_eq!(resolve(b"/tmp", None, &none).unwrap(), Location::parse("/tmp").unwrap());
    assert_eq!(r("file://server/share/My%20Dir/x").unwrap(), Location::parse(r"\\server\share\My Dir\x").unwrap());
    assert_eq!(r("file://localhost/C:/x").unwrap(), Location::parse(r"C:\x").unwrap());
    assert_eq!(r("FILE:///C:/x").unwrap(), Location::parse(r"C:\x").unwrap());
    assert_eq!(r("file:///tmp/x").unwrap(), Location::parse(r"D:\tmp\x").unwrap());
    assert!(matches!(r("file://server"), Err(ResolveError::Invalid(_))));
    assert!(matches!(r("file://server/"), Err(ResolveError::Invalid(_))));
    assert!(matches!(r("file://"), Err(ResolveError::Invalid(_))));
    assert!(matches!(r(r"\\server"), Err(ResolveError::Invalid(_))));
}

#[test]
fn select_routes_to_parent() {
    let o = parse_args(&args(&[r"/select,C:\dir\file.txt", r"D:\"]), None);
    let (routes, failed) = routes(&o, &|_: &str| None);
    assert!(failed.is_empty());
    assert_eq!(routes[0], Route { folder: Location::parse(r"D:\").unwrap(), select: None });
    assert_eq!(routes[1], Route { folder: Location::parse(r"C:\dir").unwrap(), select: Some(b"file.txt".to_vec()) });
}

#[test]
fn open_serializes() {
    let o = Open { paths: vec![b"C:\\x".to_vec(), vec![0xED, 0xA0, 0x80]], select: vec![b"/tmp/a:b".to_vec()], target: OpenTarget::NewTab, cwd: Some(b"C:\\".to_vec()), tag: Some("Work".into()) };
    let bytes = postcard::to_allocvec(&o).unwrap();
    assert_eq!(postcard::from_bytes::<Open>(&bytes).unwrap(), o);
}
