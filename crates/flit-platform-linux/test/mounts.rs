use super::*;
use crate::temp::Temp;

#[test]
fn mounted_network_folders_use_the_deepest_mount_and_path_boundaries() {
    let table = parse(b"1 0 8:1 / / rw - ext4 /dev/sda1 rw\n2 1 0:2 / /home/user/NAS rw - cifs //server/share rw\n3 1 0:3 / /home/user/ssh rw - fuse.sshfs user@host:/ rw\n4 2 8:2 / /home/user/NAS/local rw - ext4 /dev/sda2 rw\n5 1 0:4 / /run/user/1000/gvfs rw - fuse.gvfsd-fuse gvfs rw\n");
    for path in ["/home/user/NAS", "/home/user/NAS/folder", "/home/user/ssh/docs", "/run/user/1000/gvfs/smb-share:server=a"] {
        assert!(remote_path(Path::new(path), &table), "{path}");
    }
    for path in ["/", "/home/user/HDD", "/home/user/NAS-backup", "/home/user/NAS/local/docs"] {
        assert!(!remote_path(Path::new(path), &table), "{path}");
    }
}

const WSL: &str = r"75 80 0:29 / /usr/lib/modules/6.6.87.2-microsoft-standard-WSL2 rw,nosuid,nodev,noatime - overlay none rw,lowerdir=/modules
76 80 0:32 / /mnt/wsl rw,relatime shared:1 - tmpfs none rw
80 65 8:48 / / rw,relatime - ext4 /dev/sdd rw,discard,errors=remount-ro,data=ordered
81 80 0:38 / /mnt/wslg rw,relatime shared:2 - tmpfs none rw
82 81 8:48 / /mnt/wslg/distro ro,relatime shared:3 - ext4 /dev/sdd rw,discard
109 80 0:2 /init /init ro - rootfs rootfs rw,size=15809704k
112 80 0:59 / /proc rw,nosuid,nodev,noexec,noatime shared:16 - proc proc rw
120 111 0:23 / /sys/fs/cgroup rw,nosuid,nodev,noexec,relatime shared:23 - cgroup2 cgroup2 rw,nsdelegate
131 80 0:69 / /mnt/c rw,noatime - 9p C:\134 rw,aname=drvfs;path=C:\;uid=1000
132 80 0:70 / /mnt/d rw,noatime - 9p D:\134 rw,aname=drvfs;path=D:\;uid=1000
704 134 0:83 / /run/user/1000 rw,nosuid,nodev,noexec,relatime shared:276 - tmpfs tmpfs rw,mode=755
";

const DESKTOP: &str = r"22 1 0:21 / /proc rw,nosuid shared:12 - proc proc rw
26 1 0:5 / /dev rw,nosuid shared:2 - devtmpfs udev rw
29 1 0:31 /@ / rw,relatime shared:1 - btrfs /dev/nvme0n1p2 rw,ssd,subvol=/@
30 29 0:31 /@home /home rw,relatime shared:30 - btrfs /dev/nvme0n1p2 rw,ssd,subvol=/@home
31 29 259:1 / /boot/efi rw,relatime shared:31 - vfat /dev/nvme0n1p1 rw
40 29 7:3 / /snap/core22/1612 ro,nodev,relatime shared:40 - squashfs /dev/loop3 ro
50 29 8:17 / /media/ann/My\040Disk rw,nosuid,nodev,relatime shared:50 - ext4 /dev/sdb1 rw
51 29 8:33 / /run/media/ann/USB\134Stick rw,nosuid,nodev,relatime shared:51 - vfat /dev/sdc1 rw
52 29 8:17 /photos /mnt/photos rw,relatime shared:50 - ext4 /dev/sdb1 rw
53 29 0:31 /@home/ann/work /mnt/work rw,relatime shared:30 - btrfs /dev/nvme0n1p2 rw,subvol=/@home
54 29 0:31 /@data /mnt/data rw,relatime shared:30 - btrfs /dev/nvme0n1p2 rw,subvol=/@data
60 29 0:60 / /mnt/nas rw,relatime shared:60 - nfs4 nas:/export rw
61 29 0:61 / /media/bob/Other rw,relatime shared:61 - ext4 /dev/sdd1 rw
62 29 0:62 / /mnt/deep/inside rw,relatime shared:62 - ext4 /dev/sde1 rw
63 29 0:63 / /mnt/ram rw,relatime shared:63 - tmpfs tmpfs rw
70 29 8:49 / /mnt/old rw,relatime shared:70 - ext4 /dev/sdd1 rw
71 29 8:50 / /mnt/old rw,relatime shared:71 - xfs /dev/sdf1 rw
broken line without the separator
";

