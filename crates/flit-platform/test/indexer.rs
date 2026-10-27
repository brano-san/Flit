use super::*;
use flit_core::attr;
use std::sync::Mutex;

type Step = Result<Option<Vec<(u64, &'static str)>>, String>;

#[derive(Clone, Default)]
struct Script {
    scans: Arc<Mutex<u32>>,
    changes: Arc<Mutex<Vec<Step>>>,
}

struct Fake(Script);

impl IndexSource for Fake {
    fn new() -> Result<Self, PlatformError> {
        Ok(Fake(Script::default()))
    }
    fn volumes(&self) -> Result<Vec<VolumeInfo>, PlatformError> {
        Ok(vec![VolumeInfo { id: "T:".into(), label: String::new(), file_system: "TEST".into(), files: 0, state: VolumeState::Scanning { progress: 0.0 } }])
    }
    fn scan<S: IndexSink>(&mut self, _: &str, sink: &mut S) -> Result<Checkpoint, PlatformError> {
        let n = {
            let mut s = self.0.scans.lock().unwrap();
            *s += 1;
            *s
        };
        sink.upsert(&IndexRecord { id: 5, parent: 5, name: b".", size: 0, modified: 0, attrs: attr::DIR });
        for k in 0..10 {
            let name = format!("scan{n}-{k}.txt");
            sink.upsert(&IndexRecord { id: 100 + k, parent: 5, name: name.as_bytes(), size: k, modified: 0, attrs: 0 });
            sink.progress(k + 1, 10);
        }
        Ok(Checkpoint { journal: n as u64, next: 0 })
    }
    fn changes<S: IndexSink>(&mut self, _: &str, from: Checkpoint, sink: &mut S) -> Result<Option<Checkpoint>, PlatformError> {
        let step = { self.0.changes.lock().unwrap().pop() };
        match step {
            None => Ok(Some(from)),
            Some(Err(e)) => Err(PlatformError::new(5, e)),
            Some(Ok(None)) => Ok(None),
            Some(Ok(Some(items))) => {
                for (id, name) in items {
                    if name.is_empty() {
                        sink.remove(id);
                    } else {
                        sink.upsert(&IndexRecord { id, parent: 5, name: name.as_bytes(), size: 1, modified: 0, attrs: 0 });
                    }
                }
                Ok(Some(Checkpoint { next: from.next + 1, ..from }))
            }
        }
    }
}

fn wait_until(what: &str, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !f() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn names(shared: &SharedIndex) -> Vec<String> {
    let e = shared.read();
    let Some(v) = e.volumes.first() else { return Vec::new() };
    let mut out: Vec<String> = (0..v.len() as u32).filter(|&i| v.listed(i)).map(|i| String::from_utf8_lossy(v.name(i)).into_owned()).collect();
    out.sort();
    out
}

#[test]
fn scans_applies_changes_and_rescans() {
    let script = Script::default();
    script.changes.lock().unwrap().extend([Ok(None), Ok(Some(vec![(100, ""), (500, "added.txt")]))]);
    let shared = SharedIndex::new();
    start_indexer(Fake(script.clone()), shared.clone(), None).unwrap();
    wait_until("the first scan", || shared.status().1.first().is_some_and(|v| v.state == VolumeState::Ready));
    assert_eq!(shared.status().1[0].files, 10);
    wait_until("the change batch", || names(&shared).contains(&"added.txt".to_string()));
    assert!(!names(&shared).contains(&"scan1-0.txt".to_string()));
    wait_until("the rescan", || *script.scans.lock().unwrap() == 2 && names(&shared).iter().all(|n| n.starts_with("scan2")));
    assert_eq!(names(&shared).len(), 10);
    assert_eq!(shared.read().volumes.len(), 1);
    script.changes.lock().unwrap().push(Err("volume went away".into()));
    wait_until("the error state", || matches!(&shared.status().1[0].state, VolumeState::Error(m) if m == "volume went away"));
}

#[test]
fn snapshots_let_a_restart_resume_without_scanning() {
    let dir = std::env::temp_dir().join(format!("flit-snapshots-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let file = dir.join(snapshot::file_name("T:"));
    let first = Script::default();
    let shared = SharedIndex::new();
    start_indexer(Fake(first.clone()), shared.clone(), Some(dir.clone())).unwrap();
    wait_until("the snapshot after the first scan", || file.exists());
    let (_, cp) = snapshot::load(&std::fs::read(&file).unwrap()).unwrap();
    assert_eq!(cp, Checkpoint { journal: 1, next: 0 });
    drop(shared);
    std::thread::sleep(POLL * 3);
    let second = Script::default();
    second.changes.lock().unwrap().push(Ok(Some(vec![(500, "after-restart.txt")])));
    let shared = SharedIndex::new();
    start_indexer(Fake(second.clone()), shared.clone(), Some(dir.clone())).unwrap();
    wait_until("the resumed change", || names(&shared).contains(&"after-restart.txt".to_string()));
    assert_eq!(*second.scans.lock().unwrap(), 0, "the snapshot replaces the scan");
    assert!(names(&shared).contains(&"scan1-9.txt".to_string()));
    assert_eq!(shared.status().1[0].state, VolumeState::Ready);
    wait_until("the caught-up snapshot", || std::fs::read(&file).ok().and_then(|b| snapshot::load(&b).ok()).is_some_and(|(v, _)| (0..v.len() as u32).any(|i| v.name(i) == b"after-restart.txt")));
    drop(shared);
    std::thread::sleep(POLL * 3);
    std::fs::write(&file, b"garbage").unwrap();
    let third = Script::default();
    let shared = SharedIndex::new();
    start_indexer(Fake(third.clone()), shared.clone(), Some(dir.clone())).unwrap();
    wait_until("a scan instead of the damaged snapshot", || *third.scans.lock().unwrap() == 1 && shared.status().1[0].state == VolumeState::Ready);
    drop(shared);
    std::thread::sleep(POLL * 3);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn volume_ids_map_to_roots() {
    assert_eq!(root_of("C:"), (br"C:\".to_vec(), b'\\'));
    assert_eq!(root_of(r"D:\data\tree"), (br"D:\data\tree".to_vec(), b'\\'));
    assert_eq!(root_of("/home/u"), (b"/home/u".to_vec(), b'/'));
}
