use super::*;
use crate::temp::Temp;

#[test]
fn modes_read_like_ls_with_the_octal_value() {
    assert_eq!(mode_text(0o100644), "rw-r--r-- (644)");
    assert_eq!(mode_text(0o40755), "rwxr-xr-x (755)");
    assert_eq!(mode_text(0o4755), "rwsr-xr-x (4755)");
    assert_eq!(mode_text(0o1777), "rwxrwxrwt (1777)");
    assert_eq!(mode_text(0o2640), "rw-r-S--- (2640)");
}

#[test]
fn a_file_gets_access_and_date_groups() {
    let t = Temp::new("props");
    let f = t.file("a.txt", 3);
    std::fs::set_permissions(&f, std::os::unix::fs::PermissionsExt::from_mode(0o640)).unwrap();
    let groups = properties(&f);
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0].rows[0].1, "rw-r----- (640)");
    let uid = unsafe { libc::getuid() };
    assert!(groups[0].rows[1].1.ends_with(&format!("({uid})")) || groups[0].rows[1].1 == uid.to_string());
    assert!(groups[1].rows.iter().any(|(k, v)| k == S::PropertiesChanged.t() && v.contains(':')));
    assert!(properties(&t.path().join("missing")).is_empty());
}

fn acl_blob(entries: &[(u16, u16, u32)]) -> Vec<u8> {
    let mut b = 2u32.to_le_bytes().to_vec();
    for &(tag, perm, id) in entries {
        b.extend([tag.to_le_bytes().as_slice(), &perm.to_le_bytes(), &id.to_le_bytes()].concat());
    }
    b
}

#[test]
fn access_lists_read_like_getfacl_and_minimal_ones_are_marked() {
    let name = |id: u32, user: bool| format!("{}{id}", if user { "u" } else { "g" });
    let full = acl_blob(&[(1, 6, u32::MAX), (2, 4, 7), (4, 4, u32::MAX), (8, 5, 9), (0x10, 5, u32::MAX), (0x20, 0, u32::MAX)]);
    assert_eq!(acl_text(&full, name), Some(("user::rw-, user:u7:r--, group::r--, group:g9:r-x, mask::r-x, other::---".to_owned(), true)));
    let minimal = acl_blob(&[(1, 7, u32::MAX), (4, 5, u32::MAX), (0x20, 5, u32::MAX)]);
    assert_eq!(acl_text(&minimal, name).map(|(_, named)| named), Some(false));
    assert_eq!(acl_text(&[1, 0, 0, 0], name), None);
    assert_eq!(acl_text(&full[..full.len() - 1], name), None);
    assert_eq!(acl_text(&acl_blob(&[(0x40, 7, 0)]), name), None);
}

fn set_xattr(p: &Path, name: &str, value: &[u8]) {
    use std::os::unix::ffi::OsStrExt;
    let p = std::ffi::CString::new(p.as_os_str().as_bytes()).unwrap();
    let name = std::ffi::CString::new(name).unwrap();
    assert_eq!(unsafe { libc::setxattr(p.as_ptr(), name.as_ptr(), value.as_ptr().cast(), value.len(), 0) }, 0, "{}", std::io::Error::last_os_error());
}

#[test]
fn links_show_their_target_and_say_when_it_is_missing() {
    let t = Temp::new("props-link");
    t.file("a.txt", 3);
    std::os::unix::fs::symlink("a.txt", t.path().join("good")).unwrap();
    std::os::unix::fs::symlink("../gone", t.path().join("bad")).unwrap();
    let good = properties(&t.path().join("good"));
    assert_eq!((good[0].title.as_str(), &good[0].rows[..]), (S::PropertiesLink.t(), &[(S::PropertiesLinkTarget.t().to_owned(), "a.txt".to_owned())][..]));
    assert_eq!(properties(&t.path().join("bad"))[0].rows[0].1, "../gone (missing)");
    assert!(properties(&t.path().join("a.txt")).iter().all(|g| g.title != S::PropertiesLink.t()), "a plain file has no link group");
}

#[test]
fn access_lists_and_extended_attributes_get_their_own_group() {
    let t = Temp::new("props-xattr");
    let f = t.file("a.txt", 3);
    set_xattr(&f, "user.xdg.comment", b"note");
    set_xattr(&f, "system.posix_acl_access", &acl_blob(&[(1, 6, u32::MAX), (2, 4, 4242), (4, 4, u32::MAX), (0x10, 4, u32::MAX), (0x20, 0, u32::MAX)]));
    let groups = properties(&f);
    let security = groups.iter().find(|g| g.title == S::PropertiesSecurity.t()).expect("a security group");
    assert_eq!(security.rows[0].0, S::PropertiesAcl.t());
    assert!(security.rows[0].1.contains(":r--") && security.rows[0].1.starts_with("user::rw-, user:"), "{}", security.rows[0].1);
    assert_eq!(security.rows.last().unwrap(), &(S::PropertiesXattrs.t().to_owned(), "user.xdg.comment".to_owned()));
    let plain = t.file("b.txt", 1);
    assert!(extended_rows(&plain).is_empty());
    assert_eq!(properties(&plain).len(), 2);
}
