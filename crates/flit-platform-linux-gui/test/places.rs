use super::*;
use crate::temp::Temp;
use flit_core::recycle;
use flit_core::sort::CaseMode;
use flit_core::{FsPath, Listing};
use flit_platform_linux::mounts::parse;
use std::os::unix::ffi::OsStrExt;

#[test]
fn bookmarks_come_from_gtk_then_kde_without_home_duplicates_or_missing_folders() {
    let gtk = b"file:///home/u/Projects Projects\nfile:///home/u/My%20Docs\nsftp://host/x\nfile:///home/u/gone\n";
    let kde = b"<xbel><bookmark href=\"file:///home/u\"><title>Home</title></bookmark><bookmark href=\"file:///home/u/Projects\"/><bookmark href=\"file:///srv/a&amp;b\"/><bookmark href=\"trash:/\"/></xbel>";
    let got = bookmarks_from(gtk, kde, &[b"/home/u", b"/"], |p| !p.ends_with("gone"));
    let paths: Vec<String> = got.iter().map(|l| String::from_utf8_lossy(l.path().as_bytes()).into_owned()).collect();
    assert_eq!(paths, ["/home/u/Projects", "/home/u/My Docs", "/srv/a&b"]);
}

#[test]
fn unmounted_hotplug_filesystems_become_mountable_drives() {
    let lsblk = "PATH=\"/dev/sda1\" LABEL=\"\" SIZE=\"100\" MOUNTPOINT=\"/\" HOTPLUG=\"0\" FSTYPE=\"ext4\"\n\
PATH=\"/dev/sdb1\" LABEL=\"MY STICK\" SIZE=\"2048\" MOUNTPOINT=\"\" HOTPLUG=\"1\" FSTYPE=\"vfat\"\n\
PATH=\"/dev/sdb2\" LABEL=\"\" SIZE=\"4096\" MOUNTPOINT=\"\" HOTPLUG=\"1\" FSTYPE=\"exfat\"\n\
PATH=\"/dev/sdb3\" LABEL=\"\" SIZE=\"1\" MOUNTPOINT=\"\" HOTPLUG=\"1\" FSTYPE=\"swap\"\n\
PATH=\"/dev/sdc\" LABEL=\"\" SIZE=\"1\" MOUNTPOINT=\"\" HOTPLUG=\"1\" FSTYPE=\"\"\n\
PATH=\"/dev/sdd1\" LABEL=\"Mounted\" SIZE=\"1\" MOUNTPOINT=\"/media/u/M\" HOTPLUG=\"1\" FSTYPE=\"vfat\"\n";
    let d = unmounted_from(lsblk);
    let got: Vec<(&[u8], &str, u64, bool)> = d.iter().map(|d| (d.path.as_bytes(), d.label.as_str(), d.total, d.unmounted)).collect();
    assert_eq!(got, [(&b"/dev/sdb1"[..], "MY STICK", 0, true), (&b"/dev/sdb2"[..], "sdb2", 0, true)]);
}

#[test]
fn a_locked_encrypted_volume_shows_as_one_to_unlock_and_an_unlocked_one_shows_its_contents() {
    let lsblk = "PATH=\"/dev/sdb1\" LABEL=\"\" MOUNTPOINT=\"\" HOTPLUG=\"1\" FSTYPE=\"crypto_LUKS\" TYPE=\"part\" PARTTYPE=\"0x83\" KNAME=\"sdb1\" PKNAME=\"sdb\"
PATH=\"/dev/sdc1\" LABEL=\"Vault\" MOUNTPOINT=\"\" HOTPLUG=\"1\" FSTYPE=\"crypto_LUKS\" TYPE=\"part\" PARTTYPE=\"0x83\" KNAME=\"sdc1\" PKNAME=\"sdc\"
PATH=\"/dev/mapper/luks-1\" LABEL=\"Inside\" MOUNTPOINT=\"\" HOTPLUG=\"0\" FSTYPE=\"ext4\" TYPE=\"crypt\" PARTTYPE=\"\" KNAME=\"dm-0\" PKNAME=\"sdc1\"
PATH=\"/dev/nvme0n1p3\" LABEL=\"\" MOUNTPOINT=\"\" HOTPLUG=\"0\" FSTYPE=\"crypto_LUKS\" TYPE=\"part\" PARTTYPE=\"0fc63daf-8483-4772-8e79-3d69d8477de4\" KNAME=\"nvme0n1p3\" PKNAME=\"nvme0n1\"
PATH=\"/dev/mapper/root\" LABEL=\"\" MOUNTPOINT=\"/\" HOTPLUG=\"0\" FSTYPE=\"btrfs\" TYPE=\"crypt\" PARTTYPE=\"\" KNAME=\"dm-1\" PKNAME=\"nvme0n1p3\"
";
    let got: Vec<(String, String, String, bool)> = unmounted_from(lsblk).into_iter().map(|d| (String::from_utf8_lossy(d.path.as_bytes()).into_owned(), d.title(), d.file_system, d.kind.is_some())).collect();
    assert_eq!(
        got,
        [
            ("/dev/sdb1".to_owned(), "Encrypted volume (sdb1)".to_owned(), "crypto_LUKS".to_owned(), true),
            ("/dev/mapper/luks-1".to_owned(), "Inside".to_owned(), "ext4".to_owned(), false),
        ]
    );
}

