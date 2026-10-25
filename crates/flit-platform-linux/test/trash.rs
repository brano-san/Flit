use super::*;
use crate::temp::Temp;

fn home(t: &Temp) -> Trash {
    Trash::new(t.path().join("data/Trash"), unsafe { libc::getuid() })
}

#[test]
fn percent_encoding_keeps_only_safe_bytes() {
    assert_eq!(percent_encode(b"/tmp/a b%c.txt"), "/tmp/a%20b%25c.txt");
    assert_eq!(percent_encode(b"/x/\xFF:~_-"), "/x/%FF%3A~_-");
}

#[test]
fn local_time_has_trash_spec_format() {
    let s = local_time(0);
    assert_eq!(s.len(), 19);
    assert_eq!(&s[4..5], "-");
    assert_eq!(&s[10..11], "T");
}

#[test]
fn home_trash_writes_info_then_moves() {
    let t = Temp::new("trash");
    let trash = home(&t);
    let victim = t.file("a b%.txt", 7);
    let moved = trash.trash(&victim).unwrap();
    assert!(!victim.exists());
    assert_eq!(moved, t.path().join("data/Trash/files/a b%.txt"));
    assert_eq!(std::fs::read(&moved).unwrap().len(), 7);
    let info = std::fs::read_to_string(t.path().join("data/Trash/info/a b%.txt.trashinfo")).unwrap();
    let lines: Vec<&str> = info.lines().collect();
    assert_eq!(lines[0], "[Trash Info]");
    assert_eq!(lines[1], format!("Path={}", percent_encode(victim.as_os_str().as_bytes())));
    assert!(lines[2].starts_with("DeletionDate=") && lines[2].len() == "DeletionDate=".len() + 19, "{}", lines[2]);
    let mode = std::fs::metadata(t.path().join("data/Trash")).unwrap().mode() & 0o777;
    assert_eq!(mode, 0o700);
}

#[test]
fn name_collisions_get_numbered_and_folders_move_whole() {
    let t = Temp::new("trash-dup");
    let trash = home(&t);
    for sub in ["one", "two", "three"] {
        std::fs::create_dir(t.path().join(sub)).unwrap();
        t.file(&format!("{sub}/same"), 1);
    }
    let names: Vec<_> = ["one", "two", "three"].iter().map(|s| trash.trash(&t.path().join(s).join("same")).unwrap().file_name().unwrap().to_owned()).collect();
    assert_eq!(names, ["same", "same.2", "same.3"]);
    assert!(t.path().join("data/Trash/info/same.3.trashinfo").exists());
    std::fs::create_dir_all(t.path().join("dir/inner")).unwrap();
    t.file("dir/inner/f", 1);
    let raw = t.path().join(OsStr::from_bytes(b"dir"));
    let moved = trash.trash(&raw).unwrap();
    assert!(moved.join("inner/f").exists());
    let odd = t.path().join(OsStr::from_bytes(b"odd\xFF"));
    std::fs::write(&odd, b"").unwrap();
    assert_eq!(trash.trash(&odd).unwrap().file_name().unwrap().as_bytes(), b"odd\xFF");
}

#[test]
fn failures_leave_no_info_behind() {
    let t = Temp::new("trash-fail");
    let trash = home(&t);
    assert_eq!(trash.trash(&t.path().join("missing")).unwrap_err().kind(), io::ErrorKind::NotFound);
    assert!(trash.trash(Path::new("/")).is_err());
    let info = t.path().join("data/Trash/info");
    assert!(!info.exists() || std::fs::read_dir(info).unwrap().next().is_none());
}

#[test]
fn other_devices_use_the_top_directory() {
    let t = Temp::new("trash-top");
    let victim = t.file("v", 1);
    let dev = std::fs::symlink_metadata(&victim).unwrap().dev();
    let top = mount_point(&victim, dev);
    assert_eq!(std::fs::symlink_metadata(&top).unwrap().dev(), dev);
    assert!(top.parent().is_none_or(|up| std::fs::symlink_metadata(up).unwrap().dev() != dev));
    assert!(victim.starts_with(&top));
}

#[test]
fn deletion_dates_read_back_as_the_same_moment() {
    let secs = 1_761_394_200;
    assert_eq!(trash_time(local_time(secs).as_bytes()), Some(filetime(secs, 0)));
    assert_eq!(trash_time(b"2026-13-45"), None);
    assert_eq!(trash_time(b"garbage-not-a-date"), None);
}
