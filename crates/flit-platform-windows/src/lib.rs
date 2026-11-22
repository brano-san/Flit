#![cfg(windows)]

mod access;
pub mod aumid;
mod busy;
mod clipboard;
mod context_menu;
pub mod crash;
pub mod diagnose;
mod d3d;
mod digest;
mod dragdrop;
mod dwrite;
mod engine;
mod extensions;
mod fileops;
mod fs;
mod hdr_effect;
mod hdr_gpu;
mod update;
mod install;
mod integration;
mod instance;
mod jumplist;
mod library;
mod links;
mod mft;
pub mod disk_changes;
mod namespace;
mod network;
mod pipe;
mod places;
mod power;
mod preview;
mod prevhost;
mod quick;
mod recycle;
mod service;
mod shell;
mod shellwin;
mod tags;
mod toast;
mod transfer;
mod uia;
mod volume;
mod walk_index;
mod watcher;
mod window;
mod tray;
mod winsearch;
mod wsl;
mod io_priority;

pub use d3d::{D3dDevice, D3dRenderer};
pub use dwrite::DWriteText;
pub use fs::NtFs;
pub use instance::PipeInstance;
pub use mft::MftIndex;
pub use pipe::PipeConn;
pub use shell::ShellCom;
pub use shellwin::raise_explorer;
pub use volume::label_from_command_line;
pub use links::symbolic_from_command_line;
pub use walk_index::WalkIndex;
pub use watcher::DirWatcher;
pub use window::{Win32, Win32Waker, Win32Window};
pub use wsl::{native_arm64, WslLauncher};

use flit_core::{FsPath, Location};
use flit_platform::{OnClient, Platform, PlatformError, Service};
use windows::core::{w, PCWSTR, PWSTR};
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Dwm::{DwmGetCompositionTimingInfo, DWM_TIMING_INFO};
use windows::Win32::System::Com::{CoInitializeEx, CoTaskMemFree, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE};
use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
use windows::Win32::System::Time::{GetTimeZoneInformation, TIME_ZONE_INFORMATION};
use windows::Win32::UI::Input::KeyboardAndMouse::GetDoubleClickTime;
use windows::Win32::UI::Shell::{IShellItem, SHCreateItemFromParsingName, SIGDN_FILESYSPATH};
use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, SystemParametersInfoW, MB_ICONERROR, MB_OK, SPI_GETWHEELSCROLLLINES, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS};

pub(crate) fn err(e: windows::core::Error) -> PlatformError {
    win_error(e.code().0 as i64, error_reason(&e))
}

pub(crate) fn win_error(code: i64, message: impl Into<String>) -> PlatformError {
    use flit_platform::PlatformErrorKind;
    use windows::Win32::Foundation::*;
    let raw = code as u32;
    let win32 = WIN32_ERROR(if raw >> 16 == 0x8007 { raw & 0xFFFF } else { raw });
    let kind = match win32 {
        ERROR_CANCELLED | ERROR_OPERATION_ABORTED | WIN32_ERROR(0x8027_0000) | WIN32_ERROR(0x8000_4004) => PlatformErrorKind::Cancelled,
        ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND | ERROR_INVALID_NAME | ERROR_BAD_PATHNAME | ERROR_BAD_NETPATH | ERROR_BAD_NET_NAME => PlatformErrorKind::NotFound,
        ERROR_ACCESS_DENIED | ERROR_PRIVILEGE_NOT_HELD | ERROR_WRITE_PROTECT => PlatformErrorKind::AccessDenied,
        ERROR_ALREADY_EXISTS | ERROR_FILE_EXISTS => PlatformErrorKind::Exists,
        ERROR_DISK_FULL | ERROR_HANDLE_DISK_FULL | ERROR_DISK_QUOTA_EXCEEDED => PlatformErrorKind::NoSpace,
        ERROR_BUSY | ERROR_SHARING_VIOLATION | ERROR_LOCK_VIOLATION => PlatformErrorKind::Busy,
        _ => PlatformErrorKind::Other,
    };
    PlatformError::new(code, message).with_kind(kind)
}

