#![cfg(test)]

use flit_core::i18n::{self, Lang, S};
use flit_core::input::Event;
use flit_core::listing::{attr, DirSnapshot, Listing, Meta};
use flit_core::open::Route;
use flit_core::preview::{self, PreviewData};
use flit_core::provider::{Conflict, ProviderEvent, ProviderRequest};
use flit_core::sort::{self, SortKeys};
use flit_core::text::Gfx;
use flit_core::ui::{settings, App, Effect, Persisted};
use flit_core::{FsPath, Location};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const SIZE: (u32, u32) = (900, 600);
const ATLAS: u32 = 2048;
const SCENES: [&str; 5] = ["listing", "palette", "settings", "conflict", "inspector"];
const THEMES: [&str; 2] = ["dark", "light"];
const SAMPLE: &str = "field-notes.txt";
const MODIFIED: i64 = 133_801_632_000_000_000;

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("flit-gui-{}-{nonce}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn make_fixture(dir: &Path) {
    fs::create_dir(dir.join("Drafts")).unwrap();
    fs::write(dir.join(SAMPLE), "Field Notes\n\n01  Inspect the folder\n02  Review the files\n03  Archive the drafts\n").unwrap();
    fs::write(dir.join("inventory.csv"), "name,quantity\nnotebooks,4\n").unwrap();
    fs::write(dir.join("meeting.md"), "# Meeting\n\nPrepare the project summary.\n").unwrap();
    fs::write(dir.join("Drafts").join(SAMPLE), "Field Notes\n\nAn earlier draft.\n").unwrap();
}

fn meta(path: &Path) -> Meta {
    let md = fs::metadata(path).unwrap();
    Meta { size: if md.is_dir() { 0 } else { md.len() }, attrs: if md.is_dir() { attr::DIR } else { 0 }, modified: MODIFIED, created: MODIFIED }
}

fn load_listing(app: &mut App, dir: &Path) {
    let mut listed = false;
    for effect in app.drain_effects() {
        let Effect::Provider { req: ProviderRequest::List { gen, path, view, case, out, .. }, .. } = effect else { continue };
        let (mut listing, mut keys) = (Listing::default(), SortKeys::new(case));
        for entry in fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let name = entry.file_name().into_string().unwrap();
            let row = listing.push(name.as_bytes(), meta(&entry.path()));
            keys.push(listing.name(row));
        }
        let mut order = Vec::new();
        sort::sort(&listing, &keys, view, &mut order);
        out.publish(Arc::new(DirSnapshot { gen, path, listing, keys, order, view, done: true, error: None, elapsed: Duration::ZERO }));
        listed = true;
    }
    assert!(listed, "the folder must request a file listing");
    app.poll();
}

fn show_conflict(app: &mut App, dir: &Path, folder: &FsPath, now: Instant) {
    app.paste();
    let id = app.drain_effects().find_map(|effect| match effect {
        Effect::Provider { req: ProviderRequest::Op { id, .. }, .. } => Some(id),
        _ => None,
    }).expect("paste operation");
    let conflict = Conflict {
        source: folder.join(b"Drafts").join(SAMPLE.as_bytes()),
        target: folder.join(SAMPLE.as_bytes()),
        from: meta(&dir.join("Drafts").join(SAMPLE)),
        to: meta(&dir.join(SAMPLE)),
    };
    app.on_provider_event(ProviderEvent::OpConflict { tab: app.tab().id, id, conflict }, now);
}

fn show_inspector(app: &mut App, now: Instant) {
    let tree = app.accessibility();
    let row = tree.nodes.iter().find(|node| node.name == SAMPLE).expect("file row");
    app.on_event(&Event::A11ySelect(row.id), now);
    assert!(app.run_action("inspector.toggle", now));
}

#[cfg(windows)]
fn render_parts() -> (flit_platform_windows::D3dRenderer, flit_platform_windows::DWriteText) {
    use flit_platform::Renderer;
    use flit_platform_windows::{D3dRenderer, DWriteText};
    let dev = D3dRenderer::device(true).expect("create WARP device");
    (D3dRenderer::offscreen(&dev, SIZE, ATLAS).unwrap(), DWriteText::for_capture().unwrap())
}

#[cfg(target_os = "linux")]
fn render_parts() -> (flit_platform_linux_gui::GlRenderer, impl flit_core::text::TextBackend) {
    use flit_platform_linux_gui::{GlDevice, GlRenderer, Linux};
    let dev = GlDevice::headless().expect("create EGL device");
    assert!(dev.software, "snapshots require a software renderer");
    (GlRenderer::offscreen(&dev, SIZE, ATLAS).unwrap(), Linux::capture_text())
}

