use super::*;
use crate::player::frame_interval;
use std::sync::atomic::AtomicUsize;
use std::sync::mpsc;

const WAIT: Duration = Duration::from_secs(5);

#[test]
fn the_frame_buffer_keeps_the_video_shape_inside_the_pane_and_never_exceeds_720p() {
    assert_eq!(frame_size(Some((320, 240)), Some((800, 600))), (800, 600), "a small video is scaled up to the pane by mpv");
    assert_eq!(frame_size(Some((1920, 1080)), Some((1000, 1000))), (1000, 563));
    assert_eq!(frame_size(Some((1920, 1080)), Some((4000, 3000))), MAX_FRAME, "a huge pane on a dense screen is capped");
    assert_eq!(frame_size(Some((1080, 1920)), Some((4000, 4000))), (405, 720), "an upright video is capped by its height");
    assert_eq!(frame_size(None, Some((500, 300))), (500, 300), "before mpv knows the video the pane is filled");
    assert_eq!(frame_size(None, Some((2000, 1000))), (1280, 640));
}

#[test]
fn a_missing_or_empty_pane_or_video_size_still_gives_a_small_valid_buffer() {
    assert_eq!(frame_size(Some((320, 240)), None), (480, 360));
    assert_eq!(frame_size(Some((320, 240)), Some((0, 600))), (480, 360));
    assert_eq!(frame_size(Some((0, 0)), None), (640, 360));
    assert_eq!(frame_size(Some((1, 100_000)), Some((1, 1))), (1, 1));
    assert_eq!(frame_size(Some((100_000, 1)), Some((4000, 4000))), (1280, 1));
}

#[test]
fn rows_are_aligned_for_fast_copies_and_frames_come_out_opaque() {
    assert_eq!(stride(320), 1280);
    assert_eq!(stride(321), 1344);
    assert_eq!(stride(1), 64);
    let mut pixels = vec![7u8; 64 * 2];
    pixels[..8].copy_from_slice(&[1, 2, 3, 0, 4, 5, 6, 9]);
    let rgba = to_rgba(&pixels, (2, 2), 64);
    assert_eq!(rgba.len(), 16, "the padding after each row is dropped");
    assert_eq!(&rgba[..8], &[1, 2, 3, 255, 4, 5, 6, 255]);
    assert!(rgba.chunks_exact(4).all(|p| p[3] == 255));
    assert_eq!(to_rgba(&[1, 2, 3, 4], (2, 2), 64).len(), 16, "a short buffer never panics");
}

#[test]
fn the_library_is_asked_to_draw_into_the_preview_not_its_own_window() {
    let video = options(true, Some(0.5));
    let get = |all: &[(&str, String)], k: &str| all.iter().find(|(key, _)| *key == k).map(|(_, v)| v.clone());
    assert_eq!(get(&video, "vo").as_deref(), Some("libmpv"));
    assert_eq!(get(&video, "volume").as_deref(), Some("50"));
    assert_eq!(get(&video, "config").as_deref(), Some("no"), "the user's mpv.conf cannot open a window or bind keys");
    assert_eq!(get(&video, "vid"), None);
    let audio = options(false, None);
    assert_eq!(get(&audio, "vid").as_deref(), Some("no"), "audio plays without any video output");
    assert_eq!(get(&audio, "vo").as_deref(), Some("null"));
    assert_eq!(get(&audio, "volume"), None, "without a known volume mpv keeps its own");
}

#[derive(Clone, Default)]
struct Fake {
    ready: Arc<AtomicUsize>,
    drawn: Arc<Mutex<Vec<(u32, u32)>>>,
    fill: Arc<AtomicUsize>,
}

impl Surface for Fake {
    fn wants_frame(&mut self) -> bool {
        self.ready.swap(0, Ordering::AcqRel) > 0
    }

    fn draw(&mut self, size: (u32, u32), stride: usize, pixels: &mut [u8]) -> bool {
        assert!((pixels.as_ptr() as usize).is_multiple_of(64));
        assert!(stride.is_multiple_of(64) && pixels.len() >= stride * size.1 as usize);
        pixels.fill(self.fill.load(Ordering::Acquire) as u8);
        lock(&self.drawn).push(size);
        true
    }
}

struct Painter {
    canvas: Arc<Canvas>,
    fake: Fake,
    events: mpsc::Receiver<HostEvent>,
    frames: Frames,
    done: mpsc::Receiver<()>,
}

impl Painter {
    fn start(size: (u32, u32), every: Duration) -> Self {
        let canvas = Arc::new(Canvas::new(size));
        let fake = Fake::default();
        let frames = Frames::default();
        let (tx, events) = mpsc::channel();
        let (finished, done) = mpsc::channel();
        let (c, mut f, slot) = (canvas.clone(), fake.clone(), frames.clone());
        std::thread::spawn(move || {
            paint(&c, &mut f, 9, &slot, every, &move |e| {
                let _ = tx.send(e);
            });
            let _ = finished.send(());
        });
        Self { canvas, fake, events, frames, done }
    }

