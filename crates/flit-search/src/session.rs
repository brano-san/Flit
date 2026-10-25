use crate::engine::{Engine, ResultSet};
use crate::proto::{Message, Sort, VolumeInfo};
use crate::query::{self, Clock, Query};
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

    pub fn set_volume(&self, info: VolumeInfo) {
        let mut st = self.status.lock().unwrap_or_else(|e| e.into_inner());
        match st.1.iter_mut().find(|v| v.id == info.id) {
            Some(v) if *v == info => return,
            Some(v) => *v = info,
            None => st.1.push(info),
        }
        st.0 += 1;
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
}

pub struct Session {
    shared: Arc<SharedIndex>,
    utc_offset_min: i32,
    current: Option<Current>,
    status_seen: u64,
    refreshed: Instant,
}

impl Session {
    pub fn new(shared: Arc<SharedIndex>, utc_offset_min: i32) -> Self {
        Self { shared, utc_offset_min, current: None, status_seen: 0, refreshed: Instant::now() }
    }

    pub fn handle(&mut self, msg: Message, latest: &AtomicU64, out: &mut dyn FnMut(Message)) {
        match msg {
            Message::Query { gen, text, sort } => {
                if latest.load(Ordering::Acquire) != gen {
                    return;
                }
                let query = query::parse(&text, Clock::system(self.utc_offset_min));
                if let Err(message) = &query {
                    out(Message::Error { message: message.clone() });
                }
                self.current = Some(Current { gen, query, sort, version: 0, set: None });
                let shared = self.shared.clone();
                let engine = shared.read();
                if self.run(&engine, latest, out) {
                    self.send_rows(&engine, gen, 0, FIRST_PAGE, out);
                }
            }
            Message::GetRows { gen, from, to } => {
                if self.current.as_ref().is_none_or(|c| c.gen != gen) {
                    return;
                }
                let shared = self.shared.clone();
                let engine = shared.read();
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
        let Some((gen, version)) = self.current.as_ref().map(|c| (c.gen, c.version)) else { return };
        if self.refreshed.elapsed() < REFRESH_EVERY || latest.load(Ordering::Acquire) != gen {
            return;
        }
        let shared = self.shared.clone();
        let engine = shared.read();
        if version != engine.version {
            if self.run(&engine, latest, out) {
                self.send_rows(&engine, gen, 0, FIRST_PAGE, out);
            }
        }
    }

    fn run(&mut self, engine: &Engine, latest: &AtomicU64, out: &mut dyn FnMut(Message)) -> bool {
        let Some(c) = &mut self.current else { return false };
        let gen = c.gen;
        self.refreshed = Instant::now();
        c.version = engine.version;
        let cancel = || latest.load(Ordering::Acquire) != gen;
        c.set = match &c.query {
            Ok(q) => engine.search(q, c.sort, &cancel),
            Err(_) => None,
        };
        match &c.set {
            Some(set) => out(Message::QueryDone { gen, total: set.total }),
            None if c.query.is_err() => out(Message::QueryDone { gen, total: 0 }),
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
