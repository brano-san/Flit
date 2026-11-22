use super::*;
use flit_core::listing::{ChangeKind, DirChange, DirStats};
use flit_core::provider::{Conflict, Failure, LinkKind, OpCtl, OpProgress, Recover, Resolve, Template};
use flit_core::sort::CaseMode;
use flit_core::Meta;
use flit_proto::listing::EntryBatch;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

type Handler = dyn Fn(&Request) -> Option<Vec<Event>> + Send + Sync;

#[derive(Clone)]
struct Fake {
    build: String,
    installed: Arc<Mutex<Option<String>>>,
    deliver_as: Option<String>,
    hang: bool,
    launches: Arc<AtomicUsize>,
    deliveries: Arc<AtomicUsize>,
    stops: Arc<AtomicUsize>,
    seen: Arc<Mutex<Vec<Request>>>,
    handler: Arc<Handler>,
}

fn agent(build: String, mut input: std::io::PipeReader, mut output: std::io::PipeWriter, seen: Arc<Mutex<Vec<Request>>>, handler: Arc<Handler>) {
    let ours = hello(&build);
    if write_frame(&mut output, &ours).is_err() || read_hello(&mut input, &hello("")).is_err() {
        return;
    }
    let mut buf = Vec::new();
    while let Ok(req) = read_frame::<_, Request>(&mut input, &mut buf) {
        seen.lock().unwrap().push(req.clone());
        let Some(events) = handler(&req) else { return };
        for ev in events {
            if write_frame(&mut output, &ev).is_err() {
                return;
            }
        }
        if req == Request::Shutdown {
            return;
        }
    }
}

impl Fake {
    fn new(handler: impl Fn(&Request) -> Option<Vec<Event>> + Send + Sync + 'static) -> Self {
        Self {
            build: "test-1".into(),
            installed: Arc::new(Mutex::new(Some("test-1".into()))),
            deliver_as: None,
            hang: false,
            launches: Arc::default(),
            deliveries: Arc::default(),
            stops: Arc::default(),
            seen: Arc::default(),
            handler: Arc::new(handler),
        }
    }

    fn seen(&self) -> Vec<Request> {
        self.seen.lock().unwrap().clone()
    }
}

impl AgentLauncher for Fake {
    fn build(&self) -> &str {
        &self.build
    }

    fn launch(&self) -> io::Result<AgentProcess> {
        self.launches.fetch_add(1, Ordering::SeqCst);
        let (req_r, req_w) = std::io::pipe()?;
        let (ev_r, ev_w) = std::io::pipe()?;
        let installed = self.installed.lock().unwrap().clone();
        let (seen, handler, hang) = (self.seen.clone(), self.handler.clone(), self.hang);
        std::thread::spawn(move || match installed {
            _ if hang => {
                std::thread::sleep(Duration::from_secs(3));
                drop((req_r, ev_w));
            }
            Some(build) => agent(build, req_r, ev_w, seen, handler),
            None => drop((req_r, ev_w)),
        });
        let stops = self.stops.clone();
        Ok(AgentProcess { input: Box::new(req_w), output: Box::new(ev_r), stop: Box::new(move |_| {
                stops.fetch_add(1, Ordering::SeqCst);
            }) })
    }

    fn deliver(&self) -> io::Result<()> {
        self.deliveries.fetch_add(1, Ordering::SeqCst);
        *self.installed.lock().unwrap() = Some(self.deliver_as.clone().unwrap_or_else(|| self.build.clone()));
        Ok(())
    }
}

#[derive(Default)]
struct Recorder(Mutex<Vec<ProviderRequest>>);

impl Provider for Recorder {
    fn submit(&self, req: ProviderRequest) {
        self.0.lock().unwrap().push(req);
    }
}

struct Setup {
    wsl: WslProvider,
    fake: Fake,
    local: Arc<Recorder>,
    events: Receiver<ProviderEvent>,
}

fn setup(fake: Fake, timing: Timing) -> Setup {
    setup_with(fake, timing, Clipboard::default())
}

fn setup_with(fake: Fake, timing: Timing, clipboard: Clipboard) -> Setup {
    let (tx, events) = unbounded();
    let local = Arc::new(Recorder::default());
    let wsl = WslProvider::new("Ubuntu".into(), Some(Box::new(fake.clone())), local.clone(), tx, Arc::new(|| {}), timing, clipboard).unwrap();
    Setup { wsl, fake, local, events }
}

fn until(what: &str, f: impl Fn() -> bool) {
    let end = Instant::now() + Duration::from_secs(5);
    while !f() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn event<T>(rx: &Receiver<ProviderEvent>, mut f: impl FnMut(ProviderEvent) -> Option<T>) -> T {
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(t) = f(rx.recv_timeout(end.saturating_duration_since(Instant::now())).expect("provider event")) {
            return t;
        }
    }
}

fn unix(p: &str) -> FsPath {
    FsPath::parse(p).unwrap()
}

fn batch(names: &[&[u8]]) -> EntryBatch {
    let mut b = EntryBatch::default();
    names.iter().for_each(|n| b.push(n, Meta { size: n.len() as u64, ..Default::default() }));
    b
}

fn lister(req: &Request) -> Option<Vec<Event>> {
    Some(match req {
        Request::List { id, .. } => vec![Event::Entries { id: *id, batch: batch(&[b"a:b", b"bad\xFF"]) }, Event::ListDone { id: *id, error: None }],
        _ => Vec::new(),
    })
}

struct Listed {
    slot: Arc<SnapshotSlot>,
    token: GenToken,
}

fn list(wsl: &WslProvider, tab: TabId, gen: u64, path: &str) -> Listed {
    let (slot, token) = (Arc::new(SnapshotSlot::default()), GenToken::default());
    token.set(gen);
    wsl.submit(ProviderRequest::Watch { tab, gen, path: Some(unix(path)) });
    wsl.submit(ProviderRequest::List { tab, gen, path: unix(path), view: ViewSpec::default(), case: CaseMode::Sensitive, out: slot.clone(), cancel: token.clone() });
    Listed { slot, token }
}

