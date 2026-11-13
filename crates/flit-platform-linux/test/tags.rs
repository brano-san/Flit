use super::*;
use crate::fs::LinuxFs;
use crate::temp::Temp;
use flit_core::hash::FxHashMap;
use flit_core::listing::{attr, ListingBuilder};
use flit_core::sort::CaseMode;
use flit_core::tags::{TagStore, Tags};
use flit_platform::Fs;
use std::os::unix::fs::{symlink, MetadataExt};
use std::sync::Arc;

const SKIP: &str = "the temp folder is not on a local disk file system";

fn fs_path(p: &Path) -> FsPath {
    FsPath::parse_exact(p.as_os_str().as_bytes()).unwrap()
}

fn resolve(files: &[(FileKey, &Path)], trash: &[PathBuf]) -> Vec<Resolved> {
    let hints: Vec<FsPath> = files.iter().map(|f| fs_path(f.1)).collect();
    let pairs: Vec<(FileKey, &FsPath)> = files.iter().zip(&hints).map(|(f, h)| (f.0, h)).collect();
    resolve_with(&pairs, || (mounts::read().unwrap_or_default(), trash.to_vec()))
}

fn found_at(r: &Resolved) -> &[u8] {
    match r {
        Resolved::Found(p, _) => p.as_bytes(),
        other => panic!("{other:?}"),
    }
}

#[test]
fn birth_times_and_inode_generations_fill_the_high_half_of_a_key() {
    assert_eq!(key_id(42, 0, &[]), 42, "with nothing else to tell files apart the key is the inode");
    let born = 133_000_000_000_000_000;
    let (a, b, c) = (key_id(42, born, &[]), key_id(42, born + 1, &[]), key_id(42, born, &[42, 0, 0, 0, 7, 0, 0, 0]));
    assert!(a != b && a != c && b != c, "a reused inode born later or with a new generation is another file");
    assert_eq!((a as u32, b as u32, c as u32), (42, 42, 42), "the low half stays the inode for cheap directory scans");
    assert_eq!(key_id(42, born, &[]), a);
    assert_ne!(key_id(43, born, &[]), a);
    assert_ne!(key_id(42, 0, &[42, 0, 0, 0, 8, 0, 0, 0]), key_id(42, 0, &[42, 0, 0, 0, 9, 0, 0, 0]));
}

#[test]
fn keys_follow_renames_and_moves_and_resolve_back_to_paths() {
    let t = Temp::new("tags-keys");
    let a = t.file("a.txt", 3);
    std::fs::create_dir(t.path().join("sub")).unwrap();
    let Some(key) = file_key(&fs_path(&a)) else { return eprintln!("{SKIP}") };
    let dir = file_key(&fs_path(&t.path().join("sub"))).unwrap();
    let other = file_key(&fs_path(&t.file("other.txt", 1))).unwrap();
    symlink(&a, t.path().join("link")).unwrap();
    let link = file_key(&fs_path(&t.path().join("link"))).unwrap();
    assert!(key != dir && key != other && key != link, "different files have different keys");
    assert!([dir, other, link].iter().all(|k| k.volume == key.volume));
    let moved = t.path().join("sub").join("b.txt");
    std::fs::rename(&a, &moved).unwrap();
    assert_eq!(file_key(&fs_path(&moved)), Some(key), "a move within the file system keeps the key");
    std::fs::copy(&moved, t.path().join("copy.txt")).unwrap();
    assert_ne!(file_key(&fs_path(&t.path().join("copy.txt"))), Some(key), "a copy is a new file");
    let got = resolve(&[(key, &moved), (dir, &t.path().join("sub"))], &[]);
    assert!(matches!(&got[0], Resolved::Found(p, m) if p.as_bytes() == moved.as_os_str().as_bytes() && m.size == 3));
    assert!(matches!(&got[1], Resolved::Found(_, m) if m.attrs & attr::DIR != 0));
    let renamed = t.path().join("sub").join("renamed.txt");
    std::fs::rename(&moved, &renamed).unwrap();
    assert_eq!(found_at(&resolve(&[(key, &moved)], &[])[0]), renamed.as_os_str().as_bytes(), "a rename outside Flit is found next to the last known path");
    let trash = t.path().join("Trash").join("files");
    std::fs::create_dir_all(&trash).unwrap();
    std::fs::rename(&renamed, trash.join("renamed.txt")).unwrap();
    let r = resolve(&[(key, &renamed)], std::slice::from_ref(&trash));
    assert!(flit_core::recycle::in_bin(&FsPath::parse_exact(found_at(&r[0])).unwrap()), "a file trashed elsewhere is found in the Trash");
    std::fs::remove_file(trash.join("renamed.txt")).unwrap();
    assert_eq!(resolve(&[(key, &renamed)], &[trash]), [Resolved::Gone]);
    let elsewhere = FileKey { volume: key.volume ^ 0x5A5A_5A5A, id: key.id };
    assert_eq!(resolve(&[(elsewhere, Path::new("/nonexistent-flit-mount/x.txt"))], &[]), [Resolved::Unavailable], "a file system that is not mounted keeps its tags");
}