pub(crate) fn error_reason(e: &windows::core::Error) -> String {
    flit_core::i18n::os_error(e.code().0).map_or_else(|| e.message(), |s| s.en().to_owned())
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

pub(crate) fn double_click_time() -> std::time::Duration {
    std::time::Duration::from_millis(unsafe { GetDoubleClickTime() }.into())
}

pub(crate) fn wheel_lines() -> u32 {
    let mut lines = 3u32;
    let read = unsafe { SystemParametersInfoW(SPI_GETWHEELSCROLLLINES, 0, Some(&mut lines as *mut u32 as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0)) };
    if read.is_ok() { lines } else { 3 }
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

impl Windows {
    fn standard_places() -> Vec<FsPath> {
        flit_core::ui::KnownFolder::ALL.iter().filter_map(|k| Self::known_folder(k.shell_name())).filter_map(|l| l.to_windows()).collect()
    }
}

impl flit_platform::IndexPlatform for Windows {
    type Source = <Self as Platform>::IndexSource;
    type Walk = <Self as Platform>::FallbackIndex;
    type Service = <Self as Platform>::Service;

    fn walk_source(roots: &[String]) -> Result<Self::Walk, PlatformError> {
        if roots.is_empty() {
            return <Self::Walk as flit_platform::IndexSource>::new();
        }
        let parsed: Option<Vec<_>> = roots.iter().map(|r| FsPath::parse(r)).collect();
        let parsed = parsed.ok_or_else(|| PlatformError::new(-1, "a --walk root is not an absolute path"))?;
        Ok(WalkIndex::with_roots(parsed))
    }

    fn utc_offset_minutes() -> i32 {
        <Self as Platform>::utc_offset_minutes()
    }

    fn state_dir() -> Option<std::path::PathBuf> {
        <Self as Platform>::state_dir()
    }
}

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
    type FallbackIndex = WalkIndex;

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

    fn recycle_bin() -> bool {
        true
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

    fn business_onedrive() -> Vec<(String, Location)> {
        places::business_onedrive()
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

    fn templates() -> Vec<flit_core::ui::NewTemplate> {
        places::templates()
    }

    fn libraries() -> Vec<flit_core::library::Library> {
        library::libraries()
    }

    fn library(op: &flit_core::library::LibraryOp) -> Result<(), PlatformError> {
        library::apply(op)
    }

    fn recent(limit: usize) -> (Vec<flit_core::ui::Recent>, bool) {
        places::recent(limit)
    }

    fn bookmarks() -> Vec<Location> {
        quick::pinned_folders(&Self::standard_places())
    }

    fn double_click_time() -> std::time::Duration {
        double_click_time()
    }

    fn wheel_lines() -> u32 {
        wheel_lines()
    }

    fn elevated() -> bool {
        instance::elevated()
    }

    fn date_patterns() -> (String, String) {
        places::date_patterns()
    }

    fn os_version() -> String {
        places::os_version()
    }

    fn network_warning() -> Option<flit_core::provider::NetworkWarning> {
        network::discovery_warning(network::discovery_enabled)
    }

    fn ui_language() -> String {
        places::ui_language()
    }

    fn integrate(on: bool) -> Result<(), PlatformError> {
        integration::integrate(on)
    }

    fn tidy_integration() {
        integration::tidy()
    }

    fn watch_wsl(changed: Box<dyn Fn() + Send>) {
        places::watch_lxss(changed)
    }

    fn wsl_launcher(distro: &str, images: &flit_platform::AgentImages) -> Option<Box<dyn flit_platform::AgentLauncher>> {
        wsl::launcher(distro, images)
    }

    fn file_key(path: &FsPath) -> Option<flit_core::tags::FileKey> {
        tags::file_key(path)
    }

    fn shell_search(folder: Option<&FsPath>, query: &str, keep_going: &dyn Fn() -> bool, found: &mut dyn FnMut(FsPath, flit_core::Meta)) -> Result<(), PlatformError> {
        winsearch::search(folder, query, keep_going, found)
    }

    fn resolve_files(files: &[(flit_core::tags::FileKey, &FsPath)]) -> Vec<flit_platform::Resolved> {
        tags::resolve_files(&files.iter().map(|f| f.0).collect::<Vec<_>>())
    }

    fn download(url: &str, out: &mut dyn std::io::Write, progress: &mut dyn FnMut(u64, Option<u64>) -> bool) -> Result<(), PlatformError> {
        update::download(url, out, progress)
    }

    fn wait_exit(pid: u32, limit: std::time::Duration) {
        update::wait_exit(pid, limit)
    }

    fn keep_awake(reason: &str) -> Option<flit_platform::Awake> {
        power::Awake::hold(reason).map(|a| Box::new(a) as flit_platform::Awake)
    }

    fn jump_list(pinned: &[Location], frequent: &[Location]) {
        jumplist::update(pinned, frequent)
    }

    fn quick_access() -> Option<Box<dyn flit_core::quick_sync::QuickAccess>> {
        Some(Box::new(quick::ShellQuickAccess::new(Self::standard_places())))
    }

    fn never_shown_extensions() -> Vec<String> {
        extensions::never_shown()
    }

    fn run_elevated(args: &[String]) -> Result<u32, PlatformError> {
        update::run_elevated(args)
    }
}

const PIPE_NAME: &str = r"\\.\pipe\flit-indexd";

pub struct WinService;

impl Service for WinService {
    type Conn = PipeConn;
    fn endpoint() -> String {
        PIPE_NAME.into()
    }
    fn run(body: fn() -> Result<(), PlatformError>) -> Result<(), PlatformError> {
        service::run(body)
    }
    fn connect() -> Result<PipeConn, PlatformError> {
        pipe::connect_to(&Self::endpoint())
    }
    fn listen(max_clients: usize, on_client: OnClient<PipeConn>) -> Result<(), PlatformError> {
        pipe::listen_at(&Self::endpoint(), max_clients, on_client)
    }
}

#[cfg(test)]
#[path = "../test/bench.rs"]
mod bench;

#[cfg(test)]
fn test_temp() -> std::path::PathBuf {
    let temp = std::env::temp_dir();
    let long = std::fs::canonicalize(&temp).ok().and_then(|p| p.to_str().and_then(|s| s.strip_prefix(r"\\?\")).map(std::path::PathBuf::from));
    long.unwrap_or(temp)
}

#[cfg(test)]
#[path = "../test/dedup.rs"]
mod dedup_tests;
