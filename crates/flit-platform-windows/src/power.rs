use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Power::{PowerClearRequest, PowerCreateRequest, PowerRequestSystemRequired, PowerSetRequest};
use windows::Win32::System::SystemServices::POWER_REQUEST_CONTEXT_VERSION;
use windows::Win32::System::Threading::{POWER_REQUEST_CONTEXT_SIMPLE_STRING, REASON_CONTEXT, REASON_CONTEXT_0};

pub struct Awake(HANDLE);

impl Awake {
    pub fn hold(reason: &str) -> Option<Self> {
        let mut text: Vec<u16> = reason.encode_utf16().chain(Some(0)).collect();
        let context = REASON_CONTEXT { Version: POWER_REQUEST_CONTEXT_VERSION, Flags: POWER_REQUEST_CONTEXT_SIMPLE_STRING, Reason: REASON_CONTEXT_0 { SimpleReasonString: PWSTR(text.as_mut_ptr()) } };
        let made = unsafe { PowerCreateRequest(&context) }.map(Awake);
        let held = made.and_then(|a| unsafe { PowerSetRequest(a.0, PowerRequestSystemRequired) }.map(|()| a));
        held.map_err(|e| tracing::warn!("cannot keep the system awake: {}", e.message())).ok()
    }
}

impl Drop for Awake {
    fn drop(&mut self) {
        unsafe {
            let _ = PowerClearRequest(self.0, PowerRequestSystemRequired);
            let _ = CloseHandle(self.0);
        }
    }
}

#[cfg(test)]
#[path = "../test/power.rs"]
mod tests;
