use crossbeam_channel::{unbounded, Receiver, RecvTimeoutError, Sender};
use flit_core::ui::{IndexState, IndexVolume, SearchBackend, SearchEvent, SearchKey, SearchRequest, SearchRow};
use flit_platform::{start_indexer, Duplex, IndexSource, Platform, PlatformError, Service, Wake, WindowId};
use flit_proto::transport::{handshake, read_frame_limited, write_frame, Hello, Protocol, MAX_FRAME};
use flit_search::proto::{Message, Sort, SortKey, VolumeInfo, VolumeState, VERSION};
use flit_search::session::{Session, SharedIndex};
use std::collections::HashMap;
use std::io::Read;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

const TICK: Duration = Duration::from_millis(250);
const HELLO_TIMEOUT: Duration = Duration::from_secs(2);
const IDLE_POLL: Duration = Duration::from_millis(500);
const FRAME_TIMEOUT: Duration = Duration::from_secs(10);

type Conn<P> = <<P as Platform>::Service as Service>::Conn;
type Out = Sender<(WindowId, SearchEvent)>;

enum Cmd {
    Req(WindowId, Arc<AtomicU64>, SearchRequest),
    Lost(WindowId, String),
    Forget(WindowId),
}

pub struct SearchClient {
    tx: Sender<Cmd>,
    latest: HashMap<WindowId, Arc<AtomicU64>>,
    pub events: Receiver<(WindowId, SearchEvent)>,
}

impl SearchClient {
    pub fn start<P: Platform>(wake: Wake, utc_offset_min: i32) -> Result<Self, PlatformError> {
        let (tx, rx) = unbounded();
        let (out, events) = unbounded();
        let worker = Worker::<P> { rx, me: tx.clone(), out, wake, utc_offset_min, remote_failed: None, local: None, links: HashMap::new(), last_query: HashMap::new(), latests: HashMap::new() };
        std::thread::Builder::new()
            .name("flit-search".into())
            .spawn(move || worker.run())
            .map_err(|e| PlatformError::new(-1, format!("cannot start flit-search: {e}")))?;
        Ok(Self { tx, latest: HashMap::new(), events })
    }

    pub fn submit(&mut self, window: WindowId, req: SearchRequest) {
        let latest = self.latest.entry(window).or_default().clone();
        if let SearchRequest::Query { gen, .. } = &req {
            latest.store(*gen, Ordering::Release);
        }
        let _ = self.tx.send(Cmd::Req(window, latest, req));
    }

    pub fn forget(&mut self, window: WindowId) {
        self.latest.remove(&window);
        let _ = self.tx.send(Cmd::Forget(window));
    }
}

pub fn to_message(req: &SearchRequest) -> Option<Message> {
    Some(match req {
        SearchRequest::Query { gen, text, sort } => {
            let key = match sort.key {
                SearchKey::Name => SortKey::Name,
                SearchKey::Folder => SortKey::Path,
                SearchKey::Size => SortKey::Size,
                SearchKey::Modified => SortKey::Modified,
            };
            Message::Query { gen: *gen, text: text.clone(), sort: Sort { key, descending: sort.descending } }
        }
        SearchRequest::Rows { gen, from, to } => Message::GetRows { gen: *gen, from: *from, to: *to },
        SearchRequest::Close => return None,
    })
}

fn volume(v: VolumeInfo) -> IndexVolume {
    let state = match v.state {
        VolumeState::Scanning { progress } => IndexState::Scanning(progress),
        VolumeState::Ready => IndexState::Ready,
        VolumeState::Unsupported => IndexState::Error("not supported".into()),
        VolumeState::Error(e) => IndexState::Error(e),
    };
    IndexVolume { id: v.id, files: v.files, state }
}

