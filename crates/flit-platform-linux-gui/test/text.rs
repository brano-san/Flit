use super::*;

#[test]
fn built_in_menu_action_icons_are_present_in_the_bundled_font() {
    let face = Face::open(fonts::ICONS, 0, false).unwrap();
    let source = include_str!("../../flit-core/src/ui/menu.rs");
    let missing: Vec<_> = source
        .split("\\u{")
        .skip(1)
        .filter_map(|tail| {
            let code = u32::from_str_radix(tail.split('}').next()?, 16).ok()?;
            let ch = char::from_u32(code)?;
            (!face.covers(ch)).then_some(format!("U+{code:04X}"))
        })
        .collect();
    assert!(missing.is_empty(), "menu icons missing from FlitIcons.ttf: {missing:?}");
}
use flit_core::render::Color;
use flit_core::text::Gfx;
use std::time::{Duration, Instant};

fn ui(px: f32) -> FontStyle {
    FontStyle::new(FontFamily::Ui, px, false)
}

fn shaped(t: &mut LinuxText, s: &str, style: FontStyle) -> ShapedText {
    let mut out = ShapedText::default();
    t.shape(s, style, &mut out);
    out
}

fn raster(t: &mut LinuxText, g: ShapedGlyph, style: FontStyle, subpx: u8) -> GlyphBitmap {
    let mut bm = GlyphBitmap::default();
    assert!(t.rasterize(GlyphKey { font: g.font, id: g.id, size_q: style.size_q, subpx, subpixel: Default::default() }, &mut bm));
    assert_eq!(bm.data.len(), (bm.width * bm.height) as usize * if bm.color { 4 } else { 1 });
    bm
}

fn ready() -> LinuxText {
    fonts::fonts().wait();
    LinuxText::new()
}

#[test]
fn shapes_latin_and_cyrillic_and_rasterizes() {
    let mut t = ready();
    let style = ui(13.0);
    let s = shaped(&mut t, "Hello, мир", style);
    assert_eq!(s.glyphs.len(), 10, "{s:?}");
    assert!(s.width > 40.0 && s.width < 120.0, "{}", s.width);
    assert!(s.glyphs.windows(2).all(|w| w[0].x < w[1].x));
    assert_eq!(s.glyphs[8].cluster, "Hello, м".len() as u32);
    assert!(s.glyphs.iter().all(|g| g.id != 0 && g.font == s.glyphs[0].font));
    let bm = raster(&mut t, s.glyphs[0], style, 0);
    assert!(bm.width > 2 && bm.height > 5, "{bm:?}");
    assert!(bm.data.iter().any(|&a| a > 200));
    assert!(!bm.color, "ordinary letters stay a mask tinted by the text colour");
    assert!(bm.top > 5 && bm.top <= 13, "{bm:?}");
    let m = t.metrics(style);
    assert!(m.ascent > 8.0 && m.descent > 1.0, "{m:?}");
    assert!(shaped(&mut t, " ", style).width > 2.0);
    let mut space = GlyphBitmap::default();
    let sp = shaped(&mut t, " ", style).glyphs[0];
    assert!(!t.rasterize(GlyphKey { font: sp.font, id: sp.id, size_q: style.size_q, subpx: 0, subpixel: Default::default() }, &mut space) || space.width == 0);
}

#[test]
fn cjk_falls_back_to_another_face() {
    let mut t = ready();
    let style = ui(13.0);
    let latin = shaped(&mut t, "a", style).glyphs[0].font;
    let s = shaped(&mut t, "a文件夹 测试.txt", style);
    assert_eq!(s.glyphs.len(), 11, "{s:?}");
    assert!(s.glyphs.iter().all(|g| g.id != 0), "{s:?}");
    assert_ne!(s.glyphs[1].font, latin);
    assert_eq!(s.glyphs[0].font, latin);
    assert_eq!(s.glyphs[1].cluster, 1);
    assert_eq!(s.glyphs[2].cluster, 4);
    assert!(s.glyphs.windows(2).all(|w| w[0].x < w[1].x));
    let cjk = raster(&mut t, s.glyphs[1], style, 0);
    assert!(cjk.width > 6 && cjk.height > 6, "{cjk:?}");
    let bold = shaped(&mut t, "文", FontStyle::new(FontFamily::Ui, 13.0, true));
    assert_ne!(bold.glyphs[0].id, 0);
}

