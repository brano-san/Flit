use crate::err;
use flit_platform::PlatformError;
use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;
use windows::core::{w, PCWSTR, PWSTR};
use windows::Win32::Foundation::{ERROR_CALL_NOT_IMPLEMENTED, ERROR_FAILED_SERVICE_CONTROLLER_CONNECT, ERROR_SERVICE_SPECIFIC_ERROR, NO_ERROR, WIN32_ERROR};
use windows::Win32::System::Services::*;

pub(crate) const SERVICE_NAME: PCWSTR = w!("flit-indexd");
const PENDING_HINT_MS: u32 = 5000;

static BODY: OnceLock<fn() -> Result<(), PlatformError>> = OnceLock::new();
static STATUS: AtomicUsize = AtomicUsize::new(0);

fn report(state: SERVICE_STATUS_CURRENT_STATE, failed: bool) {
    let accepted = if state == SERVICE_RUNNING { SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SHUTDOWN } else { 0 };
    let pending = state == SERVICE_START_PENDING || state == SERVICE_STOP_PENDING;
    let status = SERVICE_STATUS {
        dwServiceType: SERVICE_WIN32_OWN_PROCESS,
        dwCurrentState: state,
        dwControlsAccepted: accepted,
        dwWin32ExitCode: if failed { ERROR_SERVICE_SPECIFIC_ERROR.0 } else { NO_ERROR.0 },
        dwServiceSpecificExitCode: failed as u32,
        dwCheckPoint: 0,
        dwWaitHint: if pending { PENDING_HINT_MS } else { 0 },
    };
    let handle = SERVICE_STATUS_HANDLE(STATUS.load(Ordering::Acquire) as *mut c_void);
    let _ = unsafe { SetServiceStatus(handle, &status) };
}

unsafe extern "system" fn control(code: u32, _: u32, _: *mut c_void, _: *mut c_void) -> u32 {
    match code {
        SERVICE_CONTROL_STOP | SERVICE_CONTROL_SHUTDOWN => {
            report(SERVICE_STOP_PENDING, false);
            std::thread::spawn(|| {
                report(SERVICE_STOPPED, false);
                std::process::exit(0);
            });
            NO_ERROR.0
        }
        SERVICE_CONTROL_INTERROGATE => NO_ERROR.0,
        _ => ERROR_CALL_NOT_IMPLEMENTED.0,
    }
}

unsafe extern "system" fn service_main(_: u32, _: *mut PWSTR) {
    let Ok(handle) = (unsafe { RegisterServiceCtrlHandlerExW(SERVICE_NAME, Some(control), None) }) else { return };
    STATUS.store(handle.0 as usize, Ordering::Release);
    report(SERVICE_START_PENDING, false);
    report(SERVICE_RUNNING, false);
    let failed = BODY.get().is_some_and(|body| body().is_err());
    report(SERVICE_STOPPED, failed);
}

pub(crate) fn run(body: fn() -> Result<(), PlatformError>) -> Result<(), PlatformError> {
    let _ = BODY.set(body);
    let mut name: Vec<u16> = unsafe { SERVICE_NAME.as_wide() }.iter().copied().chain(Some(0)).collect();
    let table = [SERVICE_TABLE_ENTRYW { lpServiceName: PWSTR(name.as_mut_ptr()), lpServiceProc: Some(service_main) }, SERVICE_TABLE_ENTRYW::default()];
    unsafe { StartServiceCtrlDispatcherW(table.as_ptr()) }.map_err(|e| match WIN32_ERROR::from_error(&e) {
        Some(ERROR_FAILED_SERVICE_CONTROLLER_CONNECT) => PlatformError::new(e.code().0 as i64, "this process was not started by the service control manager; run flit-indexd --console in a terminal"),
        _ => err(e),
    })
}
