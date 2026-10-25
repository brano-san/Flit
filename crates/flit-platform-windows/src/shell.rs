use crate::{com_sta, err};
use flit_core::ui::{Bitmap, IconKind, IconRequest};
use flit_core::path::PathKind;
use flit_core::FsPath;
use flit_platform::{PlatformError, Shell, Trash};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, SIZE};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::Controls::{IImageList, ILD_TRANSPARENT};
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetForegroundWindow, GetIconInfo, GetWindowThreadProcessId, HICON, ICONINFO, SW_SHOWNORMAL};

pub struct ShellCom {
    lists: Vec<(u32, IImageList)>,
}

fn wide(p: &FsPath) -> Vec<u16> {
    p.to_wide(false)
}

fn owner_window() -> Option<HWND> {
    unsafe {
        let hwnd = GetForegroundWindow();
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        (!hwnd.is_invalid() && pid == GetCurrentProcessId()).then_some(hwnd)
    }
}

fn shell_addressable(p: &FsPath) -> Result<(), PlatformError> {
    if p.components().any(|c| c.ends_with(b".") || c.ends_with(b" ")) {
        return Err(PlatformError::new(-1, format!("{} cannot be handled by the Windows shell: a name ends with a dot or a space", p.display())));
    }
    Ok(())
}

unsafe fn dib_bits(hbm: HBITMAP) -> Option<(u32, u32, Vec<u8>)> {
    let mut bm = BITMAP::default();
    if GetObjectW(HGDIOBJ(hbm.0), size_of::<BITMAP>() as i32, Some(&mut bm as *mut _ as _)) == 0 {
        return None;
    }
    let (w, h) = (bm.bmWidth, bm.bmHeight.abs());
    if w <= 0 || h <= 0 {
        return None;
    }
    let mut bi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER { biSize: size_of::<BITMAPINFOHEADER>() as u32, biWidth: w, biHeight: -h, biPlanes: 1, biBitCount: 32, biCompression: BI_RGB.0, ..Default::default() },
        ..Default::default()
    };
    let mut buf = vec![0u8; (w * h * 4) as usize];
    let dc = GetDC(None);
    let lines = GetDIBits(dc, hbm, 0, h as u32, Some(buf.as_mut_ptr() as _), &mut bi, DIB_RGB_COLORS);
    ReleaseDC(None, dc);
    (lines == h).then_some((w as u32, h as u32, buf))
}

fn bgra_to_rgba(px: &mut [u8], premultiply: bool) {
    for p in px.chunks_exact_mut(4) {
        p.swap(0, 2);
        if premultiply {
            let a = p[3] as u32;
            for c in &mut p[..3] {
                *c = ((*c as u32 * a + 127) / 255) as u8;
            }
        }
    }
}

fn box_span(i: u32, dst: u32, src: u32) -> (u32, u32) {
    let start = i * src / dst;
    (start, ((i + 1) * src / dst).max(start + 1))
}

pub(crate) fn fit(b: Bitmap, size: u32) -> Bitmap {
    let (size, longest) = (size.max(1), b.width.max(b.height));
    if longest <= size {
        return b;
    }
    let (dw, dh) = ((b.width * size / longest).max(1), (b.height * size / longest).max(1));
    let mut rgba = Vec::with_capacity((dw * dh * 4) as usize);
    for dy in 0..dh {
        let (y0, y1) = box_span(dy, dh, b.height);
        for dx in 0..dw {
            let (x0, x1) = box_span(dx, dw, b.width);
            let mut sum = [0u32; 4];
            for y in y0..y1 {
                let row = ((y * b.width + x0) * 4) as usize..((y * b.width + x1) * 4) as usize;
                for px in b.rgba[row].chunks_exact(4) {
                    sum.iter_mut().zip(px).for_each(|(s, &v)| *s += v as u32);
                }
            }
            let n = (x1 - x0) * (y1 - y0);
            rgba.extend(sum.map(|s| ((s + n / 2) / n) as u8));
        }
    }
    Bitmap { width: dw, height: dh, rgba }
}

unsafe fn icon_bitmap(icon: HICON) -> Option<Bitmap> {
    let mut ii = ICONINFO::default();
    let got = GetIconInfo(icon, &mut ii).is_ok();
    let _ = DestroyIcon(icon);
    if !got {
        return None;
    }
    let color = dib_bits(ii.hbmColor);
    let mask = dib_bits(ii.hbmMask);
    let _ = DeleteObject(HGDIOBJ(ii.hbmColor.0));
    let _ = DeleteObject(HGDIOBJ(ii.hbmMask.0));
    let (width, height, mut rgba) = color?;
    if !rgba.chunks_exact(4).any(|p| p[3] != 0) {
        let (_, _, m) = mask?;
        for (p, m) in rgba.chunks_exact_mut(4).zip(m.chunks_exact(4)) {
            p[3] = if m[0] == 0 { 255 } else { 0 };
        }
    }
    bgra_to_rgba(&mut rgba, true);
    Some(Bitmap { width, height, rgba })
}

impl ShellCom {
    fn list_for(&mut self, size: u32) -> Option<&IImageList> {
        if self.lists.is_empty() {
            for id in [SHIL_SMALL, SHIL_LARGE, SHIL_EXTRALARGE, SHIL_JUMBO] {
                if let Ok(list) = unsafe { SHGetImageList::<IImageList>(id as i32) } {
                    let (mut cx, mut cy) = (0, 0);
                    if unsafe { list.GetIconSize(&mut cx, &mut cy) }.is_ok() {
                        self.lists.push((cx as u32, list));
                    }
                }
            }
            self.lists.sort_by_key(|l| l.0);
        }
        self.lists.iter().find(|l| l.0 >= size).or(self.lists.last()).map(|l| &l.1)
    }

