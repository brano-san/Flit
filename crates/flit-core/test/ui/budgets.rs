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
const ATTEMPTS: usize = 3;

fn budget(ms: f64) -> Duration {
    Duration::from_secs_f64(ms / 1e3) * SLACK
}

struct Timing {
    what: String,
    spent: Duration,
    limit_ms: f64,
}

impl Timing {
    fn over(&self) -> bool {
        self.spent > budget(self.limit_ms)
    }

    fn line(&self) -> String {
        format!("{}: {:.3} ms (budget {} ms)", self.what, self.spent.as_secs_f64() * 1e3, self.limit_ms)
    }
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
        drives: (0..6).map(|k| Drive { path: fs(&format!("{}:\\", (b'C' + k) as char)), label: format!("Disk {k} ({}:)", (b'C' + k) as char), total: 1000, free: 300, file_system: String::new(), removable: false, unmounted: false, kind: None, remote: None }).collect(),
        recent: (0..24).map(|k| Recent { name: format!("file {k}.docx"), target: fs(&format!(r"C:\Users\u\Documents\file {k}.docx")), dir: k % 3 == 0, at: 1000 - k }).collect(),
        ..Default::default()
    })
}

struct Bench {
    app: App,
    gfx: Gfx<Mono>,
    timings: Vec<Timing>,
}

impl Bench {
    fn new() -> Self {
        let mut app = App::new(Persisted { path: loc(r"C:\big"), ..Default::default() }, Vec::new(), 0, false, 1.0, (1400, 900), Instant::now());
        app.set_places(places());
        app.set_pinned((0..8).map(|k| loc(&format!(r"C:\work\p{k}"))).collect());
        let mut gfx = Gfx::new(Mono, 1024);
        app.frame(&mut gfx, Instant::now());
        app.drain_effects().for_each(drop);
        Self { app, gfx, timings: Vec::new() }
    }

    fn frame(&mut self) {
        self.app.frame(&mut self.gfx, Instant::now());
        self.app.drain_effects().for_each(drop);
    }

