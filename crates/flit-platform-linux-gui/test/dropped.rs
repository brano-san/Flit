use super::*;
use crate::temp::Temp;

#[test]
fn only_web_addresses_count_as_dropped_links() {
    let list = b"# from a browser\r\nhttps://example.org/a%20b/page.html?x=1\r\nfile:///tmp/a\r\nHTTP://Example.org\r\nmailto:x@y\r\nhttp://\r\n";
    assert_eq!(web_urls(list), ["https://example.org/a%20b/page.html?x=1", "HTTP://Example.org"]);
    assert!(web_urls(b"file:///home/u/x.txt\r\n").is_empty(), "files are dropped as files");
}

#[test]
fn a_dropped_link_is_named_after_its_site_and_page() {
    assert_eq!(url_stem("https://example.org/docs/a%20b.html?x=1#top").as_deref(), Some("example.org a b.html"));
    assert_eq!(url_stem("https://user@example.org/").as_deref(), Some("example.org"));
    assert_eq!(url_stem("http://example.org/a/b/").as_deref(), Some("example.org b"));
    assert_eq!(url_stem("https://x.org/a%2F..%2Fb").as_deref(), Some("x.org a .. b"), "slashes never reach the file name");
}

#[test]
fn dropped_links_text_and_pictures_become_new_files_without_replacing_any() {
    let t = Temp::new("dropped");
    let dir = t.path();
    let link = save(dir, &Content::Url("https://example.org/page".into()), "Dropped text", "Dropped image").unwrap();
    assert_eq!(link, dir.join("example.org page.desktop"));
    let body = std::fs::read_to_string(&link).unwrap();
    assert!(body.starts_with("[Desktop Entry]\nType=Link\n") && body.contains("\nURL=https://example.org/page\n"), "{body}");
    let first = save(dir, &Content::Text("hello".into()), "Dropped text", "Dropped image").unwrap();
    let second = save(dir, &Content::Text("again".into()), "Dropped text", "Dropped image").unwrap();
    assert_eq!((first.file_name().unwrap(), second.file_name().unwrap()), ("Dropped text.txt".as_ref(), "Dropped text (2).txt".as_ref()));
    assert_eq!(std::fs::read_to_string(&first).unwrap(), "hello", "the first file is not overwritten");
    let png = save(dir, &Content::Image(vec![0x89, b'P', b'N', b'G']), "Dropped text", "Dropped image").unwrap();
    assert_eq!(png, dir.join("Dropped image.png"));
    assert_eq!(std::fs::read(&png).unwrap(), [0x89, b'P', b'N', b'G']);
}

#[test]
fn a_link_with_a_newline_cannot_add_keys_to_the_launcher() {
    let body = link_file("https://x.org/\nExec=rm -rf ~", "x\nExec=evil");
    assert_eq!(body.lines().filter(|l| l.starts_with("Exec=")).count(), 0, "{body}");
}

#[test]
fn files_win_over_links_pictures_and_text_offered_with_them() {
    let browser = [TypeHint::Plaintext, PNG, TypeHint::Html, TypeHint::UriList];
    assert_eq!(wanted(&browser), [TypeHint::UriList, PNG, TypeHint::Plaintext]);
    assert_eq!(wanted(&[TypeHint::Html]), Vec::<TypeHint>::new(), "only HTML is not taken");
    assert_eq!(wanted(&[]), [TypeHint::UriList], "unknown types are tried as a file list");
    let p = |s: &str| FsPath::parse(s).unwrap();
    assert_eq!(interpret(Some(TypeHint::UriList), b"file:///tmp/a\r\nhttps://x.org/\r\n"), Some(Dragged::Files(vec![p("/tmp/a")])));
    assert_eq!(interpret(Some(TypeHint::UriList), b"https://x.org/\r\n"), Some(Dragged::Content(Content::Url("https://x.org/".into()))));
    assert_eq!(interpret(Some(TypeHint::UriList), b"mailto:a@b\r\n"), None, "the next offered type is tried");
}

#[test]
fn dropped_text_that_is_only_an_address_becomes_a_link_and_other_text_stays_text() {
    assert_eq!(interpret(Some(TypeHint::Plaintext), b" https://x.org/a \n"), Some(Dragged::Content(Content::Url("https://x.org/a".into()))));
    assert_eq!(interpret(Some(TypeHint::Plaintext), b"see https://x.org/a"), Some(Dragged::Content(Content::Text("see https://x.org/a".into()))));
    assert_eq!(interpret(Some(TypeHint::Plaintext), b"  \n"), None, "blank text makes no file");
    assert_eq!(interpret(Some(PNG), b"\x89PNG"), Some(Dragged::Content(Content::Image(b"\x89PNG".to_vec()))));
    assert_eq!(interpret(Some(PNG), b""), None);
}