#[test]
fn unmounted_internal_data_partitions_show_and_system_partitions_do_not() {
    let lsblk = "PATH=\"/dev/nvme0n1p1\" LABEL=\"\" MOUNTPOINT=\"\" HOTPLUG=\"0\" FSTYPE=\"vfat\" TYPE=\"part\" PARTTYPE=\"C12A7328-F81F-11D2-BA4B-00A0C93EC93B\"\n\
PATH=\"/dev/nvme0n1p2\" LABEL=\"\" MOUNTPOINT=\"/\" HOTPLUG=\"0\" FSTYPE=\"btrfs\" TYPE=\"part\" PARTTYPE=\"0fc63daf-8483-4772-8e79-3d69d8477de4\"\n\
PATH=\"/dev/nvme1n1p3\" LABEL=\"Windows\" MOUNTPOINT=\"\" HOTPLUG=\"0\" FSTYPE=\"ntfs\" TYPE=\"part\" PARTTYPE=\"ebd0a0a2-b9e5-4433-87c0-68b6b72699c7\"\n\
PATH=\"/dev/nvme1n1p4\" LABEL=\"\" MOUNTPOINT=\"\" HOTPLUG=\"0\" FSTYPE=\"ntfs\" TYPE=\"part\" PARTTYPE=\"de94bba4-06d1-4d40-a16a-bfd50179d6ac\"\n\
PATH=\"/dev/sda2\" LABEL=\"\" MOUNTPOINT=\"\" HOTPLUG=\"0\" FSTYPE=\"swap\" TYPE=\"part\" PARTTYPE=\"0657fd6d-a4ab-43c4-84e5-0933c84b4f4f\"\n\
PATH=\"/dev/loop0\" LABEL=\"\" MOUNTPOINT=\"\" HOTPLUG=\"0\" FSTYPE=\"ext4\" TYPE=\"loop\" PARTTYPE=\"\"\n\
PATH=\"/dev/sdb1\" LABEL=\"Stick\" MOUNTPOINT=\"\" HOTPLUG=\"1\" FSTYPE=\"exfat\" TYPE=\"part\" PARTTYPE=\"0x7\"\n";
    let got: Vec<(String, bool)> = unmounted_from(lsblk).into_iter().map(|d| (d.label, d.removable)).collect();
    assert_eq!(got, [("Windows".to_owned(), false), ("Stick".to_owned(), true)]);
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
fn the_root_drive_is_named_in_the_interface_language() {
    let root = &parse(b"1 0 8:1 / / rw - ext4 /dev/sda1 rw\n")[0];
    assert_eq!(name_of(root, flit_core::i18n::Lang::En), "File System");
    assert_eq!(name_of(root, flit_core::i18n::Lang::Ru), "Файловая система");
}

#[test]
fn this_computer_lists_mounted_unmounted_and_network_volumes_under_their_labels() {
    let table = parse(b"1 0 8:1 / / rw - ext4 /dev/sda1 rw
2 1 8:17 / /media/ann/STICK rw - vfat /dev/sdb1 rw
");
    let mut volumes: Vec<Volume> = table.into_iter().map(|mount| Volume { label: name_of(&mount, flit_core::i18n::Lang::En), mount, total: 0, free: 0 }).collect();
    volumes[0].total = 1000;
    volumes[0].free = 400;
    volumes[1].label = "STICK".into();
    let unmounted = unmounted_from("PATH=\"/dev/sdc1\" LABEL=\"Backup\" MOUNTPOINT=\"\" HOTPLUG=\"1\" FSTYPE=\"ext4\" TYPE=\"part\" PARTTYPE=\"0x83\"
");
    let share = Drive { path: FsPath::parse("/run/user/1000/gvfs/smb-share:server=nas,share=photos").unwrap(), label: gvfs_label("smb-share:server=nas,share=photos"), total: 0, free: 0, file_system: String::new(), removable: false, unmounted: false, kind: None, remote: Some("smb-share:server=nas,share=photos".into()) };
    let others: Vec<Drive> = unmounted.into_iter().chain([share]).collect();
    let l = listing(|b| computer_from(b, &volumes, &others));
    let rows: Vec<(String, String, bool)> = (0..l.len() as u32).map(|i| (String::from_utf8_lossy(l.name(i)).into_owned(), String::from_utf8_lossy(l.label(i)).into_owned(), l.entries[i as usize].attrs & attr::SIZE_KNOWN != 0)).collect();
    assert_eq!(
        rows,
        [
            ("/".to_owned(), "File System".to_owned(), true),
            ("/media/ann/STICK".to_owned(), "STICK".to_owned(), false),
            ("/dev/sdc1".to_owned(), "Backup".to_owned(), false),
            ("/run/user/1000/gvfs/smb-share:server=nas,share=photos".to_owned(), "photos (nas)".to_owned(), false),
        ]
    );
    assert!((0..l.len()).all(|i| l.entries[i].attrs & (attr::DIR | attr::VOLUME) == attr::DIR | attr::VOLUME));
    assert_eq!(l.drive(0).map(|d| d.0), Some(400));
    assert_eq!(l.drive(1), None);
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

#[test]
fn gvfs_mounts_become_drives_with_readable_names() {
    assert_eq!(gvfs_label("smb-share:server=nas,share=photos"), "photos (nas)");
    assert_eq!(gvfs_label("sftp:host=box.lan,user=me"), "box.lan (sftp)");
    assert_eq!(gvfs_label("mtp:host=Google_Pixel_7_1234"), "Google_Pixel_7_1234 (mtp)");
    assert_eq!(gvfs_label("odd"), "odd");
    let root = std::env::temp_dir().join(format!("flit-gvfs-{}", std::process::id()));
    std::fs::create_dir_all(root.join("smb-share:server=nas,share=photos")).unwrap();
    std::fs::create_dir_all(root.join("mtp:host=Phone")).unwrap();
    let drives = gvfs(&root);
    assert_eq!(drives.iter().map(|d| d.label.as_str()).collect::<Vec<_>>(), ["Phone (mtp)", "photos (nas)"]);
    assert!(drives.iter().all(|d| d.remote.is_some() && !d.removable));
    assert!(gvfs(&root.join("missing")).is_empty());
    std::fs::remove_dir_all(&root).unwrap();
    assert_eq!(local_path("display name: photos\n  local path: /run/user/1000/gvfs/smb-share:server=nas,share=photos\n"), Some(PathBuf::from("/run/user/1000/gvfs/smb-share:server=nas,share=photos")));
    assert_eq!(local_path("uri: smb://nas/\n"), None);
}

#[test]
fn network_mounts_are_drives_that_are_never_removable() {
    let table = parse(b"1 0 8:1 / / rw - ext4 /dev/sda1 rw\n2 1 0:40 / /media/ann/NAS rw - cifs //server/share rw\n3 1 0:41 / /mnt/box rw - fuse.sshfs ann@box:/ rw\n");
    let v = volumes_from(&table, b"ann", Path::new("/nonexistent/by-label"));
    let got: Vec<(&[u8], &str, u64, bool)> = v.iter().map(|v| (v.mount.point.as_slice(), v.label.as_str(), v.total, v.mount.removable())).collect();
    assert_eq!(got[1..], [(&b"/media/ann/NAS"[..], "NAS", 0, false), (b"/mnt/box", "box", 0, false)]);
    assert!(v[0].total > 0);
    let local = drive_of(&v[0]).unwrap();
    let share = drive_of(&v[1]).unwrap();
    assert_eq!((local.remote.as_deref(), share.remote.as_deref()), (None, Some("//server/share")));
    assert!(!share.unmountable() && local.path == FsPath::parse("/").unwrap() && !local.unmountable());
}

fn answering_share(_: &Path) -> Option<(u64, u64)> {
    Some((500, 200))
}

fn hanging_share(p: &Path) -> Option<(u64, u64)> {
    if p.starts_with("/media/ann") {
        std::thread::sleep(std::time::Duration::from_secs(3));
    }
    Some((900, 100))
}

#[test]
fn a_network_drive_shows_its_free_space_and_a_hanging_one_does_not_hold_the_list() {
    use std::time::{Duration, Instant};
    let t = Temp::new("net-space");
    let share = format!("2 1 0:40 / {} rw - cifs //server/share rw", t.path().display());
    let table = parse(format!("1 0 8:1 / / rw - ext4 /dev/sda1 rw\n{share}\n").as_bytes());
    let live = spaces_of(&table, Duration::from_secs(5), mounts::space);
    assert!(live[1].is_some_and(|(total, free)| total > 0 && free <= total), "{live:?}");
    assert_eq!(spaces_of(&table, Duration::from_secs(5), answering_share), [Some((500, 200)), Some((500, 200))]);
    let stuck = parse(b"1 0 8:1 / / rw - ext4 /dev/sda1 rw\n2 1 0:40 / /media/ann/NAS rw - cifs //nas/a rw\n3 1 0:41 / /media/ann/BOX rw - nfs box:/b rw\n");
    let started = Instant::now();
    let got = spaces_of(&stuck, Duration::from_millis(300), hanging_share);
    assert!(started.elapsed() < Duration::from_secs(2), "{:?}", started.elapsed());
    assert_eq!(got, [Some((900, 100)), None, None]);
}

#[test]
fn trash_is_watched_in_each_files_folder_or_its_nearest_existing_parent() {
    let t = Temp::new("trash-watch");
    let trash = trash_at(&t);
    assert_eq!(trash_folders(&trash, &[], &[]), [t.path().to_path_buf()]);
    std::fs::create_dir_all(t.path().join("home/.local/share/Trash/files")).unwrap();
    std::fs::create_dir_all(t.path().join(format!("vol/.Trash-{}/files", unsafe { libc::getuid() }))).unwrap();
    std::fs::create_dir_all(t.path().join("bare")).unwrap();
    let got = trash_folders(&trash, &[t.path().join("vol"), t.path().join("bare")], &[]);
    assert_eq!(got, [t.path().join("home/.local/share/Trash/files"), t.path().join(format!("vol/.Trash-{}/files", unsafe { libc::getuid() }))]);
}

#[test]
fn a_drive_without_a_trash_yet_is_watched_for_the_trash_to_appear() {
    let t = Temp::new("trash-coming");
    let trash = trash_at(&t);
    let uid = unsafe { libc::getuid() };
    std::fs::create_dir_all(t.path().join("home/.local/share/Trash/files")).unwrap();
    std::fs::create_dir_all(t.path().join("stick")).unwrap();
    std::fs::create_dir_all(t.path().join("shared/.Trash")).unwrap();
    let (stick, shared, gone) = (t.path().join("stick"), t.path().join("shared"), t.path().join("gone"));
    let tops = [stick.clone(), shared.clone()];
    let got = trash_folders(&trash, &tops, &[stick.clone(), shared.clone(), gone]);
    assert_eq!(got, [t.path().join("home/.local/share/Trash/files"), stick.clone(), shared.join(".Trash"), shared.clone()]);
    std::fs::create_dir_all(stick.join(format!(".Trash-{uid}/files"))).unwrap();
    let got = trash_folders(&trash, &tops, std::slice::from_ref(&stick));
    assert_eq!(got, [t.path().join("home/.local/share/Trash/files"), stick.join(format!(".Trash-{uid}/files")), stick.clone()]);
}

#[test]
fn a_server_that_does_not_answer_is_given_up_on_and_a_login_prompt_is_recognised() {
    use std::os::unix::process::ExitStatusExt;
    use std::time::{Duration, Instant};
    let mut slow = std::process::Command::new("sh");
    slow.args(["-c", "sleep 5"]);
    let started = Instant::now();
    let e = run_within(slow, Duration::from_millis(200)).unwrap_err();
    assert_eq!((e.kind(), e.to_string()), (std::io::ErrorKind::TimedOut, S::ErrorsFsNotResponding.en().to_owned()));
    assert!(started.elapsed() < Duration::from_secs(2));
    let mut quick = std::process::Command::new("sh");
    quick.args(["-c", "echo mounted"]);
    let out = run_within(quick, Duration::from_secs(10)).unwrap();
    assert!(out.status.success() && out.stdout == b"mounted\n" && !needs_login(&out));
    let asked = std::process::Output { status: std::process::ExitStatus::from_raw(2 << 8), stdout: b"Authentication Required\nEnter user and password for share photos on nas:\nUser [ann]: ".to_vec(), stderr: b"gio: smb://nas/photos/: Password dialog cancelled\n".to_vec() };
    assert!(needs_login(&asked));
    let refused = std::process::Output { stderr: b"gio: smb://nas/photos/: Failed to mount Windows share: Connection refused\n".to_vec(), stdout: Vec::new(), ..asked };
    assert!(!needs_login(&refused));
}

#[test]
fn disconnecting_picks_gio_for_gvfs_fusermount_for_fuse_and_umount_for_kernel_shares() {
    let table = parse(b"1 0 0:40 / /mnt/nas rw - cifs //nas/share rw\n2 0 0:41 / /mnt/box rw - fuse.sshfs ann@box:/ rw\n");
    let gvfs = Path::new("/run/user/1000/gvfs");
    let words = |p: &str| disconnect_command(Path::new(p), &table, gvfs).iter().map(|w| w.to_string_lossy().into_owned()).collect::<Vec<_>>().join(" ");
    assert_eq!(words("/run/user/1000/gvfs/smb-share:server=nas,share=photos"), "gio mount -u /run/user/1000/gvfs/smb-share:server=nas,share=photos");
    assert_eq!(words("/mnt/box"), "fusermount -u /mnt/box");
    assert_eq!(words("/mnt/nas"), "umount /mnt/nas");
}

const GIO_MOUNT_LI: &str = "Drive(0): SanDisk Cruzer
  Type: GProxyDrive (GProxyVolumeMonitorUDisks2)
  Volume(0): STICK
    Type: GProxyVolume (GProxyVolumeMonitorUDisks2)
    ids:
     unix-device: '/dev/sdb1'
    can_mount=1
Volume(0): Pixel 7
  Type: GProxyVolume (GProxyVolumeMonitorMTP)
  ids:
   unix-device: '/dev/bus/usb/001/005'
  activation_root=mtp://Google_Pixel_7_1A2B3C/
  can_mount=1
  should_automount=1
Volume(1): Canon EOS 2000D
  Type: GProxyVolume (GProxyVolumeMonitorGPhoto2)
  activation_root=gphoto2://Canon_Inc._Canon_Digital_Camera/
  can_mount=1
Volume(2): Galaxy S21
  Type: GProxyVolume (GProxyVolumeMonitorMTP)
  activation_root=mtp://SAMSUNG_Android_R58N/
  Mount(0): Galaxy S21 -> mtp://SAMSUNG_Android_R58N/
    Type: GProxyMount (GProxyVolumeMonitorMTP)
Mount(1): photos on nas -> smb://nas/photos/
  Type: GDaemonMount
";

#[test]
fn phones_and_cameras_that_are_plugged_in_but_not_opened_show_as_drives_to_mount() {
    let root = Path::new("/run/user/1000/gvfs");
    let got: Vec<(String, String, bool)> = phones_from(GIO_MOUNT_LI, root).into_iter().map(|d| (String::from_utf8_lossy(d.path.as_bytes()).into_owned(), d.label.clone(), d.mountable())).collect();
    assert_eq!(
        got,
        [
            ("/run/user/1000/gvfs/mtp:host=Google_Pixel_7_1A2B3C".to_owned(), "Pixel 7".to_owned(), true),
            ("/run/user/1000/gvfs/gphoto2:host=Canon_Inc._Canon_Digital_Camera".to_owned(), "Canon EOS 2000D".to_owned(), true),
        ]
    );
    assert!(phones_from("", root).is_empty());
    assert_eq!(device_uri(Path::new("/run/user/1000/gvfs/mtp:host=Google_Pixel_7_1A2B3C"), root).as_deref(), Some("mtp://Google_Pixel_7_1A2B3C/"));
    assert_eq!(device_uri(Path::new("/run/user/1000/gvfs/smb-share:server=nas,share=photos"), root), None);
    assert_eq!(device_uri(Path::new("/dev/sdb1"), root), None);
}
