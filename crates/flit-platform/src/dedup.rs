use crate::{FileOps, Job, OpOutcome, PlatformError, std_path};
use flit_core::i18n::S;
use flit_core::provider::{DedupBackup, FileOp, Resolve, Stage};
use flit_core::tags::FileKey;
use flit_core::FsPath;
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, Read};
use std::time::SystemTime;

const BUFFER: usize = 256 * 1024;

#[derive(Clone, PartialEq, Eq)]
struct Stamp {
    id: FileKey,
    size: u64,
    modified: SystemTime,
    created: Option<SystemTime>,
}

fn fail(key: S) -> PlatformError {
    PlatformError::new(-1, key.en())
}

fn io_err(e: io::Error) -> PlatformError {
    PlatformError::from_io(&e, e.to_string())
}

fn ensure_active(job: &Job) -> Result<(), PlatformError> {
    if job.cancelled() {
        Err(PlatformError::cancelled(-1, crate::CANCELLED))
    } else {
        Ok(())
    }
}

fn stamp(sh: &mut impl FileOps, path: &FsPath) -> Result<Stamp, PlatformError> {
    let meta = fs::symlink_metadata(std_path(path)).map_err(io_err)?;
    if !meta.is_file() || reparse(&meta) {
        return Err(fail(S::DedupRegular));
    }
    let id = sh.file_identity(path).ok_or_else(|| fail(S::DedupRegular))?;
    Ok(Stamp { id, size: meta.len(), modified: meta.modified().map_err(io_err)?, created: meta.created().ok() })
}

#[cfg(windows)]
fn reparse(meta: &Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    meta.file_attributes() & 0x400 != 0
}

#[cfg(not(windows))]
fn reparse(_: &Metadata) -> bool {
    false
}

fn open(path: &FsPath) -> Result<File, PlatformError> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(5);
    }
    options.open(std_path(path)).map_err(io_err)
}

fn ensure(sh: &mut impl FileOps, path: &FsPath, expected: &Stamp) -> Result<(), PlatformError> {
    if stamp(sh, path)? != *expected {
        Err(fail(S::DedupChanged))
    } else {
        Ok(())
    }
}

fn compare(mut a: &File, mut b: &File, mut bytes: u64, job: &Job) -> Result<(), PlatformError> {
    let (mut left, mut right) = (vec![0; BUFFER], vec![0; BUFFER]);
    while bytes != 0 {
        ensure_active(job)?;
        let count = bytes.min(BUFFER as u64) as usize;
        a.read_exact(&mut left[..count]).map_err(io_err)?;
        b.read_exact(&mut right[..count]).map_err(io_err)?;
        if left[..count] != right[..count] {
            return Err(fail(S::DedupDifferent));
        }
        bytes -= count as u64;
        job.update(|p| p.bytes += count as u64);
    }
    if a.read(&mut left[..1]).map_err(io_err)? != 0 || b.read(&mut right[..1]).map_err(io_err)? != 0 {
        return Err(fail(S::DedupChanged));
    }
    Ok(())
}

fn recovery(path: &FsPath) -> PlatformError {
    PlatformError::new(-1, flit_core::tr_en!(S::DedupRecovery, path = path.display()))
}

fn replace(sh: &mut impl FileOps, keep: &FsPath, target: &FsPath, job: &Job) -> Result<DedupBackup, PlatformError> {
    ensure_active(job)?;
    let primary = stamp(sh, keep)?;
    let other = stamp(sh, target)?;
    if primary.id == other.id || primary.id.volume != other.id.volume {
        return Err(fail(S::DedupDistinct));
    }
    if primary.size != other.size {
        return Err(fail(S::DedupDifferent));
    }
    let a = open(keep)?;
    let b = open(target)?;
    sh.verify_dedup_data(keep)?;
    compare(&a, &b, primary.size, job)?;
    ensure(sh, keep, &primary)?;
    ensure(sh, target, &other)?;
    let parent = target.parent().ok_or_else(|| fail(S::DedupRegular))?;
    let mut staged = crate::segments::Staging::new(&parent).map_err(io_err)?;
    let original = staged.0.join(b"original");
    let next = staged.0.join(b"replacement");
    fs::hard_link(std_path(target), std_path(&original)).map_err(io_err)?;
    fs::hard_link(std_path(keep), std_path(&next)).map_err(io_err)?;
    ensure(sh, &original, &other)?;
    ensure(sh, &next, &primary)?;
    sh.verify_dedup_data(&next)?;
    ensure(sh, keep, &primary)?;
    ensure(sh, target, &other)?;
    ensure_active(job)?;
    drop(b);
    sh.rename_local_replace(&next, target)?;
    drop(a);
    if sh.file_identity(&original) != Some(other.id) {
        staged.keep();
        return Err(recovery(&original));
    }
    let Some(trash) = sh.keep_dedup_backup(&original, target) else {
        if sh.file_identity(target) == Some(primary.id) && sh.rename_local_replace(&original, target).is_ok() {
            return Err(fail(S::DedupNoBackup));
        }
        staged.keep();
        return Err(recovery(&original));
    };
    Ok(DedupBackup { target: target.clone(), trash, linked: primary.id, saved: other.id })
}

