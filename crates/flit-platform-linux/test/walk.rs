use super::*;
use crate::temp::Temp;
use flit_search::index::VolumeIndex;
use std::os::unix::fs::symlink;

fn index_of(root: &Path) -> VolumeIndex {
    let mut v = VolumeIndex::new("t", root.as_os_str().as_bytes(), b'/');
    walk(root, &mut v, &mut Tree::default()).unwrap();
    v.commit();
    v
}

fn put(t: &Temp, name: &str, bytes: usize) {
    std::fs::create_dir_all(t.path().join(name).parent().unwrap()).unwrap();
    t.file(name, bytes);
}

fn size_of(v: &VolumeIndex, name: &[u8]) -> u64 {
    (0..v.len() as u32).find(|&i| v.listed(i) && v.name(i) == name).map_or(u64::MAX, |i| v.sizes[i as usize])
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
fn mount_roots_and_foreign_inodes_cannot_replace_local_parents() {
    use crate::fs::Node;
    let local = Node { mode: libc::S_IFDIR, dev: 1, ino: 256, mount_root: Some(false), ..Default::default() };
    assert!(indexable(&local, 1, 5));
    assert!(!indexable(&Node { dev: 2, ..local }, 1, 5));
    assert!(!indexable(&Node { mount_root: Some(true), ..local }, 1, 5));
    assert!(!indexable(&Node { ino: 5, ..local }, 1, 5));
    let t = Temp::new("foreign-index");
    t.file("a", 1);
    let mut children = Vec::new();
    let batch = read_dir(&Job { dir: t.path().to_owned(), id: 5, modified: 0 }, u64::MAX, &mut children, &[]);
    assert!(batch.items.is_empty() && children.is_empty());
    assert!(WalkIndex::with_roots(Vec::new()).snapshot_key("/").starts_with("walk-v2:"));
}

#[test]
fn after_its_period_a_volume_is_reread_in_place_instead_of_walked_again() {
    let t = Temp::new("rewalk");
    put(&t, "d/grows.txt", 1);
    let mut w = WalkIndex::with_roots(vec![("v".into(), t.path().to_path_buf(), "ext4".into())]);
    let mut v = VolumeIndex::new("v", t.path().as_os_str().as_bytes(), b'/');
    assert_eq!(w.changes("v", Checkpoint::default(), &mut v).unwrap(), None, "without a walk of its own the volume is scanned");
    let cp = w.scan("v", &mut v).unwrap();
    v.commit();
    assert_eq!(w.changes("v", cp, &mut v).unwrap(), Some(cp));
    std::fs::write(t.path().join("d/grows.txt"), b"12345").unwrap();
    w.changes("v", cp, &mut v).unwrap();
    v.commit();
    assert_eq!(size_of(&v, b"grows.txt"), 1, "an edit inside an unwatched folder waits for the period");
    w.walked[0].1 -= REWALK;
    assert_eq!(w.changes("v", cp, &mut v).unwrap(), Some(cp));
    v.commit();
    assert_eq!(size_of(&v, b"grows.txt"), 5);
    assert!(w.walked[0].1.elapsed() < REWALK);
    assert!(w.live.is_none(), "an index of chosen folders adds no inotify watches");
    assert!(w.scan("nope", &mut v).is_err());
}

#[test]
fn the_root_volume_is_keyed_by_its_file_system_identity() {
    let all = mounts::read().unwrap();
    let root = all.iter().rev().find(|m| m.point == b"/").unwrap();
    let key = volume_key(root, &mounts::labels(Path::new(BY_UUID))).unwrap();
    assert!(!key.is_empty() && !key.contains('/'), "{key}");
}

#[test]
fn a_sweep_picks_up_new_removed_and_moved_entries_by_directory_mtime() {
    let t = Temp::new("sweep");
    let r = t.path();
    std::fs::create_dir_all(r.join("d/e")).unwrap();
    std::fs::write(r.join("d/old.txt"), b"x").unwrap();
    std::fs::write(r.join("d/e/inner.rs"), b"x").unwrap();
    let mut w = WalkIndex::with_roots(vec![("v".into(), r.to_path_buf(), "ext4".into())]);
    let mut v = VolumeIndex::new("v", r.as_os_str().as_bytes(), b'/');
    let cp = w.scan("v", &mut v).unwrap();
    v.commit();
    std::fs::remove_file(r.join("d/old.txt")).unwrap();
    std::fs::write(r.join("new.txt"), b"x").unwrap();
    std::fs::create_dir_all(r.join("n/m")).unwrap();
    std::fs::write(r.join("n/m/deep.txt"), b"x").unwrap();
    std::fs::rename(r.join("d/e"), r.join("moved")).unwrap();
    assert_eq!(w.changes("v", cp, &mut v).unwrap(), Some(cp));
    v.commit();
    let root = r.to_string_lossy();
    let expect: Vec<String> = ["d", "moved", "moved/inner.rs", "n", "n/m", "n/m/deep.txt", "new.txt"].iter().map(|p| format!("{root}/{p}")).collect();
    assert_eq!(paths(&v), expect);
    std::fs::remove_dir_all(r.join("n")).unwrap();
    w.trees.get_mut("v").unwrap().swept = None;
    w.changes("v", cp, &mut v).unwrap();
    v.commit();
    let expect: Vec<String> = ["d", "moved", "moved/inner.rs", "new.txt"].iter().map(|p| format!("{root}/{p}")).collect();
    assert_eq!(paths(&v), expect);
}

#[test]
fn media_comes_from_the_rotational_flag_of_the_whole_disk() {
    let t = Temp::new("media");
    let sys = t.path().join("block");
    std::fs::create_dir_all(t.path().join("devices/sda/sda1")).unwrap();
    std::fs::create_dir_all(t.path().join("devices/sda/queue")).unwrap();
    std::fs::create_dir_all(t.path().join("devices/nvme0n1/queue")).unwrap();
    std::fs::create_dir_all(&sys).unwrap();
    std::fs::write(t.path().join("devices/sda/queue/rotational"), b"1\n").unwrap();
    std::fs::write(t.path().join("devices/nvme0n1/queue/rotational"), b"0\n").unwrap();
    symlink("../devices/sda/sda1", sys.join("8:1")).unwrap();
    symlink("../devices/sda", sys.join("8:0")).unwrap();
    symlink("../devices/nvme0n1", sys.join("259:0")).unwrap();
    assert_eq!(media(&sys, libc::makedev(8, 1)), Media::Spinning);
    assert_eq!(media(&sys, libc::makedev(8, 0)), Media::Spinning);
    assert_eq!(media(&sys, libc::makedev(259, 0)), Media::Solid);
    assert_eq!(media(&sys, libc::makedev(0, 42)), Media::Unknown);
    std::fs::create_dir_all(t.path().join("devices/dm-0/slaves")).unwrap();
    symlink("../../sda/sda1", t.path().join("devices/dm-0/slaves/sda1")).unwrap();
    symlink("../devices/dm-0", sys.join("253:0")).unwrap();
    assert_eq!(media(&sys, libc::makedev(253, 0)), Media::Spinning);
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
    let mode = std::env::var("FLIT_WALK_BENCH").unwrap_or_else(|_| "adaptive".into());
    let fixed = mode.strip_prefix("fixed").and_then(|n| n.parse().ok()).map(Throttle::fixed);
    let mut runs = Vec::new();
    let mut v = None;
    for _ in 0..3 {
        let start = Instant::now();
        let mut built = VolumeIndex::new("t", t.path().as_os_str().as_bytes(), b'/');
        walk_with(t.path(), &mut built, &mut Tree::default(), fixed.clone()).unwrap();
        built.commit();
        runs.push(start.elapsed());
        v = Some(built);
    }
    runs.sort();
    let v = v.unwrap();
    eprintln!("measure: {mode} walk of {} entries median {:?}", v.live(), runs[1]);
    let mut bytes = Vec::new();
    flit_search::snapshot::save(&v, Checkpoint::default(), &mut bytes);
    let start = Instant::now();
    let (back, _) = flit_search::snapshot::load(&bytes).unwrap();
    eprintln!("measure: snapshot of {} bytes loads in {:?} with {} entries", bytes.len(), start.elapsed(), back.live());
}

#[test]
#[ignore]
fn bench_an_idle_sweep_of_twenty_thousand_directories() {
    let t = Temp::new("sweep-bench");
    for d in 0..200 {
        for s in 0..100 {
            let dir = t.path().join(format!("dir{d}/sub{s}"));
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::File::create(dir.join("f.txt")).unwrap();
        }
    }
    let mut v = VolumeIndex::new("t", t.path().as_os_str().as_bytes(), b'/');
    let mut tree = Tree::default();
    walk(t.path(), &mut v, &mut tree).unwrap();
    v.commit();
    std::thread::sleep(Duration::from_secs(3));
    tree.sweep(&mut v);
    let mut runs: Vec<Duration> = (0..5)
        .map(|_| {
            let start = Instant::now();
            assert_eq!(tree.sweep(&mut v), 0);
            start.elapsed()
        })
        .collect();
    runs.sort();
    eprintln!("measure: idle sweep of {} directories median {:?}", tree.dirs.len(), runs[2]);
}

#[test]
fn a_new_mount_is_adopted_and_a_changed_one_is_walked_again() {
    let (a, b) = (Temp::new("retarget-a"), Temp::new("retarget-b"));
    let root = |id: &str, t: &Temp| (id.to_owned(), t.path().to_path_buf(), "ext4".to_owned());
    let keys = |pairs: &[(&str, &str)]| pairs.iter().map(|(v, k)| (v.to_string(), k.to_string())).collect::<HashMap<_, _>>();
    let mut w = WalkIndex::with_roots(vec![root("a", &a)]);
    w.keys = keys(&[("a", "uuid-1")]);
    let mut v = VolumeIndex::new("a", a.path().as_os_str().as_bytes(), b'/');
    let cp = w.scan("a", &mut v).unwrap();
    assert!(!w.retarget(vec![root("a", &a)], keys(&[("a", "uuid-1")])), "the same mounts change nothing");
    assert!(w.retarget(vec![root("a", &a), root("b", &b)], keys(&[("a", "uuid-1"), ("b", "uuid-2")])));
    assert_eq!(w.volumes().unwrap().iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), ["a", "b"]);
    assert_eq!(w.changes("a", cp, &mut v).unwrap(), Some(cp), "the mount that stayed keeps its walk");
    assert!(w.scan("b", &mut VolumeIndex::new("b", b.path().as_os_str().as_bytes(), b'/')).is_ok());
    assert!(w.retarget(vec![root("a", &a), root("b", &b)], keys(&[("a", "uuid-3"), ("b", "uuid-2")])));
    assert_eq!(w.changes("a", cp, &mut v).unwrap(), None, "another disk at the same place is walked again");
    assert!(w.snapshot_key("a").contains("uuid-3"));
    assert!(w.retarget(vec![root("b", &b)], keys(&[("b", "uuid-2")])));
    assert!(w.scan("a", &mut v).is_err() && !w.trees.contains_key("a"));
    assert!(!w.refresh(), "an index of chosen folders never follows the mounts");
}

