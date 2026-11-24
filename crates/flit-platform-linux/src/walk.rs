use crate::fs::{dents, filetime, node_at, open_dir, stat_path, DENTS_BUFFER};
use crate::inotify::Notify;
use crate::mounts::{self, Mount};
use flit_core::attr;
use flit_platform::walk::{Media, Throttle};
use flit_platform::{IndexSource, PlatformError};
use flit_search::index::{Checkpoint, IndexRecord, IndexSink};
use flit_search::proto::{VolumeInfo, VolumeState};
use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const SYS_BLOCK: &str = "/sys/dev/block";
const REWALK: Duration = Duration::from_secs(15 * 60);
const SWEEP: Duration = Duration::from_secs(30);
const SETTLE_TICKS: i64 = 20_000_000;
const BY_UUID: &str = "/dev/disk/by-uuid";
const SKIPPED: [&str; 4] = ["9p", "drvfs", "virtiofs", "fuse.sshfs"];
const MAX_WATCHES: &str = "/proc/sys/fs/inotify/max_user_watches";
const WATCH_SHARE: u64 = 8;
const WATCH_CAP: usize = 16_384;
const WATCH_DEFAULT: usize = 2048;

type Key = (String, u64);

struct Live {
    notify: Notify<Key>,
    dirty: Arc<Mutex<HashSet<Key>>>,
    watched: HashMap<String, HashSet<u64>>,
    synced: HashMap<String, u64>,
}

pub type Roots = Vec<(String, PathBuf, String)>;

