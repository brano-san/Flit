use crate::find::{self, Found, Hit};
use crossbeam_channel::{unbounded, Receiver, RecvTimeoutError, Sender};
use flit_core::tags::TagCell;
use flit_core::ui::{IndexState, IndexVolume, SearchBackend, SearchEvent, SearchKey, SearchRequest, SearchRow, SearchScope, SearchSort, SearchSource};
use flit_core::FsPath;
use flit_platform::{start_indexer, Duplex, IndexSource, Platform, PlatformError, Service, Wake, WindowId};
use flit_proto::transport::{handshake, read_frame_limited, write_frame, Hello, Protocol, MAX_FRAME};
use flit_search::engine::MAX_ROWS;
use flit_search::index::lowercase;
use flit_search::proto::{Message, Sort, SortKey, VolumeInfo, VolumeState, VERSION};
use flit_search::query::{self, Clock, Query};
use flit_search::session::{Session, SharedIndex, FIRST_PAGE};
use std::collections::HashMap;
use std::io::Read;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const TICK: Duration = Duration::from_millis(250);
const LIVE_TICK: Duration = Duration::from_millis(100);
const HELLO_TIMEOUT: Duration = Duration::from_secs(2);
const IDLE_POLL: Duration = Duration::from_millis(500);
const FRAME_TIMEOUT: Duration = Duration::from_secs(10);

type Conn<P> = <<P as Platform>::Service as Service>::Conn;
type Out = Sender<(WindowId, SearchEvent)>;

enum Cmd {
    Req(WindowId, Arc<AtomicU64>, SearchRequest),
    Lost(WindowId, String),
    Forget(WindowId),
    Finished(WindowId, u64),
}

pub struct SearchClient {
    tx: Sender<Cmd>,
    latest: HashMap<WindowId, Arc<AtomicU64>>,
    pub events: Receiver<(WindowId, SearchEvent)>,
}

