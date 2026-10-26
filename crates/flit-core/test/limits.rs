use super::*;
use crate::archive;

#[test]
fn fat_volumes_cap_files_below_four_gibibytes() {
    for fs in ["FAT32", "fat", "FAT16", "FAT12"] {
        assert_eq!(max_file_size(fs), Some(FAT_MAX_FILE), "{fs}");
    }
    for fs in ["NTFS", "exFAT", "ReFS", ""] {
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
