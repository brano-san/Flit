use super::*;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const SYMLINK: u32 = 0xA000_000C;
const MOUNT_POINT: u32 = 0xA000_0003;
const CLOUD: u32 = 0x9000_001A;
const DEDUP: u32 = 0x8000_0013;

#[test]
fn only_name_surrogate_reparse_points_are_links() {
    let dir = FILE_ATTRIBUTE_DIRECTORY.0 | FILE_ATTRIBUTE_REPARSE_POINT.0;
    assert_eq!(map_attrs(dir, SYMLINK), attr::DIR | attr::LINK);
    assert_eq!(map_attrs(dir, MOUNT_POINT), attr::DIR | attr::LINK);
    assert_eq!(map_attrs(dir, CLOUD), attr::DIR);
    assert_eq!(map_attrs(FILE_ATTRIBUTE_REPARSE_POINT.0, DEDUP), 0);
    assert_eq!(map_attrs(FILE_ATTRIBUTE_REPARSE_POINT.0, SYMLINK), attr::LINK);
    assert_eq!(map_attrs(FILE_ATTRIBUTE_DIRECTORY.0, SYMLINK), attr::DIR, "EaSize is not a reparse tag without the reparse attribute");
    assert_eq!(map_attrs(FILE_ATTRIBUTE_HIDDEN.0 | FILE_ATTRIBUTE_SYSTEM.0 | FILE_ATTRIBUTE_READONLY.0, 0), attr::HIDDEN | attr::SYSTEM | attr::READONLY);
}

struct Temp(PathBuf);