fn points(ms: &[Mount]) -> Vec<String> {
    ms.iter().map(|m| String::from_utf8_lossy(&m.point).into_owned()).collect()
}

#[test]
fn unescapes_octal_and_keeps_stray_backslashes() {
    assert_eq!(unescape(br"My\040Disk"), b"My Disk");
    assert_eq!(unescape(br"a\134b\011c\012"), b"a\\b\tc\n");
    assert_eq!(unescape(br"C:\134"), b"C:\\");
    assert_eq!(unescape(br"odd\x\9"), br"odd\x\9");
    assert_eq!(unescape(br"end\04"), br"end\04");
}

#[test]
fn parses_fields_and_skips_garbage() {
    let ms = parse(DESKTOP.as_bytes());
    assert_eq!(ms.len(), 17);
    let disk = ms.iter().find(|m| m.id == 50).unwrap();
    assert_eq!(disk.point, b"/media/ann/My Disk");
    assert_eq!(disk.dev, (8, 17));
    assert_eq!(disk.fstype, "ext4");
    assert_eq!(disk.source, b"/dev/sdb1");
    assert_eq!(ms.iter().find(|m| m.id == 30).unwrap().root, b"/@home");
    let wsl = parse(WSL.as_bytes());
    assert_eq!(wsl.iter().find(|m| m.id == 131).unwrap().source, b"C:\\");
}

#[test]
fn wsl_shows_root_and_windows_drives_only() {
    assert_eq!(points(&visible(&parse(WSL.as_bytes()), b"cogitci")), ["/", "/mnt/c", "/mnt/d"]);
}

#[test]
fn desktop_rules_for_subvolumes_binds_media_and_network() {
    let shown = points(&visible(&parse(DESKTOP.as_bytes()), b"ann"));
    assert_eq!(shown, ["/", "/home", "/media/ann/My Disk", "/mnt/data", "/mnt/old", "/run/media/ann/USB\\Stick"]);
}

#[test]
fn without_a_user_name_media_folders_stay_hidden() {
    let shown = points(&visible(&parse(DESKTOP.as_bytes()), b""));
    assert!(!shown.iter().any(|p| p.starts_with("/media") || p.starts_with("/run/media")), "{shown:?}");
}

const CACHYOS: &str = r"22 1 0:21 / /proc rw,nosuid shared:5 - proc proc rw
23 1 0:22 / /sys rw,nosuid shared:6 - sysfs sysfs rw
24 1 0:5 / /dev rw,nosuid shared:2 - devtmpfs devtmpfs rw
25 23 0:23 / /sys/firmware/efi/efivars rw,nosuid shared:7 - efivarfs efivarfs rw
26 1 0:24 / /run rw,nosuid,nodev shared:12 - tmpfs tmpfs rw
1 0 0:26 /@ / rw,noatime shared:1 - btrfs /dev/nvme0n1p2 rw,ssd,subvol=/@
40 1 0:26 /@home /home rw,noatime shared:40 - btrfs /dev/nvme0n1p2 rw,ssd,subvol=/@home
41 1 0:26 /@root /root rw,noatime shared:41 - btrfs /dev/nvme0n1p2 rw,ssd,subvol=/@root
42 1 0:26 /@srv /srv rw,noatime shared:42 - btrfs /dev/nvme0n1p2 rw,ssd,subvol=/@srv
43 1 0:26 /@cache /var/cache rw,noatime shared:43 - btrfs /dev/nvme0n1p2 rw,ssd,subvol=/@cache
44 1 0:26 /@log /var/log rw,noatime shared:44 - btrfs /dev/nvme0n1p2 rw,ssd,subvol=/@log
45 1 0:26 /@tmp /var/tmp rw,noatime shared:45 - btrfs /dev/nvme0n1p2 rw,ssd,subvol=/@tmp
46 1 259:1 / /boot rw,relatime shared:46 - vfat /dev/nvme0n1p1 rw,fmask=0022
47 1 0:47 / /tmp rw,nosuid,nodev shared:47 - tmpfs tmpfs rw
50 40 8:1 / /home/maikun/HDD rw,relatime shared:50 - ext4 /dev/sda1 rw
51 26 8:17 / /run/media/maikun/Stick rw,nosuid,nodev,relatime shared:51 - exfat /dev/sdb1 rw
52 26 0:52 / /run/user/1000/doc rw,nosuid,nodev,relatime shared:52 - fuse.portal portal rw
53 1 8:33 / /mnt/games rw,relatime shared:53 - ntfs3 /dev/sdc1 rw
54 53 8:33 /Steam /mnt/games-steam rw,relatime shared:54 - ntfs3 /dev/sdc1 rw
55 1 8:49 / /media/Backup rw,relatime shared:55 - xfs /dev/sdd1 rw
56 1 8:65 / /media/guest/Other rw,relatime shared:56 - ext4 /dev/sde1 rw
57 1 0:57 / /var/lib/docker/overlay2/x/merged rw - overlay overlay rw
";

