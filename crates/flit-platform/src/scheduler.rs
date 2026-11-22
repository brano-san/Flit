use crossbeam_channel::{unbounded, Receiver, Sender};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const PER_SERVER: usize = 2;
const SLOW: Duration = Duration::from_secs(1);
const STALLED: Duration = Duration::from_secs(15);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    Background,
    Normal,
    Visible,
}

pub type ServerKey = Vec<u8>;
#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) enum LaneKey {
    Server(ServerKey),
    Volume(Vec<u8>),
}

#[derive(Clone, Default)]
pub(crate) struct Progress(Arc<Mutex<Option<Instant>>>);

impl Progress {
    pub fn record(&self) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
    }

    fn latest(&self, since: Instant) -> Instant {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).unwrap_or(since)
    }
}

type Work = Box<dyn FnOnce() + Send>;
type SlowTask = Box<dyn Fn(&str, Duration) + Send>;

struct Task {
    priority: Priority,
    label: &'static str,
    key: LaneKey,
    limit: usize,
    progress: Progress,
    work: Work,
}

enum Message {
    Submit(Task),
    Finished(u64),
    Shutdown,
}

#[derive(Default)]
struct Lane {
    waiting: [Vec<(u64, Task)>; 3],
    running: usize,
    limit: usize,
}

impl Lane {
    fn latest(&self) -> Option<(usize, u64)> {
        self.waiting.iter().enumerate().rev().find_map(|(priority, queue)| queue.last().map(|(seq, _)| (priority, *seq)))
    }

    fn empty(&self) -> bool {
        self.running == 0 && self.waiting.iter().all(Vec::is_empty)
    }
}

struct Running {
    key: LaneKey,
    label: &'static str,
    since: Instant,
    progress: Progress,
    logged: bool,
    retiring: bool,
}

struct Worker {
    tasks: Sender<Work>,
    running: Option<Running>,
}

struct Pool {
    target: usize,
    slow: Duration,
    stalled: Duration,
    report: SlowTask,
    messages: Sender<Message>,
    sequence: u64,
    lanes: HashMap<LaneKey, Lane>,
    worker_id: u64,
    workers: HashMap<u64, Worker>,
}

impl Pool {
    fn submit(&mut self, task: Task) {
        self.sequence += 1;
        let lane = self.lanes.entry(task.key.clone()).or_default();
        lane.limit = if lane.empty() { task.limit } else { lane.limit.min(task.limit) };
        lane.waiting[task.priority as usize].push((self.sequence, task));
    }

    fn completed(&mut self, id: u64) {
        let Some(running) = self.workers.get_mut(&id).and_then(|w| w.running.take()) else { return };
        if running.retiring {
            self.workers.remove(&id);
        }
        if let Some(lane) = self.lanes.get_mut(&running.key) {
            lane.running -= 1;
            if lane.empty() {
                self.lanes.remove(&running.key);
            }
        }
    }

