use super::*;
use crate::input::{Event, Key, Mods, MouseButton};
use crate::listing::{attr, DirSnapshot, Listing, Meta};
use crate::sort::{self, CaseMode, SortKeys, ViewSpec};
use crate::text::tests::Mono;
use crate::text::Gfx;
use crate::ui::{Drive, Persisted, Places};
use std::sync::Arc;
use std::time::Duration;

const ROWS: usize = 50_000;
const SLACK: u32 = if cfg!(debug_assertions) { 25 } else { 1 };

fn budget(ms: f64) -> Duration {
    Duration::from_secs_f64(ms / 1e3) * SLACK
}

fn loc(s: &str) -> Location {
    Location::parse(s).unwrap()
}

fn fs(s: &str) -> FsPath {
    FsPath::parse(s).unwrap()
}

fn names() -> Vec<String> {
    (0..ROWS).map(|k| if k % 10 == 0 { format!("folder {k:05}") } else { format!("photo {k:05}.jpg") }).collect()
}

fn snapshot(app: &App, names: &[String]) -> Arc<DirSnapshot> {
    let t = app.tab();
    let mut listing = Listing::default();
    let mut keys = SortKeys::new(CaseMode::Insensitive);
    for (k, n) in names.iter().enumerate() {
        let attrs = if k % 10 == 0 { attr::DIR } else { 0 };
        let i = listing.push(n.as_bytes(), Meta { attrs, size: (k as u64 * 7919) % 9_000_000, modified: 133_000_000_000_000_000 + k as i64 * 10_000_000, created: 132_000_000_000_000_000 });
        keys.push(listing.name(i));
    }
    let mut order = Vec::new();
    sort::sort(&listing, &keys, ViewSpec::default(), &mut order);
    Arc::new(DirSnapshot { gen: t.gen, path: t.location().path().clone(), listing, keys, order, view: ViewSpec::default(), done: true, error: None, elapsed: Duration::ZERO })
}

fn places() -> Arc<Places> {
    Arc::new(Places {
        folders: vec![(KnownFolder::Home, loc(r"C:\Users\u")), (KnownFolder::Downloads, loc(r"C:\Users\u\Downloads")), (KnownFolder::Documents, loc(r"C:\Users\u\Documents"))],
        drives: (0..6).map(|k| Drive { path: fs(&format!("{}:\\", (b'C' + k) as char)), label: format!("Disk {k} ({}:)", (b'C' + k) as char), total: 1000, free: 300, removable: false, unmounted: false, kind: None, remote: None }).collect(),
        recent: (0..24).map(|k| Recent { name: format!("file {k}.docx"), target: fs(&format!(r"C:\Users\u\Documents\file {k}.docx")), dir: k % 3 == 0, at: 1000 - k }).collect(),
        ..Default::default()
    })
}

struct Bench {
    app: App,
    gfx: Gfx<Mono>,
    report: Vec<String>,
    over: Vec<String>,
}

impl Bench {
    fn new() -> Self {
        let mut app = App::new(Persisted { path: loc(r"C:\big"), ..Default::default() }, Vec::new(), 0, false, 1.0, (1400, 900), Instant::now());
        app.set_places(places());
        app.set_pinned((0..8).map(|k| loc(&format!(r"C:\work\p{k}"))).collect());
        let mut gfx = Gfx::new(Mono, 1024);
        app.frame(&mut gfx, Instant::now());
        app.drain_effects().for_each(drop);
        Self { app, gfx, report: Vec::new(), over: Vec::new() }
    }

    fn frame(&mut self) {
        self.app.frame(&mut self.gfx, Instant::now());
        self.app.drain_effects().for_each(drop);
    }

    fn check(&mut self, what: &str, spent: Duration, limit_ms: f64) {
        let line = format!("{what}: {:.3} ms (budget {limit_ms} ms)", spent.as_secs_f64() * 1e3);
        if spent > budget(limit_ms) {
            self.over.push(line.clone());
        }
        self.report.push(line);
    }

    fn once(&mut self, what: &str, limit_ms: f64, act: impl FnOnce(&mut App)) {
        let t = Instant::now();
        act(&mut self.app);
        self.frame();
        self.check(what, t.elapsed(), limit_ms);
    }

    fn worst(&mut self, what: &str, runs: usize, limit_ms: f64, mut act: impl FnMut(&mut App, usize)) {
        let mut times: Vec<Duration> = (0..runs)
            .map(|k| {
                let t = Instant::now();
                act(&mut self.app, k);
                self.frame();
                t.elapsed()
            })
            .collect();
        times.sort();
        let p99 = times[(runs * 99 / 100).min(runs - 1)];
        self.check(what, p99, limit_ms);
    }

    fn key(&mut self, key: Key, mods: Mods) {
        self.app.on_event(&Event::Key { key, mods, pressed: true }, Instant::now());
    }

    fn finish(self) {
        eprintln!("{}", self.report.join("\n"));
        assert!(self.over.is_empty(), "over budget:\n{}", self.over.join("\n"));
    }
}

const NO: Mods = Mods { ctrl: false, shift: false, alt: false };
const CTRL: Mods = Mods { ctrl: true, shift: false, alt: false };

