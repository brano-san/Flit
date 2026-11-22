use super::*;
use crate::OpOutcome;
use flit_core::listing::Meta;
use flit_core::provider::{MountReply, OpProgress};
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
    fn cluster_size(&self, _: &FsPath) -> u64 {
        4
    }
}

struct NoWatch;

#[test]
fn size_policy_distinguishes_ssd_hdd_and_mounted_network_paths() {
    struct MediaFs { spinning: bool, network: bool }
    impl Fs for MediaFs {
        fn read_dir(&self, _: &FsPath, _: &mut ListingBuilder<'_>) -> Result<(), FsError> { panic!("unexpected directory walk") }
        fn seek_penalty(&self, _: &FsPath) -> Option<bool> { Some(self.spinning) }
        fn remote(&self, _: &FsPath) -> bool { self.network }
        fn dir_sizes(&self, _: &FsPath, items: &[(u32, Vec<u8>)], _: &(dyn Fn() -> bool + Sync), report: &(dyn Fn(u32, DirStats) + Sync)) {
            for (i, _) in items { report(*i, DirStats::default()); }
        }
    }
    for (spinning, network, kind) in [(false, false, 0), (true, false, 1), (false, true, 2), (true, true, 2)] {
        for enabled in 0..3 {
            let (tx, rx) = unbounded();
            let cancel = GenToken::default();
            let gen = 1;
            cancel.set(gen);
            let mut allowed = [false; 3];
            allowed[enabled] = true;
            let job = SizesJob { tab: 1, gen, dir: FsPath::parse("/home/user/HDD").unwrap(), items: vec![(4, b"child".to_vec())], cancel, allowed };
            let wake: Wake = Arc::new(|| {});
            run_sizes(&MediaFs { spinning, network }, job, &tx, &wake, &Progress::default());
            let event = rx.try_recv().unwrap();
            assert_eq!(matches!(event, ProviderEvent::DirSize { idx: 4, .. }), enabled == kind);
            assert_eq!(matches!(event, ProviderEvent::SizesSkipped { .. }), enabled != kind);
        }
    }
}
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
    fn file_op(&mut self, op: &FileOp, _: &Job) -> OpOutcome {
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
    provider.submit(ProviderRequest::DirSizes { tab: 1, gen: 3, dir: p(r"C:\r"), items: vec![(1, b"sub".to_vec())], cancel: cancel.clone(), allowed: [true; 3] });
    let ProviderEvent::DirSize { tab: 1, gen: 3, idx: 1, stats } = rx.recv_timeout(Duration::from_secs(5)).unwrap() else { panic!("folder size expected") };
    assert_eq!((stats.size, stats.partial), (120, false));
    provider.submit(ProviderRequest::Open { tab: 4, path: p(r"C:\r\a.txt") });
    assert!(matches!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), ProviderEvent::OpDone { tab: 4, id: 0, error: Some(_), report: None }));
    let trash = FileOp::Trash(vec![p(r"C:\r\c")]);
    provider.submit(ProviderRequest::Op { tab: 9, id: 3, op: trash.clone(), ctl: OpCtl::default() });
    let report = Some(OpReport::new(trash, vec![(p(r"C:\r\c"), p(r"C:\r\c"))]));
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
    fn file_op(&mut self, _: &FileOp, _: &Job) -> OpOutcome {
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
    provider.submit(ProviderRequest::Op { tab: 1, id: 5, op: FileOp::Copy { items: vec![p(r"C:\r\a.txt")], dest: p(r"C:\r\sub") }, ctl: OpCtl::default() });
    provider.submit(ProviderRequest::Open { tab: 2, path: p(r"C:\r\b.txt") });
    assert!(matches!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), ProviderEvent::OpDone { tab: 2, id: 0, error: Some(_), report: None }));
    RELEASE_OP.store(true, Ordering::Release);
    assert!(matches!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), ProviderEvent::OpDone { tab: 1, id: 5, error: None, report: Some(_) }));
}

static RELEASE_C: AtomicBool = AtomicBool::new(false);

