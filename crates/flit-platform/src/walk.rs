use crossbeam_channel::{unbounded, Receiver, RecvTimeoutError, Sender};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Media {
    Solid,
    Spinning,
    Network,
    Unknown,
}

const SLICE: Duration = Duration::from_millis(100);
const IDLE: Duration = Duration::from_millis(10);
const BUCKETS: usize = 256;
const CEILING: usize = 64;
const MODEST_CEILING: usize = 16;
const SPINNING_CEILING: usize = 4;
const BUFFER_BUDGET: usize = 4 << 20;
const SEEK_SECS: f64 = 0.001;
const GAIN: f64 = 0.08;
const LEAP: f64 = 0.25;
const RAMP_SLICE: Duration = Duration::from_millis(5);
const RAMP_LISTINGS: u64 = 8;
const CONTENTION: f64 = 3.0;
const CONTENDED_SLICES: u32 = 10;
const HOLD: u32 = 20;
const STEP: usize = 4;
const SMOOTHING: f64 = 0.1;
const PATTERN: [bool; 4] = [false, true, true, false];

#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    Settled {
        slices: u32,
    },
    Probing {
        candidate: usize,
        slice: usize,
        base: f64,
        probe: f64,
    },
}

#[derive(Clone, Debug)]
pub struct Throttle {
    base: usize,
    threads: usize,
    floor: usize,
    ceiling: usize,
    seek_ceiling: Option<usize>,
    slow_start: bool,
    upward: bool,
    phase: Phase,
    latency: f64,
    contended: u32,
    trail: Vec<usize>,
}

impl Throttle {
    pub fn new(media: Media, buffer: usize) -> Self {
        Self::with_cores(
            media,
            std::thread::available_parallelism().map_or(1, |n| n.get()),
            buffer,
        )
    }

    pub fn with_cores(media: Media, cores: usize, buffer: usize) -> Self {
        let ceiling = CEILING.min(BUFFER_BUDGET / buffer.max(1)).max(1);
        let (start, ceiling) = match media {
            Media::Solid => (2, ceiling),
            Media::Spinning | Media::Network => (1, ceiling.min(MODEST_CEILING)),
            Media::Unknown => (2, ceiling.min(MODEST_CEILING)),
        };
        let threads = start.min(cores.max(1)).min(ceiling);
        let seek_ceiling =
            (media == Media::Spinning).then(|| SPINNING_CEILING.min(cores.max(2)).min(ceiling));
        Self {
            base: threads,
            threads,
            floor: 1,
            ceiling,
            seek_ceiling,
            slow_start: true,
            upward: true,
            phase: Phase::Settled { slices: 0 },
            latency: 0.0,
            contended: 0,
            trail: Vec::new(),
        }
    }

    pub fn fixed(threads: usize) -> Self {
        let threads = threads.max(1);
        Self {
            base: threads,
            threads,
            floor: threads,
            ceiling: threads,
            ..Self::with_cores(Media::Solid, 1, 1)
        }
    }

    pub fn threads(&self) -> usize {
        self.threads
    }

    pub fn ramping(&self) -> bool {
        self.slow_start
    }

    pub fn trail(&self) -> &[usize] {
        &self.trail
    }

    pub fn observe(&mut self, rate: f64, latency: f64) -> usize {
        if self.floor == self.ceiling || rate <= 0.0 {
            return self.threads;
        }
        let cap = self
            .seek_ceiling
            .filter(|_| latency.max(self.latency) >= SEEK_SECS)
            .unwrap_or(self.ceiling);
        self.phase = match self.phase {
            Phase::Settled { slices } => self.settled(slices, latency, cap),
            Phase::Probing {
                candidate,
                slice,
                base,
                probe,
            } => {
                let (base, probe) = if PATTERN[slice] {
                    (base, probe + rate)
                } else {
                    (base + rate, probe)
                };
                if slice == 1 && candidate > self.base && probe > base * (1.0 + LEAP) {
                    self.leap(candidate, rate, cap)
                } else {
                    match PATTERN.get(slice + 1) {
                        Some(&next) => {
                            self.threads = if next { candidate } else { self.base };
                            Phase::Probing {
                                candidate,
                                slice: slice + 1,
                                base,
                                probe,
                            }
                        }
                        None => self.decide(candidate, base, probe, cap),
                    }
                }
            }
        };
        if self.base > cap {
            self.base = cap;
            self.phase = Phase::Settled { slices: 0 };
        }
        if matches!(self.phase, Phase::Settled { .. }) {
            self.threads = self.base;
        }
        self.threads = self.threads.clamp(self.floor, cap.max(self.floor));
        self.trail.push(self.threads);
        self.threads
    }

