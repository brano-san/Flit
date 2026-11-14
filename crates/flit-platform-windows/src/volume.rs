use flit_core::cmdline::quote_windows;
use flit_core::i18n::S;
use flit_core::input::Event;
use flit_core::FsPath;
use flit_platform::{OpOutcome, PlatformError};
use std::ffi::OsString;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, ERROR_ACCESS_DENIED, ERROR_CANCELLED, ERROR_INVALID_PARAMETER, HWND, WAIT_OBJECT_0, WIN32_ERROR};
use windows::Win32::Storage::FileSystem::SetVolumeLabelW;
use windows::Win32::System::Threading::{GetCurrentProcess, GetExitCodeProcess, WaitForSingleObject};
use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;

pub const FLAG: &str = "--label-volume";
const ELEVATED_WAIT_MS: u32 = 30_000;

pub(crate) enum Outcome {
    Done,
    Elevate,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn set(drive: &FsPath, label: &str) -> Result<(), WIN32_ERROR> {
    unsafe { SetVolumeLabelW(PCWSTR(drive.to_wide(false).as_ptr()), PCWSTR(wide(label).as_ptr())) }.map_err(|e| WIN32_ERROR::from_error(&e).unwrap_or(ERROR_INVALID_PARAMETER))
}

pub(crate) fn failure(code: WIN32_ERROR) -> String {
    flit_core::tr_en!(S::ErrorsEngineVolumeLabelFailed, error = windows::core::Error::from_hresult(code.to_hresult()).message())
}

pub(crate) fn label(drive: &FsPath, label: &str, elevated: bool) -> Result<Outcome, String> {
    match set(drive, label) {
        Ok(()) => Ok(Outcome::Done),
        Err(ERROR_ACCESS_DENIED) if !elevated => Ok(Outcome::Elevate),
        Err(code) => Err(failure(code)),
    }
}

pub(crate) fn label_elevated(owner: HWND, drive: &FsPath, label: &str) -> Event {
    let Ok(exe) = std::env::current_exe() else { return Event::ShellError(failure(ERROR_ACCESS_DENIED)) };
    let (file, args) = (wide(&exe.to_string_lossy()), wide(&format!("{FLAG} {} {}", quote_windows(&drive.display()), quote_windows(label))));
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
        hwnd: owner,
        lpVerb: w!("runas"),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(args.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    if let Err(e) = unsafe { ShellExecuteExW(&mut info) } {
        return match WIN32_ERROR::from_error(&e) {
            Some(ERROR_CANCELLED) => Event::DevicesChanged,
            code => Event::ShellError(failure(code.unwrap_or(ERROR_ACCESS_DENIED))),
        };
    }
    let mut code = 0u32;
    unsafe {
        let finished = WaitForSingleObject(info.hProcess, ELEVATED_WAIT_MS) == WAIT_OBJECT_0;
        let read = finished && GetExitCodeProcess(info.hProcess, &mut code).is_ok();
        let _ = CloseHandle(info.hProcess);
        if !read {
            return Event::DevicesChanged;
        }
    }
    match code {
        0 => Event::DevicesChanged,
        c => Event::ShellError(failure(WIN32_ERROR(c))),
    }
}

pub fn label_from_command_line(drive: Option<OsString>, label: Option<OsString>) -> i32 {
    let drive = drive.and_then(|d| FsPath::parse(&d.to_string_lossy()));
    let (Some(drive), Some(label)) = (drive, label) else { return ERROR_INVALID_PARAMETER.0 as i32 };
    match set(&drive, &label.to_string_lossy()) {
        Ok(()) => 0,
        Err(code) => code.0 as i32,
    }
}

pub(crate) fn grant_args(dir: &FsPath, sid: &str) -> String {
    format!("{} /grant *{sid}:(OI)(CI)RX", quote_windows(&dir.display()))
}

fn grant(dir: &FsPath) -> Result<(), PlatformError> {
    let sid = crate::instance::process_user_sid(unsafe { GetCurrentProcess() }).ok_or_else(|| PlatformError::new(-1, "cannot read the current user"))?;
    let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
    let (file, args) = (wide(&format!(r"{root}\System32\icacls.exe")), wide(&grant_args(dir, &sid)));
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
        hwnd: crate::shell::owner_window().unwrap_or_default(),
        lpVerb: w!("runas"),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(args.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    if let Err(e) = unsafe { ShellExecuteExW(&mut info) } {
        return Err(match WIN32_ERROR::from_error(&e) {
            Some(ERROR_CANCELLED) => PlatformError::new(ERROR_CANCELLED.0 as i64, S::ErrorsEngineCancelled.en()),
            _ => crate::err(e),
        });
    }
    let mut code = 0u32;
    unsafe {
        let finished = WaitForSingleObject(info.hProcess, ELEVATED_WAIT_MS) == WAIT_OBJECT_0;
        let read = finished && GetExitCodeProcess(info.hProcess, &mut code).is_ok();
        let _ = CloseHandle(info.hProcess);
        if !read {
            return Err(PlatformError::new(-1, format!("icacls {}", dir.display())));
        }
    }
    match code {
        0 => Ok(()),
        c => Err(PlatformError::new(i64::from(c), format!("icacls {}: {}", dir.display(), windows::core::Error::from_hresult(WIN32_ERROR(c).to_hresult()).message()))),
    }
}

pub(crate) fn grant_access(dir: &FsPath) -> OpOutcome {
    match grant(dir) {
        Ok(()) => OpOutcome { items: vec![(dir.clone(), dir.clone())], error: None },
        Err(e) => OpOutcome { items: Vec::new(), error: Some(e) },
    }
}

#[cfg(test)]
#[path = "../test/volume.rs"]
mod tests;
