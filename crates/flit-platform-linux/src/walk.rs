use crate::fs::{dents, filetime, node_at, open_dir, stat_path};
use crate::mounts::{self, Mount};
use crossbeam_channel::unbounded;
use flit_core::attr;
use flit_platform::{IndexSource, PlatformError};
use flit_search::index::{Checkpoint, IndexRecord, IndexSink};
use flit_search::proto::{VolumeInfo, VolumeState};
use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

const WALK_THREADS: usize = 8;
const REWALK: Duration = Duration::from_secs(15 * 60);
const SWEEP: Duration = Duration::from_secs(30);
const SETTLE_TICKS: i64 = 20_000_000;
const BY_UUID: &str = "/dev/disk/by-uuid";
const SKIPPED: [&str; 4] = ["9p", "drvfs", "virtiofs", "fuse.sshfs"];

pub struct WalkIndex {
    roots: Vec<(String, PathBuf, String)>,
    keys: HashMap<String, String>,
    walked: Vec<(String, Instant)>,
    trees: HashMap<String, Tree>,
}

struct Dir {
    path: PathBuf,
    modified: i64,
    children: Vec<u64>,
}

#[derive(Default)]
pub struct Tree {
    dev: u64,
    dirs: HashMap<u64, Dir>,
    swept: Option<Instant>,
}

struct Job {
    dir: PathBuf,
    id: u64,
    modified: i64,
}

#[derive(Default)]
struct Batch {
    parent: u64,
    dir: PathBuf,
    modified: i64,
    names: Vec<u8>,
    items: Vec<(u64, usize, u64, i64, u32)>,
}

impl Tree {
    fn record(&mut self, b: &Batch) {
        let children = b.items.iter().map(|i| i.0).collect();
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
        let settled = b.modified < filetime(now.as_secs() as i64, 0) - SETTLE_TICKS;
        self.dirs.insert(b.parent, Dir { path: b.dir.clone(), modified: if settled { b.modified } else { i64::MIN }, children });
    }

    fn forget<S: IndexSink>(&mut self, id: u64, sink: &mut S) {
        let mut stack = vec![id];
        while let Some(id) = stack.pop() {
            sink.remove(id);
            if let Some(d) = self.dirs.remove(&id) {
                stack.extend(d.children);
            }
        }
    }

    fn sweep<S: IndexSink>(&mut self, sink: &mut S) -> usize {
        let changed: Vec<Job> = self
            .dirs
            .iter()
            .filter_map(|(&id, d)| match stat_path(&d.path) {
                Ok(m) if m.modified == d.modified => None,
                Ok(m) => Some(Job { dir: d.path.clone(), id, modified: m.modified }),
                Err(_) => Some(Job { dir: d.path.clone(), id, modified: i64::MIN }),
            })
            .collect();
        let read = changed.len();
        let mut seen = HashSet::new();
        let mut stale = Vec::new();
        let mut queue = Vec::new();
        let mut subdirs = Vec::new();
        for job in changed {
            let batch = read_dir(&job, self.dev, &mut subdirs);
            for sub in subdirs.drain(..) {
                if self.dirs.get(&sub.id).is_none_or(|d| d.path != sub.dir) {
                    queue.push(sub);
                }
            }
            if let Some(old) = self.dirs.get(&job.id) {
                stale.extend(old.children.iter().copied());
            }
            self.take(batch, &mut seen, sink);
        }
        while let Some(job) = queue.pop() {
            let batch = read_dir(&job, self.dev, &mut queue);
            self.take(batch, &mut seen, sink);
        }
        for id in stale {
            if !seen.contains(&id) {
                self.forget(id, sink);
            }
        }
        read
    }

    fn take<S: IndexSink>(&mut self, batch: Batch, seen: &mut HashSet<u64>, sink: &mut S) {
        let mut start = 0;
        for &(id, end, size, modified, attrs) in &batch.items {
            sink.upsert(&IndexRecord { id, parent: batch.parent, name: &batch.names[start..end], size, modified, attrs });
            seen.insert(id);
            start = end;
        }
        self.record(&batch);
    }
}

pub fn indexed(mounts: &[Mount], user: &[u8]) -> Vec<Mount> {
    mounts::visible(mounts, user).into_iter().filter(|m| !SKIPPED.contains(&m.fstype.as_str())).collect()
}

fn read_dir(job: &Job, dev: u64, subdirs: &mut Vec<Job>) -> Batch {
    let mut batch = Batch { parent: job.id, dir: job.dir.clone(), modified: job.modified, ..Default::default() };
    let Ok(dir) = open_dir(&job.dir) else { return batch };
    let fd = dir.as_raw_fd();
    let _ = dents(fd, |name, _| {
        let Ok(node) = node_at(fd, name, false) else { return true };
        let bytes = name.to_bytes();
        let meta = node.meta(bytes);
        batch.names.extend_from_slice(bytes);
        batch.items.push((node.ino, batch.names.len(), meta.size, meta.modified, meta.attrs));
        if node.is_dir() && node.dev == dev {
            subdirs.push(Job { dir: job.dir.join(OsStr::from_bytes(bytes)), id: node.ino, modified: meta.modified });
        }
        true
    });
    batch
}

