use flit_core::input::{Event, Key, Mods};
use flit_core::ui::App;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const SETTLE_FRAMES: u32 = 30;
const SCROLL_FRAMES: u32 = 240;

pub enum Step {
    None,
    Inject(Event),
    Finish,
}

enum Phase {
    Loading,
    Settle(u32),
    Scroll(u32),
    Sort,
    Done,
}

pub struct Metrics {
    out: PathBuf,
    started: Instant,
    phase: Phase,
    first_frame: Option<f64>,
    first_rows: Option<f64>,
    full: Option<f64>,
    entries: usize,
    frame_ends: Vec<Instant>,
    cpu: Vec<f32>,
    latencies: Vec<f64>,
    pending_input: Option<Instant>,
    sort: Option<f64>,
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

fn percentile(v: &mut [f64], p: f64) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(f64::total_cmp);
    v[((v.len() - 1) as f64 * p).round() as usize]
}

impl Metrics {
    pub fn new(out: PathBuf, started: Instant) -> Self {
        Self {
            out,
            started,
            phase: Phase::Loading,
            first_frame: None,
            first_rows: None,
            full: None,
            entries: 0,
            frame_ends: Vec::new(),
            cpu: Vec::new(),
            latencies: Vec::new(),
            pending_input: None,
            sort: None,
        }
    }

    pub fn on_sorted(&mut self, sort_ms: f64) {
        if matches!(self.phase, Phase::Sort) {
            self.sort = Some(sort_ms);
            self.phase = Phase::Done;
        }
    }

    pub fn after_frame(&mut self, app: &App, cpu_ms: f32, now: Instant) -> Step {
        self.first_frame.get_or_insert_with(|| ms(now - self.started));
        if let Some(t) = self.pending_input.take() {
            self.latencies.push(ms(now - t));
        }
        match self.phase {
            Phase::Loading => {
                let s = app.tab().stats;
                if let Some(done) = s.done {
                    self.first_rows = s.first.map(ms);
                    self.full = Some(ms(done));
                    self.entries = s.entries;
                    self.phase = Phase::Settle(SETTLE_FRAMES);
                }
                Step::None
            }
            Phase::Settle(0) => {
                self.phase = Phase::Scroll(SCROLL_FRAMES);
                Step::None
            }
            Phase::Settle(n) => {
                self.phase = Phase::Settle(n - 1);
                Step::None
            }
            Phase::Scroll(0) => {
                self.phase = Phase::Sort;
                Step::Inject(Event::Key { key: Key::Char('H'), mods: Mods { ctrl: true, ..Mods::default() }, pressed: true })
            }
            Phase::Scroll(n) => {
                self.frame_ends.push(now);
                self.cpu.push(cpu_ms);
                self.pending_input = Some(now);
                self.phase = Phase::Scroll(n - 1);
                Step::Inject(Event::Wheel { dx: 0.0, dy: -1.0 })
            }
            Phase::Sort => Step::None,
            Phase::Done => {
                self.write();
                Step::Finish
            }
        }
    }

    fn write(&self) {
        let intervals: Vec<f64> = self.frame_ends.windows(2).map(|w| ms(w[1] - w[0])).collect();
        let period = percentile(&mut intervals.clone(), 0.5);
        let dropped = intervals.iter().filter(|&&i| i > period * 1.5).count();
        let mut cpu: Vec<f64> = self.cpu.iter().map(|&c| c as f64).collect();
        let mut lat = self.latencies.clone();
        let opt = |v: Option<f64>| v.map_or("null".to_string(), |x| format!("{x:.2}"));
        let mut s = String::new();
        let _ = write!(
            s,
            "{{\"entries\":{},\"start_to_first_frame_ms\":{},\"first_rows_ms\":{},\"full_listing_ms\":{},\"scroll_frames\":{},\"frame_period_ms\":{:.2},\"dropped_frames\":{},\"frame_cpu_p50_ms\":{:.2},\"frame_cpu_p99_ms\":{:.2},\"input_to_frame_p50_ms\":{:.2},\"input_to_frame_max_ms\":{:.2},\"sort_ms\":{}}}",
            self.entries,
            opt(self.first_frame),
            opt(self.first_rows),
            opt(self.full),
            intervals.len(),
            period,
            dropped,
            percentile(&mut cpu, 0.5),
            percentile(&mut cpu, 0.99),
            percentile(&mut lat, 0.5),
            lat.iter().copied().fold(0.0, f64::max),
            opt(self.sort),
        );
        let _ = std::fs::write(&self.out, s);
    }
}
