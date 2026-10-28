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
const QUERIES: [&str; 3] = ["report", "отчёт", "文件"];
const INDEX_WAIT: Duration = Duration::from_secs(900);

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
    Filter(usize, usize),
    SearchOpen(usize),
    IndexWait(usize, Instant),
    Search(usize, usize, usize),
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
    cold_first_rows: Option<f64>,
    cold_full: Option<f64>,
    awaiting: bool,
    dispatched: Option<Instant>,
    filter_keys: Vec<Vec<f64>>,
    filter_rows: Vec<usize>,
    index_ready: Option<f64>,
    index_wait: Option<f64>,
    index_files: u64,
    backend: String,
    private_listing: f64,
    private_indexed: f64,
    search_keys: [Vec<Vec<f64>>; 2],
    search_totals: [Vec<u64>; 2],
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

#[cfg(windows)]
fn private_mb() -> f64 {
    #[repr(C)]
    #[derive(Default)]
    struct Counters {
        cb: u32,
        page_faults: u32,
        sizes: [usize; 8],
        private: usize,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn K32GetProcessMemoryInfo(process: isize, counters: *mut Counters, cb: u32) -> i32;
        fn GetCurrentProcess() -> isize;
    }
    let mut c = Counters { cb: std::mem::size_of::<Counters>() as u32, ..Counters::default() };
    let ok = unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) } != 0;
    if ok { c.private as f64 / 1_048_576.0 } else { 0.0 }
}

#[cfg(not(windows))]
fn private_mb() -> f64 {
    0.0
}

fn flat(v: &[Vec<f64>]) -> Vec<f64> {
    v.iter().flatten().copied().collect()
}

fn nested(v: &[Vec<f64>]) -> String {
    let inner: Vec<String> = v.iter().map(|k| format!("[{}]", k.iter().map(|x| format!("{x:.2}")).collect::<Vec<_>>().join(","))).collect();
    format!("[{}]", inner.join(","))
}

fn key(key: Key, ctrl: bool) -> Event {
    Event::Key { key, mods: Mods { ctrl, ..Mods::default() }, pressed: true }
}

