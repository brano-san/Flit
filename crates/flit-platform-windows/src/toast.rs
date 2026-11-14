use crate::integration::{read, write};
use crossbeam_channel::{unbounded, Sender};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use windows::core::{IInspectable, HSTRING};
use windows::Data::Xml::Dom::XmlDocument;
use windows::Foundation::TypedEventHandler;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
use windows::Win32::System::Registry::{HKEY, HKEY_CURRENT_USER, REG_SZ};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};
use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};

pub const APP_ID: &str = "Flit.FileManager";
pub(crate) const WM_TOAST_CLICKED: u32 = WM_APP + 0x48;
const ICON: &[u8] = include_bytes!("../../../assets/icon/hicolor/128x128/apps/flit.png");
const ICON_FILE: &str = "notification.png";
const KEPT: usize = 8;

static WORKER: OnceLock<Option<Sender<(isize, String)>>> = OnceLock::new();

pub(crate) fn registration_key(classes: &str) -> String {
    format!(r"{classes}\AppUserModelId\{APP_ID}")
}

pub(crate) fn registration(icon: &Path) -> [(&'static str, String); 2] {
    [("DisplayName", "Flit".to_owned()), ("IconUri", icon.to_string_lossy().into_owned())]
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c if (c as u32) < 0x20 && !matches!(c, '\t' | '\n' | '\r') => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

pub(crate) fn toast_xml(text: &str) -> String {
    format!("<toast><visual><binding template=\"ToastGeneric\"><text>Flit</text><text>{}</text></binding></visual></toast>", escape(text))
}

fn sz(s: &str) -> Vec<u8> {
    s.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect()
}

pub(crate) fn register(hive: HKEY, classes: &str, icon: &Path) -> Result<bool, flit_platform::PlatformError> {
    let key = registration_key(classes);
    let mut changed = false;
    for (name, value) in registration(icon) {
        let data = sz(&value);
        if read(hive, &key, name) != Some((REG_SZ.0, data.clone())) {
            write(hive, &key, name, REG_SZ.0, &data)?;
            changed = true;
        }
    }
    Ok(changed)
}

pub(crate) fn keep_icon(dir: &Path) -> std::io::Result<PathBuf> {
    let path = dir.join(ICON_FILE);
    if std::fs::read(&path).ok().as_deref() != Some(ICON) {
        std::fs::create_dir_all(dir)?;
        std::fs::write(&path, ICON)?;
    }
    Ok(path)
}

fn icon_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("Flit"))
}

fn prepare() -> bool {
    let Some(icon) = icon_dir().map(|d| keep_icon(&d)) else { return false };
    let icon = match icon {
        Ok(i) => i,
        Err(e) => {
            tracing::warn!("cannot keep the notification icon: {e}");
            return false;
        }
    };
    match register(HKEY_CURRENT_USER, r"Software\Classes", &icon) {
        Ok(changed) => {
            if changed {
                tracing::info!("registered {APP_ID} for notifications");
            }
            true
        }
        Err(e) => {
            tracing::warn!("cannot register {APP_ID} for notifications: {e}");
            false
        }
    }
}

fn show(owner: isize, text: &str) -> windows::core::Result<ToastNotification> {
    let doc = XmlDocument::new()?;
    doc.LoadXml(&HSTRING::from(toast_xml(text)))?;
    let toast = ToastNotification::CreateToastNotification(&doc)?;
    toast.Activated(&TypedEventHandler::<ToastNotification, IInspectable>::new(move |_, _| {
        unsafe {
            let _ = PostMessageW(Some(HWND(owner as _)), WM_TOAST_CLICKED, WPARAM(0), LPARAM(0));
        }
        Ok(())
    }))?;
    ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(APP_ID))?.Show(&toast)?;
    Ok(toast)
}

fn work(rx: crossbeam_channel::Receiver<(isize, String)>) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    if !prepare() {
        return;
    }
    let mut shown: std::collections::VecDeque<ToastNotification> = std::collections::VecDeque::new();
    for (owner, text) in rx {
        match show(owner, &text) {
            Ok(t) => {
                shown.push_back(t);
                if shown.len() > KEPT {
                    shown.pop_front();
                }
            }
            Err(e) => tracing::warn!("cannot show a notification: {}", e.message()),
        }
    }
}

pub(crate) fn notify(owner: HWND, text: &str) {
    let worker = WORKER.get_or_init(|| {
        let (tx, rx) = unbounded();
        std::thread::Builder::new().name("flit-notifications".into()).spawn(move || work(rx)).ok().map(|_| tx)
    });
    if let Some(tx) = worker {
        let _ = tx.send((owner.0 as isize, text.to_owned()));
    }
}

#[cfg(test)]
#[path = "../test/toast.rs"]
mod tests;
