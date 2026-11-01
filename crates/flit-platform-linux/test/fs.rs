use super::*;
use crate::temp::Temp;
use std::collections::HashMap;
use std::os::unix::fs::{symlink, PermissionsExt};

fn now_filetime() -> i64 {
    let d = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap();
    filetime(d.as_secs() as i64, d.subsec_nanos() as i64)
}

fn scanned(dir: &Path) -> HashMap<Vec<u8>, Meta> {
    let mut out = HashMap::new();
    scan(dir, |name, meta| out.insert(name.to_vec(), meta).is_none()).unwrap();
    out
}

#[test]
fn filetime_counts_from_1601() {
    assert_eq!(filetime(0, 0), 116_444_736_000_000_000);
    assert_eq!(filetime(1, 550), 116_444_736_010_000_005);
    assert_eq!(filetime(-11_644_473_600, 0), 0);
}

#[test]
fn scan_reports_raw_names_and_linux_metadata() {
    let t = Temp::new("scan");
    t.file("a.txt", 5);
    t.file(".hidden", 1);
    t.file("x:y\\z", 2);
    std::fs::write(t.path().join(OsStr::from_bytes(b"bad\xFFname")), b"abc").unwrap();
    let ro = t.file("ro", 0);
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o444)).unwrap();
    std::fs::create_dir(t.path().join("sub")).unwrap();
    symlink(t.path().join("sub"), t.path().join("to-dir")).unwrap();
    symlink("a.txt", t.path().join("to-file")).unwrap();
    symlink("missing", t.path().join("broken")).unwrap();
    let m = scanned(t.path());
    assert_eq!(m.len(), 9);
    assert_eq!(m[&b"a.txt"[..]].size, 5);
    assert_eq!(m[&b"a.txt"[..]].attrs, 0);
    assert_eq!(m[&b".hidden"[..]].attrs, attr::HIDDEN);
    assert_eq!(m[&b"x:y\\z"[..]].size, 2);
    assert_eq!(m[&b"bad\xFFname"[..]].size, 3);
    assert_eq!(m[&b"ro"[..]].attrs, attr::READONLY);
    assert_eq!(m[&b"sub"[..]].attrs, attr::DIR);
    assert_eq!(m[&b"to-dir"[..]].attrs, attr::DIR | attr::LINK);
    assert_eq!(m[&b"to-file"[..]].attrs, attr::LINK);
    assert_eq!(m[&b"broken"[..]].attrs, attr::LINK);
    let now = now_filetime();
    let day = 864_000_000_000;
    for (name, meta) in &m {
        assert!((now - meta.modified).abs() < day, "{name:?} modified {}", meta.modified);
        assert!(meta.created == 0 || (now - meta.created).abs() < day, "{name:?} created {}", meta.created);
    }
    assert!(m.values().any(|meta| meta.created != 0), "ext4 under WSL2 reports birth times");
}

#[test]
fn stat_path_follows_the_same_rules() {
    let t = Temp::new("stat");
    std::fs::create_dir(t.path().join(".git")).unwrap();
    assert_eq!(stat_path(&t.path().join(".git")).unwrap().attrs, attr::DIR | attr::HIDDEN);
    assert_eq!(stat_path(&t.path().join("nope")).unwrap_err().raw_os_error(), Some(libc::ENOENT));
    assert_eq!(fs_error(&stat_path(&t.path().join("nope")).unwrap_err()).kind, FsErrorKind::NotFound);
}

#[test]
fn linux_fs_lists_in_batches_and_reports_errors() {
    let t = Temp::new("readdir");
    for i in 0..3000 {
        t.file(&format!("f{i}"), i % 7);
    }
    let mut batches = 0;
    let mut on_batch = |_: &ListingBuilder<'_>| {
        batches += 1;
        true
    };
    let mut b = ListingBuilder::new(flit_core::sort::CaseMode::Sensitive, &mut on_batch);
    let dir = FsPath::parse(t.path().to_str().unwrap()).unwrap();
    LinuxFs.read_dir(&dir, &mut b).unwrap();
    assert_eq!(b.listing.len(), 3000);
    drop(b);
    assert!(batches >= 3, "{batches}");
    let mut stop = |_: &ListingBuilder<'_>| false;
    let mut b = ListingBuilder::new(flit_core::sort::CaseMode::Sensitive, &mut stop);
    LinuxFs.read_dir(&dir, &mut b).unwrap();
    assert_eq!(b.listing.len(), 1024);
    let missing = FsPath::parse(t.path().join("missing").to_str().unwrap()).unwrap();
    let mut keep = |_: &ListingBuilder<'_>| true;
    let mut b = ListingBuilder::new(flit_core::sort::CaseMode::Sensitive, &mut keep);
    assert_eq!(LinuxFs.read_dir(&missing, &mut b).unwrap_err().kind, FsErrorKind::NotFound);
    assert_eq!(LinuxFs.read_dir(&FsPath::parse(r"C:\x").unwrap(), &mut b).unwrap_err().kind, FsErrorKind::Unavailable);
}