struct GateShell;
impl Shell for GateShell {
    fn new() -> Result<Self, PlatformError> {
        Ok(GateShell)
    }
    fn icon(&mut self, _: &IconRequest) -> Option<Bitmap> {
        None
    }
    fn type_name(&mut self, _: &[u8]) -> Option<String> {
        None
    }
    fn open(&mut self, _: &FsPath) -> Result<(), PlatformError> {
        Ok(())
    }
}
impl FileOps for GateShell {
    fn file_op(&mut self, op: &FileOp, job: &Job) -> OpOutcome {
        job.update(|p| p.stage = Stage::Running);
        while matches!(op, FileOp::Copy { dest, .. } if dest.as_bytes()[0] == b'C') && !RELEASE_C.load(Ordering::Acquire) {
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
fn transfers_to_one_volume_queue_and_others_run_alongside() {
    let (tx, rx) = unbounded();
    let provider = LocalProvider::new::<_, NoWatch, GateShell>(fs(), 1, tx, Arc::new(|| {})).unwrap();
    let copy = |dest: &str| FileOp::Copy { items: vec![p(r"E:\a.txt")], dest: p(dest) };
    let waiting = OpCtl::default();
    provider.submit(ProviderRequest::Op { tab: 1, id: 1, op: copy(r"C:\x"), ctl: OpCtl::default() });
    let next = || rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(matches!(next(), ProviderEvent::OpProgress { id: 1, progress: OpProgress { stage: Stage::Running, .. }, .. }));
    provider.submit(ProviderRequest::Op { tab: 1, id: 2, op: copy(r"c:\y"), ctl: waiting.clone() });
    assert!(matches!(next(), ProviderEvent::OpProgress { id: 2, progress: OpProgress { stage: Stage::Queued, .. }, .. }));
    provider.submit(ProviderRequest::Op { tab: 1, id: 3, op: copy(r"D:\z"), ctl: OpCtl::default() });
    assert!(matches!(next(), ProviderEvent::OpProgress { id: 3, progress: OpProgress { stage: Stage::Running, .. }, .. }));
    assert!(matches!(next(), ProviderEvent::OpDone { id: 3, error: None, .. }));
    waiting.cancel();
    assert!(matches!(next(), ProviderEvent::OpDone { id: 2, error: Some(e), .. } if e.kind == crate::PlatformErrorKind::Cancelled && e.message == CANCELLED));
    RELEASE_C.store(true, Ordering::Release);
    assert!(matches!(next(), ProviderEvent::OpDone { id: 1, error: None, .. }));
    provider.wait_idle(Duration::from_secs(5));
}

struct OverlappingShell;

impl Shell for OverlappingShell {
    fn new() -> Result<Self, PlatformError> {
        Ok(Self)
    }
    fn icon(&mut self, _: &IconRequest) -> Option<Bitmap> {
        None
    }
    fn type_name(&mut self, _: &[u8]) -> Option<String> {
        None
    }
    fn open(&mut self, _: &FsPath) -> Result<(), PlatformError> {
        Ok(())
    }
}

impl FileOps for OverlappingShell {
    fn file_op(&mut self, op: &FileOp, job: &Job) -> OpOutcome {
        job.update(|p| p.stage = Stage::Running);
        if matches!(op, FileOp::Delete(_) | FileOp::Move { .. }) {
            let limit = Instant::now() + Duration::from_secs(5);
            while !job.cancelled() && Instant::now() < limit {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        OpOutcome::default()
    }
    fn clipboard_files(&mut self) -> Result<(Vec<FsPath>, bool), PlatformError> {
        Err(PlatformError::new(-1, "empty"))
    }
    fn clear_clipboard(&mut self) {}
}

#[test]
fn quick_renames_and_restores_wait_for_overlapping_operations_only() {
    for slow in [
        FileOp::Delete(vec![p(r"C:\project\tree")]),
        FileOp::Move { items: vec![p(r"C:\project\tree")], dest: p(r"C:\elsewhere") },
    ] {
        let (tx, rx) = unbounded();
        let local = LocalProvider::new::<_, NoWatch, OverlappingShell>(fs(), 1, tx, Arc::new(|| {})).unwrap();
        let submit = |id, op, ctl| local.submit(ProviderRequest::Op { tab: 1, id, op, ctl });
        let next = || rx.recv_timeout(Duration::from_secs(3)).unwrap();
        let release = OpCtl::default();
        submit(1, slow, release.clone());
        assert!(matches!(next(), ProviderEvent::OpProgress { id: 1, progress: OpProgress { stage: Stage::Running, .. }, .. }));

        let rename = |path| FileOp::Rename { item: p(path), name: b"renamed".to_vec() };
        submit(2, rename(r"c:\PROJECT\tree\child"), OpCtl::default());
        assert!(matches!(next(), ProviderEvent::OpProgress { id: 2, progress: OpProgress { stage: Stage::Queued, .. }, .. }));
        submit(3, FileOp::Restore(vec![(p(r"D:\bin\project"), p(r"C:\project"))]), OpCtl::default());
        assert!(matches!(next(), ProviderEvent::OpProgress { id: 3, progress: OpProgress { stage: Stage::Queued, .. }, .. }));
        submit(4, rename(r"C:\project-other\child"), OpCtl::default());
        assert!(matches!(next(), ProviderEvent::OpProgress { id: 4, progress: OpProgress { stage: Stage::Running, .. }, .. }));
        assert!(matches!(next(), ProviderEvent::OpDone { id: 4, error: None, .. }));

        release.cancel();
        assert!(matches!(next(), ProviderEvent::OpDone { id: 1, .. }));
        for id in [2, 3] {
            assert!(matches!(next(), ProviderEvent::OpProgress { id: seen, progress: OpProgress { stage: Stage::Running, .. }, .. } if seen == id));
            assert!(matches!(next(), ProviderEvent::OpDone { id: seen, error: None, .. } if seen == id));
        }
    }
}

#[derive(Default)]
struct Clip {
    files: Vec<FsPath>,
    cut: bool,
    ran: Vec<FileOp>,
    cleared: bool,
    content: bool,
    data: Option<crate::Pasted>,
}

impl FileOps for Clip {
    fn file_op(&mut self, op: &FileOp, _: &Job) -> OpOutcome {
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
    fn clipboard_content(&mut self) -> bool {
        self.content
    }
    fn clipboard_data(&mut self, dest: &FsPath) -> Option<FileOp> {
        self.data.take()?.file(dest, |p| p.file_name() == b"Pasted image.png")
    }
}

#[test]
fn an_image_or_text_on_the_clipboard_is_pasted_as_a_new_file() {
    let png = b"\x89PNG\r\n\x1a\nrest".to_vec();
    let mut clip = Clip { data: Some(crate::Pasted::Image(png.clone())), ..Default::default() };
    let (error, _) = run_op(&mut clip, FileOp::Paste { dest: p(r"C:\b") });
    let made = FileOp::NewFile { dir: p(r"C:\b"), name: b"Pasted image (2).png".to_vec(), template: flit_core::provider::Template::Data(png) };
    assert_eq!((error, clip.ran), (None, vec![made]), "a taken name gets a number");
    let mut clip = Clip { data: Some(crate::Pasted::Text("hello\r\n".into())), ..Default::default() };
    run_op(&mut clip, FileOp::Paste { dest: p(r"C:\b") });
    assert_eq!(clip.ran, [FileOp::NewFile { dir: p(r"C:\b"), name: b"Pasted text.txt".to_vec(), template: flit_core::provider::Template::Data(b"hello\r\n".to_vec()) }]);
}

#[test]
fn files_on_the_clipboard_win_over_its_text_and_blank_text_pastes_nothing() {
    let mut both = Clip { files: vec![p(r"C:\a\x.txt")], data: Some(crate::Pasted::Text(r"C:\a\x.txt".into())), ..Default::default() };
    run_op(&mut both, FileOp::Paste { dest: p(r"C:\b") });
    assert_eq!(both.ran, [FileOp::Copy { items: vec![p(r"C:\a\x.txt")], dest: p(r"C:\b") }]);
    let mut blank = Clip { data: Some(crate::Pasted::Text(" \r\n".into())), ..Default::default() };
    let (error, report) = run_op(&mut blank, FileOp::Paste { dest: p(r"C:\b") });
    assert!(error.is_some_and(|e| e.contains("no files")) && report.is_none() && blank.ran.is_empty());
    assert!(crate::Pasted::Image(b"GIF89a".to_vec()).file(&p(r"C:\b"), |_| false).is_none(), "only PNG data becomes an image file");
}

#[test]
fn paste_leaves_content_without_file_paths_to_the_platform() {
    let mut clip = Clip { content: true, ..Default::default() };
    let (error, report) = run_op(&mut clip, FileOp::Paste { dest: p(r"C:\b") });
    assert_eq!((error, report.map(|r| r.op)), (None, Some(FileOp::Paste { dest: p(r"C:\b") })));
    assert_eq!((clip.ran, clip.cleared), (vec![FileOp::Paste { dest: p(r"C:\b") }], false));
    let mut both = Clip { files: vec![p(r"C:\a\x.txt")], content: true, ..Default::default() };
    run_op(&mut both, FileOp::Paste { dest: p(r"C:\b") });
    assert_eq!(both.ran, [FileOp::Copy { items: vec![p(r"C:\a\x.txt")], dest: p(r"C:\b") }], "file paths win over other content");
}

#[test]
fn paste_resolves_the_clipboard_into_copy_or_move() {
    let files = vec![p(r"C:\a\x.txt"), p(r"C:\B\y.txt")];
    let mut cut = Clip { files: files.clone(), cut: true, ..Default::default() };
    let (error, report) = run_op(&mut cut, FileOp::Paste { dest: p(r"C:\b") });
    let moved = FileOp::Move { items: vec![p(r"C:\a\x.txt")], dest: p(r"C:\b") };
    assert_eq!((error, report), (None, Some(OpReport::new(moved.clone(), vec![(p(r"C:\a\x.txt"), p(r"C:\b\x.txt"))]))));
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
    list(&fs(), 1, &p(r"C:\r"), ViewSpec::default(), CaseMode::Insensitive, Arc::default(), &slot, &cancel, &(Arc::new(|| {}) as Wake), None);
    assert!(slot.take().is_none());
}

struct IniFs(AtomicUsize);
impl Fs for IniFs {
    fn read_dir(&self, _: &FsPath, _: &mut ListingBuilder<'_>) -> Result<(), FsError> {
        Ok(())
    }
    fn seek_penalty(&self, _: &FsPath) -> Option<bool> {
        None
    }
    fn desktop_ini(&self, _: &FsPath) -> Option<flit_core::desktop_ini::DesktopIni> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Some(flit_core::desktop_ini::DesktopIni { icon: false, kind: FolderKind::Videos, name: None })
    }
}

#[test]
fn desktop_ini_is_read_only_for_listings_that_ask_for_the_folder_kind() {
    let fs = IniFs(AtomicUsize::new(0));
    let (slot, cancel, wake) = (SnapshotSlot::default(), GenToken::default(), Arc::new(|| {}) as Wake);
    cancel.set(4);
    list(&fs, 4, &p(r"C:\v"), ViewSpec::default(), CaseMode::Insensitive, Arc::default(), &slot, &cancel, &wake, None);
    assert_eq!((fs.0.load(Ordering::SeqCst), slot.kind(4)), (0, FolderKind::Unknown));
    slot.ask_kind(5);
    cancel.set(5);
    list(&fs, 5, &p(r"C:\v"), ViewSpec::default(), CaseMode::Insensitive, Arc::default(), &slot, &cancel, &wake, None);
    assert_eq!((fs.0.load(Ordering::SeqCst), slot.kind(5), slot.kind(4)), (1, FolderKind::Videos, FolderKind::Unknown));
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
    assert!(via_unc("Ubuntu", ProviderRequest::Op { tab: 1, id: 1, op: FileOp::Trash(vec![bad.clone()]), ctl: OpCtl::default() }).is_none());
    let renamed = via_unc("Ubuntu", ProviderRequest::Op { tab: 1, id: 2, op: FileOp::Rename { item: FsPath::parse("/home/a:b").unwrap(), name: b"c".to_vec() }, ctl: OpCtl::default() });
    assert!(matches!(renamed, Some(ProviderRequest::Op { op: FileOp::Rename { item, .. }, .. }) if item.display() == "\\\\wsl.localhost\\Ubuntu\\home\\a\u{F03A}b"));
    let out = Arc::new(SnapshotSlot::default());
    let req = ProviderRequest::List { tab: 1, gen: 3, path: bad, view: ViewSpec::default(), case: CaseMode::Sensitive, out: out.clone(), cancel: GenToken::default() };
    assert!(via_unc("Ubuntu", req).is_none());
    let snap = out.take().unwrap();
    assert!(snap.done && snap.error.is_some() && snap.gen == 3);
}

struct LinkShell;
impl Shell for LinkShell {
    fn new() -> Result<Self, PlatformError> {
        Ok(LinkShell)
    }
    fn icon(&mut self, _: &IconRequest) -> Option<Bitmap> {
        None
    }
    fn type_name(&mut self, _: &[u8]) -> Option<String> {
        None
    }
    fn open(&mut self, path: &FsPath) -> Result<(), PlatformError> {
        Err(PlatformError::new(1, format!("opened {}", path.display())))
    }
    fn launch(&mut self, launch: &Launch) -> Result<(), PlatformError> {
        Err(PlatformError::new(2, format!("{launch:?}")))
    }
    fn shortcut_folder(&mut self, link: &FsPath) -> Option<FsPath> {
        match link.as_bytes() {
            b"/home/u/Docs.desktop" => Some(p("/home/u/Documents")),
            b => b.ends_with(b"docs.LNK").then(|| p(r"D:\docs")),
        }
    }
    fn needs_exec_bit(&mut self, path: &FsPath) -> bool {
        path.as_bytes().ends_with(b"untrusted.desktop")
    }
    fn unlock(&mut self, device: &FsPath, passphrase: &str) -> Result<FsPath, PlatformError> {
        match passphrase {
            "right" => Ok(p("/media/u/vault")),
            "wrong" => Err(PlatformError::new(-1, format!("Cannot unlock {}: The passphrase is wrong", device.display())).wrong_passphrase()),
            _ => Err(PlatformError::new(16, format!("Cannot unlock {}: The drive is in use", device.display()))),
        }
    }
    fn open_ask(&mut self, path: &FsPath) -> Option<flit_core::provider::OpenAsk> {
        path.as_bytes().ends_with(b".sh").then_some(flit_core::provider::OpenAsk::Script)
    }
}
impl FileOps for LinkShell {
    fn file_op(&mut self, _: &FileOp, _: &Job) -> OpOutcome {
        OpOutcome::default()
    }
    fn clipboard_files(&mut self) -> Result<(Vec<FsPath>, bool), PlatformError> {
        Err(PlatformError::new(-1, "empty"))
    }
    fn clear_clipboard(&mut self) {}
}

#[test]
fn shortcuts_to_folders_navigate_and_launches_reach_the_shell() {
    let (tx, rx) = unbounded();
    let provider = LocalProvider::new::<_, NoWatch, LinkShell>(fs(), 1, tx, Arc::new(|| {})).unwrap();
    let next = || rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let error = |e: ProviderEvent| match e {
        ProviderEvent::OpDone { error: Some(e), .. } => e.message,
        e => panic!("{e:?}"),
    };
    provider.submit(ProviderRequest::Open { tab: 3, path: p(r"C:\r\docs.LNK") });
    assert_eq!(next(), ProviderEvent::Navigate { tab: 3, path: p(r"D:\docs") });
    provider.submit(ProviderRequest::Open { tab: 3, path: p(r"C:\r\app.lnk") });
    assert!(error(next()).starts_with(r"opened C:\r\app.lnk"));
    provider.submit(ProviderRequest::Open { tab: 3, path: p(r"C:\r\lnk") });
    assert!(error(next()).starts_with(r"opened C:\r\lnk"));
    provider.submit(ProviderRequest::Launch { tab: 3, launch: Launch::Terminal { dirs: vec![p(r"C:\r")], admin: false } });
    assert!(error(next()).starts_with("Terminal"));
}

#[test]
fn a_wrong_passphrase_comes_back_as_its_own_event_and_other_unlock_errors_as_failures() {
    let (tx, rx) = unbounded();
    let provider = LocalProvider::new::<_, NoWatch, LinkShell>(fs(), 1, tx, Arc::new(|| {})).unwrap();
    let next = || rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let unlock = |pass: &str| ProviderRequest::Launch { tab: 4, launch: Launch::Unlock { device: p("/dev/sdd2"), passphrase: flit_core::archive::Secret(pass.into()) } };
    provider.submit(unlock("wrong"));
    assert_eq!(next(), ProviderEvent::WrongPassphrase { tab: 4, device: p("/dev/sdd2") });
    provider.submit(unlock("busy"));
    assert!(matches!(next(), ProviderEvent::OpDone { tab: 4, id: 0, error: Some(e), report: None } if e.message.contains("in use")));
    provider.submit(unlock("right"));
    assert_eq!(next(), ProviderEvent::Navigate { tab: 4, path: p("/media/u/vault") });
}

struct SlowServerShell;
impl Shell for SlowServerShell {
    fn new() -> Result<Self, PlatformError> {
        Ok(SlowServerShell)
    }
    fn icon(&mut self, _: &IconRequest) -> Option<Bitmap> {
        None
    }
    fn type_name(&mut self, _: &[u8]) -> Option<String> {
        None
    }
    fn open(&mut self, path: &FsPath) -> Result<(), PlatformError> {
        Err(PlatformError::new(1, format!("opened {}", path.display())))
    }
    fn connect(&mut self, uri: &str) -> Result<FsPath, PlatformError> {
        std::thread::sleep(Duration::from_millis(800));
        match uri {
            "smb://nas/share" => Ok(p(r"C:\mnt\share")),
            _ => Err(PlatformError::new(3, format!("{uri} is not responding"))),
        }
    }
}
impl FileOps for SlowServerShell {
    fn file_op(&mut self, _: &FileOp, _: &Job) -> OpOutcome {
        OpOutcome::default()
    }
    fn clipboard_files(&mut self) -> Result<(Vec<FsPath>, bool), PlatformError> {
        Err(PlatformError::new(-1, "empty"))
    }
    fn clear_clipboard(&mut self) {}
}

#[test]
fn a_slow_server_connection_does_not_hold_up_opening_files() {
    let (tx, rx) = unbounded();
    let provider = LocalProvider::new::<_, NoWatch, SlowServerShell>(fs(), 1, tx, Arc::new(|| {})).unwrap();
    let next = || rx.recv_timeout(Duration::from_secs(5)).unwrap();
    provider.submit(ProviderRequest::Launch { tab: 7, launch: Launch::ConnectServer("smb://nas/share".into()) });
    provider.submit(ProviderRequest::Launch { tab: 7, launch: Launch::ConnectServer("sftp://gone/".into()) });
    provider.submit(ProviderRequest::Open { tab: 7, path: p(r"C:\r\a.txt") });
    assert!(matches!(next(), ProviderEvent::OpDone { error: Some(e), .. } if e.message.starts_with(r"opened C:\r\a.txt")));
    let mut rest = [next(), next()];
    rest.sort_by_key(|e| format!("{e:?}"));
    assert_eq!(rest[0], ProviderEvent::Navigate { tab: 7, path: p(r"C:\mnt\share") });
    assert!(matches!(&rest[1], ProviderEvent::OpDone { tab: 7, error: Some(e), .. } if e.message.contains("sftp://gone/ is not responding")));
}

struct LoginShell;
impl Shell for LoginShell {
    fn new() -> Result<Self, PlatformError> {
        Ok(LoginShell)
    }
    fn icon(&mut self, _: &IconRequest) -> Option<Bitmap> {
        None
    }
    fn type_name(&mut self, _: &[u8]) -> Option<String> {
        None
    }
    fn open(&mut self, path: &FsPath) -> Result<(), PlatformError> {
        Err(PlatformError::new(1, format!("opened {}", path.display())))
    }
    fn connect_asking(&mut self, uri: &str, prompt: &MountPrompt) -> Result<Option<FsPath>, PlatformError> {
        let ask = flit_core::provider::LoginAsk { server: uri.into(), message: "Password".into(), user: "ann".into(), domain: String::new(), flags: flit_core::provider::ASK_PASSWORD };
        let (_, rx) = prompt.ask(flit_core::provider::MountAsk::Login(ask));
        match rx.recv_timeout(Duration::from_secs(5)) {
            Ok(MountReply::Login { password, .. }) if password.as_str() == "s3cret" => Ok(Some(p(r"C:\mnt\share"))),
            Ok(MountReply::Cancel) => Ok(None),
            _ => Err(PlatformError::new(13, "login refused")),
        }
    }
}
impl FileOps for LoginShell {
    fn file_op(&mut self, _: &FileOp, _: &Job) -> OpOutcome {
        OpOutcome::default()
    }
    fn clipboard_files(&mut self) -> Result<(Vec<FsPath>, bool), PlatformError> {
        Err(PlatformError::new(-1, "empty"))
    }
    fn clear_clipboard(&mut self) {}
}

fn asked(rx: &crossbeam_channel::Receiver<ProviderEvent>) -> flit_core::provider::MountAnswer {
    match rx.recv_timeout(Duration::from_secs(5)).unwrap() {
        ProviderEvent::MountAsk { tab: 7, answer, .. } => answer,
        e => panic!("{e:?}"),
    }
}

#[test]
fn a_login_question_goes_to_the_tab_that_connected_and_its_answer_opens_the_share() {
    let (tx, rx) = unbounded();
    let provider = LocalProvider::new::<_, NoWatch, LoginShell>(fs(), 1, tx, Arc::new(|| {})).unwrap();
    provider.submit(ProviderRequest::Launch { tab: 7, launch: Launch::ConnectServer("smb://nas/share".into()) });
    let answer = asked(&rx);
    provider.submit(ProviderRequest::Open { tab: 7, path: p(r"C:\r\a.txt") });
    assert!(matches!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), ProviderEvent::OpDone { error: Some(e), .. } if e.message.starts_with("opened")), "an open login form does not block opening files");
    answer.send(MountReply::Login { user: "ann".into(), domain: String::new(), password: flit_core::provider::Password::new("s3cret".into()), anonymous: false, remember: false });
    assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), ProviderEvent::Navigate { tab: 7, path: p(r"C:\mnt\share") });
}

#[test]
fn cancelling_the_login_neither_navigates_nor_reports_an_error() {
    let (tx, rx) = unbounded();
    let provider = LocalProvider::new::<_, NoWatch, LoginShell>(fs(), 1, tx, Arc::new(|| {})).unwrap();
    provider.submit(ProviderRequest::Launch { tab: 7, launch: Launch::ConnectServer("smb://nas/share".into()) });
    asked(&rx).send(MountReply::Cancel);
    assert!(rx.recv_timeout(Duration::from_millis(500)).is_err(), "a cancelled login is silent");
    provider.submit(ProviderRequest::Launch { tab: 7, launch: Launch::ConnectServer("smb://nas/share".into()) });
    drop(asked(&rx));
    assert!(matches!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), ProviderEvent::OpDone { tab: 7, error: Some(e), .. } if e.message.contains("login refused")), "a form that went away without an answer is an error");
}

