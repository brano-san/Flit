use super::*;

const SYMLINK: u32 = 0xA000_000C;
const MOUNT_POINT: u32 = 0xA000_0003;
const CLOUD: u32 = 0x9000_001A;
const DEDUP: u32 = 0x8000_0013;

#[test]
fn only_name_surrogate_reparse_points_are_links() {
    let dir = FILE_ATTRIBUTE_DIRECTORY.0 | FILE_ATTRIBUTE_REPARSE_POINT.0;
    assert_eq!(map_attrs(dir, SYMLINK), attr::DIR | attr::LINK);
    assert_eq!(map_attrs(dir, MOUNT_POINT), attr::DIR | attr::LINK);
    assert_eq!(map_attrs(dir, CLOUD), attr::DIR);
    assert_eq!(map_attrs(FILE_ATTRIBUTE_REPARSE_POINT.0, DEDUP), 0);
    assert_eq!(map_attrs(FILE_ATTRIBUTE_REPARSE_POINT.0, SYMLINK), attr::LINK);
    assert_eq!(map_attrs(FILE_ATTRIBUTE_DIRECTORY.0, SYMLINK), attr::DIR, "EaSize is not a reparse tag without the reparse attribute");
    assert_eq!(map_attrs(FILE_ATTRIBUTE_HIDDEN.0 | FILE_ATTRIBUTE_SYSTEM.0 | FILE_ATTRIBUTE_READONLY.0, 0), attr::HIDDEN | attr::SYSTEM | attr::READONLY);
}