impl Temp {
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!("flit-sizes-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Self(root)
    }
    fn dir(&self, rel: &str) {
        std::fs::create_dir_all(self.0.join(rel)).unwrap();
    }
    fn file(&self, rel: &str, len: usize) -> u64 {
        std::fs::write(self.0.join(rel), vec![b'x'; len]).unwrap();
        len as u64
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn fs_path(p: &Path) -> FsPath {
    FsPath::parse(p.to_str().unwrap()).unwrap()
}

fn junction(link: &Path, target: &Path) -> bool {
    std::process::Command::new("cmd").arg("/c").arg("mklink").arg("/J").arg(link).arg(target).output().is_ok_and(|o| o.status.success())
}

struct Listed;

impl Fs for Listed {
    fn read_dir(&self, dir: &FsPath, out: &mut ListingBuilder<'_>) -> Result<(), FsError> {
        NtFs.read_dir(dir, out)
    }
    fn seek_penalty(&self, path: &FsPath) -> Option<bool> {
        NtFs.seek_penalty(path)
    }
}

fn sizes(fs: &impl Fs, dir: &FsPath, items: &[(u32, Vec<u8>)], keep_going: &(dyn Fn() -> bool + Sync)) -> Vec<(u32, u64)> {
    let got = Mutex::new(Vec::new());
    fs.dir_sizes(dir, items, keep_going, &|idx, size| got.lock().unwrap().push((idx, size)));
    let mut got = got.into_inner().unwrap();
    got.sort();
    got
}

fn items(names: &[&str]) -> Vec<(u32, Vec<u8>)> {
    names.iter().enumerate().map(|(i, n)| (i as u32 * 10, n.as_bytes().to_vec())).collect()
}

#[test]
fn parallel_walk_matches_the_listing_walk_and_skips_junctions() {
    let t = Temp::new("eq");
    t.dir("outside");
    t.file("outside/big", 5000);
    t.dir("items/a/sub");
    let mut a = t.file("items/a/f1", 10) + t.file("items/a/f2", 20) + t.file("items/a/sub/f3", 30);
    let mut chain = String::from("items/a/sub");
    for k in 0..40 {
        chain.push_str(&format!("/d{k}"));
        t.dir(&chain);
        a += t.file(&format!("{chain}/leaf"), k + 1);
    }
    let linked = junction(&t.0.join("items/a/link"), &t.0.join("outside"));
    t.dir("items/b");
    let long = "n".repeat(120);
    let b: u64 = (0..600).map(|k| t.file(&format!("items/b/{long}{k:04}"), k % 7)).sum();
    t.dir("items/c");
    let dir = fs_path(&t.0.join("items"));
    let items = items(&["a", "b", "c", "missing"]);
    let fast = sizes(&NtFs, &dir, &items, &|| true);
    assert_eq!(fast, [(0, a), (10, b), (20, 0), (30, 0)], "junction created: {linked}");
    assert_eq!(fast, sizes(&Listed, &dir, &items, &|| true));
}

#[test]
fn cancelled_walk_stops_early() {
    let t = Temp::new("cancel");
    for a in 0..20 {
        for b in 0..30 {
            t.dir(&format!("tree/a{a}/b{b}"));
        }
    }
    let dir = fs_path(&t.0);
    let items = items(&["tree"]);
    let calls = AtomicUsize::new(0);
    sizes(&NtFs, &dir, &items, &|| calls.fetch_add(1, Ordering::Relaxed) < usize::MAX);
    let full = calls.swap(0, Ordering::Relaxed);
    sizes(&NtFs, &dir, &items, &|| calls.fetch_add(1, Ordering::Relaxed) < 10);
    let cancelled = calls.load(Ordering::Relaxed);
    assert!(cancelled * 4 < full, "{cancelled} checks after cancel vs {full} for the full walk");
}

#[test]
fn listing_a_file_reports_that_it_is_not_a_folder() {
    let t = Temp::new("notdir");
    t.file("plain.txt", 3);
    let mut go = |_: &ListingBuilder<'_>| true;
    let mut b = ListingBuilder::new(flit_core::sort::CaseMode::Insensitive, &mut go);
    let err = NtFs.read_dir(&fs_path(&t.0.join("plain.txt")), &mut b).unwrap_err();
    assert_eq!(err.kind, FsErrorKind::NotADirectory);
    let err = NtFs.read_dir(&fs_path(&t.0.join("missing")), &mut b).unwrap_err();
    assert_eq!(err.kind, FsErrorKind::NotFound);
}

#[test]
fn alternate_data_streams_are_listed_only_on_request() {
    let t = Temp::new("streams");
    t.file("a.txt", 3);
    t.dir("sub");
    std::fs::write(t.0.join("a.txt:meta"), b"12345").unwrap();
    let list = |streams: bool| {
        let mut go = |_: &ListingBuilder<'_>| true;
        let mut b = ListingBuilder::new(flit_core::sort::CaseMode::Insensitive, &mut go).with_streams(streams);
        NtFs.read_dir(&fs_path(&t.0), &mut b).unwrap();
        let l = b.listing;
        let mut out: Vec<(String, u64, bool)> = (0..l.len() as u32).map(|i| (String::from_utf8_lossy(l.name(i)).into_owned(), l.entries[i as usize].size, l.entries[i as usize].attrs & attr::STREAM != 0)).collect();
        out.sort();
        out
    };
    assert_eq!(list(false), [("a.txt".to_owned(), 3, false), ("sub".to_owned(), 0, false)]);
    assert_eq!(list(true), [("a.txt".to_owned(), 3, false), ("a.txt:meta".to_owned(), 5, true), ("sub".to_owned(), 0, false)]);
}

#[test]
fn names_the_file_system_of_a_path() {
    let fs = NtFs.file_system(&FsPath::parse(&std::env::temp_dir().to_string_lossy()).unwrap()).unwrap();
    assert!(["NTFS", "ReFS", "FAT32", "exFAT"].contains(&fs.as_str()), "{fs}");
}

#[test]
fn large_folders_listed_in_parallel_chunks_keep_every_entry_once() {
    let t = Temp::new("parallel-list");
    let names: Vec<String> = (0..12_000).map(|k| format!("{k} {}.txt", "Длинное имя файла для проверки параллельного чтения ".repeat(3))).collect();
    for (k, n) in names.iter().enumerate() {
        t.file(n, k % 7);
    }
    let dir = fs_path(&t.0);
    let mut calls = 0;
    let mut go = |_: &ListingBuilder<'_>| {
        calls += 1;
        true
    };
    let mut b = ListingBuilder::new(flit_core::sort::CaseMode::Insensitive, &mut go);
    NtFs.read_dir(&dir, &mut b).unwrap();
    let l = &b.listing;
    let mut got: Vec<(String, u64)> = (0..l.len() as u32).map(|i| (String::from_utf8(l.name(i).to_vec()).unwrap(), l.entries[i as usize].size)).collect();
    got.sort();
    let mut want: Vec<(String, u64)> = names.iter().enumerate().map(|(k, n)| (n.clone(), (k % 7) as u64)).collect();
    want.sort();
    assert_eq!(got, want);
    for i in 0..l.len() as u32 {
        let mut key = Vec::new();
        flit_core::sort::encode_key(l.name(i), flit_core::sort::CaseMode::Insensitive, &mut key);
        assert_eq!(b.keys.key(i), key.as_slice());
    }
    drop(b);
    assert!(calls > SERIAL_BUFFERS, "{calls} batches");

    let mut seen = 0;
    let mut stop = |b: &ListingBuilder<'_>| {
        seen = b.listing.len();
        seen < 10_000
    };
    let mut b = ListingBuilder::new(flit_core::sort::CaseMode::Insensitive, &mut stop);
    NtFs.read_dir(&dir, &mut b).unwrap();
    assert!(b.listing.len() < names.len(), "a stopped listing ends early");
}
