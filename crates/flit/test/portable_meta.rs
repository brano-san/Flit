use super::*;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let folder = std::env::temp_dir().join(format!("flit-meta-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir(&folder).unwrap();
        Self(folder.canonicalize().unwrap())
    }
    fn item(&self, name: &str) -> FsPath {
        let path = self.0.join(name);
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            FsPath::from_utf16(&path.as_os_str().encode_wide().collect::<Vec<_>>()).unwrap()
        }
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            FsPath::parse_exact(path.as_os_str().as_bytes()).unwrap()
        }
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn users_can_save_metadata_export_it_as_xmp_and_import_it_to_another_file() {
    let temp = Temp::new();
    let path = temp.item("beach.jpg");
    let other = temp.item("forest.jpg");
    let sidecar = temp.item("beach.xmp");
    for image in [&path, &other] {
        fs::write(std_path(image), b"the same file contents").unwrap();
    }
    let empty = Metadata::default();
    let metadata = Metadata { rating: 9, comment: "Blue & green <water> '☀'".into(), tags: vec!["Travel".into(), "Лето".into()] };
    assert_eq!(request(Request::Store { path: path.clone(), before: empty.clone(), after: metadata.clone() }).unwrap(), metadata);
    request(Request::Export { path, sidecar: sidecar.clone(), value: metadata.clone() }).unwrap();
    assert_eq!(request(Request::Import { path: other.clone(), sidecar, before: empty }).unwrap(), metadata);
    assert_eq!(read(&other).unwrap(), metadata);
    assert_eq!(fs::read(std_path(&other)).unwrap(), b"the same file contents");
}

#[test]
fn an_invalid_or_existing_sidecar_leaves_user_data_in_place() {
    let temp = Temp::new();
    let path = temp.item("beach.jpg");
    let sidecar = temp.item("beach.xmp");
    fs::write(std_path(&path), b"data").unwrap();
    let malformed = "<!DOCTYPE x [<!ENTITY e SYSTEM \"urn:flit:test\">]><x>&e;</x>";
    fs::write(std_path(&sidecar), malformed).unwrap();
    let empty = Metadata::default();
    assert!(request(Request::Import { path: path.clone(), sidecar: sidecar.clone(), before: empty.clone() }).is_err());
    assert!(request(Request::Export { path: path.clone(), sidecar: sidecar.clone(), value: empty.clone() }).is_err());
    assert_eq!(read(&path).unwrap(), empty);
    assert_eq!(fs::read_to_string(std_path(&sidecar)).unwrap(), malformed);
}

#[test]
fn importing_an_xmp_rating_does_not_clear_other_fields() {
    let before = Metadata { rating: 7, comment: "An existing caption".into(), tags: vec!["Beach".into()] };
    let packet = r#"<r:Description xmlns:r="http://www.w3.org/1999/02/22-rdf-syntax-ns#" xmlns:x="http://ns.adobe.com/xap/1.0/" x:Rating="5"/>"#;
    let applied = xmp::read(packet, &before).unwrap();
    assert_eq!(applied, Metadata { rating: 10, ..before });
}

#[cfg(windows)]
#[test]
fn saving_a_rating_preserves_the_files_last_write_time() {
    let temp = Temp::new();
    let path = temp.item("rated.jpg");
    fs::write(std_path(&path), b"contents").unwrap();
    let file = fs::OpenOptions::new().write(true).open(std_path(&path)).unwrap();
    file.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000)).unwrap();
    let modified = file.metadata().unwrap().modified().unwrap();
    drop(file);
    let after = Metadata { rating: 8, ..Metadata::default() };
    request(Request::Store { path: path.clone(), before: Metadata::default(), after: after.clone() }).unwrap();
    assert_eq!(fs::metadata(std_path(&path)).unwrap().modified().unwrap(), modified);
    assert_eq!(read(&path).unwrap(), after);
}

#[cfg(windows)]
#[test]
fn editing_a_large_metadata_history_compacts_it_and_rejects_stale_edits() {
    let temp = Temp::new();
    let path = temp.item("history.jpg");
    fs::write(std_path(&path), b"contents").unwrap();
    let stream = std_path(&temp.item("history.jpg:flit.metadata"));
    let before = Metadata { rating: 3, ..Metadata::default() };
    let after = Metadata { rating: 9, ..Metadata::default() };
    let line = format!("{}\n", serde_json::to_string(&before).unwrap());
    fs::write(&stream, line.repeat(3 * 1024 * 1024 / line.len())).unwrap();
    request(Request::Store { path: path.clone(), before: before.clone(), after: after.clone() }).unwrap();
    assert_eq!(read(&path).unwrap(), after);
    let record = format!("{}\n", serde_json::to_string(&after).unwrap());
    assert_eq!(fs::metadata(&stream).unwrap().len(), record.len() as u64);
    assert_eq!(fs::read_to_string(&stream).unwrap(), record);
    assert!(request(Request::Store { path: path.clone(), before, after: Metadata::default() }).is_err());
    assert_eq!(read(&path).unwrap(), after);
}
