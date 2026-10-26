use super::*;
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
    assert!(t.rasterize(GlyphKey { font: g.font, id: g.id, size_q: style.size_q, subpx }, &mut bm));
    assert_eq!(bm.data.len(), (bm.width * bm.height) as usize);
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
    assert!(bm.top > 5 && bm.top <= 13, "{bm:?}");
    let m = t.metrics(style);
    assert!(m.ascent > 8.0 && m.descent > 1.0, "{m:?}");
    assert!(shaped(&mut t, " ", style).width > 2.0);
    let mut space = GlyphBitmap::default();
    let sp = shaped(&mut t, " ", style).glyphs[0];
    assert!(!t.rasterize(GlyphKey { font: sp.font, id: sp.id, size_q: style.size_q, subpx: 0 }, &mut space) || space.width == 0);
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
fn emoji_get_a_monochrome_glyph_when_any_font_has_one() {
    let mut t = ready();
    let style = ui(16.0);
    let s = shaped(&mut t, "😀", style);
    assert_eq!(s.glyphs.len(), 1);
    if s.glyphs[0].id != 0 {
        let bm = raster(&mut t, s.glyphs[0], style, 0);
        assert!(bm.width > 4 && bm.data.iter().any(|&a| a > 128), "{bm:?}");
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
        let text = std::fs::read_to_string(&p).unwrap();
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
    assert!(points.len() >= 60, "{points:?}");
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
    let mut keys: Vec<GlyphKey> = glyphs.iter().map(|g| GlyphKey { font: g.font, id: g.id, size_q: style.size_q, subpx: (g.x.fract() * 4.0) as u8 & 3 }).collect();
    keys.sort_by_key(|k| (k.font, k.id, k.subpx));
    keys.dedup();
    let mut bm = GlyphBitmap::default();
    let (_, raster) = time(|| keys.iter().for_each(|&k| { t.rasterize(k, &mut bm); }));
    println!("rasterize {} distinct glyphs: {:?} per glyph", keys.len(), raster / keys.len() as u32);
    let mut g = Gfx::new(LinuxText::new(), 2048);
    g.begin(Color::default());
    let (_, frame) = time(|| names.iter().enumerate().for_each(|(i, n)| { g.text(0.0, i as f32 * 20.0, n, style, Color::default(), 260.0); }));
    g.begin(Color::default());
    let (_, again) = time(|| names.iter().enumerate().for_each(|(i, n)| { g.text(0.0, i as f32 * 20.0, n, style, Color::default(), 260.0); }));
    println!("Gfx::text 200 names, cold atlas {frame:?} ({} uploads), warm {again:?}", g.uploads.rects.len());
    let (families, list) = time(|| t.families());
    println!("families(): {} in {list:?}", families.len());
    let (_, switch) = time(|| t.set_ui_family("DejaVu Sans Mono"));
    println!("set_ui_family {switch:?}");
}
