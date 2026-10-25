use super::*;
use flit_core::listing::attr;
use std::path::{Path, PathBuf};
use windows::Win32::UI::Shell::LSF_OVERRIDEEXISTING;

struct Temp(PathBuf);

impl Temp {
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!("flit-library-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Self(root)
    }

    fn at(&self, rel: &str) -> FsPath {
        FsPath::parse(self.0.join(rel).to_str().unwrap()).unwrap()
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn temp_library(dir: &Path, name: &str) -> FsPath {
    com_sta();
    unsafe {
        let lib: IShellLibrary = CoCreateInstance(&ShellLibrary, None, CLSCTX_INPROC_SERVER).unwrap();
        let dir = FsPath::parse(dir.to_str().unwrap()).unwrap();
        let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        path_of(&lib.Save(&item(&dir).unwrap(), PCWSTR(wide.as_ptr()), LSF_OVERRIDEEXISTING).unwrap()).unwrap()
    }
}

#[test]
fn a_temp_library_gains_and_loses_folders_and_lists_them_merged() {
    let t = Temp::new("ops");
    for d in ["one", "two", "libs"] {
        std::fs::create_dir_all(t.0.join(d)).unwrap();
    }
    std::fs::write(t.0.join(r"one\a.txt"), "a").unwrap();
    std::fs::create_dir(t.0.join(r"two\sub")).unwrap();
    let lib = temp_library(&t.0.join("libs"), "Test");
    assert!(flit_core::library::is_library(&lib));
    let (one, two) = (t.at("one"), t.at("two"));
    apply(&LibraryOp::Add { library: lib.clone(), folder: one.clone() }).unwrap();
    apply(&LibraryOp::Add { library: lib.clone(), folder: two.clone() }).unwrap();
    let read_back = read(&lib).unwrap();
    assert_eq!(read_back.folders, [one.clone(), two.clone()]);
    assert_eq!(read_back.name, "Test");
    apply(&LibraryOp::SetSave { library: lib.clone(), folder: two.clone() }).unwrap();
    assert_eq!(read(&lib).unwrap().save, Some(two.clone()));
    let mut go = |_: &ListingBuilder<'_>| true;
    let mut b = ListingBuilder::new(CaseMode::Insensitive, &mut go);
    NtFs.read_dir(&lib, &mut b).unwrap();
    let l = b.listing;
    let mut rows: Vec<(String, String, bool)> = (0..l.len() as u32).map(|i| (String::from_utf8_lossy(l.name(i)).into_owned(), String::from_utf8_lossy(library::folder(&l, i)).into_owned(), l.entries[i as usize].attrs & attr::DIR != 0)).collect();
    rows.sort();
    assert_eq!(rows, [("a.txt".to_owned(), one.display().into_owned(), false), ("sub".to_owned(), two.display().into_owned(), true)]);
    apply(&LibraryOp::Remove { library: lib.clone(), folder: one.clone() }).unwrap();
    assert_eq!(folders(&lib), [two]);
    assert!(apply(&LibraryOp::Add { library: t.at(r"libs\missing.library-ms"), folder: one }).is_err());
}

#[test]
fn restoring_defaults_writes_only_missing_classic_libraries_that_the_shell_can_read() {
    let t = Temp::new("restore");
    std::fs::write(t.0.join("Music.library-ms"), "kept").unwrap();
    assert_eq!(restore(&t.0, &DEFAULTS).unwrap(), 3);
    assert_eq!(std::fs::read_to_string(t.0.join("Music.library-ms")).unwrap(), "kept", "existing libraries are never overwritten");
    assert_eq!(restore(&t.0, &DEFAULTS).unwrap(), 0);
    let docs = read(&t.at("Documents.library-ms")).expect("the shell reads the restored library");
    assert_eq!(docs.folders.len(), 1);
    assert_eq!(docs.save.as_ref(), docs.folders.first());
}
