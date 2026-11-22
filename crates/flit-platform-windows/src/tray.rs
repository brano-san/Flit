use flit_platform::{tray::{Action, Menu, Tray}, SystemPlatform};
use std::cell::Cell;
use std::time::{Duration, Instant};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
use windows::Win32::UI::HiDpi::{GetDpiForSystem, GetDpiForWindow, GetSystemMetricsForDpi};
use windows::Win32::UI::Shell::{Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETVERSION, NIN_SELECT, NOTIFYICONDATAW, NOTIFYICON_VERSION_4};
use windows::Win32::UI::WindowsAndMessaging::*;

pub const CALLBACK: u32 = WM_APP + 4;
pub const RETRY_TIMER: usize = 1;
const NIN_KEYSELECT: u32 = NIN_SELECT | 1;
const RETRY_WINDOW: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Refresh {
    Normal,
    Theme,
    Taskbar,
    Timer,
}

#[derive(Clone, Copy)]
enum Retry {
    Ready,
    Waiting(Instant),
    Exhausted,
}

impl Retry {
    fn can_try(self, trigger: Refresh, now: Instant) -> bool {
        match self {
            Self::Ready => trigger != Refresh::Timer,
            Self::Waiting(until) => trigger == Refresh::Taskbar || (trigger == Refresh::Timer && now < until),
            Self::Exhausted => trigger == Refresh::Taskbar,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Click {
    Activate,
    Menu(i32, i32),
}

fn click(wp: WPARAM, lp: LPARAM) -> Option<Click> {
    match (lp.0 as u32) & 0xffff {
        NIN_SELECT | NIN_KEYSELECT => Some(Click::Activate),
        WM_CONTEXTMENU => Some(Click::Menu(wp.0 as i16 as i32, (wp.0 >> 16) as i16 as i32)),
        _ => None,
    }
}

pub struct Icon {
    hwnd: HWND,
    added: Cell<bool>,
    appearance: Cell<Option<(bool, i32, i32)>>,
    retry: Cell<Retry>,
    pub taskbar_created: u32,
}

impl Icon {
    pub fn new(hwnd: HWND) -> Self {
        let taskbar_created = unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) };
        if taskbar_created != 0 && crate::Windows::elevated() {
            unsafe { let _ = ChangeWindowMessageFilterEx(hwnd, taskbar_created, MSGFLT_ALLOW, None); }
        }
        Self { hwnd, added: Cell::new(false), appearance: Cell::new(None), retry: Cell::new(Retry::Ready), taskbar_created }
    }

    fn data(&self) -> NOTIFYICONDATAW {
        NOTIFYICONDATAW { cbSize: size_of::<NOTIFYICONDATAW>() as u32, hWnd: self.hwnd, uID: 1, ..Default::default() }
    }

    pub fn refresh(&self, tray: &Tray, trigger: Refresh) {
        if trigger == Refresh::Taskbar {
            self.added.set(false);
            self.stop_retry(Retry::Ready);
        }
        if tray.menu().is_none() {
            self.remove();
            self.stop_retry(Retry::Ready);
            tray.set_available(true);
            return;
        }
        let added = self.added.get();
        let now = Instant::now();
        let retry = self.retry.get();
        if added && trigger != Refresh::Theme {
            return;
        }
        if !added && !retry.can_try(trigger, now) {
            if trigger == Refresh::Timer && matches!(retry, Retry::Waiting(_)) {
                self.stop_retry(Retry::Exhausted);
                tray.set_available(false);
            }
            return;
        }
        let light = taskbar_light();
        let (width, height) = icon_size();
        if added && self.appearance.get() == Some((light, width, height)) {
            return;
        }
        let mut data = self.data();
        data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP;
        data.uCallbackMessage = CALLBACK;
        data.hIcon = themed_icon(light, width, height);
        for (dest, ch) in data.szTip.iter_mut().zip("Flit".encode_utf16()) {
            *dest = ch;
        }
        let ok = unsafe {
            let mut ok = !data.hIcon.is_invalid() && Shell_NotifyIconW(if added { NIM_MODIFY } else { NIM_ADD }, &data).as_bool();
            if !ok && !added && trigger == Refresh::Taskbar && !data.hIcon.is_invalid() {
                ok = Shell_NotifyIconW(NIM_MODIFY, &data).as_bool();
            }
            if !data.hIcon.is_invalid() {
                let _ = DestroyIcon(data.hIcon);
            }
            if ok && !added {
                data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
                if !Shell_NotifyIconW(NIM_SETVERSION, &data).as_bool() {
                    let _ = Shell_NotifyIconW(NIM_DELETE, &data);
                    ok = false;
                }
            }
            ok
        };
        if ok {
            self.added.set(true);
            self.appearance.set(Some((light, width, height)));
            self.stop_retry(Retry::Ready);
            tray.set_available(true);
        } else if !added {
            if matches!(retry, Retry::Ready) {
                if unsafe { SetTimer(Some(self.hwnd), RETRY_TIMER, 2_000, None) } != 0 {
                    self.retry.set(Retry::Waiting(now + RETRY_WINDOW));
                } else {
                    self.retry.set(Retry::Exhausted);
                }
            }
            tray.set_available(matches!(self.retry.get(), Retry::Waiting(_)));
        }
    }

    fn stop_retry(&self, state: Retry) {
        if matches!(self.retry.replace(state), Retry::Waiting(_)) {
            unsafe { let _ = KillTimer(Some(self.hwnd), RETRY_TIMER); }
        }
    }

    pub fn message(&self, tray: &Tray, wp: WPARAM, lp: LPARAM) {
        match click(wp, lp) {
            Some(Click::Activate) => tray.send(Action::Activate),
            Some(Click::Menu(x, y)) => {
                if let Some(action) = tray.menu().and_then(|menu| popup(self.hwnd, &menu, x, y)) {
                    tray.send(action);
                }
            }
            None => {}
        }
    }

    fn remove(&self) {
        if self.added.replace(false) {
            unsafe { let _ = Shell_NotifyIconW(NIM_DELETE, &self.data()); }
        }
    }
}

impl Drop for Icon {
    fn drop(&mut self) {
        self.remove();
        self.stop_retry(Retry::Ready);
    }
}

fn taskbar_light() -> bool {
    let mut light = 0u32;
    let mut size = size_of::<u32>() as u32;
    let result = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"), w!("SystemUsesLightTheme"),
            RRF_RT_REG_DWORD, None, Some((&mut light as *mut u32).cast()), Some(&mut size))
    };
    result.is_ok() && light != 0
}

