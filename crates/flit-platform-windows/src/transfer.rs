use crate::clipboard::{effect_is_move, global_bytes};
use crate::fileops::{exists, std_path};
use crate::{com_sta, err};
use flit_core::i18n::S;
use flit_core::provider::Resolve;
use flit_core::FsPath;
use flit_platform::{Job, OpOutcome, PlatformError};
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::Path;
use std::time::{Duration, SystemTime};
use windows::core::PCWSTR;
use windows::Win32::Foundation::S_OK;
use windows::Win32::System::Com::StructuredStorage::{StgCreateDocfile, IStorage};
use windows::Win32::System::Com::{IDataObject, IStream, DVASPECT_CONTENT, FORMATETC, STGC_DEFAULT, STGM_FAILIFTHERE, STGM_READWRITE, STGM_SHARE_EXCLUSIVE, TYMED_HGLOBAL, TYMED_ISTORAGE, TYMED_ISTREAM};
use windows::Win32::System::DataExchange::{IsClipboardFormatAvailable, RegisterClipboardFormatW};
use windows::Win32::System::Ole::{OleGetClipboard, OleInitialize, ReleaseStgMedium};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{IShellItem, SHCreateItemFromIDList, CFSTR_FILECONTENTS, CFSTR_FILEDESCRIPTORW, CFSTR_INETURLA, CFSTR_INETURLW, CFSTR_PREFERREDDROPEFFECT, CFSTR_SHELLIDLIST};

const DESCRIPTOR: usize = 592;
const DESCRIPTOR_NAME_AT: usize = 72;
const FD_ATTRIBUTES: u32 = 0x4;
const FD_WRITESTIME: u32 = 0x20;
const FD_FILESIZE: u32 = 0x40;
const ATTRIBUTE_DIRECTORY: u32 = 0x10;
const MAX_TITLE: usize = 120;
const CHUNK: usize = 1 << 16;
const UNIX_EPOCH_TICKS: u64 = 116_444_736_000_000_000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Content {
    Items,
    Files,
    Url,
}

