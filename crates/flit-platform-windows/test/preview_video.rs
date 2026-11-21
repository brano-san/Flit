use super::*;
use std::path::{Path, PathBuf};
use std::process::Command;
use windows::Win32::Foundation::S_OK;
use windows::Win32::Graphics::Gdi::{CreateCompatibleDC, CreateDIBSection, DeleteDC, GdiFlush, SelectObject, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS};
use windows::Win32::Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS};
use windows::Win32::System::Com::CoUninitialize;

const WIDTH: usize = 640;
const HEIGHT: usize = 360;
const TONEMAP: &str = "zscale=t=linear:npl=100,format=gbrpf32le,zscale=p=bt709,tonemap=tonemap=hable:desat=0,zscale=t=bt709:m=bt709:r=tv,format=bgra";

struct MediaThread;

impl MediaThread {
    unsafe fn start() -> Self {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok().unwrap();
        let thread = Self;
        start_mf().unwrap();
        thread
    }
}

impl Drop for MediaThread {
    fn drop(&mut self) {
        unsafe {
            if MF.replace(false) {
                MFShutdown().unwrap();
            }
            CoUninitialize();
        }
    }
}

struct TestEngine(IMFMediaEngine);

impl Drop for TestEngine {
    fn drop(&mut self) {
        unsafe { let _ = self.0.Shutdown(); }
    }
}

struct TestWindow(HWND);

impl TestWindow {
    unsafe fn new() -> Self {
        let class = w!("FlitPreviewVideoTest");
        register(class, Some(host_proc));
        Self(CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE, class, class, WS_POPUP | WS_VISIBLE, 40, 40, WIDTH as i32, HEIGHT as i32, None, None, Some(hinst()), None).unwrap())
    }
}

impl Drop for TestWindow {
    fn drop(&mut self) {
        forget(self.0);
        unsafe { let _ = DestroyWindow(self.0); }
    }
}

unsafe fn wait_for<T>(description: &str, mut probe: impl FnMut() -> Option<T>) -> T {
    let until = Instant::now() + Duration::from_secs(15);
    loop {
        let mut msg = MSG::default();
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        if let Some(result) = probe() {
            return result;
        }
        assert!(Instant::now() < until, "timeout waiting for {description}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn fixture(clip: &str) -> PathBuf {
    let dir = std::env::var_os("FLIT_HDR_FIXTURES").expect("set FLIT_HDR_FIXTURES to the directory containing hdr10.mp4, hlg.mp4 and sdr.mp4");
    PathBuf::from(dir).join(format!("{clip}.mp4"))
}

fn reference(clip: &str) -> Vec<u8> {
    let mut ffmpeg = Command::new("ffmpeg");
    ffmpeg.args(["-v", "error", "-threads", "2", "-filter_threads", "2", "-i"]).arg(fixture(clip));
    if clip != "sdr" {
        ffmpeg.args(["-vf", TONEMAP]);
    }
    let output = ffmpeg.args(["-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "bgra", "pipe:1"]).output().unwrap();
    assert!(output.status.success(), "ffmpeg: {}", String::from_utf8_lossy(&output.stderr));
    output.stdout
}

fn brightness(bytes: &[u8]) -> (f64, f64) {
    assert_eq!(bytes.len(), WIDTH * HEIGHT * 4);
    let mut sum = 0.0;
    let mut white = 0;
    for pixel in bytes.chunks_exact(4) {
        let y = 0.0722 * f64::from(pixel[0]) + 0.7152 * f64::from(pixel[1]) + 0.2126 * f64::from(pixel[2]);
        sum += y;
        white += usize::from(y > 245.0);
    }
    let count = (WIDTH * HEIGHT) as f64;
    (sum / count, white as f64 / count)
}

fn compare(clip: &str, frame: &[u8], expected: &[u8]) {
    let (mean, white) = brightness(frame);
    let (reference_mean, reference_white) = brightness(expected);
    let total_error: u64 = frame.chunks_exact(4).zip(expected.chunks_exact(4)).map(|(a, b)| {
        (0..3).map(|i| u64::from(a[i].abs_diff(b[i]))).sum::<u64>()
    }).sum();
    let rgb_error = total_error as f64 / (WIDTH * HEIGHT * 3) as f64;
    eprintln!("{clip}: mean {mean:.3}/{reference_mean:.3}, white {white:.5}/{reference_white:.5}, RGB MAE {rgb_error:.3}");
    assert!((mean - reference_mean).abs() <= 10.0, "{clip}: mean {mean:.3}, reference {reference_mean:.3}");
    assert!((white - reference_white).abs() <= 0.02, "{clip}: white {white:.5}, reference {reference_white:.5}");
    assert!(rgb_error <= 7.0, "{clip}: RGB MAE {rgb_error:.3}");
}

unsafe fn transferred_frame(device: &ID3D11Device, engine: &IMFMediaEngine) -> windows_core::Result<Vec<u8>> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: WIDTH as u32, Height: HEIGHT as u32, MipLevels: 1, ArraySize: 1, Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 }, BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32, ..Default::default()
    };
    let mut target = None;
    device.CreateTexture2D(&desc, None, Some(&mut target))?;
    let target = target.unwrap();
    let rc = RECT { right: WIDTH as i32, bottom: HEIGHT as i32, ..Default::default() };
    engine.TransferVideoFrame(&target, None, &rc, None)?;
    let staging_desc = D3D11_TEXTURE2D_DESC { Usage: D3D11_USAGE_STAGING, BindFlags: 0, CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32, ..desc };
    let mut staging = None;
    device.CreateTexture2D(&staging_desc, None, Some(&mut staging))?;
    let staging = staging.unwrap();
    let ctx = device.GetImmediateContext()?;
    ctx.CopyResource(&staging, &target);
    let mut pixels = Vec::with_capacity(WIDTH * HEIGHT * 4);
    let mut mapping = D3D11_MAPPED_SUBRESOURCE::default();
    ctx.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapping))?;
    for row in 0..HEIGHT {
        let start = mapping.pData.cast::<u8>().add(row * mapping.RowPitch as usize);
        pixels.extend_from_slice(std::slice::from_raw_parts(start, WIDTH * 4));
    }
    ctx.Unmap(&staging, 0);
    Ok(pixels)
}

