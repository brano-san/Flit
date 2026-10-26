use crate::take_pwstr;
use flit_core::preview::{PropGroup, MAX_PROPS};
use flit_core::ui::Bitmap;
use flit_core::FsPath;
use windows::core::{w, GUID, PCWSTR, PWSTR};
use windows::Win32::Foundation::{GENERIC_READ, LPARAM, PROPERTYKEY, WPARAM};
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::StructuredStorage::{PropVariantClear, PropVariantToUInt16, PROPVARIANT};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::UI::Shell::PropertiesSystem::{IPropertyDescription, IPropertyDescriptionList, IPropertyStore, PSFormatForDisplayAlloc, GPS_BESTEFFORT, PDFF_DEFAULT};
use windows::Win32::UI::Shell::{AssocQueryStringW, IShellItem2, SHCreateItemFromParsingName, ASSOCF_NOTRUNCATE, ASSOCSTR_SHELLEXTENSION};
use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, SendMessageTimeoutW, SMTO_ABORTIFHUNG, WM_COPYDATA};

const FULL_DETAILS: PROPERTYKEY = PROPERTYKEY { fmtid: GUID::from_u128(0xc9944a21_a406_48fe_8225_aec7e24c211b), pid: 2 };
const PREVIEW_HANDLER: PCWSTR = w!("{8895b1c6-b41f-4c1c-a562-0d564250836f}");
const SEER_INVOKE: usize = 5000;

fn wide_str(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

pub(crate) fn decode_image(path: &FsPath, max: u32) -> Option<Bitmap> {
    unsafe {
        let factory: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()?;
        let name = path.to_wide(true);
        let decoder = factory.CreateDecoderFromFilename(PCWSTR(name.as_ptr()), None, GENERIC_READ, WICDecodeMetadataCacheOnDemand).ok()?;
        let frame = decoder.GetFrame(0).ok()?;
        let (mut w, mut h) = (0u32, 0u32);
        frame.GetSize(&mut w, &mut h).ok()?;
        if w == 0 || h == 0 {
            return None;
        }
        let k = (max.max(1) as f64 / w.max(h) as f64).min(1.0);
        let (sw, sh) = (((w as f64 * k).round() as u32).max(1), ((h as f64 * k).round() as u32).max(1));
        let scaler = factory.CreateBitmapScaler().ok()?;
        scaler.Initialize(&frame, sw, sh, WICBitmapInterpolationModeFant).ok()?;
        let mut source: IWICBitmapSource = scaler.into();
        let turn = match orientation(&frame) {
            2 => WICBitmapTransformFlipHorizontal,
            3 => WICBitmapTransformRotate180,
            4 => WICBitmapTransformFlipVertical,
            5 => WICBitmapTransformOptions(WICBitmapTransformRotate90.0 | WICBitmapTransformFlipHorizontal.0),
            6 => WICBitmapTransformRotate90,
            7 => WICBitmapTransformOptions(WICBitmapTransformRotate270.0 | WICBitmapTransformFlipHorizontal.0),
            8 => WICBitmapTransformRotate270,
            _ => WICBitmapTransformRotate0,
        };
        if turn != WICBitmapTransformRotate0 {
            let rot = factory.CreateBitmapFlipRotator().ok()?;
            rot.Initialize(&source, turn).ok()?;
            source = rot.into();
        }
        let conv = factory.CreateFormatConverter().ok()?;
        conv.Initialize(&source, &GUID_WICPixelFormat32bppPRGBA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom).ok()?;
        let (mut width, mut height) = (0u32, 0u32);
        conv.GetSize(&mut width, &mut height).ok()?;
        let mut rgba = vec![0u8; (width * height * 4) as usize];
        conv.CopyPixels(std::ptr::null(), width * 4, &mut rgba).ok()?;
        Some(Bitmap { width, height, rgba })
    }
}

unsafe fn orientation(frame: &IWICBitmapFrameDecode) -> u16 {
    let Ok(reader) = frame.GetMetadataQueryReader() else { return 1 };
    let mut v = PROPVARIANT::default();
    if reader.GetMetadataByName(w!("System.Photo.Orientation"), &mut v).is_err() {
        return 1;
    }
    let o = PropVariantToUInt16(&v).unwrap_or(1);
    let _ = PropVariantClear(&mut v);
    o
}

pub(crate) fn properties(path: &FsPath) -> Vec<PropGroup> {
    let mut groups: Vec<PropGroup> = Vec::new();
    unsafe {
        let name = path.to_wide(false);
        let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem2>(PCWSTR(name.as_ptr()), None) else { return groups };
        let (Ok(store), Ok(list)) = (item.GetPropertyStore::<IPropertyStore>(GPS_BESTEFFORT), item.GetPropertyDescriptionList::<IPropertyDescriptionList>(&FULL_DETAILS)) else { return groups };
        let mut rows = 0;
        for i in 0..list.GetCount().unwrap_or(0) {
            let Ok(desc) = list.GetAt::<IPropertyDescription>(i) else { continue };
            let label = desc.GetDisplayName().ok().and_then(|p| take_pwstr(p)).map(|w| String::from_utf16_lossy(&w)).unwrap_or_default();
            let canonical = desc.GetCanonicalName().ok().and_then(|p| take_pwstr(p)).map(|w| String::from_utf16_lossy(&w)).unwrap_or_default();
            if canonical.starts_with("System.PropGroup.") {
                groups.push(PropGroup { title: label, rows: Vec::new() });
                continue;
            }
            let mut key = PROPERTYKEY::default();
            if label.is_empty() || desc.GetPropertyKey(&mut key).is_err() {
                continue;
            }
            let Ok(mut value) = store.GetValue(&key) else { continue };
            let text = PSFormatForDisplayAlloc(&key, &value, PDFF_DEFAULT).ok().and_then(|p: PWSTR| take_pwstr(p)).map(|w| String::from_utf16_lossy(&w));
            let _ = PropVariantClear(&mut value);
            let Some(text) = text.filter(|t| !t.trim().is_empty()) else { continue };
            if groups.is_empty() {
                groups.push(PropGroup::default());
            }
            groups.last_mut().unwrap().rows.push((label, text.replace(['\u{200E}', '\u{200F}'], "")));
            rows += 1;
            if rows == MAX_PROPS {
                break;
            }
        }
    }
    groups.retain(|g| !g.rows.is_empty());
    groups
}

pub(crate) fn handler(ext: &[u8]) -> Option<String> {
    let mut dotted = b".".to_vec();
    dotted.extend_from_slice(ext);
    let mut assoc = Vec::new();
    flit_core::name::to_utf16(&dotted, &mut assoc);
    assoc.push(0);
    let mut buf = [0u16; 64];
    let mut len = buf.len() as u32;
    let hr = unsafe { AssocQueryStringW(ASSOCF_NOTRUNCATE, ASSOCSTR_SHELLEXTENSION, PCWSTR(assoc.as_ptr()), PREVIEW_HANDLER, Some(PWSTR(buf.as_mut_ptr())), &mut len) };
    let n = buf.iter().position(|&c| c == 0).unwrap_or(0);
    (hr.is_ok() && n > 0).then(|| String::from_utf16_lossy(&buf[..n]))
}

fn quick_look() -> Option<std::path::PathBuf> {
    let local = std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from);
    let programs = std::env::var_os("ProgramFiles").map(std::path::PathBuf::from);
    [local.as_ref().map(|d| d.join(r"Programs\QuickLook\QuickLook.exe")), programs.map(|d| d.join(r"QuickLook\QuickLook.exe")), local.map(|d| d.join(r"Microsoft\WindowsApps\QuickLook.exe"))]
        .into_iter()
        .flatten()
        .find(|p| std::fs::symlink_metadata(p).is_ok())
}