#[test]
fn hebrew_and_arabic_are_shaped_right_to_left() {
    let mut t = ready();
    let style = ui(14.0);
    let he = shaped(&mut t, "שלום", style);
    assert_eq!(he.glyphs.len(), 4, "{he:?}");
    assert!(he.glyphs.iter().all(|g| g.id != 0));
    assert!(he.glyphs.windows(2).all(|w| w[0].cluster < w[1].cluster && w[0].x > w[1].x), "{he:?}");
    assert!(he.glyphs[3].x >= -0.01 && he.glyphs[0].x < he.width);
    let mixed = shaped(&mut t, "ab שלום 12", style);
    let x_of = |s: &ShapedText, byte: usize| s.glyphs.iter().find(|g| g.cluster as usize == byte).unwrap().x;
    let shin = "ab ".len();
    let mem = "ab של".len() + "ו".len();
    assert!(x_of(&mixed, 0) < x_of(&mixed, 1) && x_of(&mixed, 1) < x_of(&mixed, mem) && x_of(&mixed, mem) < x_of(&mixed, shin), "{mixed:?}");
    let one = "ab שלום ".len();
    assert!(x_of(&mixed, 2) < x_of(&mixed, one) && x_of(&mixed, one) < x_of(&mixed, one + 1) && x_of(&mixed, one + 1) < x_of(&mixed, mem), "{mixed:?}");
    let isolated = shaped(&mut t, "م", style).glyphs[0].id;
    let word = shaped(&mut t, "مرحبا", style);
    assert!(word.glyphs.iter().all(|g| g.id != 0));
    assert_eq!(word.glyphs[0].cluster, 0);
    assert_ne!(word.glyphs[0].id, isolated, "initial form differs from the isolated one");
    assert!(word.glyphs[0].x > word.glyphs[word.glyphs.len() - 1].x);
}

#[test]
fn combining_marks_stay_with_their_base() {
    let mut t = ready();
    let style = ui(16.0);
    let composed = shaped(&mut t, "é", style);
    let decomposed = shaped(&mut t, "e\u{301}", style);
    assert!(decomposed.glyphs.iter().all(|g| g.cluster == 0 && g.id != 0), "{decomposed:?}");
    assert!((decomposed.width - composed.width).abs() < 0.5, "{} vs {}", decomposed.width, composed.width);
    let stacked = shaped(&mut t, "a\u{323}\u{302}b", style);
    let b = stacked.glyphs.iter().find(|g| g.cluster == 5).unwrap();
    assert!(stacked.glyphs.iter().filter(|g| g.cluster < 5).all(|g| g.x < b.x), "{stacked:?}");
    let cyr = shaped(&mut t, "и\u{306}", style);
    assert!(cyr.glyphs.iter().all(|g| g.id != 0 && g.font == cyr.glyphs[0].font), "{cyr:?}");
}

#[test]
fn emoji_are_drawn_in_colour_from_a_colour_font_and_text_stays_a_mask() {
    let mut t = ready();
    let style = ui(16.0);
    let s = shaped(&mut t, "😀", style);
    assert_eq!(s.glyphs.len(), 1);
    if s.glyphs[0].id != 0 {
        let bm = raster(&mut t, s.glyphs[0], style, 0);
        assert!(bm.width > 4 && bm.data.iter().any(|&a| a > 128), "{bm:?}");
        assert_eq!(bm.color, t.faces[s.glyphs[0].font as usize].color, "a colour font gives colour pixels");
        if bm.color {
            assert!(bm.data.chunks_exact(4).any(|p| p[0] != p[1] || p[1] != p[2]), "the emoji has more than one colour");
        }
    }
    let family = shaped(&mut t, "👨\u{200D}👩\u{200D}👧 ok", style);
    assert!(!family.glyphs.is_empty() && family.width > 0.0);
    assert!(family.glyphs.windows(2).all(|w| w[0].cluster <= w[1].cluster));
}