#[derive(Clone, Copy, Default, Debug)]
pub(crate) struct Offered {
    pub(crate) items: bool,
    pub(crate) files: bool,
    pub(crate) url: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Described {
    pub(crate) name: String,
    pub(crate) dir: bool,
    pub(crate) size: Option<u64>,
    pub(crate) written: Option<u64>,
}

pub(crate) fn content(offered: Offered, names: &[String]) -> Option<Content> {
    if offered.url && names.iter().all(|n| n.to_ascii_lowercase().ends_with(".url")) {
        return Some(Content::Url);
    }
    if offered.items {
        return Some(Content::Items);
    }
    (offered.files && !names.is_empty()).then_some(Content::Files)
}

fn word(bytes: &[u8], at: usize) -> Option<u32> {
    bytes.get(at..at.checked_add(4)?).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

pub(crate) fn descriptors(bytes: &[u8]) -> Vec<Described> {
    let count = word(bytes, 0).unwrap_or(0) as usize;
    (0..count)
        .map_while(|i| {
            let d = bytes.get(4 + i * DESCRIPTOR..4 + (i + 1) * DESCRIPTOR)?;
            let w = |at: usize| u64::from(word(d, at).unwrap_or(0));
            let flags = w(0) as u32;
            let units: Vec<u16> = d[DESCRIPTOR_NAME_AT..].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&u| u != 0).collect();
            Some(Described {
                name: String::from_utf16_lossy(&units),
                dir: flags & FD_ATTRIBUTES != 0 && w(36) as u32 & ATTRIBUTE_DIRECTORY != 0,
                size: (flags & FD_FILESIZE != 0).then(|| (w(64) << 32) | w(68)),
                written: (flags & FD_WRITESTIME != 0).then(|| (w(60) << 32) | w(56)),
            })
        })
        .collect()
}

fn pidl_at(bytes: &[u8], at: usize) -> Option<&[u8]> {
    let mut end = at;
    loop {
        let cb = usize::from(u16::from_le_bytes(bytes.get(end..end + 2)?.try_into().ok()?));
        match cb {
            0 => return bytes.get(at..end + 2),
            1 => return None,
            _ => end = end.checked_add(cb)?,
        }
    }
}

pub(crate) fn cida_pidls(bytes: &[u8]) -> Vec<Vec<u8>> {
    let offset = |i: usize| word(bytes, 4 + 4 * i).map(|o| o as usize);
    let (Some(count), Some(parent)) = (word(bytes, 0), offset(0).and_then(|o| pidl_at(bytes, o))) else { return Vec::new() };
    let base = &parent[..parent.len() - 2];
    (1..=count as usize).map_while(|i| pidl_at(bytes, offset(i)?).map(|child| [base, child].concat())).collect()
}

pub(crate) fn clean_name(part: &str) -> Option<String> {
    if part == "." || part == ".." {
        return None;
    }
    let s: String = part.chars().map(|c| if c.is_control() || r#"<>:"/\|?*"#.contains(c) { '_' } else { c }).collect();
    let s = s.trim().trim_end_matches(['.', ' ']);
    (!s.is_empty()).then(|| s.to_owned())
}

fn free_name(name: &str, file: bool, taken: impl Fn(&str) -> bool) -> String {
    let (stem, ext) = match name.rfind('.') {
        Some(k) if file && k > 0 => name.split_at(k),
        _ => (name, ""),
    };
    (1..).map(|k| if k == 1 { name.to_owned() } else { format!("{stem} ({k}){ext}") }).find(|n| !taken(n)).unwrap_or_default()
}

pub(crate) fn placement(items: &[Described], taken: impl Fn(&str) -> bool) -> Vec<Option<Vec<String>>> {
    let mut tops: Vec<(String, String)> = Vec::new();
    let mut out = Vec::with_capacity(items.len());
    for d in items {
        let parts: Option<Vec<String>> = d.name.split(['\\', '/']).filter(|p| !p.is_empty()).map(clean_name).collect();
        let Some((top, rest)) = parts.as_ref().and_then(|p| p.split_first()) else {
            out.push(None);
            continue;
        };
        let key = top.to_lowercase();
        let chosen = match tops.iter().find(|(k, _)| *k == key) {
            Some((_, chosen)) => chosen.clone(),
            None => {
                let chosen = free_name(top, rest.is_empty() && !d.dir, |n| taken(n) || tops.iter().any(|(_, c)| c.eq_ignore_ascii_case(n)));
                tops.push((key, chosen.clone()));
                chosen
            }
        };
        out.push(Some(std::iter::once(chosen).chain(rest.iter().cloned()).collect()));
    }
    out
}

pub(crate) fn url_text(bytes: &[u8], wide: bool) -> Option<String> {
    let text = match wide {
        true => String::from_utf16_lossy(&bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&u| u != 0).collect::<Vec<_>>()),
        false => String::from_utf8_lossy(&bytes[..bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len())]).into_owned(),
    };
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

fn host(url: &str) -> Option<&str> {
    let authority = url.split_once("://")?.1.split(['/', '?', '#']).next()?;
    let host = authority.rsplit('@').next()?;
    let host = match host.rsplit_once(':') {
        Some((h, port)) if port.bytes().all(|b| b.is_ascii_digit()) => h,
        _ => host,
    };
    (!host.is_empty()).then_some(host)
}

pub(crate) fn url_name(url: &str, title: Option<&str>) -> String {
    let title = title.map(|t| match t.len().checked_sub(4).filter(|&k| t.is_char_boundary(k) && t[k..].eq_ignore_ascii_case(".url")) {
        Some(k) => &t[..k],
        None => t,
    });
    let base = title.and_then(clean_name).or_else(|| host(url).and_then(clean_name)).unwrap_or_else(|| "Link".to_owned());
    let base: String = base.chars().take(MAX_TITLE).collect();
    format!("{}.url", base.trim_end_matches(['.', ' ']))
}

pub(crate) fn internet_shortcut(url: &str) -> Vec<u8> {
    let mut clean = String::with_capacity(url.len());
    for c in url.trim().chars().filter(|c| !c.is_control()) {
        if c.is_ascii() {
            clean.push(c);
        } else {
            let mut utf8 = [0; 4];
            for b in c.encode_utf8(&mut utf8).bytes() {
                let _ = write!(clean, "%{b:02X}");
            }
        }
    }
    format!("[InternetShortcut]\r\nURL={clean}\r\n").into_bytes()
}

fn format(name: PCWSTR) -> u16 {
    unsafe { RegisterClipboardFormatW(name) as u16 }
}

fn etc(cf: u16, index: i32, tymed: u32) -> FORMATETC {
    FORMATETC { cfFormat: cf, ptd: std::ptr::null_mut(), dwAspect: DVASPECT_CONTENT.0, lindex: index, tymed }
}

fn offers(data: &IDataObject, name: PCWSTR) -> bool {
    unsafe { data.QueryGetData(&etc(format(name), -1, TYMED_HGLOBAL.0 as u32)) == S_OK }
}

fn bytes(data: &IDataObject, name: PCWSTR) -> Option<Vec<u8>> {
    unsafe {
        let mut medium = data.GetData(&etc(format(name), -1, TYMED_HGLOBAL.0 as u32)).ok()?;
        let out = if medium.tymed == TYMED_HGLOBAL.0 as u32 { global_bytes(medium.u.hGlobal) } else { None };
        ReleaseStgMedium(&mut medium);
        out
    }
}

fn described(data: &IDataObject) -> Vec<Described> {
    bytes(data, CFSTR_FILEDESCRIPTORW).map(|b| descriptors(&b)).unwrap_or_default()
}

pub(crate) fn content_of(data: &IDataObject) -> Option<Content> {
    let names: Vec<String> = described(data).into_iter().map(|d| d.name).collect();
    let offered = Offered { items: offers(data, CFSTR_SHELLIDLIST), files: offers(data, CFSTR_FILEDESCRIPTORW), url: offers(data, CFSTR_INETURLW) || offers(data, CFSTR_INETURLA) };
    content(offered, &names)
}

fn io(e: std::io::Error) -> PlatformError {
    PlatformError::new(e.raw_os_error().map_or(-1, i64::from), e.to_string())
}

fn failed(error: PlatformError) -> OpOutcome {
    OpOutcome { items: Vec::new(), error: Some(error) }
}

fn nothing() -> PlatformError {
    PlatformError::new(-1, S::ErrorsEngineClipboardNoFiles.en())
}

unsafe fn copy_items(pidls: &[Vec<u8>], dest: &FsPath, moving: bool, job: &Job) -> OpOutcome {
    let items: Result<Vec<IShellItem>, PlatformError> = pidls.iter().map(|p| SHCreateItemFromIDList(p.as_ptr() as *const ITEMIDLIST).map_err(err)).collect();
    match items {
        Ok(items) if items.is_empty() => failed(nothing()),
        Ok(items) => crate::fileops::transfer_items(&items, dest, moving, job),
        Err(e) => failed(e),
    }
}

fn pump(stream: &IStream, target: &Path, job: &Job) -> Result<(), PlatformError> {
    let mut file = std::fs::File::create_new(target).map_err(io)?;
    let mut buf = vec![0u8; CHUNK];
    loop {
        if job.cancelled() {
            return Err(PlatformError::new(-1, S::ErrorsEngineCancelled.en()));
        }
        let mut read = 0u32;
        unsafe { stream.Read(buf.as_mut_ptr().cast(), buf.len() as u32, Some(&mut read)) }.ok().map_err(err)?;
        if read == 0 {
            return Ok(());
        }
        file.write_all(&buf[..read as usize]).map_err(io)?;
    }
}

unsafe fn store(source: &IStorage, path: &FsPath) -> Result<(), PlatformError> {
    let wide = path.to_wide(false);
    let target = StgCreateDocfile(PCWSTR(wide.as_ptr()), STGM_FAILIFTHERE | STGM_READWRITE | STGM_SHARE_EXCLUSIVE, None).map_err(err)?;
    source.CopyTo(None, None, &target).map_err(err)?;
    target.Commit(STGC_DEFAULT.0 as u32).map_err(err)
}

unsafe fn save(data: &IDataObject, index: usize, d: &Described, path: &FsPath, job: &Job) -> Result<(), PlatformError> {
    let target = std_path(path);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(io)?;
    }
    let wanted = (TYMED_ISTREAM.0 | TYMED_HGLOBAL.0 | TYMED_ISTORAGE.0) as u32;
    let mut medium = data.GetData(&etc(format(CFSTR_FILECONTENTS), index as i32, wanted)).map_err(err)?;
    let saved = match medium.tymed {
        t if t == TYMED_HGLOBAL.0 as u32 => match global_bytes(medium.u.hGlobal) {
            Some(mut b) => {
                if let Some(n) = d.size.and_then(|n| usize::try_from(n).ok()) {
                    b.truncate(n);
                }
                std::fs::File::create_new(&target).and_then(|mut f| f.write_all(&b)).map_err(io)
            }
            None => Err(nothing()),
        },
        t if t == TYMED_ISTREAM.0 as u32 => match medium.u.pstm.as_ref() {
            Some(stream) => pump(stream, &target, job),
            None => Err(nothing()),
        },
        t if t == TYMED_ISTORAGE.0 as u32 => match medium.u.pstg.as_ref() {
            Some(storage) => store(storage, path),
            None => Err(nothing()),
        },
        _ => Err(nothing()),
    };
    ReleaseStgMedium(&mut medium);
    if let (Ok(()), Some(ticks)) = (&saved, d.written.filter(|&t| t >= UNIX_EPOCH_TICKS)) {
        let when = SystemTime::UNIX_EPOCH + Duration::from_nanos((ticks - UNIX_EPOCH_TICKS).saturating_mul(100));
        let _ = std::fs::File::options().write(true).open(&target).and_then(|f| f.set_modified(when));
    }
    saved
}