    fn new_frame(&self, fill: u8) {
        self.fake.fill.store(fill.into(), Ordering::Release);
        self.fake.ready.store(1, Ordering::Release);
        self.canvas.poke();
    }

    fn drawn(&self, n: usize) -> Vec<(u32, u32)> {
        let until = Instant::now() + WAIT;
        while lock(&self.fake.drawn).len() < n && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(2));
        }
        lock(&self.fake.drawn).clone()
    }

    fn ends_on_stop(&self) -> bool {
        self.canvas.stop();
        self.done.recv_timeout(WAIT).is_ok()
    }
}

#[test]
fn the_render_thread_hands_over_frames_of_the_wanted_size_and_ends_on_stop() {
    let p = Painter::start((320, 180), Duration::ZERO);
    p.new_frame(40);
    assert!(matches!(p.events.recv_timeout(WAIT), Ok(HostEvent::Frame(f)) if f == p.frames));
    let (gen, frame) = p.frames.take().unwrap();
    assert_eq!((gen, frame.width, frame.height), (9, 320, 180));
    assert_eq!(&frame.rgba[..4], &[40, 40, 40, 255]);
    p.canvas.resize((64, 36));
    assert!(matches!(p.events.recv_timeout(WAIT), Ok(HostEvent::Frame(_))), "a resized pane gets the current picture again");
    assert_eq!(p.frames.take().map(|(_, b)| (b.width, b.height)), Some((64, 36)));
    assert!(p.ends_on_stop());
}

#[test]
fn nothing_is_drawn_while_the_video_has_no_new_frame() {
    let p = Painter::start((320, 180), Duration::ZERO);
    p.canvas.poke();
    p.canvas.resize((100, 50));
    std::thread::sleep(Duration::from_millis(50));
    assert!(p.drawn(0).is_empty(), "a wake-up without a decoded frame and a resize before the first frame cost nothing");
    assert!(p.events.try_recv().is_err());
    assert!(p.ends_on_stop(), "an idle render thread still ends on stop");
}

#[test]
fn a_frame_the_screen_has_not_taken_yet_is_replaced_by_the_next() {
    let p = Painter::start((32, 16), Duration::ZERO);
    p.new_frame(1);
    assert_eq!(p.drawn(1).len(), 1);
    p.new_frame(2);
    assert_eq!(p.drawn(2).len(), 2);
    assert!(p.events.recv_timeout(WAIT).is_ok());
    assert!(p.events.recv_timeout(Duration::from_millis(50)).is_err(), "only one wake-up waits for the screen");
    assert_eq!(p.frames.take().map(|(_, b)| b.rgba[0]), Some(2), "the newest frame is the one shown");
    assert!(p.frames.take().is_none());
    assert!(p.ends_on_stop());
}

#[test]
fn frames_wait_for_the_screen_refresh_and_stop_cuts_the_wait_short() {
    let every = Duration::from_millis(150);
    let p = Painter::start((32, 16), every);
    p.new_frame(1);
    assert_eq!(p.drawn(1).len(), 1);
    let first = Instant::now();
    p.new_frame(2);
    assert_eq!(p.drawn(2).len(), 2);
    assert!(first.elapsed() >= every - Duration::from_millis(20), "two frames are never closer than one screen refresh");
    let slow = Painter::start((32, 16), Duration::from_secs(600));
    slow.new_frame(1);
    assert_eq!(slow.drawn(1).len(), 1);
    slow.new_frame(2);
    std::thread::sleep(Duration::from_millis(20));
    let asked = Instant::now();
    assert!(slow.ends_on_stop());
    assert!(asked.elapsed() < Duration::from_secs(2), "closing the file does not wait for the next refresh");
    assert_eq!(slow.drawn(1).len(), 1);
    assert!(p.ends_on_stop());
}

#[test]
fn closing_the_last_window_waits_a_bounded_time_for_players_still_shutting_down() {
    assert!(settle(Duration::ZERO), "with nothing playing the exit does not wait");
    let (release, held) = mpsc::channel::<()>();
    lock(&RUNNING).push(std::thread::spawn(move || {
        let _ = held.recv();
    }));
    let asked = Instant::now();
    assert!(!settle(Duration::from_millis(60)), "a stuck player cannot keep Flit from exiting");
    assert!(asked.elapsed() < Duration::from_secs(2));
    drop(release);
    assert!(settle(WAIT));
    assert!(lock(&RUNNING).is_empty(), "finished players are forgotten");
}

fn live(video: bool, area: (u32, u32)) -> Option<(Embedded, mpsc::Receiver<HostEvent>)> {
    let path = std::env::var_os("FLIT_MPV_SAMPLE")?;
    let (tx, rx) = mpsc::channel();
    let report: Report = Arc::new(move |e| {
        let _ = tx.send(e);
    });
    let start = Start { path: path.into(), video, gen: 5, volume: Some(0.0), area: Some(area), every: frame_interval(None) };
    Some((Embedded::start(start, report), rx))
}

