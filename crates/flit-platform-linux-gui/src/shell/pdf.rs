use flit_core::preview::{pdf, PreviewRequest};
use flit_core::ui::Bitmap;
use std::ffi::OsString;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(3);
const MAX_INFO: u64 = 2 << 20;
const MAX_PNG: u64 = 4 << 20;
const INITIAL_PAGES: u32 = 128;

pub(super) fn document_command(path: &Path, last: u32) -> Vec<OsString> {
    ["pdfinfo", "-f", "1", "-l", &last.to_string()].into_iter().map(OsString::from)
        .chain([path.as_os_str().to_owned()]).collect()
}

pub(super) fn page_command(path: &Path, page: u32, size: u32) -> Vec<OsString> {
    let (page, size) = (page.to_string(), size.to_string());
    ["pdftoppm", "-png", "-f", &page, "-l", &page, "-singlefile", "-cropbox", "-scale-to", &size]
        .into_iter().map(OsString::from).chain([path.as_os_str().to_owned()]).collect()
}

fn output(args: &[OsString], max: u64, req: &PreviewRequest) -> Option<Vec<u8>> {
    let current = || req.cancel.is_current(req.gen);
    if !current() {
        return None;
    }
    let mut child = Command::new(&args[0]).args(&args[1..]).env("LC_ALL", "C")
        .stdin(Stdio::null()).stderr(Stdio::null()).stdout(Stdio::piped()).spawn().ok()?;
    let reader = child.stdout.take().and_then(|pipe| {
        std::thread::Builder::new().name("flit-poppler-output".into()).spawn(move || {
            let mut bytes = Vec::new();
            pipe.take(max + 1).read_to_end(&mut bytes).ok()?;
            (bytes.len() as u64 <= max).then_some(bytes)
        }).ok()
    });
    let deadline = Instant::now() + TIMEOUT;
    if let Some(reader) = reader {
        while current() && Instant::now() < deadline {
            match child.try_wait() {
                Ok(Some(status)) => return reader.join().ok()?.filter(|_| status.success()),
                Ok(None) => std::thread::sleep(Duration::from_millis(4)),
                Err(_) => break,
            }
        }
        let _ = child.kill();
        let _ = child.wait();
        let _ = reader.join();
    } else {
        let _ = child.kill();
        let _ = child.wait();
    }
    None
}

pub(super) fn document(path: &Path, req: &PreviewRequest) -> Option<Vec<Option<pdf::Size>>> {
    let sizes = |last| pdf::sizes(&output(&document_command(path, last), MAX_INFO, req)?);
    sizes(INITIAL_PAGES).or_else(|| sizes(1))
}

pub(super) fn page(path: &Path, req: &PreviewRequest, page: u32) -> Option<Bitmap> {
    let px = req.px.clamp(1, pdf::PAGE_PX);
    let image = crate::thumb::png_bytes(&output(&page_command(path, page, px), MAX_PNG, req)?)?;
    (image.width.max(image.height) <= px.saturating_add(1)).then(|| crate::thumb::fit(image, px))
}
