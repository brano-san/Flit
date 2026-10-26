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