#[test]
fn a_shell_without_login_questions_connects_as_before() {
    let (tx, rx) = unbounded();
    let provider = LocalProvider::new::<_, NoWatch, SlowServerShell>(fs(), 1, tx, Arc::new(|| {})).unwrap();
    provider.submit(ProviderRequest::Launch { tab: 7, launch: Launch::ConnectServer("smb://nas/share".into()) });
    assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), ProviderEvent::Navigate { tab: 7, path: p(r"C:\mnt\share") });
}

#[test]
fn only_a_successful_requested_login_returns_an_address_to_save() {
    let (tx, rx) = unbounded();
    let provider = LocalProvider::new::<_, NoWatch, LoginShell>(fs(), 1, tx, Arc::new(|| {})).unwrap();
    let uri = "sftp://ann@192.168.1.8";
    let connect = || provider.submit(ProviderRequest::Launch { tab: 7, launch: Launch::ConnectAndSaveServer(uri.into()) });
    connect();
    asked(&rx).send(MountReply::Cancel);
    connect();
    drop(asked(&rx));
    assert!(matches!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), ProviderEvent::OpDone { error: Some(e), .. } if e.message.contains("login refused")));
    connect();
    asked(&rx).send(MountReply::Login { user: "ann".into(), domain: String::new(), password: flit_core::provider::Password::new("s3cret".into()), anonymous: false, remember: false });
    assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), ProviderEvent::SavedServerConnected { tab: 7, address: uri.into(), path: p(r"C:\mnt\share") });
}

