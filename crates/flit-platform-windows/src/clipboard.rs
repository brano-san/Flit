use flit_core::preview::straight_rgba;
use flit_core::ui::Bitmap;
use flit_core::FsPath;
use flit_platform::{Pasted, PlatformError};
use std::cell::OnceCell;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;
use windows::core::w;
use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND};
use windows::Win32::Graphics::Imaging::{CLSID_WICImagingFactory, GUID_ContainerFormatPng, GUID_WICPixelFormat32bppBGRA, IWICImagingFactory, WICBitmapEncoderNoCache};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER, STATFLAG_NONAME, STATSTG, STREAM_SEEK_SET};
use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, GetClipboardData, GetClipboardSequenceNumber, OpenClipboard, RegisterClipboardFormatW, SetClipboardData};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::System::Ole::{CF_DIB, CF_DIBV5, CF_HDROP, CF_UNICODETEXT, DROPEFFECT_COPY, DROPEFFECT_LINK, DROPEFFECT_MOVE};
use windows::Win32::UI::Shell::SHCreateMemStream;

const HEADER: usize = 20;
const OPEN_ATTEMPTS: u32 = 20;
const INFO_HEADER: u32 = 40;
const V5_HEADER: u32 = 124;
const BI_RGB: u32 = 0;
const BI_BITFIELDS: u32 = 3;
const LCS_SRGB: u32 = 0x7352_4742;
const LCS_GM_IMAGES: u32 = 4;
const PELS_PER_METER: u32 = 2835;

thread_local! {
    static WIC: OnceCell<Option<IWICImagingFactory>> = const { OnceCell::new() };
}

static READ_SEQUENCE: AtomicU32 = AtomicU32::new(0);

pub(crate) fn drop_effect_format() -> u32 {
    unsafe { RegisterClipboardFormatW(w!("Preferred DropEffect")) }
}

pub(crate) fn dropfiles(paths: &[FsPath]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER + paths.len() * 64);
    out.extend_from_slice(&(HEADER as u32).to_le_bytes());
    out.extend_from_slice(&[0; 12]);
    out.extend_from_slice(&1u32.to_le_bytes());
    for p in paths {
        for u in p.to_wide(false) {
            out.extend_from_slice(&u.to_le_bytes());
        }
    }
    out.extend_from_slice(&[0, 0]);
    out
}

pub(crate) fn parse_dropfiles(bytes: &[u8]) -> Vec<FsPath> {
    let word = |at: usize| bytes.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let (Some(start), Some(wide)) = (word(0), word(16)) else { return Vec::new() };
    let body = bytes.get(start as usize..).unwrap_or_default();
    let names: Vec<Vec<u16>> = if wide != 0 {
        let units: Vec<u16> = body.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        units.split(|&u| u == 0).take_while(|s| !s.is_empty()).map(<[u16]>::to_vec).collect()
    } else {
        body.split(|&b| b == 0).take_while(|s| !s.is_empty()).map(|s| String::from_utf8_lossy(s).encode_utf16().collect()).collect()
    };
    names.iter().filter_map(|n| FsPath::from_utf16(n)).collect()
}

pub(crate) fn effect_is_move(effect: u32) -> bool {
    effect & DROPEFFECT_MOVE.0 != 0 && effect & (DROPEFFECT_COPY.0 | DROPEFFECT_LINK.0) == 0
}

pub(crate) unsafe fn global_bytes(h: HGLOBAL) -> Option<Vec<u8>> {
    let p = GlobalLock(h) as *const u8;
    if p.is_null() {
        return None;
    }
    let bytes = std::slice::from_raw_parts(p, GlobalSize(h)).to_vec();
    let _ = GlobalUnlock(h);
    Some(bytes)
}

pub(crate) unsafe fn global(bytes: &[u8]) -> Option<HGLOBAL> {
    let h = GlobalAlloc(GMEM_MOVEABLE, bytes.len()).ok()?;
    let p = GlobalLock(h) as *mut u8;
    if p.is_null() {
        let _ = GlobalFree(Some(h));
        return None;
    }
    std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
    let _ = GlobalUnlock(h);
    Some(h)
}

unsafe fn put(format: u32, bytes: &[u8]) -> bool {
    let Some(h) = global(bytes) else { return false };
    let owned_by_clipboard = SetClipboardData(format, Some(HANDLE(h.0))).is_ok();
    if !owned_by_clipboard {
        let _ = GlobalFree(Some(h));
    }
    owned_by_clipboard
}

pub(crate) unsafe fn open(owner: Option<HWND>) -> Result<(), PlatformError> {
    for attempt in 0..OPEN_ATTEMPTS {
        if OpenClipboard(owner).is_ok() {
            return Ok(());
        }
        if attempt + 1 < OPEN_ATTEMPTS {
            std::thread::sleep(Duration::from_millis(15));
        }
    }
    Err(PlatformError::new(-1, flit_core::i18n::S::ErrorsEngineClipboardBusy.en()))
}

