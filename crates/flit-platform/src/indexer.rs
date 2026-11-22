use crate::{IndexSource, PlatformError};
use flit_search::index::{Checkpoint, IndexRecord, IndexSink, VolumeIndex};
use flit_search::proto::{VolumeInfo, VolumeState};
use flit_search::session::SharedIndex;
use flit_search::snapshot;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

const POLL: Duration = Duration::from_millis(500);
const RETRY: Duration = Duration::from_secs(30);
const FLUSH_MIN: Duration = Duration::from_millis(250);
const FLUSH_COST_FACTOR: u32 = 4;
const CHECK_EVERY: usize = 4096;
const SAVE_EVERY: Duration = Duration::from_secs(300);
const VOLUMES_EVERY: Duration = Duration::from_secs(5);

pub fn root_of(id: &str) -> (Vec<u8>, u8) {
    let b = id.as_bytes();
    if b.len() == 2 && b[1] == b':' {
        return (vec![b[0], b':', b'\\'], b'\\');
    }
    let sep = if id.contains('/') && !id.contains('\\') { b'/' } else { b'\\' };
    (b.to_vec(), sep)
}

fn empty_volume(id: &str) -> VolumeIndex {
    let (root, sep) = root_of(id);
    VolumeIndex::new(id, &root, sep)
}

enum Op {
    Upsert { id: u64, parent: u64, name: (usize, usize), size: u64, modified: i64, attrs: u32 },
    Remove(u64),
}

struct Feed<'a> {
    shared: &'a SharedIndex,
    info: VolumeInfo,
    private: Option<VolumeIndex>,
    ops: Vec<Op>,
    names: Vec<u8>,
    next: Instant,
    progress: f32,
    dirty: bool,
}

impl<'a> Feed<'a> {
    fn new(shared: &'a SharedIndex, info: VolumeInfo, private: Option<VolumeIndex>) -> Self {
        Self { shared, info, private, ops: Vec::new(), names: Vec::new(), next: Instant::now(), progress: 0.0, dirty: false }
    }

    fn flush(&mut self, state: VolumeState) {
        let started = Instant::now();
        if !self.ops.is_empty() {
            let mut e = self.shared.write();
            if let Some(v) = e.volume_mut(&self.info.id) {
                for op in &self.ops {
                    match *op {
                        Op::Upsert { id, parent, name: (a, b), size, modified, attrs } => v.upsert(&IndexRecord { id, parent, name: &self.names[a..b], size, modified, attrs }),
                        Op::Remove(id) => v.remove(id),
                    }
                }
                let changed = v.commit();
                self.dirty |= changed;
                self.info.files = v.live() as u64;
                e.version += changed as u64;
            }
        }
        self.ops.clear();
        self.names.clear();
        self.info.state = state;
        self.shared.set_volume(self.info.clone());
        self.next = Instant::now() + FLUSH_MIN.max(started.elapsed() * FLUSH_COST_FACTOR);
    }

    fn tick(&mut self) {
        if (self.ops.len() + self.private.as_ref().map_or(0, |v| v.pending())).is_multiple_of(CHECK_EVERY) && Instant::now() >= self.next {
            self.flush(VolumeState::Scanning { progress: self.progress });
        }
    }
}

impl IndexSink for Feed<'_> {
    fn upsert(&mut self, r: &IndexRecord<'_>) {
        match &mut self.private {
            Some(v) => v.upsert(r),
            None => {
                let a = self.names.len();
                self.names.extend_from_slice(r.name);
                self.ops.push(Op::Upsert { id: r.id, parent: r.parent, name: (a, self.names.len()), size: r.size, modified: r.modified, attrs: r.attrs });
            }
        }
        self.tick();
    }

    fn remove(&mut self, id: u64) {
        match &mut self.private {
            Some(v) => v.remove(id),
            None => self.ops.push(Op::Remove(id)),
        }
        self.tick();
    }

    fn knows(&self, id: u64) -> bool {
        match &self.private {
            Some(v) => v.knows(id),
            None => self.shared.read().volumes.iter().find(|v| v.id == self.info.id).is_some_and(|v| v.knows(id)),
        }
    }

    fn progress(&mut self, done: u64, total: u64) {
        self.progress = if total == 0 { 0.0 } else { (done as f64 / total as f64).clamp(0.0, 1.0) as f32 };
    }
}

