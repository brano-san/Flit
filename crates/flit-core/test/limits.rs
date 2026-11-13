use super::*;
use crate::archive;

#[test]
fn fat_volumes_cap_files_below_four_gibibytes() {
    for fs in ["FAT32", "fat", "FAT16", "FAT12", "vfat", "msdos", "umsdos"] {
        assert_eq!(max_file_size(fs), Some(FAT_MAX_FILE), "{fs}");
    }
    for fs in ["NTFS", "exFAT", "exfat", "ntfs3", "fuseblk", "ext4", "ReFS", ""] {
        assert_eq!(max_file_size(fs), None, "{fs}");
    }
    let fat = max_file_size("FAT32");
    assert!(!exceeds(FAT_MAX_FILE, fat) && exceeds(FAT_MAX_FILE + 1, fat) && !exceeds(u64::MAX, None));
}

#[test]
fn too_large_conflicts_are_told_apart_from_name_and_unreadable_ones() {
    let dest = FsPath::parse(r"E:\backup").unwrap();
    let file = FsPath::parse(r"C:\v\movie.mkv").unwrap();
    let space = short_of_space(&dest, 10, 3);
    assert!(too_large(&space) && is_short_of_space(&space) && (space.from.size, space.to.size) == (10, 3));
    let big = over_limit(&file, &dest.join(b"movie.mkv"), 5 << 30, FAT_MAX_FILE);
    assert!(too_large(&big) && !is_short_of_space(&big) && !archive::is_unreadable(&big));
    let plain = Conflict { source: file.clone(), target: dest.clone(), from: Meta::default(), to: Meta::default() };
    assert!(!too_large(&plain) && !too_large(&archive::unreadable(&file, Meta::default())));
}

#[test]
fn fat_and_exfat_refuse_windows_names_and_other_file_systems_take_anything() {
    for fs in ["vfat", "msdos", "exfat", "FAT32", "exFAT"] {
        assert!(windows_names(fs), "{fs}");
    }
    for fs in ["ext4", "btrfs", "ntfs3", "ntfs", "fuseblk", "NTFS", "tmpfs", ""] {
        assert!(!windows_names(fs), "{fs}");
    }
    let cases: [(&[u8], Option<BadName>); 21] = [
        (b"a:b.txt", Some(BadName::Char(b':'))),
        (b"what?", Some(BadName::Char(b'?'))),
        (b"star*", Some(BadName::Char(b'*'))),
        (b"q\"uote", Some(BadName::Char(b'"'))),
        (b"<x>", Some(BadName::Char(b'<'))),
        (b"pi|pe", Some(BadName::Char(b'|'))),
        (br"back\slash", Some(BadName::Char(b'\\'))),
        (b"tab\there", Some(BadName::Char(b'\t'))),
        (b"dot.", Some(BadName::End)),
        (b"space ", Some(BadName::End)),
        (b"CON", Some(BadName::Reserved)),
        (b"nul.txt", Some(BadName::Reserved)),
        (b"Com1", Some(BadName::Reserved)),
        (b"lpt9.log", Some(BadName::Reserved)),
        (b"aux .c", Some(BadName::Reserved)),
        (b"COM0", None),
        (b"CONSOLE", None),
        (b"com10", None),
        (b"report (2).txt", None),
        (b".hidden", None),
        ("привет.txt".as_bytes(), None),
    ];
    for (name, bad) in cases {
        assert_eq!(bad_name(name), bad, "{:?}", String::from_utf8_lossy(name));
    }
}

#[test]
fn bad_name_errors_name_the_rule() {
    let path = |s: &str| FsPath::parse(s).unwrap();
    assert_eq!(bad_name_error(&path("/media/usb/a:b.txt"), BadName::Char(b':')), r#"/media/usb/a:b.txt: FAT and exFAT drives do not allow : in names (nor any of \ : * ? " < > |)"#);
    assert!(bad_name_error(&path("/m/a"), BadName::Char(1)).contains("do not allow U+0001 in names"));
    assert_eq!(bad_name_error(&path("/m/x."), BadName::End), "/m/x.: FAT and exFAT drives do not allow names that end with a dot or a space");
    assert_eq!(bad_name_error(&path("/m/con.txt"), BadName::Reserved), "/m/con.txt: FAT and exFAT drives do not allow the name con.txt, which is reserved for devices");
}

#[test]
fn fat_safe_names_replace_what_fat_refuses() {
    let safe = |n: &str| String::from_utf8(fat_safe(n.as_bytes())).unwrap();
    assert_eq!(safe("a:b?.txt"), "a_b_.txt");
    assert_eq!(safe("notes. . "), "notes");
    assert_eq!(safe("..."), "_");
    assert_eq!(safe("con.txt"), "_con.txt");
    assert_eq!(safe("fine.txt"), "fine.txt");
    assert!(["a:b", "x.", "COM1", "tab\tname"].iter().all(|n| bad_name(&fat_safe(n.as_bytes())).is_none()));
}