pub(crate) fn write_files(owner: HWND, paths: &[FsPath], cut: bool) -> bool {
    let effect = if cut { DROPEFFECT_MOVE.0 } else { DROPEFFECT_COPY.0 | DROPEFFECT_LINK.0 };
    unsafe {
        if open(Some(owner)).is_err() {
            return false;
        }
        let ok = EmptyClipboard().is_ok() && put(CF_HDROP.0 as u32, &dropfiles(paths)) && put(drop_effect_format(), &effect.to_le_bytes());
        let _ = CloseClipboard();
        ok
    }
}

pub(crate) fn png_format() -> u32 {
    unsafe { RegisterClipboardFormatW(w!("PNG")) }
}

fn whole(image: &Bitmap) -> bool {
    image.width > 0 && image.height > 0 && image.rgba.len() == image.width as usize * image.height as usize * 4
}

pub(crate) fn dib(image: &Bitmap, v5: bool) -> Vec<u8> {
    if !whole(image) {
        return Vec::new();
    }
    let (w, h) = (image.width as usize, image.height as usize);
    let header = if v5 { V5_HEADER } else { INFO_HEADER };
    let mut out = Vec::with_capacity(header as usize + w * h * 4);
    let mut word = |v: u32| out.extend_from_slice(&v.to_le_bytes());
    word(header);
    word(image.width);
    word(image.height);
    word(1 | (32 << 16));
    word(if v5 { BI_BITFIELDS } else { BI_RGB });
    word((w * h * 4) as u32);
    word(PELS_PER_METER);
    word(PELS_PER_METER);
    word(0);
    word(0);
    if v5 {
        for v in [0x00FF_0000, 0x0000_FF00, 0x0000_00FF, 0xFF00_0000, LCS_SRGB] {
            word(v);
        }
        for _ in 0..12 {
            word(0);
        }
        for v in [LCS_GM_IMAGES, 0, 0, 0] {
            word(v);
        }
    }
    let straight = if v5 { straight_rgba(&image.rgba) } else { Vec::new() };
    for row in (0..h).rev() {
        for k in row * w..(row + 1) * w {
            let p = &image.rgba[k * 4..k * 4 + 4];
            if v5 {
                let s = &straight[k * 4..k * 4 + 4];
                out.extend_from_slice(&[s[2], s[1], s[0], s[3]]);
            } else {
                let white = 255 - p[3];
                out.extend_from_slice(&[p[2].saturating_add(white), p[1].saturating_add(white), p[0].saturating_add(white), 255]);
            }
        }
    }
    out
}

fn wic() -> Option<IWICImagingFactory> {
    WIC.with(|f| f.get_or_init(|| unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER) }.map_err(|e| tracing::info!("no image encoder: {}", e.message())).ok()).clone())
}

pub(crate) fn png(image: &Bitmap) -> Option<Vec<u8>> {
    if !whole(image) {
        return None;
    }
    let factory = wic()?;
    let bgra: Vec<u8> = straight_rgba(&image.rgba).chunks_exact(4).flat_map(|p| [p[2], p[1], p[0], p[3]]).collect();
    unsafe {
        let stream = SHCreateMemStream(None)?;
        let encoder = factory.CreateEncoder(&GUID_ContainerFormatPng, std::ptr::null()).ok()?;
        encoder.Initialize(&stream, WICBitmapEncoderNoCache).ok()?;
        let (mut frame, mut options) = (None, None);
        encoder.CreateNewFrame(&mut frame, &mut options).ok()?;
        let frame = frame?;
        frame.Initialize(options.as_ref()).ok()?;
        frame.SetSize(image.width, image.height).ok()?;
        let mut format = GUID_WICPixelFormat32bppBGRA;
        frame.SetPixelFormat(&mut format).ok()?;
        if format != GUID_WICPixelFormat32bppBGRA {
            return None;
        }
        frame.WritePixels(image.height, image.width * 4, &bgra).ok()?;
        frame.Commit().ok()?;
        encoder.Commit().ok()?;
        let mut stat = STATSTG::default();
        stream.Stat(&mut stat, STATFLAG_NONAME).ok()?;
        stream.Seek(0, STREAM_SEEK_SET, None).ok()?;
        let mut out = vec![0u8; usize::try_from(stat.cbSize).ok()?];
        let mut read = 0u32;
        stream.Read(out.as_mut_ptr().cast(), u32::try_from(out.len()).ok()?, Some(&mut read)).ok().ok()?;
        out.truncate(read as usize);
        Some(out)
    }
}

