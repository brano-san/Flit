use super::*;
use crate::temp::Temp;
use flit_core::recycle;
use flit_core::sort::CaseMode;
use flit_core::{FsPath, Listing};
use flit_platform_linux::mounts::parse;
use std::os::unix::ffi::OsStrExt;

#[test]
fn eject_unmounts_then_powers_off_the_block_device_behind_the_mount() {
    let table = parse(b"40 1 8:17 / /media/u/STICK rw,nosuid - vfat /dev/sdb1 rw\n41 1 0:50 / /media/u/net rw - fuse.sshfs host:/ rw\n");
    let steps = eject_steps(&table, Path::new("/media/u/STICK")).unwrap();
    let words = |s: &[std::ffi::OsString; 4]| s.iter().map(|w| w.to_string_lossy().into_owned()).collect::<Vec<_>>().join(" ");
    assert_eq!(words(&steps[0]), "udisksctl unmount -b /dev/sdb1");
    assert_eq!(words(&steps[1]), "udisksctl power-off -b /dev/sdb1");
    assert!(eject_steps(&table, Path::new("/media/u/net")).is_none());
    assert!(eject_steps(&table, Path::new("/nowhere")).is_none());
}

#[test]
fn recent_files_come_from_the_xbel_newest_first_existing_and_unique() {
    let xbel = br#"<?xml version="1.0"?>
<xbel version="1.0">
  <bookmark href="file:///home/ann/old.txt" added="2020-01-01T00:00:00Z" modified="2020-01-01T00:00:00Z" visited="2020-01-01T00:00:00Z"/>
  <bookmark href="file:///home/ann/Tom%20&amp;%20Jerry" modified="2024-05-01T10:00:00.123456Z"><info/></bookmark>
  <bookmark href="file:///home/ann/gone.txt" modified="2025-01-01T00:00:00Z"/>
  <bookmark href="https://example.com/x" modified="2025-02-01T00:00:00Z"/>
  <bookmark href="file:///home/ann/old.txt" modified="2019-01-01T00:00:00Z"/>
</xbel>"#;
    let exists = |p: &Path| match p.as_os_str().as_bytes() {
        b"/home/ann/old.txt" => Some(false),
        b"/home/ann/Tom & Jerry" => Some(true),
        _ => None,
    };
    let got = recent_from(xbel, 10, exists);
    let names: Vec<(&str, bool)> = got.iter().map(|r| (r.name.as_str(), r.dir)).collect();
    assert_eq!(names, [("Tom & Jerry", true), ("old.txt", false)]);
    assert_eq!(got[0].at, 1_714_557_600 * 10_000_000 + 116_444_736_000_000_000);
    assert_eq!(recent_from(xbel, 1, exists).len(), 1);
    assert!(recent_from(b"", 5, exists).is_empty());
}