fn icon_size() -> (i32, i32) {
    unsafe {
        let dpi = FindWindowW(w!("Shell_TrayWnd"), PCWSTR::null()).ok().map_or(0, |hwnd| GetDpiForWindow(hwnd));
        let dpi = if dpi == 0 { GetDpiForSystem() } else { dpi };
        (GetSystemMetricsForDpi(SM_CXSMICON, dpi).max(16), GetSystemMetricsForDpi(SM_CYSMICON, dpi).max(16))
    }
}

fn icon_pixels(light: bool, width: usize, height: usize) -> (Vec<u8>, Vec<u8>) {
    let stride = width.div_ceil(16) * 2;
    let mut mask = vec![0xffu8; stride * height];
    let mut pixels = vec![0; width * height * 4];
    let ink = if light { 36 } else { 245 };
    for y in 0..height {
        let base_y = y * 16 / height;
        let edge = if base_y <= 4 { 7 } else { 14 };
        for x in 0..width {
            let base_x = x * 16 / width;
            if (3..13).contains(&base_y) && (2..edge).contains(&base_x)
                && (matches!(base_y, 3 | 5 | 12) || base_x == 2 || base_x == edge - 1)
            {
                mask[y * stride + x / 8] &= !(0x80 >> (x % 8));
                pixels[(y * width + x) * 4..][..4].copy_from_slice(&[ink, ink, ink, 255]);
            }
        }
    }
    (mask, pixels)
}

fn themed_icon(light: bool, width: i32, height: i32) -> HICON {
    let (mask, pixels) = icon_pixels(light, width as usize, height as usize);
    unsafe { CreateIcon(GetModuleHandleW(None).ok().map(Into::into), width, height, 1, 32, mask.as_ptr(), pixels.as_ptr()).unwrap_or_default() }
}

fn wide(label: &str) -> Vec<u16> {
    label.replace('&', "&&").encode_utf16().chain(Some(0)).collect()
}

fn popup(hwnd: HWND, menu: &Menu, x: i32, y: i32) -> Option<Action> {
    unsafe {
        let root = CreatePopupMenu().ok()?;
        let _ = AppendMenuW(root, MF_STRING, 1, PCWSTR(wide(&menu.new_window).as_ptr()));
        if !menu.pinned.is_empty() {
            if let Ok(bookmarks) = CreatePopupMenu() {
                for (i, (_, label)) in menu.pinned.iter().enumerate() {
                    let _ = AppendMenuW(bookmarks, MF_STRING, i + 3, PCWSTR(wide(label).as_ptr()));
                }
                if AppendMenuW(root, MF_POPUP, bookmarks.0 as usize, PCWSTR(wide(&menu.bookmarks).as_ptr())).is_err() {
                    let _ = DestroyMenu(bookmarks);
                }
            }
        }
        let _ = AppendMenuW(root, MF_STRING, 2, PCWSTR(wide(&menu.quit).as_ptr()));
        let mut at = POINT { x, y };
        if x == -1 && y == -1 {
            let _ = GetCursorPos(&mut at);
        }
        let _ = SetForegroundWindow(hwnd);
        let chosen = TrackPopupMenuEx(root, (TPM_RETURNCMD | TPM_RIGHTBUTTON).0, at.x, at.y, hwnd, None).0;
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(root);
        match chosen {
            1 => Some(Action::NewWindow),
            2 => Some(Action::Quit),
            n if n >= 3 => menu.pinned.get(n as usize - 3).map(|(loc, _)| Action::Bookmark(loc.clone())),
            _ => None,
        }
    }
}

#[cfg(test)]
#[path = "../test/tray.rs"]
mod tests;