fn finished(l: &Listed) -> Arc<flit_core::listing::DirSnapshot> {
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(s) = l.slot.take().filter(|s| s.done) {
            return s;
        }
        assert!(Instant::now() < end, "no final snapshot");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn names(s: &flit_core::listing::DirSnapshot) -> Vec<Vec<u8>> {
    let mut v: Vec<Vec<u8>> = (0..s.listing.len() as u32).map(|i| s.listing.name(i).to_vec()).collect();
    v.sort();
    v
}

#[test]
fn lists_exact_names_through_the_agent() {
    let s = setup(Fake::new(lister), Timing::default());
    let l = list(&s.wsl, 1, 1, "/home/u");
    let snap = finished(&l);
    assert_eq!(names(&snap), [b"a:b".to_vec(), b"bad\xFF".to_vec()]);
    assert_eq!(snap.path, unix("/home/u"));
    assert_eq!(snap.error, None);
    assert!(matches!(s.events.try_recv(), Ok(ProviderEvent::Status { tab: 1, gen: 1, text }) if text == "Starting Ubuntu…"));
    assert_eq!((s.fake.launches.load(Ordering::SeqCst), s.fake.deliveries.load(Ordering::SeqCst)), (1, 0));
    assert!(matches!(&s.fake.seen()[..], [Request::Watch { id: 1, .. }, Request::List { id: 2, .. }]));
    assert!(s.local.0.lock().unwrap().is_empty());
}

#[test]
fn linux_errors_from_the_agent_show_errno_names_instead_of_windows_codes() {
    let failing = |req: &Request| {
        Some(match req {
            Request::List { id, .. } => vec![Event::ListDone { id: *id, error: Some(FsError { kind: FsErrorKind::Other, code: 5 }) }],
            _ => Vec::new(),
        })
    };
    let s = setup(Fake::new(failing), Timing::default());
    let error = finished(&list(&s.wsl, 1, 1, "/mnt/broken")).error.clone().unwrap();
    assert_eq!(error.linux_errno(), Some(5));
    assert_eq!(error.to_string(), "Error (EIO)");
    let denied = |req: &Request| {
        Some(match req {
            Request::List { id, .. } => vec![Event::ListDone { id: *id, error: Some(FsError { kind: FsErrorKind::AccessDenied, code: 13 }) }],
            _ => Vec::new(),
        })
    };
    let s = setup(Fake::new(denied), Timing::default());
    assert_eq!(finished(&list(&s.wsl, 1, 1, "/root")).error.as_ref().unwrap().to_string(), "Access denied");
}

#[test]
fn folders_inside_archives_are_read_through_the_share_without_the_agent() {
    let s = setup(Fake::new(lister), Timing::default());
    list(&s.wsl, 1, 1, "/home/u/a.zip/docs");
    until("share", || s.local.0.lock().unwrap().len() == 2);
    let local = s.local.0.lock().unwrap();
    assert!(matches!(&local[1], ProviderRequest::List { path, .. } if path.as_bytes() == br"\\wsl.localhost\Ubuntu\home\u\a.zip\docs"));
    assert_eq!(s.fake.launches.load(Ordering::SeqCst), 0);
}

#[test]
fn a_missing_agent_is_delivered_once() {
    let fake = Fake::new(lister);
    *fake.installed.lock().unwrap() = None;
    let s = setup(fake, Timing::default());
    assert_eq!(names(&finished(&list(&s.wsl, 1, 1, "/"))).len(), 2);
    assert_eq!((s.fake.launches.load(Ordering::SeqCst), s.fake.deliveries.load(Ordering::SeqCst)), (2, 1));
}

#[test]
fn a_foreign_build_after_delivery_falls_back_to_the_share() {
    let mut fake = Fake::new(lister);
    *fake.installed.lock().unwrap() = Some("old".into());
    fake.deliver_as = Some("still-old".into());
    let s = setup(fake, Timing::default());
    list(&s.wsl, 1, 1, "/etc");
    until("fallback", || s.local.0.lock().unwrap().len() == 2);
    let local = s.local.0.lock().unwrap();
    assert!(matches!(&local[1], ProviderRequest::List { path, .. } if path.as_bytes() == br"\\wsl.localhost\Ubuntu\etc"));
    assert_eq!(s.fake.deliveries.load(Ordering::SeqCst), 1);
    assert_eq!(s.fake.stops.load(Ordering::SeqCst), 2);
}

#[test]
fn a_silent_agent_times_out_into_the_share() {
    let mut fake = Fake::new(lister);
    fake.hang = true;
    let s = setup(fake, Timing { hello: Duration::from_millis(100), ..Timing::default() });
    list(&s.wsl, 1, 1, "/");
    until("fallback", || s.local.0.lock().unwrap().len() == 2);
    assert_eq!(s.fake.stops.load(Ordering::SeqCst), 1);
    assert_eq!(s.fake.deliveries.load(Ordering::SeqCst), 0);
}

#[test]
fn watches_map_to_tabs_and_are_cancelled() {
    let fake = Fake::new(|req| {
        Some(match req {
            Request::Watch { id, .. } => vec![Event::Changes { id: *id, changes: vec![DirChange { label: flit_core::listing::Label::Keep, kind: ChangeKind::Added, name: b"x".to_vec(), meta: Some(Meta::default()) }] }],
            other => lister(other)?,
        })
    });
    let s = setup(fake, Timing::default());
    finished(&list(&s.wsl, 7, 3, "/srv"));
    let changes = event(&s.events, |e| if let ProviderEvent::Changes { tab, gen, changes } = e { Some((tab, gen, changes)) } else { None });
    assert_eq!((changes.0, changes.1, changes.2[0].name.as_slice()), (7, 3, &b"x"[..]));
    s.wsl.submit(ProviderRequest::Watch { tab: 7, gen: 3, path: None });
    until("cancel", || s.fake.seen().contains(&Request::Cancel { id: 1 }));
}

#[test]
fn stale_listings_are_cancelled_on_the_agent() {
    let fake = Fake::new(|req| {
        if matches!(req, Request::List { .. }) {
            std::thread::sleep(Duration::from_millis(150));
        }
        lister(req)
    });
    let s = setup(fake, Timing::default());
    let l = list(&s.wsl, 1, 1, "/");
    until("listing", || s.fake.seen().iter().any(|r| matches!(r, Request::List { .. })));
    l.token.set(2);
    until("cancel", || s.fake.seen().contains(&Request::Cancel { id: 2 }));
    assert!(l.slot.take().is_none());
}

#[test]
fn hostile_names_drop_the_connection() {
    let fake = Fake::new(|req| {
        Some(match req {
            Request::List { id, .. } => vec![Event::Entries { id: *id, batch: batch(&[b"ok", b"../../etc"]) }],
            _ => Vec::new(),
        })
    });
    let s = setup(fake, Timing::default());
    let snap = finished(&list(&s.wsl, 4, 9, "/"));
    assert_eq!(snap.error.as_ref().unwrap().kind, FsErrorKind::Unavailable);
    assert!(snap.listing.is_empty());
    let note = event(&s.events, |e| if let ProviderEvent::OpDone { tab: 4, id: 0, error, .. } = e { error } else { None });
    assert!(note.message.contains("Press F5"));
}

#[test]
fn crashes_never_restart_by_themselves_and_three_fall_back() {
    let fake = Fake::new(|req| if matches!(req, Request::List { .. }) { None } else { Some(Vec::new()) });
    let s = setup(fake, Timing::default());
    for gen in 1..=3 {
        let snap = finished(&list(&s.wsl, 1, gen, "/"));
        assert_eq!(snap.error.as_ref().unwrap().kind, FsErrorKind::Unavailable);
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(s.fake.launches.load(Ordering::SeqCst), gen as usize);
    }
    list(&s.wsl, 1, 4, "/");
    until("fallback", || s.local.0.lock().unwrap().len() == 2);
    assert_eq!(s.fake.launches.load(Ordering::SeqCst), 3);
}

#[test]
fn an_idle_agent_stops_and_starts_again_on_demand() {
    let s = setup(Fake::new(lister), Timing { idle: Duration::from_millis(100), ..Timing::default() });
    let (slot, token) = (Arc::new(SnapshotSlot::default()), GenToken::default());
    token.set(1);
    s.wsl.submit(ProviderRequest::List { tab: 1, gen: 1, path: unix("/"), view: ViewSpec::default(), case: CaseMode::Sensitive, out: slot.clone(), cancel: token.clone() });
    finished(&Listed { slot, token });
    until("shutdown", || s.fake.seen().contains(&Request::Shutdown));
    until("stop", || s.fake.stops.load(Ordering::SeqCst) == 1);
    finished(&list(&s.wsl, 1, 2, "/"));
    assert_eq!(s.fake.launches.load(Ordering::SeqCst), 2);
    std::thread::sleep(Duration::from_millis(250));
    assert_eq!(s.fake.stops.load(Ordering::SeqCst), 1, "a watched tab keeps the agent");
}

#[test]
fn dropping_the_provider_stops_its_agent() {
    let s = setup(Fake::new(lister), Timing::default());
    finished(&list(&s.wsl, 1, 1, "/"));
    let stops = s.fake.stops.clone();
    drop(s);
    until("stop", || stops.load(Ordering::SeqCst) == 1);
}

#[test]
fn trash_goes_to_the_agent_and_the_rest_to_the_share() {
    let fake = Fake::new(|req| {
        Some(match req {
            Request::Op { id, .. } => vec![Event::OpDone { id: *id, error: Some("nope".into()), items: Vec::new() }],
            Request::DirSizes { id, items, .. } => items.iter().map(|(idx, _)| Event::DirSize { id: *id, idx: *idx, stats: DirStats { size: 5, ..Default::default() } }).chain([Event::DirSize { id: *id, idx: 99, stats: DirStats { size: 1, ..Default::default() } }]).collect(),
            _ => Vec::new(),
        })
    });
    let s = setup(fake, Timing::default());
    s.wsl.submit(ProviderRequest::Op { tab: 2, id: 11, op: FileOp::Trash(vec![unix("/tmp/a")]), ctl: OpCtl::default() });
    let done = event(&s.events, |e| if let ProviderEvent::OpDone { tab: 2, id: 11, error, report } = e { Some((error, report)) } else { None });
    assert_eq!(done, (Some("nope".into()), Some(OpReport::new(FileOp::Trash(vec![unix("/tmp/a")]), Vec::new()))));
    let token = GenToken::default();
    token.set(1);
    s.wsl.submit(ProviderRequest::DirSizes { tab: 2, gen: 1, dir: unix("/"), items: vec![(3, b"usr".to_vec())], cancel: token, allowed: [true; 3] });
    let size = event(&s.events, |e| if let ProviderEvent::DirSize { idx, stats, .. } = e { Some((idx, stats.size)) } else { None });
    assert_eq!(size, (3, 5));
    s.wsl.submit(ProviderRequest::Op { tab: 2, id: 12, op: FileOp::NewFile { dir: unix("/tmp"), name: b"n".to_vec(), template: Template::Empty }, ctl: OpCtl::default() });
    s.wsl.submit(ProviderRequest::Open { tab: 2, path: unix("/tmp/x:y") });
    until("share", || s.local.0.lock().unwrap().len() == 2);
    let local = s.local.0.lock().unwrap();
    assert!(matches!(&local[0], ProviderRequest::Op { id: 12, op: FileOp::NewFile { dir, .. }, .. } if dir.as_bytes() == br"\\wsl.localhost\Ubuntu\tmp"));
    assert!(matches!(&local[1], ProviderRequest::Open { path, .. } if path.as_bytes() == "\\\\wsl.localhost\\Ubuntu\\tmp\\x\u{F03A}y".as_bytes()));
    assert!(s.events.try_iter().all(|e| !matches!(e, ProviderEvent::DirSize { idx: 99, .. })));
}

#[test]
fn without_an_agent_everything_uses_the_share() {
    let (tx, events) = unbounded();
    let local = Arc::new(Recorder::default());
    let wsl = WslProvider::new("Ubuntu".into(), None, local.clone(), tx, Arc::new(|| {}), Timing::default(), Clipboard::default()).unwrap();
    let l = list(&wsl, 1, 1, "/home");
    until("share", || local.0.lock().unwrap().len() == 2);
    assert!(events.try_recv().is_err());
    assert!(l.slot.take().is_none());
}

#[test]
fn simplified_mode_reports_unmappable_operations() {
    let mut fake = Fake::new(lister);
    fake.hang = true;
    let s = setup(fake, Timing { hello: Duration::from_millis(50), ..Timing::default() });
    s.wsl.submit(ProviderRequest::Op { tab: 3, id: 5, op: FileOp::Trash(vec![FsPath::parse_bytes(b"/tmp/bad\xFF").unwrap()]), ctl: OpCtl::default() });
    let done = event(&s.events, |e| if let ProviderEvent::OpDone { tab: 3, id: 5, error, .. } = e { error } else { None });
    assert_eq!(done.message, UNMAPPABLE);
}

fn op_events(req: &Request) -> Option<Vec<Event>> {
    let progress = OpProgress { items: 1, total_items: 2, ..OpProgress::default() };
    let conflict = Conflict { source: unix("/src/a"), target: unix("/dst/a"), from: Meta::default(), to: Meta::default() };
    Some(match req {
        Request::Op { id, .. } => vec![Event::OpProgress { id: *id, progress }, Event::OpConflict { id: *id, conflict }],
        Request::Answer { id, choice: Some(_), .. } => vec![Event::OpDone { id: *id, error: None, items: vec![(unix("/src/a"), unix("/dst/a"))] }],
        Request::Cancel { id } => vec![Event::OpDone { id: *id, error: Some(flit_core::provider::OpError::new(crate::PlatformErrorKind::Cancelled, "abgebrochen")), items: Vec::new() }],
        _ => Vec::new(),
    })
}

fn copy() -> FileOp {
    FileOp::Copy { items: vec![unix("/src/a")], dest: unix("/dst") }
}

#[test]
fn undo_for_unreplaced_operations_is_sent_back_to_the_wsl_agent() {
    let original = unix("/src/a");
    let moved = unix("/src/b");
    let op = FileOp::Rename { item: original.clone(), name: b"b".to_vec() };
    let inverse = FileOp::Rename { item: moved.clone(), name: b"a".to_vec() };
    let stamp = flit_core::undo::Stamp { size: 4, modified: 100, device: 2, file: 3, access: None };
    let plan = flit_core::undo::Replay {
        steps: vec![inverse.clone()],
        checks: vec![(moved.clone(), Some(stamp)), (original.clone(), None)],
        recheck_tree: false, captured_ns: Some(100),
    };
    let agent_plan = plan.clone();
    let s = setup(Fake::new(move |req| Some(match req {
        Request::Op { id, .. } => vec![
            Event::OpUndo { id: *id, undo: Undo::Checked(agent_plan.clone()) },
            Event::OpDone { id: *id, error: None, items: vec![(original.clone(), moved.clone())] },
        ],
        Request::Replay { id, plan } => {
            assert_eq!(plan, &agent_plan);
            vec![Event::OpDone { id: *id, error: None, items: vec![(moved.clone(), original.clone())] }]
        }
        _ => Vec::new(),
    })), Timing::default());
    s.wsl.submit(ProviderRequest::Op { tab: 1, id: 1, op, ctl: OpCtl::default() });
    let report = event(&s.events, |e| match e {
        ProviderEvent::OpDone { id: 1, error: None, report, .. } => report,
        _ => None,
    });
    assert_eq!(report.undo.checked, Some(Step::Checked(plan.clone())));
    s.wsl.submit(ProviderRequest::Op { tab: 1, id: 2, op: inverse, ctl: OpCtl::checked(plan) });
    let undone = event(&s.events, |e| match e {
        ProviderEvent::OpDone { id: 2, error: None, report, .. } => report,
        _ => None,
    });
    assert_eq!(undone.items, vec![(unix("/src/b"), unix("/src/a"))]);
    assert!(s.local.0.lock().unwrap().is_empty());
}

#[test]
fn a_wsl_replacement_without_a_backup_reports_the_specific_refusal() {
    let s = setup(Fake::new(|req| Some(match req {
        Request::Op { id, .. } => vec![
            Event::OpReplaced { id: *id, targets: vec![(unix("/dst/a"), None)] },
            Event::OpUndo { id: *id, undo: Undo::BackupUnavailable },
            Event::OpDone { id: *id, error: None, items: vec![(unix("/src/a"), unix("/dst/a"))] },
        ],
        _ => Vec::new(),
    })), Timing::default());
    s.wsl.submit(ProviderRequest::Op { tab: 1, id: 1, op: copy(), ctl: OpCtl::default() });
    let report = event(&s.events, |e| match e {
        ProviderEvent::OpDone { id: 1, error: None, report, .. } => report,
        _ => None,
    });
    assert_eq!(report.undo.replaced, vec![(unix("/dst/a"), None)]);
    assert_eq!(report.undo.checked, Some(Step::Refuse(S::ErrorsOpsUndoBackup.en())));
}

#[test]
fn operations_run_on_the_agent_with_progress_and_conflicts() {
    let s = setup(Fake::new(op_events), Timing::default());
    let ctl = OpCtl::default();
    s.wsl.submit(ProviderRequest::Op { tab: 1, id: 4, op: copy(), ctl: ctl.clone() });
    let progress = event(&s.events, |e| if let ProviderEvent::OpProgress { tab: 1, id: 4, progress } = e { Some(progress) } else { None });
    assert_eq!((progress.items, progress.total_items), (1, 2));
    let conflict = event(&s.events, |e| if let ProviderEvent::OpConflict { tab: 1, id: 4, conflict } = e { Some(conflict) } else { None });
    assert_eq!(conflict.target, unix("/dst/a"));
    ctl.answer(Resolve::Replace, true);
    let done = event(&s.events, |e| if let ProviderEvent::OpDone { tab: 1, id: 4, error, report } = e { Some((error, report)) } else { None });
    let mut report = OpReport::new(copy(), vec![(unix("/src/a"), unix("/dst/a"))]);
    report.undo.checked = Some(flit_core::undo::Step::Refuse(S::ErrorsOpsUndoUnavailable.en()));
    assert_eq!(done, (None, Some(report)));
    assert_eq!(ctl.policy(), Some(Resolve::Replace));
    assert!(matches!(&s.fake.seen()[..], [Request::Lang { code }, Request::Op { id: 1, policy: None, .. }, Request::Answer { id: 1, choice: Some(Resolve::Replace), policy: Some(Resolve::Replace) }] if code == flit_core::i18n::lang().code()));
    assert!(s.local.0.lock().unwrap().is_empty());
}

#[test]
fn a_replace_older_answer_reaches_the_agent_as_its_decision_and_its_policy() {
    let s = setup(Fake::new(op_events), Timing::default());
    let ctl = OpCtl::default();
    s.wsl.submit(ProviderRequest::Op { tab: 1, id: 4, op: copy(), ctl: ctl.clone() });
    event(&s.events, |e| if let ProviderEvent::OpConflict { tab: 1, id: 4, conflict } = e { Some(conflict) } else { None });
    ctl.answer(Resolve::ReplaceOlder, true);
    event(&s.events, |e| if let ProviderEvent::OpDone { tab: 1, id: 4, error, .. } = e { Some(error) } else { None });
    assert!(matches!(&s.fake.seen()[..], [Request::Lang { .. }, Request::Op { .. }, Request::Answer { id: 1, choice: Some(Resolve::Skip), policy: Some(Resolve::ReplaceOlder) }]), "{:?}", s.fake.seen());
}

#[test]
fn the_agent_receives_one_conflict_review_answer_and_the_live_copy_limit() {
    let make_events = |request: &Request| Some(match request {
        Request::Op { id, .. } => vec![Event::OpConflicts { id: *id, conflicts: vec![Conflict {
            source: unix("/src/a"), target: unix("/dst/a"), from: Meta::default(), to: Meta::default(),
        }] }],
        Request::ConflictAnswers { id, choices: Some(_) } => vec![Event::OpDone { id: *id, error: None, items: Vec::new() }],
        _ => Vec::new(),
    });
    let s = setup(Fake::new(make_events), Timing::default());
    let ctl = OpCtl::default();
    s.wsl.submit(ProviderRequest::Op { tab: 1, id: 4, op: copy(), ctl: ctl.clone() });
    let batch = event(&s.events, |e| if let ProviderEvent::OpConflicts { tab: 1, id: 4, conflicts } = e { Some(conflicts) } else { None });
    assert_eq!(batch.len(), 1);
    ctl.set_copy_limit(13);
    until("copy limit", || s.fake.seen().contains(&Request::CopyLimit { id: 1, megabytes_per_second: 13 }));
    ctl.set_copy_limit(0);
    until("unlimited", || s.fake.seen().contains(&Request::CopyLimit { id: 1, megabytes_per_second: 0 }));
    ctl.answer_conflicts(vec![Resolve::Skip]);
    let error = event(&s.events, |e| if let ProviderEvent::OpDone { tab: 1, id: 4, error, .. } = e { Some(error) } else { None });
    assert_eq!(error, None);
    assert!(s.fake.seen().contains(&Request::ConflictAnswers { id: 1, choices: Some(vec![Resolve::Skip]) }));
}

#[test]
fn pausing_and_resuming_an_operation_reaches_the_agent() {
    let s = setup(Fake::new(|req| if matches!(req, Request::Op { .. }) { Some(Vec::new()) } else { op_events(req) }), Timing::default());
    let ctl = OpCtl::default();
    s.wsl.submit(ProviderRequest::Op { tab: 1, id: 5, op: copy(), ctl: ctl.clone() });
    until("op", || s.fake.seen().iter().any(|r| matches!(r, Request::Op { .. })));
    ctl.pause(true);
    until("pause", || s.fake.seen().contains(&Request::Pause { id: 1, paused: true }));
    ctl.pause(false);
    until("resume", || s.fake.seen().contains(&Request::Pause { id: 1, paused: false }));
    ctl.cancel();
    until("cancel", || s.fake.seen().contains(&Request::Cancel { id: 1 }));
    assert_eq!(s.fake.seen().iter().filter(|r| matches!(r, Request::Pause { .. })).count(), 2, "each change is sent once");
}

#[test]
fn cancelling_an_operation_reaches_the_agent() {
    let s = setup(Fake::new(|req| if matches!(req, Request::Op { .. }) { Some(Vec::new()) } else { op_events(req) }), Timing::default());
    let ctl = OpCtl::new(Some(Resolve::Skip));
    s.wsl.submit(ProviderRequest::Op { tab: 1, id: 5, op: copy(), ctl: ctl.clone() });
    until("op", || s.fake.seen().iter().any(|r| matches!(r, Request::Op { policy: Some(Resolve::Skip), .. })));
    ctl.cancel();
    let error = event(&s.events, |e| if let ProviderEvent::OpDone { id: 5, error, .. } = e { Some(error) } else { None });
    let error = error.unwrap();
    assert_eq!(error.kind, crate::PlatformErrorKind::Cancelled);
    assert_eq!(error.message, "abgebrochen");
    assert!(s.fake.seen().contains(&Request::Cancel { id: 1 }));
}

#[test]
fn reports_outside_the_operation_drop_the_connection() {
    let fake = Fake::new(|req| {
        Some(match req {
            Request::Op { id, .. } => vec![Event::OpDone { id: *id, error: None, items: vec![(unix("/src/a"), unix("/etc/passwd"))] }],
            _ => Vec::new(),
        })
    });
    let s = setup(fake, Timing::default());
    s.wsl.submit(ProviderRequest::Op { tab: 1, id: 6, op: copy(), ctl: OpCtl::default() });
    let done = event(&s.events, |e| if let ProviderEvent::OpDone { id: 6, error, report, .. } = e { Some((error, report)) } else { None });
    assert_eq!(done, (Some("The connection to Ubuntu was lost".into()), None));
    until("stop", || s.fake.stops.load(Ordering::SeqCst) == 1);
}

static CLEARED: AtomicUsize = AtomicUsize::new(0);

fn cut_inside() -> Result<(Vec<FsPath>, bool), PlatformError> {
    Ok((vec![FsPath::parse(r"\\wsl.localhost\Ubuntu\src\a").unwrap(), FsPath::parse(r"\\wsl$\ubuntu\src\b").unwrap(), FsPath::parse(r"\\wsl.localhost\Ubuntu\dst\c").unwrap()], true))
}

fn copied_from_windows() -> Result<(Vec<FsPath>, bool), PlatformError> {
    Ok((vec![FsPath::parse(r"\\wsl.localhost\Ubuntu\src\a").unwrap(), FsPath::parse(r"C:\x").unwrap()], false))
}

#[test]
fn paste_inside_the_distro_runs_on_the_agent_and_clears_a_cut() {
    let fake = Fake::new(|req| {
        Some(match req {
            Request::Op { id, .. } => vec![Event::OpDone { id: *id, error: None, items: vec![(unix("/src/a"), unix("/dst/a"))] }],
            _ => Vec::new(),
        })
    });
    let clear = || {
        CLEARED.fetch_add(1, Ordering::SeqCst);
    };
    let s = setup_with(fake, Timing::default(), Clipboard { files: cut_inside, clear });
    s.wsl.submit(ProviderRequest::Op { tab: 1, id: 7, op: FileOp::Paste { dest: unix("/dst") }, ctl: OpCtl::default() });
    let report = event(&s.events, |e| if let ProviderEvent::OpDone { id: 7, report, .. } = e { report } else { None });
    assert_eq!(report.op, FileOp::Move { items: vec![unix("/src/a"), unix("/src/b")], dest: unix("/dst") });
    assert_eq!(CLEARED.load(Ordering::SeqCst), 1);
}

#[test]
fn paste_from_outside_the_distro_goes_to_the_share() {
    let s = setup_with(Fake::new(op_events), Timing::default(), Clipboard { files: copied_from_windows, clear: || {} });
    s.wsl.submit(ProviderRequest::Op { tab: 1, id: 8, op: FileOp::Paste { dest: unix("/dst") }, ctl: OpCtl::default() });
    until("share", || s.local.0.lock().unwrap().len() == 1);
    assert!(matches!(&s.local.0.lock().unwrap()[0], ProviderRequest::Op { id: 8, op: FileOp::Paste { dest }, .. } if dest.as_bytes() == br"\\wsl.localhost\Ubuntu\dst"));
    assert_eq!(s.fake.launches.load(Ordering::SeqCst), 0);
}

#[test]
fn windows_form_operations_skip_the_agent() {
    let s = setup(Fake::new(op_events), Timing::default());
    let op = FileOp::Delete(vec![FsPath::parse(r"\\wsl.localhost\Ubuntu\tmp\a").unwrap()]);
    s.wsl.submit(ProviderRequest::Op { tab: 1, id: 9, op: op.clone(), ctl: OpCtl::default() });
    until("share", || s.local.0.lock().unwrap().len() == 1);
    assert!(matches!(&s.local.0.lock().unwrap()[0], ProviderRequest::Op { id: 9, op: o, .. } if *o == op));
}

fn failing_events(req: &Request) -> Option<Vec<Event>> {
    let failure = |path: &str| Failure { path: unix(path), error: "Permission denied".into(), fatal: false };
    Some(match req {
        Request::Op { id, .. } => vec![Event::OpFailure { id: *id, failure: failure("/src/a") }],
        Request::Recover { id, choice: Some(Recover::Retry), .. } => vec![Event::OpFailure { id: *id, failure: failure("/etc/shadow") }],
        _ => Vec::new(),
    })
}

#[test]
fn failures_are_asked_through_the_agent_and_paths_outside_the_operation_end_it() {
    let s = setup(Fake::new(failing_events), Timing::default());
    let ctl = OpCtl::default();
    s.wsl.submit(ProviderRequest::Op { tab: 1, id: 6, op: copy(), ctl: ctl.clone() });
    let failure = event(&s.events, |e| if let ProviderEvent::OpFailure { tab: 1, id: 6, failure } = e { Some(failure) } else { None });
    assert_eq!((failure.path, failure.error.message.as_str(), failure.fatal), (unix("/src/a"), "Permission denied", false));
    ctl.answer_failure(Recover::Retry, false);
    let lost = event(&s.events, |e| if let ProviderEvent::OpDone { tab: 1, id: 6, error, .. } = e { Some(error) } else { None });
    assert!(lost.is_some_and(|e| e.message.contains("was lost")), "a failure naming another path is refused like a forged conflict");
    assert!(s.fake.seen().iter().any(|r| matches!(r, Request::Recover { id: 1, choice: Some(Recover::Retry), all: false })));
}

fn admin_setup(fake: Fake, timing: Timing, clipboard: Clipboard) -> Setup {
    let (tx, events) = unbounded();
    let local = Arc::new(Recorder::default());
    let wsl = WslProvider::admin(Some(Box::new(fake.clone())), local.clone(), tx, Arc::new(|| {}), timing, clipboard).unwrap();
    Setup { wsl, fake, local, events }
}

struct Refused {
    why: &'static str,
    launches: Arc<AtomicUsize>,
    missing: bool,
}

struct Complaint(&'static str);

impl Read for Complaint {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::other(self.0))
    }
}

