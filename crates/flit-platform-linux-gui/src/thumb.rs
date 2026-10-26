use flit_core::ui::Bitmap;
use md5::{Digest, Md5};
use std::fs::File;
use std::io::BufReader;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

const MAX_SOURCE_BYTES: u64 = 256 << 20;

fn lower_ext(path: &Path) -> Vec<u8> {
    path.extension().map(|e| e.as_bytes().to_ascii_lowercase()).unwrap_or_default()
}

pub fn supported(path: &Path) -> bool {
    matches!(lower_ext(path).as_slice(), b"jpg" | b"jpeg" | b"jpe" | b"jfif" | b"png")
}

pub fn freedesktop_name(path: &Path) -> String {
    let uri = format!("file://{}", flit_platform_linux::percent_encode(path.as_os_str().as_bytes()));
    Md5::digest(uri.as_bytes()).iter().map(|b| format!("{b:02x}")).collect::<String>() + ".png"
}

fn cached(path: &Path, cache: &Path, size: u32) -> Option<Bitmap> {
    let mtime = std::fs::metadata(path).ok()?.mtime().to_string();
    let dirs: &[&str] = if size <= 128 { &["normal", "large", "x-large"] } else { &["large", "x-large"] };
    let name = freedesktop_name(path);
    dirs.iter().find_map(|d| {
        let file = cache.join(d).join(&name);
        let decoder = png::Decoder::new(BufReader::new(File::open(file).ok()?));
        let reader = decoder.read_info().ok()?;
        let fresh = reader.info().uncompressed_latin1_text.iter().any(|t| t.keyword == "Thumb::MTime" && t.text == mtime);
        fresh.then(|| decode_png_from(reader)).flatten()
    })
}

fn decode_png_from<R: std::io::BufRead + std::io::Seek>(mut reader: png::Reader<R>) -> Option<Bitmap> {
    let mut buf = vec![0; reader.output_buffer_size()?];
    let frame = reader.next_frame(&mut buf).ok()?;
    buf.truncate(frame.buffer_size());
    let (w, h) = (frame.width, frame.height);
    let premultiply = |p: &[u8]| {
        let a = p[3] as u32;
        [p[0], p[1], p[2]].map(|c| ((c as u32 * a + 127) / 255) as u8)
    };
    let rgba = match (frame.color_type, frame.bit_depth) {
        (png::ColorType::Rgba, png::BitDepth::Eight) => buf.chunks_exact(4).flat_map(|p| {
            let [r, g, b] = premultiply(p);
            [r, g, b, p[3]]
        }).collect(),
        (png::ColorType::Rgb, png::BitDepth::Eight) => buf.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        (png::ColorType::GrayscaleAlpha, png::BitDepth::Eight) => buf.chunks_exact(2).flat_map(|p| {
            let [g, _, _] = premultiply(&[p[0], p[0], p[0], p[1]]);
            [g, g, g, p[1]]
        }).collect(),
        (png::ColorType::Grayscale, png::BitDepth::Eight) => buf.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        _ => return None,
    };
    Some(Bitmap { width: w, height: h, rgba })
}

pub fn decode_png(path: &Path) -> Option<Bitmap> {
    let mut decoder = png::Decoder::new(BufReader::new(File::open(path).ok()?));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    decode_png_from(decoder.read_info().ok()?)
}

fn decode_jpeg_bytes(data: &[u8], size: u32) -> Option<Bitmap> {
    let mut decoder = jpeg_decoder::Decoder::new(data);
    let target = size.clamp(1, u16::MAX as u32) as u16;
    let (w, h) = decoder.scale(target, target).ok()?;
    let pixels = decoder.decode().ok()?;
    let rgba = match decoder.info()?.pixel_format {
        jpeg_decoder::PixelFormat::RGB24 => pixels.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        jpeg_decoder::PixelFormat::L8 => pixels.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        _ => return None,
    };
    Some(Bitmap { width: w as u32, height: h as u32, rgba })
}

fn read_u16(b: &[u8], at: usize, le: bool) -> Option<u16> {
    let v = [*b.get(at)?, *b.get(at + 1)?];
    Some(if le { u16::from_le_bytes(v) } else { u16::from_be_bytes(v) })
}

