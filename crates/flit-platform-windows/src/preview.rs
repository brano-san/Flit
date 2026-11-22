use crate::take_pwstr;
use crate::fs::LinkInfo;
use flit_core::i18n::S;
use flit_core::preview::{PropGroup, MAX_PROPS};
use flit_core::ui::Bitmap;
use flit_core::listing::{ColumnValues, SHELL_PROPS};
use flit_core::FsPath;
use windows::core::{w, Interface, GUID, PCWSTR, PWSTR};
use windows::Win32::Foundation::{GENERIC_READ, LPARAM, PROPERTYKEY, WPARAM};
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::StructuredStorage::{PropVariantClear, PropVariantToUInt16, PropVariantToUInt64, PROPVARIANT};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::UI::Shell::PropertiesSystem::{IPropertyDescription, IPropertyDescriptionList, IPropertyStore, PSFormatForDisplayAlloc, PSGetPropertyKeyFromName, GETPROPERTYSTOREFLAGS, GPS_BESTEFFORT, GPS_FASTPROPERTIESONLY, PDFF_DEFAULT};
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
        let reduced = reduce(&factory, &frame, (w, h), (sw, sh));
        let scaler = factory.CreateBitmapScaler().ok()?;
        match &reduced {
            Some(b) => scaler.Initialize(b, sw, sh, WICBitmapInterpolationModeFant).ok()?,
            None => scaler.Initialize(&frame, sw, sh, WICBitmapInterpolationModeFant).ok()?,
        }
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

unsafe fn reduce(factory: &IWICImagingFactory, frame: &IWICBitmapFrameDecode, (w, h): (u32, u32), (sw, sh): (u32, u32)) -> Option<IWICBitmap> {
    let shrink = [8, 4, 2].into_iter().find(|&d| w / d >= sw && h / d >= sh)?;
    let native: IWICBitmapSourceTransform = frame.cast().ok()?;
    let (mut cw, mut ch) = (w.div_ceil(shrink), h.div_ceil(shrink));
    native.GetClosestSize(&mut cw, &mut ch).ok()?;
    if cw < sw || ch < sh || cw >= w || ch >= h {
        return None;
    }
    let mut format = frame.GetPixelFormat().ok()?;
    native.GetClosestPixelFormat(&mut format).ok()?;
    let info: IWICPixelFormatInfo = factory.CreateComponentInfo(&format).ok()?.cast().ok()?;
    let stride = (cw * info.GetBitsPerPixel().ok()?).div_ceil(32) * 4;
    let mut pixels = vec![0u8; (stride * ch) as usize];
    native.CopyPixels(std::ptr::null(), cw, ch, &format, WICBitmapTransformRotate0, stride, &mut pixels).ok()?;
    factory.CreateBitmapFromMemory(cw, ch, &format, stride, &pixels).ok()
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

pub(crate) fn store_flags(not_local: bool) -> GETPROPERTYSTOREFLAGS {
    if not_local { GPS_FASTPROPERTIESONLY } else { GPS_BESTEFFORT }
}

const SIZE_PROPS: [&str; 2] = ["System.Image.HorizontalSize", "System.Image.VerticalSize"];

fn column_keys() -> &'static [Option<PROPERTYKEY>] {
    static KEYS: std::sync::OnceLock<Vec<Option<PROPERTYKEY>>> = std::sync::OnceLock::new();
    KEYS.get_or_init(|| {
        SHELL_PROPS
            .iter()
            .chain(&SIZE_PROPS)
            .map(|name| {
                let wide = wide_str(name);
                let mut key = PROPERTYKEY::default();
                unsafe { PSGetPropertyKeyFromName(PCWSTR(wide.as_ptr()), &mut key) }.ok().map(|_| key)
            })
            .collect()
    })
}

unsafe fn display_value(store: &IPropertyStore, key: &PROPERTYKEY) -> String {
    let Ok(mut value) = store.GetValue(key) else { return String::new() };
    let text = PSFormatForDisplayAlloc(key, &value, PDFF_DEFAULT).ok().and_then(|p: PWSTR| take_pwstr(p)).map(|w| String::from_utf16_lossy(&w));
    let _ = PropVariantClear(&mut value);
    text.map(|t| t.replace(['\u{200E}', '\u{200F}', '\u{202A}', '\u{202C}'], "").trim().to_owned()).unwrap_or_default()
}