impl AgentLauncher for Refused {
    fn build(&self) -> &str {
        "test-1"
    }

    fn launch(&self) -> io::Result<AgentProcess> {
        self.launches.fetch_add(1, Ordering::SeqCst);
        if self.missing {
            return Err(io::Error::new(io::ErrorKind::NotFound, self.why));
        }
        Ok(AgentProcess { input: Box::new(io::sink()), output: Box::new(Complaint(self.why)), stop: Box::new(|_| {}) })
    }

    fn deliver(&self) -> io::Result<()> {
        panic!("the administrator agent is never delivered")
    }
}

fn refused_setup(why: &'static str, missing: bool) -> (WslProvider, Arc<AtomicUsize>, Arc<Recorder>, Receiver<ProviderEvent>) {
    let (tx, events) = unbounded();
    let local = Arc::new(Recorder::default());
    let launches = Arc::new(AtomicUsize::new(0));
    let launcher = Refused { why, launches: launches.clone(), missing };
    let wsl = WslProvider::admin(Some(Box::new(launcher)), local.clone(), tx, Arc::new(|| {}), Timing::default(), Clipboard::default()).unwrap();
    (wsl, launches, local, events)
}

#[test]
fn the_admin_provider_lists_through_its_agent_and_says_it_waits_for_the_password() {
    let s = admin_setup(Fake::new(lister), Timing::default(), Clipboard::default());
    let snap = finished(&list(&s.wsl, 1, 1, "/root"));
    assert_eq!(names(&snap), [b"a:b".to_vec(), b"bad\xFF".to_vec()]);
    assert!(matches!(s.events.try_recv(), Ok(ProviderEvent::Status { tab: 1, gen: 1, text }) if text == "Waiting for the administrator password…"));
    finished(&list(&s.wsl, 2, 1, "/etc"));
    assert_eq!(s.fake.launches.load(Ordering::SeqCst), 1, "one agent and one password serve every root tab");
    assert!(s.local.0.lock().unwrap().is_empty());
}

