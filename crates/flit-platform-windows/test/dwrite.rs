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