    fn generic_icon(&mut self, path: &FsPath, dir: bool, size: u32) -> Option<Bitmap> {
        let attrs = if dir { FILE_ATTRIBUTE_DIRECTORY } else { FILE_ATTRIBUTE_NORMAL };
        let w = wide(path);
        let mut info = SHFILEINFOW::default();
        let ok = unsafe { SHGetFileInfoW(PCWSTR(w.as_ptr()), attrs, Some(&mut info), size_of::<SHFILEINFOW>() as u32, SHGFI_SYSICONINDEX | SHGFI_USEFILEATTRIBUTES) };
        if ok == 0 {
            return None;
        }
        let list = self.list_for(size)?;
        let icon = unsafe { list.GetIcon(info.iIcon, ILD_TRANSPARENT.0) }.ok()?;
        unsafe { icon_bitmap(icon) }.map(|b| fit(b, size))
    }

    fn item_icon(&self, path: &FsPath, size: u32) -> Option<Bitmap> {
        let w = wide(path);
        unsafe {
            let factory: IShellItemImageFactory = SHCreateItemFromParsingName(PCWSTR(w.as_ptr()), None).ok()?;
            let hbm = factory.GetImage(SIZE { cx: size as i32, cy: size as i32 }, SIIGBF_ICONONLY).ok()?;
            let bits = dib_bits(hbm);
            let _ = DeleteObject(HGDIOBJ(hbm.0));
            let (width, height, mut rgba) = bits?;
            bgra_to_rgba(&mut rgba, false);
            Some(fit(Bitmap { width, height, rgba }, size))
        }
    }
}

impl Shell for ShellCom {
    fn new() -> Result<Self, PlatformError> {
        com_sta();
        static INIT: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = INIT.lock().unwrap_or_else(|e| e.into_inner());
        let mut shell = Self { lists: Vec::new() };
        shell.list_for(0);
        let mut info = SHFILEINFOW::default();
        unsafe { SHGetFileInfoW(w!("x"), FILE_ATTRIBUTE_NORMAL, Some(&mut info), size_of::<SHFILEINFOW>() as u32, SHGFI_SYSICONINDEX | SHGFI_USEFILEATTRIBUTES) };
        Ok(shell)
    }

    fn icon(&mut self, req: &IconRequest) -> Option<Bitmap> {
        match req.kind {
            IconKind::Type => self.generic_icon(&req.path, false, req.size),
            IconKind::Folder => self.generic_icon(&req.path, true, req.size),
            IconKind::File | IconKind::Volume => self.item_icon(&req.path, req.size).or_else(|| self.generic_icon(&req.path, false, req.size)),
        }
    }

    fn type_name(&mut self, ext: &[u8]) -> Option<String> {
        let mut name = b"x.".to_vec();
        name.extend_from_slice(ext);
        let mut w = Vec::new();
        flit_core::name::to_utf16(&name, &mut w);
        w.push(0);
        let mut info = SHFILEINFOW::default();
        let ok = unsafe { SHGetFileInfoW(PCWSTR(w.as_ptr()), FILE_ATTRIBUTE_NORMAL, Some(&mut info), size_of::<SHFILEINFOW>() as u32, SHGFI_TYPENAME | SHGFI_USEFILEATTRIBUTES) };
        if ok == 0 {
            return None;
        }
        let n = info.szTypeName.iter().position(|&c| c == 0).unwrap_or(info.szTypeName.len());
        Some(String::from_utf16_lossy(&info.szTypeName[..n])).filter(|s| !s.is_empty())
    }

    fn open(&mut self, path: &FsPath) -> Result<(), PlatformError> {
        shell_addressable(path)?;
        let file = wide(path);
        let dir = path.parent().filter(|p| p.kind() != PathKind::Computer).map(|p| wide(&p));
        let mut info = SHELLEXECUTEINFOW {
            cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NOASYNC | SEE_MASK_FLAG_LOG_USAGE,
            hwnd: owner_window().unwrap_or_default(),
            lpFile: PCWSTR(file.as_ptr()),
            lpDirectory: dir.as_ref().map_or(PCWSTR::null(), |d| PCWSTR(d.as_ptr())),
            nShow: SW_SHOWNORMAL.0,
            ..Default::default()
        };
        unsafe { ShellExecuteExW(&mut info) }.map_err(err)
    }
}

impl Trash for ShellCom {
    fn trash(&mut self, paths: &[FsPath]) -> Result<(), PlatformError> {
        paths.iter().try_for_each(shell_addressable)?;
        unsafe {
            let op: IFileOperation = CoCreateInstance(&FileOperation, None, CLSCTX_ALL).map_err(err)?;
            op.SetOperationFlags(FOF_ALLOWUNDO | FOFX_ADDUNDORECORD | FOFX_RECYCLEONDELETE).map_err(err)?;
            if let Some(owner) = owner_window() {
                op.SetOwnerWindow(owner).map_err(err)?;
            }
            for p in paths {
                let w = wide(p);
                let item: IShellItem = SHCreateItemFromParsingName(PCWSTR(w.as_ptr()), None).map_err(err)?;
                op.DeleteItem(&item, None).map_err(err)?;
            }
            op.PerformOperations().map_err(err)?;
            if op.GetAnyOperationsAborted().map_err(err)?.as_bool() {
                return Err(PlatformError::new(-1, "Deletion was cancelled"));
            }
            Ok(())
        }
    }
}

#[cfg(test)]
#[path = "../test/shell.rs"]
mod tests;