#[test]
fn a_dismissed_password_denies_the_tab_and_the_next_request_asks_again() {
    let (wsl, launches, local, events) = refused_setup("the password prompt was dismissed", false);
    let l = list(&wsl, 4, 1, "/root");
    let error = event(&events, |e| if let ProviderEvent::Denied { tab: 4, error } = e { Some(error) } else { None });
    assert_eq!(error, "Cannot open the folder as administrator: the password prompt was dismissed");
    assert_eq!(finished(&l).error.as_ref().unwrap().kind, FsErrorKind::Unavailable, "the listing ends instead of hanging");
    assert!(local.0.lock().unwrap().is_empty(), "the folder is never listed as the user instead");
    list(&wsl, 5, 1, "/root");
    event(&events, |e| if let ProviderEvent::Denied { tab: 5, .. } = e { Some(()) } else { None });
    assert_eq!(launches.load(Ordering::SeqCst), 2);
}

#[test]
fn a_missing_pkexec_and_queued_operations_get_the_reason() {
    let (wsl, _, local, events) = refused_setup("pkexec is not installed", true);
    wsl.submit(ProviderRequest::Op { tab: 6, id: 3, op: FileOp::Delete(vec![unix("/root/a")]), ctl: OpCtl::default() });
    let done = event(&events, |e| if let ProviderEvent::OpDone { tab: 6, id: 3, error, .. } = e { error } else { None });
    assert_eq!(done.message, "Cannot open the folder as administrator: pkexec is not installed");
    assert!(local.0.lock().unwrap().is_empty());
}

