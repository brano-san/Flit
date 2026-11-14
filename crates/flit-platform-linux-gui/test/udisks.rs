use super::*;

const BLOCKS: &str = "/org/freedesktop/UDisks2/block_devices/";
const DRIVES: &str = "/org/freedesktop/UDisks2/drives/";

fn s(x: &str) -> Arg {
    Arg::S(x.into())
}

fn yes(x: bool) -> Arg {
    Arg::I(x.into())
}

fn ay(x: &str) -> Arg {
    Arg::L(x.bytes().chain([0]).map(|b| Arg::I(b.into())).collect())
}

fn aay(xs: &[&str]) -> Arg {
    Arg::L(xs.iter().map(|x| ay(x)).collect())
}

fn props(kv: Vec<(&str, Arg)>) -> Arg {
    Arg::L(kv.into_iter().map(|(k, v)| Arg::L(vec![s(k), Arg::L(vec![v])])).collect())
}

fn object(path: String, ifaces: Vec<(&str, Arg)>) -> Arg {
    Arg::L(vec![Arg::S(path), Arg::L(ifaces.into_iter().map(|(i, p)| Arg::L(vec![s(i), p])).collect())])
}

fn block(name: &str, drive: &str, backing: &str, fs_type: &str, label: &str) -> Vec<(&'static str, Arg)> {
    let drive = if drive.is_empty() { "/".to_owned() } else { format!("{DRIVES}{drive}") };
    let backing = if backing.is_empty() { "/".to_owned() } else { format!("{BLOCKS}{backing}") };
    vec![(
        BLOCK,
        props(vec![
            ("Device", ay(&format!("/dev/{name}"))),
            ("PreferredDevice", ay(&format!("/dev/{}", if name.starts_with("dm-") { "mapper/luks-5e1f" } else { name }))),
            ("Symlinks", aay(&[&format!("/dev/disk/by-id/usb-{name}")])),
            ("Drive", Arg::S(drive)),
            ("CryptoBackingDevice", Arg::S(backing)),
            ("IdType", s(fs_type)),
            ("IdLabel", s(label)),
            ("HintIgnore", yes(false)),
            ("HintSystem", yes(name.starts_with("nvme"))),
            ("Size", Arg::I(1 << 30)),
        ]),
    )]
}

