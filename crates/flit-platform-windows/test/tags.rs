use super::*;
use crate::fs::NtFs;
use flit_core::hash::FxHashMap;
use flit_core::listing::{attr, ListingBuilder};
use flit_core::sort::CaseMode;
use flit_core::tags::{TagStore, Tags};
use flit_platform::Fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

struct Temp(PathBuf);

impl Temp {
    fn new(tag: &str) -> Self {
        let root = crate::test_temp().join(format!("flit-tags-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Self(root)
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

#[test]
fn keys_follow_moves_and_resolve_back_to_paths() {
    let t = Temp::new("keys");
    std::fs::create_dir_all(t.0.join("sub")).unwrap();
    std::fs::write(t.0.join("a.txt"), b"abc").unwrap();
    let Some(key) = file_key(&fs_path(&t.0.join("a.txt"))) else { return eprintln!("the temp folder is not on a local NTFS drive") };
    let dir = file_key(&fs_path(&t.0.join("sub"))).unwrap();
    assert_ne!(key, dir);
    std::fs::rename(t.0.join("a.txt"), t.0.join("sub").join("b.txt")).unwrap();
    assert_eq!(file_key(&fs_path(&t.0.join("sub").join("b.txt"))), Some(key), "a move within the volume keeps the key");
    let missing = FileKey { volume: key.volume, id: (0x7FFF << 48) | 0xFFFF_FFF0 };
    let elsewhere = FileKey { volume: key.volume ^ 0x5A5A_5A5A, id: key.id };
    let got = resolve_files(&[key, dir, missing, elsewhere]);
    let Resolved::Found(path, meta) = &got[0] else { panic!("{:?}", got[0]) };
    assert_eq!((path.file_name(), meta.size, file_key(path)), (&b"b.txt"[..], 3, Some(key)));
    assert!(matches!(&got[1], Resolved::Found(p, m) if p.file_name() == b"sub" && m.attrs & attr::DIR != 0));
    assert_eq!(got[2..], [Resolved::Gone, Resolved::Unavailable]);
    std::fs::remove_file(t.0.join("sub").join("b.txt")).unwrap();
    assert_eq!(resolve_files(&[key]), [Resolved::Gone]);
    assert_eq!(file_key(&FsPath::parse(r"\\server\share\x.txt").unwrap()), None);
    assert_eq!(file_key(&fs_path(&t.0.join("nothing"))), None);
}

#[test]
fn listings_carry_the_tags_of_their_entries() {
    let t = Temp::new("listing");
    for n in ["a.txt", "b.txt", "c.txt"] {
        std::fs::write(t.0.join(n), b"x").unwrap();
    }
    let b = fs_path(&t.0.join("b.txt"));
    let Some(key) = file_key(&b) else { return eprintln!("the temp folder is not on a local NTFS drive") };
    let mut store = TagStore::default();
    store.add("Work", 0).unwrap();
    store.update(key, b, |_| 0b10);
    let mut on_batch = |_: &ListingBuilder<'_>| true;
    let mut lb = ListingBuilder::new(CaseMode::Insensitive, &mut on_batch).with_tags(Arc::new(Tags::new(1, &store, &FxHashMap::default())));
    NtFs.read_dir(&fs_path(&t.0), &mut lb).unwrap();
    let l = &lb.listing;
    let mut got: Vec<(String, u32)> = (0..l.len() as u32).map(|i| (String::from_utf8_lossy(l.name(i)).into_owned(), l.entries[i as usize].tags)).collect();
    got.sort();
    assert_eq!(got, [("a.txt".into(), 0), ("b.txt".into(), 0b10), ("c.txt".into(), 0)]);
}
