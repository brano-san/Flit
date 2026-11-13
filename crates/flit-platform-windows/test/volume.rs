use super::*;

fn missing_drive() -> String {
    (b'D'..=b'Z').rev().map(|c| format!("{}:\\", c as char)).find(|r| !std::path::Path::new(r).exists()).unwrap()
}

#[test]
fn a_label_for_a_missing_drive_fails_with_a_readable_reason() {
    let missing = missing_drive();
    let failed = label(&FsPath::parse(&missing).unwrap(), "Backup", false).err().unwrap();
    assert!(failed.starts_with("Cannot change the drive label: ") && failed.len() > "Cannot change the drive label: ".len(), "{failed}");
}

#[test]
fn the_elevated_helper_reports_the_windows_error_as_its_exit_code() {
    assert_eq!(label_from_command_line(None, None), ERROR_INVALID_PARAMETER.0 as i32);
    assert_eq!(label_from_command_line(Some("not a drive".into()), Some("x".into())), ERROR_INVALID_PARAMETER.0 as i32);
    assert_ne!(label_from_command_line(Some(missing_drive().into()), Some("x".into())), 0);
}
