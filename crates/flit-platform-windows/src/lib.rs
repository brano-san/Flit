#![cfg(windows)]

mod d3d;
mod dwrite;
mod fs;
mod instance;
mod places;
mod shell;
mod watcher;
mod window;

pub use d3d::{D3dDevice, D3dRenderer};
pub use dwrite::DWriteText;
pub use fs::NtFs;
pub use instance::PipeInstance;
pub use shell::ShellCom;
pub use watcher::DirWatcher;
pub use window::{Win32, Win32Waker, Win32Window};

use flit_core::{FsPath, Location};
use flit_platform::{IndexSource, Platform, PlatformError, Service};
use flit_search::index::{Checkpoint, IndexSink};
use flit_search::proto::VolumeInfo;
use windows::core::{w, PCWSTR, PWSTR};
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Dwm::{DwmGetCompositionTimingInfo, DWM_TIMING_INFO};
use windows::Win32::System::Com::{CoInitializeEx, CoTaskMemFree, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE};
use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
use windows::Win32::System::Time::{GetTimeZoneInformation, TIME_ZONE_INFORMATION};
use windows::Win32::UI::Shell::{IShellItem, SHCreateItemFromParsingName, SIGDN_FILESYSPATH};
use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};

pub(crate) fn err(e: windows::core::Error) -> PlatformError {
    PlatformError::new(e.code().0 as i64, e.message())
}

pub(crate) fn dark_mode() -> bool {
    let mut v = 1u32;
    let mut n = 4u32;
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut v as *mut u32 as _),
            Some(&mut n),
        )
    };
    r.is_ok() && v == 0
}

pub(crate) fn com_sta() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
    }
}

pub(crate) unsafe fn take_pwstr(p: PWSTR) -> Option<Vec<u16>> {
    if p.is_null() {
        return None;
    }
    let v = p.as_wide().to_vec();
    CoTaskMemFree(Some(p.0 as _));
    Some(v)
}

pub struct Windows;

impl Platform for Windows {
    type WindowSystem = Win32;
    type Renderer = D3dRenderer;
    type Text = DWriteText;
    type Fs = NtFs;
    type Watcher = DirWatcher;
    type Shell = ShellCom;
    type Instance = PipeInstance;
    type IndexSource = MftIndex;
    type Service = WinService;

    fn text() -> Result<DWriteText, PlatformError> {
        DWriteText::new()
    }

    fn fs() -> NtFs {
        NtFs
    }

    fn utc_offset_minutes() -> i32 {
        let mut tz = TIME_ZONE_INFORMATION::default();
        let r = unsafe { GetTimeZoneInformation(&mut tz) };
        let extra = match r {
            1 => tz.StandardBias,
            2 => tz.DaylightBias,
            _ => 0,
        };
        -(tz.Bias + extra)
    }

    fn dark_mode() -> bool {
        dark_mode()
    }

    fn state_dir() -> Option<std::path::PathBuf> {
        std::env::var_os("LOCALAPPDATA").map(|d| std::path::PathBuf::from(d).join("Flit"))
    }

    fn home() -> FsPath {
        std::env::var("USERPROFILE").ok().and_then(|h| FsPath::parse(&h)).unwrap_or_else(FsPath::computer)
    }

    fn known_folder(name: &str) -> Option<Location> {
        com_sta();
        let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        unsafe {
            let item: IShellItem = SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None).ok()?;
            let path = take_pwstr(item.GetDisplayName(SIGDN_FILESYSPATH).ok()?)?;
            FsPath::from_utf16(&path).map(Location::from_local)
        }
    }

    fn fatal_error(message: &str) {
        let text: Vec<u16> = message.encode_utf16().chain(Some(0)).collect();
        unsafe {
            MessageBoxW(None, PCWSTR(text.as_ptr()), w!("Flit"), MB_ICONERROR | MB_OK);
        }
    }

    fn refresh_period() -> Option<std::time::Duration> {
        let mut info = DWM_TIMING_INFO { cbSize: size_of::<DWM_TIMING_INFO>() as u32, ..Default::default() };
        unsafe { DwmGetCompositionTimingInfo(HWND::default(), &mut info) }.ok()?;
        let rate = info.rateRefresh;
        (rate.uiNumerator > 0 && rate.uiDenominator > 0).then(|| std::time::Duration::from_secs_f64(rate.uiDenominator as f64 / rate.uiNumerator as f64))
    }

    fn drives() -> Vec<flit_core::ui::Drive> {
        places::drives()
    }

    fn wsl_distros() -> Vec<String> {
        places::wsl_distros()
    }
}

pub struct MftIndex;

impl IndexSource for MftIndex {
    fn volumes(&self) -> Result<Vec<VolumeInfo>, PlatformError> {
        Ok(Vec::new())
    }
    fn scan<S: IndexSink>(&mut self, _: &str, _: &mut S) -> Result<Checkpoint, PlatformError> {
        Err(PlatformError::unsupported("MFT scan (stage 2)"))
    }
    fn changes<S: IndexSink>(&mut self, _: &str, _: Checkpoint, _: &mut S) -> Result<Checkpoint, PlatformError> {
        Err(PlatformError::unsupported("USN journal (stage 2)"))
    }
}

pub struct WinService;

impl Service for WinService {
    fn endpoint() -> String {
        r"\\.\pipe\flit-indexd".into()
    }
    fn run(body: fn() -> Result<(), PlatformError>) -> Result<(), PlatformError> {
        body()
    }
}
