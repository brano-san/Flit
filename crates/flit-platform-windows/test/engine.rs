use super::*;
use flit_core::i18n::{self, Lang};
use windows::core::{Error, HRESULT};

#[test]
fn windows_operation_and_shell_errors_follow_the_interface_language() {
    let original = i18n::lang();
    i18n::set_lang(Lang::Ru);
    let path = FsPath::parse(r"C:\missing.txt").unwrap();
    let error = Error::new(HRESULT(0x80070005u32 as i32), "system language");
    let expected = S::ErrorsOsPermissionDenied.t();
    let why = reason(&path, &error);
    assert_eq!(i18n::localize(&why), expected);
    let shell = crate::err(error);
    assert_eq!(shell.code, 0x80070005u32 as i32 as i64);
    assert_eq!(i18n::localize(&shell.message), expected);
    assert_eq!(i18n::localize(&shell.to_string()), format!("{expected} (0x80070005)"));
    let summary = tr_en!(S::ErrorsEngineFailed, count = 2, error = format!("{}: {why}", path.display()));
    assert_eq!(i18n::localize(&summary), flit_core::tr!(S::ErrorsEngineFailed, count = 2, error = format!("{}: {expected}", path.display())));
    i18n::set_lang(original);
}

#[test]
fn windows_errors_translate_known_codes_without_overwriting_unknown_system_reasons() {
    let original = i18n::lang();
    i18n::set_lang(Lang::De);
    let path = FsPath::parse(r"C:\missing.txt").unwrap();
    let known = Error::new(HRESULT(0x80070003u32 as i32), "system language");
    assert_eq!(i18n::localize(&reason(&path, &known)), S::ErrorsOsNotFound.t());
    for code in [0x80004005u32, 0x80040005, 0x80071234] {
        let unknown = Error::new(HRESULT(code as i32), "a specific system reason");
        let expected = unknown.message();
        assert_eq!(reason(&path, &unknown), expected.trim_end());
        assert_eq!(crate::err(unknown).message, expected);
    }
    i18n::set_lang(original);
}
