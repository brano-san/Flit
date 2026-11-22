use flit_core::preview::IMAGE_PX;
use flit_core::ui::Bitmap;
use flit_core::FsPath;
use resvg::{tiny_skia, usvg};
use std::ffi::OsStr;
use std::io::{Read, Write};
use std::path::Path;
use std::time::Duration;

pub(crate) const COMMAND: &str = "--preview-svg-worker";
const FILE_LIMIT: u64 = 2 << 20;
const EXPANDED_LIMIT: u64 = 16 << 20;
const MEMORY_LIMIT: usize = 256 << 20;

pub(super) fn load(path: &FsPath, size: u32, current: impl Fn() -> bool) -> Option<Bitmap> {
    let path = super::std_path(path);
    let size = size.clamp(1, IMAGE_PX);
    let (status, mut bytes) = super::child::run(
        COMMAND, &[path.as_os_str(), OsStr::new(&size.to_string())],
        u64::from(size) * u64::from(size) * 4 + 8, Duration::from_secs(2), Some(MEMORY_LIMIT), current,
    )?;
    if !status.success() {
        return None;
    }
    let width = u32::from_le_bytes(bytes.get(..4)?.try_into().ok()?);
    let height = u32::from_le_bytes(bytes.get(4..8)?.try_into().ok()?);
    if !(1..=size).contains(&width) || !(1..=size).contains(&height) || bytes.len() != (width * height * 4 + 8) as usize {
        return None;
    }
    bytes.drain(..8);
    Some(Bitmap { width, height, rgba: bytes })
}

pub(crate) fn serve(path: Option<&OsStr>, size: Option<&OsStr>) -> i32 {
    #[cfg(windows)]
    {
        let mut ready = [0];
        if std::io::stdin().lock().read_exact(&mut ready).is_err() || ready != [1] {
            return 1;
        }
    }
    fn write(image: Bitmap) -> std::io::Result<()> {
        let mut out = std::io::stdout().lock();
        out.write_all(&image.width.to_le_bytes())?;
        out.write_all(&image.height.to_le_bytes())?;
        out.write_all(&image.rgba)
    }
    let image = path.zip(size.and_then(OsStr::to_str).and_then(|s| s.parse().ok())).and_then(|(p, px)| render_file(Path::new(p), px));
    i32::from(!image.is_some_and(|b| write(b).is_ok()))
}

pub(super) fn render_file(path: &Path, size: u32) -> Option<Bitmap> {
    let file = super::regular_file(path)?;
    let meta = file.metadata().ok()?;
    if !meta.is_file() || meta.len() > FILE_LIMIT {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(FILE_LIMIT + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > FILE_LIMIT {
        return None;
    }
    if bytes.starts_with(&[0x1f, 0x8b]) {
        let mut expanded = Vec::new();
        flate2::read::MultiGzDecoder::new(bytes.as_slice()).take(EXPANDED_LIMIT + 1).read_to_end(&mut expanded).ok()?;
        if expanded.len() as u64 > EXPANDED_LIMIT {
            return None;
        }
        bytes = expanded;
    }
    let mut options = usvg::Options {
        max_depth: 64,
        image_href_resolver: usvg::ImageHrefResolver {
            resolve_data: Box::new(|_, _, _| None),
            resolve_string: Box::new(|_, _| None),
        },
        ..Default::default()
    };
    if bytes.windows(4).any(|b| b == b"text") {
        let family = if cfg!(windows) { "Arial" } else { "DejaVu Sans" };
        options.font_family = family.to_owned();
        let fonts = options.fontdb_mut();
        fonts.load_system_fonts();
        fonts.set_sans_serif_family(family);
        fonts.set_serif_family(if cfg!(windows) { "Times New Roman" } else { "DejaVu Serif" });
        fonts.set_monospace_family(if cfg!(windows) { "Consolas" } else { "DejaVu Sans Mono" });
    }
    let tree = usvg::Tree::from_str(std::str::from_utf8(&bytes).ok()?, &options).ok()?;
    let dimensions = tree.size();
    let size = size.clamp(1, IMAGE_PX);
    let scale = size as f32 / dimensions.width().max(dimensions.height());
    if !scale.is_finite() {
        return None;
    }
    let fit = |v: f32| ((v * scale).ceil() as u32).clamp(1, size);
    let (width, height) = (fit(dimensions.width()), fit(dimensions.height()));
    let mut pixmap = tiny_skia::Pixmap::new(width, height)?;
    resvg::render(&tree, tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    Some(Bitmap { width, height, rgba: pixmap.take() })
}
