use super::*;
use flit_core::render::{Color, Rect};
use std::path::PathBuf;
use std::time::Instant;

const SIDE: u32 = 48;
const ATLAS: u32 = 64;

fn read(r: &GlRenderer) -> Vec<u8> {
    let (w, h) = r.size;
    let Target::Offscreen(fbo, _) = r.target else { panic!("offscreen only") };
    let gl = &r.shared.gl;
    let mut px = vec![0u8; (w * h * 4) as usize];
    unsafe {
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
        gl.read_pixels(0, 0, w as i32, h as i32, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelPackData::Slice(Some(&mut px)));
    }
    px.chunks_exact((w * 4) as usize).rev().flatten().copied().collect()
}

fn pixel(px: &[u8], x: u32, y: u32) -> [u8; 4] {
    let i = ((y * SIDE + x) * 4) as usize;
    px[i..i + 4].try_into().unwrap()
}

fn disk(side: u32) -> Vec<u8> {
    let c = side as f32 / 2.0;
    (0..side * side)
        .flat_map(|i| {
            let (x, y) = ((i % side) as f32 + 0.5 - c, (i / side) as f32 + 0.5 - c);
            let a = ((c - 1.0 - (x * x + y * y).sqrt()) + 0.5).clamp(0.0, 1.0);
            [255, 255, 255, (a * 255.0).round() as u8]
        })
        .collect()
}

fn gradient(side: u32, alpha: u8) -> Vec<u8> {
    (0..side * side)
        .flat_map(|i| {
            let (x, y) = (i % side, i / side);
            let pm = |v: u32| (v * 255 / (side - 1) * alpha as u32 / 255) as u8;
            [pm(x), pm(y), pm(side - 1 - x), alpha]
        })
        .collect()
}

fn checker(side: u32) -> Vec<u8> {
    (0..side * side)
        .flat_map(|i| if ((i % side) / 4 + (i / side) / 4).is_multiple_of(2) { [230, 230, 230, 255] } else { [40, 90, 200, 255] })
        .collect()
}

fn uv(slot: Slot) -> [f32; 4] {
    let n = ATLAS as f32;
    [slot.x as f32 / n, slot.y as f32 / n, (slot.x + slot.w) as f32 / n, (slot.y + slot.h) as f32 / n]
}

const GLYPH: Slot = Slot { x: 0, y: 0, w: 16, h: 16 };
const IMAGE: Slot = Slot { x: 20, y: 0, w: 8, h: 8 };
const THUMB: Slot = Slot { x: 0, y: 0, w: 16, h: 16 };

fn scenes() -> Vec<(&'static str, DrawList)> {
    let mut out = Vec::new();
    let mut dl = DrawList::default();
    dl.reset(Color::rgb(0x202020));
    dl.rect(Rect::new(2.0, 2.0, 20.0, 14.0), Color::rgb(0xE04040));
    dl.round_rect(Rect::new(10.0, 8.0, 30.0, 22.0), Color::rgba(0x40C060B0), 8.0);
    dl.outline(Rect::new(6.0, 30.0, 36.0, 14.0), Color::rgb(0xFFFFFF), 6.0, 2.0);
    dl.round_rect(Rect::new(30.5, 2.25, 12.0, 12.0), Color::rgb(0x3080FF), 20.0);
    out.push(("shapes", dl));

    let mut dl = DrawList::default();
    dl.reset(Color::rgb(0xF0F0F0));
    dl.glyph(Rect::new(2.0, 2.0, 16.0, 16.0), uv(GLYPH), Color::rgb(0x101010));
    dl.glyph(Rect::new(20.25, 3.5, 16.0, 16.0), uv(GLYPH), Color::rgba(0xC02020C0));
    dl.glyph(Rect::new(38.0, 4.0, 8.0, 8.0), uv(GLYPH), Color::rgb(0x2050D0));
    dl.image(Rect::new(2.0, 24.0, 16.0, 16.0), uv(IMAGE), 255);
    dl.image(Rect::new(22.0, 24.0, 8.0, 8.0), uv(IMAGE), 128);
    out.push(("glyph_image", dl));

    let mut dl = DrawList::default();
    dl.reset(Color::rgb(0x303030));
    let n = THUMB_ATLAS as f32;
    dl.thumb(Rect::new(2.0, 2.0, 24.0, 24.0), [0.0, 0.0, 16.0 / n, 16.0 / n], 255);
    dl.thumb(Rect::new(28.0, 2.0, 16.0, 16.0), [0.0, 0.0, 16.0 / n, 16.0 / n], 100);
    dl.preview(Rect::new(4.0, 28.0, 40.0, 16.0));
    out.push(("thumb_preview", dl));

    let mut dl = DrawList::default();
    dl.reset(Color::rgb(0xFAFAFA));
    dl.shadow(Rect::new(12.0, 14.0, 24.0, 20.0), Color::rgba(0x00000060), 6.0, 8.0);
    dl.round_rect(Rect::new(12.0, 12.0, 24.0, 20.0), Color::rgb(0xFFFFFF), 6.0);
    dl.shadow(Rect::new(38.0, 38.0, 6.0, 6.0), Color::rgba(0x0000FFFF), 0.0, 2.0);
    out.push(("shadow", dl));

    let mut dl = DrawList::default();
    dl.reset(Color::rgb(0x101828));
    dl.set_clip(Some(Rect::new(8.0, 8.0, 24.0, 20.0)));
    dl.round_rect(Rect::new(0.0, 0.0, 40.0, 40.0), Color::rgb(0xF0A020), 10.0);
    dl.glyph(Rect::new(24.0, 18.0, 16.0, 16.0), uv(GLYPH), Color::rgb(0x2020FF));
    dl.set_clip(None);
    dl.rect(Rect::new(36.0, 36.0, 10.0, 10.0), Color::rgb(0x20FF20));
    out.push(("clip", dl));
    out
}

