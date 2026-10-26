use super::*;

#[test]
fn shapes_and_rasterizes() {
    let mut t = DWriteText::new().unwrap();
    let style = FontStyle::new(FontFamily::Ui, 13.0, false);
    let mut s = ShapedText::default();
    t.shape("Hello, мир", style, &mut s);
    assert_eq!(s.glyphs.len(), 10, "{s:?}");
    assert!(s.width > 40.0 && s.width < 120.0, "{}", s.width);
    assert!(s.glyphs.windows(2).all(|w| w[0].x < w[1].x));
    assert_eq!(s.glyphs[8].cluster, "Hello, м".len() as u32);
    let g = s.glyphs[0];
    let mut bm = GlyphBitmap::default();
    assert!(t.rasterize(GlyphKey { font: g.font, id: g.id, size_q: style.size_q, subpx: 0 }, &mut bm));
    assert!(bm.width > 2 && bm.height > 5, "{bm:?}");
    assert!(bm.data.iter().any(|&a| a > 200));
    let m = t.metrics(style);
    assert!(m.ascent > 8.0 && m.descent > 1.0, "{m:?}");
    let mut icons = ShapedText::default();
    t.shape("\u{E72B}", FontStyle::new(FontFamily::Icons, 12.0, false), &mut icons);
    assert_eq!(icons.glyphs.len(), 1);
    assert_ne!(icons.glyphs[0].id, 0);
}

#[test]
fn the_interface_font_is_chosen_from_the_installed_families_and_falls_back() {
    let mut t = DWriteText::new().unwrap();
    let families = t.families();
    assert!(families.iter().any(|f| f == "Consolas") && families.iter().any(|f| f == "Segoe UI"), "{families:?}");
    assert!(families.windows(2).all(|w| w[0].to_lowercase() <= w[1].to_lowercase()));
    let style = FontStyle::new(FontFamily::Ui, 13.0, false);
    let width = |t: &mut DWriteText| {
        let mut s = ShapedText::default();
        t.shape("iiiiiiiiii", style, &mut s);
        s.width
    };
    let default = width(&mut t);
    assert!(t.set_ui_family("Consolas"));
    let mono = width(&mut t);
    assert!(mono > default * 1.5, "Consolas has wide i: {mono} vs {default}");
    assert!(!t.set_ui_family("No Such Font Family 42"));
    assert_eq!(width(&mut t), default, "a missing font falls back to the default");
    assert!(t.set_ui_family(""));
    assert_eq!(width(&mut t), default);
    assert!(t.set_ui_family("Courier New"), "family names with spaces are offered by families() and must be accepted");
    assert!(width(&mut t) > default * 1.5);
    assert!(!t.set_ui_family("Courier New\0Arial"));
}

#[test]
fn reshaping_reuses_font_faces() {
    let mut t = DWriteText::new().unwrap();
    let style = FontStyle::new(FontFamily::Ui, 13.0, false);
    let mut s = ShapedText::default();
    t.shape("Hello, мир 你好", style, &mut s);
    let faces = t.faces.len();
    for _ in 0..50 {
        t.shape("Hello, мир 你好", style, &mut s);
    }
    assert_eq!(t.faces.len(), faces);
}