fn read_u32(b: &[u8], at: usize, le: bool) -> Option<u32> {
    let v = [*b.get(at)?, *b.get(at + 1)?, *b.get(at + 2)?, *b.get(at + 3)?];
    Some(if le { u32::from_le_bytes(v) } else { u32::from_be_bytes(v) })
}

pub fn exif_block(jpeg: &[u8]) -> Option<&[u8]> {
    let mut at = 2;
    while at + 4 <= jpeg.len() && jpeg[at] == 0xFF {
        let marker = jpeg[at + 1];
        let len = read_u16(jpeg, at + 2, false)? as usize;
        if marker == 0xDA {
            return None;
        }
        let body = jpeg.get(at + 4..at + 2 + len)?;
        if marker == 0xE1 && body.starts_with(b"Exif\0\0") {
            return Some(&body[6..]);
        }
        at += 2 + len;
    }
    None
}

pub fn exif_thumbnail(jpeg: &[u8]) -> Option<&[u8]> {
    let tiff = exif_block(jpeg)?;
    let le = match tiff.get(..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let ifd0 = read_u32(tiff, 4, le)? as usize;
    let count0 = read_u16(tiff, ifd0, le)? as usize;
    let ifd1 = read_u32(tiff, ifd0 + 2 + count0 * 12, le)? as usize;
    if ifd1 == 0 {
        return None;
    }
    let (mut offset, mut length) = (None, None);
    for k in 0..read_u16(tiff, ifd1, le)? as usize {
        let entry = ifd1 + 2 + k * 12;
        match read_u16(tiff, entry, le)? {
            0x0201 => offset = read_u32(tiff, entry + 8, le),
            0x0202 => length = read_u32(tiff, entry + 8, le),
            _ => {}
        }
    }
    let (offset, length) = (offset? as usize, length? as usize);
    tiff.get(offset..offset.checked_add(length)?)
}

fn decode_jpeg(path: &Path, size: u32) -> Option<Bitmap> {
    let data = std::fs::read(path).ok()?;
    let embedded = exif_thumbnail(&data).and_then(|t| decode_jpeg_bytes(t, size)).filter(|b| b.width.max(b.height) >= size);
    embedded.or_else(|| decode_jpeg_bytes(&data, size))
}

pub fn fit(b: Bitmap, size: u32) -> Bitmap {
    let scale = size as f64 / b.width.max(b.height).max(1) as f64;
    if scale >= 1.0 {
        return b;
    }
    let (w, h) = (((b.width as f64 * scale).round() as u32).max(1), ((b.height as f64 * scale).round() as u32).max(1));
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        let (y0, y1) = (y * b.height / h, ((y + 1) * b.height / h).max(y * b.height / h + 1));
        for x in 0..w {
            let (x0, x1) = (x * b.width / w, ((x + 1) * b.width / w).max(x * b.width / w + 1));
            let mut sum = [0u32; 4];
            for sy in y0..y1 {
                let row = &b.rgba[(sy * b.width * 4) as usize..][..(b.width * 4) as usize];
                for px in row[(x0 * 4) as usize..(x1 * 4) as usize].chunks_exact(4) {
                    for c in 0..4 {
                        sum[c] += px[c] as u32;
                    }
                }
            }
            let n = (y1 - y0) * (x1 - x0);
            rgba.extend(sum.map(|s| (s / n) as u8));
        }
    }
    Bitmap { width: w, height: h, rgba }
}

pub fn thumbnail(path: &Path, size: u32, cache: Option<PathBuf>) -> Option<Bitmap> {
    if !supported(path) || std::fs::metadata(path).ok()?.len() > MAX_SOURCE_BYTES {
        return None;
    }
    if let Some(b) = cache.and_then(|c| cached(path, &c, size)) {
        return Some(fit(b, size));
    }
    let b = if lower_ext(path) == b"png" { decode_png(path)? } else { decode_jpeg(path, size)? };
    Some(fit(b, size))
}

#[cfg(test)]
#[path = "../test/thumb.rs"]
mod tests;
