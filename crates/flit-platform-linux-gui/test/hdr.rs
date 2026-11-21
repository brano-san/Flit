use super::*;
use std::path::Path;
use std::process::Command;
use std::sync::mpsc;

const REFERENCE: &str = "zscale=t=linear:npl=100,format=gbrpf32le,zscale=p=bt709,tonemap=tonemap=hable:desat=0,zscale=t=bt709:m=bt709:r=tv,format=rgb24";

#[test]
fn only_hlg_and_pq_request_conversion_to_sdr() {
    for gamma in [c"hlg", c"pq"] {
        let filter = color_filter(hdr_transfer(gamma)).unwrap().to_str().unwrap();
        assert!(filter.contains("t=linear:npl=100,format=gbrpf32le,zscale=p=bt709,tonemap=tonemap=hable"));
        assert!(filter.contains("zscale=t=bt709:m=bt709:r=tv"));
    }
    for gamma in [c"bt.1886", c"srgb", c"linear", c"auto", c""] {
        assert_eq!(color_filter(hdr_transfer(gamma)), None);
    }
}

fn preview(path: &Path, size: (u32, u32)) -> (Embedded, mpsc::Receiver<HostEvent>) {
    assert!(api().is_some(), "libmpv is required for this test");
    let (tx, events) = mpsc::channel();
    let start = Start { path: path.into(), video: true, gen: 1, volume: Some(0.0), area: Some(size), every: Duration::from_millis(16) };
    let player = Embedded::start(start, Arc::new(move |event| {
        let _ = tx.send(event);
    }));
    (player, events)
}

fn first_frame(path: &Path, size: (u32, u32), paused: bool) -> Bitmap {
    let (player, events) = preview(path, size);
    if paused {
        assert!(player.order(Order::Pause(true)));
    }
    let deadline = Instant::now() + Duration::from_secs(25);
    let frame = loop {
        let event = events.recv_timeout(deadline.saturating_duration_since(Instant::now())).expect("first video frame");
        let HostEvent::Frame(slot) = event else { continue };
        let (_, frame) = slot.take().expect("published frame");
        assert_eq!((frame.width, frame.height), size);
        break frame;
    };
    player.stop().unwrap().join().unwrap();
    frame
}

fn reference(path: &Path, hdr: bool, size: (u32, u32)) -> Vec<u8> {
    let (w, h) = size;
    let convert = if hdr { REFERENCE } else { "format=rgb24" };
    let output = Command::new("ffmpeg")
        .args(["-v", "error", "-threads", "2", "-filter_threads", "2", "-i"])
        .arg(path)
        .args(["-vf", &format!("{convert},scale={w}:{h}:flags=area"), "-frames:v", "1", "-pix_fmt", "rgb24", "-f", "rawvideo", "-threads", "1", "-"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(output.stdout.len(), w as usize * h as usize * 3);
    output.stdout
}

fn luminance(rgb: &[u8]) -> (f64, f64) {
    let (mut sum, mut bright) = (0.0, 0);
    for p in rgb.chunks_exact(3) {
        let y = 0.2126 * f64::from(p[0]) + 0.7152 * f64::from(p[1]) + 0.0722 * f64::from(p[2]);
        sum += y;
        bright += usize::from(y > 245.0);
    }
    let count = (rgb.len() / 3) as f64;
    (sum / count, 100.0 * bright as f64 / count)
}

#[test]
#[ignore = "needs libmpv, FFmpeg and FLIT_HDR_FIXTURES"]
fn preview_frames_match_the_reference_at_the_start_of_hdr_and_sdr_videos() {
    let Some(dir) = std::env::var_os("FLIT_HDR_FIXTURES") else { return };
    let mut failures = Vec::new();
    for (name, hdr, size) in [("user-hlg", true, (360, 720)), ("hlg", true, (640, 360)), ("hdr10", true, (640, 360)), ("sdr", false, (640, 360)), ("gray", true, (640, 360))] {
        let path = Path::new(&dir).join(format!("{name}.mp4"));
        let frame = first_frame(&path, size, true);
        let rgb: Vec<_> = frame.rgba.chunks_exact(4).flat_map(|p| &p[..3]).copied().collect();
        let expected = reference(&path, hdr, size);
        let (mean, bright) = luminance(&rgb);
        let (target, white) = luminance(&expected);
        let error = rgb.iter().zip(&expected).map(|(a, b)| f64::from(a.abs_diff(*b))).sum::<f64>() / rgb.len() as f64;
        println!("{name}: mean {mean:.3} / {target:.3}, bright % {bright:.3} / {white:.3}, RGB MAE {error:.3}");
        if let Some(out) = std::env::var_os("FLIT_HDR_CAPTURE_DIR") {
            let file = std::fs::File::create(Path::new(&out).join(format!("{name}.png"))).unwrap();
            let mut png = png::Encoder::new(file, size.0, size.1);
            png.set_color(png::ColorType::Rgb);
            png.set_depth(png::BitDepth::Eight);
            png.write_header().unwrap().write_image_data(&rgb).unwrap();
        }
        if (mean - target).abs() > 3.0 || (bright - white).abs() > 1.0 || error > 4.0 {
            failures.push(name);
        }
    }
    assert!(failures.is_empty(), "frames outside tolerance: {failures:?}");
}

#[test]
#[ignore = "needs libmpv, FFmpeg and FLIT_HDR_FIXTURES"]
fn phone_video_starts_playing_with_a_tone_mapped_frame() {
    let Some(dir) = std::env::var_os("FLIT_HDR_FIXTURES") else { return };
    let path = Path::new(&dir).join("user-hlg.mp4");
    let size = (360, 720);
    let frame = first_frame(&path, size, false);
    let rgb: Vec<_> = frame.rgba.chunks_exact(4).flat_map(|p| &p[..3]).copied().collect();
    let (mean, bright) = luminance(&rgb);
    let (target, white) = luminance(&reference(&path, true, size));
    assert!((mean - target).abs() < 4.0 && (bright - white).abs() < 2.0, "first playing frame: mean {mean}, bright {bright}%");
}