    fn check(&mut self, what: &str, spent: Duration, limit_ms: f64) {
        self.timings.push(Timing { what: what.to_owned(), spent, limit_ms });
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

    fn finish(self) -> Vec<Timing> {
        self.timings
    }
}

const NO: Mods = Mods { ctrl: false, shift: false, alt: false };
const CTRL: Mods = Mods { ctrl: true, shift: false, alt: false };

#[test]
fn every_hot_path_stays_within_its_time_budget() {
    let mut best = hot_paths();
    for _ in 1..ATTEMPTS {
        if !best.iter().any(Timing::over) {
            break;
        }
        for (b, again) in best.iter_mut().zip(hot_paths()) {
            b.spent = b.spent.min(again.spent);
        }
    }
    eprintln!("{}", best.iter().map(Timing::line).collect::<Vec<_>>().join("\n"));
    let over: Vec<String> = best.iter().filter(|t| t.over()).map(Timing::line).collect();
    assert!(over.is_empty(), "over budget in each of {ATTEMPTS} attempts:\n{}", over.join("\n"));
}

fn hot_paths() -> Vec<Timing> {
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
    b.once("collapse a sidebar section", 10.0, |a| a.toggle_section(Section::Storage));
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
    b.finish()
}

#[test]
fn ten_thousand_extensionless_content_icons_stay_within_the_ui_budget() {
    let names: Vec<_> = (0..10_000).map(|i| format!("file{i:05}")).collect();
    let mut best = Duration::MAX;
    for _ in 0..ATTEMPTS {
        let mut b = Bench::new();
        let path = Location::parse("/tmp/content-icons").unwrap();
        b.app = App::new(Persisted { path: path.clone(), ..Default::default() }, Vec::new(), 0, false, 1.0, (1400, 900), Instant::now());
        let mut snap = (*snapshot(&b.app, &names)).clone();
        snap.path = path.path().clone();
        for e in &mut snap.listing.entries { e.attrs = 0; e.size = 100; }
        b.app.tab().slot.publish(Arc::new(snap));
        b.app.poll();
        let start = Instant::now();
        for _ in 0..100 {
            b.app.on_event(&Event::Wheel { dx: 0.0, dy: -3.0 }, Instant::now());
            b.frame();
        }
        best = best.min(start.elapsed());
    }
    eprintln!("10k extensionless scroll: {:?} per frame, Content request limit 8", best / 100);
    assert!(best / 100 < budget(8.0));
}

#[test]
fn twenty_thousand_files_sorted_by_length_keep_every_frame_fast_while_all_lengths_arrive() {
    let names: Vec<String> = (0..20_000).map(|k| format!("clip {k:05}.mp4")).collect();
    let files = names.len() - names.len() / 10;
    let (mut worst, mut whole) = (Duration::MAX, Duration::MAX);
    for _ in 0..ATTEMPTS {
        let mut b = Bench::new();
        let snap = snapshot(&b.app, &names);
        b.app.tab().slot.publish(snap);
        b.app.poll();
        b.frame();
        b.app.sort_column(crate::sort::SortField::Duration);
        let (start, mut now, mut read, mut frames) = (Instant::now(), Instant::now(), 0, Vec::new());
        while read < files && frames.len() < 10_000 {
            let t = Instant::now();
            b.app.frame(&mut b.gfx, now);
            frames.push(t.elapsed());
            for e in b.app.drain_effects().collect::<Vec<_>>() {
                match e {
                    Effect::ShellProps { tab, gen, ask, names, .. } => {
                        read += names.len();
                        let rows = names
                            .into_iter()
                            .map(|n| {
                                let k = n.iter().filter(|d| d.is_ascii_digit()).fold(0u64, |a, &d| a * 10 + u64::from(d - b'0'));
                                let text = vec![String::new(), format!("{k}"), String::new(), String::new()];
                                (n, crate::listing::ColumnValues { text, duration: Some(k * 7919 % 36_000 * 10_000_000), ..Default::default() })
                            })
                            .collect();
                        b.app.on_shell_props(tab, gen, ask, rows);
                    }
                    Effect::Resort { tab, gen, view, snap } => {
                        let mut order = Vec::new();
                        sort::sort(&snap.listing, &snap.keys, view, &mut order);
                        b.app.on_resorted(tab, gen, view, snap.listing.len(), order);
                    }
                    _ => {}
                }
            }
            now += Duration::from_millis(16);
        }
        for _ in 0..40 {
            b.app.frame(&mut b.gfx, now);
            b.app.drain_effects().for_each(drop);
            now += Duration::from_millis(16);
        }
        let took = start.elapsed();
        assert_eq!(read, files, "every file is read once");
        let v = b.app.tab().view.as_ref().unwrap();
        let s = v.snap();
        assert_eq!(s.listing.props.len(), files);
        assert!(sort::is_sorted(&s.listing, &s.keys, &s.view, &s.order));
        frames.sort();
        worst = worst.min(frames[frames.len() * 99 / 100]);
        whole = whole.min(took);
    }
    eprintln!("20k sorted by length: p99 frame {worst:?}, all lengths in {whole:?}");
    assert!(worst < budget(8.0), "p99 frame {worst:?}");
    assert!(whole < Duration::from_secs(30), "reading every length took {whole:?}");
}

#[test]
fn scrolling_ten_thousand_files_with_shell_overlays_stays_within_the_ui_budget() {
    let names: Vec<_> = (0..10_000).map(|i| format!("file{i:05}.txt")).collect();
    let mut best = Duration::MAX;
    let mut asked_most = 0;
    for _ in 0..ATTEMPTS {
        let mut b = Bench::new();
        let path = loc(r"C:\overlays");
        b.app = App::new(Persisted { path: path.clone(), ..Default::default() }, Vec::new(), 0, false, 1.0, (1400, 900), Instant::now());
        b.app.set_shell_overlays(true);
        let mut snap = (*snapshot(&b.app, &names)).clone();
        snap.path = path.path().clone();
        for e in &mut snap.listing.entries { e.attrs = 0; }
        b.app.tab().slot.publish(Arc::new(snap));
        b.app.poll();
        let start = Instant::now();
        for k in 0..100 {
            b.app.on_event(&Event::Wheel { dx: 0.0, dy: -3.0 }, Instant::now());
            b.app.frame(&mut b.gfx, Instant::now());
            for e in b.app.drain_effects().collect::<Vec<_>>() {
                if let Effect::Overlays(batch) = e {
                    asked_most = asked_most.max(batch.items.len());
                    let found = batch.items.iter().map(|i| (i.key, (k % 3 == 0).then_some(1))).collect();
                    let images = if batch.have.contains(&1) { Vec::new() } else { vec![(1, crate::ui::Bitmap { width: 16, height: 16, rgba: vec![255; 1024] })] };
                    b.app.on_overlays(&mut b.gfx, crate::ui::OverlayReply { id: batch.id, size: batch.size, found, skipped: Vec::new(), images });
                }
            }
        }
        best = best.min(start.elapsed());
    }
    eprintln!("10k overlay scroll: {:?} per frame, largest batch {asked_most}", best / 100);
    assert!(asked_most > 0 && asked_most <= crate::ui::OVERLAY_BATCH);
    assert!(best / 100 < budget(8.0));
}
