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
