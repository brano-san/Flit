mod compress;
mod extract;
mod index;
mod names;

pub use index::{days_from_civil, dos_ticks};
pub use names::{decode, detect, safe_path};

use flit_core::archive::{self, ArchiveOp, PASSWORD_CODE};
use flit_core::listing::{attr, FsError, FsErrorKind, ListingBuilder, Meta};
use flit_core::path::FsPath;
use flit_core::provider::FileOp;
use flit_platform::{Job, OpOutcome, PlatformError};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

pub struct Env<'a> {
    pub utc_offset: i32,
    pub free_space: &'a dyn Fn(&Path) -> Option<u64>,
}

#[cfg(windows)]
pub fn std_path(p: &FsPath) -> PathBuf {
    use std::os::windows::ffi::OsStringExt;
    let mut w = p.to_wide(true);
    w.pop();
    PathBuf::from(std::ffi::OsString::from_wide(&w))
}

#[cfg(not(windows))]
pub fn std_path(p: &FsPath) -> PathBuf {
    PathBuf::from(p.display().into_owned())
}

pub(crate) fn io_error(e: std::io::Error) -> PlatformError {
    PlatformError::new(e.raw_os_error().map_or(-1, i64::from), e.to_string())
}

pub(crate) fn corrupt(message: &str) -> PlatformError {
    PlatformError::new(-1, message)
}

pub(crate) fn password_error(had_password: bool) -> PlatformError {
    PlatformError::new(PASSWORD_CODE, if had_password { archive::WRONG_PASSWORD } else { archive::PASSWORD_NEEDED })
}

pub fn locate(path: &FsPath) -> Option<(FsPath, String)> {
    archive::splits(path).into_iter().find(|(a, _)| std::fs::metadata(std_path(a)).is_ok_and(|m| m.is_file())).map(|(a, inner)| (a, String::from_utf8_lossy(&inner).into_owned()))
}

pub fn inside(path: &FsPath) -> bool {
    locate(path).is_some_and(|(_, inner)| !inner.is_empty())
}

fn fs_error(e: PlatformError) -> FsError {
    let kind = match e.code {
        PASSWORD_CODE => FsErrorKind::AccessDenied,
        2 | 3 => FsErrorKind::NotFound,
        5 => FsErrorKind::AccessDenied,
        _ => FsErrorKind::Other,
    };
    FsError { kind, code: e.code }
}

pub fn read_dir(dir: &FsPath, out: &mut ListingBuilder<'_>, utc_offset: i32) -> Option<Result<(), FsError>> {
    let (archive, inner) = locate(dir)?;
    Some(list(&archive, &inner, out, utc_offset))
}

struct Child {
    name: String,
    size: u64,
    modified: i64,
    dir: bool,
}

fn list(archive: &FsPath, inner: &str, out: &mut ListingBuilder<'_>, utc_offset: i32) -> Result<(), FsError> {
    let index = index::open(archive, utc_offset).map_err(fs_error)?;
    let prefix = if inner.is_empty() { String::new() } else { format!("{}/", names::fold(inner)) };
    let mut children: Vec<Child> = Vec::new();
    let mut at: HashMap<String, usize> = HashMap::new();
    let mut found = inner.is_empty();
    for item in &index.items {
        let Some(rest) = item.key.strip_prefix(prefix.as_str()) else {
            found |= item.key == prefix.trim_end_matches('/') && item.dir;
            continue;
        };
        found = true;
        let cut = rest.find('/');
        let name_len = cut.unwrap_or(rest.len());
        let key = &rest[..name_len];
        let name = &item.path[prefix.len()..prefix.len() + name_len];
        let k = *at.entry(key.to_owned()).or_insert_with(|| {
            children.push(Child { name: name.to_owned(), size: 0, modified: 0, dir: cut.is_some() });
            children.len() - 1
        });
        let c = &mut children[k];
        c.size += item.size;
        c.dir |= cut.is_some() || item.dir;
        c.modified = c.modified.max(item.modified);
    }
    if !found {
        return Err(FsError { kind: FsErrorKind::NotFound, code: 3 });
    }
    let known = if index.declared { attr::SIZE_KNOWN } else { 0 };
    for c in children {
        let attrs = attr::ARCHIVED | known | if c.dir { attr::DIR | attr::SIZE_KNOWN } else { 0 };
        out.push(c.name.as_bytes(), Meta { size: c.size, modified: c.modified, created: 0, attrs });
    }
    out.batch_done();
    Ok(())
}

fn paths_of(op: &FileOp) -> Vec<FsPath> {
    let seen = RefCell::new(Vec::new());
    op.map_paths(|p| {
        seen.borrow_mut().push(p.clone());
        Some(p.clone())
    });
    seen.into_inner()
}

pub fn file_op(op: &FileOp, job: &Job, env: &Env<'_>) -> Option<OpOutcome> {
    let done = |r: extract::Outcome| match r {
        Ok(items) => OpOutcome { items, error: None },
        Err((items, e)) => OpOutcome { items, error: Some(e) },
    };
    match op {
        FileOp::Archive(ArchiveOp::Compress { items, archive, options }) => Some(done(compress::compress(items, archive, options, job, env))),
        FileOp::Archive(ArchiveOp::Extract { archives, dest, place }) => Some(done(extract::archives(archives, dest.as_ref(), *place, job, env))),
        FileOp::Archive(ArchiveOp::Open { item }) => Some(done(open(item, job, env))),
        FileOp::Copy { items, dest } => {
            if let Some((archive, inner)) = locate(dest) {
                return Some(done(compress::add(items, &archive, &inner, job, env)));
            }
            let mut located = items.iter().filter_map(|i| locate(i).filter(|(_, inner)| !inner.is_empty()));
            let (archive, _) = located.next()?;
            Some(done(extract::entries(&archive, items, dest, job, env)))
        }
        op => paths_of(op).iter().any(inside).then(|| OpOutcome { items: Vec::new(), error: Some(PlatformError::new(-1, archive::INSIDE)) }),
    }
}

fn open(item: &FsPath, job: &Job, env: &Env<'_>) -> extract::Outcome {
    static OPENED: AtomicU64 = AtomicU64::new(0);
    let Some((archive, _)) = locate(item).filter(|(_, inner)| !inner.is_empty()) else { return Err((Vec::new(), PlatformError::new(2, "The item is not inside an archive"))) };
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis());
    let dir = std::env::temp_dir().join("Flit").join(format!("{}-{stamp}-{}", std::process::id(), OPENED.fetch_add(1, Ordering::Relaxed)));
    let dest = FsPath::parse(&dir.to_string_lossy()).ok_or_else(|| (Vec::new(), PlatformError::new(-1, "The temporary folder cannot be used")))?;
    extract::entries(&archive, std::slice::from_ref(item), &dest, job, env)
}

pub fn forget(archive: &FsPath) {
    index::forget(archive);
}

#[cfg(test)]
#[path = "../test/lib.rs"]
mod tests;
