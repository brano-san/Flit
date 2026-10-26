use super::*;

#[test]
fn globs_pick_the_heaviest_simple_extension() {
    let mut map = HashMap::new();
    parse_globs("# comment\n50:text/plain:*.txt\n80:text/x-rust:*.rs\n40:text/x-other:*.rs\n50:text/x-readme:README\n50:x/y:*.[ch]\n60:image/jpeg:*.JPG\n", &mut map);
    assert_eq!(map.get("rs").map(|v| v.1.as_str()), Some("text/x-rust"));
    assert_eq!(map.get("jpg").map(|v| v.1.as_str()), Some("image/jpeg"));
    assert!(!map.contains_key("README") && !map.contains_key("[ch]"));
}

#[test]
fn the_comment_without_a_language_names_the_type() {
    let xml = r#"<mime-type type="text/x-rust"><comment xml:lang="de">Rust-Quelltext</comment><comment>Rust source &amp; code</comment></mime-type>"#;
    assert_eq!(parse_comment(xml).as_deref(), Some("Rust source & code"));
    assert_eq!(parse_comment("<mime-type/>"), None);
}

#[test]
fn installed_types_have_names_when_shared_mime_info_is_present() {
    if !std::path::Path::new("/usr/share/mime/globs2").exists() {
        return;
    }
    assert_eq!(type_name(b"PNG").as_deref(), Some("PNG image"));
    assert_eq!(type_name(b"no-such-extension-here"), None);
}