fn threads_named(prefix: &str) -> usize {
    std::fs::read_dir("/proc/self/task").map(|d| d.flatten().filter(|t| std::fs::read_to_string(t.path().join("comm")).is_ok_and(|c| c.trim().starts_with(prefix))).count()).unwrap_or(0)
}

#[test]
#[ignore = "needs libmpv and a sample video in FLIT_MPV_SAMPLE"]
fn libmpv_plays_a_video_into_frames_of_the_pane_size_and_reports_the_end() {
    let (player, events) = live(true, (160, 120)).expect("FLIT_MPV_SAMPLE");
    assert!(loaded(), "libmpv is present");
    let (mut sizes, mut end, mut colors) = (Vec::new(), None, 0);
    let until = Instant::now() + Duration::from_secs(20);
    while end.is_none() && Instant::now() < until {
        match events.recv_timeout(Duration::from_secs(1)) {
            Ok(HostEvent::Frame(f)) => {
                if let Some((5, b)) = f.take() {
                    colors = colors.max(b.rgba.chunks_exact(4).map(|p| [p[0], p[1], p[2]]).collect::<std::collections::BTreeSet<_>>().len());
                    sizes.push((b.width, b.height));
                }
            }
            Ok(HostEvent::Media { position, duration, playing: false, .. }) if duration > 0.0 && position >= duration => end = Some((position, duration)),
            _ => {}
        }
    }
    println!("live: {} frames, sizes {:?}, up to {colors} colors in a frame, end {:?}", sizes.len(), sizes.iter().collect::<std::collections::BTreeSet<_>>(), end);
    assert!(sizes.len() >= 10, "a two-second clip gives a steady stream of frames");
    assert!(colors > 8, "the frames hold the picture, not a blank surface");
    assert!(sizes.iter().all(|&s| s == (160, 120)), "every frame has the pane's size and the clip's 4:3 shape");
    let (_, duration) = end.expect("the end is reported");
    assert!((duration - 2.0).abs() < 0.2);
    let thread = player.stop().expect("player thread");
    let asked = Instant::now();
    thread.join().unwrap();
    assert!(asked.elapsed() < WAIT);
    assert_eq!(threads_named("flit-mpv"), 0, "no player thread is left behind");
}

#[test]
#[ignore = "needs libmpv and a sample video in FLIT_MPV_SAMPLE"]
fn leaving_the_file_mid_video_ends_both_player_threads() {
    let (player, events) = live(true, (320, 240)).expect("FLIT_MPV_SAMPLE");
    let first = loop {
        match events.recv_timeout(WAIT) {
            Ok(HostEvent::Frame(f)) => break f,
            Ok(_) => {}
            Err(e) => panic!("no frame: {e}"),
        }
    };
    assert_eq!(first.take().map(|(_, b)| (b.width, b.height)), Some((320, 240)));
    player.place((200, 100));
    let resized = (0..200).find_map(|_| match events.recv_timeout(WAIT) {
        Ok(HostEvent::Frame(f)) => f.take().map(|(_, b)| (b.width, b.height)).filter(|&s| s != (320, 240)),
        _ => None,
    });
    assert_eq!(resized, Some((133, 100)), "a narrower pane gets narrower frames of the same shape");
    assert!(player.order(Order::Pause(true)));
    assert!(player.order(Order::Seek(0.5)));
    assert!(player.order(Order::Volume(30.0)));
    let asked = Instant::now();
    crate::player::Player::Embedded(player).stop();
    assert!(asked.elapsed() < Duration::from_millis(50), "leaving the file never waits for the player");
    assert!(settle(WAIT), "the player threads end on their own");
    println!("live: stopped mid-video in {:?}", asked.elapsed());
    assert_eq!(threads_named("flit-mpv"), 0);
}

#[test]
#[ignore = "needs libmpv and a sample video in FLIT_MPV_SAMPLE"]
fn audio_through_libmpv_draws_no_frames_and_still_ends() {
    let (player, events) = live(false, (320, 240)).expect("FLIT_MPV_SAMPLE");
    let mut frames = 0;
    let started = Instant::now();
    let ended = loop {
        match events.recv_timeout(Duration::from_secs(20)) {
            Ok(HostEvent::Frame(_)) => frames += 1,
            Ok(HostEvent::Media { position, duration, playing: false, .. }) => break Some((position, duration)),
            Ok(_) => {}
            Err(_) => break None,
        }
    };
    println!("live: audio-only ended {ended:?} after {:?} with {frames} frames", started.elapsed());
    assert_eq!(frames, 0, "audio never draws into the preview");
    let (position, duration) = ended.expect("the end is reported");
    assert!((duration - 2.0).abs() < 0.2 && position == duration, "the bar ends full after the whole sound");
    assert!(!player.order(Order::Pause(false)), "an ended player is replaced on the next Play");
    player.stop().expect("player thread").join().unwrap();
    assert_eq!(threads_named("flit-mpv"), 0);
}
