use crate::fs::{dents, node_at, open_dir};
use crate::mounts::{self, Mount};
use crossbeam_channel::unbounded;
use flit_core::attr;
use flit_platform::{IndexSource, PlatformError};
use flit_search::index::{Checkpoint, IndexRecord, IndexSink};
use flit_search::proto::{VolumeInfo, VolumeState};
use std::ffi::OsStr;
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

const WALK_THREADS: usize = 8;
const REWALK: Duration = Duration::from_secs(15 * 60);
const SKIPPED: [&str; 4] = ["9p", "drvfs", "virtiofs", "fuse.sshfs"];

pub struct WalkIndex {
    roots: Vec<(String, PathBuf, String)>,
    walked: Vec<(String, Instant)>,
}

struct Job {
    dir: PathBuf,
    id: u64,
}

#[derive(Default)]
struct Batch {
    parent: u64,
    names: Vec<u8>,
    items: Vec<(u64, usize, u64, i64, u32)>,
}

pub fn indexed(mounts: &[Mount], user: &[u8]) -> Vec<Mount> {
    mounts::visible(mounts, user).into_iter().filter(|m| !SKIPPED.contains(&m.fstype.as_str())).collect()
}

fn read_dir(job: &Job, dev: u64, subdirs: &mut Vec<Job>) -> Batch {
    let mut batch = Batch { parent: job.id, ..Default::default() };
    let Ok(dir) = open_dir(&job.dir) else { return batch };
    let fd = dir.as_raw_fd();
    let _ = dents(fd, |name, _| {
        let Ok(node) = node_at(fd, name, false) else { return true };
        let bytes = name.to_bytes();
        let meta = node.meta(bytes);
        batch.names.extend_from_slice(bytes);
        batch.items.push((node.ino, batch.names.len(), meta.size, meta.modified, meta.attrs));
        if node.is_dir() && node.dev == dev {
            subdirs.push(Job { dir: job.dir.join(OsStr::from_bytes(bytes)), id: node.ino });
        }
        true
    });
    batch
}

pub fn walk<S: IndexSink>(root: &Path, sink: &mut S) -> Result<u64, PlatformError> {
    let fail = |e: std::io::Error| PlatformError::new(e.raw_os_error().unwrap_or(-1) as i64, format!("cannot open {}: {e}", root.display()));
    let dir = open_dir(root).map_err(fail)?;
    let top = node_at(dir.as_raw_fd(), c".", false).map_err(fail)?;
    sink.upsert(&IndexRecord { id: top.ino, parent: top.ino, name: b".", size: 0, modified: 0, attrs: attr::DIR });
    let (job_tx, job_rx) = unbounded::<Job>();
    let (out_tx, out_rx) = unbounded::<Batch>();
    let pending = AtomicUsize::new(1);
    let stop = AtomicBool::new(false);
    drop(dir);
    let _ = job_tx.send(Job { dir: root.to_path_buf(), id: top.ino });
    std::thread::scope(|s| {
        for k in 0..WALK_THREADS {
            let (job_tx, job_rx, out_tx, pending, stop) = (job_tx.clone(), job_rx.clone(), out_tx.clone(), &pending, &stop);
            let _ = std::thread::Builder::new().name(format!("flit-walk-{k}")).spawn_scoped(s, move || {
                let mut subdirs = Vec::new();
                while pending.load(Ordering::Acquire) > 0 && !stop.load(Ordering::Relaxed) {
                    let Ok(job) = job_rx.recv_timeout(Duration::from_millis(10)) else { continue };
                    let batch = read_dir(&job, top.dev, &mut subdirs);
                    pending.fetch_add(subdirs.len(), Ordering::AcqRel);
                    for sub in subdirs.drain(..) {
                        let _ = job_tx.send(sub);
                    }
                    let _ = out_tx.send(batch);
                    pending.fetch_sub(1, Ordering::AcqRel);
                }
            });
        }
        drop(out_tx);
        let mut done = 0u64;
        for batch in out_rx {
            let mut start = 0;
            for &(id, end, size, modified, attrs) in &batch.items {
                sink.upsert(&IndexRecord { id, parent: batch.parent, name: &batch.names[start..end], size, modified, attrs });
                start = end;
            }
            done += 1;
            sink.progress(done, done + pending.load(Ordering::Relaxed) as u64);
            if sink.cancelled() {
                stop.store(true, Ordering::Relaxed);
            }
        }
    });
    Ok(top.ino)
}

impl WalkIndex {
    pub fn with_roots(roots: Vec<(String, PathBuf, String)>) -> Self {
        Self { roots, walked: Vec::new() }
    }

    fn root(&self, volume: &str) -> Result<&Path, PlatformError> {
        self.roots.iter().find(|r| r.0 == volume).map(|r| r.1.as_path()).ok_or_else(|| PlatformError::new(-1, format!("unknown volume {volume}")))
    }
}

impl IndexSource for WalkIndex {
    fn new() -> Result<Self, PlatformError> {
        let all = mounts::read().map_err(|e| PlatformError::new(-1, format!("cannot read {}: {e}", mounts::MOUNTINFO)))?;
        let roots = indexed(&all, &mounts::user_name()).into_iter().map(|m| (String::from_utf8_lossy(&m.point).into_owned(), m.path().to_path_buf(), m.fstype)).collect();
        Ok(Self::with_roots(roots))
    }

    fn volumes(&self) -> Result<Vec<VolumeInfo>, PlatformError> {
        Ok(self.roots.iter().map(|(id, _, fs)| VolumeInfo { id: id.clone(), label: String::new(), file_system: fs.clone(), files: 0, state: VolumeState::Scanning { progress: 0.0 } }).collect())
    }

    fn scan<S: IndexSink>(&mut self, volume: &str, sink: &mut S) -> Result<Checkpoint, PlatformError> {
        let root = self.root(volume)?.to_path_buf();
        let id = walk(&root, sink)?;
        self.walked.retain(|(v, _)| v != volume);
        self.walked.push((volume.to_owned(), Instant::now()));
        Ok(Checkpoint { journal: id, next: 0 })
    }

    fn changes<S: IndexSink>(&mut self, volume: &str, from: Checkpoint, _: &mut S) -> Result<Option<Checkpoint>, PlatformError> {
        let fresh = self.walked.iter().any(|(v, at)| v == volume && at.elapsed() < REWALK);
        Ok(fresh.then_some(from))
    }
}

#[cfg(test)]
#[path = "../test/walk.rs"]
mod tests;