#[test]
fn every_hot_path_stays_within_its_time_budget() {
    let names = names();
    let mut b = Bench::new();
    let snap = snapshot(&b.app, &names);
    b.once("50k folder: snapshot to first frame", 60.0, |a| {
        a.tab().slot.publish(snap);
        a.poll();
    });
    b.worst("50k folder: wheel scroll frame p99", 300, 8.0, |a, k| a.on_event(&Event::Wheel { dx: 0.0, dy: if k < 150 { -3.0 } else { 3.0 } }, Instant::now()));
    b.worst("50k folder: hover frame p99", 300, 6.0, |a, k| a.on_event(&Event::MouseMove { x: 400.0 + (k % 40) as f32 * 9.0, y: 200.0 + (k % 30) as f32 * 18.0 }, Instant::now()));
    b.worst("50k folder: arrow key frame p99", 300, 6.0, |a, _| a.on_event(&Event::Key { key: Key::Down, mods: NO, pressed: true }, Instant::now()));
    b.once("50k folder: select all", 30.0, |a| a.on_event(&Event::Key { key: Key::Char('A'), mods: CTRL, pressed: true }, Instant::now()));
    b.once("50k folder: context menu on 50k selected", 12.0, |a| {
        std::hint::black_box(a.build_menu(true));
    });
    b.once("50k folder: properties of 50k selected", 15.0, |a| a.properties_dialog());
    b.key(Key::Escape, NO);
    b.frame();
    b.once("50k folder: sort by size", 80.0, |a| {
        a.run_action("view.sort.size", Instant::now());
    });
    b.once("50k folder: sort by name", 80.0, |a| {
        a.run_action("view.sort.name", Instant::now());
    });
    b.once("50k folder: group by type", 120.0, |a| {
        a.run_action("view.group.type", Instant::now());
    });
    b.once("50k folder: ungroup", 80.0, |a| {
        a.run_action("view.group.none", Instant::now());
    });
    b.once("50k folder: switch to grid", 40.0, |a| {
        a.run_action("view.layout.grid", Instant::now());
    });
    b.worst("50k folder: grid wheel frame p99", 200, 8.0, |a, k| a.on_event(&Event::Wheel { dx: 0.0, dy: if k < 100 { -3.0 } else { 3.0 } }, Instant::now()));
    b.once("50k folder: back to details", 40.0, |a| {
        a.run_action("view.layout.details", Instant::now());
    });
    b.worst("50k folder: filter keystroke p99", 8, 30.0, |a, k| {
        if k == 0 {
            a.run_action("view.filter", Instant::now());
        }
        a.on_event(&Event::Char(['p', 'h', 'o', 't', 'o', ' ', '1', '2'][k]), Instant::now());
    });
    b.key(Key::Escape, NO);
    b.key(Key::Escape, NO);
    b.frame();
    b.once("open the address bar", 10.0, |a| a.on_event(&Event::Key { key: Key::Char('L'), mods: CTRL, pressed: true }, Instant::now()));
    b.worst("address bar keystroke p99", 10, 10.0, |a, k| a.on_event(&Event::Char(['x', 'q', 'z', 'w', 'v', '1', '2', '3', '4', '5'][k]), Instant::now()));
    b.key(Key::Escape, NO);
    b.key(Key::Escape, NO);
    b.frame();
    b.once("open the palette", 15.0, |a| a.open_palette());
    b.worst("palette keystroke p99", 6, 10.0, |a, k| a.on_event(&Event::Char(['s', 'o', 'r', 't', ' ', 's'][k]), Instant::now()));
    b.worst("palette wheel frame p99", 100, 6.0, |a, k| a.on_event(&Event::Wheel { dx: 0.0, dy: if k < 50 { -1.0 } else { 1.0 } }, Instant::now()));
    b.key(Key::Escape, NO);
    b.frame();
    b.once("open settings", 30.0, |a| a.on_event(&Event::Key { key: Key::Char(','), mods: CTRL, pressed: true }, Instant::now()));
    b.worst("settings hover frame p99", 200, 6.0, |a, k| a.on_event(&Event::MouseMove { x: 500.0 + (k % 40) as f32 * 15.0, y: 120.0 + (k % 50) as f32 * 14.0 }, Instant::now()));
    b.worst("settings wheel frame p99", 200, 6.0, |a, k| a.on_event(&Event::Wheel { dx: 0.0, dy: if k < 100 { -1.0 } else { 1.0 } }, Instant::now()));
    b.worst("settings search keystroke p99", 9, 15.0, |a, k| a.on_event(&Event::Char("thumbnail".as_bytes()[k] as char), Instant::now()));
    b.key(Key::Escape, NO);
    b.key(Key::Escape, NO);
    b.frame();
    b.worst("sidebar hover frame p99", 200, 6.0, |a, k| a.on_event(&Event::MouseMove { x: 60.0 + (k % 10) as f32 * 10.0, y: 140.0 + (k % 40) as f32 * 16.0 }, Instant::now()));
    b.once("collapse a sidebar section", 10.0, |a| a.toggle_section(Section::Drives));
    b.once("new tab", 20.0, |a| {
        a.run_action("tab.new", Instant::now());
    });
    b.once("open Home", 20.0, |a| a.go_home_page(false, Instant::now()));
    b.worst("Home hover frame p99", 200, 6.0, |a, k| a.on_event(&Event::MouseMove { x: 400.0 + (k % 50) as f32 * 8.0, y: 200.0 + (k % 40) as f32 * 12.0 }, Instant::now()));
    b.once("switch back to the 50k tab", 30.0, |a| {
        a.run_action("tab.previous", Instant::now());
    });
    b.once("split the pane", 40.0, |a| {
        a.run_action("panel.open_bottom", Instant::now());
    });
    b.once("right click on a row", 40.0, |a| {
        a.on_event(&Event::MouseMove { x: 500.0, y: 300.0 }, Instant::now());
        a.on_event(&Event::MouseDown { x: 500.0, y: 300.0, button: MouseButton::Right, clicks: 1 }, Instant::now());
        a.on_event(&Event::MouseUp { x: 500.0, y: 300.0, button: MouseButton::Right }, Instant::now());
    });
    b.finish();
}
