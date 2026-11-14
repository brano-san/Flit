use std::ffi::CStr;

pub(crate) fn named_time_locale(var: impl Fn(&str) -> Option<String>) -> bool {
    let name = ["LC_ALL", "LC_TIME", "LANG"].iter().filter_map(|k| var(k)).find(|v| !v.is_empty()).unwrap_or_default();
    let base = name.split(['.', '@']).next().unwrap_or_default();
    !matches!(base, "" | "C" | "POSIX")
}

fn langinfo(locale: libc::locale_t, item: libc::nl_item) -> Option<String> {
    let text = unsafe { libc::nl_langinfo_l(item, locale) };
    (!text.is_null()).then(|| unsafe { CStr::from_ptr(text) }.to_string_lossy().into_owned())
}

pub(crate) fn date_patterns() -> Option<(String, String)> {
    if !named_time_locale(|k| std::env::var(k).ok()) {
        return None;
    }
    let locale = unsafe { libc::newlocale(libc::LC_TIME_MASK, c"".as_ptr(), std::ptr::null_mut()) };
    if locale.is_null() {
        return None;
    }
    let formats = langinfo(locale, libc::D_FMT).zip(langinfo(locale, libc::T_FMT));
    unsafe { libc::freelocale(locale) };
    let (date, time) = formats?;
    flit_platform::posix_date_patterns(&date, &time)
}

#[cfg(test)]
#[path = "../test/locale.rs"]
mod tests;
