use super::*;

fn p(s: &str) -> FsPath {
    FsPath::parse(s).unwrap_or_else(|| panic!("parse {s}"))
}

#[test]
fn drives() {
    let a = p("c:/Users//user/./x/..");
    assert_eq!(a.as_bytes(), br"C:\Users\user");
    assert_eq!(a.kind(), PathKind::Drive);
    assert_eq!(p("C:").as_bytes(), br"C:\");
    assert!(p(r"C:\").is_root());
    assert_eq!(p(r"C:\..\..").as_bytes(), br"C:\");
    assert_eq!(p(r#" "D:\Games" "#).as_bytes(), br"D:\Games");
}

#[test]
fn verbatim_and_unc() {
    assert_eq!(p(r"\\?\C:\Windows").as_bytes(), br"C:\Windows");
    let u = p(r"\\server\share\dir\f.txt");
    assert_eq!(u.kind(), PathKind::Unc);
    assert_eq!(u.as_bytes(), br"\\server\share\dir\f.txt");
    assert_eq!(p(r"\\?\UNC\server\share\dir"), p(r"\\server\share\dir"));
    assert!(FsPath::parse(r"\\server").is_none());
    let v = p(r"\\?\Volume{1234}\dir");
    assert_eq!(v.kind(), PathKind::Verbatim);
    assert_eq!(v.parent().unwrap().as_bytes(), br"\\?\Volume{1234}\");
    assert!(FsPath::parse("relative\\path").is_none());
    assert!(FsPath::parse_bytes(b"C:\\dir\\a\0.txt").is_none());
    assert!(FsPath::parse_bytes(b"/tmp/\0x").is_none());
}

#[test]
fn unix() {
    let u = p("/home//user/./docs/../x");
    assert_eq!(u.as_bytes(), b"/home/user/x");
    assert_eq!(u.parent().unwrap().as_bytes(), b"/home/user");
    assert_eq!(p("/").parent(), None);
    assert_eq!(p("/home").parent().unwrap().as_bytes(), b"/");
    assert_eq!(p("/").join(b"etc").as_bytes(), b"/etc");
}

#[test]
fn parent_join_name() {
    let a = p(r"C:\Users\user");
    assert_eq!(a.file_name(), b"user");
    assert_eq!(a.parent().unwrap().as_bytes(), br"C:\Users");
    assert_eq!(p(r"C:\Users").parent().unwrap().as_bytes(), br"C:\");
    assert_eq!(p(r"C:\").parent().unwrap(), FsPath::computer());
    assert_eq!(FsPath::computer().parent(), None);
    assert_eq!(FsPath::computer().join(b"D:"), p(r"D:\"));
    assert_eq!(p(r"C:\").join(b"x").as_bytes(), br"C:\x");
    assert_eq!(p(r"C:\").file_name(), b"C:");
    assert_eq!(p(r"\\srv\sh").file_name(), br"\\srv\sh");
}

#[test]
fn crumbs() {
    let c: Vec<_> = p(r"C:\Users\b").crumbs().into_iter().map(|(l, _)| l).collect();
    assert_eq!(c, ["This PC", "C:", "Users", "b"]);
    let c: Vec<_> = p("/usr/lib").crumbs().into_iter().map(|(l, _)| l).collect();
    assert_eq!(c, ["/", "usr", "lib"]);
}

#[test]
fn wide() {
    let w = |s: &str, v| String::from_utf16(&p(s).to_wide(v)[..]).unwrap();
    assert_eq!(w(r"C:\a", true), "\\\\?\\C:\\a\0");
    assert_eq!(w(r"C:\a", false), "C:\\a\0");
    assert_eq!(w(r"\\srv\sh\a", true), "\\\\?\\UNC\\srv\\sh\\a\0");
}

#[test]
fn invalid_utf16_component() {
    let mut w: Vec<u16> = r"C:\bad".encode_utf16().collect();
    w.push(0xD800);
    let path = FsPath::from_utf16(&w).unwrap();
    let back = path.to_wide(false);
    assert_eq!(&back[..back.len() - 1], &w[..]);
    assert_eq!(path.display(), "C:\\bad\u{FFFD}");
}

#[test]
fn long_path() {
    let long = format!(r"C:\{}", ["segment_name_long"; 30].join("\\"));
    let path = p(&long);
    assert!(path.as_bytes().len() > 260);
    assert!(path.to_wide(true).starts_with(&r"\\?\".encode_utf16().collect::<Vec<_>>()));
}

#[test]
fn oversized_roots_are_rejected() {
    let server = "s".repeat(65533);
    assert!(FsPath::parse(&format!(r"\\{server}\share\dir")).is_none());
    assert!(FsPath::parse(&format!(r"\\?\{server}{server}\dir")).is_none());
    let ok = p(&format!(r"\\{}\share\dir", "s".repeat(1000)));
    assert_eq!(ok.file_name(), b"dir");
    assert_eq!(ok.unc_parts().unwrap().0.len(), 1000);
}
