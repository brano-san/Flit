use flit_core::FsPath;
use flit_platform::PlatformError;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;
use windows::core::w;
use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND};
use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, GetClipboardData, GetClipboardSequenceNumber, OpenClipboard, RegisterClipboardFormatW, SetClipboardData};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::System::Ole::{CF_HDROP, DROPEFFECT_COPY, DROPEFFECT_LINK, DROPEFFECT_MOVE};

const HEADER: usize = 20;
const OPEN_ATTEMPTS: u32 = 20;

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
