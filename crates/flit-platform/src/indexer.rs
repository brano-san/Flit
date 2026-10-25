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

    fn progress(&mut self, done: u64, total: u64) {
        self.progress = if total == 0 { 0.0 } else { (done as f64 / total as f64).clamp(0.0, 1.0) as f32 };
    }
}

pub fn start_indexer<S: IndexSource + 'static>(source: S, shared: Arc<SharedIndex>, snapshots: Option<PathBuf>) -> Result<(), PlatformError> {
    std::thread::Builder::new()
        .name("flit-indexer".into())
        .spawn(move || run(source, shared, snapshots))
        .map(drop)
        .map_err(|e| PlatformError::new(-1, format!("cannot start the indexer: {e}")))
}

fn scan<S: IndexSource>(source: &mut S, shared: &SharedIndex, info: &VolumeInfo) -> Option<Checkpoint> {
    let live = shared.read().volumes.iter().find(|v| v.id == info.id && !v.is_empty()).map(|v| v.live() as u64);
    let mut feed = Feed::new(shared, VolumeInfo { files: live.unwrap_or(0), ..info.clone() }, live.map(|_| empty_volume(&info.id)));
    feed.flush(VolumeState::Scanning { progress: 0.0 });
    match source.scan(&info.id, &mut feed) {
        Ok(cp) => {
            if let Some(mut v) = feed.private.take() {
                v.commit();
                feed.info.files = v.live() as u64;
                let mut e = shared.write();
                if let Some(slot) = e.volume_mut(&info.id) {
                    *slot = v;
                }
                e.version += 1;
            }
            feed.flush(VolumeState::Ready);
            Some(cp)
        }
        Err(e) => {
            feed.private = None;
            feed.flush(VolumeState::Error(e.message));
            None
        }
    }
}

fn load_snapshot(dir: &Path, id: &str) -> Option<(VolumeIndex, Checkpoint)> {
    let bytes = std::fs::read(dir.join(snapshot::file_name(id))).ok()?;
    let (v, cp) = snapshot::load(&bytes).ok()?;
    (v.id == id && (v.root.clone(), v.sep) == root_of(id)).then_some((v, cp))
}

fn save_snapshot(dir: &Path, shared: &SharedIndex, id: &str, cp: Checkpoint) -> std::io::Result<()> {
    let mut buf = Vec::new();
    {
        let e = shared.read();
        let Some(v) = e.volumes.iter().find(|v| v.id == id) else { return Ok(()) };
        snapshot::save(v, cp, &mut buf);
    }
    std::fs::create_dir_all(dir)?;
    let path = dir.join(snapshot::file_name(id));
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, &buf)?;
    std::fs::rename(&tmp, &path)
}

fn run<S: IndexSource>(mut source: S, owner: Arc<SharedIndex>, snapshots: Option<PathBuf>) {
    let shared = &*owner;
    let volumes = match source.volumes() {
        Ok(v) => v,
        Err(e) => {
            shared.set_volume(VolumeInfo { id: String::new(), label: String::new(), file_system: String::new(), files: 0, state: VolumeState::Error(e.message) });
            return;
        }
    };
    {
        let mut e = shared.write();
        for info in &volumes {
            if e.volume_mut(&info.id).is_none() {
                e.volumes.push(empty_volume(&info.id));
            }
            shared.set_volume(info.clone());
        }
    }
    let mut points: Vec<Option<Checkpoint>> = vec![None; volumes.len()];
    let mut retry_at = vec![Instant::now(); volumes.len()];
    let mut dirty = vec![false; volumes.len()];
    let mut saved_at = vec![Instant::now(); volumes.len()];
    if let Some(dir) = &snapshots {
        for (k, info) in volumes.iter().enumerate() {
            let Some((v, cp)) = load_snapshot(dir, &info.id) else { continue };
            let files = v.live() as u64;
            {
                let mut e = shared.write();
                if let Some(slot) = e.volume_mut(&info.id) {
                    *slot = v;
                }
                e.version += 1;
            }
            shared.set_volume(VolumeInfo { files, state: VolumeState::Ready, ..info.clone() });
            points[k] = Some(cp);
        }
    }
    while Arc::strong_count(&owner) > 1 {
        for (k, info) in volumes.iter().enumerate() {
            let mut fresh = false;
            match points[k] {
                None if Instant::now() >= retry_at[k] => {
                    points[k] = scan(&mut source, shared, info);
                    if points[k].is_none() {
                        retry_at[k] = Instant::now() + RETRY;
                    }
                    fresh = points[k].is_some();
                    dirty[k] |= fresh;
                }
                None => {}
                Some(cp) => {
                    let mut feed = Feed::new(shared, info.clone(), None);
                    feed.info.files = shared.read().volumes.iter().find(|v| v.id == info.id).map_or(0, |v| v.live() as u64);
                    match source.changes(&info.id, cp, &mut feed) {
                        Ok(next) => {
                            points[k] = next;
                            feed.flush(VolumeState::Ready);
                            dirty[k] |= feed.dirty;
                        }
                        Err(e) => {
                            points[k] = None;
                            retry_at[k] = Instant::now() + RETRY;
                            feed.flush(VolumeState::Error(e.message));
                        }
                    }
                }
            }
            if let (Some(dir), Some(cp)) = (&snapshots, points[k]) {
                if dirty[k] && (fresh || saved_at[k].elapsed() >= SAVE_EVERY) {
                    dirty[k] = save_snapshot(dir, shared, &info.id, cp).is_err();
                    saved_at[k] = Instant::now();
                }
            }
        }
        std::thread::sleep(POLL);
    }
}

#[cfg(test)]
#[path = "../test/indexer.rs"]
mod tests;
