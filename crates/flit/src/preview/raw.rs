use flit_core::preview::IMAGE_PX;
use flit_core::ui::Bitmap;
use jpeg_decoder::{CodingProcess, Decoder, ImageInfo, PixelFormat};
use std::cmp::Reverse;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

const MAX_IFDS: usize = 32;
const MAX_ENTRIES: usize = 256;
const MAX_SUB_IFDS: usize = 16;
const MAX_JPEG_BYTES: u32 = 8 << 20;
const MAX_JPEG_PIXELS: u32 = 24_000_000;
const MAX_PROGRESSIVE_PIXELS: u32 = 8_000_000;

#[derive(Clone, Copy)]
pub(super) enum Source {
    Inspector,
    Thumbnail,
}

#[derive(Clone, Copy)]
enum Order {
    Le,
    Be,
}

impl Order {
    fn u16(self, src: &[u8]) -> u16 {
        let bytes = [src[0], src[1]];
        match self { Self::Le => u16::from_le_bytes(bytes), Self::Be => u16::from_be_bytes(bytes) }
    }

    fn u32(self, src: &[u8]) -> u32 {
        let bytes = [src[0], src[1], src[2], src[3]];
        match self { Self::Le => u32::from_le_bytes(bytes), Self::Be => u32::from_be_bytes(bytes) }
    }
}

fn in_file(size: u64, at: u32, len: u32) -> bool {
    at >= 8 && u64::from(at) + u64::from(len) <= size
}

fn read_at<R: Read + Seek>(r: &mut R, size: u64, at: u32, buf: &mut [u8]) -> Option<()> {
    if !in_file(size, at, buf.len().try_into().ok()?) {
        return None;
    }
    r.seek(SeekFrom::Start(u64::from(at))).ok()?;
    r.read_exact(buf).ok()
}

fn queue(offsets: &mut Vec<u32>, at: u32) {
    if at >= 8 && offsets.len() < MAX_IFDS && !offsets.contains(&at) {
        offsets.push(at);
    }
}

type JpegLocation = (u32, u32);

pub(super) fn locations<R: Read + Seek>(r: &mut R, size: u64, current: impl Fn() -> bool) -> Option<(u16, Vec<JpegLocation>)> {
    let mut header = [0; 8];
    r.read_exact(&mut header).ok()?;
    let order = match &header[..2] {
        b"II" => Order::Le,
        b"MM" => Order::Be,
        _ => return None,
    };
    let magic = order.u16(&header[2..]);
    if !matches!(magic, 42 | 85 | 0x4f52 | 0x5352) {
        return None;
    }
    let mut offsets = Vec::with_capacity(MAX_IFDS);
    queue(&mut offsets, order.u32(&header[4..]));
    let mut items = Vec::new();
    let mut orientation = 1;
    let mut index = 0;
    let mut storage = [0; MAX_ENTRIES * 12 + 4];
    while let Some(&at) = offsets.get(index) {
        index += 1;
        if !current() {
            return None;
        }
        let mut count = [0; 2];
        if read_at(r, size, at, &mut count).is_none() {
            continue;
        }
        let count = usize::from(order.u16(&count));
        if count > MAX_ENTRIES {
            continue;
        }
        let bytes = &mut storage[..count * 12 + 4];
        if at.checked_add(2).and_then(|pos| read_at(r, size, pos, bytes)).is_none() {
            continue;
        }
        queue(&mut offsets, order.u32(&bytes[count * 12..]));
        let (mut jpeg, mut strips, mut tiles) = ((0, 0), (0, 0), (0, 0));
        let mut compression = 0;
        for e in bytes[..count * 12].chunks_exact(12) {
            let tag = order.u16(e);
            let (kind, n, value) = (order.u16(&e[2..]), order.u32(&e[4..]), order.u32(&e[8..]));
            if tag == 330 && matches!(kind, 4 | 13) && (1..=MAX_SUB_IFDS as u32).contains(&n) {
                if n == 1 {
                    queue(&mut offsets, value);
                } else {
                    let mut sub = [0; MAX_SUB_IFDS * 4];
                    let range = &mut sub[..n as usize * 4];
                    if read_at(r, size, value, range).is_some() {
                        for offset in range.chunks_exact(4) {
                            queue(&mut offsets, order.u32(offset));
                        }
                    }
                }
            } else if magic == 85 && tag == 0x2e && matches!(kind, 1 | 7) && n > 4 {
                jpeg = (value, n);
            } else if n == 1 && matches!(kind, 3 | 4) {
                let value = if kind == 3 { u32::from(order.u16(&e[8..])) } else { value };
                match tag {
                    259 => compression = value,
                    273 => strips.0 = value,
                    274 if index == 1 && (1..=8).contains(&value) => orientation = value as u16,
                    279 => strips.1 = value,
                    324 => tiles.0 = value,
                    325 => tiles.1 = value,
                    513 => jpeg.0 = value,
                    514 => jpeg.1 = value,
                    _ => {}
                }
            }
        }
        let other = if matches!(compression, 6 | 7) { [strips, tiles] } else { [(0, 0); 2] };
        for (offset, len) in [jpeg, other[0], other[1]] {
            if (4..=MAX_JPEG_BYTES).contains(&len) && in_file(size, offset, len) && !items.contains(&(offset, len)) {
                let mut magic = [0; 3];
                if read_at(r, size, offset, &mut magic).is_some() && magic == [0xFF, 0xD8, 0xFF] {
                    items.push((offset, len));
                }
            }
        }
    }
    Some((orientation, items))
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
        Source::Thumbnail => candidates.sort_unstable_by_key(|&(_, len, side, pixels)| {
            if side >= px { (false, side, pixels, len) } else { (true, u32::MAX - side, u32::MAX - pixels, len) }
        }),
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
    if side <= max { (w, h) } else { ((w * max / side).max(1), (h * max / side).max(1)) }
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
