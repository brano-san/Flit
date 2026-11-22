use super::*;
use std::os::unix::ffi::OsStrExt;

#[test]
fn download_origin_attributes_are_allowed_and_other_metadata_is_named() {
    let temp = crate::temp::Temp::new("portable-dedup");
    let file = temp.file("download.txt", 12);
    let path = FsPath::parse_exact(file.as_os_str().as_bytes()).unwrap();
    let handle = open(&path).unwrap();
    write_attr(&handle, c"user.xdg.origin.url", Some(b"https://example.invalid/download")).unwrap();
    write_attr(&handle, c"user.xdg.referrer.url", Some(b"https://example.invalid/")).unwrap();
    assert_eq!(other_user_attribute(&path).unwrap(), None);
    write_attr(&handle, c"user.xdg.comment", Some(b"keep this caption")).unwrap();
    assert_eq!(other_user_attribute(&path).unwrap().as_deref(), Some(b"user.xdg.comment".as_slice()));
}