    fn settled(&mut self, slices: u32, latency: f64, cap: usize) -> Phase {
        if self.latency > 0.0 && latency > self.latency * CONTENTION {
            self.contended += 1;
            if self.contended < CONTENDED_SLICES {
                return Phase::Settled { slices };
            }
            self.contended = 0;
            self.base = (self.base / 2).max(self.floor);
            self.slow_start = false;
            self.upward = true;
            self.latency = latency;
            return Phase::Settled { slices: 0 };
        }
        self.contended = 0;
        self.latency = if self.latency == 0.0 {
            latency
        } else {
            self.latency + SMOOTHING * (latency - self.latency)
        };
        if self.slow_start || slices + 1 >= HOLD {
            return self.probe(cap);
        }
        Phase::Settled { slices: slices + 1 }
    }

    fn probe(&mut self, cap: usize) -> Phase {
        let step = if self.slow_start {
            self.base
        } else {
            (self.base / STEP).max(1)
        };
        let candidate = if self.upward {
            (self.base + step).min(cap)
        } else {
            self.base.saturating_sub(step).max(self.floor)
        };
        if candidate == self.base {
            self.slow_start = false;
            self.upward = !self.upward;
            return Phase::Settled { slices: 0 };
        }
        Phase::Probing {
            candidate,
            slice: 0,
            base: 0.0,
            probe: 0.0,
        }
    }

    fn leap(&mut self, candidate: usize, rate: f64, cap: usize) -> Phase {
        self.base = candidate;
        self.latency = 0.0;
        if !self.slow_start {
            return Phase::Settled { slices: 0 };
        }
        match self.probe(cap) {
            Phase::Probing { candidate, .. } => {
                self.threads = candidate;
                Phase::Probing {
                    candidate,
                    slice: 1,
                    base: rate,
                    probe: 0.0,
                }
            }
            settled => settled,
        }
    }

    fn decide(&mut self, candidate: usize, base: f64, probe: f64, cap: usize) -> Phase {
        let up = candidate > self.base;
        if (up && probe > base * (1.0 + GAIN)) || (!up && probe >= base * (1.0 - GAIN)) {
            self.base = candidate;
            self.latency = 0.0;
            if self.slow_start || !up {
                return self.probe(cap);
            }
        } else {
            self.slow_start = false;
            self.upward = !up;
        }
        Phase::Settled { slices: 0 }
    }
}

struct Gate {
    threads: AtomicUsize,
    over: AtomicBool,
    lock: Mutex<()>,
    wake: Condvar,
}

impl Gate {
    fn admit(&self, k: usize) -> bool {
        if self.over.load(Ordering::Acquire) {
            return false;
        }
        if k < self.threads.load(Ordering::Acquire) {
            return true;
        }
        let mut g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        while k >= self.threads.load(Ordering::Acquire) && !self.over.load(Ordering::Acquire) {
            g = self.wake.wait(g).unwrap_or_else(|e| e.into_inner());
        }
        !self.over.load(Ordering::Acquire)
    }

    fn set(&self, threads: usize) {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        self.threads.store(threads, Ordering::Release);
        self.wake.notify_all();
    }

    fn close(&self) {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        self.over.store(true, Ordering::Release);
        self.wake.notify_all();
    }
}

struct Pool<'l, J, L> {
    list: &'l L,
    gate: Gate,
    pending: AtomicUsize,
    spread: [AtomicU64; BUCKETS],
    jobs: Sender<J>,
    queue: Receiver<J>,
}