#[test]
fn an_admin_agent_without_a_greeting_is_denied_not_simplified() {
    let mut fake = Fake::new(lister);
    fake.installed = Arc::new(Mutex::new(None));
    let s = admin_setup(fake, Timing::default(), Clipboard::default());
    list(&s.wsl, 1, 1, "/root");
    let error = event(&s.events, |e| if let ProviderEvent::Denied { error, .. } = e { Some(error) } else { None });
    assert_eq!(error, "Cannot open the folder as administrator: the administrator agent did not answer");
    assert_eq!(s.fake.deliveries.load(Ordering::SeqCst), 0);
    assert!(s.local.0.lock().unwrap().is_empty());
}

#[test]
fn admin_operations_run_on_the_agent_and_the_rest_stay_local_without_share_paths() {
    let fake = Fake::new(|req| {
        Some(match req {
            Request::Op { id, .. } => vec![Event::OpDone { id: *id, error: None, items: Vec::new() }],
            _ => Vec::new(),
        })
    });
    let s = admin_setup(fake, Timing::default(), Clipboard::default());
    s.wsl.submit(ProviderRequest::Op { tab: 2, id: 11, op: FileOp::Rename { item: unix("/etc/a"), name: b"b".to_vec() }, ctl: OpCtl::default() });
    event(&s.events, |e| if let ProviderEvent::OpDone { id: 11, error: None, .. } = e { Some(()) } else { None });
    assert!(s.fake.seen().iter().any(|r| matches!(r, Request::Op { op: FileOp::Rename { .. }, .. })));
    s.wsl.submit(ProviderRequest::Open { tab: 2, path: unix("/etc/x:y") });
    until("local", || s.local.0.lock().unwrap().len() == 1);
    assert!(matches!(&s.local.0.lock().unwrap()[0], ProviderRequest::Open { path, .. } if path.as_bytes() == b"/etc/x:y"));
}