pub struct WalkIndex {
    roots: Roots,
    keys: HashMap<String, String>,
    walked: Vec<(String, Instant)>,
    trees: HashMap<String, Tree>,
    follow: bool,
    home: Option<PathBuf>,
    budget: usize,
    live: Option<Live>,
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
    shape: u64,
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
        let old = self.dirs.insert(b.parent, Dir { path: b.dir.clone(), modified: if settled { b.modified } else { i64::MIN }, children });
        self.shape += old.is_none_or(|d| d.path != b.dir) as u64;
    }

    fn forget<S: IndexSink>(&mut self, id: u64, sink: &mut S) {
        let mut stack = vec![id];
        while let Some(id) = stack.pop() {
            sink.remove(id);
            if let Some(d) = self.dirs.remove(&id) {
                self.shape += 1;
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
        self.reread(changed, sink);
        read
    }

    fn reread<S: IndexSink>(&mut self, changed: Vec<Job>, sink: &mut S) {
        let remote = mounts::read_remote_points();
        let mut seen = HashSet::new();
        let mut stale = Vec::new();
        let mut queue = Vec::new();
        let mut subdirs = Vec::new();
        for job in changed {
            let batch = read_dir(&job, self.dev, &mut subdirs, &remote);
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
            let batch = read_dir(&job, self.dev, &mut queue, &remote);
            self.take(batch, &mut seen, sink);
        }
        for id in stale {
            if !seen.contains(&id) {
                self.forget(id, sink);
            }
        }
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

pub fn budget(limit: Option<u64>) -> usize {
    limit.map_or(WATCH_DEFAULT, |l| ((l / WATCH_SHARE) as usize).min(WATCH_CAP))
}

pub fn watchable(dir: &Path, home: &Path) -> bool {
    dir.strip_prefix(home).is_ok_and(|rest| !rest.components().any(|c| c.as_os_str().as_bytes().starts_with(b".")))
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn job_at(id: u64, dir: &Path, same: bool) -> Option<Job> {
    let m = std::fs::metadata(dir).ok().filter(|m| !same || m.ino() == id);
    if same && m.is_none() {
        return None;
    }
    Some(Job { dir: dir.to_path_buf(), id, modified: m.map_or(i64::MIN, |m| filetime(m.mtime(), m.mtime_nsec())) })
}

pub fn indexed(mounts: &[Mount], user: &[u8]) -> Vec<Mount> {
    mounts::visible(mounts, user).into_iter().filter(|m| !SKIPPED.contains(&m.fstype.as_str())).collect()
}

fn indexable(node: &crate::fs::Node, dev: u64, parent: u64) -> bool {
    node.dev == dev && !node.mounted_on(dev) && node.ino != parent
}

fn read_dir(job: &Job, dev: u64, subdirs: &mut Vec<Job>, remote: &[PathBuf]) -> Batch {
    let mut batch = Batch { parent: job.id, dir: job.dir.clone(), modified: job.modified, ..Default::default() };
    if remote.iter().any(|p| job.dir.starts_with(p)) {
        return batch;
    }
    let children = mounts::remote_children(&job.dir, remote);
    let Ok(dir) = open_dir(&job.dir) else { return batch };
    let fd = dir.as_raw_fd();
    let _ = dents(fd, |name, _| {
        if children.contains(&name.to_bytes()) {
            return true;
        }
        let Ok(node) = node_at(fd, name, false) else { return true };
        if !indexable(&node, dev, job.id) {
            return true;
        }
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
    walk_with(root, sink, tree, None)
}

fn walk_with<S: IndexSink>(root: &Path, sink: &mut S, tree: &mut Tree, throttle: Option<Throttle>) -> Result<u64, PlatformError> {
    let _idle = crate::io_priority::IdleIo::enter();
    let fail = |e: std::io::Error| PlatformError::from_io(&e, format!("cannot open {}: {e}", root.display()));
    let dir = open_dir(root).map_err(fail)?;
    let top = node_at(dir.as_raw_fd(), c".", false).map_err(fail)?;
    sink.upsert(&IndexRecord { id: top.ino, parent: top.ino, name: b".", size: 0, modified: 0, attrs: attr::DIR });
    drop(dir);
    tree.dev = top.dev;
    tree.dirs.clear();
    let first = Job { dir: root.to_path_buf(), id: top.ino, modified: top.meta(b".").modified };
    let remote = mounts::read_remote_points();
    let list = |job: &Job, subdirs: &mut Vec<Job>| read_dir(job, top.dev, subdirs, &remote);
    let throttle = throttle.unwrap_or_else(|| Throttle::new(media(Path::new(SYS_BLOCK), top.dev), DENTS_BUFFER));
    flit_platform::walk::walk(first, throttle, list, |batch, done, total| {
        let mut start = 0;
        for &(id, end, size, modified, attrs) in &batch.items {
            sink.upsert(&IndexRecord { id, parent: batch.parent, name: &batch.names[start..end], size, modified, attrs });
            start = end;
        }
        tree.record(&batch);
        sink.progress(done, total);
        !sink.cancelled()
    });
    Ok(top.ino)
}

pub(crate) fn media(sys_block: &Path, dev: u64) -> Media {
    let block = sys_block.join(format!("{}:{}", libc::major(dev), libc::minor(dev)));
    block_media(&block, 0)
}

fn block_media(block: &Path, depth: u8) -> Media {
    if depth >= 16 {
        return Media::Unknown;
    }
    let slaves: Vec<_> = std::fs::read_dir(block.join("slaves")).into_iter().flatten().filter_map(Result::ok).map(|e| block_media(&e.path(), depth + 1)).collect();
    if slaves.contains(&Media::Spinning) {
        return Media::Spinning;
    }
    if !slaves.is_empty() {
        return if slaves.iter().all(|m| *m == Media::Solid) { Media::Solid } else { Media::Unknown };
    }
    let flag = ["queue/rotational", "../queue/rotational"].iter().find_map(|p| std::fs::read_to_string(block.join(p)).ok());
    match flag.as_deref().map(str::trim) {
        Some("0") => Media::Solid,
        Some("1") => Media::Spinning,
        _ => Media::Unknown,
    }
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

fn discover() -> Result<(Roots, HashMap<String, String>), PlatformError> {
    let all = mounts::read().map_err(|e| PlatformError::new(-1, format!("cannot read {}: {e}", mounts::MOUNTINFO)))?;
    let mounts = indexed(&all, &mounts::user_name());
    let uuids = mounts::labels(Path::new(BY_UUID));
    let keys = mounts.iter().filter_map(|m| Some((String::from_utf8_lossy(&m.point).into_owned(), volume_key(m, &uuids)?))).collect();
    let roots = mounts.into_iter().map(|m| (String::from_utf8_lossy(&m.point).into_owned(), m.path().to_path_buf(), m.fstype)).collect();
    Ok((roots, keys))
}

impl WalkIndex {
    pub fn with_roots(roots: Roots) -> Self {
        Self { roots, keys: HashMap::new(), walked: Vec::new(), trees: HashMap::new(), follow: false, home: None, budget: 0, live: None }
    }

    pub fn watching(mut self, home: PathBuf, budget: usize) -> Self {
        self.home = Some(home);
        self.budget = budget;
        self
    }

    fn sync(&mut self, volume: &str) {
        let (Some(home), Some(tree)) = (&self.home, self.trees.get(volume)) else { return };
        if self.budget == 0 || self.live.as_ref().is_some_and(|l| l.synced.get(volume) == Some(&tree.shape)) {
            return;
        }
        if self.live.is_none() {
            let dirty: Arc<Mutex<HashSet<Key>>> = Arc::default();
            let mark = dirty.clone();
            let Ok(notify) = Notify::new(move |key, _| {
                lock(&mark).insert(key);
            }) else {
                self.budget = 0;
                return;
            };
            self.live = Some(Live { notify, dirty, watched: HashMap::new(), synced: HashMap::new() });
        }
        let Some(live) = self.live.as_mut() else { return };
        let mut watched = live.watched.remove(volume).unwrap_or_default();
        watched.retain(|id| {
            let keep = tree.dirs.get(id).is_some_and(|d| watchable(&d.path, home));
            if !keep {
                live.notify.unwatch(&(volume.to_owned(), *id));
            }
            keep
        });
        let used = live.watched.values().map(HashSet::len).sum::<usize>() + watched.len();
        let mut fresh: Vec<(u64, &Path)> = tree.dirs.iter().filter(|(id, d)| !watched.contains(id) && watchable(&d.path, home)).map(|(id, d)| (*id, d.path.as_path())).collect();
        fresh.sort_by(|a, b| a.1.components().count().cmp(&b.1.components().count()).then_with(|| a.1.cmp(b.1)));
        for (id, path) in fresh.into_iter().take(self.budget.saturating_sub(used)) {
            match live.notify.watch((volume.to_owned(), id), path) {
                Ok(()) => {
                    watched.insert(id);
                }
                Err(e) if e.raw_os_error() == Some(libc::ENOSPC) => break,
                Err(_) => {}
            }
        }
        live.watched.insert(volume.to_owned(), watched);
        live.synced.insert(volume.to_owned(), tree.shape);
    }

    fn unwatch_volume(&mut self, volume: &str) {
        let Some(live) = self.live.as_mut() else { return };
        for id in live.watched.remove(volume).unwrap_or_default() {
            live.notify.unwatch(&(volume.to_owned(), id));
        }
        live.synced.remove(volume);
        lock(&live.dirty).retain(|k| k.0 != volume);
    }

    fn take_dirty(&self, volume: &str) -> Vec<u64> {
        let Some(live) = &self.live else { return Vec::new() };
        let mut dirty = lock(&live.dirty);
        let ids = dirty.iter().filter(|k| k.0 == volume).map(|k| k.1).collect();
        dirty.retain(|k| k.0 != volume);
        ids
    }

    pub fn retarget(&mut self, roots: Roots, keys: HashMap<String, String>) -> bool {
        let same = |id: &str| self.roots.iter().find(|r| r.0 == id) == roots.iter().find(|r| r.0 == id) && self.keys.get(id) == keys.get(id);
        let changed: Vec<String> = self.roots.iter().map(|r| r.0.clone()).filter(|id| !same(id)).collect();
        let added = roots.iter().any(|r| !self.roots.iter().any(|o| o.0 == r.0));
        if changed.is_empty() && !added {
            return false;
        }
        for id in &changed {
            self.trees.remove(id);
            self.walked.retain(|(v, _)| v != id);
            self.unwatch_volume(id);
        }
        self.roots = roots;
        self.keys = keys;
        true
    }

    fn root(&self, volume: &str) -> Result<&Path, PlatformError> {
        self.roots.iter().find(|r| r.0 == volume).map(|r| r.1.as_path()).ok_or_else(|| PlatformError::new(-1, format!("unknown volume {volume}")))
    }
}

impl IndexSource for WalkIndex {
    fn new() -> Result<Self, PlatformError> {
        let (roots, keys) = discover()?;
        let limit = std::fs::read_to_string(MAX_WATCHES).ok().and_then(|t| t.trim().parse().ok());
        let index = Self { keys, follow: true, ..Self::with_roots(roots) };
        Ok(match std::env::var_os("HOME").map(PathBuf::from).filter(|h| h.is_absolute()) {
            Some(home) => index.watching(home, budget(limit)),
            None => index,
        })
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
        self.sync(volume);
        Ok(Checkpoint { journal: id, next: 0 })
    }

    fn changes<S: IndexSink>(&mut self, volume: &str, from: Checkpoint, sink: &mut S) -> Result<Option<Checkpoint>, PlatformError> {
        let Some(walked) = self.walked.iter().position(|(v, _)| v == volume) else { return Ok(None) };
        let dirty = self.take_dirty(volume);
        let watched = self.live.as_ref().and_then(|l| l.watched.get(volume));
        let Some(tree) = self.trees.get_mut(volume) else { return Ok(None) };
        let rewalk = self.walked[walked].1.elapsed() >= REWALK;
        let sweep = tree.swept.is_none_or(|at| at.elapsed() >= SWEEP);
        let _idle = (rewalk || !dirty.is_empty() || sweep).then(crate::io_priority::IdleIo::enter);
        if rewalk {
            let jobs = tree.dirs.iter().filter(|(id, _)| watched.is_none_or(|w| !w.contains(id))).filter_map(|(&id, d)| job_at(id, &d.path, false)).collect();
            tree.reread(jobs, sink);
            self.walked[walked].1 = Instant::now();
        }
        if !dirty.is_empty() {
            let jobs = dirty.into_iter().filter_map(|id| job_at(id, &tree.dirs.get(&id)?.path, true)).collect();
            tree.reread(jobs, sink);
        }
        if sweep {
            tree.sweep(sink);
            tree.swept = Some(Instant::now());
        }
        self.sync(volume);
        Ok(Some(from))
    }

    fn snapshot_key(&self, volume: &str) -> String {
        format!("walk-v2:{}", self.keys.get(volume).map_or_else(|| volume.to_owned(), |k| format!("{k}{volume}")))
    }

    fn refresh(&mut self) -> bool {
        self.follow && discover().is_ok_and(|(roots, keys)| self.retarget(roots, keys))
    }
}

#[cfg(test)]
#[path = "../test/walk.rs"]
mod tests;
