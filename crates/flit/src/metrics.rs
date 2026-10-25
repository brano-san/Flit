use flit_core::input::{Event, Key, Mods};
use flit_core::ui::App;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const SETTLE_FRAMES: u32 = 30;
const FALLBACK_PERIOD: Duration = Duration::from_micros(16_667);
const SCROLL_TIME: Duration = Duration::from_secs(2);
const GOLDEN_FRACTION: f64 = 0.618_033_988_749_895;
const SETTINGS_SEARCH: &str = "thumbnail";
const THEME_SWITCHES: u32 = 8;
const SETTINGS_CLOSE: u32 = 2;

pub enum Step {
    None,
    Inject(Event),
    InjectAfter(Duration, Event),
    Finish,
}

enum Phase {
    Loading,
    Warm(u32),
    Reloading(Instant),
    Settle(u32),
    Scroll,
    Sort,
    Done,
    Settings(usize),
    Theme(u32),
}

pub struct Metrics {
    out: PathBuf,
    started: Instant,
    refresh: Option<Duration>,
    phase: Phase,
    first_frame: Option<f64>,
    first_rows: Option<f64>,
    full: Option<f64>,
    first_rows_worker: Option<f64>,
    full_worker: Option<f64>,
    entries: usize,
    frame_ends: Vec<Instant>,
    scroll_from: usize,
    scroll_rows: usize,
    software_frames: u32,
    cpu: Vec<f32>,
    latencies: Vec<f64>,
    injected_at: Option<Instant>,
    injection_pending: bool,
    injections: u32,
    sort: Option<f64>,
    split: bool,
    split_at: Option<Instant>,
    split_ready: Option<f64>,
    settings_mark: Option<Instant>,
    settings_input: Vec<(f64, f64)>,
    theme_switch: Vec<(f64, f64)>,
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

pub fn dual_at_start() -> bool {
    std::env::var("FLIT_METRICS_SPLIT").is_ok_and(|s| s == "start")
}

fn key(key: Key, ctrl: bool) -> Event {
    Event::Key { key, mods: Mods { ctrl, ..Mods::default() }, pressed: true }
}

impl Metrics {
    pub fn new(out: PathBuf, started: Instant, refresh: Option<Duration>) -> Self {
        Self {
            out,
            started,
            refresh,
            phase: Phase::Loading,
            first_frame: None,
            first_rows: None,
            full: None,
            first_rows_worker: None,
            full_worker: None,
            entries: 0,
            frame_ends: Vec::new(),
            scroll_from: 0,
            scroll_rows: 0,
            software_frames: 0,
            cpu: Vec::new(),
            latencies: Vec::new(),
            injected_at: None,
            injection_pending: false,
            injections: 0,
            sort: None,
            split: std::env::var_os("FLIT_METRICS_SPLIT").is_some(),
            split_at: dual_at_start().then_some(started),
            split_ready: None,
            settings_mark: None,
            settings_input: Vec::new(),
            theme_switch: Vec::new(),
        }
    }

    pub fn on_injected(&mut self, at: Instant) {
        self.injected_at = Some(at);
        self.injection_pending = false;
    }

    pub fn on_sorted(&mut self, sort_ms: f64) {
        if matches!(self.phase, Phase::Sort) {
            self.sort = Some(sort_ms);
            self.phase = Phase::Done;
        }
    }

    fn period(&self) -> Duration {
        self.refresh.unwrap_or(FALLBACK_PERIOD)
    }

    pub fn after_frame(&mut self, app: &App, software: bool, cpu_ms: f32, now: Instant) -> Step {
        self.first_frame.get_or_insert_with(|| ms(now - self.started));
        if let Some(t) = self.injected_at.take() {
            self.latencies.push(ms(now - t));
        }
        let stats = app.tab().stats;
        match self.phase {
            Phase::Loading => {
                if stats.done.is_none() {
                    return Step::None;
                }
                if self.split && self.split_at.is_none() {
                    self.split_at = Some(now);
                    return Step::Inject(Event::Key { key: Key::Char('V'), mods: Mods { shift: true, alt: true, ctrl: false }, pressed: true });
                }
                if app.other_pane().is_some_and(|t| t.stats.done.is_none()) {
                    return Step::None;
                }
                self.split_ready = self.split_at.filter(|_| app.other_pane().is_some()).map(|at| ms(now - at));
                self.phase = Phase::Warm(SETTLE_FRAMES);
                Step::None
            }
            Phase::Warm(0) => {
                self.phase = Phase::Reloading(now);
                Step::Inject(key(Key::F(5), false))
            }
            Phase::Warm(n) => {
                self.phase = Phase::Warm(n - 1);
                Step::None
            }
            Phase::Reloading(at) => {
                if self.first_rows.is_none() && stats.first.is_some() && stats.entries > 0 {
                    self.first_rows = Some(ms(now - at));
                }
                if let Some(done) = stats.done {
                    self.first_rows_worker = stats.first.map(ms);
                    self.full_worker = Some(ms(done));
                    self.full = Some(ms(now - at));
                    self.entries = stats.entries;
                    self.phase = Phase::Settle(SETTLE_FRAMES);
                }
                Step::None
            }
            Phase::Settle(0) => {
                self.phase = Phase::Scroll;
                self.scroll_from = app.visible_rows().1 as usize;
                Step::None
            }
            Phase::Settle(n) => {
                self.phase = Phase::Settle(n - 1);
                Step::None
            }
            Phase::Scroll => {
                self.frame_ends.push(now);
                self.cpu.push(cpu_ms);
                self.software_frames += software as u32;
                if self.injection_pending {
                    return Step::None;
                }
                let (_, first, last) = app.visible_rows();
                let rows = app.tab().rows();
                if last as usize >= rows {
                    self.scroll_rows = rows.saturating_sub(self.scroll_from);
                    self.phase = Phase::Sort;
                    return Step::Inject(key(Key::Char('H'), true));
                }
                self.injection_pending = true;
                self.injections += 1;
                let page = last.saturating_sub(first).saturating_sub(1).max(1) as f64;
                let step = page.max(rows as f64 * self.period().as_secs_f64() / SCROLL_TIME.as_secs_f64());
                let offset = (self.injections as f64 * GOLDEN_FRACTION).fract();
                Step::InjectAfter(self.period().mul_f64(offset), Event::Wheel { dx: 0.0, dy: -(step / 3.0) as f32 })
            }
            Phase::Sort => Step::None,
            Phase::Done => {
                self.phase = Phase::Settings(0);
                self.settings_mark = Some(now);
                Step::Inject(key(Key::Char(','), true))
            }
            Phase::Settings(n) => {
                if let Some(t) = self.settings_mark.take() {
                    self.settings_input.push((ms(now - t), cpu_ms as f64));
                }
                match SETTINGS_SEARCH.chars().nth(n) {
                    Some(c) => {
                        self.phase = Phase::Settings(n + 1);
                        self.settings_mark = Some(now);
                        Step::Inject(Event::Char(c))
                    }
                    None => self.switch_theme(0, now),
                }
            }
            Phase::Theme(n) => {
                if let Some(t) = self.settings_mark.take() {
                    self.theme_switch.push((ms(now - t), cpu_ms as f64));
                }
                if n < SETTINGS_CLOSE + THEME_SWITCHES {
                    return self.switch_theme(n + 1, now);
                }
                self.write();
                Step::Finish
            }
        }
    }