fn copied_on_linux() -> Result<(Vec<FsPath>, bool), PlatformError> {
    Ok((vec![FsPath::parse("/home/u/a").unwrap()], false))
}

#[test]
fn admin_paste_copies_through_the_agent() {
    let fake = Fake::new(|req| {
        Some(match req {
            Request::Op { id, .. } => vec![Event::OpDone { id: *id, error: None, items: vec![(unix("/home/u/a"), unix("/etc/a"))] }],
            _ => Vec::new(),
        })
    });
    let s = admin_setup(fake, Timing::default(), Clipboard { files: copied_on_linux, clear: || {} });
    s.wsl.submit(ProviderRequest::Op { tab: 1, id: 7, op: FileOp::Paste { dest: unix("/etc") }, ctl: OpCtl::default() });
    let report = event(&s.events, |e| if let ProviderEvent::OpDone { id: 7, report, .. } = e { report } else { None });
    assert_eq!(report.op, FileOp::Copy { items: vec![unix("/home/u/a")], dest: unix("/etc") });
}

#[test]
fn a_crashed_admin_agent_reports_it_and_starts_again_only_on_demand() {
    let fake = Fake::new(|req| if matches!(req, Request::List { .. }) { None } else { Some(Vec::new()) });
    let s = admin_setup(fake, Timing::default(), Clipboard::default());
    for gen in 1..=4 {
        let snap = finished(&list(&s.wsl, 1, gen, "/root"));
        assert_eq!(snap.error.as_ref().unwrap().kind, FsErrorKind::Unavailable);
        let stopped = event(&s.events, |e| if let ProviderEvent::OpDone { tab: 1, id: 0, error, .. } = e { error } else { None });
        assert!(stopped.message.starts_with("The administrator agent stopped"), "{stopped}");
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(s.fake.launches.load(Ordering::SeqCst), gen as usize);
    }
    assert!(s.local.0.lock().unwrap().is_empty(), "crashes never fall back to listing as the user");
}

