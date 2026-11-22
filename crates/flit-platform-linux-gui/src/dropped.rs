use flit_core::provider::{DroppedFile, FileOp, Stage};
use flit_core::FsPath;
use flit_platform::{Job, OpOutcome, PlatformError};
use std::io::Write;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use winit::data_transfer::TypeHint;

const WEB_SCHEMES: [&str; 3] = ["http://", "https://", "ftp://"];
const MAX_STEM: usize = 80;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Content {
    Url(String),
    Text(String),
    Image(Vec<u8>),
    Repository(String),
}

pub fn web_urls(uri_list: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(uri_list)
        .lines()
        .map(str::trim)
        .filter(|l| !l.starts_with('#'))
        .filter(|l| WEB_SCHEMES.iter().any(|s| l.len() > s.len() && l.get(..s.len()).is_some_and(|prefix| prefix.eq_ignore_ascii_case(s))))
        .map(str::to_owned)
        .collect()
}

fn clean(name: &str) -> String {
    let kept: String = name.chars().map(|c| if c == '/' || c.is_control() { ' ' } else { c }).collect();
    let trimmed = kept.split_whitespace().collect::<Vec<_>>().join(" ");
    let cut: String = trimmed.chars().take(MAX_STEM).collect();
    cut.trim_matches('.').trim().to_owned()
}

pub fn url_stem(url: &str) -> Option<String> {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let rest = rest.split(['?', '#']).next().unwrap_or(rest);
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let host = host.rsplit('@').next().unwrap_or(host);
    let last = path.trim_end_matches('/').rsplit('/').next().filter(|s| !s.is_empty());
    let stem = clean(&last.map_or_else(|| host.to_owned(), |l| format!("{host} {}", String::from_utf8_lossy(&flit_platform_linux::percent_decode(l.as_bytes())))));
    (!stem.is_empty()).then_some(stem)
}

pub fn link_file(url: &str, name: &str) -> String {
    let one_line = |s: &str| s.replace(['\n', '\r'], " ");
    format!("[Desktop Entry]\nType=Link\nName={}\nURL={}\nIcon=text-html\n", one_line(name), one_line(url))
}

impl Content {
    pub fn repository_url(&self) -> Option<&str> {
        match self {
            Self::Repository(url) => Some(url),
            Self::Url(url) | Self::Text(url) => flit_core::git::dropped_repo_url(url),
            Self::Image(_) => None,
        }
    }

    pub fn file(self, text_stem: &str, image_stem: &str) -> Option<DroppedFile> {
        let (stem, extension, bytes) = match self {
            Self::Url(url) => {
                let stem = url_stem(&url).unwrap_or_else(|| clean(text_stem));
                let bytes = link_file(&url, &stem).into_bytes();
                (stem, "desktop", bytes)
            }
            Self::Text(text) => (clean(text_stem), "txt", text.into_bytes()),
            Self::Image(png) => (clean(image_stem), "png", png),
            Self::Repository(_) => return None,
        };
        Some(DroppedFile { name: format!("{stem}.{extension}").into_bytes(), bytes })
    }
}

static NEXT_STAGE: AtomicU64 = AtomicU64::new(0);
const WRITE_CHUNK: usize = 64 * 1024;

struct Staged {
    path: PathBuf,
    dir: PathBuf,
}

fn cancelled() -> std::io::Error {
    std::io::Error::from_raw_os_error(libc::ECANCELED)
}

impl Staged {
    fn new(data: &DroppedFile, job: &Job) -> std::io::Result<Self> {
        Self::new_at(&std::env::temp_dir(), data, job, NEXT_STAGE.fetch_add(1, Ordering::Relaxed))
    }

    fn new_at(root: &Path, data: &DroppedFile, job: &Job, id: u64) -> std::io::Result<Self> {
        if job.cancelled() {
            return Err(cancelled());
        }
        if flit_proto::listing::check_name(&data.name, false).is_err() || data.name.iter().any(u8::is_ascii_control) {
            return Err(std::io::Error::from_raw_os_error(libc::EINVAL));
        }
        use std::os::unix::ffi::OsStrExt;
        let dir = private_dir(root, "flit-drop", id, || job.cancelled())?;
        let staged = Self { path: dir.join(std::ffi::OsStr::from_bytes(&data.name)), dir };
        let mut file = std::fs::File::create_new(&staged.path)?;
        job.update(|p| {
            p.stage = Stage::Preparing;
            p.total_bytes = data.bytes.len() as u64;
            p.total_items = 1;
        });
        for chunk in data.bytes.chunks(WRITE_CHUNK) {
            if job.cancelled() {
                return Err(cancelled());
            }
            file.write_all(chunk)?;
        }
        if job.cancelled() {
            return Err(cancelled());
        }
        Ok(staged)
    }
}

