use super::*;

fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
    move |k| pairs.iter().find(|(name, _)| *name == k).map(|(_, v)| (*v).to_owned())
}

#[test]
fn a_desktop_language_with_a_region_uses_its_own_date_format() {
    assert!(named_time_locale(env(&[("LANG", "ru_RU.UTF-8")])));
    assert!(named_time_locale(env(&[("LANG", "C.UTF-8"), ("LC_TIME", "de_DE.UTF-8")])));
    assert!(named_time_locale(env(&[("LC_ALL", "sr_RS@latin"), ("LANG", "C")])));
}

#[test]
fn the_c_locale_and_no_locale_keep_the_iso_dates() {
    assert!(!named_time_locale(env(&[])));
    assert!(!named_time_locale(env(&[("LANG", "C.UTF-8")])));
    assert!(!named_time_locale(env(&[("LANG", "POSIX")])));
    assert!(!named_time_locale(env(&[("LC_ALL", ""), ("LC_TIME", "C"), ("LANG", "en_US.UTF-8")])));
    let c_only = unsafe { libc::newlocale(libc::LC_TIME_MASK, c"C".as_ptr(), std::ptr::null_mut()) };
    assert!(!c_only.is_null());
    let formats = langinfo(c_only, libc::D_FMT).zip(langinfo(c_only, libc::T_FMT));
    unsafe { libc::freelocale(c_only) };
    assert_eq!(formats, Some(("%m/%d/%y".to_owned(), "%H:%M:%S".to_owned())), "this is why the C locale keeps ISO dates");
}