fn done_ops(req: &Request) -> Option<Vec<Event>> {
    Some(match req {
        Request::Op { id, .. } => vec![Event::OpDone { id: *id, error: None, items: Vec::new() }],
        _ => Vec::new(),
    })
}

#[test]
fn no_operation_in_an_admin_tab_runs_as_the_user() {
    let s = admin_setup(Fake::new(done_ops), Timing::default(), Clipboard { files: copied_from_windows, clear: || {} });
    let on_agent = [
        FileOp::NewFile { dir: unix("/etc"), name: b"n".to_vec(), template: Template::Empty },
        FileOp::Link { items: vec![unix("/etc/a")], dest: unix("/etc/b") },
        FileOp::Flatten(vec![unix("/etc/d")]),
    ];
    let refused = [
        FileOp::LinkAs { items: vec![unix("/etc/a")], dest: unix("/etc/b"), kind: LinkKind::Symbolic },
        FileOp::PasteDropped { dest: unix("/etc"), moving: false, data: None },
        FileOp::PasteLink { dest: unix("/etc") },
        FileOp::InstallFonts(vec![unix("/etc/f.ttf")]),
        FileOp::Paste { dest: unix("/etc") },
    ];
    for (k, op) in on_agent.iter().chain(&refused).enumerate() {
        s.wsl.submit(ProviderRequest::Op { tab: 1, id: 100 + k as u64, op: op.clone(), ctl: OpCtl::default() });
        let id = 100 + k as u64;
        let error = event(&s.events, |e| if let ProviderEvent::OpDone { id: got, error, .. } = e { (got == id).then_some(error) } else { None });
        if k < on_agent.len() {
            assert_eq!(error, None, "{op:?} runs on the agent");
        } else {
            assert_eq!(error.as_ref().map(|e| e.message.as_str()), Some("This command is not available in an administrator tab"), "{op:?}");
        }
    }
    let ran: Vec<FileOp> = s.fake.seen().into_iter().filter_map(|r| if let Request::Op { op, .. } = r { Some(op) } else { None }).collect();
    assert_eq!(ran, on_agent);
    assert!(s.local.0.lock().unwrap().is_empty(), "nothing ran as the user");
}

