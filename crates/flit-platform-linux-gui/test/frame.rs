use super::*;
use flit_core::listing::{DirSnapshot, Listing, Meta};
use flit_core::open::Route;
use flit_core::provider::ProviderRequest;
use flit_core::sort::{self, SortKeys};
use flit_core::text::Gfx;
use flit_core::ui::{App, Effect, Persisted};
use std::time::{Duration, Instant};

const SIZE: (u32, u32) = (900, 600);
const ATLAS: u32 = 2048;
const NAMES: [&str; 6] = ["alpha.txt", "beta", "gamma.rs", "delta.md", "epsilon.png", "zeta"];

fn app_with_folder() -> App {
    let folder = Location::from_local(FsPath::parse("/tmp/flit-frame-check").unwrap());
    let mut app = App::new(Persisted::default(), vec![Route { folder, select: None }], 0, false, 1.0, SIZE, Instant::now());
    app.caps = Linux::caps();
    for effect in app.drain_effects().collect::<Vec<_>>() {
        let Effect::Provider { req: ProviderRequest::List { gen, path, view, case, out, .. }, .. } = effect else { continue };
        let (mut listing, mut keys) = (Listing::default(), SortKeys::new(case));
        for name in NAMES {
            let i = listing.push(name.as_bytes(), Meta { size: 1, ..Default::default() });
            keys.push(listing.name(i));
        }
        let mut order = Vec::new();
        sort::sort(&listing, &keys, view, &mut order);
        out.publish(Arc::new(DirSnapshot { gen, path, listing, keys, order, view, done: true, error: None, elapsed: Duration::ZERO }));
    }
    app.poll();
    app
}

fn dark_pixels(px: &[u8], (x0, y0, x1, y1): (u32, u32, u32, u32)) -> usize {
    (y0..y1).flat_map(|y| (x0..x1).map(move |x| ((y * SIZE.0 + x) * 4) as usize)).filter(|&i| px[i..i + 3].iter().map(|&c| c as u32).sum::<u32>() < 3 * 110).count()
}

fn save_png(path: &std::path::Path, px: &[u8]) {
    let mut enc = png::Encoder::new(std::fs::File::create(path).unwrap(), SIZE.0, SIZE.1);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(px).unwrap();
}

#[test]
fn a_frame_of_the_app_shows_the_tab_strip_and_the_listing() {
    let dev = GlDevice::headless().unwrap();
    let mut r = gl::GlRenderer::offscreen(&dev, SIZE, ATLAS).unwrap();
    let mut app = app_with_folder();
    let mut gfx = Gfx::new(Linux::text().unwrap(), ATLAS);
    for _ in 0..3 {
        app.frame(&mut gfx, Instant::now());
        for &(slot, off) in &gfx.uploads.rects {
            r.upload(slot, &gfx.uploads.data[off..off + (slot.w * slot.h * 4) as usize]);
        }
        gfx.uploads.clear();
        r.render(&gfx.dl).unwrap();
    }
    let px = gl::tests::read(&r);
    save_png(&std::env::temp_dir().join("flit-app-frame.png"), &px);
    let tab_strip = dark_pixels(&px, (0, 0, 300, 36));
    let list = dark_pixels(&px, (230, 140, 700, 300));
    assert!(tab_strip > 40 && list > 400, "tab strip {tab_strip} and list {list} dark pixels");
}