#[test]
fn launchers_to_folders_navigate_and_scripts_ask_before_opening() {
    let (tx, rx) = unbounded();
    let provider = LocalProvider::new::<_, NoWatch, LinkShell>(fs(), 1, tx, Arc::new(|| {})).unwrap();
    let next = || rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let opened = |e: ProviderEvent| matches!(e, ProviderEvent::OpDone { error: Some(e), .. } if e.message.starts_with("opened "));
    provider.submit(ProviderRequest::Open { tab: 3, path: p("/home/u/Docs.desktop") });
    assert_eq!(next(), ProviderEvent::Navigate { tab: 3, path: p("/home/u/Documents") });
    provider.submit(ProviderRequest::Open { tab: 3, path: p("/home/u/untrusted.desktop") });
    assert_eq!(next(), ProviderEvent::NotExecutable { tab: 3, path: p("/home/u/untrusted.desktop") }, "a launcher that is not a folder link still passes the trust check");
    provider.submit(ProviderRequest::Open { tab: 3, path: p("/home/u/run.sh") });
    assert_eq!(next(), ProviderEvent::OpenAsk { tab: 3, path: p("/home/u/run.sh"), ask: flit_core::provider::OpenAsk::Script });
    provider.submit(ProviderRequest::Open { tab: 3, path: p("/home/u/app.desktop") });
    assert!(opened(next()), "a trusted launcher reaches the shell");
    provider.submit(ProviderRequest::Open { tab: 3, path: p("/home/u/notes.txt") });
    assert!(opened(next()), "ordinary files open without a question");
}

