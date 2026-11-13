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

#[test]
fn lineage_walks_subclasses_and_text_falls_back_to_plain_text_but_never_to_octet_stream() {
    let mut map = HashMap::new();
    parse_subclasses("application/x-shellscript application/x-executable\napplication/x-shellscript text/plain\ntext/x-rust text/x-csrc\napplication/x-executable application/octet-stream\n", &mut map);
    let parents = |m: &str| map.get(m).cloned().unwrap_or_default();
    assert_eq!(lineage("application/x-shellscript", parents), ["application/x-shellscript", "application/x-executable", "text/plain"]);
    assert_eq!(lineage("text/x-rust", parents), ["text/x-rust", "text/x-csrc", "text/plain"]);
    assert_eq!(lineage("image/png", parents), ["image/png"]);
}

#[test]
fn sniffing_tells_text_from_binary_even_when_a_character_is_cut_at_the_end() {
    assert_eq!(sniff(b"all: build\n"), "text/plain");
    assert_eq!(sniff(&"привет".as_bytes()[..5]), "text/plain");
    assert_eq!(sniff(b""), "text/plain");
    assert_eq!(sniff(b"\x7fELF\x02\x01\x01\0"), OCTET_STREAM);
    assert_eq!(sniff(b"\xff\xfe\xfa"), OCTET_STREAM);
}
