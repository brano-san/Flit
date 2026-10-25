use flit_core::preview::{self, FolderSummary, Kind, PreviewData, PreviewRequest, FOLDER_SCAN, IMAGE_FILE_LIMIT, POSTER_PX, RTF_BYTES, SNIFF_LIMIT, TEXT_BYTES};
use flit_core::FsPath;
use flit_platform::Shell;
use std::io::Read;
use std::path::PathBuf;
use std::time::UNIX_EPOCH;

#[cfg(windows)]
fn std_path(p: &FsPath) -> PathBuf {
    use std::os::windows::ffi::OsStringExt;
    let mut w = p.to_wide(true);
    w.pop();
    PathBuf::from(std::ffi::OsString::from_wide(&w))
}

#[cfg(not(windows))]
fn std_path(p: &FsPath) -> PathBuf {
    PathBuf::from(p.display().into_owned())
}

#[cfg(windows)]
fn hidden(m: &std::fs::Metadata, _: &str) -> bool {
    std::os::windows::fs::MetadataExt::file_attributes(m) & 2 != 0
}

#[cfg(not(windows))]
fn hidden(_: &std::fs::Metadata, name: &str) -> bool {
    name.starts_with('.')
}

fn ticks(m: &std::fs::Metadata) -> i64 {
    m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| (d.as_secs() as i64 + 11_644_473_600) * 10_000_000)
}

fn head(path: &FsPath, limit: usize) -> Option<Vec<u8>> {
    let mut buf = Vec::with_capacity(limit.min(1 << 20));
    std::fs::File::open(std_path(path)).ok()?.take(limit as u64).read_to_end(&mut buf).ok()?;
    Some(buf)
}

fn folder(req: &PreviewRequest) -> Option<FolderSummary> {
    let mut s = FolderSummary::default();
    for (n, e) in std::fs::read_dir(std_path(&req.path)).ok()?.enumerate() {
        if n == FOLDER_SCAN {
            return Some(s.finish(false));
        }
        if n % 256 == 0 && !req.cancel.is_current(req.gen) {
            return None;
        }
        let Ok((e, m)) = e.and_then(|e| e.metadata().map(|m| (e, m))) else { continue };
        let name = e.file_name().to_string_lossy().into_owned();
        s.add(name.as_bytes(), m.is_dir(), hidden(&m, &name), m.len(), ticks(&m));
    }
    Some(s.finish(true))
}

fn basic<S: Shell>(shell: &mut S, req: &PreviewRequest, note: Option<String>) -> PreviewData {
    PreviewData::Basic { poster: shell.poster(&req.path, POSTER_PX), note }
}

fn other<S: Shell>(shell: &mut S, req: &PreviewRequest, ext: &[u8]) -> Option<PreviewData> {
    if ext.eq_ignore_ascii_case(b"lnk") {
        let note = shell.link_target(&req.path).map(|t| format!("Shortcut to {}", t.display()));
        return Some(PreviewData::Basic { poster: None, note });
    }
    if let Some(clsid) = shell.preview_handler(ext) {
        return Some(PreviewData::Handler(clsid));
    }
    if req.size <= SNIFF_LIMIT {
        let bytes = head(&req.path, SNIFF_LIMIT as usize)?;
        if !preview::looks_binary(&bytes) {
            return Some(PreviewData::Doc(preview::build(Kind::Text, &bytes[..bytes.len().min(TEXT_BYTES)])));
        }
    }
    Some(basic(shell, req, None))
}

pub fn load<S: Shell>(shell: &mut S, req: &PreviewRequest) -> Option<PreviewData> {
    if !req.cancel.is_current(req.gen) {
        return None;
    }
    if req.details {
        return Some(PreviewData::Props(shell.properties(&req.path)));
    }
    let ext = req.path.file_name();
    let ext = &ext[ext.len() - flit_core::name::ext_len(ext)..];
    match req.kind {
        Kind::Folder => folder(req).map(PreviewData::Folder),
        Kind::Image if req.size > IMAGE_FILE_LIMIT => Some(basic(shell, req, Some("The image is too large to preview".into()))),
        Kind::Image => Some(shell.preview_image(&req.path, req.px).map_or_else(|| basic(shell, req, None), PreviewData::Image)),
        Kind::Audio | Kind::Video => Some(PreviewData::Media { video: req.kind == Kind::Video, poster: shell.poster(&req.path, POSTER_PX) }),
        Kind::Rtf => match shell.preview_handler(ext) {
            Some(clsid) => Some(PreviewData::Handler(clsid)),
            None => Some(PreviewData::Doc(preview::build(Kind::Rtf, &head(&req.path, RTF_BYTES)?))),
        },
        Kind::Text | Kind::Markdown | Kind::Code(_) => {
            let bytes = head(&req.path, TEXT_BYTES)?;
            if preview::looks_binary(&bytes) {
                return other(shell, req, ext);
            }
            Some(PreviewData::Doc(preview::build(req.kind, &bytes)))
        }
        Kind::Other => other(shell, req, ext),
    }
}

#[cfg(test)]
#[path = "../test/preview.rs"]
mod tests;
