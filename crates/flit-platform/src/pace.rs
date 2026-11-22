use crate::Job;
use std::time::{Duration, Instant};

const SLICE: Duration = Duration::from_millis(100);
const MEGABYTE: u64 = 1_000_000;

#[derive(Default)]
pub(crate) struct Bucket {
    last: Option<Instant>,
    rate: f64,
    tokens: f64,
}

impl Bucket {
    fn charge(&mut self, now: Instant, limit: u32, bytes: u64) -> Duration {
        if limit == 0 {
            *self = Self::default();
            return Duration::ZERO;
        }
        let rate = (u64::from(limit) * MEGABYTE) as f64;
        let capacity = rate * SLICE.as_secs_f64();
        let available = self.last.map_or(capacity, |last| self.tokens + now.duration_since(last).as_secs_f64() * self.rate);
        self.last = Some(now);
        self.rate = rate;
        self.tokens = available.min(capacity) - bytes as f64;
        Duration::from_secs_f64((-self.tokens / rate).max(0.0))
    }
}

impl Job {
    pub fn copy_chunk_size(&self, maximum: usize) -> usize {
        let limit = self.ctl().copy_limit();
        if limit == 0 {
            return maximum;
        }
        maximum.min(usize::try_from(u64::from(limit) * (MEGABYTE / 10)).unwrap_or(usize::MAX))
    }

    pub fn pace_copy(&self, mut bytes: u64) -> bool {
        if bytes == 0 || self.ctl().copy_limit() == 0 {
            return !self.cancelled();
        }
        self.ctl().wait_while_paused();
        while !self.cancelled() {
            let delay = {
                let mut bucket = self.0.copy_bucket.lock().unwrap_or_else(|e| e.into_inner());
                bucket.charge(Instant::now(), self.ctl().copy_limit(), bytes)
            };
            if delay.is_zero() {
                return true;
            }
            bytes = 0;
            std::thread::sleep(delay.min(SLICE));
            self.ctl().wait_while_paused();
        }
        false
    }
}

#[cfg(test)]
#[path = "../test/pace.rs"]
mod tests;
