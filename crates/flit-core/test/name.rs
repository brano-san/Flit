use super::*;

fn rt(u: &[u16]) {
    let mut w = Vec::new();
    push_utf16(&mut w, u);
    let mut back = Vec::new();
    to_utf16(&w, &mut back);
    assert_eq!(back, u, "roundtrip of {u:x?}");
}

#[test]
fn utf16_roundtrip() {
    rt(&[]);
    rt(&"hello.txt".encode_utf16().collect::<Vec<_>>());
    rt(&"Привет мир 日本 🎉".encode_utf16().collect::<Vec<_>>());
    rt(&[0xD800]);
    rt(&[0xDC00, b'a' as u16]);
    rt(&[0xD83D, 0xD83D, 0xDE00]);
    rt(&[b'x' as u16, 0xDFFF]);
}

#[test]
fn valid_utf16_is_utf8() {
    let s = "Привет 🎉.txt";
    let mut w = Vec::new();
    push_utf16(&mut w, &s.encode_utf16().collect::<Vec<_>>());
    assert_eq!(w, s.as_bytes());
    assert!(matches!(display(&w), Cow::Borrowed(_)));
}

#[test]
fn display_lossy() {
    let mut w = b"a".to_vec();
    push_utf16(&mut w, &[0xD800]);
    w.push(b'b');
    assert_eq!(display(&w), "a\u{FFFD}b");
    assert_eq!(display(b"x\xFFy"), "x\u{FFFD}y");
}

#[test]
fn extension() {
    assert_eq!(ext_len(b"a.txt"), 3);
    assert_eq!(ext_len(b"archive.tar.gz"), 2);
    assert_eq!(ext_len(b".gitignore"), 9);
    assert_eq!(ext_len(b"noext"), 0);
    assert_eq!(ext_len(b"trailing."), 0);
}

#[test]
fn type_ahead_prefix() {
    assert!(starts_with_ci("Документы".as_bytes(), "док"));
    assert!(starts_with_ci(b"README.md", "read"));
    assert!(!starts_with_ci(b"RE", "read"));
}
