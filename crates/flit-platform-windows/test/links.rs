use super::*;
use flit_core::provider::{Failure, OpCtl, Recover, Resolve};
use flit_platform::Job;
use std::cell::Cell;
use std::os::windows::ffi::OsStringExt;

#[test]
fn equal_unc_share_names_do_not_prove_that_an_unknown_volume_supports_hard_links() {
    let source = FsPath::parse(r"\\server\share\source.txt").unwrap();
    let destination = FsPath::parse(r"\\server\share\out").unwrap();
    assert_eq!(source.root_bytes(), destination.root_bytes());
    let root: Vec<u16> = r"\\server\share\".encode_utf16().chain([0]).collect();
    let unknown = identify_volume(&root, false, |read, _| {
        assert_eq!(read, root);
        Err(error(ERROR_NOT_SUPPORTED))
    });
    assert!(unknown.is_none());
    assert!(!same_volume(&source, &destination, unknown.as_ref(), unknown.as_ref()));
    assert_eq!(item_choices(&source, &destination, true, false, unknown.as_ref(), unknown.as_ref()).0, [true, true, false, false]);
    assert_ne!(item_choices(&source, &destination, true, false, None, None), LinkChoices::default());
    assert_eq!(item_choices(&source, &destination, false, true, None, None).0, [true, true, false, false]);
    let known = (r"\\?\Volume{known}\".encode_utf16().collect::<Vec<_>>(), true);
    let other = (r"\\?\Volume{other}\".encode_utf16().collect::<Vec<_>>(), true);
    assert!(same_volume(&source, &destination, Some(&known), Some(&known)));
    assert!(!same_volume(&source, &destination, Some(&known), Some(&other)));
    assert!(!same_volume(&source, &destination, Some(&known), None));
    assert_eq!(item_choices(&source, &destination, true, false, Some(&known), Some(&known)).0, [true, true, false, true]);
    assert_eq!(item_choices(&source, &destination, true, false, Some(&known), Some(&other)).0, [true, true, false, false]);
}

#[test]
fn elevated_arguments_preserve_unpaired_utf16_and_never_substitute_a_neighbor() {
    let mut units: Vec<u16> = r"C:\source-".encode_utf16().collect();
    units.extend([0xD800, b'.' as u16, b't' as u16, b'x' as u16, b't' as u16]);
    let source = FsPath::from_utf16(&units).unwrap();
    let neighbor = FsPath::parse("C:\\source-\u{FFFD}.txt").unwrap();
    assert_ne!(encoded(&source), encoded(&neighbor));
    assert_eq!(decoded(OsString::from(encoded(&source))), Some(source.clone()));
    let path = FsPath::parse(r"C:\out\link with spaces").unwrap();
    let launched = Cell::new(false);
    elevated_with(&source, &path, |args| {
        launched.set(true);
        assert_eq!(args[0], "--create-symbolic-link");
        assert_eq!(decoded(OsString::from(&args[1])), Some(source.clone()));
        assert_eq!(decoded(OsString::from(&args[2])), Some(path.clone()));
        assert_eq!(OsString::from_wide(&units).encode_wide().collect::<Vec<_>>(), units);
        Ok(0)
    }).unwrap();
    assert!(launched.get());
}

#[test]
fn malformed_hex_nuls_extra_arguments_and_overlong_elevation_are_rejected() {
    let path = FsPath::parse(r"C:\out").unwrap();
    for arg in ["", "0", "zzzz", "0043003a005c0000", "002f0074006d0070"] {
        assert!(decoded(OsString::from(arg)).is_none());
        assert_eq!(symbolic_from_command_line([OsString::from(arg), OsString::from(encoded(&path))]), ERROR_INVALID_PARAMETER.0 as i32);
    }
    assert_eq!(symbolic_from_command_line([OsString::from(encoded(&path)), OsString::from(encoded(&path)), OsString::from("extra")]), ERROR_INVALID_PARAMETER.0 as i32);
    let long = FsPath::parse(&format!("C:\\{}", "a".repeat(9000))).unwrap();
    let result = elevated_with(&long, &path, |_| panic!("overlong launch must be rejected before UAC"));
    assert_eq!(WIN32_ERROR::from_error(&result.unwrap_err()), Some(ERROR_FILENAME_EXCED_RANGE));
}

#[test]
fn privilege_failure_names_developer_mode_and_retry_uses_the_existing_elevated_launcher() {
    let ctl = OpCtl::new(Some(Resolve::KeepBoth));
    let answer = ctl.clone();
    let job = Job::asking(ctl, Box::new(|_| {}), Box::new(|_| {}), Box::new(move |failure: Failure| {
        assert_eq!(failure.error, flit_core::i18n::S::ErrorsEngineSymlinkPrivilege.en());
        answer.answer_failure(Recover::Retry, false);
    }));
    let calls = Cell::new(0);
    let target = FsPath::parse(r"C:\source with spaces.txt").unwrap();
    let result = with_elevation(&job, &target, || Err(error(ERROR_PRIVILEGE_NOT_HELD)), || { calls.set(calls.get() + 1); Ok(()) });
    assert!(result.is_ok());
    assert_eq!(calls.get(), 1);
}

#[test]
fn declining_or_cancelling_privilege_retry_never_launches_an_elevated_process() {
    for cancel in [false, true] {
        let ctl = OpCtl::new(Some(Resolve::KeepBoth));
        let answer = ctl.clone();
        let job = Job::asking(ctl, Box::new(|_| {}), Box::new(|_| {}), Box::new(move |_| {
            if cancel { answer.cancel(); } else { answer.answer_failure(Recover::Skip, false); }
        }));
        let target = FsPath::parse(r"C:\source.txt").unwrap();
        let result = with_elevation(&job, &target, || Err(error(ERROR_PRIVILEGE_NOT_HELD)), || panic!("elevated after declining"));
        assert_eq!(WIN32_ERROR::from_error(&result.unwrap_err()), Some(if cancel { ERROR_CANCELLED } else { ERROR_PRIVILEGE_NOT_HELD }));
    }
}

#[test]
fn malformed_elevated_link_arguments_are_rejected_without_writing_anything() {
    assert_eq!(symbolic_from_command_line(Vec::<OsString>::new()), ERROR_INVALID_PARAMETER.0 as i32);
    assert_eq!(symbolic_from_command_line([OsString::from("relative"), OsString::from("also relative")]), ERROR_INVALID_PARAMETER.0 as i32);
    assert_eq!(choices(&[], &FsPath::computer()), LinkChoices::default());
}