#[test]
fn the_watch_budget_is_a_share_of_the_inotify_limit_and_skips_hidden_folders() {
    assert_eq!(budget(Some(8192)), 1024);
    assert_eq!(budget(Some(1_048_576)), 16_384);
    assert_eq!(budget(Some(100)), 12);
    assert_eq!(budget(None), 2048);
    let home = Path::new("/home/jo");
    assert!(watchable(home, home) && watchable(Path::new("/home/jo/Documents/x"), home));
    assert!(!watchable(Path::new("/home/jo/.cache/thumbnails"), home));
    assert!(!watchable(Path::new("/home/jo2/Documents"), home));
    assert!(!watchable(Path::new("/usr/share"), home));
}

#[test]
fn watched_folders_reach_the_index_at_once_and_hidden_ones_wait() {
    let t = Temp::new("watched");
    put(&t, "docs/a.txt", 1);
    put(&t, "docs/sub/b.txt", 1);
    put(&t, ".hidden/h.txt", 1);
    let mut w = WalkIndex::with_roots(vec![("v".into(), t.path().to_path_buf(), "ext4".into())]).watching(t.path().to_path_buf(), 100);
    let mut v = VolumeIndex::new("v", t.path().as_os_str().as_bytes(), b'/');
    let cp = w.scan("v", &mut v).unwrap();
    v.commit();
    assert_eq!(w.changes("v", cp, &mut v).unwrap(), Some(cp));
    v.commit();
    std::fs::write(t.path().join("docs/a.txt"), b"12345").unwrap();
    std::fs::write(t.path().join("docs/sub/new.txt"), b"xy").unwrap();
    std::fs::write(t.path().join(".hidden/h.txt"), b"12345").unwrap();
    std::thread::sleep(Duration::from_millis(250));
    assert_eq!(w.changes("v", cp, &mut v).unwrap(), Some(cp));
    v.commit();
    assert_eq!(size_of(&v, b"a.txt"), 5, "an edit in place is seen without waiting for the walk period");
    assert_eq!(size_of(&v, b"new.txt"), 2);
    assert_eq!(size_of(&v, b"h.txt"), 1, "hidden folders are not watched");
    std::fs::remove_dir_all(t.path().join("docs/sub")).unwrap();
    std::thread::sleep(Duration::from_millis(250));
    w.changes("v", cp, &mut v).unwrap();
    v.commit();
    assert_eq!(size_of(&v, b"b.txt"), u64::MAX);
    assert_eq!(w.live.as_ref().unwrap().watched["v"].len(), 2, "the removed folder gives its watch back");
}

