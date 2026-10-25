use crate::local::{snapshot, via_unc, PARTIAL_COST_FACTOR, PARTIAL_EVERY};
use crate::{PlatformError, Provider, Wake};
use crossbeam_channel::{unbounded, Receiver, RecvTimeoutError, Sender};
use flit_core::listing::Listing;
use flit_core::provider::{FileOp, GenToken, OpId, OpReport, SnapshotSlot};
use flit_core::sort::{SortKeys, ViewSpec};
use flit_core::{FsError, FsErrorKind, FsPath, ProviderEvent, ProviderRequest, TabId};
use flit_proto::provider::{Event, ReqId, Request, VERSION};
use flit_proto::transport::{read_frame, read_hello, write_frame, Hello, ProtoError, Protocol};
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{self, Read, Write};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Clone, Copy)]
pub struct AgentImage {
    pub bytes: &'static [u8],
    pub build: &'static str,
}

#[derive(Clone, Copy, Default)]
pub struct AgentImages {
    pub x64: Option<AgentImage>,
    pub arm64: Option<AgentImage>,
}

pub struct AgentProcess {
    pub input: Box<dyn Write + Send>,
    pub output: Box<dyn Read + Send>,
    pub stop: Box<dyn FnOnce(Duration) + Send>,
}

pub trait AgentLauncher: Send + 'static {
    fn build(&self) -> &str;
    fn launch(&self) -> io::Result<AgentProcess>;
    fn deliver(&self) -> io::Result<()>;
}

#[derive(Clone, Copy, Debug)]
pub struct Timing {
    pub hello: Duration,
    pub idle: Duration,
    pub crash_window: Duration,
    pub max_crashes: usize,
    pub stop_grace: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self { hello: Duration::from_secs(15), idle: Duration::from_secs(300), crash_window: Duration::from_secs(300), max_crashes: 3, stop_grace: Duration::from_secs(1) }
    }
}

pub const UNMAPPABLE: &str = "Some names cannot be used by Windows shell operations";

enum Msg {
    Req(ProviderRequest),
    Hello(u64, Result<String, ProtoError>),
    Event(u64, Event),
    Lost(u64, String),
    Quit,
}

struct Conn {
    id: u64,
    input: Box<dyn Write + Send>,
    stop: Box<dyn FnOnce(Duration) + Send>,
}

impl Conn {
    fn end(self, grace: Duration) {
        drop(self.input);
        (self.stop)(grace);
    }
}

enum State {
    Idle,
    Starting { conn: Conn, deadline: Instant, delivered: bool },
    Ready(Conn),
    Simplified,
}

struct ListState {
    gen: u64,
    path: FsPath,
    view: ViewSpec,
    out: Arc<SnapshotSlot>,
    cancel: GenToken,
    listing: Listing,
    keys: SortKeys,
    start: Instant,
    next_partial: Instant,
}

impl ListState {
    fn publish(&mut self, wake: &Wake) {
        let began = Instant::now();
        if began < self.next_partial {
            return;
        }
        let snap = snapshot(self.gen, &self.path, self.listing.clone(), self.keys.clone(), self.view, false, None, self.start.elapsed());
        if self.cancel.is_current(self.gen) {
            self.out.publish(snap);
            wake();
        }
        let done = Instant::now();
        self.next_partial = done + PARTIAL_EVERY.max((done - began) * PARTIAL_COST_FACTOR);
    }

    fn finish(self, error: Option<FsError>, wake: &Wake) {
        if self.cancel.is_current(self.gen) {
            self.out.publish(snapshot(self.gen, &self.path, self.listing, self.keys, self.view, true, error, self.start.elapsed()));
            wake();
        }
    }
}

enum Pending {
    List(Box<ListState>),
    Sizes { tab: TabId, gen: u64, cancel: GenToken, wanted: HashSet<u32> },
    Op { tab: TabId, op: OpId, request: FileOp },
}