impl SearchClient {
    pub fn start<P: Platform>(wake: Wake, utc_offset_min: i32, tags: TagCell) -> Result<Self, PlatformError> {
        let (tx, rx) = unbounded();
        let (out, events) = unbounded();
        let worker = Worker::<P>::new(rx, tx.clone(), out, wake, utc_offset_min, tags);
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

fn index_text(text: &str, scope: &SearchScope) -> String {
    match scope {
        SearchScope::Indexed(folder) => {
            let shown = folder.path().display();
            format!("{text} path:\"{}\\\"", shown.trim_end_matches('\\'))
        }
        _ => text.to_owned(),
    }
}

pub fn to_message(req: &SearchRequest) -> Option<Message> {
    Some(match req {
        SearchRequest::Query { gen, text, sort, scope } => {
            let key = match sort.key {
                SearchKey::Name => SortKey::Name,
                SearchKey::Folder => SortKey::Path,
                SearchKey::Size => SortKey::Size,
                SearchKey::Modified => SortKey::Modified,
            };
            Message::Query { gen: *gen, text: index_text(text, scope), sort: Sort { key, descending: sort.descending } }
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
        Message::Groups { gen, starts } => SearchEvent::Groups { gen, starts },
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

enum Kind {
    Tags { query: Query, folder: Option<Vec<u8>>, version: u64 },
    Live(SearchSource),
}

struct Hits {
    gen: u64,
    sort: SearchSort,
    found: Arc<Mutex<Found>>,
    kind: Kind,
    sent: Option<(usize, bool)>,
    cancel: Arc<AtomicBool>,
}

impl Drop for Hits {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}

impl Hits {
    fn new(gen: u64, sort: SearchSort, kind: Kind) -> Self {
        Self { gen, sort, found: Arc::default(), kind, sent: None, cancel: Arc::default() }
    }

    fn running(&self) -> bool {
        matches!(self.kind, Kind::Live(_)) && self.sent.is_none_or(|(_, done)| !done)
    }

    fn rows(&self, from: u64, to: u64) -> Vec<SearchRow> {
        let total = self.sent.map_or(0, |(n, _)| n) as u64;
        let end = to.min(from.saturating_add(MAX_ROWS)).min(total);
        let f = self.found.lock().unwrap_or_else(|e| e.into_inner());
        f.hits.get(from as usize..end as usize).unwrap_or_default().iter().map(|h| h.row.clone()).collect()
    }

    fn publish(&mut self, send: &mut dyn FnMut(SearchEvent)) {
        let mut f = self.found.lock().unwrap_or_else(|e| e.into_inner());
        let (total, done, folders) = (f.hits.len(), f.done, f.folders);
        if self.sent == Some((total, done)) {
            return;
        }
        find::sort(&mut f.hits, self.sort);
        let starts = (self.sort.key == SearchKey::Folder).then(|| find::groups(&f.hits));
        let first: Vec<SearchRow> = f.hits.iter().take(FIRST_PAGE as usize).map(|h| h.row.clone()).collect();
        let error = f.error.take();
        drop(f);
        self.sent = Some((total, done));
        let gen = self.gen;
        if let Some(message) = error {
            send(SearchEvent::Error { message });
        }
        send(match self.kind {
            Kind::Live(source) => SearchEvent::Found { gen, total: total as u64, folders, done, source },
            Kind::Tags { .. } => SearchEvent::Done { gen, total: total as u64 },
        });
        if let Some(starts) = starts.filter(|s| !s.is_empty()) {
            send(SearchEvent::Groups { gen, starts });
        }
        if !first.is_empty() {
            send(SearchEvent::Rows { gen, from: 0, rows: first });
        }
    }
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
    tags: TagCell,
    hits: HashMap<WindowId, Hits>,
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
    fn new(rx: Receiver<Cmd>, me: Sender<Cmd>, out: Out, wake: Wake, utc_offset_min: i32, tags: TagCell) -> Self {
        Self { rx, me, out, wake, utc_offset_min, remote_failed: None, local: None, links: HashMap::new(), last_query: HashMap::new(), latests: HashMap::new(), tags, hits: HashMap::new() }
    }

    fn run(mut self) {
        loop {
            let wait = if self.hits.values().any(Hits::running) { LIVE_TICK } else { TICK };
            match self.rx.recv_timeout(wait) {
                Ok(Cmd::Req(window, latest, req)) => self.request(window, latest, req),
                Ok(Cmd::Lost(window, why)) => self.lost(window, why),
                Ok(Cmd::Finished(window, gen)) => self.publish(window, gen),
                Ok(Cmd::Forget(window)) => {
                    self.drop_link(window);
                    self.hits.remove(&window);
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

    fn publish(&mut self, window: WindowId, gen: u64) {
        let out = self.out.clone();
        let Some(h) = self.hits.get_mut(&window).filter(|h| h.gen == gen) else { return };
        let mut sent = false;
        h.publish(&mut |ev| sent |= out.send((window, ev)).is_ok());
        if sent {
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
                start_indexer(source, shared.clone(), P::state_dir().map(|d| d.join("search"))).map_err(|e| e.message)?;
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
        if self.local_request(window, &latest, &req) {
            return;
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

    fn local_request(&mut self, window: WindowId, latest: &Arc<AtomicU64>, req: &SearchRequest) -> bool {
        match req {
            SearchRequest::Query { gen, text, sort, scope } => {
                self.hits.remove(&window);
                let folder = scope.folder().and_then(|f| f.to_windows());
                if let Some(aqs) = query::shell_query(text) {
                    self.start_live(window, latest, *gen, *sort, SearchSource::Shell, move |keep_going, found| run_shell::<P>(folder.as_ref(), &aqs, keep_going, found));
                    return true;
                }
                match query::parse(text, Clock::system(self.utc_offset_min)) {
                    Ok(q) if q.has_tags() => {
                        let folder = folder.map(|f| lowercase(f.as_bytes()));
                        self.run_tags(window, *gen, q, *sort, folder);
                        true
                    }
                    Ok(q) => match (scope, folder) {
                        (SearchScope::Walk(_), Some(root)) => {
                            self.start_live(window, latest, *gen, *sort, SearchSource::Walk, move |keep_going, found| find::walk(&P::fs(), &root, &q, keep_going, found));
                            true
                        }
                        _ => false,
                    },
                    Err(_) => false,
                }
            }
            SearchRequest::Rows { gen, from, to } => {
                let Some(h) = self.hits.get(&window).filter(|h| h.gen == *gen) else { return false };
                let rows = h.rows(*from, *to);
                if !rows.is_empty() {
                    self.send(window, SearchEvent::Rows { gen: *gen, from: *from, rows });
                }
                true
            }
            SearchRequest::Close => {
                self.hits.remove(&window);
                false
            }
        }
    }

    fn start_live<F>(&mut self, window: WindowId, latest: &Arc<AtomicU64>, gen: u64, sort: SearchSort, source: SearchSource, body: F)
    where
        F: FnOnce(find::KeepGoing<'_>, &Mutex<Found>) + Send + 'static,
    {
        let hits = Hits::new(gen, sort, Kind::Live(source));
        let (found, cancel, latest, me) = (hits.found.clone(), hits.cancel.clone(), latest.clone(), self.me.clone());
        self.hits.insert(window, hits);
        let spawned = std::thread::Builder::new().name("flit-find".into()).spawn(move || {
            let started = Instant::now();
            let keep_going = || !cancel.load(Ordering::Acquire) && latest.load(Ordering::Acquire) == gen;
            body(&keep_going, &found);
            let mut f = found.lock().unwrap_or_else(|e| e.into_inner());
            f.done = true;
            tracing::debug!("live search found {} items in {} folders in {:.1} ms", f.hits.len(), f.folders, started.elapsed().as_secs_f64() * 1e3);
            drop(f);
            let _ = me.send(Cmd::Finished(window, gen));
        });
        if let Err(e) = spawned {
            self.hits.remove(&window);
            self.send(window, SearchEvent::Error { message: format!("cannot start the search: {e}") });
            self.send(window, SearchEvent::Done { gen, total: 0 });
        }
    }

    fn run_tags(&mut self, window: WindowId, gen: u64, query: Query, sort: SearchSort, folder: Option<Vec<u8>>) {
        let tags = self.tags.get();
        match crate::tags::search(&tags, &query, sort) {
            Ok(rows) => {
                let mut hits = Hits::new(gen, sort, Kind::Tags { query, folder: folder.clone(), version: tags.version });
                {
                    let mut f = hits.found.lock().unwrap_or_else(|e| e.into_inner());
                    f.hits = rows.into_iter().map(Hit::new).filter(|h| folder.as_deref().is_none_or(|lf| h.within(lf))).collect();
                    f.done = true;
                }
                let out = self.out.clone();
                hits.publish(&mut |ev| {
                    let _ = out.send((window, ev));
                });
                (self.wake)();
                self.hits.insert(window, hits);
            }
            Err(message) => {
                self.hits.remove(&window);
                self.send(window, SearchEvent::Error { message });
                self.send(window, SearchEvent::Done { gen, total: 0 });
            }
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
        let version = self.tags.get().version;
        let current = |w: &WindowId, gen: u64| self.latests.get(w).is_some_and(|l| l.load(Ordering::Acquire) == gen);
        let mut stale = Vec::new();
        let mut live = Vec::new();
        for (w, h) in &self.hits {
            match &h.kind {
                Kind::Tags { query, folder, version: v } if *v != version && current(w, h.gen) => stale.push((*w, h.gen, query.clone(), h.sort, folder.clone())),
                Kind::Live(_) if h.running() => live.push((*w, h.gen)),
                _ => {}
            }
        }
        for (window, gen, query, sort, folder) in stale {
            self.run_tags(window, gen, query, sort, folder);
        }
        for (window, gen) in live {
            self.publish(window, gen);
        }
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

fn run_shell<P: Platform>(folder: Option<&FsPath>, aqs: &str, keep_going: find::KeepGoing<'_>, found: &Mutex<Found>) {
    let mut batch = Vec::new();
    let flush = |batch: &mut Vec<Hit>| found.lock().unwrap_or_else(|e| e.into_inner()).hits.append(batch);
    let result = P::shell_search(folder, aqs, keep_going, &mut |path, meta| {
        let dir = path.parent().map(|p| p.as_bytes().to_vec()).unwrap_or_default();
        batch.push(Hit::new(SearchRow { name: path.file_name().to_vec(), dir, meta }));
        if batch.len() >= FIRST_PAGE as usize {
            flush(&mut batch);
        }
    });
    flush(&mut batch);
    if let Err(e) = result {
        tracing::warn!("Windows Search: {e}");
        found.lock().unwrap_or_else(|e| e.into_inner()).error = Some(format!("Windows Search: {}", e.message.trim()));
    }
}

#[cfg(test)]
#[path = "../test/search_client.rs"]
mod tests;
