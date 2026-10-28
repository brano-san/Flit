use super::*;
use crate::OpOutcome;
use flit_core::listing::Meta;
use flit_core::provider::OpProgress;
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
    provider.submit(ProviderRequest::DirSizes { tab: 1, gen: 3, dir: p(r"C:\r"), items: vec![(1, b"sub".to_vec())], cancel: cancel.clone() });
    let ProviderEvent::DirSize { tab: 1, gen: 3, idx: 1, stats } = rx.recv_timeout(Duration::from_secs(5)).unwrap() else { panic!("folder size expected") };
    assert_eq!((stats.size, stats.partial), (120, false));
    provider.submit(ProviderRequest::Open { tab: 4, path: p(r"C:\r\a.txt") });
    assert!(matches!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), ProviderEvent::OpDone { tab: 4, id: 0, error: Some(_), report: None }));
    let trash = FileOp::Trash(vec![p(r"C:\r\c")]);
    provider.submit(ProviderRequest::Op { tab: 9, id: 3, op: trash.clone(), ctl: OpCtl::default() });
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
    assert!(matches!(next(), ProviderEvent::OpDone { id: 2, error: Some(e), .. } if e == CANCELLED));
    RELEASE_C.store(true, Ordering::Release);
    assert!(matches!(next(), ProviderEvent::OpDone { id: 1, error: None, .. }));
    provider.wait_idle(Duration::from_secs(5));
}

#[derive(Default)]
struct Clip {
    files: Vec<FsPath>,
    cut: bool,
    ran: Vec<FileOp>,
    cleared: bool,
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
    list(&fs(), 1, &p(r"C:\r"), ViewSpec::default(), CaseMode::Insensitive, Arc::default(), &slot, &cancel, &(Arc::new(|| {}) as Wake));
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
        Some(flit_core::desktop_ini::DesktopIni { icon: false, kind: FolderKind::Videos })
    }
}

#[test]
fn desktop_ini_is_read_only_for_listings_that_ask_for_the_folder_kind() {
    let fs = IniFs(AtomicUsize::new(0));
    let (slot, cancel, wake) = (SnapshotSlot::default(), GenToken::default(), Arc::new(|| {}) as Wake);
    cancel.set(4);
    list(&fs, 4, &p(r"C:\v"), ViewSpec::default(), CaseMode::Insensitive, Arc::default(), &slot, &cancel, &wake);
    assert_eq!((fs.0.load(Ordering::SeqCst), slot.kind(4)), (0, FolderKind::Unknown));
    slot.ask_kind(5);
    cancel.set(5);
    list(&fs, 5, &p(r"C:\v"), ViewSpec::default(), CaseMode::Insensitive, Arc::default(), &slot, &cancel, &wake);
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
        link.as_bytes().ends_with(b"docs.LNK").then(|| p(r"D:\docs"))
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
        ProviderEvent::OpDone { error: Some(e), .. } => e,
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
fn a_silent_share_reports_not_responding_until_it_answers() {
    let (release, gate) = bounded::<()>(0);
    let fs: Arc<dyn Fs> = Arc::new(HangFs(Mutex::new(Some(gate))));
    let (slot, cancel) = (Arc::new(SnapshotSlot::default()), GenToken::default());
    cancel.set(1);
    let (watch, _) = unbounded();
    let job = ListJob { tab: 1, gen: 1, path: p(r"\\srv\share"), view: ViewSpec::default(), case: CaseMode::Insensitive, out: slot.clone(), cancel: cancel.clone(), watch: None, tags: Arc::default() };
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