#[test]
fn folder_sizes_skip_links_and_honour_cancellation() {
    let t = Temp::new("sizes");
    let outside = Temp::new("sizes-outside");
    outside.file("big", 100_000);
    std::fs::create_dir_all(t.path().join("a/deep/er")).unwrap();
    t.file("a/one", 100);
    t.file("a/deep/er/two", 200);
    symlink(outside.path(), t.path().join("a/link")).unwrap();
    std::fs::create_dir(t.path().join("b")).unwrap();
    t.file("b/three", 300);
    let st = tree_size(&t.path().join("a"), &|| true);
    assert_eq!((st.size, st.folders, st.partial), (300, 3, false), "a link to a folder counts but is not followed");
    let got = std::sync::Mutex::new(Vec::new());
    let items = vec![(4, PathBuf::from("a")), (9, PathBuf::from("b"))];
    dir_sizes(t.path(), &items, &|| true, &|i, s| got.lock().unwrap().push((i, s.size)));
    let mut got = got.into_inner().unwrap();
    got.sort();
    assert_eq!(got, [(4, 300), (9, 300)]);
    let none = std::sync::Mutex::new(Vec::new());
    dir_sizes(t.path(), &items, &|| false, &|i, s| none.lock().unwrap().push((i, s.size)));
    assert!(none.into_inner().unwrap().is_empty());
}

fn deep(root: &Path, walls: &HashMap<Vec<u8>, bool>, limit: usize, keep_going: &dyn Fn() -> bool) -> Vec<String> {
    let mut go = |_: &ListingBuilder<'_>| true;
    let mut b = ListingBuilder::new(flit_core::sort::CaseMode::Sensitive, &mut go);
    LinuxFs.read_dir(&FsPath::parse(root.to_str().unwrap()).unwrap(), &mut b).unwrap();
    read_deep(root, &mut b, walls, limit, keep_going);
    let l = &b.listing;
    let full = |i: u32| if l.folder(i).is_empty() { l.name(i).to_vec() } else { [l.folder(i), b"/", l.name(i)].concat() };
    let mut names: Vec<String> = (0..l.len() as u32).map(|i| String::from_utf8_lossy(&full(i)).into_owned()).collect();
    names.sort();
    names
}

#[test]
fn subfolders_never_follow_links_or_enter_walled_mounts_and_stop_on_cancel_or_limit() {
    let t = Temp::new("deep");
    for dir in ["a/b", "proc/1", "c"] {
        std::fs::create_dir_all(t.path().join(dir)).unwrap();
    }
    t.file("a/b/f.txt", 1);
    t.file("proc/1/status", 1);
    symlink(t.path(), t.path().join("a/up")).unwrap();
    let walls = HashMap::from([(t.path().join("proc").as_os_str().as_bytes().to_vec(), true)]);
    assert_eq!(deep(t.path(), &walls, usize::MAX, &|| true), ["a", "a/b", "a/b/f.txt", "a/up", "c", "proc"]);
    assert_eq!(deep(t.path(), &walls, 4, &|| true).len(), 4);
    assert_eq!(deep(t.path(), &walls, usize::MAX, &|| false), ["a", "c", "proc"]);
    assert_eq!(deep(&t.path().join("proc"), &walls, usize::MAX, &|| true), ["1"], "a walled folder lists only its own entries");
}

#[test]
fn virtual_and_network_file_systems_are_walled_off() {
    let table = crate::mounts::parse(b"1 0 8:2 / / rw - ext4 /dev/sda2 rw\n2 1 0:21 / /proc rw - proc proc rw\n3 1 0:22 / /sys rw - sysfs sysfs rw\n4 1 0:24 / /run rw - tmpfs tmpfs rw\n5 4 8:17 / /run/media/u/S rw - exfat /dev/sdb1 rw\n6 1 0:60 / /mnt/nas rw - nfs4 nas:/x rw\n7 1 0:61 / /home/u/remote rw - fuse.rclone r: rw\n8 1 0:62 / /mnt/sshfs rw - fuse.sshfs h:/ rw\n");
    let w = walls(&table);
    for (path, wall) in [("/proc/1/task", true), ("/sys", true), ("/run/user/1000", true), ("/run/media/u/S/photos", false), ("/mnt/nas", true), ("/home/u/remote", true), ("/home/u", false), ("/", false)] {
        assert_eq!(walled(path.as_bytes(), &w), wall, "{path}");
    }
}

#[test]
fn names_the_file_system_of_a_path() {
    let t = Temp::new("fstype");
    let fs = LinuxFs.file_system(&FsPath::parse(t.path().to_str().unwrap()).unwrap()).unwrap();
    assert!(!fs.is_empty());
    assert_eq!(LinuxFs.file_system(&FsPath::parse("/proc/self").unwrap()).as_deref(), Some("proc"));
}