const SEER_CLASS: &str = "SeerWindowClass";
const QUICK_LOOK_TOGGLE: &str = "QuickLook.App.PipeMessages.Toggle";

fn quick_look_pipe() -> String {
    format!(r"\\.\pipe\QuickLook.App.Pipe.{}", crate::instance::user_sid().unwrap_or_default())
}

fn quick_look_running(pipe: &str, path: &str) -> bool {
    use std::io::Write;
    std::fs::OpenOptions::new().write(true).open(pipe).and_then(|mut f| f.write_all(format!("{QUICK_LOOK_TOGGLE}|{path}\r\n").as_bytes())).is_ok()
}

fn seer(class: &str, path: &str) -> bool {
    unsafe {
        let class = wide_str(class);
        let Ok(hwnd) = FindWindowW(PCWSTR(class.as_ptr()), PCWSTR::null()) else { return false };
        let text = wide_str(path);
        let data = COPYDATASTRUCT { dwData: SEER_INVOKE, cbData: (text.len() * 2) as u32, lpData: text.as_ptr() as *mut _ };
        let mut result = 0usize;
        SendMessageTimeoutW(hwnd, WM_COPYDATA, WPARAM(0), LPARAM(&data as *const _ as isize), SMTO_ABORTIFHUNG, 1000, Some(&mut result)).0 != 0
    }
}

pub(crate) fn quick_view(path: &FsPath) -> bool {
    let text = path.display().into_owned();
    if seer(SEER_CLASS, &text) || quick_look_running(&quick_look_pipe(), &text) {
        return true;
    }
    quick_look().is_some_and(|exe| std::process::Command::new(exe).arg(&text).spawn().is_ok())
}

#[cfg(test)]
#[path = "../test/preview.rs"]
pub(crate) mod tests;