impl Drop for Staged {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        let _ = std::fs::remove_dir(&self.dir);
    }
}

pub fn private_dir(root: &Path, prefix: &str, id: u64, stop: impl Fn() -> bool) -> std::io::Result<PathBuf> {
    let mut attempt = 0u64;
    loop {
        if stop() {
            return Err(cancelled());
        }
        let dir = root.join(format!("{prefix}-{}-{id}-{attempt}", std::process::id()));
        match std::fs::DirBuilder::new().mode(0o700).create(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => attempt = attempt.wrapping_add(1),
            Err(e) => return Err(e),
        }
    }
}

pub fn failed(e: &std::io::Error) -> OpOutcome {
    let message = if e.raw_os_error() == Some(libc::ECANCELED) { flit_platform::CANCELLED.to_owned() } else { flit_core::fmt::io_reason(e) };
    OpOutcome { items: Vec::new(), error: Some(PlatformError::new(e.raw_os_error().map_or(-1, i64::from), message)) }
}

pub fn queued(dest: &FsPath, data: &DroppedFile, job: &Job) -> OpOutcome {
    if let Some(promised) = crate::xds::queued(dest, data, job) {
        return promised;
    }
    let staged = match Staged::new(data, job) {
        Ok(staged) => staged,
        Err(e) => return failed(&e),
    };
    copy_staged(&staged.path, dest, job)
}

pub fn copy_staged(path: &Path, dest: &FsPath, job: &Job) -> OpOutcome {
    use std::os::unix::ffi::OsStrExt;
    let Some(source) = FsPath::parse_exact(path.as_os_str().as_bytes()) else {
        return OpOutcome { items: Vec::new(), error: Some(PlatformError::new(libc::EINVAL as i64, flit_core::fmt::io_reason(&std::io::Error::from_raw_os_error(libc::EINVAL)))) };
    };
    let out = flit_platform_linux::run_op(&FileOp::Copy { items: vec![source], dest: dest.clone() }, job, None);
    OpOutcome { items: out.items.into_iter().map(|(_, target)| (target.clone(), target)).collect(), error: out.error }
}

pub const PNG: TypeHint = TypeHint::Image { extension_hint: Some("png") };

#[derive(Debug, PartialEq, Eq)]
pub enum Dragged {
    Files(Vec<FsPath>),
    Content(Content),
}

pub fn wanted(offered: &[TypeHint]) -> Vec<TypeHint> {
    if offered.is_empty() {
        return vec![TypeHint::UriList];
    }
    [TypeHint::UriList, PNG, TypeHint::Plaintext].into_iter().filter(|w| offered.contains(w)).collect()
}

pub fn fetched(offered: &[TypeHint], direct_save: bool) -> Vec<TypeHint> {
    let mut kinds = wanted(offered);
    if direct_save {
        kinds.retain(|k| offered.contains(k));
    }
    kinds
}

pub fn interpret(kind: Option<TypeHint>, bytes: &[u8]) -> Option<Dragged> {
    match kind.unwrap_or(TypeHint::UriList) {
        TypeHint::UriList => {
            let paths = crate::shell::parse_file_list(bytes).0;
            if !paths.is_empty() {
                return Some(Dragged::Files(paths));
            }
            let content = web_urls(bytes).into_iter().next().map(Content::Url).or_else(|| {
                String::from_utf8_lossy(bytes).lines().find_map(flit_core::git::dropped_repo_url).map(|url| Content::Repository(url.to_owned()))
            })?;
            Some(Dragged::Content(content))
        }
        TypeHint::Plaintext => {
            let text = String::from_utf8_lossy(bytes).into_owned();
            let single = web_urls(text.as_bytes()).into_iter().next().filter(|u| u == text.trim());
            match single {
                Some(url) => Some(Dragged::Content(Content::Url(url))),
                None => (!text.trim().is_empty()).then_some(Dragged::Content(Content::Text(text))),
            }
        }
        hint if hint == PNG => (!bytes.is_empty()).then(|| Dragged::Content(Content::Image(bytes.to_vec()))),
        _ => None,
    }
}

#[cfg(test)]
#[path = "../test/dropped.rs"]
mod tests;
