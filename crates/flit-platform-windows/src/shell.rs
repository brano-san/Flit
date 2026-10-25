use crate::fileops::{is_dir, std_path};
use crate::{com_sta, err};
use flit_core::desktop_ini::DesktopIni;
use flit_core::preview::PropGroup;
use flit_core::ui::{Bitmap, IconKind, IconRequest};
use flit_core::path::PathKind;
use flit_core::provider::Launch;
use flit_core::{FsPath, Location};
use flit_platform::{PlatformError, Shell};
use windows::core::{w, Interface, PCWSTR};
use windows::Win32::Foundation::{ERROR_CANCELLED, HWND, SIZE};
use windows::Win32::System::Com::{CoCreateInstance, IPersistFile, CLSCTX_INPROC_SERVER, STGM_READ};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::Controls::{IImageList, ILD_TRANSPARENT};
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::{CreatePopupMenu, DestroyIcon, DestroyMenu, GetForegroundWindow, GetIconInfo, GetWindowThreadProcessId, HICON, ICONINFO, SW_SHOWNORMAL};

pub struct ShellCom {
    lists: Vec<(u32, IImageList)>,
}

fn wide(p: &FsPath) -> Vec<u16> {
    p.to_wide(false)
}

pub(crate) fn owner_window() -> Option<HWND> {
    unsafe {
        let hwnd = GetForegroundWindow();
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        (!hwnd.is_invalid() && pid == GetCurrentProcessId()).then_some(hwnd)
    }
}

pub(crate) fn desktop_ini(dir: &FsPath) -> Option<DesktopIni> {
    std::fs::read(std_path(&dir.join(b"desktop.ini"))).ok().map(|b| flit_core::desktop_ini::parse(&b))
}

