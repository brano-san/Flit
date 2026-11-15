use super::*;
use flit_core::ui::{Bitmap, IconRequest, OverlayItem};
use flit_core::FsPath;
use flit_platform::PlatformError;
use std::sync::atomic::AtomicBool;

struct Probe {
    made: AtomicU32,
    asked: AtomicU32,
    images: AtomicU32,
    release: AtomicBool,
}

impl Probe {
    const fn new() -> Probe {
        Probe { made: AtomicU32::new(0), asked: AtomicU32::new(0), images: AtomicU32::new(0), release: AtomicBool::new(false) }
    }
}

static PROBES: [Probe; 5] = [Probe::new(), Probe::new(), Probe::new(), Probe::new(), Probe::new()];

struct Handlers<const N: usize>;

impl<const N: usize> Shell for Handlers<N> {
    fn new() -> Result<Self, PlatformError> {
        PROBES[N].made.fetch_add(1, Ordering::SeqCst);
        Ok(Handlers)
    }
    fn icon(&mut self, _: &IconRequest) -> Option<Bitmap> {
        None
    }
    fn type_name(&mut self, _: &[u8]) -> Option<String> {
        None
    }
    fn open(&mut self, _: &FsPath) -> Result<(), PlatformError> {
        Ok(())
    }
    fn overlay_slot(&mut self, path: &FsPath) -> Option<u8> {
        let p = &PROBES[N];
        p.asked.fetch_add(1, Ordering::SeqCst);
        let name = path.file_name();
        if name == b"hang" {
            while !p.release.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        if name == b"slow" {
            std::thread::sleep(Duration::from_millis(60));
        }
        if name == b"odd" {
            return Some(40);
        }
        name.starts_with(b"mark").then_some(5)
    }
    fn overlay_image(&mut self, slot: u8, size: u32) -> Option<Bitmap> {
        PROBES[N].images.fetch_add(1, Ordering::SeqCst);
        Some(Bitmap { width: size, height: size, rgba: vec![slot; (size * size * 4) as usize] })
    }
}

fn item(key: u64, name: &str) -> OverlayItem {
    OverlayItem { key, path: FsPath::parse(&format!(r"C:\repo\{name}")).unwrap() }
}

fn job(id: u64, items: Vec<OverlayItem>, have: Vec<u8>, shown: &Shown) -> Job {
    Job { window: 9, batch: OverlayBatch { id, size: 16, items, fetch: Vec::new(), have }, shown: shown.clone() }
}

fn started<S: Shell + 'static>(hung: Duration, budget: Duration) -> (Overlays, crossbeam_channel::Receiver<(WindowId, OverlayReply)>) {
    let (tx, rx) = crossbeam_channel::unbounded();
    let overlays = Overlays::with_limits::<S>(Arc::new(move |w, r| drop(tx.send((w, r)))), hung, budget);
    (overlays, rx)
}

fn shown(keys: &[u64]) -> Shown {
    Arc::new(Mutex::new(keys.to_vec()))
}

const WAIT: Duration = Duration::from_secs(10);

#[test]
fn marked_files_get_their_slot_and_each_slot_image_comes_once() {
    let (o, rx) = started::<Handlers<0>>(HUNG, BUDGET);
    let seen = shown(&[1, 2, 3, 4]);
    o.send(job(1, vec![item(1, "mark-a"), item(2, "mark-b"), item(3, "plain.txt"), item(4, "odd")], Vec::new(), &seen));
    let (window, reply) = rx.recv_timeout(WAIT).unwrap();
    assert_eq!(window, 9);
    assert_eq!(reply.found, [(1, Some(5)), (2, Some(5)), (3, None), (4, None)]);
    assert_eq!(reply.images.iter().map(|i| (i.0, i.1.width)).collect::<Vec<_>>(), [(5, 16)]);
    *seen.lock().unwrap() = vec![5];
    o.send(job(2, vec![item(5, "mark-c")], vec![5], &seen));
    let (_, reply) = rx.recv_timeout(WAIT).unwrap();
    assert_eq!(reply.found, [(5, Some(5))]);
    assert!(reply.images.is_empty(), "an image the window already has is not sent again");
    assert_eq!(PROBES[0].images.load(Ordering::SeqCst), 1);
    assert_eq!(PROBES[0].made.load(Ordering::SeqCst), 1, "one long-lived shell object");
}

#[test]
fn rows_scrolled_away_and_rows_past_the_batch_budget_are_skipped_without_asking_the_shell() {
    let (o, rx) = started::<Handlers<1>>(HUNG, Duration::from_millis(50));
    o.send(job(1, vec![item(1, "mark"), item(2, "mark"), item(3, "slow"), item(4, "mark"), item(5, "mark")], Vec::new(), &shown(&[1, 3, 4])));
    let (_, reply) = rx.recv_timeout(WAIT).unwrap();
    assert_eq!(reply.found, [(1, Some(5)), (3, None)]);
    assert_eq!(reply.skipped, [2, 4, 5]);
    assert_eq!(PROBES[1].asked.load(Ordering::SeqCst), 2);
}

#[test]
fn a_handler_that_stops_answering_is_left_behind_and_later_rows_still_get_overlays() {
    let (o, rx) = started::<Handlers<2>>(Duration::from_millis(150), BUDGET);
    let seen = shown(&[1, 2, 3, 4]);
    let t = Instant::now();
    o.send(job(1, vec![item(1, "mark-a"), item(2, "hang"), item(3, "mark-b")], Vec::new(), &seen));
    let (_, reply) = rx.recv_timeout(WAIT).unwrap();
    assert!(t.elapsed() < Duration::from_secs(5));
    assert_eq!(reply.found, [(1, Some(5)), (2, None), (3, None)], "rows of the stuck batch stop waiting");
    o.send(job(2, vec![item(4, "mark-c")], Vec::new(), &seen));
    let (_, reply) = rx.recv_timeout(WAIT).unwrap();
    assert_eq!(reply.found, [(4, Some(5))]);
    assert_eq!(PROBES[2].made.load(Ordering::SeqCst), 2);
    PROBES[2].release.store(true, Ordering::SeqCst);
    assert!(rx.recv_timeout(Duration::from_millis(300)).is_err(), "the abandoned thread delivers nothing more");
}

#[test]
fn after_repeated_hangs_overlays_are_answered_empty_without_calling_handlers() {
    let (o, rx) = started::<Handlers<3>>(Duration::from_millis(100), BUDGET);
    let seen = shown(&[1, 2, 3]);
    for (id, key) in [(1, 1), (2, 2)] {
        o.send(job(id, vec![item(key, "hang")], Vec::new(), &seen));
        assert_eq!(rx.recv_timeout(WAIT).unwrap().1.found, [(key, None)]);
    }
    o.send(job(3, vec![item(3, "mark")], Vec::new(), &seen));
    let (_, reply) = rx.recv_timeout(WAIT).unwrap();
    assert_eq!(reply.found, [(3, None)]);
    assert_eq!(PROBES[3].asked.load(Ordering::SeqCst), 2);
    assert_eq!(PROBES[3].made.load(Ordering::SeqCst), 2);
    PROBES[3].release.store(true, Ordering::SeqCst);
}

#[test]
fn platforms_without_overlays_start_no_thread() {
    let (tx, rx) = crossbeam_channel::unbounded();
    let o = Overlays::start::<Handlers<4>>(Arc::new(move |w, r| drop(tx.send((w, r)))));
    o.send(job(1, vec![item(1, "mark")], Vec::new(), &shown(&[1])));
    assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());
    assert_eq!(PROBES[4].made.load(Ordering::SeqCst), 0);
}