fn restore(sh: &mut impl FileOps, keep: &FsPath, backup: &DedupBackup, job: &Job) -> Result<Option<PlatformError>, PlatformError> {
    ensure_active(job)?;
    let live = stamp(sh, &backup.target)?;
    let saved = stamp(sh, &backup.trash)?;
    if live.id != backup.linked || saved.id != backup.saved || stamp(sh, keep)?.id != live.id || live.size != saved.size {
        return Err(fail(S::DedupChanged));
    }
    let linked = open(&backup.target)?;
    let archived = open(&backup.trash)?;
    compare(&linked, &archived, live.size, job)?;
    let parent = backup.target.parent().ok_or_else(|| fail(S::DedupRegular))?;
    let staged = crate::segments::Staging::new(&parent).map_err(io_err)?;
    let restored = staged.0.join(b"restored");
    fs::hard_link(std_path(&backup.trash), std_path(&restored)).map_err(io_err)?;
    ensure(sh, &restored, &saved)?;
    ensure(sh, &backup.trash, &saved)?;
    ensure(sh, &backup.target, &live)?;
    ensure_active(job)?;
    drop(linked);
    sh.rename_local_replace(&restored, &backup.target)?;
    drop(archived);

    let cleanup = (sh.file_identity(&backup.trash) == Some(saved.id)).then(|| sh.file_op(&FileOp::Purge(vec![(backup.trash.clone(), backup.target.clone())]), &Job::detached(Resolve::Skip)).error).flatten();
    Ok(cleanup)
}

fn begin(job: &Job, count: usize, bytes: u64) {
    job.update(|p| {
        p.stage = Stage::Running;
        p.total_items = count as u64;
        p.total_bytes = bytes;
    });
}

pub fn run(sh: &mut impl FileOps, op: &FileOp, job: &Job) -> Option<OpOutcome> {
    let mut out = OpOutcome::default();
    match op {
        FileOp::Dedup { keep, candidates, .. } => {
            let total = fs::metadata(std_path(keep)).map_or(0, |m| m.len()).saturating_mul(candidates.len() as u64);
            begin(job, candidates.len(), total);
            let mut backups = Vec::new();
            for target in candidates {
                match replace(sh, keep, target, job) {
                    Ok(backup) => {
                        out.items.push((target.clone(), target.clone()));
                        backups.push(backup);
                    }
                    Err(e) => {
                        out.error = Some(e);
                        break;
                    }
                }
                job.update(|p| p.items += 1);
            }
            job.performed(FileOp::Dedup { keep: keep.clone(), candidates: candidates.clone(), backups });
        }
        FileOp::RestoreDedup { keep, backups } => {
            let total = fs::metadata(std_path(keep)).map_or(0, |m| m.len()).saturating_mul(backups.len() as u64);
            begin(job, backups.len(), total);
            for backup in backups {
                if job.cancelled() {
                    out.error.get_or_insert_with(|| PlatformError::cancelled(-1, crate::CANCELLED));
                    break;
                }
                match restore(sh, keep, backup, job) {
                    Ok(cleanup) => {
                        out.items.push((backup.target.clone(), backup.target.clone()));
                        if let Some(error) = cleanup {
                            out.error.get_or_insert(error);
                        }
                    }
                    Err(e) => {
                        out.error.get_or_insert(e);
                    }
                }
                job.update(|p| p.items += 1);
            }
        }
        _ => return None,
    }
    Some(out)
}