    fn new_worker(&mut self) -> std::io::Result<u64> {
        self.worker_id += 1;
        let id = self.worker_id;
        let (tasks, inbox) = unbounded::<Work>();
        let messages = self.messages.clone();
        std::thread::Builder::new().name(format!("flit-background-{id}")).spawn(move || {
            for work in inbox {
                if std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)).is_err() {
                    tracing::warn!("background worker {id} panicked");
                }
                if messages.send(Message::Finished(id)).is_err() {
                    break;
                }
            }
        })?;
        self.workers.insert(id, Worker { tasks, running: None });
        Ok(id)
    }

    fn dispatch(&mut self) {
        loop {
            let healthy = self.workers.values().filter(|w| w.running.as_ref().is_some_and(|r| !r.retiring)).count();
            if healthy >= self.target {
                return;
            }
            let next = self.lanes.iter()
                .filter(|(_, lane)| lane.running < lane.limit)
                .filter_map(|(key, lane)| lane.latest().map(|(priority, seq)| (priority, seq, key)))
                .max_by_key(|&(priority, seq, _)| (priority, seq))
                .map(|(priority, _, key)| (priority, key.clone()));
            let Some((priority, key)) = next else { return };
            let idle = self.workers.iter().find_map(|(&id, w)| w.running.is_none().then_some(id));
            let id = if let Some(id) = idle {
                id
            } else if self.workers.len() < self.target.saturating_mul(2) {
                match self.new_worker() {
                    Ok(id) => id,
                    Err(error) => {
                        tracing::warn!("cannot start a background worker: {error}");
                        return;
                    }
                }
            } else {
                return;
            };
            let lane = self.lanes.get_mut(&key).unwrap();
            let (_, task) = lane.waiting[priority].pop().unwrap();
            lane.running += 1;
            let worker = self.workers.get_mut(&id).unwrap();
            worker.running = Some(Running { key, label: task.label, since: Instant::now(), progress: task.progress, logged: false, retiring: false });
            if worker.tasks.send(task.work).is_err() {
                self.completed(id);
                self.workers.remove(&id);
            }
        }
    }

    fn check_slow(&mut self) {
        let now = Instant::now();
        for work in self.workers.values_mut().filter_map(|w| w.running.as_mut()).filter(|r| !r.retiring) {
            let elapsed = now.saturating_duration_since(work.since);
            if !work.logged && elapsed >= self.slow {
                work.logged = true;
                (self.report)(work.label, elapsed);
            }
            if now.saturating_duration_since(work.progress.latest(work.since)) >= self.stalled {
                work.retiring = true;
            }
        }
    }

    fn next_check(&self) -> Option<Instant> {
        self.workers.values().filter_map(|w| w.running.as_ref()).filter(|r| !r.retiring).map(|r| {
            let stall = r.progress.latest(r.since) + self.stalled;
            if r.logged { stall } else { stall.min(r.since + self.slow) }
        }).min()
    }

    fn receive(&mut self, message: Message) -> bool {
        match message {
            Message::Submit(task) => self.submit(task),
            Message::Finished(id) => self.completed(id),
            Message::Shutdown => return false,
        }
        true
    }

    fn run(mut self, inbox: Receiver<Message>) {
        loop {
            for message in inbox.try_iter() {
                if !self.receive(message) {
                    return;
                }
            }
            self.check_slow();
            self.dispatch();
            let message = match self.next_check() {
                Some(due) => inbox.recv_deadline(due).ok(),
                None => inbox.recv().ok(),
            };
            if message.is_some_and(|message| !self.receive(message)) {
                return;
            }
        }
    }
}

pub struct Scheduler(Sender<Message>);

impl Scheduler {
    pub fn new(workers: usize) -> std::io::Result<Self> {
        Self::start(workers, SLOW, STALLED, Box::new(|label, elapsed| tracing::warn!(task = label, ?elapsed, "slow background task")))
    }

    fn start(workers: usize, slow: Duration, stalled: Duration, report: SlowTask) -> std::io::Result<Self> {
        let (messages, inbox) = unbounded();
        let pool = Pool { target: workers.max(1), slow, stalled, report, messages: messages.clone(), sequence: 0, lanes: HashMap::new(), worker_id: 0, workers: HashMap::new() };
        std::thread::Builder::new().name("flit-background".into()).spawn(move || pool.run(inbox))?;
        Ok(Self(messages))
    }

    pub(crate) fn schedule(&self, priority: Priority, key: LaneKey, limit: usize, label: &'static str, work: impl FnOnce(Progress) + Send + 'static) {
        let limit = if matches!(key, LaneKey::Server(_)) { PER_SERVER.min(limit) } else { limit };
        let progress = Progress::default();
        let run_progress = progress.clone();
        let work = Box::new(move || work(run_progress));
        let _ = self.0.send(Message::Submit(Task { priority, label, key, limit, progress, work }));
    }
}

impl Drop for Scheduler {
    fn drop(&mut self) {
        let _ = self.0.send(Message::Shutdown);
    }
}

#[cfg(test)]
#[path = "../test/scheduler.rs"]
mod tests;