#[test]
#[ignore = "spawned by gui_snapshots with an isolated capture path"]
fn capture_frame() {
    let path = PathBuf::from(std::env::var_os("FLIT_HIDDEN_CAPTURE").expect("capture path"));
    let dir = PathBuf::from(std::env::var_os("FLIT_CAPTURE_FIXTURE").expect("fixture folder"));
    let scene = std::env::var("FLIT_CAPTURE_SCENE").unwrap();
    let theme = std::env::var("FLIT_CAPTURE_THEME").unwrap();
    let unix = cfg!(target_os = "linux");
    i18n::set_lang(Lang::En);
    i18n::set_unix_terms(unix);
    flit_core::ui::set_unix_defaults(unix);
    let folder = FsPath::parse(if unix { "/Fieldwork" } else { r"C:\Fieldwork" }).unwrap();
    let route = Route { folder: Location::from_local(folder.clone()), select: None };
    let now = Instant::now();
    let mut app = App::new(Persisted::default(), vec![route], 0, theme == "dark", 1.0, SIZE, now);
    app.set_caps(flit_core::caps::Caps { unix_paths: unix });
    let doc = settings::parse(&serde_json::json!({
        "appearance.system_scheme": false,
        "appearance.color_scheme": theme,
        "appearance.animations": false,
        "appearance.font": if unix { "" } else { "Segoe UI" },
        "appearance.inspector_font": if unix { "" } else { "Consolas" },
        "files.thumbnails": "never",
        "files.auto_sizes": false,
        "files.extensions": true
    }).to_string());
    assert!(doc.problems.is_empty(), "{:?}", doc.problems);
    app.apply_doc(&doc);
    assert_eq!(app.is_dark(), theme == "dark");
    app.set_clock(MODIFIED);
    app.set_date_patterns("yyyy-MM-dd".into(), "HH:mm".into());
    load_listing(&mut app, &dir);

    let (mut renderer, text) = render_parts();
    #[cfg(windows)]
    use flit_platform::Renderer;
    let mut gfx = Gfx::new(text, ATLAS);
    app.frame(&mut gfx, now);

    match scene.as_str() {
        "listing" => {
            assert!(app.accessibility().nodes.iter().any(|node| node.name == SAMPLE));
        }
        "palette" => {
            assert!(app.run_action("palette.show", now));
            for ch in "toggle".chars() {
                app.on_event(&Event::Char(ch), now);
            }
        }
        "settings" => assert!(app.run_action("options.open", now)),
        "conflict" => show_conflict(&mut app, &dir, &folder, now),
        "inspector" => show_inspector(&mut app, now),
        _ => panic!("unknown capture scene {scene}"),
    }

    app.frame(&mut gfx, now);
    if scene == "inspector" {
        let request = app.drain_effects().find_map(|effect| match effect {
            Effect::Preview(request) if request.path.file_name() == SAMPLE.as_bytes() => Some(request),
            _ => None,
        }).expect("selected text file should start a preview");
        let text = fs::read_to_string(dir.join(SAMPLE)).unwrap();
        app.on_preview(request.gen, PreviewData::Doc(preview::text_doc(&text, None)));
        app.frame(&mut gfx, now);
    }

    let tree = app.accessibility();
    let expected = match scene.as_str() {
        "listing" => SAMPLE,
        "palette" => S::PalettePlaceholderCommands.t(),
        "settings" => S::SettingsPageLineTitle.t(),
        "conflict" => S::UiConflictTitle.t(),
        "inspector" => S::A11yRegionPreview.t(),
        _ => unreachable!(),
    };
    assert!(tree.nodes.iter().any(|node| node.name == expected), "missing scene {scene} ({expected})");

    for (slot, offset) in &gfx.uploads.rects {
        let end = offset + (slot.w * slot.h * 4) as usize;
        renderer.upload(*slot, &gfx.uploads.data[*offset..end]);
    }
    renderer.render_capture(&gfx.dl, &path).expect("GPU frame readback");
}

struct Image {
    size: (u32, u32),
    rgba: Vec<u8>,
}

