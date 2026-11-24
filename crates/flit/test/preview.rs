use super::*;
use flit_core::provider::GenToken;
use flit_core::ui::{Bitmap, IconRequest};
use flit_platform::PlatformError;

struct Fake;

impl Shell for Fake {
    fn new() -> Result<Self, PlatformError> {
        Ok(Fake)
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
    fn preview_handler(&mut self, ext: &[u8]) -> Option<String> {
        ext.eq_ignore_ascii_case(b"docx").then(|| "{handler}".into())
    }

    fn preview_image(&mut self, path: &FsPath, _: u32) -> Option<Bitmap> {
        (path.file_name() == b"fallback.dng").then(|| Bitmap { width: 1, height: 1, rgba: vec![0, 0, 255, 255] })
    }
}

struct Dir(std::path::PathBuf);

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn temp() -> Dir {
    let d = std::env::temp_dir().join(format!("flit-preview-{}-{:?}", std::process::id(), std::thread::current().id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    Dir(d)
}

fn req(dir: &Dir, name: &str, details: bool) -> PreviewRequest {
    let p = dir.0.join(name);
    let size = std::fs::metadata(&p).map_or(0, |m| m.len());
    let path = FsPath::parse(p.to_str().unwrap()).unwrap();
    let kind = if p.is_dir() { Kind::Folder } else { preview::kind(p.extension().map_or(b"", |e| e.as_encoded_bytes())) };
    let cancel = GenToken::default();
    cancel.set(7);
    PreviewRequest { gen: 7, cancel, path, kind, size, details, px: 512 }
}

#[test]
fn pretty_formats_structured_files_and_preserves_mixed_xml_content() {
    let d = temp();
    for (name, input, part) in [
        ("data.json", r#"{"b":123456789012345678901,"a":[1,2],"a":3}"#, r#"  "b": 123456789012345678901,"#),
        ("data.yaml", "list:\n -  a\n -   b\n", "  - a\n  - b"),
        ("data.toml", "count=3 # keep\nnames=[1,2]\n", "count = 3 # keep\nnames = [1, 2]"),
        ("data.xml", "<root><mixed>a <em>b</em> c</mixed><empty/></root>", "  <mixed>a <em>b</em> c</mixed>\n  <empty/>"),
    ] {
        let file = d.0.join(name);
        std::fs::write(&file, input).unwrap();
        assert!(super::pretty::format_file(&file).unwrap().contains(part), "{name}");
        assert_eq!(std::fs::read_to_string(file).unwrap(), input);
    }
    let file = d.0.join("encoded.json");
    let source = r#"{"file":"пример"}"#;
    let utf8 = [b"\xef\xbb\xbf".as_slice(), source.as_bytes()].concat();
    let utf16 = [0xff, 0xfe].into_iter().chain(source.encode_utf16().flat_map(u16::to_le_bytes)).collect::<Vec<_>>();
    for bytes in [utf8, utf16] {
        std::fs::write(&file, bytes).unwrap();
        assert_eq!(super::pretty::format_file(&file).unwrap(), "{\n  \"file\": \"пример\"\n}\n");
    }
}

#[test]
fn pretty_refuses_invalid_formats_and_files_over_its_read_limit() {
    let d = temp();
    for (name, input) in [("bad.json", "[1,"), ("bad.yaml", "items: ["), ("bad.toml", "items = ["), ("bad.xml", "<a><b></a>")] {
        let p = d.0.join(name);
        std::fs::write(&p, input).unwrap();
        assert_eq!(super::pretty::format_file(&p), Err(preview::PrettyError::Invalid), "{name}");
    }
    let p = d.0.join("big.json");
    std::fs::File::create(&p).unwrap().set_len(preview::PRETTY_FILE_LIMIT + 1).unwrap();
    assert_eq!(super::pretty::format_file(&p), Err(preview::PrettyError::TooLarge));
}

#[test]
fn svg_renders_shapes_but_never_loads_a_local_image_href() {
    let d = temp();
    let outside = d.0.join("outside.svg");
    std::fs::write(&outside, r#"<svg xmlns="http://www.w3.org/2000/svg" width="32" height="32"><rect width="32" height="32" fill="red"/></svg>"#).unwrap();
    let source = d.0.join("source.svg");
    let href = quick_xml::escape::escape(outside.to_string_lossy());
    std::fs::write(&source, format!(r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="32" height="32"><image xlink:href="{href}" width="32" height="32"/><rect width="4" height="4" fill="lime"/></svg>"#)).unwrap();
    let image = super::svg::render_file(&source, 32).unwrap();
    assert_eq!((image.width, image.height), (32, 32));
    assert_eq!(&image.rgba[..4], &[0, 255, 0, 255]);
    assert_eq!(&image.rgba[(16 * 32 + 16) * 4..][..4], &[0, 0, 0, 0]);
}

#[test]
fn svg_refuses_an_oversized_source_and_caps_the_raster_dimensions() {
    let d = temp();
    let source = d.0.join("source.svg");
    std::fs::write(&source, r#"<svg xmlns="http://www.w3.org/2000/svg" width="10000" height="1000"><rect width="10000" height="1000"/></svg>"#).unwrap();
    let image = super::svg::render_file(&source, 200).unwrap();
    assert_eq!((image.width, image.height), (200, 20));
    std::fs::write(&source, r#"<svg xmlns="http://www.w3.org/2000/svg" width="2082" height="100"><rect width="2082" height="100"/></svg>"#).unwrap();
    assert_eq!(super::svg::render_file(&source, 1600).unwrap().width, 1600);
    std::fs::File::options().write(true).open(&source).unwrap().set_len(3 << 20).unwrap();
    assert!(super::svg::render_file(&source, 200).is_none());
}

#[test]
fn review_svg_rasterizes_a_small_icon_at_each_requested_size() {
    let d = temp();
    let path = d.0.join("icon.svg");
    std::fs::write(&path, r#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="8"><rect width="16" height="8" fill="red"/></svg>"#).unwrap();
    for (px, expected) in [(256, (256, 128)), (1024, (1024, 512)), (u32::MAX, (1600, 800))] {
        let image = super::svg::render_file(&path, px).unwrap();
        assert_eq!((image.width, image.height), expected);
        assert_eq!(&image.rgba[image.rgba.len() - 4..], &[255, 0, 0, 255]);
    }
}

#[test]
fn review_svg_gzip_enforces_the_expanded_limit_and_loads_fonts() {
    use std::io::Write;
    let d = temp();
    let path = d.0.join("drawing.svgz");
    let compress = |svg: &[u8]| {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(svg).unwrap();
        let bytes = encoder.finish().unwrap();
        assert!(bytes.len() < 2 << 20);
        std::fs::write(&path, bytes).unwrap();
    };
    let mut svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="80" height="32"><rect width="2" height="2" fill="red"/>"#.to_vec();
    svg.resize(16 << 20, b' ');
    svg.extend_from_slice(b"</svg>");
    compress(&svg);
    assert!(super::svg::render_file(&path, 80).is_none());
    svg.truncate((16 << 20) - b"</svg>".len());
    svg.extend_from_slice(b"</svg>");
    compress(&svg);
    assert!(super::svg::render_file(&path, 80).is_some());

    let text = br#"<svg xmlns="http://www.w3.org/2000/svg" width="80" height="32"><text x="2" y="28" font-size="26">Flit</text></svg>"#;
    let plain = d.0.join("drawing.svg");
    std::fs::write(&plain, text).unwrap();
    compress(text);
    let rendered = super::svg::render_file(&plain, 80).unwrap();
    assert!(rendered.rgba.chunks_exact(4).any(|p| p[3] != 0));
    assert_eq!(super::svg::render_file(&path, 80).unwrap().rgba, rendered.rgba);
    std::fs::write(&path, b"\x1f\x8b\x08").unwrap();
    assert!(super::svg::render_file(&path, 80).is_none());
}

#[cfg(windows)]
#[test]
fn review_svg_worker_job_enforces_a_per_process_memory_limit() {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::JobObjects::{JobObjectExtendedLimitInformation, QueryInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_PROCESS_MEMORY};

    let bytes = 256 << 20;
    let job = super::child::memory_job(bytes).unwrap();
    let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    unsafe {
        QueryInformationJobObject(Some(HANDLE(job.as_raw_handle())), JobObjectExtendedLimitInformation, (&raw mut info).cast(), size_of_val(&info) as u32, None).unwrap();
    }
    assert_eq!(info.BasicLimitInformation.LimitFlags & JOB_OBJECT_LIMIT_PROCESS_MEMORY, JOB_OBJECT_LIMIT_PROCESS_MEMORY);
    assert_eq!(info.ProcessMemoryLimit, bytes);
}

#[cfg(target_os = "linux")]
#[test]
fn review_svg_worker_limits_its_linux_address_space_before_exec() {
    let mut child = std::process::Command::new("sh");
    child.args(["-c", "ulimit -v"]);
    let bytes = 256 << 20;
    super::child::limit_address_space(&mut child, bytes);
    let output = child.output().unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8(output.stdout).unwrap().trim().parse::<usize>().unwrap() <= bytes / 1024);
}

#[test]
fn review_raw_inspector_falls_back_to_the_windows_image_decoder_for_regular_files() {
    let d = temp();
    let path = d.0.join("fallback.dng");
    std::fs::write(&path, b"no embedded JPEG").unwrap();
    let result = load(&mut Fake, &req(&d, "fallback.dng", false));
    if cfg!(windows) {
        assert!(matches!(result, Some(PreviewData::Image(image)) if image.rgba == [0, 0, 255, 255]));
    } else {
        assert!(matches!(result, Some(PreviewData::Basic { poster: None, .. })));
    }
    std::fs::write(d.0.join("unsupported.dng"), b"no embedded JPEG").unwrap();
    assert!(matches!(load(&mut Fake, &req(&d, "unsupported.dng", false)), Some(PreviewData::Basic { poster: None, .. })));
    std::fs::remove_file(path).unwrap();
    assert!(matches!(load(&mut Fake, &req(&d, "fallback.dng", false)), Some(PreviewData::Basic { poster: None, .. })));
}

#[test]
fn raw_preview_reads_the_embedded_jpeg_at_its_tiff_offset() {
    use std::io::{Seek, SeekFrom, Write};
    let d = temp();
    let path = d.0.join("photo.DNG");
    let jpeg = include_bytes!("fixtures/raw-preview.jpg");
    let offset = 32 << 20;
    let mut file = std::fs::File::create(&path).unwrap();
    file.write_all(&raw_ifd(offset, jpeg.len() as u32, 0)).unwrap();
    file.seek(SeekFrom::Start(u64::from(offset))).unwrap();
    file.write_all(jpeg).unwrap();
    drop(file);
    let Some(PreviewData::Image(b)) = load(&mut Fake, &req(&d, "photo.DNG", false)) else {
        panic!("the embedded JPEG should be previewed");
    };
    assert_eq!((b.width, b.height, b.rgba.len()), (16, 8, 512));
    assert!(b.rgba[2] > b.rgba[1] && b.rgba[1] > b.rgba[0] && b.rgba[3] == 255);
}

#[test]
fn review_raw_thumbnail_uses_the_smallest_adequate_jpeg_and_inspector_uses_the_largest() {
    use flit_core::ui::IconKind;
    let d = temp();
    let small = include_bytes!("fixtures/raw-preview.jpg");
    let large = include_bytes!("fixtures/raw-preview-large.jpg");
    let small_at = 68;
    let large_at = small_at + small.len() as u32;
    let mut data = raw_ifd(small_at, small.len() as u32, 38);
    data.extend_from_slice(&raw_ifd(large_at, large.len() as u32, 0)[8..]);
    data.extend_from_slice(small);
    data.extend_from_slice(large);
    std::fs::write(d.0.join("camera.dng"), data).unwrap();
    let mut request = req(&d, "camera.dng", false);
    let mut thumb = IconRequest { key: 1, kind: IconKind::Thumbnail, path: request.path.clone(), size: 16, gen: 1, row: 0 };
    let small_image = thumbnail(&mut Fake, &thumb, || true).unwrap();
    assert!(small_image.rgba[2] > small_image.rgba[0]);
    thumb.size = 256;
    let large_image = thumbnail(&mut Fake, &thumb, || true).unwrap();
    assert_eq!((large_image.width, large_image.height), (256, 128));
    assert!(large_image.rgba[0] > large_image.rgba[2]);
    thumb.size = 512;
    assert_eq!(thumbnail(&mut Fake, &thumb, || true).unwrap().width, 320);
    request.px = 16;
    assert!(matches!(load(&mut Fake, &request), Some(PreviewData::Image(b)) if b.rgba[0] > b.rgba[2]));
}

#[test]
fn raw_preview_rejects_truncated_cyclic_and_out_of_range_tiff_catalogs() {
    use std::io::Cursor;
    let mut huge = raw_ifd(38, 512, 0);
    huge[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
    for bytes in [raw_ifd(38, 512, 0)[..22].to_vec(), raw_ifd(38, 512, 8), raw_ifd(u32::MAX, u32::MAX, 0), huge] {
        let result = super::raw::locations(&mut Cursor::new(&bytes), bytes.len() as u64, || true);
        assert!(result.is_none_or(|(_, candidates)| candidates.is_empty()));
    }
}

fn raw_ifd(jpeg_at: u32, jpeg_len: u32, next: u32) -> Vec<u8> {
    let mut bytes = b"II\x2a\x00\x08\x00\x00\x00\x02\x00".to_vec();
    for (tag, value) in [(513u16, jpeg_at), (514, jpeg_len)] {
        bytes.extend_from_slice(&tag.to_le_bytes());
        bytes.extend_from_slice(&4u16.to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes.extend_from_slice(&next.to_le_bytes());
    bytes
}

#[test]
fn loads_by_kind_within_limits() {
    let d = temp();
    std::fs::write(d.0.join("a.rs"), "fn main() {}\n").unwrap();
    std::fs::write(d.0.join("big.txt"), "x".repeat(200_000)).unwrap();
    std::fs::write(d.0.join("notes.cfg"), "key = value\n").unwrap();
    std::fs::write(d.0.join("blob.bin"), [0u8, 1, 2, 3, 0, 9]).unwrap();
    std::fs::write(d.0.join("fake.ts"), [0x47u8, 0, 0, 0x10]).unwrap();
    std::fs::write(d.0.join("doc.docx"), "PK").unwrap();
    std::fs::create_dir(d.0.join("sub")).unwrap();
    let mut s = Fake;
    assert!(matches!(load(&mut s, &req(&d, "a.rs", false)), Some(PreviewData::Doc(doc)) if doc.text.starts_with("fn main")));
    match load(&mut s, &req(&d, "big.txt", false)) {
        Some(PreviewData::Doc(doc)) => assert!(doc.truncated && doc.text.len() == preview::TEXT_CHARS),
        _ => panic!("big text"),
    }
    assert!(matches!(load(&mut s, &req(&d, "notes.cfg", false)), Some(PreviewData::Doc(_))));
    assert!(matches!(load(&mut s, &req(&d, "blob.bin", false)), Some(PreviewData::Basic { .. })));
    assert!(matches!(load(&mut s, &req(&d, "fake.ts", false)), Some(PreviewData::Basic { .. })));
    assert!(matches!(load(&mut s, &req(&d, "doc.docx", false)), Some(PreviewData::Handler(h)) if h == "{handler}"));
    assert!(matches!(load(&mut s, &req(&d, "a.rs", true)), Some(PreviewData::Props(p)) if p.is_empty()));
    match load(&mut s, &req(&d, "", false)) {
        Some(PreviewData::Folder(f)) => assert_eq!((f.dirs, f.files, f.complete), (1, 6, true)),
        _ => panic!("folder summary"),
    }
    let stale = req(&d, "a.rs", false);
    stale.cancel.set(8);
    assert!(load(&mut s, &stale).is_none());
}

#[test]
fn archive_preview_uses_basic_details_for_time_size_and_password_limits() {
    use parse_process::Error;
    let d = temp();
    let req = req(&d, "archive.7z", false);
    let mut shell = Fake;
    assert!(matches!(archive_preview(&mut shell, &req, Err(Error::TooLarge)), Some(PreviewData::Basic { note: Some(Note::TooLarge), .. })));
    for error in [Error::TimedOut, Error::PasswordRequired] {
        assert!(matches!(archive_preview(&mut shell, &req, Err(error)), Some(PreviewData::Basic { note: None, .. })));
    }
    let response = |names: &[u8]| [parse_process::ARCHIVE_HEADER, names].concat();
    for broken in [Err(Error::Failed), Ok(response(&[255, b'\n'])), Ok(response(b"truncated")), Ok(b"unframed\n".to_vec())] {
        assert!(matches!(archive_preview(&mut shell, &req, broken), Some(PreviewData::Basic { note: Some(Note::Message(_)), .. })));
    }
    assert!(archive_preview(&mut shell, &req, Err(Error::Cancelled)).is_none());
    assert!(matches!(archive_preview(&mut shell, &req, Ok(response(b"sample.txt\n"))), Some(PreviewData::Doc(_))));
}