#[test]
fn wsl_launches_map_to_unc() {
    let launch = Launch::With { program: FsPath::parse("/bin/tool.exe").unwrap(), args: vec![FsPath::parse("/home/a").unwrap()] };
    let Some(ProviderRequest::Launch { launch: Launch::With { program, args }, .. }) = via_unc("Ubuntu", ProviderRequest::Launch { tab: 1, launch }) else { panic!() };
    assert_eq!(program.display(), r"\\wsl.localhost\Ubuntu\bin\tool.exe");
    assert_eq!(args[0].display(), r"\\wsl.localhost\Ubuntu\home\a");
}

#[test]
fn hash_requests_answer_on_their_own_thread() {
    let (tx, rx) = unbounded();
    let provider = LocalProvider::new::<_, NoWatch, NoShell>(fs(), 1, tx, Arc::new(|| {})).unwrap();
    provider.submit(ProviderRequest::Hash { tab: 4, id: 9, file: p(r"C:\r\a.txt"), kinds: vec![HashKind::Sha256], ctl: OpCtl::default() });
    match rx.recv_timeout(Duration::from_secs(5)).unwrap() {
        ProviderEvent::Hashed { tab: 4, id: 9, result: Err(e) } => assert!(!e.is_empty() && !e.contains("not supported"), "the default hash reads the file: {e}"),
        e => panic!("{e:?}"),
    }
    let Some(ProviderRequest::Hash { file, .. }) = via_unc("Ubuntu", ProviderRequest::Hash { tab: 1, id: 2, file: FsPath::parse("/home/a.iso").unwrap(), kinds: Vec::new(), ctl: OpCtl::default() }) else { panic!() };
    assert_eq!(file.display(), r"\\wsl.localhost\Ubuntu\home\a.iso");
}

struct MountFs;
impl Fs for MountFs {
    fn read_dir(&self, _: &FsPath, _: &mut ListingBuilder<'_>) -> Result<(), FsError> {
        Ok(())
    }
    fn seek_penalty(&self, _: &FsPath) -> Option<bool> {
        None
    }
    fn volume_key(&self, path: &FsPath) -> Vec<u8> {
        if path.as_bytes().starts_with(b"/media/usb") { b"usb".to_vec() } else { b"root".to_vec() }
    }
}

