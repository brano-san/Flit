use super::*;
use flit_core::listing::Meta;
use flit_core::ui::{Bitmap, IconRequest};
use std::collections::HashMap;

struct MemFs(HashMap<Vec<u8>, Vec<(&'static str, bool, u64)>>);

impl Fs for MemFs {
    fn read_dir(&self, dir: &FsPath, out: &mut ListingBuilder<'_>) -> Result<(), FsError> {
        let items = self.0.get(dir.as_bytes()).ok_or(FsError { kind: flit_core::FsErrorKind::NotFound, code: 2 })?;
        for chunk in items.chunks(2) {
            for &(n, d, s) in chunk {
                out.push(n.as_bytes(), Meta { size: s, attrs: if d { attr::DIR } else { 0 }, ..Default::default() });
            }
            if !out.batch_done() {
                break;
            }
        }
        Ok(())
    }
    fn seek_penalty(&self, _: &FsPath) -> Option<bool> {
        Some(false)
    }
}

struct NoWatch;
impl Watcher for NoWatch {
    fn new(_: Sender<ProviderEvent>, _: Wake) -> Result<Self, PlatformError> {
        Ok(NoWatch)
    }
    fn watch(&mut self, _: TabId, _: u64, _: Option<&FsPath>) {}
}

struct NoShell;
impl Shell for NoShell {
    fn new() -> Result<Self, PlatformError> {
        Ok(NoShell)
    }
    fn icon(&mut self, _: &IconRequest) -> Option<Bitmap> {
        None
    }
    fn type_name(&mut self, _: &[u8]) -> Option<String> {
        None
    }
    fn open(&mut self, _: &FsPath) -> Result<(), PlatformError> {
        Err(PlatformError::new(5, "denied"))
    }
}
impl Trash for NoShell {
    fn trash(&mut self, _: &[FsPath]) -> Result<(), PlatformError> {
        Ok(())
    }
}

fn p(s: &str) -> FsPath {
    FsPath::parse(s).unwrap()
}

fn fs() -> MemFs {
    let mut m = HashMap::new();
    m.insert(p(r"C:\r").as_bytes().to_vec(), vec![("b.txt", false, 5), ("sub", true, 0), ("a.txt", false, 7), ("c", false, 1), ("d", false, 1)]);
    m.insert(p(r"C:\r\sub").as_bytes().to_vec(), vec![("x", false, 100), ("deep", true, 0)]);
    m.insert(p(r"C:\r\sub\deep").as_bytes().to_vec(), vec![("y", false, 20)]);
    MemFs(m)
}

#[test]
fn list_streams_sorted_snapshots_and_sizes() {
    let (tx, rx) = unbounded();
    let provider = LocalProvider::new::<_, NoWatch, NoShell>(fs(), 2, tx, Arc::new(|| {})).unwrap();
    let slot = Arc::new(SnapshotSlot::default());
    let cancel = GenToken::default();
    cancel.set(3);
    provider.submit(ProviderRequest::List { tab: 1, gen: 3, path: p(r"C:\r"), view: ViewSpec::default(), case: CaseMode::Insensitive, out: slot.clone(), cancel: cancel.clone() });
    let snap = loop {
        if let Some(s) = slot.take().filter(|s| s.done) {
            break s;
        }
        std::thread::yield_now();
    };
    let names: Vec<_> = snap.order.iter().map(|&i| String::from_utf8_lossy(snap.listing.name(i)).into_owned()).collect();
    assert_eq!(names, ["sub", "a.txt", "b.txt", "c", "d"]);
    assert_eq!(snap.gen, 3);
    provider.submit(ProviderRequest::DirSizes { tab: 1, gen: 3, dir: p(r"C:\r"), items: vec![(1, b"sub".to_vec())], cancel: cancel.clone() });
    assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), ProviderEvent::DirSize { tab: 1, gen: 3, idx: 1, size: 120 });
    provider.submit(ProviderRequest::Open { tab: 4, path: p(r"C:\r\a.txt") });
    assert!(matches!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), ProviderEvent::OpDone { tab: 4, error: Some(_) }));
    provider.submit(ProviderRequest::Op { tab: 9, op: FileOp::Trash(vec![p(r"C:\r\c")]) });
    assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), ProviderEvent::OpDone { tab: 9, error: None });
}