struct Ctl {
    distro: String,
    launcher: Option<Box<dyn AgentLauncher>>,
    local: Arc<dyn Provider>,
    events: Sender<ProviderEvent>,
    wake: Wake,
    tx: Sender<Msg>,
    timing: Timing,
    state: State,
    conns: u64,
    next_id: ReqId,
    queue: Vec<ProviderRequest>,
    pending: HashMap<ReqId, Pending>,
    watches: HashMap<TabId, (u64, ReqId)>,
    crashes: VecDeque<Instant>,
    busy_at: Instant,
}

pub struct WslProvider {
    tx: Sender<Msg>,
}

impl WslProvider {
    pub fn new(distro: String, launcher: Option<Box<dyn AgentLauncher>>, local: Arc<dyn Provider>, events: Sender<ProviderEvent>, wake: Wake, timing: Timing) -> Result<Self, PlatformError> {
        let (tx, rx) = unbounded();
        let name = format!("flit-wsl-{distro}");
        let present = launcher.is_some();
        let ctl = Ctl {
            distro,
            launcher,
            local,
            events,
            wake,
            tx: tx.clone(),
            timing,
            state: if present { State::Idle } else { State::Simplified },
            conns: 0,
            next_id: 0,
            queue: Vec::new(),
            pending: HashMap::new(),
            watches: HashMap::new(),
            crashes: VecDeque::new(),
            busy_at: Instant::now(),
        };
        std::thread::Builder::new().name(name.clone()).spawn(move || ctl.run(rx)).map_err(|e| PlatformError::new(-1, format!("cannot start {name}: {e}")))?;
        Ok(Self { tx })
    }
}

impl Provider for WslProvider {
    fn submit(&self, req: ProviderRequest) {
        let _ = self.tx.send(Msg::Req(req));
    }
}

impl Drop for WslProvider {
    fn drop(&mut self) {
        let _ = self.tx.send(Msg::Quit);
    }
}

fn hello(build: &str) -> Hello {
    Hello { build: build.to_owned(), ..Hello::new(Protocol::Provider, VERSION) }
}

fn starts_agent(req: &ProviderRequest) -> bool {
    matches!(req, ProviderRequest::List { .. } | ProviderRequest::DirSizes { .. } | ProviderRequest::Op { op: FileOp::Trash(_), .. })
}

fn for_agent(req: &ProviderRequest) -> bool {
    starts_agent(req) || matches!(req, ProviderRequest::Watch { .. })
}

fn read_events(id: u64, mut output: Box<dyn Read + Send>, ours: Hello, tx: Sender<Msg>) {
    let theirs = read_hello(&mut output, &ours).map(|h| h.build);
    let ok = theirs.is_ok();
    if tx.send(Msg::Hello(id, theirs)).is_err() || !ok {
        return;
    }
    let mut buf = Vec::new();
    let lost = loop {
        match read_frame::<_, Event>(&mut output, &mut buf) {
            Ok(ev) => match ev.validate(false) {
                Ok(()) => {
                    if tx.send(Msg::Event(id, ev)).is_err() {
                        return;
                    }
                }
                Err(e) => break e.to_string(),
            },
            Err(e) => break e.to_string(),
        }
    };
    let _ = tx.send(Msg::Lost(id, lost));
}

impl Ctl {
    fn run(mut self, rx: Receiver<Msg>) {
        loop {
            let msg = match self.deadline() {
                Some(at) => rx.recv_timeout(at.saturating_duration_since(Instant::now())),
                None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
            };
            match msg {
                Ok(Msg::Req(req)) => self.request(req),
                Ok(Msg::Hello(id, theirs)) => self.hello(id, theirs),
                Ok(Msg::Event(id, ev)) => {
                    if matches!(&self.state, State::Ready(c) if c.id == id) {
                        self.busy_at = Instant::now();
                        self.event(ev);
                    }
                }
                Ok(Msg::Lost(id, why)) => {
                    if matches!(&self.state, State::Ready(c) if c.id == id) {
                        self.crashed(&why);
                    }
                }
                Err(RecvTimeoutError::Timeout) => self.timer(),
                Ok(Msg::Quit) | Err(RecvTimeoutError::Disconnected) => return self.quit(),
            }
        }
    }