pub fn start_indexer<S: IndexSource + 'static>(source: S, shared: Arc<SharedIndex>, snapshots: Option<PathBuf>) -> Result<(), PlatformError> {
    std::thread::Builder::new().name("flit-indexer".into()).spawn(move || run(source, shared, snapshots)).map(drop).map_err(|e| PlatformError::new(-1, format!("cannot start the indexer: {e}")))
}

fn scan<S: IndexSource>(source: &mut S, shared: &SharedIndex, info: &VolumeInfo) -> Option<Checkpoint> {
    let live = shared.read().volumes.iter().find(|v| v.id == info.id && !v.is_empty()).map(|v| v.live() as u64);
    let mut feed = Feed::new(shared, VolumeInfo { files: live.unwrap_or(0), ..info.clone() }, live.map(|_| empty_volume(&info.id)));
    feed.flush(VolumeState::Scanning { progress: 0.0 });
    match source.scan(&info.id, &mut feed) {
        Ok(cp) => {
            let cold = feed.private.is_none();
            if let Some(mut v) = feed.private.take() {
                v.commit();
                v.shrink();
                feed.info.files = v.live() as u64;
                let mut e = shared.write();
                if let Some(slot) = e.volume_mut(&info.id) {
                    *slot = v;
                }
                e.version += 1;
            }
            feed.flush(VolumeState::Ready);
            if let Some(v) = shared.write().volume_mut(&info.id).filter(|_| cold) {
                v.shrink();
            }
            Some(cp)
        }
        Err(e) => {
            feed.private = None;
            feed.flush(VolumeState::Error(e.message));
            None
        }
    }
}

fn load_snapshot(dir: &Path, id: &str, key: &str) -> Option<(VolumeIndex, Checkpoint)> {
    let bytes = std::fs::read(dir.join(snapshot::file_name(key))).ok()?;
    let (v, cp) = snapshot::load(&bytes).ok()?;
    (v.id == id && (v.root.clone(), v.sep) == root_of(id)).then_some((v, cp))
}

fn save_snapshot(dir: &Path, shared: &SharedIndex, id: &str, key: &str, cp: Checkpoint) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(snapshot::file_name(key));
    let tmp = path.with_extension("tmp");
    let mut bytes = Vec::new();
    {
        let e = shared.read();
        let Some(v) = e.volumes.iter().find(|v| v.id == id) else { return Ok(()) };
        bytes.reserve(v.len() * 48 + v.names.len() + v.lower.len());
        snapshot::save(v, cp, &mut bytes);
    }
    std::fs::write(&tmp, &bytes)?;
    drop(bytes);
    std::fs::rename(&tmp, &path)
}

struct Slot {
    info: VolumeInfo,
    key: String,
    point: Option<Checkpoint>,
    retry_at: Instant,
    dirty: bool,
    saved_at: Option<Instant>,
}

impl Slot {
    fn new(info: VolumeInfo, key: String) -> Self {
        Self { info, key, point: None, retry_at: Instant::now(), dirty: false, saved_at: None }
    }

    fn save(&mut self, dir: &Path, shared: &SharedIndex) {
        if let Some(cp) = self.point.filter(|_| self.dirty) {
            self.dirty = save_snapshot(dir, shared, &self.info.id, &self.key, cp).is_err();
            self.saved_at = Some(Instant::now());
        }
    }