#[test]
fn the_shallowest_folders_get_the_watches_when_the_budget_is_short() {
    let t = Temp::new("budget");
    put(&t, "top/mid/deep/f", 1);
    let mut w = WalkIndex::with_roots(vec![("v".into(), t.path().to_path_buf(), "ext4".into())]).watching(t.path().to_path_buf(), 2);
    let mut v = VolumeIndex::new("v", t.path().as_os_str().as_bytes(), b'/');
    w.scan("v", &mut v).unwrap();
    let ino = |p: &Path| std::fs::metadata(p).unwrap().ino();
    let watched = &w.live.as_ref().unwrap().watched["v"];
    assert_eq!(watched.len(), 2);
    assert!(watched.contains(&ino(t.path())) && watched.contains(&ino(&t.path().join("top"))));
}

#[test]
fn indexing_a_parent_leaves_automount_entries_out_of_the_job_queue() {
    let t = Temp::new("index-automount");
    let mount = t.path().join("Media");
    std::fs::create_dir(&mount).unwrap();
    t.file("local", 1);
    let fd = open_dir(t.path()).unwrap();
    let top = node_at(fd.as_raw_fd(), c".", false).unwrap();
    let mut children = Vec::new();
    let batch = read_dir(&Job { dir: t.path().to_owned(), id: top.ino, modified: 0 }, top.dev, &mut children, &[mount]);
    assert!(children.is_empty());
    assert_eq!(batch.names, b"local");
}
