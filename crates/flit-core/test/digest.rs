use super::*;

#[test]
fn expected_values_are_found_in_pasted_text() {
    let sha = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    assert_eq!(expected(&format!("SHA256: {}  file.iso", sha.to_uppercase())), Some(sha.to_owned()));
    assert_eq!(expected("crc 0xDEADBEEF"), Some("deadbeef".to_owned()));
    assert_eq!(expected("not a hash"), None);
}

#[test]
fn matching_names_the_algorithm() {
    let values = vec![(HashKind::Crc32, "00000000".to_owned()), (HashKind::Md5, "d41d8cd98f00b204e9800998ecf8427e".to_owned())];
    assert_eq!(matching("D41D8CD98F00B204E9800998ECF8427E", &values), Some(HashKind::Md5));
    assert_eq!(matching("d41d8cd98f00b204e9800998ecf8427f", &values), None);
}