pub(crate) fn shell_addressable(p: &FsPath) -> Result<(), PlatformError> {
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

    fn item_image(&self, path: &FsPath, size: u32, flags: SIIGBF) -> Option<Bitmap> {
        let w = wide(path);
        unsafe {
            let factory: IShellItemImageFactory = SHCreateItemFromParsingName(PCWSTR(w.as_ptr()), None).ok()?;
            let hbm = factory.GetImage(SIZE { cx: size as i32, cy: size as i32 }, flags).ok()?;
            let bits = dib_bits(hbm);
            let _ = DeleteObject(HGDIOBJ(hbm.0));
            let (width, height, mut rgba) = bits?;
            if !rgba.chunks_exact(4).any(|p| p[3] != 0) {
                rgba.chunks_exact_mut(4).for_each(|p| p[3] = 255);
            }
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
            IconKind::Volume => match Location::from_local(req.path.clone()) {
                Location::Wsl { distro, .. } => crate::places::wsl_shortcut(&distro).and_then(|lnk| self.item_image(&lnk, req.size, SIIGBF_ICONONLY)),
                _ => self.item_image(&req.path, req.size, SIIGBF_ICONONLY).or_else(|| self.generic_icon(&req.path, false, req.size)),
            },
            IconKind::File => self.item_image(&req.path, req.size, SIIGBF_ICONONLY).or_else(|| self.generic_icon(&req.path, false, req.size)),
            IconKind::CustomFolder => desktop_ini(&req.path)
                .filter(|ini| ini.icon)
                .and_then(|_| self.item_image(&req.path, req.size, SIIGBF_ICONONLY))
                .or_else(|| self.generic_icon(&req.path, true, req.size)),
            IconKind::Thumbnail => self.thumbnail(req),
        }
    }

    fn thumbnail(&mut self, req: &IconRequest) -> Option<Bitmap> {
        self.item_image(&req.path, req.size, SIIGBF_THUMBNAILONLY)
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

    fn launch(&mut self, launch: &Launch) -> Result<(), PlatformError> {
        if let Launch::Eject(drive) = launch {
            return eject(drive);
        }
        commands(launch, windows_terminal().as_deref()).iter().try_for_each(execute)
    }

    fn shortcut_folder(&mut self, link: &FsPath) -> Option<FsPath> {
        unsafe { link_target(link) }.filter(is_dir)
    }

    fn link_target(&mut self, link: &FsPath) -> Option<FsPath> {
        unsafe { link_target(link) }
    }

    fn preview_image(&mut self, path: &FsPath, max: u32) -> Option<Bitmap> {
        crate::preview::decode_image(path, max)
    }

    fn poster(&mut self, path: &FsPath, size: u32) -> Option<Bitmap> {
        self.item_image(path, size, SIIGBF_THUMBNAILONLY)
    }

    fn properties(&mut self, path: &FsPath) -> Vec<PropGroup> {
        crate::preview::properties(path)
    }

    fn preview_handler(&mut self, ext: &[u8]) -> Option<String> {
        crate::preview::handler(ext)
    }

    fn quick_view(&mut self, path: &FsPath) -> bool {
        crate::preview::quick_view(path)
    }
}

#[derive(Debug, PartialEq)]
pub(crate) struct Command {
    pub program: String,
    pub args: String,
    pub dir: Option<String>,
    pub admin: bool,
}

fn quote_path(s: &str) -> String {
    if !s.is_empty() && !s.contains([' ', '\t']) {
        return s.to_owned();
    }
    let trailing = s.len() - s.trim_end_matches('\\').len();
    format!("\"{s}{}\"", "\\".repeat(trailing))
}

fn wsl_args(distro: &str, path: &str) -> String {
    format!("-d {} --cd {}", quote_path(distro), quote_path(path))
}

pub(crate) fn commands(launch: &Launch, terminal: Option<&str>) -> Vec<Command> {
    let text = |p: &FsPath| p.display().into_owned();
    match launch {
        Launch::With { program, args } => {
            let args = args.iter().map(|a| quote_path(&text(a))).collect::<Vec<_>>().join(" ");
            vec![Command { program: text(program), args, dir: program.parent().map(|d| text(&d)), admin: false }]
        }
        Launch::Terminal { dirs, admin } => match terminal {
            Some(wt) => {
                let tab = |d: &FsPath| match Location::from_local(d.clone()) {
                    Location::Wsl { distro, path } => format!("new-tab wsl.exe {}", wsl_args(&distro, &text(&path).replace(';', r"\;"))),
                    _ => format!("new-tab -d {}", quote_path(&text(d).replace(';', r"\;"))),
                };
                vec![Command { program: wt.to_owned(), args: dirs.iter().map(tab).collect::<Vec<_>>().join(" ; "), dir: None, admin: *admin }]
            }
            None => dirs
                .iter()
                .map(|d| match Location::from_local(d.clone()) {
                    Location::Wsl { distro, path } => Command { program: "wsl.exe".into(), args: wsl_args(&distro, &text(&path)), dir: None, admin: *admin },
                    _ => Command { program: "cmd.exe".into(), args: format!("/k pushd \"{}\"", text(d)), dir: None, admin: *admin },
                })
                .collect(),
        },
        Launch::Eject(_) => Vec::new(),
    }
}

fn eject(drive: &FsPath) -> Result<(), PlatformError> {
    com_sta();
    let hwnd = owner_window().unwrap_or_default();
    let menu: IContextMenu = crate::context_menu::Pidls::parse(std::slice::from_ref(drive))?.ui_object(hwnd)?;
    unsafe {
        let popup = CreatePopupMenu().map_err(err)?;
        let queried = menu.QueryContextMenu(popup, 0, 1, 0x7FFF, CMF_NORMAL);
        let info = CMINVOKECOMMANDINFO { cbSize: size_of::<CMINVOKECOMMANDINFO>() as u32, hwnd, lpVerb: windows::core::s!("eject"), nShow: SW_SHOWNORMAL.0, ..Default::default() };
        let result = queried.ok().and_then(|_| menu.InvokeCommand(&info)).map_err(err);
        let _ = DestroyMenu(popup);
        result
    }
}

fn windows_terminal() -> Option<String> {
    let wt = std::path::Path::new(&std::env::var_os("LOCALAPPDATA")?).join(r"Microsoft\WindowsApps\wt.exe");
    std::fs::symlink_metadata(&wt).is_ok().then(|| wt.to_string_lossy().into_owned())
}

fn execute(c: &Command) -> Result<(), PlatformError> {
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let (file, args, dir) = (wide(&c.program), wide(&c.args), c.dir.as_deref().map(wide));
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOASYNC,
        hwnd: owner_window().unwrap_or_default(),
        lpVerb: if c.admin { w!("runas") } else { PCWSTR::null() },
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(args.as_ptr()),
        lpDirectory: dir.as_ref().map_or(PCWSTR::null(), |d| PCWSTR(d.as_ptr())),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    match unsafe { ShellExecuteExW(&mut info) } {
        Err(e) if e.code() == ERROR_CANCELLED.to_hresult() => Ok(()),
        r => r.map_err(err),
    }
}

pub(crate) unsafe fn link_target(link: &FsPath) -> Option<FsPath> {
    let sl: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
    sl.cast::<IPersistFile>().ok()?.Load(PCWSTR(wide(link).as_ptr()), STGM_READ).ok()?;
    let mut buf = [0u16; 1024];
    sl.GetPath(&mut buf, std::ptr::null_mut(), 0).ok()?;
    let n = buf.iter().position(|&c| c == 0)?;
    FsPath::from_utf16(&buf[..n])
}

#[cfg(test)]
#[path = "../test/shell.rs"]
pub(crate) mod tests;