#[test]
fn ellipsis_fits_the_width() {
    let mut g = Gfx::new(ready(), 1024);
    g.begin(Color::default());
    let style = ui(13.0);
    let name = "A very long file name that does not fit the column — Отчёт за квартал.pdf";
    let full = g.measure(name, style);
    let ell = g.measure("…", style);
    assert!(ell > 3.0 && ell < 15.0, "{ell}");
    let before = g.dl.instances.len();
    let w = g.text(0.0, 20.0, name, style, Color::default(), 150.0);
    assert!(w <= 150.0 && w > 150.0 - 12.0, "{w} of {full}");
    let drawn = g.dl.instances.len() - before;
    assert!(drawn > 10 && drawn < name.chars().count(), "{drawn}");
    assert_eq!(g.text(0.0, 40.0, "short", style, Color::default(), 150.0), g.measure("short", style));
    assert_eq!(g.text(0.0, 60.0, name, style, Color::default(), 2.0), ell);
}

#[test]
fn the_interface_font_is_chosen_from_the_installed_families_and_falls_back() {
    let mut t = ready();
    let families = t.families();
    assert!(families.iter().any(|f| f == "DejaVu Sans Mono") && families.iter().any(|f| f == "DejaVu Sans"), "{families:?}");
    assert!(families.windows(2).all(|w| w[0].to_lowercase() <= w[1].to_lowercase()));
    let style = ui(13.0);
    let width = |t: &mut LinuxText| shaped(t, "iiiiiiiiii", style).width;
    let default = width(&mut t);
    assert!(t.set_ui_family("DejaVu Sans Mono"));
    let mono = width(&mut t);
    assert!(mono > default * 1.5, "monospace has wide i: {mono} vs {default}");
    assert!(t.set_ui_family("dejavu sans mono"));
    assert!(!t.set_ui_family("No Such Font Family 42"));
    assert_eq!(width(&mut t), default, "a missing font falls back to the default");
    assert!(t.set_ui_family(""));
    assert_eq!(width(&mut t), default);
    let mut g = Gfx::new(ready(), 256);
    assert!(g.set_ui_family("DejaVu Sans Mono"));
    assert_eq!(g.measure("iiiiiiiiii", style), mono);
    assert!(!g.set_ui_family("No Such Font Family 42"));
    assert_eq!(g.measure("iiiiiiiiii", style), default);
}