fn search_key(scope: usize) -> Event {
    Event::Key { key: Key::Char('F'), mods: Mods { ctrl: true, alt: scope == 0, shift: scope != 0 }, pressed: true }
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
            split_at: None,
            split_ready: None,
            settings_mark: None,
            settings_input: Vec::new(),
            theme_switch: Vec::new(),
            cold_first_rows: None,
            cold_full: None,
            awaiting: false,
            dispatched: None,
            filter_keys: vec![Vec::new(); QUERIES.len()],
            filter_rows: Vec::new(),
            index_ready: None,
            index_wait: None,
            index_files: 0,
            backend: String::new(),
            private_listing: 0.0,
            private_indexed: 0.0,
            search_keys: [vec![Vec::new(); QUERIES.len()], vec![Vec::new(); QUERIES.len()]],
            search_totals: [Vec::new(), Vec::new()],
        }
    }

    pub fn on_injected(&mut self, at: Instant) {
        self.injected_at = Some(at);
        self.dispatched = Some(at);
        self.injection_pending = false;
    }

    pub fn on_sorted(&mut self, sort_ms: f64) {
        if matches!(self.phase, Phase::Sort) {
            self.sort = Some(sort_ms);
            self.phase = Phase::Filter(0, 0);
        }
    }

    fn period(&self) -> Duration {
        self.refresh.unwrap_or(FALLBACK_PERIOD)
    }

    pub fn after_frame(&mut self, app: &App, software: bool, cpu_ms: f32, now: Instant) -> Step {
        self.first_frame.get_or_insert_with(|| ms(now - self.started));
        let latency = self.injected_at.take().map(|t| ms(now - t));
        let typing = matches!(self.phase, Phase::Filter(..) | Phase::SearchOpen(_) | Phase::IndexWait(..) | Phase::Search(..));
        if let Some(l) = latency.filter(|_| !typing) {
            self.latencies.push(l);
        }
        if self.awaiting {
            if latency.is_none() {
                return Step::None;
            }
            self.awaiting = false;
        }
        let stats = app.tab().stats;
        match self.phase {
            Phase::Loading => {
                if self.cold_first_rows.is_none() && stats.first.is_some() && stats.entries > 0 {
                    self.cold_first_rows = Some(ms(now - self.started));
                }
                if stats.done.is_some() && self.cold_full.is_none() {
                    self.cold_full = Some(ms(now - self.started));
                }
                if stats.done.is_none() {
                    return Step::None;
                }
                if self.split && self.split_at.is_none() {
                    self.split_at = Some(now);
                    return Step::Inject(Event::Key { key: Key::Char('S'), mods: Mods { shift: true, ctrl: true, alt: false }, pressed: true });
                }
                if app.parked_tabs().any(|t| t.stats.done.is_none()) {
                    return Step::None;
                }
                self.split_ready = self.split_at.filter(|_| app.panel_count() > 1).map(|at| ms(now - at));
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
                self.private_listing = private_mb();
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
            Phase::Filter(q, c) => {
                if let (Some(l), true) = (latency, c >= 2) {
                    self.filter_keys[q].push(l);
                }
                let chars: Vec<char> = QUERIES[q].chars().collect();
                if c == 0 {
                    self.phase = Phase::Filter(q, 1);
                    return self.typed(key(Key::Char('F'), true));
                }
                if let Some(&ch) = chars.get(c - 1) {
                    self.phase = Phase::Filter(q, c + 1);
                    return self.typed(Event::Char(ch));
                }
                self.filter_rows.push(app.tab().rows());
                if q + 1 < QUERIES.len() {
                    self.phase = Phase::Filter(q + 1, 0);
                    return Step::None;
                }
                self.phase = Phase::SearchOpen(0);
                self.typed(key(Key::Escape, false))
            }
            Phase::SearchOpen(scope) => {
                self.phase = Phase::IndexWait(scope, now);
                self.typed(search_key(scope))
            }
            Phase::IndexWait(scope, since) => {
                if let Some(files) = app.index_ready() {
                    if self.index_ready.is_none() {
                        self.index_ready = Some(ms(now - self.started));
                        self.index_wait = Some(ms(now - since));
                        self.index_files = files;
                        self.backend = format!("{:?}", app.search_backend());
                        self.private_indexed = private_mb();
                    }
                    self.phase = Phase::Search(scope, 0, 0);
                    return Step::None;
                }
                if now - since > INDEX_WAIT {
                    self.phase = Phase::Done;
                    return self.typed(key(Key::Escape, false));
                }
                Step::None
            }
            Phase::Search(scope, q, c) => {
                let chars: Vec<char> = QUERIES[q].chars().collect();
                if c >= 2 {
                    let Some(total) = app.search_finished() else { return Step::None };
                    self.search_keys[scope][q].push(self.dispatched.map_or(0.0, |t| ms(now - t)));
                    if c - 1 == chars.len() {
                        self.search_totals[scope].push(total);
                    }
                }
                if c == 0 {
                    self.phase = Phase::Search(scope, q, 1);
                    return self.typed(search_key(scope));
                }
                if let Some(&ch) = chars.get(c - 1) {
                    self.phase = Phase::Search(scope, q, c + 1);
                    return self.typed(Event::Char(ch));
                }
                if q + 1 < QUERIES.len() {
                    self.phase = Phase::Search(scope, q + 1, 0);
                    return Step::None;
                }
                self.phase = if scope == 0 { Phase::SearchOpen(1) } else { Phase::Done };
                self.typed(key(Key::Escape, false))
            }
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

    fn typed(&mut self, ev: Event) -> Step {
        self.awaiting = true;
        Step::InjectAfter(Duration::ZERO, ev)
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
            "{{\"entries\":{},\"start_to_first_frame_ms\":{},\"first_rows_ms\":{},\"full_listing_ms\":{},\"first_rows_worker_ms\":{},\"full_listing_worker_ms\":{},\"scroll_rows\":{},\"scroll_frames\":{},\"scroll_ms\":{:.2},\"software_frames\":{},\"frame_period_ms\":{:.2},\"refresh_from_display\":{},\"frame_interval_p50_ms\":{:.2},\"dropped_frames\":{},\"frame_cpu_p50_ms\":{:.2},\"frame_cpu_p99_ms\":{:.2},\"input_to_frame_p50_ms\":{:.2},\"input_to_frame_max_ms\":{:.2},\"sort_ms\":{},\"panes\":{},\"split_ready_ms\":{},\"settings_open_ms\":{},\"settings_open_cpu_ms\":{},\"settings_search_p50_ms\":{:.2},\"settings_search_cpu_p50_ms\":{:.2},\"theme_switch_p50_ms\":{:.2},\"theme_switch_cpu_p50_ms\":{:.2},\"cold_first_rows_ms\":{},\"cold_full_listing_ms\":{},\"filter_key_p50_ms\":{:.2},\"filter_key_max_ms\":{:.2},\"filter_rows\":{:?},\"filter_keys_ms\":{},\"index_ready_ms\":{},\"index_wait_ms\":{},\"index_files\":{},\"search_backend\":\"{}\",\"private_listing_mb\":{:.0},\"private_indexed_mb\":{:.0},\"private_end_mb\":{:.0},\"search_key_p50_ms\":{:.2},\"search_key_max_ms\":{:.2},\"search_totals\":{:?},\"search_keys_ms\":{},\"folder_search_key_p50_ms\":{:.2},\"folder_search_key_max_ms\":{:.2},\"folder_search_totals\":{:?},\"folder_search_keys_ms\":{}}}",
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
            opt(self.cold_first_rows),
            opt(self.cold_full),
            percentile(&mut flat(&self.filter_keys), 0.5),
            flat(&self.filter_keys).iter().copied().fold(0.0, f64::max),
            self.filter_rows,
            nested(&self.filter_keys),
            opt(self.index_ready),
            opt(self.index_wait),
            self.index_files,
            self.backend,
            self.private_listing,
            self.private_indexed,
            private_mb(),
            percentile(&mut flat(&self.search_keys[0]), 0.5),
            flat(&self.search_keys[0]).iter().copied().fold(0.0, f64::max),
            self.search_totals[0],
            nested(&self.search_keys[0]),
            percentile(&mut flat(&self.search_keys[1]), 0.5),
            flat(&self.search_keys[1]).iter().copied().fold(0.0, f64::max),
            self.search_totals[1],
            nested(&self.search_keys[1]),
        );
        let _ = std::fs::write(&self.out, s);
    }
}
