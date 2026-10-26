use super::*;
use flit_core::FsErrorKind;

fn p(s: &str) -> FsPath {
    FsPath::parse(s).unwrap()
}

#[test]
fn dialogs_get_the_drive_letter_and_the_share() {
    assert_eq!(letter(&p(r"Z:\")).as_deref(), Some("Z:"));
    assert_eq!(letter(&p(r"\\nas\share")), None);
    assert_eq!(share_root(&p(r"\\nas\share\deep\dir")).as_deref(), Some(r"\\nas\share"));
    assert_eq!(share_root(&p(r"C:\dir")), None);
    assert_eq!(crate::places::share_name(r"\\nas\share"), r"share (\\nas)");
    assert_eq!(crate::places::share_name(r"\\nas\share\sub\"), r"sub (\\nas\share)");
}

#[test]
fn shares_and_shell_places_count_as_remote() {
    assert!(remote(&p(r"\\nas\share\x")));
    assert!(remote(&FsPath::network()));
    assert!(remote(&p(r"This PC\Pixel 7")));
    assert!(!remote(&p(r"C:\Windows")));
    assert!(!remote(&FsPath::computer()));
}

#[test]
fn only_share_roots_and_logon_failures_ask_for_credentials() {
    let denied = FsError { kind: FsErrorKind::AccessDenied, code: ERROR_ACCESS_DENIED.0 as i64 };
    let logon = FsError { kind: FsErrorKind::Other, code: ERROR_LOGON_FAILURE.0 as i64 };
    assert!(needs_logon(&p(r"\\nas\share"), &denied));
    assert!(!needs_logon(&p(r"\\nas\share\private"), &denied));
    assert!(needs_logon(&p(r"\\nas\share\private"), &logon));
    assert!(!needs_logon(&p(r"C:\private"), &logon));
    DECLINED.lock().unwrap().get_or_insert_with(HashSet::new).insert(r"\\nas\share".into());
    assert!(!needs_logon(&p(r"\\NAS\Share"), &logon));
}

#[test]
fn a_cancelled_dialog_is_not_an_error() {
    assert!(finished(NO_ERROR).is_ok());
    assert!(finished(ERROR_CANCELLED).is_ok());
    assert!(finished(WIN32_ERROR(CANCELLED_BY_USER)).is_ok());
    assert!(finished(ERROR_LOGON_FAILURE).is_err());
}
