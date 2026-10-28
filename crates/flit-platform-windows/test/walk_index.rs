use super::*;
use flit_search::index::VolumeIndex;
use std::time::Instant;

fn record(action: FILE_ACTION, path: &str, id: i64, parent: i64, attrs: u32, next: u32) -> Vec<u8> {
    let wide: Vec<u16> = path.encode_utf16().collect();
    let mut r = vec![0u8; offset_of!(Record, FileName)];
    let mut put = |at: usize, bytes: &[u8]| r[at..at + bytes.len()].copy_from_slice(bytes);
    put(offset_of!(Record, NextEntryOffset), &next.to_le_bytes());
    put(offset_of!(Record, Action), &action.0.to_le_bytes());
    put(offset_of!(Record, LastModificationTime), &9i64.to_le_bytes());
    put(offset_of!(Record, FileSize), &42i64.to_le_bytes());
    put(offset_of!(Record, FileAttributes), &attrs.to_le_bytes());
    put(offset_of!(Record, FileId), &id.to_le_bytes());
    put(offset_of!(Record, ParentFileId), &parent.to_le_bytes());
    put(offset_of!(Record, FileNameLength), &(wide.len() as u32 * 2).to_le_bytes());
    r.extend(wide.iter().flat_map(|c| c.to_le_bytes()));
    if next != 0 {
        r.resize(next as usize, 0);
    }
    r
}

#[test]
fn extended_notices_carry_ids_and_relative_paths() {
    let mut buf = record(FILE_ACTION_RENAMED_OLD_NAME, r"a\old.txt", 7, 3, FILE_ATTRIBUTE_ARCHIVE.0, 120);
    buf.extend(record(FILE_ACTION_RENAMED_NEW_NAME, r"b\new.txt", 7, 4, FILE_ATTRIBUTE_ARCHIVE.0, 0));
    let got = notices(&buf).unwrap();
    assert_eq!(got.len(), 2);
    assert_eq!((got[1].id, got[1].parent, got[1].size, got[1].modified), (7, 4, 42, 9));
    assert_eq!(String::from_utf16(&got[1].path).unwrap(), r"b\new.txt");
    let cut = record(FILE_ACTION_ADDED, "x", 1, 2, 0, 0);
    assert!(notices(&cut[..cut.len() - 1]).is_none());
}

#[test]
fn renames_pair_up_and_unknown_ids_force_a_rescan() {
    let w = WalkIndex::with_roots(Vec::new());
    let root = FsPath::parse(r"C:\nowhere").unwrap();
    let mut v = VolumeIndex::new("x", br"C:\nowhere", b'\\');
    v.upsert(&IndexRecord { id: 3, parent: 3, name: b".", size: 0, modified: 0, attrs: attr::DIR });
    v.upsert(&IndexRecord { id: 7, parent: 3, name: b"old.txt", size: 0, modified: 0, attrs: 0 });
    v.upsert(&IndexRecord { id: 8, parent: 3, name: b"gone.txt", size: 0, modified: 0, attrs: 0 });
    v.commit();
    let mut buf = record(FILE_ACTION_RENAMED_OLD_NAME, "old.txt", 7, 3, 0, 120);
    buf.extend(record(FILE_ACTION_RENAMED_NEW_NAME, "new.txt", 7, 3, 0, 120));
    buf.extend(record(FILE_ACTION_RENAMED_OLD_NAME, "gone.txt", 8, 3, 0, 0));
    assert!(w.apply(&root, notices(&buf).unwrap(), &mut v));
    v.commit();
    assert_eq!(v.name(v.lookup(7).unwrap()), b"new.txt");
    assert!(v.lookup(8).is_none(), "moved out of the watched tree");
    assert!(!w.apply(&root, notices(&record(FILE_ACTION_REMOVED, "x", 0, 3, 0, 0)).unwrap(), &mut v));
}

