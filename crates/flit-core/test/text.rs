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
        *out = GlyphBitmap { left: 1, top: 7, width: 6, height: 8, data: vec![255; 48], color: false };
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

struct Emoji;

impl TextBackend for Emoji {
    fn shape(&mut self, text: &str, s: FontStyle, out: &mut ShapedText) {
        Mono.shape(text, s, out)
    }
    fn rasterize(&mut self, key: GlyphKey, out: &mut GlyphBitmap) -> bool {
        match key.id {
            0x263A => *out = GlyphBitmap { left: 0, top: 8, width: 2, height: 2, data: [10, 20, 30, 255].repeat(4), color: true },
            0x2639 => *out = GlyphBitmap { left: 0, top: 8, width: 2, height: 2, data: vec![255; 4], color: true },
            _ => *out = GlyphBitmap { left: 0, top: 8, width: 2, height: 2, data: vec![200; 4], color: false },
        }
        true
    }
    fn metrics(&mut self, _: FontStyle) -> FontMetrics {
        FontMetrics { ascent: 8.0, descent: 2.0 }
    }
}

#[test]
fn colour_glyphs_keep_their_colours_and_follow_only_the_text_alpha() {
    let mut g = Gfx::new(Emoji, 256);
    g.begin(Color::default());
    g.text(0.0, 10.0, "a\u{263A}", style(), Color::rgb(0x112233).alpha(0x80), 1000.0);
    let kinds: Vec<u32> = g.dl.instances.iter().map(|i| i.kind).collect();
    assert_eq!(kinds, [crate::render::kind::GLYPH, crate::render::kind::IMAGE]);
    assert_eq!(g.dl.instances[1].color >> 24, 0x80, "the emoji fades with the text but is not tinted");
    let emoji_upload = &g.uploads.data[g.uploads.rects[1].1..g.uploads.rects[1].1 + 16];
    assert_eq!(emoji_upload, [10, 20, 30, 255].repeat(4).as_slice(), "the colour pixels go to the atlas as they are");
    assert_eq!(&g.uploads.data[..4], [200, 200, 200, 200], "a plain glyph is still a tinted mask");
}

#[test]
fn a_colour_glyph_with_short_pixel_data_is_dropped_instead_of_corrupting_the_atlas() {
    let mut g = Gfx::new(Emoji, 256);
    g.begin(Color::default());
    g.text(0.0, 10.0, "\u{2639}b", style(), Color::default(), 1000.0);
    assert_eq!(g.dl.instances.len(), 1, "only the plain glyph is drawn");
    assert_eq!(g.uploads.data.len(), 16);
}