#[test]
fn items_written_by_root_into_a_folder_of_the_user_stay_with_root() {
    let dir = std::env::temp_dir().join(format!("flit-admin-owner-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let dest = FsPath::parse(dir.to_str().unwrap()).unwrap();
    let reported = dest.join(b"hosts");
    let fake = Fake::new(move |req| {
        Some(match req {
            Request::Op { id, op: FileOp::Copy { .. }, .. } => vec![Event::OpDone { id: *id, error: None, items: vec![(unix("/etc/hosts"), reported.clone())] }],
            Request::Op { id, .. } => vec![Event::OpDone { id: *id, error: None, items: Vec::new() }],
            _ => Vec::new(),
        })
    });
    let s = admin_setup(fake, Timing::default(), Clipboard::default());
    let ops = [
        FileOp::Copy { items: vec![unix("/etc/hosts")], dest: unix("/home/u") },
        FileOp::Move { items: vec![unix("/etc/a")], dest: unix("/home/u") },
        FileOp::NewFolder { dir: unix("/home/u"), name: b"n".to_vec() },
    ];
    for (k, op) in ops.into_iter().enumerate() {
        s.wsl.submit(ProviderRequest::Op { tab: 1, id: 5 + k as u64, op, ctl: OpCtl::default() });
        event(&s.events, |e| if let ProviderEvent::OpDone { id, .. } = e { (id == 5 + k as u64).then_some(()) } else { None });
    }
    if dest.kind() == PathKind::Unix {
        s.wsl.submit(ProviderRequest::Op { tab: 1, id: 9, op: FileOp::Copy { items: vec![unix("/etc/hosts")], dest: dest.clone() }, ctl: OpCtl::default() });
        event(&s.events, |e| if let ProviderEvent::OpDone { id: 9, .. } = e { Some(()) } else { None });
    }
    std::thread::sleep(Duration::from_millis(150));
    let changed: Vec<Request> = s.fake.seen().into_iter().filter(|r| matches!(r, Request::Op { op: FileOp::SetAccess { .. } | FileOp::Access(_), .. })).collect();
    assert!(changed.is_empty(), "the agent never changes owners after it writes: {changed:?}");
    let _ = std::fs::remove_dir_all(&dir);
}