#[test]
fn a_new_file_on_a_reused_inode_does_not_inherit_tags() {
    let t = Temp::new("tags-reuse");
    let p = t.file("a.txt", 1);
    let Some(key) = file_key(&fs_path(&p)) else { return eprintln!("{SKIP}") };
    if node(&p, false).unwrap().btime == 0 {
        return eprintln!("the temp file system records no birth times");
    }
    let first = std::fs::symlink_metadata(&p).unwrap().ino();
    let mut reused = false;
    for _ in 0..50 {
        std::fs::remove_file(&p).unwrap();
        std::fs::write(&p, b"y").unwrap();
        assert_ne!(file_key(&fs_path(&p)), Some(key), "a new file under the old name is another file");
        reused = std::fs::symlink_metadata(&p).unwrap().ino() == first;
        if reused {
            break;
        }
    }
    assert_eq!(resolve(&[(key, &p)], &[]), [Resolved::Gone]);
    if !reused {
        eprintln!("the file system never reused the inode, so only distinct inodes were checked");
    }
}

#[test]
fn listings_carry_tags_across_renames() {
    let t = Temp::new("tags-listing");
    for n in ["a.txt", "b.txt", "c.txt"] {
        t.file(n, 1);
    }
    let b = fs_path(&t.path().join("b.txt"));
    let Some(key) = file_key(&b) else { return eprintln!("{SKIP}") };
    let mut store = TagStore::default();
    store.add("Work", 0).unwrap();
    store.update(key, b, |_| 0b10);
    let tags = Arc::new(Tags::new(1, &store, &FxHashMap::default()));
    let listed = || {
        let mut on_batch = |_: &ListingBuilder<'_>| true;
        let mut lb = ListingBuilder::new(CaseMode::Sensitive, &mut on_batch).with_tags(tags.clone());
        LinuxFs.read_dir(&fs_path(t.path()), &mut lb).unwrap();
        let l = &lb.listing;
        let mut got: Vec<(String, u32)> = (0..l.len() as u32).map(|i| (String::from_utf8_lossy(l.name(i)).into_owned(), l.entries[i as usize].tags)).collect();
        got.sort();
        got
    };
    assert_eq!(listed(), [("a.txt".into(), 0), ("b.txt".into(), 0b10), ("c.txt".into(), 0)]);
    std::fs::rename(t.path().join("b.txt"), t.path().join("B renamed.txt")).unwrap();
    assert_eq!(listed(), [("B renamed.txt".into(), 0b10), ("a.txt".into(), 0), ("c.txt".into(), 0)]);
}

#[test]
fn virtual_and_missing_places_are_not_taggable() {
    for p in ["/proc/self/status", "/proc", "/sys/kernel", "/dev/null"] {
        assert_eq!(file_key(&FsPath::parse(p).unwrap()), None, "{p}");
    }
    if Path::new("/dev/shm").is_dir() {
        assert_eq!(file_key(&FsPath::parse("/dev/shm").unwrap()), None, "tmpfs forgets its files at shutdown");
    }
    assert_eq!(file_key(&FsPath::parse("/tmp/flit-no-such-file-for-tags").unwrap()), None);
    assert_eq!(file_key(&FsPath::parse(r"C:\x.txt").unwrap()), None);
}