#[test]
#[ignore = "requires D3D11, HEVC, ffmpeg with zscale and FLIT_HDR_FIXTURES"]
fn transferred_preview_matches_reference() {
    unsafe {
        let _thread = MediaThread::start();
        let gpu = video_device().unwrap();
        for clip in ["hdr10", "hlg", "sdr"] {
            let path = FsPath::parse(&fixture(clip).to_string_lossy()).unwrap();
            let expected = reference(clip);
            let player = TestEngine(create_engine(None, true, Some(&gpu), hdr_color(&path)).unwrap());
            let engine = &player.0;
            load_media(engine, &path, 0.25).unwrap();
            assert!((engine.GetVolume() - 0.25).abs() < 0.0001);
            engine.Load().unwrap();
            engine.Play().unwrap();
            wait_for(clip, || {
                if let Ok(error) = engine.GetError() {
                    panic!("{clip}: media error {}, {:?}", error.GetErrorCode(), error.GetExtendedErrorCode());
                }
                if engine.GetReadyState() < 2 { return None; }
                let mut pts = 0;
                let hr = (engine.vtable().OnVideoStreamTick)(engine.as_raw(), &mut pts);
                hr.ok().unwrap();
                (hr == S_OK).then_some(())
            });
            let pixels = transferred_frame(&gpu.0, engine).unwrap();
            drop(player);
            compare(clip, &pixels, &expected);
        }
    }
}

unsafe fn window_frame(hwnd: HWND) -> Vec<u8> {
    let dc = CreateCompatibleDC(None);
    assert!(!dc.is_invalid());
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER { biSize: size_of::<BITMAPINFOHEADER>() as u32, biWidth: WIDTH as i32, biHeight: -(HEIGHT as i32), biPlanes: 1, biBitCount: 32, ..Default::default() },
        ..Default::default()
    };
    let mut bits = std::ptr::null_mut();
    let dib = CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0).unwrap();
    let old = SelectObject(dc, HGDIOBJ(dib.0));
    let printed = PrintWindow(hwnd, dc, PRINT_WINDOW_FLAGS(PW_RENDERFULLCONTENT)).as_bool();
    let _ = GdiFlush();
    let pixels = std::slice::from_raw_parts(bits.cast::<u8>(), WIDTH * HEIGHT * 4).to_vec();
    SelectObject(dc, old);
    let _ = DeleteObject(HGDIOBJ(dib.0));
    let _ = DeleteDC(dc);
    assert!(printed);
    pixels
}

