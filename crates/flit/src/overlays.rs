use crossbeam_channel::{unbounded, Receiver, Sender};
use flit_core::ui::{OverlayBatch, OverlayReply};
use flit_platform::{Shell, WindowId};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

pub(crate) const HUNG: Duration = Duration::from_secs(3);
pub(crate) const BUDGET: Duration = Duration::from_millis(400);
pub(crate) const MAX_HANGS: u32 = 2;

pub(crate) type Shown = Arc<Mutex<Vec<u64>>>;
pub(crate) type Deliver = Arc<dyn Fn(WindowId, OverlayReply) + Send + Sync>;

pub(crate) struct Job {
    pub window: WindowId,
    pub batch: OverlayBatch,
    pub shown: Shown,
}

struct Busy {
    worker: u64,
    since: Instant,
    window: WindowId,
    reply: OverlayReply,
    left: Vec<u64>,
}

struct Gate {
    busy: Mutex<Option<Busy>>,
    seen: Condvar,
    ids: AtomicU64,
    hangs: AtomicU32,
    hung: Duration,
    budget: Duration,
}

pub(crate) struct Overlays {
    tx: Sender<Job>,
}

fn locked<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Overlays {
    pub(crate) fn start<S: Shell + 'static>(deliver: Deliver) -> Overlays {
        if !S::ICON_OVERLAYS {
            return Overlays { tx: unbounded().0 };
        }
        Self::with_limits::<S>(deliver, HUNG, BUDGET)
    }

    pub(crate) fn with_limits<S: Shell + 'static>(deliver: Deliver, hung: Duration, budget: Duration) -> Overlays {
        let (tx, rx) = unbounded::<Job>();
        let gate = Arc::new(Gate { busy: Mutex::new(None), seen: Condvar::new(), ids: AtomicU64::new(1), hangs: AtomicU32::new(0), hung, budget });
        spawn_worker::<S>(1, rx.clone(), gate.clone(), deliver.clone());
        let watched = gate.clone();
        let spawned = std::thread::Builder::new().name("flit-overlay-watch".into()).spawn(move || watch::<S>(&watched, &rx, &deliver));
        if let Err(e) = spawned {
            tracing::warn!("icon overlay watchdog: {e}");
        }
        Overlays { tx }
    }

    pub(crate) fn send(&self, job: Job) {
        let _ = self.tx.send(job);
    }
}

fn spawn_worker<S: Shell + 'static>(id: u64, rx: Receiver<Job>, gate: Arc<Gate>, deliver: Deliver) {
    let spawned = std::thread::Builder::new().name(format!("flit-overlay-{id}")).spawn(move || {
        let mut shell = if gate.hangs.load(Ordering::SeqCst) < MAX_HANGS { S::new().ok() } else { None };
        for job in rx {
            if !serve(&mut shell, id, &gate, &deliver, job) {
                return;
            }
        }
    });
    if let Err(e) = spawned {
        tracing::warn!("icon overlay thread: {e}");
    }
}

fn serve<S: Shell>(shell: &mut Option<S>, me: u64, gate: &Gate, deliver: &Deliver, job: Job) -> bool {
    let Job { window, batch, shown } = job;
    let started = Instant::now();
    let mut reply = OverlayReply { id: batch.id, size: batch.size, ..OverlayReply::default() };
    let keys: Vec<u64> = batch.items.iter().map(|i| i.key).collect();
    for (k, item) in batch.items.iter().enumerate() {
        if !locked(&shown).contains(&item.key) || started.elapsed() >= gate.budget {
            reply.skipped.push(item.key);
            continue;
        }
        let Some(sh) = shell.as_mut() else {
            reply.found.push((item.key, None));
            continue;
        };
        *locked(&gate.busy) = Some(Busy { worker: me, since: Instant::now(), window, reply: reply.clone(), left: keys[k..].to_vec() });
        gate.seen.notify_all();
        let slot = sh.overlay_slot(&item.path);
        let mut busy = locked(&gate.busy);
        if busy.as_ref().is_none_or(|b| b.worker != me) {
            return false;
        }
        *busy = None;
        drop(busy);
        reply.found.push((item.key, slot.filter(|s| flit_core::ui::OVERLAY_SLOTS.contains(s))));
    }
    if let Some(sh) = shell.as_mut() {
        let mut slots: Vec<u8> = reply.found.iter().filter_map(|f| f.1).chain(batch.fetch.iter().copied()).filter(|s| !batch.have.contains(s)).collect();
        slots.sort_unstable();
        slots.dedup();
        reply.images = slots.into_iter().filter_map(|s| sh.overlay_image(s, batch.size).map(|b| (s, b))).collect();
    }
    deliver(window, reply);
    true
}

fn watch<S: Shell + 'static>(gate: &Arc<Gate>, rx: &Receiver<Job>, deliver: &Deliver) {
    loop {
        let mut busy = locked(&gate.busy);
        let Some(since) = busy.as_ref().map(|b| b.since) else {
            drop(gate.seen.wait(busy).unwrap_or_else(|e| e.into_inner()));
            continue;
        };
        let left = gate.hung.saturating_sub(since.elapsed());
        if !left.is_zero() {
            drop(gate.seen.wait_timeout(busy, left).unwrap_or_else(|e| e.into_inner()));
            continue;
        }
        let Some(stuck) = busy.take() else { continue };
        drop(busy);
        let hangs = gate.hangs.fetch_add(1, Ordering::SeqCst) + 1;
        tracing::warn!("a shell icon overlay handler stopped answering for {} ms ({hangs} so far), so overlays move to a new thread", stuck.since.elapsed().as_millis());
        let mut reply = stuck.reply;
        reply.found.extend(stuck.left.into_iter().map(|key| (key, None)));
        deliver(stuck.window, reply);
        let id = gate.ids.fetch_add(1, Ordering::SeqCst) + 1;
        spawn_worker::<S>(id, rx.clone(), gate.clone(), deliver.clone());
    }
}

#[cfg(test)]
#[path = "../test/overlays.rs"]
mod tests;