impl<J, L> Pool<'_, J, L> {
    fn work<B>(&self, k: usize, out: Sender<B>)
    where
        L: Fn(&J, &mut Vec<J>) -> B,
    {
        let mut subdirs = Vec::new();
        while self.gate.admit(k) {
            let Ok(job) = self.queue.recv_timeout(IDLE) else {
                continue;
            };
            let started = Instant::now();
            let batch = (self.list)(&job, &mut subdirs);
            self.spread[bucket(started.elapsed().as_nanos() as u64)]
                .fetch_add(1, Ordering::Relaxed);
            self.pending.fetch_add(subdirs.len(), Ordering::AcqRel);
            for sub in subdirs.drain(..) {
                let _ = self.jobs.send(sub);
            }
            let _ = out.send(batch);
            if self.pending.fetch_sub(1, Ordering::AcqRel) == 1 {
                self.gate.close();
            }
        }
    }

    fn spread(&self) -> Vec<u64> {
        self.spread
            .iter()
            .map(|n| n.load(Ordering::Relaxed))
            .collect()
    }
}

fn bucket(nanos: u64) -> usize {
    let top = 63 - (nanos | 8).leading_zeros() as usize;
    ((top << 3) | ((nanos >> (top - 3)) & 7) as usize).min(BUCKETS - 1)
}

fn bucket_nanos(b: usize) -> f64 {
    let top = b >> 3;
    ((8 | (b & 7)) as f64 + 0.5) * (1u64 << (top - 3)) as f64
}

fn median_secs(spread: &[u64]) -> f64 {
    let half = spread.iter().sum::<u64>().div_ceil(2);
    let mut seen = 0;
    spread
        .iter()
        .position(|&n| {
            seen += n;
            half > 0 && seen >= half
        })
        .map_or(0.0, |b| bucket_nanos(b) / 1e9)
}

pub fn walk<J, B, L, T>(root: J, mut throttle: Throttle, list: L, mut take: T) -> Throttle
where
    J: Send,
    B: Send,
    L: Fn(&J, &mut Vec<J>) -> B + Sync,
    T: FnMut(B, u64, u64) -> bool,
{
    let (jobs, queue) = unbounded::<J>();
    let (out_tx, out_rx) = unbounded::<B>();
    let gate = Gate {
        threads: AtomicUsize::new(throttle.threads()),
        over: AtomicBool::new(false),
        lock: Mutex::new(()),
        wake: Condvar::new(),
    };
    let pool = Pool {
        list: &list,
        gate,
        pending: AtomicUsize::new(1),
        spread: std::array::from_fn(|_| AtomicU64::new(0)),
        jobs,
        queue,
    };
    let _ = pool.jobs.send(root);
    std::thread::scope(|s| {
        let pool = &pool;
        let mut spawned = 0;
        let mut grow = |upto: usize, out: &Sender<B>| {
            while spawned < upto {
                let (k, out) = (spawned, out.clone());
                if std::thread::Builder::new()
                    .name(format!("flit-walk-{k}"))
                    .spawn_scoped(s, move || pool.work(k, out))
                    .is_err()
                {
                    break;
                }
                spawned += 1;
            }
        };
        grow(throttle.threads(), &out_tx);
        let mut out = Some(out_tx);
        let (mut since, mut before, mut counted) = (Instant::now(), pool.spread(), 0u64);
        let mut done = 0u64;
        loop {
            let quick = throttle.ramping() && done - counted >= RAMP_LISTINGS;
            let due = since + if quick { RAMP_SLICE } else { SLICE };
            match out_rx.recv_timeout(due.saturating_duration_since(Instant::now())) {
                Ok(batch) => {
                    done += 1;
                    if !take(
                        batch,
                        done,
                        done + pool.pending.load(Ordering::Relaxed) as u64,
                    ) {
                        pool.gate.close();
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            if pool.gate.over.load(Ordering::Acquire) {
                out = None;
            }
            let Some(sender) = &out else { continue };
            if Instant::now() < due {
                continue;
            }
            let now = pool.spread();
            let slice: Vec<u64> = now.iter().zip(&before).map(|(a, b)| a - b).collect();
            let rate = slice.iter().sum::<u64>() as f64 / since.elapsed().as_secs_f64();
            let threads = throttle.observe(rate, median_secs(&slice));
            (since, before, counted) = (Instant::now(), now, done);
            grow(threads, sender);
            pool.gate.set(threads);
        }
    });
    throttle
}

#[cfg(test)]
#[path = "../test/walk.rs"]
mod tests;