pub fn to_event(msg: Message, backend: SearchBackend, note: &Option<String>) -> Option<SearchEvent> {
    Some(match msg {
        Message::QueryDone { gen, total } => SearchEvent::Done { gen, total },
        Message::Rows { gen, from, entries, dirs } => {
            let rows = match entries.entries(true) {
                Ok(it) if dirs.len() == entries.len() => it.zip(dirs).map(|((name, meta), dir)| SearchRow { name: name.to_vec(), dir, meta }).collect(),
                Ok(_) => return Some(SearchEvent::Error { message: "the index sent rows without folders".into() }),
                Err(e) => return Some(SearchEvent::Error { message: format!("the index sent invalid rows: {e}") }),
            };
            SearchEvent::Rows { gen, from, rows }
        }
        Message::IndexStatus { volumes } => SearchEvent::Status { backend, volumes: volumes.into_iter().map(volume).collect(), note: note.clone() },
        Message::Error { message } => SearchEvent::Error { message },
        Message::Query { .. } | Message::GetRows { .. } => return None,
    })
}

enum Link<C> {
    Remote { conn: C, alive: Arc<AtomicBool> },
    Local { session: Session, latest: Arc<AtomicU64> },
}

struct Worker<P: Platform> {
    rx: Receiver<Cmd>,
    me: Sender<Cmd>,
    out: Out,
    wake: Wake,
    utc_offset_min: i32,
    remote_failed: Option<String>,
    local: Option<Arc<SharedIndex>>,
    links: HashMap<WindowId, Link<Conn<P>>>,
    last_query: HashMap<WindowId, SearchRequest>,
    latests: HashMap<WindowId, Arc<AtomicU64>>,
}

fn connect<P: Platform>() -> Result<Conn<P>, String> {
    let mut conn = P::Service::connect().map_err(|e| format!("flit-indexd is not reachable: {}", e.message.trim()))?;
    conn.set_read_timeout(Some(HELLO_TIMEOUT));
    handshake(&mut conn, &Hello::new(Protocol::Index, VERSION)).map_err(|e| format!("flit-indexd: {e}"))?;
    Ok(conn)
}

fn reader<C: Duplex>(mut conn: C, window: WindowId, alive: Arc<AtomicBool>, out: Out, wake: Wake, me: Sender<Cmd>) {
    let mut buf = Vec::new();
    let lost = |why: String| {
        if alive.load(Ordering::Acquire) {
            let _ = me.send(Cmd::Lost(window, why));
        }
    };
    loop {
        conn.set_read_timeout(Some(IDLE_POLL));
        let mut first = [0u8; 1];
        match conn.read(&mut first) {
            Ok(1) => {}
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut && alive.load(Ordering::Acquire) => continue,
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => return,
            _ => return lost("the index service closed the connection".into()),
        }
        conn.set_read_timeout(Some(FRAME_TIMEOUT));
        match read_frame_limited::<_, Message>(&mut (&first[..]).chain(&mut conn), &mut buf, MAX_FRAME) {
            Ok(msg) => {
                if let Some(ev) = to_event(msg, SearchBackend::Service, &None) {
                    if out.send((window, ev)).is_err() {
                        return;
                    }
                    wake();
                }
            }
            Err(e) => return lost(format!("the index service connection failed: {e}")),
        }
    }
}