#[test]
fn transfer_queues_follow_the_volume_key() {
    let mv = |from: &str, to: &str| FileOp::Move { items: vec![p(from)], dest: p(to) };
    assert_eq!(transfer_volume(&MountFs, &FileOp::Copy { items: vec![p("/home/a")], dest: p("/media/usb/x") }), Some(b"usb".to_vec()));
    assert_eq!(transfer_volume(&MountFs, &mv("/home/a", "/tmp")), None, "a move inside one volume is a rename");
    assert_eq!(transfer_volume(&MountFs, &mv("/home/a", "/media/usb")), Some(b"usb".to_vec()));
    let mem = fs();
    assert_eq!(transfer_volume(&mem, &FileOp::Copy { items: vec![p(r"E:\a")], dest: p(r"C:\x") }), Some(br"c:\".to_vec()));
    assert_eq!(transfer_volume(&mem, &mv(r"C:\a", r"c:\b")), None);
}

#[test]
fn measure_counts_contents_rounds_to_clusters_and_keeps_archives_apart() {
    let mut m = fs();
    m.0.insert(p(r"C:\r\a.zip").as_bytes().to_vec(), vec![("in", true, 0), ("big", false, 1000)]);
    m.0.insert(p(r"C:\r\a.zip\in").as_bytes().to_vec(), vec![("small", false, 3)]);
    let mut reports = 0;
    let t = measure(&m, &[(p(r"C:\r\sub"), None), (p(r"C:\r\a.txt"), Some(7))], &|| true, &mut |_| reports += 1);
    assert_eq!((t.files, t.folders, t.size, t.on_disk, t.unpacked), (2, 1, 127, 128, 0));
    let t = measure(&m, &[(p(r"C:\r\a.zip"), Some(50)), (p(r"C:\r\a.zip"), None)], &|| true, &mut |_| {});
    assert_eq!((t.files, t.folders, t.size, t.on_disk, t.unpacked), (2, 1, 50, 52, 1003));
    let t = measure(&m, &[(p(r"C:\r\sub"), None)], &|| false, &mut |_| {});
    assert_eq!((t.files, t.folders), (0, 0));
    let t = measure(&m, &[(p(r"C:\r\a.txt"), None)], &|| true, &mut |_| {});
    assert_eq!((t.files, t.folders, t.size, t.on_disk), (0, 0, 7, 8), "a file of unknown size is looked up in its folder");
    let t = measure(&m, &[(p(r"C:\r\gone.txt"), None)], &|| true, &mut |_| {});
    assert_eq!((t.size, t.on_disk), (0, 0));
    let Some(ProviderRequest::Measure { items, .. }) = via_unc("Ubuntu", ProviderRequest::Measure { tab: 1, id: 2, items: vec![(FsPath::parse("/home").unwrap(), None)], ctl: OpCtl::default() }) else { panic!() };
    assert!(items[0].0.display().starts_with(r"\\wsl"));
}
struct HangFs(Mutex<Option<crossbeam_channel::Receiver<()>>>);
impl Fs for HangFs {
    fn read_dir(&self, dir: &FsPath, out: &mut ListingBuilder<'_>) -> Result<(), FsError> {
        if self.remote(dir) {
            let gate = self.0.lock().unwrap().clone();
            if let Some(gate) = gate {
                let _ = gate.recv();
            }
        }
        out.push(b"x", Meta::default());
        Ok(())
    }
    fn seek_penalty(&self, _: &FsPath) -> Option<bool> {
        None
    }
    fn remote(&self, path: &FsPath) -> bool {
        path.kind() == flit_core::path::PathKind::Unc
    }
}

fn request(tab: TabId, path: &str, slot: &Arc<SnapshotSlot>, cancel: &GenToken) -> ProviderRequest {
    ProviderRequest::List { tab, gen: 1, path: p(path), view: ViewSpec::default(), case: CaseMode::Insensitive, out: slot.clone(), cancel: cancel.clone() }
}

fn wait_for(slot: &SnapshotSlot) -> Arc<DirSnapshot> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(s) = slot.take().filter(|s| s.done) {
            return s;
        }
        assert!(Instant::now() < deadline, "no listing");
        std::thread::yield_now();
    }
}

#[test]
fn a_hanging_share_never_blocks_local_listings() {
    let (release, gate) = bounded::<()>(0);
    let (tx, _rx) = unbounded();
    let provider = LocalProvider::new::<_, NoWatch, NoShell>(HangFs(Mutex::new(Some(gate))), 1, tx, Arc::new(|| {})).unwrap();
    let (remote, local, cancel) = (Arc::new(SnapshotSlot::default()), Arc::new(SnapshotSlot::default()), GenToken::default());
    cancel.set(1);
    provider.submit(request(1, r"\\srv\share", &remote, &cancel));
    provider.submit(request(2, r"C:\here", &local, &cancel));
    assert_eq!(wait_for(&local).listing.len(), 1);
    assert!(remote.take().is_none());
    drop(release);
    assert_eq!(wait_for(&remote).listing.len(), 1);
}