#[test]
fn storage_shows_each_file_system_once_and_only_real_disks() {
    let shown = points(&disks(&parse(CACHYOS.as_bytes()), b"maikun"));
    assert_eq!(shown, ["/", "/home/maikun/HDD", "/media/Backup", "/mnt/games", "/run/media/maikun/Stick"]);
    let desktop = points(&disks(&parse(DESKTOP.as_bytes()), b"ann"));
    assert_eq!(desktop, ["/", "/media/ann/My Disk", "/mnt/deep/inside", "/mnt/old", "/run/media/ann/USB\\Stick"]);
    assert_eq!(points(&disks(&parse(WSL.as_bytes()), b"cogitci")), ["/", "/mnt/c", "/mnt/d"]);
}

#[test]
fn a_separate_home_partition_stays_next_to_the_root() {
    let table = "1 0 259:2 / / rw - ext4 /dev/nvme0n1p2 rw\n2 1 259:3 / /home rw - ext4 /dev/nvme0n1p3 rw\n3 1 259:1 / /boot/efi rw - vfat /dev/nvme0n1p1 rw\n";
    assert_eq!(points(&disks(&parse(table.as_bytes()), b"ann")), ["/", "/home"]);
}

#[test]
fn overmounted_point_keeps_the_top_mount() {
    let ms = visible(&parse(DESKTOP.as_bytes()), b"ann");
    assert_eq!(ms.iter().find(|m| m.point == b"/mnt/old").unwrap().fstype, "xfs");
}

#[test]
fn storage_classification() {
    let ms = parse(DESKTOP.as_bytes());
    let kind = |id: u32| ms.iter().find(|m| m.id == id).unwrap();
    assert!(kind(22).is_pseudo());
    assert!(kind(60).is_network());
    assert!(!kind(40).is_storage());
    assert!(kind(31).is_storage());
    assert!(kind(50).removable() && kind(51).removable() && !kind(54).removable());
}

#[test]
fn labels_decode_udev_escapes_and_resolve_links() {
    let t = Temp::new("labels");
    let devs = t.path().join("devs");
    std::fs::create_dir(&devs).unwrap();
    std::fs::write(devs.join("sdb1"), b"").unwrap();
    let by = t.path().join("by-label");
    std::fs::create_dir(&by).unwrap();
    std::os::unix::fs::symlink("../devs/sdb1", by.join(r"My\x20Disk")).unwrap();
    let map = labels(&by);
    assert_eq!(unescape_label(br"a\x2fb\x20c"), "a/b c");
    let m = Mount { id: 1, dev: (8, 17), root: b"/".to_vec(), point: b"/media/x".to_vec(), fstype: "ext4".into(), source: devs.join("sdb1").as_os_str().as_bytes().to_vec() };
    assert_eq!(label_of(&m, &map).as_deref(), Some("My Disk"));
    assert_eq!(label_of(&Mount { source: b"none".to_vec(), ..m }, &map), None);
}

#[test]
fn live_table_has_root_and_statvfs_works() {
    let ms = read().unwrap();
    assert!(ms.iter().any(|m| m.point == b"/"));
    let (total, free) = space(Path::new("/")).unwrap();
    assert!(total > 0 && free <= total);
    assert!(space(Path::new("/nonexistent/flit")).is_none());
}

#[test]
fn watch_starts_on_mountinfo_and_fails_on_missing_file() {
    assert!(watch(Path::new(MOUNTINFO), || {}).is_ok());
    assert!(watch(Path::new("/tmp/flit-missing-mountinfo"), || {}).is_err());
}