fn mounted(mut ifaces: Vec<(&'static str, Arg)>, points: &[&str]) -> Vec<(&'static str, Arg)> {
    ifaces.push((FILESYSTEM, props(vec![("MountPoints", aay(points)), ("Size", Arg::I(1 << 30))])));
    ifaces
}

fn encrypted(mut ifaces: Vec<(&'static str, Arg)>) -> Vec<(&'static str, Arg)> {
    ifaces.push((ENCRYPTED, props(vec![("HintEncryptionType", s("LUKS2")), ("MetadataSize", Arg::I(16 << 20))])));
    ifaces
}

fn drive(name: &str, ejectable: bool, power_off: bool, optical: bool) -> Arg {
    object(format!("{DRIVES}{name}"), vec![(DRIVE, props(vec![("Ejectable", yes(ejectable)), ("CanPowerOff", yes(power_off)), ("Optical", yes(optical)), ("Removable", yes(true)), ("Vendor", s("Acme"))]))])
}

fn recorded(unlocked: bool) -> Vec<Arg> {
    let mut objects = vec![
        object(format!("{BLOCKS}nvme0n1p2"), mounted(block("nvme0n1p2", "Samsung_SSD", "", "ext4", ""), &["/"])),
        drive("Samsung_SSD", false, false, false),
        object(format!("{BLOCKS}sdb"), block("sdb", "SanDisk_Cruzer", "", "", "")),
        object(format!("{BLOCKS}sdb1"), mounted(block("sdb1", "SanDisk_Cruzer", "", "vfat", "STICK"), &["/media/ann/STICK"])),
        object(format!("{BLOCKS}sdb2"), encrypted(block("sdb2", "SanDisk_Cruzer", "", "crypto_LUKS", ""))),
        drive("SanDisk_Cruzer", true, true, false),
        object(format!("{BLOCKS}sr0"), mounted(block("sr0", "DVD_RW", "", "iso9660", "Ubuntu"), &["/media/ann/Ubuntu"])),
        drive("DVD_RW", true, false, true),
        object(format!("{BLOCKS}loop3"), mounted(block("loop3", "", "", "ext4", "img"), &["/media/ann/img"])),
        object("/org/freedesktop/UDisks2/jobs/7".into(), vec![("org.freedesktop.UDisks2.Job", props(vec![("Operation", s("filesystem-mount"))]))]),
    ];
    if unlocked {
        objects.push(object(format!("{BLOCKS}dm_2d0"), mounted(block("dm-0", "", "sdb2", "ext4", "Secret"), &["/media/ann/Secret"])));
    }
    vec![Arg::L(objects)]
}

fn obj(name: &str) -> String {
    if name.contains('_') && !name.starts_with("dm") {
        format!("{DRIVES}{name}")
    } else {
        format!("{BLOCKS}{name}")
    }
}

#[test]
fn the_recorded_device_list_is_read_into_blocks_and_drives() {
    let d = devices(&recorded(true));
    assert_eq!(d.blocks.len(), 7);
    assert_eq!(d.disks.iter().map(|x| x.object.rsplit('/').next().unwrap()).collect::<Vec<_>>(), ["Samsung_SSD", "SanDisk_Cruzer", "DVD_RW"]);
    let stick = d.blocks.iter().find(|b| b.object.ends_with("sdb1")).unwrap();
    assert_eq!((stick.device.as_slice(), stick.label.as_str(), stick.fs_type.as_str()), (&b"/dev/sdb1"[..], "STICK", "vfat"));
    assert_eq!(stick.mount_points, [b"/media/ann/STICK".to_vec()]);
    assert!(stick.filesystem && !stick.encrypted && !stick.system);
    let clear = d.by_device(b"/dev/mapper/luks-5e1f").unwrap();
    assert_eq!(clear.backing, obj("sdb2"));
    assert_eq!(d.by_device(b"/dev/disk/by-id/usb-sdb1").unwrap().object, obj("sdb1"));
    assert!(d.by_device(b"/dev/sdz9").is_none());
    assert!(devices(&[]).blocks.is_empty() && devices(&[s("garbage")]).disks.is_empty());
}

#[test]
fn eject_unmounts_every_volume_of_the_drive_locks_the_encrypted_one_and_powers_the_stick_off() {
    let d = devices(&recorded(true));
    let want = vec![Step::Unmount(obj("sdb1")), Step::Unmount(obj("dm_2d0")), Step::Lock(obj("sdb2")), Step::PowerOff(obj("SanDisk_Cruzer"))];
    assert_eq!(d.eject_plan(b"/media/ann/STICK").unwrap(), want);
    assert_eq!(d.eject_plan(b"/media/ann/Secret").unwrap(), want);
    assert_eq!(d.eject_plan(b"/media/ann/Ubuntu").unwrap(), [Step::Unmount(obj("sr0")), Step::Eject(obj("DVD_RW"))]);
}

#[test]
fn unmount_leaves_the_drive_and_its_other_volumes_alone() {
    let d = devices(&recorded(true));
    assert_eq!(d.unmount_plan(b"/media/ann/STICK").unwrap(), [Step::Unmount(obj("sdb1"))]);
    assert_eq!(d.unmount_plan(b"/media/ann/Secret").unwrap(), [Step::Unmount(obj("dm_2d0")), Step::Lock(obj("sdb2"))]);
    assert_eq!(d.eject_plan(b"/media/ann/img").unwrap(), [Step::Unmount(obj("loop3"))]);
    assert!(d.eject_plan(b"/media/ann/gone").is_none() && d.unmount_plan(b"/media/ann").is_none());
    let locked = devices(&recorded(false));
    assert_eq!(locked.eject_plan(b"/media/ann/STICK").unwrap(), [Step::Unmount(obj("sdb1")), Step::PowerOff(obj("SanDisk_Cruzer"))]);
}

#[test]
fn an_encrypted_volume_is_locked_until_its_cleartext_device_appears() {
    let locked = devices(&recorded(false));
    let outer = locked.by_device(b"/dev/sdb2").unwrap();
    assert!(locked.locked(outer) && locked.cleartext(outer).is_none());
    let open = devices(&recorded(true));
    let outer = open.by_device(b"/dev/sdb2").unwrap();
    assert!(!open.locked(outer));
    assert_eq!(open.cleartext(outer).unwrap().mount_points, [b"/media/ann/Secret".to_vec()]);
    assert!(!open.locked(open.by_device(b"/dev/sdb1").unwrap()));
}

fn failure(name: &str, message: &str) -> Failure {
    Failure { name: name.into(), message: message.into() }
}

#[test]
fn a_busy_volume_stops_the_eject_and_a_drive_that_will_not_power_off_does_not() {
    let steps = devices(&recorded(true)).eject_plan(b"/media/ann/STICK").unwrap();
    let mut done = Vec::new();
    let busy = run(&steps, &mut |s| {
        done.push(s.clone());
        if matches!(s, Step::Unmount(o) if o.ends_with("dm_2d0")) { Err(failure("org.freedesktop.UDisks2.Error.DeviceBusy", "Error unmounting /dev/dm-0: target is busy")) } else { Ok(()) }
    });
    assert_eq!(busy.unwrap_err().reason(), S::ErrorsFsDriveBusy.en());
    assert_eq!(done.len(), 2, "nothing is locked or powered off after a busy volume: {done:?}");
    let mut done = Vec::new();
    let stays = run(&steps, &mut |s| {
        done.push(s.clone());
        if s.of_drive() { Err(failure("org.freedesktop.UDisks2.Error.Failed", "cannot power off")) } else { Ok(()) }
    });
    assert!(stays.is_ok() && done.len() == 4);
    let cancelled = run(&steps, &mut |_| Err(failure("org.freedesktop.UDisks2.Error.NotAuthorizedDismissed", "Not authorized")));
    assert_eq!(io::Error::from(cancelled.unwrap_err()).kind(), io::ErrorKind::Interrupted);
}

#[test]
fn failures_read_as_sentences_and_a_dismissed_password_prompt_is_silent() {
    assert_eq!(failure("org.freedesktop.UDisks2.Error.Failed", " Error mounting /dev/sdb1: wrong fs type \n").reason(), "Error mounting /dev/sdb1: wrong fs type");
    assert_eq!(failure("org.freedesktop.DBus.Error.ServiceUnknown", "The name is not activatable").reason(), S::ErrorsFsNoUdisks.en());
    let wrong = unlock_error(failure("org.freedesktop.UDisks2.Error.Failed", "Error unlocking /dev/sdb2: Failed to activate device: Incorrect passphrase."));
    assert_eq!((wrong.kind(), wrong.to_string()), (io::ErrorKind::PermissionDenied, S::ErrorsFsWrongPassphrase.en().to_owned()));
    assert_eq!(unlock_error(failure("org.freedesktop.UDisks2.Error.NotAuthorizedDismissed", "Not authorized")).kind(), io::ErrorKind::Interrupted);
    assert_eq!(io::Error::from(failure("org.freedesktop.UDisks2.Error.Failed", "Operation not permitted")).kind(), io::ErrorKind::Other);
    let busy: io::Error = failure("org.freedesktop.UDisks2.Error.DeviceBusy", "target is busy").into();
    let shown = crate::places::eject_error(Path::new("/media/ann/STICK"), &busy).to_string();
    assert!(shown.starts_with("Cannot eject /media/ann/STICK: The drive is in use"), "{shown}");
    let quiet = crate::places::drive_error(S::ErrorsFsUnmountFailed, Path::new("/media/ann/STICK"), &io::Error::from(io::ErrorKind::Interrupted));
    assert_eq!(quiet.to_string(), flit_platform::CANCELLED);
}

#[test]
fn only_device_changes_wake_the_drive_list() {
    assert!(relevant("InterfacesAdded", &[s("/org/freedesktop/UDisks2/block_devices/sdc1")]));
    assert!(relevant("InterfacesRemoved", &[]));
    assert!(relevant("PropertiesChanged", &[s(DRIVE), Arg::L(Vec::new())]));
    assert!(relevant("Mounted", &[]) && relevant("VolumeAdded", &[]));
    assert!(!relevant("PropertiesChanged", &[s("org.freedesktop.UDisks2.Job"), Arg::L(Vec::new())]));
    assert!(!relevant("PropertiesChanged", &[s("org.freedesktop.UDisks2.Drive.Ata")]));
    assert!(!relevant("NameOwnerChanged", &[]));
}

#[test]
fn no_bus_at_the_address_is_an_error_not_a_hang() {
    let started = Instant::now();
    assert!(Bus::open("unix:path=/nonexistent/flit-test-bus").is_err());
    assert!(started.elapsed() < Duration::from_secs(5));
}