pub(crate) fn write_image(owner: HWND, image: &Bitmap) -> bool {
    if !whole(image) {
        return false;
    }
    let png = png(image);
    let (v5, plain) = (dib(image, true), dib(image, false));
    unsafe {
        if open(Some(owner)).is_err() {
            return false;
        }
        let ok = EmptyClipboard().is_ok() && png.as_ref().is_none_or(|b| put(png_format(), b)) && put(CF_DIBV5.0 as u32, &v5) && put(CF_DIB.0 as u32, &plain);
        let _ = CloseClipboard();
        ok
    }
}

pub(crate) fn read_files() -> Result<(Vec<FsPath>, bool), PlatformError> {
    unsafe {
        open(None)?;
        let files = GetClipboardData(CF_HDROP.0 as u32).ok().and_then(|h| global_bytes(HGLOBAL(h.0))).map(|b| parse_dropfiles(&b));
        let effect = GetClipboardData(drop_effect_format()).ok().and_then(|h| global_bytes(HGLOBAL(h.0))).and_then(|b| b.get(..4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])));
        READ_SEQUENCE.store(GetClipboardSequenceNumber(), Ordering::Release);
        let _ = CloseClipboard();
        match files {
            Some(files) if !files.is_empty() => Ok((files, effect.is_some_and(effect_is_move))),
            _ => Err(PlatformError::new(-1, flit_core::i18n::S::ErrorsEngineClipboardNoFiles.en())),
        }
    }
}

pub(crate) fn from_dib(b: &[u8]) -> Option<Bitmap> {
    let u32_at = |at: usize| b.get(at..at + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]));
    let header = u32_at(0)? as usize;
    let (width, height) = (u32_at(4)? as i32, u32_at(8)? as i32);
    let bits = u16::from_le_bytes([*b.get(14)?, *b.get(15)?]);
    let compression = u32_at(16)?;
    if header < INFO_HEADER as usize || width <= 0 || height == 0 || !matches!(bits, 24 | 32) || !matches!(compression, BI_RGB | BI_BITFIELDS) {
        return None;
    }
    if compression == BI_BITFIELDS && (bits != 32 || [u32_at(40)?, u32_at(44)?, u32_at(48)?] != [0x00FF_0000, 0x0000_FF00, 0x0000_00FF]) {
        return None;
    }
    let masks = if compression == BI_BITFIELDS && header == INFO_HEADER as usize { 12 } else { 0 };
    let (w, h) = (width as usize, height.unsigned_abs() as usize);
    let stride = (w * bits as usize / 8).div_ceil(4) * 4;
    let pixels = b.get(header + masks..)?.get(..stride.checked_mul(h)?)?;
    let alpha = bits == 32 && pixels.chunks_exact(stride).any(|row| row[..w * 4].chunks_exact(4).any(|p| p[3] != 0));
    let mut rgba = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        let row = if height > 0 { h - 1 - y } else { y };
        for p in pixels[row * stride..][..w * bits as usize / 8].chunks_exact(bits as usize / 8) {
            let a = if alpha { p[3] } else { 255 };
            let mix = |c: u8| ((u32::from(c) * u32::from(a) + 127) / 255) as u8;
            rgba.extend_from_slice(&[mix(p[2]), mix(p[1]), mix(p[0]), a]);
        }
    }
    Some(Bitmap { width: w as u32, height: h as u32, rgba })
}

pub(crate) fn read_content() -> Option<Pasted> {
    unsafe {
        open(None).ok()?;
        let get = |format: u32| GetClipboardData(format).ok().and_then(|h| global_bytes(HGLOBAL(h.0)));
        let png = get(png_format());
        let dib = if png.is_none() { get(CF_DIBV5.0 as u32).or_else(|| get(CF_DIB.0 as u32)) } else { None };
        let text = get(CF_UNICODETEXT.0 as u32);
        let _ = CloseClipboard();
        if let Some(image) = png.or_else(|| dib.as_deref().and_then(from_dib).and_then(|b| self::png(&b))) {
            return Some(Pasted::Image(image));
        }
        let wide: Vec<u16> = text?.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&c| c != 0).collect();
        Some(Pasted::Text(String::from_utf16_lossy(&wide)))
    }
}

pub(crate) fn clear_if_unchanged() {
    unsafe {
        if GetClipboardSequenceNumber() != READ_SEQUENCE.load(Ordering::Acquire) || open(None).is_err() {
            return;
        }
        if GetClipboardSequenceNumber() == READ_SEQUENCE.load(Ordering::Acquire) {
            let _ = EmptyClipboard();
        }
        let _ = CloseClipboard();
    }
}

#[cfg(test)]
#[path = "../test/clipboard.rs"]
mod tests;