fn read_png(path: &Path) -> Result<Image, String> {
    let file = fs::File::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut decoder = png::Decoder::new(std::io::BufReader::new(file));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(|error| error.to_string())?;
    let mut pixels = vec![0; reader.output_buffer_size().ok_or("PNG is too large")?];
    let info = reader.next_frame(&mut pixels).map_err(|error| error.to_string())?;
    let pixels = &pixels[..info.buffer_size()];
    let rgba = match info.color_type {
        png::ColorType::Rgba => pixels.to_vec(),
        png::ColorType::Rgb => pixels.chunks_exact(3).flat_map(|px| [px[0], px[1], px[2], 255]).collect(),
        other => return Err(format!("unexpected PNG format {other:?}")),
    };
    Ok(Image { size: (info.width, info.height), rgba })
}

fn write_png(path: &Path, image: &Image) {
    let file = fs::File::create(path).unwrap();
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), image.size.0, image.size.1);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().unwrap();
    writer.write_image_data(&image.rgba).unwrap();
    writer.finish().unwrap();
}

fn compare(actual: &Image, golden: &Image) -> Result<(), String> {
    if actual.size != golden.size {
        return Err(format!("expected {:?}, got {:?}", golden.size, actual.size));
    }
    let mut total = 0u64;
    let mut changed = 0;
    for (actual, golden) in actual.rgba.chunks_exact(4).zip(golden.rgba.chunks_exact(4)) {
        let delta = std::array::from_fn::<_, 4, _>(|i| actual[i].abs_diff(golden[i]));
        total += delta.iter().map(|&x| u64::from(x)).sum::<u64>();
        changed += usize::from(delta.into_iter().any(|x| x > 24));
    }
    let mean = total as f64 / actual.rgba.len() as f64;
    let ratio = changed as f64 / (actual.rgba.len() / 4) as f64;
    if mean <= 0.35 && ratio <= 0.001 {
        Ok(())
    } else {
        Err(format!("mean channel error {mean:.3} (limit 0.35), {} affected pixels ({:.3}%, limit 0.1%)", changed, ratio * 100.0))
    }
}

#[test]
fn comparison_tolerates_subpixel_noise_and_rejects_missing_content() {
    let image = |rgba| Image { size: (100, 100), rgba };
    let golden = image(vec![200; 40_000]);
    let mut actual = image(golden.rgba.clone());
    actual.rgba[400..600].fill(199);
    assert!(compare(&actual, &golden).is_ok());
    actual.rgba[400..600].fill(0);
    assert!(compare(&actual, &golden).is_err());
    actual.size = (200, 50);
    assert!(compare(&actual, &golden).is_err());
}

fn capture_child(scene: &str, theme: &str, fixture: &Path, output: &Path) {
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "capture_frame", "--ignored", "--nocapture"])
        .env("FLIT_HIDDEN_CAPTURE", output)
        .env("FLIT_CAPTURE_FIXTURE", fixture)
        .env("FLIT_CAPTURE_SCENE", scene)
        .env("FLIT_CAPTURE_THEME", theme)
        .spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "capture {scene}-{theme} exited with {status}");
            return;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("timeout capturing {scene}-{theme}");
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

#[test]
#[cfg_attr(target_os = "linux", ignore = "requires Mesa software EGL and the Linux golden PNGs")]
fn gui_snapshots() {
    let scratch = Scratch::new();
    let fixture = scratch.0.join("fixture");
    fs::create_dir(&fixture).unwrap();
    make_fixture(&fixture);

    let output = std::env::var_os("FLIT_SNAPSHOT_OUTPUT").map_or_else(|| scratch.0.join("actual"), PathBuf::from);
    fs::create_dir_all(&output).unwrap();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("test/golden").join(std::env::consts::OS);
    let update = std::env::var_os("UPDATE_SNAPSHOTS").is_some_and(|v| v == "1");
    if update {
        fs::create_dir_all(&root).unwrap();
    }
    let mut failures = Vec::new();
    for theme in THEMES {
        for scene in SCENES {
            let name = format!("{scene}-{theme}.png");
            let (actual, golden) = (output.join(&name), root.join(&name));
            capture_child(scene, theme, &fixture, &actual);
            let image = read_png(&actual).unwrap();
            assert_eq!(image.size, SIZE, "{name}");
            if update {
                write_png(&golden, &image);
            } else if let Err(error) = read_png(&golden).and_then(|reference| compare(&image, &reference)) {
                failures.push(format!("{name}: {error}"));
            }
        }
    }
    assert!(failures.is_empty(), "{}\nRun with UPDATE_SNAPSHOTS=1 to regenerate the goldens. Set FLIT_SNAPSHOT_OUTPUT to keep actual PNGs.", failures.join("\n"));
}
