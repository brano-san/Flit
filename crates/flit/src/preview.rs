pub(crate) mod pretty;
pub(crate) mod svg;
mod child;
mod raw;

use flit_core::preview::{self, FolderSummary, Kind, Note, PreviewData, PreviewRequest, FOLDER_SCAN, IMAGE_FILE_LIMIT, POSTER_PX, RTF_BYTES, SNIFF_LIMIT, TEXT_BYTES};
use flit_core::FsPath;
use flit_platform::{parse_process, PreviewPicture, Shell};
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::time::UNIX_EPOCH;

pub(crate) use flit_platform::std_path;

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
    regular_file(&std_path(path))?.take(limit as u64).read_to_end(&mut buf).ok()?;
    Some(buf)
}

fn regular_file(path: &Path) -> Option<File> {
    if !std::fs::metadata(path).ok()?.is_file() {
        return None;
    }
    let mut options = File::options();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = options.open(path).ok()?;
    file.metadata().ok()?.is_file().then_some(file)
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

fn basic<S: Shell>(shell: &mut S, req: &PreviewRequest, note: Option<Note>) -> PreviewData {
    PreviewData::Basic { poster: shell.poster(&req.path, POSTER_PX), note }
}

fn cannot_open() -> PreviewData {
    PreviewData::Basic { poster: None, note: Some(Note::Message(flit_core::i18n::S::PreviewCannotOpen.t().to_owned())) }
}

fn archive<S: Shell>(shell: &mut S, req: &PreviewRequest) -> Option<PreviewData> {
    let current = || req.cancel.is_current(req.gen);
    let result = parse_process::run(&std_path(&req.path), "archive", &[], TEXT_BYTES + parse_process::ARCHIVE_HEADER.len(), &current);
    if !current() {
        return None;
    }
    archive_preview(shell, req, result)
}

fn archive_preview<S: Shell>(shell: &mut S, req: &PreviewRequest, result: Result<Vec<u8>, parse_process::Error>) -> Option<PreviewData> {
    use parse_process::Error;
    match result {
        Ok(bytes) => {
            let names = bytes.strip_prefix(parse_process::ARCHIVE_HEADER).filter(|names| (names.is_empty() || names.ends_with(b"\n")) && std::str::from_utf8(names).is_ok());
            Some(names.map_or_else(cannot_open, |names| PreviewData::Doc(preview::build(Kind::Text, names))))
        }
        Err(Error::TooLarge) => Some(basic(shell, req, Some(Note::TooLarge))),
        Err(Error::TimedOut | Error::PasswordRequired) => Some(basic(shell, req, None)),
        Err(Error::Cancelled) => None,
        _ => Some(cannot_open()),
    }
}

fn other<S: Shell>(shell: &mut S, req: &PreviewRequest, ext: &[u8]) -> Option<PreviewData> {
    if ext.eq_ignore_ascii_case(b"lnk") {
        let note = shell.link_target(&req.path).map(|t| Note::Shortcut(t.display().into_owned()));
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
    if ext.eq_ignore_ascii_case(b"pdf") && req.kind != Kind::Folder {
        if let Some(pages) = shell.pdf_document(req) {
            return Some(PreviewData::Pdf(pages));
        }
        if !req.cancel.is_current(req.gen) {
            return None;
        }
    }
    if preview::is_svg(ext) && req.kind != Kind::Folder {
        let result = svg::load(&req.path, req.px, || req.cancel.is_current(req.gen));
        if !req.cancel.is_current(req.gen) {
            return None;
        }
        let fallback = || {
            let kind = preview::syntax::by_ext("xml").map_or(Kind::Text, Kind::Code);
            match head(&req.path, TEXT_BYTES) {
                Some(bytes) if !preview::looks_binary(&bytes) => PreviewData::Doc(preview::build(kind, &bytes)),
                _ => PreviewData::Basic { poster: None, note: None },
            }
        };
        return Some(result.map_or_else(fallback, PreviewData::Image));
    }
    if preview::is_raw(ext) && req.kind != Kind::Folder {
        let path = std_path(&req.path);
        let result = raw::load(&path, req.px, raw::Source::Inspector, || req.cancel.is_current(req.gen));
        return req.cancel.is_current(req.gen).then(|| {
            result.map_or_else(
                || {
                    if cfg!(windows) && std::fs::metadata(path).is_ok_and(|m| m.is_file()) {
                        shell.preview_image(&req.path, req.px).map_or_else(|| basic(shell, req, None), PreviewData::Image)
                    } else {
                        PreviewData::Basic { poster: None, note: None }
                    }
                },
                PreviewData::Image,
            )
        });
    }
    match req.kind {
        Kind::Folder => folder(req).map(PreviewData::Folder),
        Kind::Image if req.size > IMAGE_FILE_LIMIT => Some(basic(shell, req, Some(Note::TooLarge))),
        Kind::Image => {
            let current = || req.cancel.is_current(req.gen);
            let result = shell.preview_picture(&req.path, req.px, &current);
            current().then(|| match result {
                PreviewPicture::Ready(bitmap) => PreviewData::Image(bitmap),
                PreviewPicture::Missing => basic(shell, req, None),
                PreviewPicture::Failed => cannot_open(),
            })
        }
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
        Kind::Other if flit_core::archive::browsable(req.path.file_name()).is_some() => archive(shell, req),
        Kind::Other => other(shell, req, ext),
    }
}

pub(crate) fn thumbnail<S: Shell>(shell: &mut S, req: &flit_core::ui::IconRequest, current: impl Fn() -> bool) -> Option<flit_core::ui::Bitmap> {
    let file = req.path.file_name();
    let ext = &file[file.len() - flit_core::name::ext_len(file)..];
    if preview::is_svg(ext) {
        return svg::load(&req.path, req.size, current);
    }
    if preview::is_raw(ext) {
        let path = std_path(&req.path);
        if let Some(image) = raw::load(&path, req.size, raw::Source::Thumbnail, &current) {
            return Some(image);
        }
        if !cfg!(windows) || !std::fs::metadata(path).is_ok_and(|m| m.is_file()) {
            return None;
        }
    }
    if current() {
        shell.thumbnail_current(req, &current)
    } else {
        None
    }
}

#[cfg(test)]
#[path = "../test/preview.rs"]
mod tests;