unsafe fn write_files(data: &IDataObject, dest: &FsPath, job: &Job) -> OpOutcome {
    let items = described(data);
    let plan = placement(&items, |n| exists(&dest.join(n.as_bytes())));
    let mut out = OpOutcome::default();
    for (index, (d, parts)) in items.iter().zip(plan).enumerate() {
        let Some(parts) = parts else { continue };
        let path = parts.iter().fold(dest.clone(), |p, part| p.join(part.as_bytes()));
        let made = if d.dir { std::fs::create_dir_all(std_path(&path)).map_err(io) } else { save(data, index, d, &path, job) };
        if let Err(e) = made {
            out.error = Some(e);
            break;
        }
        let top = dest.join(parts[0].as_bytes());
        if !out.items.iter().any(|(t, _)| *t == top) {
            out.items.push((top.clone(), top));
        }
    }
    out
}

fn write_url(data: &IDataObject, dest: &FsPath) -> OpOutcome {
    let url = bytes(data, CFSTR_INETURLW).and_then(|b| url_text(&b, true)).or_else(|| bytes(data, CFSTR_INETURLA).and_then(|b| url_text(&b, false)));
    let Some(url) = url else { return failed(nothing()) };
    let title = described(data).into_iter().next().map(|d| d.name);
    let name = free_name(&url_name(&url, title.as_deref()), true, |n| exists(&dest.join(n.as_bytes())));
    let path = dest.join(name.as_bytes());
    match std::fs::File::create_new(std_path(&path)).and_then(|mut f| f.write_all(&internet_shortcut(&url))) {
        Ok(()) => OpOutcome { items: vec![(path.clone(), path)], error: None },
        Err(e) => failed(io(e)),
    }
}