    fn quit(self) {
        if let State::Starting { conn, .. } | State::Ready(conn) = self.state {
            conn.end(Duration::ZERO);
        }
    }

    fn idle(&self) -> bool {
        self.watches.is_empty() && self.pending.is_empty()
    }

    fn deadline(&self) -> Option<Instant> {
        match &self.state {
            State::Starting { deadline, .. } => Some(*deadline),
            State::Ready(_) if self.idle() => Some(self.busy_at + self.timing.idle),
            _ => None,
        }
    }

    fn timer(&mut self) {
        let now = Instant::now();
        match std::mem::replace(&mut self.state, State::Idle) {
            State::Starting { conn, deadline, .. } if now >= deadline => {
                tracing::warn!("WSL agent for {} sent no greeting in {:?}", self.distro, self.timing.hello);
                conn.end(Duration::ZERO);
                self.simplify();
            }
            State::Ready(mut conn) if self.idle() && now >= self.busy_at + self.timing.idle => {
                tracing::info!("stopping the idle WSL agent for {}", self.distro);
                let _ = write_frame(&mut conn.input, &Request::Shutdown).and_then(|()| conn.input.flush().map_err(ProtoError::Io));
                conn.end(self.timing.stop_grace);
            }
            other => self.state = other,
        }
    }

    fn emit(&self, ev: ProviderEvent) {
        let _ = self.events.send(ev);
        (self.wake)();
    }

    fn fallback(&self, req: ProviderRequest) {
        let op = match &req {
            ProviderRequest::Op { tab, id, .. } => Some((*tab, *id)),
            _ => None,
        };
        match (via_unc(&self.distro, req), op) {
            (Some(req), _) => self.local.submit(req),
            (None, Some((tab, id))) => self.emit(ProviderEvent::OpDone { tab, id, error: Some(UNMAPPABLE.into()), report: None }),
            (None, None) => tracing::warn!("path is not reachable through \\\\wsl.localhost\\{}", self.distro),
        }
    }

    fn note(&self, req: &ProviderRequest) {
        if let ProviderRequest::List { tab, gen, .. } = req {
            self.emit(ProviderEvent::Status { tab: *tab, gen: *gen, text: format!("Starting {}…", self.distro) });
        }
    }

    fn request(&mut self, req: ProviderRequest) {
        if matches!(self.state, State::Simplified) || !for_agent(&req) {
            return self.fallback(req);
        }
        if matches!(self.state, State::Ready(_)) {
            return self.dispatch(req);
        }
        if let ProviderRequest::Watch { tab, path: None, .. } = req {
            return self.queue.retain(|q| !matches!(q, ProviderRequest::Watch { tab: t, .. } if *t == tab));
        }
        let start = matches!(self.state, State::Idle) && starts_agent(&req);
        if !start && matches!(self.state, State::Starting { .. }) {
            self.note(&req);
        }
        self.queue.push(req);
        if start {
            self.queue.iter().for_each(|r| self.note(r));
            self.start(false);
        }
    }

    fn build(&self) -> &str {
        self.launcher.as_ref().map_or("", |l| l.build())
    }