static LOG: Mutex<Vec<String>> = Mutex::new(Vec::new());

struct LogFs;
impl Fs for LogFs {
    fn read_dir(&self, dir: &FsPath, _: &mut ListingBuilder<'_>) -> Result<(), FsError> {
        LOG.lock().unwrap().push(format!("list {}", dir.display()));
        Ok(())
    }
    fn seek_penalty(&self, _: &FsPath) -> Option<bool> {
        None
    }
}

struct SlowLogWatch;
impl Watcher for SlowLogWatch {
    fn new(_: Sender<ProviderEvent>, _: Wake) -> Result<Self, PlatformError> {
        Ok(SlowLogWatch)
    }
    fn watch(&mut self, tab: TabId, gen: u64, dir: Option<&FsPath>) {
        std::thread::sleep(Duration::from_millis(30));
        LOG.lock().unwrap().push(match dir {
            Some(d) => format!("watch {tab} {gen} {}", d.display()),
            None => format!("stop {tab}"),
        });
    }
}

#[test]
fn watch_is_armed_before_the_listing_starts() {
    let (tx, _rx) = unbounded();
    let provider = LocalProvider::new::<_, SlowLogWatch, NoShell>(LogFs, 2, tx, Arc::new(|| {})).unwrap();
    let slot = Arc::new(SnapshotSlot::default());
    let cancel = GenToken::default();
    cancel.set(7);
    provider.submit(ProviderRequest::Watch { tab: 1, gen: 7, path: Some(p(r"C:\w")) });
    provider.submit(ProviderRequest::List { tab: 1, gen: 7, path: p(r"C:\w"), view: ViewSpec::default(), case: CaseMode::Insensitive, out: slot.clone(), cancel: cancel.clone() });
    while slot.take().is_none() {
        std::thread::yield_now();
    }
    provider.submit(ProviderRequest::Watch { tab: 1, gen: 7, path: None });
    let deadline = Instant::now() + Duration::from_secs(5);
    while LOG.lock().unwrap().len() < 3 && Instant::now() < deadline {
        std::thread::yield_now();
    }
    assert_eq!(*LOG.lock().unwrap(), [r"watch 1 7 C:\w", r"list C:\w", "stop 1"]);
}

#[test]
fn cancelled_list_publishes_nothing() {
    let slot = SnapshotSlot::default();
    let cancel = GenToken::default();
    cancel.set(2);
    list(&fs(), 1, &p(r"C:\r"), ViewSpec::default(), CaseMode::Insensitive, &slot, &cancel, &(Arc::new(|| {}) as Wake));
    assert!(slot.take().is_none());
}

#[test]
fn wsl_requests_map_to_unc() {
    let req = via_unc("Ubuntu", ProviderRequest::Open { tab: 1, path: FsPath::parse("/home/a:b").unwrap() }).unwrap();
    let ProviderRequest::Open { path, .. } = req else { panic!() };
    assert_eq!(path.display(), "\\\\wsl.localhost\\Ubuntu\\home\\a\u{F03A}b");
}

#[test]
fn unmappable_wsl_paths_never_reach_win32() {
    let bad = FsPath::parse_bytes(b"/tmp/\xFF").unwrap();
    assert!(via_unc("Ubuntu", ProviderRequest::Op { tab: 1, op: FileOp::Trash(vec![bad.clone()]) }).is_none());
    let out = Arc::new(SnapshotSlot::default());
    let req = ProviderRequest::List { tab: 1, gen: 3, path: bad, view: ViewSpec::default(), case: CaseMode::Sensitive, out: out.clone(), cancel: GenToken::default() };
    assert!(via_unc("Ubuntu", req).is_none());
    let snap = out.take().unwrap();
    assert!(snap.done && snap.error.is_some() && snap.gen == 3);
}
