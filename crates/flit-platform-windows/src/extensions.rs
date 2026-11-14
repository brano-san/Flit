use crate::integration::read;
use std::sync::OnceLock;
use windows::core::PWSTR;
use windows::Win32::System::Registry::{RegEnumKeyExW, HKEY_CLASSES_ROOT};

const FLAG: &str = "NeverShowExt";

static HIDDEN: OnceLock<Vec<String>> = OnceLock::new();

pub(crate) fn hidden_by(classes: &[(String, Option<String>)], flagged: impl Fn(&str) -> bool) -> Vec<String> {
    let mut out: Vec<String> = classes
        .iter()
        .filter(|(ext, class)| ext.len() > 1 && (flagged(ext) || class.as_deref().is_some_and(|c| !c.is_empty() && flagged(c))))
        .map(|(ext, _)| ext[1..].to_ascii_lowercase())
        .collect();
    out.sort();
    out.dedup();
    out
}

fn text(data: &[u8]) -> String {
    let units: Vec<u16> = data.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&c| c != 0).collect();
    String::from_utf16_lossy(&units)
}

fn extensions() -> Vec<String> {
    let mut out = Vec::new();
    let mut buf = [0u16; 256];
    for index in 0.. {
        let mut len = buf.len() as u32;
        let r = unsafe { RegEnumKeyExW(HKEY_CLASSES_ROOT, index, Some(PWSTR(buf.as_mut_ptr())), &mut len, None, None, None, None) };
        if r.is_err() {
            break;
        }
        let name = String::from_utf16_lossy(&buf[..len as usize]);
        if name.starts_with('.') {
            out.push(name);
        }
    }
    out
}

fn scan() -> Vec<String> {
    let flagged = |key: &str| read(HKEY_CLASSES_ROOT, key, FLAG).is_some();
    let classes: Vec<(String, Option<String>)> = extensions().into_iter().map(|ext| (ext.clone(), read(HKEY_CLASSES_ROOT, &ext, "").map(|(_, d)| text(&d)))).collect();
    hidden_by(&classes, flagged)
}

pub fn never_shown() -> Vec<String> {
    HIDDEN.get_or_init(scan).clone()
}

#[cfg(test)]
#[path = "../test/extensions.rs"]
mod tests;
