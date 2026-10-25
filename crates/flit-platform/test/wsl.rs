use super::*;
use flit_core::listing::{ChangeKind, DirChange};
use flit_core::provider::{OpCtl, Template};
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
    let (tx, events) = unbounded();
    let local = Arc::new(Recorder::default());
    let wsl = WslProvider::new("Ubuntu".into(), Some(Box::new(fake.clone())), local.clone(), tx, Arc::new(|| {}), timing).unwrap();
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
            Request::Watch { id, .. } => vec![Event::Changes { id: *id, changes: vec![DirChange { kind: ChangeKind::Added, name: b"x".to_vec(), meta: Some(Meta::default()) }] }],
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
    assert!(note.contains("Press F5"));
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
fn trash_goes_to_the_agent_and_the_rest_to_the_share() {
    let fake = Fake::new(|req| {
        Some(match req {
            Request::Op { id, .. } => vec![Event::OpDone { id: *id, error: Some("nope".into()) }],
            Request::DirSizes { id, items, .. } => items.iter().map(|(idx, _)| Event::DirSize { id: *id, idx: *idx, size: 5 }).chain([Event::DirSize { id: *id, idx: 99, size: 1 }]).collect(),
            _ => Vec::new(),
        })
    });
    let s = setup(fake, Timing::default());
    s.wsl.submit(ProviderRequest::Op { tab: 2, id: 11, op: FileOp::Trash(vec![unix("/tmp/a")]), ctl: OpCtl::default() });
    let done = event(&s.events, |e| if let ProviderEvent::OpDone { tab: 2, id: 11, error, report } = e { Some((error, report)) } else { None });
    assert_eq!(done, (Some("nope".into()), Some(OpReport { op: FileOp::Trash(vec![unix("/tmp/a")]), items: Vec::new() })));
    let token = GenToken::default();
    token.set(1);
    s.wsl.submit(ProviderRequest::DirSizes { tab: 2, gen: 1, dir: unix("/"), items: vec![(3, b"usr".to_vec())], cancel: token });
    let size = event(&s.events, |e| if let ProviderEvent::DirSize { idx, size, .. } = e { Some((idx, size)) } else { None });
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
    let wsl = WslProvider::new("Ubuntu".into(), None, local.clone(), tx, Arc::new(|| {}), Timing::default()).unwrap();
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
    assert_eq!(done, UNMAPPABLE);
}
