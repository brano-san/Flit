use super::*;
use crate::OpOutcome;
use flit_core::listing::Meta;
use flit_core::ui::{Bitmap, IconRequest};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

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
impl FileOps for NoShell {
    fn file_op(&mut self, op: &FileOp) -> OpOutcome {
        let items = match op {
            FileOp::Trash(v) => v.iter().map(|p| (p.clone(), p.clone())).collect(),
            _ => Vec::new(),
        };
        OpOutcome { items, error: None }
    }
    fn clipboard_files(&mut self) -> Result<(Vec<FsPath>, bool), PlatformError> {
        Err(PlatformError::new(-1, "empty"))
    }
    fn clear_clipboard(&mut self) {}
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
    assert!(matches!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), ProviderEvent::OpDone { tab: 4, id: 0, error: Some(_), report: None }));
    let trash = FileOp::Trash(vec![p(r"C:\r\c")]);
    provider.submit(ProviderRequest::Op { tab: 9, id: 3, op: trash.clone() });
    let report = Some(OpReport { op: trash, items: vec![(p(r"C:\r\c"), p(r"C:\r\c"))] });
    assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), ProviderEvent::OpDone { tab: 9, id: 3, error: None, report });
}

static RELEASE_OP: AtomicBool = AtomicBool::new(false);

struct BusyShell;
impl Shell for BusyShell {
    fn new() -> Result<Self, PlatformError> {
        Ok(BusyShell)
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
impl FileOps for BusyShell {
    fn file_op(&mut self, _: &FileOp) -> OpOutcome {
        while !RELEASE_OP.load(Ordering::Acquire) {
            std::thread::sleep(Duration::from_millis(1));
        }
        OpOutcome::default()
    }
    fn clipboard_files(&mut self) -> Result<(Vec<FsPath>, bool), PlatformError> {
        Err(PlatformError::new(-1, "empty"))
    }
    fn clear_clipboard(&mut self) {}
}

#[test]
fn open_is_not_queued_behind_a_running_op() {
    let (tx, rx) = unbounded();
    let provider = LocalProvider::new::<_, NoWatch, BusyShell>(fs(), 1, tx, Arc::new(|| {})).unwrap();
    provider.submit(ProviderRequest::Op { tab: 1, id: 5, op: FileOp::Copy { items: vec![p(r"C:\r\a.txt")], dest: p(r"C:\r\sub") } });
    provider.submit(ProviderRequest::Open { tab: 2, path: p(r"C:\r\b.txt") });
    assert!(matches!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), ProviderEvent::OpDone { tab: 2, id: 0, error: Some(_), report: None }));
    RELEASE_OP.store(true, Ordering::Release);
    assert!(matches!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), ProviderEvent::OpDone { tab: 1, id: 5, error: None, report: Some(_) }));
}

#[derive(Default)]
struct Clip {
    files: Vec<FsPath>,
    cut: bool,
    ran: Vec<FileOp>,
    cleared: bool,
}

impl FileOps for Clip {
    fn file_op(&mut self, op: &FileOp) -> OpOutcome {
        self.ran.push(op.clone());
        let items = match op {
            FileOp::Copy { items, dest } | FileOp::Move { items, dest } => items.iter().map(|i| (i.clone(), dest.join(i.file_name()))).collect(),
            _ => Vec::new(),
        };
        OpOutcome { items, error: None }
    }
    fn clipboard_files(&mut self) -> Result<(Vec<FsPath>, bool), PlatformError> {
        if self.files.is_empty() {
            return Err(PlatformError::new(-1, "no files"));
        }
        Ok((self.files.clone(), self.cut))
    }
    fn clear_clipboard(&mut self) {
        self.cleared = true;
    }
}

#[test]
fn paste_resolves_the_clipboard_into_copy_or_move() {
    let files = vec![p(r"C:\a\x.txt"), p(r"C:\B\y.txt")];
    let mut cut = Clip { files: files.clone(), cut: true, ..Default::default() };
    let (error, report) = run_op(&mut cut, FileOp::Paste { dest: p(r"C:\b") });
    let moved = FileOp::Move { items: vec![p(r"C:\a\x.txt")], dest: p(r"C:\b") };
    assert_eq!((error, report), (None, Some(OpReport { op: moved.clone(), items: vec![(p(r"C:\a\x.txt"), p(r"C:\b\x.txt"))] })));
    assert_eq!((cut.ran, cut.cleared), (vec![moved], true));

    let mut copy = Clip { files: files.clone(), ..Default::default() };
    let (_, report) = run_op(&mut copy, FileOp::Paste { dest: p(r"C:\b") });
    assert_eq!(report.unwrap().op, FileOp::Copy { items: files, dest: p(r"C:\b") });
    assert!(!copy.cleared);

    let mut here = Clip { files: vec![p(r"C:\b\z")], cut: true, ..Default::default() };
    let (error, report) = run_op(&mut here, FileOp::Paste { dest: p(r"C:\b") });
    assert!(error.is_none() && report.unwrap().items.is_empty() && here.ran.is_empty());

    let (error, report) = run_op(&mut Clip::default(), FileOp::Paste { dest: p(r"C:\b") });
    assert_eq!((error.as_deref().map(|e| e.contains("no files")), report), (Some(true), None));
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
    assert!(via_unc("Ubuntu", ProviderRequest::Op { tab: 1, id: 1, op: FileOp::Trash(vec![bad.clone()]) }).is_none());
    let renamed = via_unc("Ubuntu", ProviderRequest::Op { tab: 1, id: 2, op: FileOp::Rename { item: FsPath::parse("/home/a:b").unwrap(), name: b"c".to_vec() } });
    assert!(matches!(renamed, Some(ProviderRequest::Op { op: FileOp::Rename { item, .. }, .. }) if item.display() == "\\\\wsl.localhost\\Ubuntu\\home\\a\u{F03A}b"));
    let out = Arc::new(SnapshotSlot::default());
    let req = ProviderRequest::List { tab: 1, gen: 3, path: bad, view: ViewSpec::default(), case: CaseMode::Sensitive, out: out.clone(), cancel: GenToken::default() };
    assert!(via_unc("Ubuntu", req).is_none());
    let snap = out.take().unwrap();
    assert!(snap.done && snap.error.is_some() && snap.gen == 3);
}