fn reference(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test").join("gl").join(format!("{name}.png"))
}

fn save(path: &PathBuf, px: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut enc = png::Encoder::new(std::fs::File::create(path).unwrap(), SIDE, SIDE);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(px).unwrap();
}

fn load(path: &PathBuf) -> Vec<u8> {
    let file = std::io::BufReader::new(std::fs::File::open(path).unwrap_or_else(|e| panic!("{}: {e}", path.display())));
    let mut reader = png::Decoder::new(file).read_info().unwrap();
    let mut px = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut px).unwrap();
    assert_eq!((info.width, info.height, info.color_type), (SIDE, SIDE, png::ColorType::Rgba), "{}", path.display());
    px
}

fn mismatches(a: &[u8], b: &[u8]) -> usize {
    a.chunks_exact(4).zip(b.chunks_exact(4)).filter(|(p, q)| p.iter().zip(q.iter()).any(|(x, y)| x.abs_diff(*y) > 3)).count()
}

fn renderer(dev: &GlDevice, side: (u32, u32)) -> GlRenderer {
    let mut r = GlRenderer::offscreen(dev, side, ATLAS).unwrap();
    r.upload(GLYPH, &disk(16));
    r.upload(IMAGE, &gradient(8, 255));
    r.upload_thumb(THUMB, &checker(16));
    r.upload_preview(Some(&Bitmap { width: 4, height: 2, rgba: gradient(4, 200)[..32].to_vec() }));
    r
}

#[test]
fn draws_like_the_reference_images() {
    let dev = GlDevice::headless().unwrap();
    let mut r = renderer(&dev, (SIDE, SIDE));
    let bless = std::env::var_os("FLIT_BLESS").is_some();
    let mut failures = Vec::new();
    for (name, dl) in scenes() {
        r.render(&dl).unwrap();
        let px = read(&r);
        let path = reference(name);
        if bless {
            save(&path, &px);
            continue;
        }
        let bad = mismatches(&px, &load(&path));
        if bad > (SIDE * SIDE / 200) as usize {
            save(&std::env::temp_dir().join(format!("flit-gl-{name}.png")), &px);
            failures.push(format!("{name}: {bad} pixels differ"));
        }
    }
    assert!(failures.is_empty(), "{failures:?}");
}

#[test]
fn matches_the_shader_math_at_known_pixels() {
    let dev = GlDevice::headless().unwrap();
    let mut r = renderer(&dev, (SIDE, SIDE));
    let scenes = scenes();
    let at = |r: &mut GlRenderer, name: &str| {
        r.render(&scenes.iter().find(|s| s.0 == name).unwrap().1).unwrap();
        read(r)
    };
    let shapes = at(&mut r, "shapes");
    assert_eq!(pixel(&shapes, 4, 4), [0xE0, 0x40, 0x40, 0xFF]);
    assert_eq!(pixel(&shapes, 46, 46), [0x20, 0x20, 0x20, 0xFF]);
    let a = 0xB0 as f32 / 255.0;
    let blended = |src: f32, dst: f32| ((src / 255.0 * a + dst / 255.0 * (1.0 - a)) * 255.0).round() as i32;
    let p = pixel(&shapes, 25, 25);
    for (got, want) in p.iter().zip([blended(64.0, 32.0), blended(192.0, 32.0), blended(96.0, 32.0)]) {
        assert!((*got as i32 - want).abs() <= 1, "{p:?}");
    }
    assert_eq!(pixel(&shapes, 20, 37), [0x20, 0x20, 0x20, 0xFF]);
    assert_eq!(pixel(&shapes, 6, 37), [0xFF, 0xFF, 0xFF, 0xFF]);

    let clip = at(&mut r, "clip");
    assert_eq!(pixel(&clip, 4, 20), [0x10, 0x18, 0x28, 0xFF]);
    assert_eq!(pixel(&clip, 20, 20), [0xF0, 0xA0, 0x20, 0xFF]);
    assert_eq!(pixel(&clip, 32, 26), [0x10, 0x18, 0x28, 0xFF]);
    assert_eq!(pixel(&clip, 40, 40), [0x20, 0xFF, 0x20, 0xFF]);

    let glyph = at(&mut r, "glyph_image");
    assert_eq!(pixel(&glyph, 10, 10), [0x10, 0x10, 0x10, 0xFF]);
    assert_eq!(pixel(&glyph, 1, 1), [0xF0, 0xF0, 0xF0, 0xFF]);
}

