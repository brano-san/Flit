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