    fn switch_theme(&mut self, n: u32, now: Instant) -> Step {
        self.phase = Phase::Theme(n);
        if n < SETTINGS_CLOSE {
            return Step::Inject(key(Key::Escape, false));
        }
        self.settings_mark = Some(now);
        Step::Inject(Event::Key { key: Key::Char('T'), mods: Mods { ctrl: true, alt: true, shift: false }, pressed: true })
    }

    fn write(&self) {
        let intervals: Vec<f64> = self.frame_ends.windows(2).map(|w| ms(w[1] - w[0])).collect();
        let scroll_ms = self.frame_ends.first().zip(self.frame_ends.last()).map_or(0.0, |(a, b)| ms(*b - *a));
        let period = ms(self.period());
        let dropped = intervals.iter().filter(|&&i| i > period * 1.5).count();
        let mut cpu: Vec<f64> = self.cpu.iter().map(|&c| c as f64).collect();
        let mut lat = self.latencies.clone();
        let opt = |v: Option<f64>| v.map_or("null".to_string(), |x| format!("{x:.2}"));
        let open = self.settings_input.first().copied();
        let mut search: Vec<f64> = self.settings_input.iter().skip(1).map(|s| s.0).collect();
        let mut search_cpu: Vec<f64> = self.settings_input.iter().skip(1).map(|s| s.1).collect();
        let mut theme: Vec<f64> = self.theme_switch.iter().map(|s| s.0).collect();
        let mut theme_cpu: Vec<f64> = self.theme_switch.iter().map(|s| s.1).collect();
        let mut s = String::new();
        let _ = write!(
            s,
            "{{\"entries\":{},\"start_to_first_frame_ms\":{},\"first_rows_ms\":{},\"full_listing_ms\":{},\"first_rows_worker_ms\":{},\"full_listing_worker_ms\":{},\"scroll_rows\":{},\"scroll_frames\":{},\"scroll_ms\":{:.2},\"software_frames\":{},\"frame_period_ms\":{:.2},\"refresh_from_display\":{},\"frame_interval_p50_ms\":{:.2},\"dropped_frames\":{},\"frame_cpu_p50_ms\":{:.2},\"frame_cpu_p99_ms\":{:.2},\"input_to_frame_p50_ms\":{:.2},\"input_to_frame_max_ms\":{:.2},\"sort_ms\":{},\"panes\":{},\"split_ready_ms\":{},\"settings_open_ms\":{},\"settings_open_cpu_ms\":{},\"settings_search_p50_ms\":{:.2},\"settings_search_cpu_p50_ms\":{:.2},\"theme_switch_p50_ms\":{:.2},\"theme_switch_cpu_p50_ms\":{:.2}}}",
            self.entries,
            opt(self.first_frame),
            opt(self.first_rows),
            opt(self.full),
            opt(self.first_rows_worker),
            opt(self.full_worker),
            self.scroll_rows,
            intervals.len(),
            scroll_ms,
            self.software_frames,
            period,
            self.refresh.is_some(),
            percentile(&mut intervals.clone(), 0.5),
            dropped,
            percentile(&mut cpu, 0.5),
            percentile(&mut cpu, 0.99),
            percentile(&mut lat, 0.5),
            lat.iter().copied().fold(0.0, f64::max),
            opt(self.sort),
            if self.split { 2 } else { 1 },
            opt(self.split_ready),
            opt(open.map(|o| o.0)),
            opt(open.map(|o| o.1)),
            percentile(&mut search, 0.5),
            percentile(&mut search_cpu, 0.5),
            percentile(&mut theme, 0.5),
            percentile(&mut theme_cpu, 0.5),
        );
        let _ = std::fs::write(&self.out, s);
    }
}