#[test]
#[ignore]
fn presents_to_an_x11_window() {
    use raw_window_handle::{XlibDisplayHandle, XlibWindowHandle};
    let xlib = x11_dl::xlib::Xlib::open().unwrap();
    unsafe {
        let display = (xlib.XOpenDisplay)(std::ptr::null());
        assert!(!display.is_null(), "no X display");
        let screen = (xlib.XDefaultScreen)(display);
        let window = (xlib.XCreateSimpleWindow)(display, (xlib.XRootWindow)(display, screen), 0, 0, 160, 120, 0, 0, 0);
        (xlib.XMapWindow)(display, window);
        (xlib.XFlush)(display);
        let dev = GlDevice::new(RawDisplayHandle::Xlib(XlibDisplayHandle::new(std::ptr::NonNull::new(display.cast()), screen))).unwrap();
        let mut r = GlRenderer::new(&dev, RawWindowHandle::Xlib(XlibWindowHandle::new(window)), (160, 120), ATLAS).unwrap();
        r.set_swap_interval(1).unwrap();
        let dl = scenes().into_iter().find(|s| s.0 == "shapes").unwrap().1;
        let mut times = Vec::new();
        for _ in 0..60 {
            let t = Instant::now();
            r.render(&dl).unwrap();
            times.push(t.elapsed().as_secs_f64() * 1e3);
        }
        r.resize(200, 150);
        r.render(&dl).unwrap();
        println!("{}: 60 presented frames, render() with swap p50 {:.3} ms", r.shared.gl.get_parameter_string(glow::RENDERER), percentile(&mut times, 0.5));
        drop(r);
        drop(dev);
        (xlib.XDestroyWindow)(display, window);
        (xlib.XCloseDisplay)(display);
    }
}

fn listing(rows: usize, width: f32) -> DrawList {
    let mut dl = DrawList::default();
    dl.reset(Color::rgb(0x1C1C1C));
    let n = THUMB_ATLAS as f32;
    for i in 0..rows {
        let y = (i % 44) as f32 * 24.0 + 40.0;
        let x = (i / 44) as f32 * 3.0;
        dl.set_clip(Some(Rect::new(0.0, 40.0, width, 1040.0)));
        if i.is_multiple_of(2) {
            dl.rect(Rect::new(x, y, width, 24.0), Color::rgb(0x242424));
        }
        if i.is_multiple_of(7) {
            dl.round_rect(Rect::new(x + 4.0, y + 1.0, width - 8.0, 22.0), Color::rgba(0x3060A0C0), 4.0);
        }
        dl.thumb(Rect::new(x + 8.0, y + 4.0, 16.0, 16.0), [0.0, 0.0, 16.0 / n, 16.0 / n], 255);
        for (start, count) in [(32.0, 40), (900.0, 10), (1100.0, 16)] {
            for g in 0..count {
                dl.glyph(Rect::new(x + start + g as f32 * 7.5, y + 6.0, 8.0, 12.0), uv(GLYPH), Color::rgb(0xE6E6E6));
            }
        }
    }
    dl
}

fn percentile(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(f64::total_cmp);
    v[((v.len() - 1) as f64 * p).round() as usize]
}

#[test]
#[ignore]
fn measures_draw_cpu_time_for_a_listing() {
    let dev = GlDevice::headless().unwrap();
    let mut r = renderer(&dev, (1920, 1080));
    let dl = listing(147, 1900.0);
    let renderer_name = unsafe { r.shared.gl.get_parameter_string(glow::RENDERER) };
    let (mut cpu, mut total) = (Vec::new(), Vec::new());
    for frame in 0..320 {
        let t = Instant::now();
        r.render(&dl).unwrap();
        let cpu_ms = t.elapsed().as_secs_f64() * 1e3;
        unsafe { r.shared.gl.finish() };
        if frame >= 20 {
            cpu.push(cpu_ms);
            total.push(t.elapsed().as_secs_f64() * 1e3);
        }
    }
    println!(
        "{renderer_name}: {} instances, render() CPU p50 {:.3} ms p99 {:.3} ms, with glFinish p50 {:.3} ms p99 {:.3} ms",
        dl.instances.len(),
        percentile(&mut cpu, 0.5),
        percentile(&mut cpu, 0.99),
        percentile(&mut total, 0.5),
        percentile(&mut total, 0.99)
    );
}