impl<P: Platform> Worker<P> {
    fn run(mut self) {
        loop {
            match self.rx.recv_timeout(TICK) {
                Ok(Cmd::Req(window, latest, req)) => self.request(window, latest, req),
                Ok(Cmd::Lost(window, why)) => self.lost(window, why),
                Ok(Cmd::Forget(window)) => {
                    self.drop_link(window);
                    self.last_query.remove(&window);
                    self.latests.remove(&window);
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            self.tick();
        }
    }

    fn send(&self, window: WindowId, ev: SearchEvent) {
        if self.out.send((window, ev)).is_ok() {
            (self.wake)();
        }
    }

    fn drop_link(&mut self, window: WindowId) {
        if let Some(Link::Remote { alive, .. }) = self.links.remove(&window) {
            alive.store(false, Ordering::Release);
        }
    }

    fn local_link(&mut self, latest: Arc<AtomicU64>) -> Result<Link<Conn<P>>, String> {
        let shared = match &self.local {
            Some(s) => s.clone(),
            None => {
                let shared = SharedIndex::new();
                let source = P::FallbackIndex::new().map_err(|e| e.message)?;
                start_indexer(source, shared.clone()).map_err(|e| e.message)?;
                self.local = Some(shared.clone());
                shared
            }
        };
        Ok(Link::Local { session: Session::new(shared, self.utc_offset_min), latest })
    }

    fn open(&mut self, window: WindowId, latest: Arc<AtomicU64>) {
        if self.remote_failed.is_none() {
            match connect::<P>() {
                Ok(conn) => {
                    let alive = Arc::new(AtomicBool::new(true));
                    if let Ok(read_half) = conn.try_clone() {
                        let (a, out, wake, me) = (alive.clone(), self.out.clone(), self.wake.clone(), self.me.clone());
                        let spawned = std::thread::Builder::new().name("flit-search-read".into()).spawn(move || reader(read_half, window, a, out, wake, me));
                        if spawned.is_ok() {
                            self.links.insert(window, Link::Remote { conn, alive });
                            self.send(window, SearchEvent::Status { backend: SearchBackend::Service, volumes: Vec::new(), note: None });
                            return;
                        }
                    }
                    self.remote_failed = Some("cannot read from flit-indexd".into());
                }
                Err(why) => {
                    tracing::info!("search falls back to the in-process index: {why}");
                    self.remote_failed = Some(why);
                }
            }
        }
        let note = self.remote_failed.clone();
        match self.local_link(latest) {
            Ok(link) => {
                self.links.insert(window, link);
                self.send(window, SearchEvent::Status { backend: SearchBackend::Fallback, volumes: Vec::new(), note });
            }
            Err(e) => self.send(window, SearchEvent::Error { message: format!("search is unavailable: {e}") }),
        }
    }

    fn request(&mut self, window: WindowId, latest: Arc<AtomicU64>, req: SearchRequest) {
        self.latests.insert(window, latest.clone());
        if matches!(req, SearchRequest::Query { .. }) {
            self.last_query.insert(window, req.clone());
        }
        if req == SearchRequest::Close {
            if let Some(Link::Local { session, .. }) = self.links.get_mut(&window) {
                *session = Session::new(self.local.clone().unwrap_or_default(), self.utc_offset_min);
            }
            return;
        }
        if !self.links.contains_key(&window) {
            self.open(window, latest);
        }
        let Some(msg) = to_message(&req) else { return };
        let note = self.remote_failed.clone();
        let (out, wake) = (self.out.clone(), self.wake.clone());
        match self.links.get_mut(&window) {
            Some(Link::Remote { conn, .. }) => {
                if let Err(e) = write_frame(conn, &msg) {
                    self.lost(window, format!("cannot send to flit-indexd: {e}"));
                }
            }
            Some(Link::Local { session, latest }) => {
                session.handle(msg, latest, &mut |m| {
                    if let Some(ev) = to_event(m, SearchBackend::Fallback, &note) {
                        let _ = out.send((window, ev));
                    }
                });
                wake();
            }
            None => {}
        }
    }

    fn lost(&mut self, window: WindowId, why: String) {
        if !matches!(self.links.get(&window), Some(Link::Remote { .. })) {
            return;
        }
        tracing::warn!("{why}; switching to the in-process index");
        self.drop_link(window);
        self.remote_failed = Some(why);
        let latest = self.latests.get(&window).cloned().unwrap_or_default();
        self.open(window, latest.clone());
        if let Some(q) = self.last_query.get(&window).cloned() {
            self.request(window, latest, q);
        }
    }

    fn tick(&mut self) {
        let note = self.remote_failed.clone();
        let mut sent = false;
        for (&window, link) in self.links.iter_mut() {
            if let Link::Local { session, latest } = link {
                session.tick(latest, &mut |m| {
                    if let Some(ev) = to_event(m, SearchBackend::Fallback, &note) {
                        sent |= self.out.send((window, ev)).is_ok();
                    }
                });
            }
        }
        if sent {
            (self.wake)();
        }
    }
}

#[cfg(test)]
#[path = "../test/search_client.rs"]
mod tests;
