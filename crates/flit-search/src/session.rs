use crate::engine::{order_of, Engine, ResultSet, Visibility};
use crate::proto::{Message, Sort, VolumeInfo};
use crate::query::{self, Clock, Query};
use flit_core::query_attrs::{Facets, FolderCounts};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::{Duration, Instant};

pub const FIRST_PAGE: u64 = 64;
const REFRESH_EVERY: Duration = Duration::from_secs(1);

#[derive(Default)]
pub struct SharedIndex {
    engine: RwLock<Engine>,
    status: Mutex<(u64, Vec<VolumeInfo>)>,
}

impl SharedIndex {
    pub fn new() -> Arc<Self> {
        Arc::default()
    }

    pub fn read(&self) -> RwLockReadGuard<'_, Engine> {
        self.engine.read().unwrap_or_else(|e| e.into_inner())
    }

    pub fn write(&self) -> RwLockWriteGuard<'_, Engine> {
        self.engine.write().unwrap_or_else(|e| e.into_inner())
    }

    pub fn read_sorted(&self, sort: Sort) -> RwLockReadGuard<'_, Engine> {
        let order = order_of(sort.key);
        loop {
            let engine = self.read();
            if engine.has_order(order) {
                return engine;
            }
            drop(engine);
            self.write().build_order(order);
        }
    }

    pub fn set_volume(&self, info: VolumeInfo) {
        let mut st = self.status.lock().unwrap_or_else(|e| e.into_inner());
        match st.1.iter_mut().find(|v| v.id == info.id) {
            Some(v) if *v == info => return,
            Some(v) => *v = info,
            None => st.1.push(info),
        }
        st.0 += 1;
    }

    pub fn remove_volume(&self, id: &str) {
        {
            let mut e = self.write();
            let before = e.volumes.len();
            e.volumes.retain(|v| v.id != id);
            e.version += (e.volumes.len() != before) as u64;
        }
        let mut st = self.status.lock().unwrap_or_else(|e| e.into_inner());
        let before = st.1.len();
        st.1.retain(|v| v.id != id);
        st.0 += (st.1.len() != before) as u64;
    }

    pub fn status(&self) -> (u64, Vec<VolumeInfo>) {
        self.status.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

struct Current {
    gen: u64,
    query: Result<Query, String>,
    sort: Sort,
    version: u64,
    set: Option<ResultSet>,
    facets: Option<Facets>,
    folders: Option<FolderCounts>,
}

pub struct Session {
    shared: Arc<SharedIndex>,
    utc_offset_min: i32,
    facets_open: Arc<AtomicU64>,
    current: Option<Box<Current>>,
    access: Option<Arc<dyn Visibility>>,
    status_seen: u64,
    refreshed: Instant,
}

impl Session {
    pub fn new(shared: Arc<SharedIndex>, utc_offset_min: i32) -> Self {
        Self { shared, utc_offset_min, facets_open: Arc::default(), current: None, access: None, status_seen: 0, refreshed: Instant::now() }
    }

    pub fn with_facets(mut self, facets_open: Arc<AtomicU64>) -> Self {
        self.facets_open = facets_open;
        self
    }

    pub fn with_access(mut self, access: Option<Arc<dyn Visibility>>) -> Self {
        self.access = access;
        self
    }

    pub fn handle(&mut self, msg: Message, latest: &AtomicU64, out: &mut dyn FnMut(Message)) {
        match msg {
            Message::Query { gen, text, conditions, sort, paths } => {
                if latest.load(Ordering::Acquire) != gen {
                    return;
                }
                let query = query::parse_with_conditions(&text, &conditions, Clock::system(self.utc_offset_min)).map(|mut q| {
                    q.paths = paths;
                    q
                }).map_err(|error| error.to_string());
                if let Err(message) = &query {
                    out(Message::Error { message: message.clone() });
                }
                self.current = Some(Box::new(Current { gen, query, sort, version: 0, set: None, facets: None, folders: None }));
                let shared = self.shared.clone();
                let engine = shared.read_sorted(sort);
                if self.run(&engine, latest, out) {
                    self.send_rows(&engine, gen, 0, FIRST_PAGE, out);
                }
            }
            Message::GetFacets { gen, open: true } => {
                let facets_open = &self.facets_open;
                let cancel = || latest.load(Ordering::Acquire) != gen || facets_open.load(Ordering::Acquire) != gen;
                if cancel() {
                    return;
                }
                let shared = self.shared.clone();
                let engine = shared.read();
                if let Some(c) = self.current.as_mut().filter(|c| c.gen == gen && c.version == engine.version) {
                    if c.facets.is_none() {
                        c.facets = c.set.as_ref().and_then(|set| engine.result_facets(set, self.utc_offset_min, &cancel));
                    }
                    if let Some(counts) = c.facets.as_ref().filter(|_| !cancel()) {
                        out(Message::Facets { gen, counts: counts.clone() });
                    }
                }
            }
            Message::GetFolders { gen } => {
                let cancel = || latest.load(Ordering::Acquire) != gen;
                if cancel() {
                    return;
                }
                let shared = self.shared.clone();
                let engine = shared.read();
                if let Some(c) = self.current.as_mut().filter(|c| c.gen == gen && c.version == engine.version) {
                    if c.folders.is_none() {
                        c.folders = c.set.as_ref().and_then(|set| engine.result_folders(set, &cancel));
                    }
                    if let Some(counts) = c.folders.as_ref().filter(|_| !cancel()) {
                        out(Message::Folders { gen, counts: counts.clone() });
                    }
                }
            }
            Message::GetRows { gen, from, to } => {
                let Some(sort) = self.current.as_ref().filter(|c| c.gen == gen).map(|c| c.sort) else { return };
                let shared = self.shared.clone();
                let engine = shared.read_sorted(sort);
                let stale = self.current.as_ref().is_some_and(|c| c.version != engine.version || c.set.is_none());
                if !stale || self.run(&engine, latest, out) {
                    self.send_rows(&engine, gen, from, to, out);
                }
            }
            _ => {}
        }
    }

    pub fn tick(&mut self, latest: &AtomicU64, out: &mut dyn FnMut(Message)) {
        let (version, volumes) = self.shared.status();
        if version != self.status_seen {
            self.status_seen = version;
            out(Message::IndexStatus { volumes });
        }
        let Some((gen, version, sort)) = self.current.as_ref().map(|c| (c.gen, c.version, c.sort)) else { return };
        if self.refreshed.elapsed() < REFRESH_EVERY || latest.load(Ordering::Acquire) != gen {
            return;
        }
        let shared = self.shared.clone();
        let engine = shared.read_sorted(sort);
        if version != engine.version && self.run(&engine, latest, out) {
            self.send_rows(&engine, gen, 0, FIRST_PAGE, out);
        }
    }

    fn run(&mut self, engine: &Engine, latest: &AtomicU64, out: &mut dyn FnMut(Message)) -> bool {
        let Some(c) = &mut self.current else { return false };
        let gen = c.gen;
        self.refreshed = Instant::now();
        let cancel = || latest.load(Ordering::Acquire) != gen;
        let set = match &c.query {
            Ok(q) => engine.search_as(q, c.sort, self.access.as_deref(), &cancel),
            Err(_) => None,
        };
        if c.version != engine.version || c.set.as_ref().map(|s| s.total) != set.as_ref().map(|s| s.total) {
            c.facets = None;
            c.folders = None;
        }
        c.version = engine.version;
        c.set = set;
        match &mut c.set {
            Some(set) => {
                out(Message::QueryDone { gen, total: set.total, version: engine.version });
                if let Some(starts) = engine.groups(set) {
                    out(Message::Groups { gen, starts });
                }
            }
            None if c.query.is_err() => out(Message::QueryDone { gen, total: 0, version: engine.version }),
            None => return false,
        }
        c.set.is_some()
    }

    fn send_rows(&mut self, engine: &Engine, gen: u64, from: u64, to: u64, out: &mut dyn FnMut(Message)) {
        let Some(set) = self.current.as_mut().and_then(|c| c.set.as_mut()) else { return };
        let (entries, dirs) = engine.rows(set, from, to);
        if !entries.is_empty() {
            out(Message::Rows { gen, from, entries, dirs });
        }
    }
}

#[cfg(test)]
#[path = "../test/session.rs"]
mod tests;