unsafe fn transfer(data: &IDataObject, kind: Content, dest: &FsPath, moving: bool, job: &Job) -> OpOutcome {
    match kind {
        Content::Items => match bytes(data, CFSTR_SHELLIDLIST) {
            Some(cida) => copy_items(&cida_pidls(&cida), dest, moving, job),
            None => failed(nothing()),
        },
        Content::Files => write_files(data, dest, job),
        Content::Url => write_url(data, dest),
    }
}

pub(crate) fn clipboard_content() -> bool {
    let available = |name: PCWSTR| unsafe { IsClipboardFormatAvailable(u32::from(format(name))).is_ok() };
    [CFSTR_SHELLIDLIST, CFSTR_FILEDESCRIPTORW, CFSTR_INETURLW, CFSTR_INETURLA].into_iter().any(available)
}

pub(crate) fn paste(dest: &FsPath, job: &Job) -> OpOutcome {
    unsafe {
        let _ = OleInitialize(None);
        let data = match OleGetClipboard() {
            Ok(d) => d,
            Err(e) => return failed(err(e)),
        };
        let Some(kind) = content_of(&data) else { return failed(nothing()) };
        let moving = kind == Content::Items && bytes(&data, CFSTR_PREFERREDDROPEFFECT).and_then(|b| word(&b, 0)).is_some_and(effect_is_move);
        let out = transfer(&data, kind, dest, moving, job);
        if moving && out.error.is_none() {
            crate::clipboard::clear_if_unchanged();
        }
        out
    }
}

pub(crate) fn drop_into(data: &IDataObject, kind: Content, dest: &FsPath, moving: bool) {
    let report = |out: OpOutcome| {
        if let Some(e) = out.error {
            tracing::warn!("drop into {}: {e}", dest.display());
        }
    };
    if kind != Content::Items {
        return report(unsafe { transfer(data, kind, dest, false, &Job::detached(Resolve::Skip)) });
    }
    let Some(cida) = bytes(data, CFSTR_SHELLIDLIST) else { return };
    let dest = dest.clone();
    let spawned = std::thread::Builder::new().name("flit-drop".into()).spawn(move || {
        com_sta();
        let out = unsafe { copy_items(&cida_pidls(&cida), &dest, moving, &Job::detached(Resolve::Skip)) };
        if let Some(e) = out.error {
            tracing::warn!("drop into {}: {e}", dest.display());
        }
    });
    if let Err(e) = spawned {
        tracing::warn!("drop thread: {e}");
    }
}

#[cfg(test)]
#[path = "../test/transfer.rs"]
mod tests;