#[test]
fn slow_network_discovery_publishes_to_the_ui_while_local_folders_stay_available() {
    struct SlowDiscovery(crossbeam_channel::Receiver<()>);

    impl Fs for SlowDiscovery {
        fn read_dir(&self, path: &FsPath, out: &mut ListingBuilder<'_>) -> Result<(), FsError> {
            if self.remote(path) {
                let dir = Meta { attrs: attr::DIR, ..Default::default() };
                out.push(b"smb://nas/", dir);
                if out.batch_done() && self.0.recv().is_ok() {
                    out.push(b"sftp://nas/", dir);
                }
            } else {
                out.push(b"local", Meta::default());
            }
            Ok(())
        }

        fn remote(&self, path: &FsPath) -> bool {
            path.is_network()
        }

        fn seek_penalty(&self, _: &FsPath) -> Option<bool> {
            None
        }
    }

    use flit_core::ui::{App, Effect, Persisted};
    let (release, gate) = bounded(0);
    let (events, _) = unbounded();
    let (wake, woke) = unbounded();
    let provider = LocalProvider::new::<_, NoWatch, NoShell>(SlowDiscovery(gate), 1, events, Arc::new(move || { let _ = wake.send(()); })).unwrap();
    let mut app = App::new(Persisted::default(), Vec::new(), 0, false, 1.0, (1000, 700), Instant::now());
    app.set_caps(flit_core::caps::Caps { unix_paths: true });
    app.drain_effects().for_each(drop);
    assert!(app.run_action("nav.network", Instant::now()));
    for effect in app.drain_effects() {
        if let Effect::Provider { req, .. } = effect {
            provider.submit(req);
        }
    }
    woke.recv_timeout(Duration::from_secs(5)).expect("the first network batch wakes the UI");
    app.poll();
    assert_eq!(app.tab().rows(), 1);
    let (local, cancel) = (Arc::new(SnapshotSlot::default()), GenToken::default());
    cancel.set(1);
    provider.submit(request(2, r"C:\here", &local, &cancel));
    assert_eq!(wait_for(&local).listing.name(0), b"local");
    release.send_timeout((), Duration::from_secs(5)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while app.tab().rows() < 2 {
        woke.recv_deadline(deadline).expect("the finished network list wakes the UI");
        app.poll();
    }
    assert_eq!(app.tab().rows(), 2);
}

#[test]
fn a_silent_share_reports_not_responding_until_it_answers() {
    let (release, gate) = bounded::<()>(0);
    let fs: Arc<dyn Fs> = Arc::new(HangFs(Mutex::new(Some(gate))));
    let (slot, cancel) = (Arc::new(SnapshotSlot::default()), GenToken::default());
    cancel.set(1);
    let (watch, _) = unbounded();
    let job = ListJob { tab: 1, gen: 1, path: p(r"\\srv\share"), view: ViewSpec::default(), case: CaseMode::Insensitive, out: slot.clone(), cancel: cancel.clone(), watch: None, tags: Arc::default(), picker_mime: None };
    list_remote(fs, job, watch, Arc::new(|| {}), Duration::from_millis(50)).unwrap();
    let late = wait_for(&slot);
    assert_eq!(late.error, Some(FsError::not_responding()));
    drop(release);
    let answer = wait_for(&slot);
    assert_eq!((answer.error.clone(), answer.listing.len()), (None, 1));
}

#[test]
fn a_deep_listing_walks_every_subfolder_and_marks_nested_items_with_their_folder() {
    let (tx, rx) = unbounded();
    let provider = LocalProvider::new::<_, NoWatch, NoShell>(fs(), 2, tx, Arc::new(|| {})).unwrap();
    let slot = Arc::new(SnapshotSlot::default());
    let cancel = GenToken::default();
    cancel.set(5);
    let view = ViewSpec { deep: true, ..ViewSpec::default() };
    provider.submit(ProviderRequest::List { tab: 1, gen: 5, path: p(r"C:\r"), view, case: CaseMode::Insensitive, out: slot.clone(), cancel: cancel.clone() });
    let snap = loop {
        if let Some(s) = slot.take().filter(|s| s.done) {
            break s;
        }
        std::thread::yield_now();
    };
    let l = &snap.listing;
    let mut found: Vec<(String, String)> = (0..l.len() as u32).map(|i| (String::from_utf8_lossy(l.folder(i)).into_owned(), String::from_utf8_lossy(l.name(i)).into_owned())).collect();
    found.sort();
    let want = [("", "a.txt"), ("", "b.txt"), ("", "c"), ("", "d"), ("", "sub"), ("sub", "deep"), ("sub", "x"), (r"sub\deep", "y")];
    assert_eq!(found, want.map(|(f, n)| (f.to_owned(), n.to_owned())));
    assert_eq!((l.nested, snap.order.len()), (3, 8), "nested items are listed and sorted with the rest");
    let shallow = ViewSpec::default();
    assert_eq!((0..l.len() as u32).filter(|&i| flit_core::sort::visible(l, i, &shallow)).count(), 5, "without subfolders they hide");
    drop(rx);
}

#[test]
fn paste_shortcut_links_the_clipboard_items_into_the_folder() {
    let files = vec![p(r"C:\a\x.txt"), p(r"C:\a\y")];
    let mut clip = Clip { files: files.clone(), cut: true, ..Default::default() };
    let (error, _) = run_op(&mut clip, FileOp::PasteLink { dest: p(r"C:\b") });
    assert!(error.is_none());
    assert_eq!((clip.ran, clip.cleared), (vec![FileOp::Link { items: files, dest: p(r"C:\b") }], false), "a cut clipboard stays as it is");
    let (error, report) = run_op(&mut Clip::default(), FileOp::PasteLink { dest: p(r"C:\b") });
    assert!(error.is_some() && report.is_none());
}

#[test]
fn typed_paste_links_use_the_same_queue_and_do_not_clear_a_cut_clipboard() {
    for kind in [flit_core::provider::LinkKind::Symbolic, flit_core::provider::LinkKind::Junction, flit_core::provider::LinkKind::Hard] {
        let files = vec![p(r"C:\a\x.txt")];
        let mut clip = Clip { files: files.clone(), cut: true, ..Default::default() };
        let (error, report) = run_op(&mut clip, FileOp::PasteLinkAs { dest: p(r"C:\b"), kind });
        let expected = FileOp::LinkAs { items: files, dest: p(r"C:\b"), kind };
        assert!(error.is_none());
        assert_eq!(clip.ran.as_slice(), std::slice::from_ref(&expected));
        assert_eq!(report.unwrap().op, expected);
        assert!(!clip.cleared);
        let (error, report) = run_op(&mut Clip::default(), FileOp::PasteLinkAs { dest: p(r"C:\b"), kind });
        assert!(error.is_some() && report.is_none());
    }
}

#[test]
fn open_containing_path_reveals_the_real_location() {
    let dir = std::env::temp_dir().join(format!("flit-platform-reveal-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("inner")).unwrap();
    std::fs::write(dir.join("inner").join("f.txt"), b"x").unwrap();
    let roundabout = dir.join("inner").join("..").join("inner").join("f.txt");
    let item = FsPath::parse(roundabout.to_str().unwrap()).unwrap();
    let expected = FsPath::parse(dir.canonicalize().unwrap().join("inner").join("f.txt").to_str().unwrap().trim_start_matches(r"\?\")).unwrap();
    assert_eq!(real_path(&mut LinkShell, &item), Some(expected.clone()));
    let (tx, rx) = unbounded();
    let provider = LocalProvider::new::<_, NoWatch, LinkShell>(fs(), 1, tx, Arc::new(|| {})).unwrap();
    provider.submit(ProviderRequest::Launch { tab: 5, launch: Launch::Reveal { item, new_tab: true } });
    assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), ProviderEvent::Reveal { tab: 5, path: expected, new_tab: true });
    let gone = FsPath::parse(dir.join("missing.txt").to_str().unwrap()).unwrap();
    provider.submit(ProviderRequest::Launch { tab: 5, launch: Launch::Reveal { item: gone, new_tab: false } });
    assert!(matches!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), ProviderEvent::OpDone { error: Some(_), .. }));
    std::fs::remove_dir_all(&dir).unwrap();
}

struct StuckBin;
impl FileOps for StuckBin {
    fn file_op(&mut self, op: &FileOp, _: &Job) -> OpOutcome {
        let FileOp::Trash(items) = op else { return OpOutcome::default() };
        let error = PlatformError::new(-1, "Cannot move to the Trash").untrashable(items[1..].to_vec());
        OpOutcome { items: vec![(items[0].clone(), p(r"C:\bin\a"))], error: Some(error) }
    }
    fn clipboard_files(&mut self) -> Result<(Vec<FsPath>, bool), PlatformError> {
        Err(PlatformError::new(-1, "empty"))
    }
    fn clear_clipboard(&mut self) {}
}

#[test]
fn untrashable_items_reach_the_core_just_before_the_operation_ends() {
    let trash = FileOp::Trash(vec![p(r"C:\r\a"), p(r"C:\r\b"), p(r"C:\r\c")]);
    let finished = perform(&mut StuckBin, trash, false, &Job::detached(flit_core::provider::Resolve::Skip));
    assert_eq!(finished.2, vec![p(r"C:\r\b"), p(r"C:\r\c")]);
    let (tx, rx) = unbounded();
    Reply { tab: 7, id: 2, events: tx, wake: Arc::new(|| {}) }.finish(finished);
    assert_eq!(rx.try_recv().unwrap(), ProviderEvent::Untrashable { tab: 7, id: 2, items: vec![p(r"C:\r\b"), p(r"C:\r\c")] });
    assert!(matches!(rx.try_recv().unwrap(), ProviderEvent::OpDone { tab: 7, id: 2, error: Some(_), report: Some(r) } if r.items.len() == 1));
    assert!(perform(&mut StuckBin, FileOp::Delete(vec![p(r"C:\r\a")]), false, &Job::detached(flit_core::provider::Resolve::Skip)).2.is_empty());
}

struct LabelFs;
impl Fs for LabelFs {
    fn read_dir(&self, _: &FsPath, out: &mut ListingBuilder<'_>) -> Result<(), FsError> {
        for (n, attrs) in [("Downloads", attr::DIR | attr::READONLY), ("Documents", attr::DIR | attr::READONLY), ("Archive", attr::DIR)] {
            out.push(n.as_bytes(), flit_core::Meta { attrs, ..Default::default() });
        }
        out.batch_done();
        Ok(())
    }
    fn seek_penalty(&self, _: &FsPath) -> Option<bool> {
        None
    }
    fn localized_name(&self, dir: &FsPath) -> Option<String> {
        match dir.file_name() {
            b"Downloads" => Some("Загрузки".into()),
            b"Documents" => Some("Документы".into()),
            b"flit-label-root" => Some("Корень".into()),
            _ => None,
        }
    }
}

#[test]
fn localized_folder_names_label_sort_and_filter_while_real_names_stay() {
    let (slot, cancel, wake) = (SnapshotSlot::default(), GenToken::default(), Arc::new(|| {}) as Wake);
    cancel.set(1);
    let dir = p(r"C:\flit-label-root");
    list(&LabelFs, 1, &dir, ViewSpec::default(), CaseMode::Insensitive, Arc::default(), &slot, &cancel, &wake, None);
    let snap = slot.take().unwrap();
    let l = &snap.listing;
    let shown: Vec<&str> = snap.order.iter().map(|&i| std::str::from_utf8(l.label(i)).unwrap()).collect();
    assert_eq!(shown, ["Archive", "Документы", "Загрузки"], "sorted by the shown name");
    let real: Vec<&[u8]> = snap.order.iter().map(|&i| l.name(i)).collect();
    assert_eq!(real, [&b"Archive"[..], b"Documents", b"Downloads"]);
    let mut f = flit_core::filter::NameFilter::new("загр").unwrap();
    f.sync(l, &snap.order);
    assert_eq!((0..l.len() as u32).filter(|&i| f.matched(i)).map(|i| l.name(i)).collect::<Vec<_>>(), [&b"Downloads"[..]]);
    assert_eq!(desktop_ini::label(&dir).as_deref(), Some("Корень"));
    assert_eq!(desktop_ini::label(&dir.join(b"Documents")).as_deref(), Some("Документы"));
    assert_eq!(Location::Local(dir.join(b"Documents")).crumbs().last().unwrap().0, "Документы");
}

static LABELS_WANTED: Mutex<Vec<bool>> = Mutex::new(Vec::new());

struct DisplayLabelFs;
impl Fs for DisplayLabelFs {
    fn read_dir(&self, _: &FsPath, out: &mut ListingBuilder<'_>) -> Result<(), FsError> {
        LABELS_WANTED.lock().unwrap().push(out.wants_labels());
        Ok(())
    }
    fn seek_penalty(&self, _: &FsPath) -> Option<bool> {
        None
    }
}

#[test]
fn folder_listings_ask_the_file_system_for_display_labels() {
    let slot = SnapshotSlot::default();
    let cancel = GenToken::default();
    cancel.set(1);
    list(&DisplayLabelFs, 1, &p(r"C:\labels"), ViewSpec::default(), CaseMode::Insensitive, Arc::default(), &slot, &cancel, &(Arc::new(|| {}) as Wake), None);
    assert_eq!(*LABELS_WANTED.lock().unwrap(), [true]);
}

#[test]
fn file_picker_listing_classifies_files_only_when_requested() {
    let wake: Wake = Arc::new(|| {});
    let case = CaseMode::Insensitive;
    let view = ViewSpec::default();
    let slot = SnapshotSlot::default();
    let cancel = GenToken::default();
    let dir = p(r"C:\r");
    cancel.set(1);
    let classify = |path: &FsPath| {
        assert_ne!(path.file_name(), b"sub", "the picker must not probe directories");
        (path.file_name() == b"b.txt").then(|| vec!["text/plain".into()].into())
    };
    list(&fs(), 1, &dir, view, case, Arc::default(), &slot, &cancel, &wake, Some(classify));
    let listed = slot.take().unwrap();
    assert!(listed.done);
    assert_eq!(listed.listing.name(0), b"b.txt");
    assert_eq!(listed.listing.picker_mime[&0].as_ref(), ["text/plain"]);
    assert_eq!(listed.listing.picker_mime.len(), 1);
    list(&fs(), 1, &dir, view, case, Arc::default(), &slot, &cancel, &wake, None);
    assert!(slot.take().unwrap().listing.picker_mime.is_empty());
}