fn temp_root(tag: &str) -> std::path::PathBuf {
    let root = crate::test_temp().join(format!("flit-walk-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn paths(v: &VolumeIndex) -> Vec<String> {
    let mut out: Vec<String> = (0..v.len() as u32)
        .filter(|&i| v.listed(i))
        .map(|i| {
            let mut p = Vec::new();
            v.path_into(i, false, &mut Vec::new(), &mut p);
            String::from_utf8(p).unwrap()
        })
        .collect();
    out.sort();
    out
}

#[test]
fn scans_a_tree_and_follows_live_changes() {
    let root = temp_root("live");
    let outside = temp_root("outside");
    std::fs::create_dir_all(root.join("a").join("b")).unwrap();
    std::fs::write(root.join("a").join("b").join("deep.txt"), b"12345").unwrap();
    std::fs::write(root.join("отчёт.md"), b"x").unwrap();
    std::fs::create_dir_all(outside.join("moved").join("inner")).unwrap();
    std::fs::write(outside.join("moved").join("inner").join("m.txt"), b"m").unwrap();
    let rs = root.to_str().unwrap().to_owned();
    let mut w = WalkIndex::with_roots(vec![FsPath::parse(&rs).unwrap()]);
    let volume = w.volumes().unwrap()[0].id.clone();
    assert_eq!(volume, rs);
    let mut v = VolumeIndex::new(&volume, rs.as_bytes(), b'\\');
    let cp = w.scan(&volume, &mut v).unwrap();
    v.commit();
    let p = |rel: &str| format!(r"{rs}\{rel}");
    assert_eq!(paths(&v), [p("a"), p(r"a\b"), p(r"a\b\deep.txt"), p("отчёт.md")]);
    let deep = (0..v.len() as u32).find(|&i| v.name(i) == b"deep.txt").unwrap();
    assert_eq!(v.sizes[deep as usize], 5);

    std::fs::write(root.join("new.txt"), b"n").unwrap();
    std::fs::rename(root.join("a"), root.join("renamed")).unwrap();
    std::fs::remove_file(root.join("отчёт.md")).unwrap();
    std::fs::rename(outside.join("moved"), root.join("moved")).unwrap();
    let want = [p("moved"), p(r"moved\inner"), p(r"moved\inner\m.txt"), p("new.txt"), p("renamed"), p(r"renamed\b"), p(r"renamed\b\deep.txt")];
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut rescans = 0;
    loop {
        match w.changes(&volume, cp, &mut v).unwrap() {
            Some(_) => {}
            None => {
                rescans += 1;
                v = VolumeIndex::new(&volume, rs.as_bytes(), b'\\');
                w.scan(&volume, &mut v).unwrap();
            }
        }
        v.commit();
        if paths(&v) == want {
            break;
        }
        assert!(Instant::now() < deadline, "{:?}", paths(&v));
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(rescans, 0, "changes were applied incrementally");
    drop(w);
    std::fs::remove_dir_all(&root).unwrap();
    let _ = std::fs::remove_dir_all(&outside);
}

fn listing(v: &VolumeIndex) -> Vec<(String, u64)> {
    let mut out: Vec<(String, u64)> = (0..v.len() as u32)
        .filter(|&i| v.listed(i))
        .map(|i| {
            let mut p = Vec::new();
            v.path_into(i, false, &mut Vec::new(), &mut p);
            (String::from_utf8(p).unwrap(), v.sizes[i as usize])
        })
        .collect();
    out.sort();
    out
}

#[test]
fn a_restart_catches_up_from_the_journal_without_walking_again() {
    let root = temp_root("resume");
    let outside = temp_root("resume-outside");
    for d in ["keep/deep", "gone/inner", "leaving/inner", "renamed-me"] {
        std::fs::create_dir_all(root.join(d)).unwrap();
    }
    std::fs::write(root.join("keep/deep/a.txt"), b"1").unwrap();
    std::fs::write(root.join("gone/inner/b.txt"), b"2").unwrap();
    std::fs::write(root.join("leaving/inner/c.txt"), b"3").unwrap();
    std::fs::write(root.join("grows.txt"), b"4").unwrap();
    std::fs::create_dir_all(outside.join("arriving/inner")).unwrap();
    std::fs::write(outside.join("arriving/inner/d.txt"), b"5").unwrap();
    let rs = root.to_str().unwrap().to_owned();
    let fresh = |rs: &str| {
        let mut w = WalkIndex::with_roots(vec![FsPath::parse(rs).unwrap()]);
        let volume = w.volumes().unwrap()[0].id.clone();
        let mut v = VolumeIndex::new(&volume, rs.as_bytes(), b'\\');
        let cp = w.scan(&volume, &mut v).unwrap();
        v.commit();
        (v, cp, volume)
    };
    let (mut v, cp, volume) = fresh(&rs);
    assert_ne!(cp.next, 0, "the temp folder sits on a volume with a change journal");

    std::fs::remove_dir_all(root.join("gone")).unwrap();
    std::fs::create_dir_all(root.join("new/a/b/c")).unwrap();
    std::fs::write(root.join("new/a/b/c/e.txt"), b"66").unwrap();
    std::fs::rename(root.join("renamed-me"), root.join("renamed")).unwrap();
    std::fs::rename(root.join("leaving"), outside.join("left")).unwrap();
    std::fs::rename(outside.join("arriving"), root.join("keep/arrived")).unwrap();
    std::fs::write(root.join("grows.txt"), b"4444").unwrap();

    let mut w = WalkIndex::with_roots(vec![FsPath::parse(&rs).unwrap()]);
    let next = w.changes(&volume, cp, &mut v).unwrap().expect("the journal still covers the checkpoint");
    v.commit();
    assert!(next.next > cp.next);
    assert_eq!(listing(&v), listing(&fresh(&rs).0));
    assert!(listing(&v).iter().any(|(p, s)| p.ends_with(r"keep\arrived\inner\d.txt") && *s == 1));
    assert!(listing(&v).iter().any(|(p, s)| p.ends_with("grows.txt") && *s == 4));

    let stale = Checkpoint { journal: cp.journal ^ 1, next: cp.next };
    assert_eq!(WalkIndex::with_roots(vec![FsPath::parse(&rs).unwrap()]).changes(&volume, stale, &mut v).unwrap(), None, "another journal means a full walk");
    drop(w);
    std::fs::remove_dir_all(&root).unwrap();
    let _ = std::fs::remove_dir_all(&outside);
}
