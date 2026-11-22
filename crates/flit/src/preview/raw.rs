use flit_core::preview::IMAGE_PX;
use flit_core::ui::Bitmap;
use jpeg_decoder::{CodingProcess, Decoder, ImageInfo, PixelFormat};
use std::cmp::Reverse;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

#[path = "raw_tiff.rs"]
mod tiff;
pub(super) use tiff::locations;
use tiff::MAX_JPEG_BYTES;

const MAX_JPEG_PIXELS: u32 = 24_000_000;
const MAX_PROGRESSIVE_PIXELS: u32 = 8_000_000;

#[derive(Clone, Copy)]
pub(super) enum Source {
    Inspector,
    Thumbnail,
}

pub(super) fn load(path: &Path, px: u32, source: Source, current: impl Fn() -> bool) -> Option<Bitmap> {
    let mut r = super::regular_file(path)?;
    let meta = r.metadata().ok()?;
    if !meta.is_file() {
        return None;
    }
    let (orient, locations) = locations(&mut r, meta.len(), &current)?;
    let px = px.clamp(1, IMAGE_PX);
    let mut candidates = Vec::new();
    let mut header_budget = MAX_JPEG_BYTES * 2;
    for (offset, len) in locations {
        if !current() {
            return None;
        }
        if header_budget == 0 {
            break;
        }
        if r.seek(SeekFrom::Start(u64::from(offset))).is_err() {
            continue;
        }
        let max_read = len.min(header_budget);
        let mut head = BufReader::new((&mut r).take(u64::from(max_read)));
        if let Some(info) = read_info(&mut Decoder::new(&mut head)) {
            let (w, h) = (u32::from(info.width), u32::from(info.height));
            candidates.push((offset, len, w.max(h), w * h));
        }
        header_budget -= max_read - head.get_ref().limit() as u32;
    }
    match source {
        Source::Inspector => candidates.sort_unstable_by_key(|&(_, len, side, pixels)| (Reverse(pixels), Reverse(side), len)),
        Source::Thumbnail => candidates.sort_unstable_by_key(|&(_, len, side, pixels)| if side >= px { (false, side, pixels, len) } else { (true, u32::MAX - side, u32::MAX - pixels, len) }),
    }
    let mut budget = MAX_JPEG_BYTES * 2;
    for (offset, len, _, _) in candidates {
        if !current() {
            return None;
        }
        let Some(left) = budget.checked_sub(len) else { continue };
        budget = left;
        if r.seek(SeekFrom::Start(u64::from(offset))).is_ok() {
            let jpeg = BufReader::new((&mut r).take(u64::from(len)));
            if let Some(image) = decode(jpeg, px, orient) {
                return current().then_some(image);
            }
        }
    }
    None
}

fn fit(w: u32, h: u32, max: u32) -> (u32, u32) {
    let side = w.max(h);
    if side <= max {
        (w, h)
    } else {
        ((w * max / side).max(1), (h * max / side).max(1))
    }
}

fn read_info(decoder: &mut Decoder<impl Read>) -> Option<ImageInfo> {
    decoder.read_info().ok()?;
    let info = decoder.info()?;
    let (w, h) = (u32::from(info.width), u32::from(info.height));
    let limit = if info.coding_process == CodingProcess::DctProgressive { MAX_PROGRESSIVE_PIXELS } else { MAX_JPEG_PIXELS };
    (w * h <= limit && matches!(info.pixel_format, PixelFormat::RGB24 | PixelFormat::L8)).then_some(info)
}

fn decode(jpeg: impl Read, px: u32, orient: u16) -> Option<Bitmap> {
    let mut decoder = Decoder::new(jpeg);
    decoder.set_max_decoding_buffer_size(MAX_JPEG_PIXELS as usize * 4);
    let info = read_info(&mut decoder)?;
    let (w, h) = (u32::from(info.width), u32::from(info.height));
    let channels = if info.pixel_format == PixelFormat::RGB24 { 3 } else { 1 };
    let (dw, dh) = fit(w, h, px);
    let (w, h) = decoder.scale(dw as u16, dh as u16).ok()?;
    let pixels = decoder.decode().ok()?;
    let (w, h) = (u32::from(w), u32::from(h));
    if pixels.len() != (w * h) as usize * channels {
        return None;
    }
    let (width, height) = if orient >= 5 { (dh, dw) } else { (dw, dh) };
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            let (sx, sy) = match orient {
                2 => (dw - 1 - x, y),
                3 => (dw - 1 - x, dh - 1 - y),
                4 => (x, dh - 1 - y),
                5 => (y, x),
                6 => (y, dh - 1 - x),
                7 => (dw - 1 - y, dh - 1 - x),
                8 => (dw - 1 - y, x),
                _ => (x, y),
            };
            let i = (sy * h / dh * w + sx * w / dw) as usize * channels;
            let rgb = if channels == 3 { [pixels[i], pixels[i + 1], pixels[i + 2]] } else { [pixels[i]; 3] };
            rgba.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
        }
    }
    Some(Bitmap { width, height, rgba })
}