fn listing(fill: impl FnOnce(&mut ListingBuilder<'_>)) -> Listing {
    let mut go = |_: &ListingBuilder<'_>| true;
    let mut b = ListingBuilder::new(CaseMode::Sensitive, &mut go);
    fill(&mut b);
    b.listing
}

fn fs(p: &Path) -> FsPath {
    FsPath::parse_exact(p.as_os_str().as_bytes()).unwrap()
}

#[test]
fn volumes_take_labels_and_sizes() {
    let t = Temp::new("volumes");
    let devs = t.path().join("dev");
    std::fs::create_dir(&devs).unwrap();
    std::fs::write(devs.join("sdb1"), b"").unwrap();
    std::fs::write(devs.join("sda1"), b"").unwrap();
    let by = t.path().join("by-label");
    std::fs::create_dir(&by).unwrap();
    std::os::unix::fs::symlink(devs.join("sdb1"), by.join(r"Photo\x20Disk")).unwrap();
    std::os::unix::fs::symlink(devs.join("sda1"), by.join("rootfs")).unwrap();
    let table = format!(
        "1 0 8:1 / / rw - ext4 {root} rw\n2 1 8:17 / /media/ann/disk rw - ext4 {disk} rw\n3 1 0:70 / /mnt/c rw - 9p C:\\134 rw\n",
        root = devs.join("sda1").display(),
        disk = devs.join("sdb1").display()
    );
    let v = volumes_from(&parse(table.as_bytes()), b"ann", &by);
    let got: Vec<(&[u8], &str, bool)> = v.iter().map(|v| (v.mount.point.as_slice(), v.label.as_str(), v.mount.removable())).collect();
    assert_eq!(got, [(&b"/"[..], "File System", false), (b"/media/ann/disk", "Photo Disk", true), (b"/mnt/c", "c", false)]);
    assert!(v[0].total > 0 && v[0].free <= v[0].total);
    assert_eq!(v[1].total, 0);
}

#[test]
fn computer_lists_absolute_mount_points_as_volumes() {
    let l = listing(|b| list_computer(b).unwrap());
    let root = (0..l.len() as u32).find(|&i| l.name(i) == b"/").expect("the root is always shown");
    let e = &l.entries[root as usize];
    assert_eq!(e.attrs & (attr::DIR | attr::VOLUME | attr::SIZE_KNOWN), attr::DIR | attr::VOLUME | attr::SIZE_KNOWN);
    assert!(e.size > 0);
    assert_eq!(FsPath::computer().join(l.name(root)), FsPath::parse("/").unwrap());
    assert!(drives().iter().any(|d| d.path == FsPath::parse("/").unwrap() && d.total > 0));
}

fn trash_at(t: &Temp) -> Trash {
    Trash::new(t.path().join("home/.local/share/Trash"), unsafe { libc::getuid() })
}

#[test]
fn trash_place_lists_home_and_volume_trash_with_origins() {
    let t = Temp::new("trash-place");
    let trash = trash_at(&t);
    let doc = t.file("work/report.txt", 5);
    std::fs::create_dir_all(t.path().join("work/folder/inner")).unwrap();
    let in_home = trash.trash(&doc).unwrap();
    let folder = trash.trash(&t.path().join("work/folder")).unwrap();
    let volume = t.path().join("volume");
    let uid = unsafe { libc::getuid() };
    let vt = volume.join(format!(".Trash-{uid}"));
    std::fs::create_dir_all(vt.join("files")).unwrap();
    std::fs::create_dir_all(vt.join("info")).unwrap();
    std::fs::write(vt.join("files/photo.jpg"), b"jpeg").unwrap();
    std::fs::write(vt.join("info/photo.jpg.trashinfo"), "[Trash Info]\nPath=pics/photo.jpg\nDeletionDate=2026-10-25T12:30:00\n").unwrap();
    std::fs::write(vt.join("files/orphan"), b"").unwrap();
    let dirs = trash.dirs(&[volume.clone(), t.path().join("missing")]);
    assert_eq!(dirs, [t.path().join("home/.local/share/Trash"), vt.clone()]);
    let l = listing(|b| trash.list(&dirs, b));
    assert_eq!(l.len(), 3);
    let find = |name: &[u8]| (0..l.len() as u32).find(|&i| l.name(i) == name).unwrap();
    let r = find(b"report.txt");
    assert_eq!(recycle::recycled(&l, r), Some(fs(&in_home)));
    assert_eq!(recycle::original(&l, r), Some(fs(&doc)));
    assert_eq!(l.entries[r as usize].size, 5);
    assert!(l.entries[r as usize].modified > 0);
    let f = find(b"folder");
    assert!(l.entries[f as usize].is_dir());
    assert_eq!(recycle::recycled(&l, f), Some(fs(&folder)));
    let p = find(b"photo.jpg");
    assert_eq!(recycle::original(&l, p), Some(fs(&volume.join("pics/photo.jpg"))));
    let expected = flit_platform_linux::trash_time(b"2026-10-25T12:30:00").unwrap();
    assert_eq!(l.entries[p as usize].modified, expected);
}

#[test]
fn restore_purge_and_empty_on_home_and_volume_trash() {
    let t = Temp::new("trash-ops");
    let trash = trash_at(&t);
    let a = t.file("a.txt", 1);
    let b = t.file("b.txt", 2);
    let ta = trash.trash(&a).unwrap();
    let tb = trash.trash(&b).unwrap();
    let job = flit_platform::Job::detached(flit_core::provider::Resolve::Skip);
    let back = flit_platform_linux::run_op(&flit_core::provider::FileOp::Restore(vec![(fs(&ta), fs(&a))]), &job, Some(&trash));
    assert!(back.error.is_none(), "{:?}", back.error);
    assert!(a.exists() && !ta.exists());
    assert!(!t.path().join("home/.local/share/Trash/info/a.txt.trashinfo").exists());
    let out = purge(&trash, &[(fs(&tb), fs(&b))]);
    assert!(out.error.is_none(), "{:?}", out.error);
    assert!(!tb.exists() && !t.path().join("home/.local/share/Trash/info/b.txt.trashinfo").exists());
    let volume = t.path().join("vol");
    let uid = unsafe { libc::getuid() };
    let shared = volume.join(".Trash").join(uid.to_string());
    std::fs::create_dir_all(shared.join("files/dir/sub")).unwrap();
    std::fs::create_dir_all(shared.join("info")).unwrap();
    std::fs::write(shared.join("info/dir.trashinfo"), "[Trash Info]\nPath=dir\nDeletionDate=2026-01-01T00:00:00\n").unwrap();
    std::fs::write(shared.join("info/stale.trashinfo"), "[Trash Info]\nPath=stale\n").unwrap();
    trash.trash(&t.file("c.txt", 1)).unwrap();
    let done = empty(&trash, std::slice::from_ref(&volume));
    assert!(done.error.is_none(), "{:?}", done.error);
    assert_eq!(done.items.len(), 2);
    assert_eq!(std::fs::read_dir(shared.join("files")).unwrap().count(), 0);
    assert_eq!(std::fs::read_dir(shared.join("info")).unwrap().count(), 0);
    assert_eq!(std::fs::read_dir(t.path().join("home/.local/share/Trash/info")).unwrap().count(), 0);
    let again = empty(&trash, &[volume]);
    assert!(again.error.is_some_and(|e| e.message.contains("already empty")));
}

#[test]
fn purge_refuses_items_outside_the_trash() {
    let t = Temp::new("trash-refuse");
    let trash = trash_at(&t);
    let loose = t.file("loose.txt", 1);
    let out = purge(&trash, &[(fs(&loose), fs(&loose))]);
    assert!(out.error.is_some() && out.items.is_empty());
    assert!(loose.exists());
}