fn core_icon_code_points() -> Vec<char> {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../flit-core/src");
    let mut files = vec![src];
    let mut out = Vec::new();
    while let Some(p) = files.pop() {
        if p.is_dir() {
            files.extend(std::fs::read_dir(&p).unwrap().map(|e| e.unwrap().path()));
            continue;
        }
        let source = std::fs::read_to_string(&p).unwrap();
        let mut windows_only = false;
        let mut kept = Vec::new();
        for line in source.lines() {
            let attribute = line.trim_start().starts_with("#[cfg(windows)]");
            if !attribute && !windows_only {
                kept.push(line);
            }
            windows_only = attribute && line.trim_end().ends_with(']');
        }
        let text = kept.join("\n");
        for (i, _) in text.match_indices("\\u{E") {
            let hex: String = text[i + 3..].chars().take_while(|c| c.is_ascii_hexdigit()).collect();
            if hex.len() == 4 {
                out.push(char::from_u32(u32::from_str_radix(&hex, 16).unwrap()).unwrap());
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

#[test]
fn every_icon_the_core_draws_resolves_to_a_centered_glyph() {
    let mut t = LinuxText::new();
    let points = core_icon_code_points();
    assert!(points.len() >= 50, "{points:?}");
    let mapped: Vec<char> = fonts::subset::icon_names().iter().map(|&(cp, _)| char::from_u32(cp).unwrap()).collect();
    assert_eq!(points, mapped, "the bundled icon font and the core agree");
    let style = FontStyle::new(FontFamily::Icons, 16.0, false);
    let m = t.metrics(style);
    assert!((m.ascent + m.descent - 16.0).abs() < 2.5, "{m:?}");
    for c in points {
        let s = shaped(&mut t, &c.to_string(), style);
        assert_eq!(s.glyphs.len(), 1, "{c:?}");
        let g = s.glyphs[0];
        assert!(g.id != 0 && g.font == t.icons, "U+{:04X}", c as u32);
        assert!((s.width - 16.0).abs() < 0.5, "U+{:04X} advance {}", c as u32, s.width);
        let bm = raster(&mut t, g, style, 0);
        assert!(bm.width >= 4 && bm.height >= 1 && bm.data.iter().any(|&a| a > 100), "U+{:04X} {bm:?}", c as u32);
        assert!(bm.top as f32 <= m.ascent + 1.0 && (bm.top - bm.height as i32) as f32 >= -m.descent - 1.0, "U+{:04X} {bm:?} {m:?}", c as u32);
    }
}

#[test]
fn rasterized_glyphs_follow_subpixel_offsets_and_weight() {
    let mut t = ready();
    let style = ui(15.0);
    let g = shaped(&mut t, "H", style).glyphs[0];
    let a = raster(&mut t, g, style, 0);
    let b = raster(&mut t, g, style, 2);
    assert_ne!(a.data, b.data);
    let ink = |bm: &GlyphBitmap| bm.data.iter().map(|&v| v as u32).sum::<u32>();
    let bold = FontStyle::new(FontFamily::Ui, 15.0, true);
    let gb = shaped(&mut t, "H", bold).glyphs[0];
    assert!(ink(&raster(&mut t, gb, bold, 0)) > ink(&a) * 11 / 10);
    let big = raster(&mut t, g, ui(30.0), 0);
    assert!(ink(&big) > ink(&a) * 3);
}

#[test]
fn the_bundled_faces_cover_latin_cyrillic_and_punctuation() {
    let sans = Face::open(fonts::SANS, 0, false).unwrap();
    assert!("Hello, мир — «Ёлка» №5 … €".chars().all(|c| sans.covers(c)));
    assert!(!sans.covers('文'));
    let icons = Face::open(fonts::ICONS, 0, false).unwrap();
    assert!(icons.covers('\u{E8B7}') && !icons.covers('a'));
    let bold = Face::open(fonts::SANS, 0, true).unwrap();
    assert!(bold.embolden);
}

fn names() -> Vec<String> {
    let stems = ["IMG_2024", "Документ", "report-final", "photo", "Отчёт за квартал", "build.log", "数据", "setup", "README", "archive"];
    let exts = ["jpg", "docx", "pdf", "png", "xlsx", "txt", "rs", "zip", "md", "mp4"];
    (0..200).map(|i| format!("{} {:03} ({}).{}", stems[i % 10], i, i * 7 % 13, exts[i / 10 % 10])).collect()
}

fn time<T>(f: impl FnOnce() -> T) -> (T, Duration) {
    let t = Instant::now();
    let r = f();
    (r, t.elapsed())
}

#[test]
#[ignore]
fn bench_first_face() {
    let (mut t, new) = time(LinuxText::new);
    let style = ui(13.0);
    let (s, first) = time(|| shaped(&mut t, "Hello", style));
    assert_eq!(s.glyphs.len(), 5);
    let from = if t.fonts_ready() { "cached" } else { "bundled" };
    let (live, wait) = time(|| fonts::fonts().wait());
    println!("first usable face ({from}): new {new:?} + shape {first:?}; fontconfig ready {wait:?} later (init {:?}, resolve total {:?})", live.init, live.total);
}

#[test]
#[ignore]
fn bench_text() {
    let (live, wait) = time(|| fonts::fonts().wait());
    println!("fontconfig thread: init {:?}, resolve total {:?}, waited {wait:?}", live.init, live.total);
    let names = names();
    let style = ui(13.0);
    let (mut t, new) = time(LinuxText::new);
    let mut out = ShapedText::default();
    let (_, first) = time(|| t.shape("Hello", style, &mut out));
    println!("LinuxText::new {new:?}, first shape {first:?}");
    let (_, cold) = time(|| names.iter().for_each(|n| t.shape(n, style, &mut out)));
    let runs = 20;
    let (_, warm) = time(|| (0..runs).for_each(|_| names.iter().for_each(|n| t.shape(n, style, &mut out))));
    let latin: Vec<String> = names.iter().map(|n| n.chars().filter(char::is_ascii).collect()).collect();
    let (_, ascii) = time(|| (0..runs).for_each(|_| latin.iter().for_each(|n| t.shape(n, style, &mut out))));
    println!("shape 200 names: first pass {cold:?}, then {:?} per 200 (ASCII-only {:?})", warm / runs, ascii / runs);
    let glyphs: Vec<ShapedGlyph> = names.iter().flat_map(|n| shaped(&mut t, n, style).glyphs).collect();
    let mut keys: Vec<GlyphKey> = glyphs.iter().map(|g| GlyphKey { font: g.font, id: g.id, size_q: style.size_q, subpx: (g.x.fract() * 4.0) as u8 & 3, subpixel: Default::default() }).collect();
    keys.sort_by_key(|k| (k.font, k.id, k.subpx));
    keys.dedup();
    let mut bm = GlyphBitmap::default();
    let (_, raster) = time(|| {
        keys.iter().for_each(|&k| {
            t.rasterize(k, &mut bm);
        })
    });
    println!("rasterize {} distinct glyphs: {:?} per glyph", keys.len(), raster / keys.len() as u32);
    let mut g = Gfx::new(LinuxText::new(), 2048);
    g.begin(Color::default());
    let (_, frame) = time(|| {
        names.iter().enumerate().for_each(|(i, n)| {
            g.text(0.0, i as f32 * 20.0, n, style, Color::default(), 260.0);
        })
    });
    g.begin(Color::default());
    let (_, again) = time(|| {
        names.iter().enumerate().for_each(|(i, n)| {
            g.text(0.0, i as f32 * 20.0, n, style, Color::default(), 260.0);
        })
    });
    println!("Gfx::text 200 names, cold atlas {frame:?} ({} uploads), warm {again:?}", g.uploads.rects.len());
    let (families, list) = time(|| t.families());
    println!("families(): {} in {list:?}", families.len());
    let (_, switch) = time(|| t.set_ui_family("DejaVu Sans Mono"));
    println!("set_ui_family {switch:?}");
}

fn square(x0: f32, y0: f32, x1: f32, y1: f32) -> Vec<Command> {
    let v = Vector::new;
    vec![Command::MoveTo(v(x0, y0)), Command::LineTo(v(x0, y1)), Command::LineTo(v(x1, y1)), Command::LineTo(v(x1, y0)), Command::Close]
}

fn texel(bm: &GlyphBitmap, x: i32, y: i32) -> [u8; 4] {
    let (col, row) = ((x - bm.left) as u32, (bm.top - 1 - y) as u32);
    let i = ((row * bm.width + col) * 4) as usize;
    bm.data[i..i + 4].try_into().unwrap()
}

#[test]
fn subpixel_masks_put_each_colour_where_its_stripe_sits() {
    let path = square(2.5, 0.0, 8.5, 6.0);
    let mut rgb = GlyphBitmap::default();
    render_mask(&path, Subpixel::Rgb, Vector::ZERO, &mut rgb);
    assert!(rgb.lcd && !rgb.color);
    assert_eq!(rgb.data.len(), (rgb.width * rgb.height * 4) as usize);
    let [r, g, b, a] = texel(&rgb, 2, 3);
    assert!(r < g && g < b, "on a left edge the blue stripe on the right is covered most: {:?}", [r, g, b]);
    assert_eq!(a, g, "the middle stripe doubles as the plain coverage");
    let [r, g, b, _] = texel(&rgb, 8, 3);
    assert!(r > g && g > b, "on a right edge the red stripe on the left is covered most: {:?}", [r, g, b]);
    let [r, g, b, _] = texel(&rgb, 5, 3);
    assert_eq!([r, g, b], [255, 255, 255], "inside the shape every stripe is lit");

    let mut bgr = GlyphBitmap::default();
    render_mask(&path, Subpixel::Bgr, Vector::ZERO, &mut bgr);
    let ([r1, g1, b1, _], [r2, g2, b2, _]) = (texel(&rgb, 2, 3), texel(&bgr, 2, 3));
    assert_eq!((r1, g1, b1), (b2, g2, r2), "bgr mirrors the red and blue channels");

    let mut vrgb = GlyphBitmap::default();
    render_mask(&square(1.0, 0.0, 7.0, 5.5), Subpixel::Vrgb, Vector::ZERO, &mut vrgb);
    let [r, g, b, _] = texel(&vrgb, 3, 5);
    assert!(r < g && g < b, "on a top edge the red stripe on top is covered least: {:?}", [r, g, b]);
}

#[test]
fn plain_masks_are_one_byte_per_pixel_and_match_the_middle_subpixel_stripe() {
    let path = square(2.5, 0.0, 8.5, 6.0);
    let (mut plain, mut rgb) = (GlyphBitmap::default(), GlyphBitmap::default());
    render_mask(&path, Subpixel::None, Vector::new(0.25, 0.0), &mut plain);
    render_mask(&path, Subpixel::Rgb, Vector::new(0.25, 0.0), &mut rgb);
    assert!(!plain.lcd);
    assert_eq!(plain.data.len(), (plain.width * plain.height) as usize);
    for y in plain.top - plain.height as i32..plain.top {
        for x in plain.left..plain.left + plain.width as i32 {
            let v = plain.data[((plain.top - 1 - y) as u32 * plain.width + (x - plain.left) as u32) as usize];
            let [r, g, b, a] = texel(&rgb, x, y);
            assert_eq!((g, a), (v, v), "at {x},{y}");
            if v == 255 || v == 0 {
                continue;
            }
            assert_ne!(r, b, "an edge pixel differs per channel at {x},{y}");
        }
    }
}

fn bundled_glyph(t: &mut LinuxText, c: char) -> GlyphKey {
    let font = t.load(fonts::SANS, 0, false).unwrap();
    let id = t.faces[font as usize].charmap.map(c as u32);
    GlyphKey { font, id, size_q: 13 * 4, subpx: 0, subpixel: Subpixel::None }
}

fn rasterized(t: &mut LinuxText, key: GlyphKey) -> GlyphBitmap {
    let mut bm = GlyphBitmap::default();
    assert!(t.rasterize(key, &mut bm));
    bm
}

#[test]
fn a_letter_rasterizes_with_three_channels_in_subpixel_mode_and_one_otherwise() {
    let mut t = ready();
    t.set_hinting(Hinting::None);
    let key = bundled_glyph(&mut t, 'l');
    let plain = rasterized(&mut t, key);
    assert!(!plain.lcd && plain.data.len() == (plain.width * plain.height) as usize, "none keeps the one-channel mask: {plain:?}");
    let lcd = rasterized(&mut t, GlyphKey { subpixel: Subpixel::Rgb, ..key });
    assert!(lcd.lcd && lcd.data.len() == (lcd.width * lcd.height * 4) as usize);
    assert!(lcd.data.chunks_exact(4).any(|p| p[0] != p[2]), "the stem edges are coloured: {lcd:?}");
    assert!(lcd.width >= plain.width);
}

#[test]
fn the_subpixel_mode_needs_a_renderer_that_blends_per_channel() {
    let lcd = fonts::Rendering { hinting: Hinting::Slight, subpixel: Subpixel::Bgr };
    assert_eq!(glyph_mode_for(lcd, true).subpixel, Subpixel::Bgr);
    assert_eq!(glyph_mode_for(lcd, false).subpixel, Subpixel::None, "without dual-source blending glyphs stay plain");
    assert_ne!(glyph_mode_for(lcd, true), glyph_mode_for(fonts::Rendering { hinting: Hinting::Full, ..lcd }, true), "a hinting change rebuilds the glyph cache");
}

fn profiles(bm: &GlyphBitmap) -> (Vec<u32>, Vec<u32>) {
    let (w, h) = (bm.width as usize, bm.height as usize);
    let cols = (0..w).map(|x| (0..h).map(|y| u32::from(bm.data[y * w + x])).sum()).collect();
    let rows = (0..h).map(|y| (0..w).map(|x| u32::from(bm.data[y * w + x])).sum()).collect();
    (cols, rows)
}

#[test]
fn slight_hinting_snaps_glyphs_vertically_and_leaves_their_horizontal_shape() {
    let mut t = ready();
    let mut differs = 0;
    for c in ['H', 'x', 'e', 'o', 'n', 'E'] {
        t.set_hinting(Hinting::None);
        let key = bundled_glyph(&mut t, c);
        let plain = rasterized(&mut t, key);
        t.set_hinting(Hinting::Slight);
        let light = rasterized(&mut t, key);
        assert!(!light.lcd);
        assert_eq!((light.left, light.width), (plain.left, plain.width), "{c}: no horizontal move");
        let ((pc, pr), (lc, lr)) = (profiles(&plain), profiles(&light));
        let (pt, lt) = (pc.iter().sum::<u32>() as f32, lc.iter().sum::<u32>() as f32);
        for (x, (a, b)) in pc.iter().zip(&lc).enumerate() {
            assert!((*a as f32 / pt - *b as f32 / lt).abs() < 0.04, "{c}: column {x} keeps its share of ink: {pc:?} {lc:?}");
        }
        if pr != lr || plain.top != light.top || plain.height != light.height {
            differs += 1;
        }
    }
    assert!(differs >= 3, "light hinting changes the vertical profile of most letters ({differs})");
}

#[test]
fn slight_hinting_changes_synthetic_bold_ink_and_preserves_advances() {
    let mut t = ready();
    let regular = t.load(fonts::SANS, 0, false).unwrap();
    let bold = t.load(fonts::SANS, 0, true).unwrap();
    assert!(t.faces[bold as usize].embolden);
    t.ui = [regular, bold];
    let style = FontStyle::new(FontFamily::Ui, 13.0, true);
    t.set_hinting(Hinting::None);
    let before = shaped(&mut t, "Hex", style);
    t.set_hinting(Hinting::Slight);
    let after = shaped(&mut t, "Hex", style);
    assert_eq!(before.width, after.width);
    assert!(before.glyphs.iter().zip(&after.glyphs).all(|(a, b)| a.x == b.x));
    let coverage = |mask: &GlyphBitmap| mask.data.iter().map(|&alpha| u32::from(alpha)).sum::<u32>();
    for (&glyph, letter) in before.glyphs.iter().zip(['H', 'e']) {
        t.set_hinting(Hinting::None);
        let plain = raster(&mut t, glyph, style, 0);
        t.set_hinting(Hinting::Slight);
        let hinted = raster(&mut t, glyph, style, 0);
        assert_ne!(plain.data, hinted.data, "{letter}");
        let regular_ink = rasterized(&mut t, GlyphKey { font: regular, id: glyph.id, size_q: style.size_q, subpx: 0, subpixel: Subpixel::None });
        assert!(coverage(&hinted) > coverage(&regular_ink), "{letter}");
        t.set_hinting(Hinting::None);
        assert_eq!(plain.data, raster(&mut t, glyph, style, 0).data, "{letter}");
    }
}

#[test]
fn synthetic_bold_keeps_the_unhinted_outline_when_a_curve_command_differs() {
    let mut t = ready();
    let font = t.load(fonts::SANS, 0, true).unwrap();
    let key = GlyphKey { font, id: t.faces[font as usize].charmap.map('e' as u32), size_q: 13 * 4, subpx: 0, subpixel: Subpixel::None };
    t.set_hinting(Hinting::Slight);
    assert!(t.swash_path(key, 13.0, false));
    let unhinted = t.path.clone();
    assert!(t.light_path(key, 13.0));
    let curve = t.path.iter_mut().find(|c| matches!(c, Command::QuadTo(..) | Command::CurveTo(..))).unwrap();
    *curve = Command::LineTo(Vector::ZERO);
    assert!(t.swash_path(key, 13.0, true));
    assert_eq!(t.path, unhinted);
    let mut mask = GlyphBitmap::default();
    render_mask(&t.path, Subpixel::None, Vector::ZERO, &mut mask);
    assert!(mask.data.iter().any(|&alpha| alpha > 0));
}