unsafe fn number_value(store: &IPropertyStore, key: Option<PROPERTYKEY>) -> Option<u64> {
    let mut value = store.GetValue(&key?).ok()?;
    let n = PropVariantToUInt64(&value).ok();
    let _ = PropVariantClear(&mut value);
    n
}

pub(crate) fn column_props(path: &FsPath) -> ColumnValues {
    unsafe {
        let name = path.to_wide(false);
        let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem2>(PCWSTR(name.as_ptr()), None) else { return ColumnValues::default() };
        let Ok(store) = item.GetPropertyStore::<IPropertyStore>(store_flags(crate::fs::not_local(path))) else { return ColumnValues::default() };
        let keys = column_keys();
        let text: Vec<String> = keys.iter().take(SHELL_PROPS.len()).map(|k| k.as_ref().map(|k| display_value(&store, k)).unwrap_or_default()).collect();
        let shown = |k: usize| text.get(k).is_some_and(|t| !t.is_empty());
        let side = |k: usize| number_value(&store, keys.get(SHELL_PROPS.len() + k).copied().flatten()).and_then(|n| u32::try_from(n).ok());
        let (width, height) = if shown(0) { (side(0), side(1)) } else { (None, None) };
        let duration = if shown(1) { number_value(&store, keys.get(1).copied().flatten()) } else { None };
        ColumnValues { text, width, height, duration }
    }
}

pub(crate) fn link_group(link: &LinkInfo) -> PropGroup {
    let row = |label: S, value: String| (label.t().to_owned(), value);
    let target = |t: &Option<FsPath>| t.as_ref().map(|t| row(S::PropertiesLinkTarget, t.display().into_owned()));
    let rows = match link {
        LinkInfo::Symbolic(t) => [Some(row(S::PropertiesLinkType, S::PropertiesSymbolicLink.t().to_owned())), target(t)],
        LinkInfo::Junction(t) => [Some(row(S::PropertiesLinkType, S::PropertiesJunction.t().to_owned())), target(t)],
        LinkInfo::Hard(n) => [Some(row(S::PropertiesLinkType, S::PropertiesHardLink.t().to_owned())), Some(row(S::PropertiesHardLinks, n.to_string()))],
    };
    PropGroup { title: S::PropertiesLink.t().to_owned(), rows: rows.into_iter().flatten().collect() }
}

pub(crate) const BLOCKING_ZONE: u32 = 3;

pub(crate) fn blocked(path: &FsPath) -> bool {
    use flit_archive::zone;
    let file = crate::fileops::std_path(path);
    !file.is_dir() && zone::read(&file).and_then(|b| zone::zone_id(&b)).is_some_and(|z| z >= BLOCKING_ZONE)
}

pub(crate) fn security_group() -> PropGroup {
    PropGroup { title: S::PropertiesSecurity.t().to_owned(), rows: vec![(S::PropertiesSecurity.t().to_owned(), S::PropertiesBlocked.t().to_owned())] }
}

pub(crate) fn properties(path: &FsPath) -> Vec<PropGroup> {
    let link = crate::fs::link_info(path).map(|l| link_group(&l));
    link.into_iter().chain(shell_properties(path)).chain(blocked(path).then(security_group)).collect()
}

fn shell_properties(path: &FsPath) -> Vec<PropGroup> {
    let mut groups: Vec<PropGroup> = Vec::new();
    unsafe {
        let name = path.to_wide(false);
        let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem2>(PCWSTR(name.as_ptr()), None) else { return groups };
        let (Ok(store), Ok(list)) = (item.GetPropertyStore::<IPropertyStore>(store_flags(crate::fs::not_local(path))), item.GetPropertyDescriptionList::<IPropertyDescriptionList>(&FULL_DETAILS)) else { return groups };
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
            let row = (label, text.replace(['\u{200E}', '\u{200F}'], ""));
            match groups.last_mut() {
                Some(group) => group.rows.push(row),
                None => groups.push(PropGroup { rows: vec![row], ..Default::default() }),
            }
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
