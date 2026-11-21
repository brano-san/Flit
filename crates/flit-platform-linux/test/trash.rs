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

#[test]
fn other_mounts_prefer_a_sticky_shared_trash_and_otherwise_their_own_folder() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let t = Temp::new("trash-shared");
    let trash = home(&t);
    let uid = unsafe { libc::getuid() };
    let own = |top: &Path| top.join(format!(".Trash-{uid}"));
    let plain = t.path().join("plain");
    std::fs::create_dir(&plain).unwrap();
    assert_eq!(trash.top_trash(&plain), own(&plain));
    let sticky = t.path().join("sticky");
    std::fs::create_dir_all(sticky.join(".Trash")).unwrap();
    std::fs::set_permissions(sticky.join(".Trash"), std::fs::Permissions::from_mode(0o1777)).unwrap();
    assert_eq!(trash.top_trash(&sticky), sticky.join(".Trash").join(uid.to_string()));
    assert!(sticky.join(".Trash").join(uid.to_string()).is_dir());
    let loose = t.path().join("loose");
    std::fs::create_dir_all(loose.join(".Trash")).unwrap();
    std::fs::set_permissions(loose.join(".Trash"), std::fs::Permissions::from_mode(0o777)).unwrap();
    assert_eq!(trash.top_trash(&loose), own(&loose), "a shared Trash without the sticky bit is not safe");
    let linked = t.path().join("linked");
    std::fs::create_dir(&linked).unwrap();
    symlink(sticky.join(".Trash"), linked.join(".Trash")).unwrap();
    assert_eq!(trash.top_trash(&linked), own(&linked), "a symbolic link is never trusted as the shared Trash");
}

fn as_root() -> bool {
    (unsafe { libc::geteuid() }) == 0
}

#[test]
fn emptying_the_trash_removes_folders_with_read_only_subfolders() {
    use std::os::unix::fs::PermissionsExt;
    if as_root() {
        return;
    }
    let t = Temp::new("trash-readonly");
    let trash = home(&t);
    let project = t.path().join("project");
    let objects = project.join(".git/objects/ab");
    std::fs::create_dir_all(&objects).unwrap();
    std::fs::write(objects.join("cdef"), b"blob").unwrap();
    std::fs::set_permissions(objects.join("cdef"), std::fs::Permissions::from_mode(0o444)).unwrap();
    std::fs::set_permissions(&objects, std::fs::Permissions::from_mode(0o555)).unwrap();
    std::fs::set_permissions(project.join(".git/objects"), std::fs::Permissions::from_mode(0o555)).unwrap();
    let moved = trash.trash(&project).unwrap();
    assert!(std::fs::remove_dir_all(&moved).is_err(), "a plain recursive delete is refused, as the user saw");
    let (removed, error) = trash.empty(&[t.path().join("data/Trash")]);
    assert!(error.is_none(), "{error:?}");
    assert_eq!(removed, std::slice::from_ref(&moved));
    assert!(!moved.exists());
    assert_eq!(std::fs::read_dir(t.path().join("data/Trash/info")).unwrap().count(), 0, "its info file goes too");
}

#[test]
fn forcing_a_delete_never_follows_links_out_of_the_trash() {
    use std::os::unix::fs::PermissionsExt;
    if as_root() {
        return;
    }
    let t = Temp::new("trash-links");
    let trash = home(&t);
    let outside = t.path().join("outside");
    std::fs::create_dir_all(outside.join("keep")).unwrap();
    std::fs::write(outside.join("keep/file"), b"x").unwrap();
    std::fs::set_permissions(outside.join("keep"), std::fs::Permissions::from_mode(0o555)).unwrap();
    let folder = t.path().join("folder");
    let locked = folder.join("locked");
    std::fs::create_dir_all(&locked).unwrap();
    std::os::unix::fs::symlink(&outside, locked.join("link")).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
    let moved = trash.trash(&folder).unwrap();
    let (_, error) = trash.empty(&[t.path().join("data/Trash")]);
    assert!(error.is_none(), "{error:?}");
    assert!(!moved.exists());
    assert!(outside.join("keep/file").exists(), "the link target is untouched");
    assert_eq!(std::fs::metadata(outside.join("keep")).unwrap().mode() & 0o777, 0o555, "its permissions too");
    std::fs::set_permissions(outside.join("keep"), std::fs::Permissions::from_mode(0o755)).unwrap();
}
