use flit_core::listing::{attr, Meta};
use flit_core::ui::Drive;
use flit_core::{FsError, FsErrorKind, ListingBuilder};
use flit_platform::{OpOutcome, PlatformError};
use flit_platform_linux::mounts::{self, Mount};
use flit_platform_linux::Trash;
use std::path::{Path, PathBuf};

pub struct Volume {
    pub mount: Mount,
    pub label: String,
    pub total: u64,
    pub free: u64,
}

fn name_of(m: &Mount) -> String {
    match m.point.as_slice() {
        b"/" => "File System".into(),
        p => String::from_utf8_lossy(p.rsplit(|&b| b == b'/').next().unwrap_or(p)).into_owned(),
    }
}

pub fn volumes_from(table: &[Mount], user: &[u8], by_label: &Path) -> Vec<Volume> {
    let labels = mounts::labels(by_label);
    mounts::visible(table, user)
        .into_iter()
        .map(|m| {
            let (total, free) = mounts::space(m.path()).unwrap_or_default();
            let label = mounts::label_of(&m, &labels).filter(|_| m.point != b"/").unwrap_or_else(|| name_of(&m));
            Volume { mount: m, label, total, free }
        })
        .collect()
}

pub fn volumes() -> Vec<Volume> {
    volumes_from(&mounts::read().unwrap_or_default(), &mounts::user_name(), Path::new(mounts::BY_LABEL))
}

pub fn drives() -> Vec<Drive> {
    volumes()
        .into_iter()
        .filter_map(|v| Some(Drive { path: crate::xdg::fs_path(v.mount.path())?, label: v.label, total: v.total, free: v.free, removable: v.mount.removable() }))
        .collect()
}

pub fn list_computer(out: &mut ListingBuilder<'_>) -> Result<(), FsError> {
    let table = mounts::read().map_err(|e| FsError { kind: FsErrorKind::Unavailable, code: e.raw_os_error().unwrap_or(-1) as i64 })?;
    for m in mounts::visible(&table, &mounts::user_name()) {
        let mut meta = Meta { attrs: attr::DIR | attr::VOLUME, ..Default::default() };
        if let Some((total, _)) = mounts::space(m.path()) {
            meta.size = total;
            meta.attrs |= attr::SIZE_KNOWN;
        }
        out.push(&m.point, meta);
    }
    out.batch_done();
    Ok(())
}

pub fn watch_mounts(changed: Box<dyn Fn() + Send>) {
    if let Err(e) = mounts::watch(Path::new(mounts::MOUNTINFO), changed) {
        eprintln!("flit: cannot watch {}: {e}", mounts::MOUNTINFO);
    }
}

pub fn storage_tops() -> Vec<PathBuf> {
    mounts::read().unwrap_or_default().iter().filter(|m| m.is_storage()).map(|m| m.path().to_path_buf()).collect()
}

pub fn trash() -> Result<Trash, PlatformError> {
    Trash::from_env().ok_or_else(|| PlatformError::new(-1, "The Trash folder is unknown: neither XDG_DATA_HOME nor HOME is set"))
}

pub fn list_trash(out: &mut ListingBuilder<'_>) -> Result<(), FsError> {
    let trash = trash().map_err(|_| FsError { kind: FsErrorKind::Unavailable, code: 0 })?;
    trash.list(&trash.dirs(&storage_tops()), out);
    Ok(())
}

fn failure(p: &Path, e: std::io::Error) -> PlatformError {
    PlatformError::new(e.raw_os_error().unwrap_or(-1) as i64, format!("{}: {e}", p.display()))
}

pub fn purge(trash: &Trash, pairs: &[(flit_core::FsPath, flit_core::FsPath)]) -> OpOutcome {
    let mut out = OpOutcome::default();
    for (trashed, original) in pairs {
        let Some(path) = flit_platform_linux::host(trashed) else {
            out.error = Some(PlatformError::new(-1, format!("{} is not in the Trash", trashed.display())));
            continue;
        };
        match trash.purge(path) {
            Ok(()) => out.items.push((trashed.clone(), original.clone())),
            Err(e) => out.error = Some(failure(path, e)),
        }
    }
    out
}

pub fn empty(trash: &Trash, tops: &[PathBuf]) -> OpOutcome {
    let dirs = trash.dirs(tops);
    let (removed, error) = trash.empty(&dirs);
    if removed.is_empty() && error.is_none() {
        return OpOutcome { items: Vec::new(), error: Some(PlatformError::new(-1, "The Trash is already empty")) };
    }
    let items = removed.iter().filter_map(|p| crate::xdg::fs_path(p)).map(|p| (p.clone(), p)).collect();
    OpOutcome { items, error: error.map(|e| PlatformError::new(e.raw_os_error().unwrap_or(-1) as i64, format!("Some items could not be deleted from the Trash: {e}"))) }
}

#[cfg(test)]
#[path = "../test/places.rs"]
mod tests;