unsafe fn wait_event(hwnd: HWND, description: &str, mut condition: impl FnMut(HostEvent) -> bool) {
    wait_for(description, || {
        take(hwnd).into_iter().any(|event| {
            if let HostEvent::Failed { message, .. } = &event { panic!("{description}: {message}"); }
            condition(event)
        }).then_some(())
    });
}

struct ClipCopy(PathBuf);

impl ClipCopy {
    fn new(source: &Path) -> Self {
        let name = format!("flit-hdr-release-{}.mp4", std::process::id());
        let path = source.with_file_name(name);
        let mut output = std::fs::File::create_new(&path).unwrap();
        let copy = Self(path);
        std::io::copy(&mut std::fs::File::open(source).unwrap(), &mut output).unwrap();
        copy
    }
}

impl Drop for ClipCopy {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[test]
#[ignore = "requires a desktop, D3D11, HEVC, ffmpeg with zscale and FLIT_HDR_FIXTURES"]
fn hosted_preview_matches_reference_and_releases_the_playing_file() {
    unsafe {
        let _thread = MediaThread::start();
        for clip in ["hdr10", "hlg", "sdr"] {
            let expected = reference(clip);
            let file = fixture(clip);
            let copy = (clip == "hdr10").then(|| ClipCopy::new(&file));
            let window = TestWindow::new();
            let hwnd = window.0;
            let path = FsPath::parse(&copy.as_ref().map_or(&file, |c| &c.0).to_string_lossy()).unwrap();
            let rect = Some(Rect::new(0.0, 0.0, WIDTH as f32, HEIGHT as f32));
            command(hwnd, HostCmd::Show { gen: 1, what: Hosted::Media { video: true }, path: path.clone(), rect, volume: 0.0, bg: 0 });
            wait_event(hwnd, clip, |ev| matches!(ev, HostEvent::Media { duration, .. } if duration > 0.0));
            command(hwnd, HostCmd::Place(rect));
            let child = existing(hwnd).unwrap();
            let frame = wait_for("first visible video frame", || {
                let pixels = window_frame(child);
                pixels.chunks_exact(4).step_by(100).any(|p| p[0].abs_diff(p[1]) > 30).then_some(pixels)
            });
            if let Some(dir) = std::env::var_os("FLIT_HDR_CAPTURE_DIR") {
                std::fs::write(PathBuf::from(dir).join(format!("{clip}-window.bgra")), &frame).unwrap();
            }
            compare(clip, &frame, &expected);
            if clip == "hdr10" {
                command(hwnd, HostCmd::Media(MediaCmd::Volume(0.25)));
                command(hwnd, HostCmd::Media(MediaCmd::Play));
                wait_event(hwnd, "play", |ev| matches!(ev, HostEvent::Media { playing: true, position, .. } if position > 0.0));
                command(hwnd, HostCmd::Media(MediaCmd::Pause));
                wait_event(hwnd, "pause", |ev| matches!(ev, HostEvent::Media { playing: false, .. }));
                command(hwnd, HostCmd::Media(MediaCmd::Seek(2.0)));
                wait_event(hwnd, "seek", |ev| matches!(ev, HostEvent::Media { playing: false, position, .. } if (position - 2.0).abs() < 0.1));
                command(hwnd, HostCmd::Media(MediaCmd::Play));
                wait_event(hwnd, "end", |ev| matches!(ev, HostEvent::Media { playing: false, duration, position, .. } if duration > 0.0 && duration == position));
                command(hwnd, HostCmd::Media(MediaCmd::Play));
                wait_event(hwnd, "replay", |ev| matches!(ev, HostEvent::Media { playing: true, position, .. } if position < 1.5));
            }
            std::thread::scope(|scope| {
                let worker = scope.spawn(move || release(vec![path]));
                wait_for("file operation", || worker.is_finished().then_some(()));
                worker.join().unwrap();
            });
            if let Some(mut copy) = copy {
                let renamed = copy.0.with_extension("renamed.mp4");
                std::fs::rename(&copy.0, &renamed).unwrap();
                copy.0 = renamed;
                std::fs::remove_file(&copy.0).unwrap();
            }
            wait_event(hwnd, "release", |ev| matches!(ev, HostEvent::Released { gen: 1 }));
        }
    }
}