pub fn walk<S: IndexSink>(root: &Path, sink: &mut S, tree: &mut Tree) -> Result<u64, PlatformError> {
    let fail = |e: std::io::Error| PlatformError::new(e.raw_os_error().unwrap_or(-1) as i64, format!("cannot open {}: {e}", root.display()));
    let dir = open_dir(root).map_err(fail)?;
    let top = node_at(dir.as_raw_fd(), c".", false).map_err(fail)?;
    sink.upsert(&IndexRecord { id: top.ino, parent: top.ino, name: b".", size: 0, modified: 0, attrs: attr::DIR });
    let (job_tx, job_rx) = unbounded::<Job>();
    let (out_tx, out_rx) = unbounded::<Batch>();
    let pending = AtomicUsize::new(1);
    let stop = AtomicBool::new(false);
    drop(dir);
    tree.dev = top.dev;
    tree.dirs.clear();
    let _ = job_tx.send(Job { dir: root.to_path_buf(), id: top.ino, modified: top.meta(b".").modified });
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
            tree.record(&batch);
            done += 1;
            sink.progress(done, done + pending.load(Ordering::Relaxed) as u64);
            if sink.cancelled() {
                stop.store(true, Ordering::Relaxed);
            }
        }
    });
    Ok(top.ino)
}

fn volume_key(m: &Mount, uuids: &HashMap<PathBuf, String>) -> Option<String> {
    if let Some(uuid) = mounts::label_of(m, uuids) {
        return Some(uuid);
    }
    let c = std::ffi::CString::new(m.point.clone()).ok()?;
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    let id: [libc::c_int; 2] = unsafe { std::mem::transmute(st.f_fsid) };
    (id != [0, 0]).then(|| format!("fsid-{:08x}{:08x}", id[0] as u32, id[1] as u32))
}

impl WalkIndex {
    pub fn with_roots(roots: Vec<(String, PathBuf, String)>) -> Self {
        Self { roots, keys: HashMap::new(), walked: Vec::new(), trees: HashMap::new() }
    }

    fn root(&self, volume: &str) -> Result<&Path, PlatformError> {
        self.roots.iter().find(|r| r.0 == volume).map(|r| r.1.as_path()).ok_or_else(|| PlatformError::new(-1, format!("unknown volume {volume}")))
    }
}

impl IndexSource for WalkIndex {
    fn new() -> Result<Self, PlatformError> {
        let all = mounts::read().map_err(|e| PlatformError::new(-1, format!("cannot read {}: {e}", mounts::MOUNTINFO)))?;
        let mounts = indexed(&all, &mounts::user_name());
        let uuids = mounts::labels(Path::new(BY_UUID));
        let keys = mounts.iter().filter_map(|m| Some((String::from_utf8_lossy(&m.point).into_owned(), volume_key(m, &uuids)?))).collect();
        let roots = mounts.into_iter().map(|m| (String::from_utf8_lossy(&m.point).into_owned(), m.path().to_path_buf(), m.fstype)).collect();
        Ok(Self { keys, ..Self::with_roots(roots) })
    }

    fn volumes(&self) -> Result<Vec<VolumeInfo>, PlatformError> {
        Ok(self.roots.iter().map(|(id, _, fs)| VolumeInfo { id: id.clone(), label: String::new(), file_system: fs.clone(), files: 0, state: VolumeState::Scanning { progress: 0.0 } }).collect())
    }

    fn scan<S: IndexSink>(&mut self, volume: &str, sink: &mut S) -> Result<Checkpoint, PlatformError> {
        let root = self.root(volume)?.to_path_buf();
        let mut tree = Tree::default();
        let id = walk(&root, sink, &mut tree)?;
        self.trees.insert(volume.to_owned(), tree);
        self.walked.retain(|(v, _)| v != volume);
        self.walked.push((volume.to_owned(), Instant::now()));
        Ok(Checkpoint { journal: id, next: 0 })
    }

    fn changes<S: IndexSink>(&mut self, volume: &str, from: Checkpoint, sink: &mut S) -> Result<Option<Checkpoint>, PlatformError> {
        let fresh = self.walked.iter().any(|(v, at)| v == volume && at.elapsed() < REWALK);
        if !fresh {
            return Ok(None);
        }
        if let Some(tree) = self.trees.get_mut(volume).filter(|t| t.swept.is_none_or(|at| at.elapsed() >= SWEEP)) {
            tree.sweep(sink);
            tree.swept = Some(Instant::now());
        }
        Ok(Some(from))
    }

    fn snapshot_key(&self, volume: &str) -> String {
        self.keys.get(volume).map_or_else(|| volume.to_owned(), |k| format!("{k}{volume}"))
    }
}

#[cfg(test)]
#[path = "../test/walk.rs"]
mod tests;
