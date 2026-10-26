use super::*;
use crate::temp::Temp;
use flit_search::index::VolumeIndex;
use std::os::unix::fs::symlink;

fn index_of(root: &Path) -> VolumeIndex {
    let mut v = VolumeIndex::new("t", root.as_os_str().as_bytes(), b'/');
    walk(root, &mut v).unwrap();
    v.commit();
    v
}

fn paths(v: &VolumeIndex) -> Vec<String> {
    let (mut chain, mut out) = (Vec::new(), Vec::new());
    let mut all: Vec<String> = (0..v.len() as u32)
        .filter(|&i| v.listed(i))
        .map(|i| {
            out.clear();
            v.path_into(i, false, &mut chain, &mut out);
            String::from_utf8_lossy(&out).into_owned()
        })
        .collect();
    all.sort();
    all
}

#[test]
fn the_walk_lists_every_entry_under_its_parent_and_never_follows_links() {
    let t = Temp::new("walk");
    let r = t.path();
    t.file("a.txt", 3);
    std::fs::create_dir_all(r.join("d/e")).unwrap();
    std::fs::write(r.join("d/e/deep.rs"), b"x").unwrap();
    std::fs::write(r.join("d/.hidden"), b"").unwrap();
    symlink(r.join("d"), r.join("loop")).unwrap();
    let v = index_of(r);
    let root = r.to_string_lossy();
    let expect: Vec<String> = ["a.txt", "d", "d/.hidden", "d/e", "d/e/deep.rs", "loop"].iter().map(|p| format!("{root}/{p}")).collect();
    assert_eq!(paths(&v), expect);
    let deep = (0..v.len() as u32).find(|&i| v.name(i) == b"deep.rs").unwrap();
    assert!(!v.is_dir(deep));
    assert!((0..v.len() as u32).any(|i| v.name(i) == b"e" && v.is_dir(i)));
}

#[test]
fn volumes_skip_pseudo_network_and_windows_drive_mounts() {
    let info = b"22 1 8:1 / / rw - ext4 /dev/sda1 rw\n\
        23 22 0:5 / /proc rw - proc proc rw\n\
        24 22 0:40 / /mnt/c rw - 9p drvfs rw\n\
        25 22 8:2 / /home rw - btrfs /dev/sda2 rw\n\
        26 22 0:41 / /mnt/share rw - nfs4 srv:/x rw\n";
    let points: Vec<Vec<u8>> = indexed(&mounts::parse(info), b"ann").into_iter().map(|m| m.point).collect();
    assert_eq!(points, [b"/".to_vec(), b"/home".to_vec()]);
}

#[test]
fn a_volume_is_rewalked_only_after_its_period() {
    let t = Temp::new("rewalk");
    let mut w = WalkIndex::with_roots(vec![("v".into(), t.path().to_path_buf(), "ext4".into())]);
    let mut v = VolumeIndex::new("v", t.path().as_os_str().as_bytes(), b'/');
    assert_eq!(w.changes("v", Checkpoint::default(), &mut v).unwrap(), None);
    let cp = w.scan("v", &mut v).unwrap();
    assert_eq!(w.changes("v", cp, &mut v).unwrap(), Some(cp));
    w.walked[0].1 -= REWALK;
    assert_eq!(w.changes("v", cp, &mut v).unwrap(), None);
    assert!(w.scan("nope", &mut v).is_err());
}

#[test]
#[ignore]
fn bench_walk_and_snapshot_of_four_hundred_thousand_entries() {
    let t = Temp::new("walk-bench");
    for d in 0..400 {
        let dir = t.path().join(format!("dir{d}"));
        std::fs::create_dir(&dir).unwrap();
        for f in 0..999 {
            std::fs::File::create(dir.join(format!("file-{f}.txt"))).unwrap();
        }
    }
    let mut runs = Vec::new();
    let mut v = None;
    for _ in 0..3 {
        let start = Instant::now();
        let built = index_of(t.path());
        runs.push(start.elapsed());
        v = Some(built);
    }
    runs.sort();
    let v = v.unwrap();
    eprintln!("measure: walk of {} entries median {:?}", v.live(), runs[1]);
    let mut bytes = Vec::new();
    flit_search::snapshot::save(&v, Checkpoint::default(), &mut bytes);
    let start = Instant::now();
    let (back, _) = flit_search::snapshot::load(&bytes).unwrap();
    eprintln!("measure: snapshot of {} bytes loads in {:?} with {} entries", bytes.len(), start.elapsed(), back.live());
}