    fn start(&mut self, delivered: bool) {
        let Some(launcher) = &self.launcher else { return self.simplify() };
        let process = match launcher.launch() {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("cannot start the WSL agent for {}: {e}", self.distro);
                return self.simplify();
            }
        };
        self.conns += 1;
        let ours = hello(self.build());
        let mut input = process.input;
        let _ = write_frame(&mut input, &ours).and_then(|()| input.flush().map_err(ProtoError::Io));
        let (id, tx, output) = (self.conns, self.tx.clone(), process.output);
        let reader = std::thread::Builder::new().name(format!("flit-wsl-read-{}", self.distro)).spawn(move || read_events(id, output, ours, tx));
        let conn = Conn { id, input, stop: process.stop };
        if let Err(e) = reader {
            tracing::warn!("cannot read from the WSL agent for {}: {e}", self.distro);
            conn.end(Duration::ZERO);
            return self.simplify();
        }
        self.state = State::Starting { conn, deadline: Instant::now() + self.timing.hello, delivered };
    }

    fn hello(&mut self, id: u64, theirs: Result<String, ProtoError>) {
        if !matches!(&self.state, State::Starting { conn, .. } if conn.id == id) {
            return;
        }
        let State::Starting { conn, delivered, .. } = std::mem::replace(&mut self.state, State::Idle) else { unreachable!() };
        let why = match theirs {
            Ok(build) if build == self.build() => {
                self.state = State::Ready(conn);
                self.busy_at = Instant::now();
                tracing::info!("WSL agent {build} is ready in {}", self.distro);
                return std::mem::take(&mut self.queue).into_iter().for_each(|r| self.request(r));
            }
            Ok(build) => format!("it is build {build:?}"),
            Err(e) => e.to_string(),
        };
        conn.end(Duration::ZERO);
        if delivered {
            tracing::warn!("WSL agent for {} still fails after delivery ({why})", self.distro);
            return self.simplify();
        }
        tracing::info!("delivering the WSL agent to {} ({why})", self.distro);
        match self.launcher.as_ref().map_or_else(|| Err(io::Error::other("no agent")), |l| l.deliver()) {
            Ok(()) => self.start(true),
            Err(e) => {
                tracing::warn!("cannot deliver the WSL agent to {}: {e}", self.distro);
                self.simplify();
            }
        }
    }

    fn simplify(&mut self) {
        tracing::warn!("{} uses \\\\wsl.localhost until Flit restarts", self.distro);
        self.state = State::Simplified;
        std::mem::take(&mut self.queue).into_iter().for_each(|r| self.fallback(r));
    }

    fn send(&mut self, req: Request) {
        let State::Ready(conn) = &mut self.state else { return };
        if let Err(e) = write_frame(&mut conn.input, &req).and_then(|()| conn.input.flush().map_err(ProtoError::Io)) {
            self.crashed(&e.to_string());
        }
    }

    fn id(&mut self) -> ReqId {
        self.next_id += 1;
        self.next_id
    }

    fn dispatch(&mut self, req: ProviderRequest) {
        self.busy_at = Instant::now();
        match req {
            ProviderRequest::List { gen, path, view, case, out, cancel, .. } => {
                if !cancel.is_current(gen) {
                    return;
                }
                let id = self.id();
                let now = Instant::now();
                let list = ListState { gen, path: path.clone(), view, out, cancel, listing: Listing::default(), keys: SortKeys::new(case), start: now, next_partial: now };
                self.pending.insert(id, Pending::List(Box::new(list)));
                self.send(Request::List { id, path });
            }
            ProviderRequest::Watch { tab, gen, path: Some(path) } => {
                let id = self.id();
                if let Some((_, old)) = self.watches.insert(tab, (gen, id)) {
                    self.send(Request::Cancel { id: old });
                }
                self.send(Request::Watch { id, path });
            }
            ProviderRequest::Watch { tab, path: None, .. } => {
                if let Some((_, old)) = self.watches.remove(&tab) {
                    self.send(Request::Cancel { id: old });
                }
            }
            ProviderRequest::DirSizes { tab, gen, dir, items, cancel } => {
                if !cancel.is_current(gen) || items.is_empty() {
                    return;
                }
                let id = self.id();
                self.pending.insert(id, Pending::Sizes { tab, gen, cancel, wanted: items.iter().map(|(i, _)| *i).collect() });
                self.send(Request::DirSizes { id, dir, items });
            }
            ProviderRequest::Op { tab, id: op, op: FileOp::Trash(paths), .. } => {
                let id = self.id();
                self.pending.insert(id, Pending::Op { tab, op, request: FileOp::Trash(paths.clone()) });
                self.send(Request::Op { id, op: FileOp::Trash(paths) });
            }
            other => self.fallback(other),
        }
    }

    fn cancel(&mut self, id: ReqId) {
        self.pending.remove(&id);
        self.send(Request::Cancel { id });
    }

    fn watcher(&self, id: ReqId) -> Option<(TabId, u64)> {
        self.watches.iter().find(|(_, (_, w))| *w == id).map(|(&tab, &(gen, _))| (tab, gen))
    }

    fn event(&mut self, ev: Event) {
        match ev {
            Event::Entries { id, batch } => {
                let Some(Pending::List(l)) = self.pending.get_mut(&id) else { return };
                if !l.cancel.is_current(l.gen) {
                    return self.cancel(id);
                }
                for (name, meta) in batch.entries(false).into_iter().flatten() {
                    let i = l.listing.push(name, meta);
                    l.keys.push(l.listing.name(i));
                }
                l.publish(&self.wake);
            }
            Event::ListDone { id, error } => {
                if let Some(Pending::List(l)) = self.pending.remove(&id) {
                    l.finish(error, &self.wake);
                }
            }
            Event::Changes { id, changes } => {
                if let Some((tab, gen)) = self.watcher(id) {
                    self.emit(ProviderEvent::Changes { tab, gen, changes });
                }
            }
            Event::Rescan { id } => {
                if let Some((tab, gen)) = self.watcher(id) {
                    self.emit(ProviderEvent::Rescan { tab, gen });
                }
            }
            Event::DirSize { id, idx, size } => {
                let Some(Pending::Sizes { tab, gen, cancel, wanted }) = self.pending.get_mut(&id) else { return };
                if !cancel.is_current(*gen) {
                    return self.cancel(id);
                }
                let (tab, gen, fresh) = (*tab, *gen, wanted.remove(&idx));
                if wanted.is_empty() {
                    self.pending.remove(&id);
                }
                if fresh {
                    self.emit(ProviderEvent::DirSize { tab, gen, idx, size });
                }
            }
            Event::OpDone { id, error } => {
                if let Some(Pending::Op { tab, op, request }) = self.pending.remove(&id) {
                    self.emit(ProviderEvent::OpDone { tab, id: op, error, report: Some(OpReport { op: request, items: Vec::new() }) });
                }
            }
            Event::Stat { .. } => {}
        }
    }

    fn crashed(&mut self, why: &str) {
        tracing::warn!("WSL agent for {} stopped: {why}", self.distro);
        if let State::Ready(conn) = std::mem::replace(&mut self.state, State::Idle) {
            conn.end(Duration::ZERO);
        }
        let lost = format!("The connection to {} was lost", self.distro);
        for (_, p) in std::mem::take(&mut self.pending) {
            match p {
                Pending::List(l) => l.finish(Some(FsError { kind: FsErrorKind::Unavailable, code: 0 }), &self.wake),
                Pending::Op { tab, op, .. } => self.emit(ProviderEvent::OpDone { tab, id: op, error: Some(lost.clone()), report: None }),
                Pending::Sizes { .. } => {}
            }
        }
        for (tab, _) in std::mem::take(&mut self.watches) {
            self.emit(ProviderEvent::OpDone { tab, id: 0, error: Some(format!("{lost}: this folder no longer updates. Press F5 to reload it.")), report: None });
        }
        let now = Instant::now();
        self.crashes.push_back(now);
        while self.crashes.front().is_some_and(|&t| now.duration_since(t) > self.timing.crash_window) {
            self.crashes.pop_front();
        }
        if self.crashes.len() >= self.timing.max_crashes {
            self.simplify();
        }
    }
}

#[cfg(test)]
#[path = "../test/wsl.rs"]
mod tests;
