use super::*;

pub struct Mono;

impl TextBackend for Mono {
    fn shape(&mut self, text: &str, _: FontStyle, out: &mut ShapedText) {
        out.glyphs.clear();
        for (i, (b, c)) in text.char_indices().enumerate() {
            out.glyphs.push(ShapedGlyph { font: 0, id: c as u16, x: i as f32 * 10.0, y: 0.0, cluster: b as u32 });
        }
        out.width = out.glyphs.len() as f32 * 10.0;
    }
    fn rasterize(&mut self, key: GlyphKey, out: &mut GlyphBitmap) -> bool {
        if key.id == ' ' as u16 {
            return false;
        }
        *out = GlyphBitmap { left: 1, top: 7, width: 6, height: 8, data: vec![255; 48] };
        true
    }
    fn metrics(&mut self, _: FontStyle) -> FontMetrics {
        FontMetrics { ascent: 8.0, descent: 2.0 }
    }
}

fn style() -> FontStyle {
    FontStyle::new(FontFamily::Ui, 12.0, false)
}

#[test]
fn caches_glyphs_and_ellipsizes() {
    let mut g = Gfx::new(Mono, 256);
    g.begin(Color::default());
    assert_eq!(g.text(0.0, 10.0, "ab a", style(), Color::default(), 1000.0), 40.0);
    assert_eq!(g.dl.instances.len(), 3, "space has no quad");
    assert_eq!(g.uploads.rects.len(), 2, "'a' rasterized once");
    g.begin(Color::default());
    let w = g.text(0.0, 10.0, "abcdefgh", style(), Color::default(), 45.0);
    assert_eq!(w, 40.0);
    assert_eq!(g.dl.instances.len(), 4);
}

#[test]
fn caret_math() {
    let mut g = Gfx::new(Mono, 256);
    assert_eq!(g.caret_x("hello", style(), 0), 0.0);
    assert_eq!(g.caret_x("hello", style(), 2), 20.0);
    assert_eq!(g.caret_x("hello", style(), 5), 50.0);
    assert_eq!(g.hit("hello", style(), 23.0), 2);
    assert_eq!(g.hit("hello", style(), 99.0), 5);
    assert_eq!(g.hit("при", style(), 11.0), 2, "byte offsets of multibyte chars");
}

#[test]
fn atlas_overflow_resets() {
    let mut g = Gfx::new(Mono, 16);
    g.begin(Color::default());
    g.text(0.0, 10.0, "abcdefg", style(), Color::default(), 1e9);
    assert!(g.atlas_reset);
}

#[test]
fn glyph_overflow_forgets_image_slots() {
    let mut g = Gfx::new(Mono, 16);
    g.add_image(7, 4, 4, vec![9; 64]);
    g.begin(Color::default());
    assert!(g.image(7, Rect::new(0.0, 0.0, 4.0, 4.0)));
    g.text(0.0, 10.0, "abcdefg", style(), Color::default(), 1e9);
    assert!(g.atlas_reset);
    g.uploads.clear();
    assert!(g.image(7, Rect::new(0.0, 0.0, 4.0, 4.0)));
    assert_eq!(g.uploads.rects.len(), 1, "image re-uploaded after the glyph overflow");
}