    fn step<S: IndexSource>(&mut self, source: &mut S, shared: &SharedIndex, snapshots: Option<&Path>) {
        let mut fresh = false;
        match self.point {
            None if Instant::now() >= self.retry_at => {
                self.point = scan(source, shared, &self.info);
                if self.point.is_none() {
                    self.retry_at = Instant::now() + RETRY;
                }
                fresh = self.point.is_some();
                self.dirty |= fresh;
            }
            None => {}
            Some(cp) => {
                let mut feed = Feed::new(shared, self.info.clone(), None);
                feed.info.files = shared.read().volumes.iter().find(|v| v.id == self.info.id).map_or(0, |v| v.live() as u64);
                match source.changes(&self.info.id, cp, &mut feed) {
                    Ok(next) => {
                        self.point = next;
                        feed.flush(VolumeState::Ready);
                        self.dirty |= feed.dirty;
                    }
                    Err(e) => {
                        self.point = None;
                        self.retry_at = Instant::now() + RETRY;
                        feed.flush(VolumeState::Error(e.message));
                    }
                }
            }
        }
        if let Some(dir) = snapshots.filter(|_| fresh || self.saved_at.is_none_or(|t| t.elapsed() >= SAVE_EVERY)) {
            self.save(dir, shared);
        }
    }
}

fn adopt<S: IndexSource>(source: &S, shared: &SharedIndex, snapshots: Option<&Path>, infos: Vec<VolumeInfo>) -> Vec<Slot> {
    {
        let mut e = shared.write();
        for info in &infos {
            if e.volume_mut(&info.id).is_none() {
                e.volumes.push(empty_volume(&info.id));
            }
            shared.set_volume(info.clone());
        }
    }
    let mut slots: Vec<Slot> = infos.into_iter().map(|info| Slot::new(info.clone(), source.snapshot_key(&info.id))).collect();
    let Some(dir) = snapshots else { return slots };
    let points: Vec<Option<Checkpoint>> = std::thread::scope(|s| {
        let loads: Vec<_> = slots
            .iter()
            .map(|slot| {
                let (info, key) = (&slot.info, &slot.key);
                s.spawn(move || {
                    let (v, cp) = load_snapshot(dir, &info.id, key)?;
                    let files = v.live() as u64;
                    {
                        let mut e = shared.write();
                        if let Some(at) = e.volume_mut(&info.id) {
                            *at = v;
                        }
                        e.version += 1;
                    }
                    shared.set_volume(VolumeInfo { files, state: VolumeState::Ready, ..info.clone() });
                    Some(cp)
                })
            })
            .collect();
        loads.into_iter().map(|h| h.join().ok().flatten()).collect()
    });
    for (slot, point) in slots.iter_mut().zip(points) {
        slot.point = point;
    }
    slots
}

fn follow<S: IndexSource>(source: &S, shared: &SharedIndex, snapshots: Option<&Path>, slots: &mut Vec<Slot>, now: Vec<VolumeInfo>) {
    let kept = |slot: &Slot| now.iter().any(|info| info.id == slot.info.id) && source.snapshot_key(&slot.info.id) == slot.key;
    let (stay, gone): (Vec<Slot>, Vec<Slot>) = std::mem::take(slots).into_iter().partition(kept);
    *slots = stay;
    for mut slot in gone {
        if let Some(dir) = snapshots {
            slot.save(dir, shared);
        }
        shared.remove_volume(&slot.info.id);
    }
    let added: Vec<VolumeInfo> = now.into_iter().filter(|info| !slots.iter().any(|slot| slot.info.id == info.id)).collect();
    if !added.is_empty() {
        slots.extend(adopt(source, shared, snapshots, added));
    }
}

fn run<S: IndexSource>(mut source: S, owner: Arc<SharedIndex>, snapshots: Option<PathBuf>) {
    let shared = &*owner;
    let snapshots = snapshots.as_deref();
    let volumes = match source.volumes() {
        Ok(v) => v,
        Err(e) => {
            shared.set_volume(VolumeInfo { id: String::new(), label: String::new(), file_system: String::new(), files: 0, state: VolumeState::Error(e.message) });
            return;
        }
    };
    let mut slots = adopt(&source, shared, snapshots, volumes);
    let mut checked = Instant::now();
    while Arc::strong_count(&owner) > 1 {
        if checked.elapsed() >= VOLUMES_EVERY {
            checked = Instant::now();
            if source.refresh() {
                if let Ok(now) = source.volumes() {
                    follow(&source, shared, snapshots, &mut slots, now);
                }
            }
        }
        for slot in &mut slots {
            slot.step(&mut source, shared, snapshots);
        }
        std::thread::sleep(POLL);
    }
}

#[cfg(test)]
#[path = "../test/indexer.rs"]
mod tests;
