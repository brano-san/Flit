use super::*;

#[test]
fn font_registry_values_are_named_after_the_file_and_its_format() {
    assert_eq!(font_value_name(b"Inter-Bold.ttf").as_deref(), Some("Inter-Bold (TrueType)"));
    assert_eq!(font_value_name(b"Cambria.TTC").as_deref(), Some("Cambria (TrueType)"));
    assert_eq!(font_value_name(b"Source Sans 3.otf").as_deref(), Some("Source Sans 3 (OpenType)"));
    assert_eq!(font_value_name("Шрифт.ttf".as_bytes()).as_deref(), Some("Шрифт (TrueType)"));
    for not_font in [&b"readme.txt"[..], b"ttf", b".ttf", b"font.woff2", b""] {
        assert!(!is_font(not_font), "{}", String::from_utf8_lossy(not_font));
    }
}
