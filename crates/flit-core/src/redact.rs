use regex::Regex;
use std::borrow::Cow;
use std::sync::LazyLock;

static USERINFO: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)([a-z][a-z0-9+.-]*://[^/\\:@?#\s<>"]*:)[^/\\\s<>"]+@"#).unwrap()
});
static PASSWORD_PARAM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r##"(?i)(\bpassword[ \t]*=[ \t]*)(?:\\"[^\r\n]*?\\"|"(?:\\.|[^"\\\r\n])*"?|'[^'\r\n]*'?|[^\s&<>"#]+)"##).unwrap()
});

pub fn passwords(text: &str) -> Cow<'_, str> {
    let urls = if text.contains("://") {
        USERINFO.replace_all(text, "${1}***@")
    } else {
        Cow::Borrowed(text)
    };
    match if urls.contains('=') {
        PASSWORD_PARAM.replace_all(&urls, "${1}***")
    } else {
        Cow::Borrowed(urls.as_ref())
    } {
        Cow::Owned(s) => Cow::Owned(s),
        Cow::Borrowed(_) => urls,
    }
}
#[cfg(test)]
#[path = "../test/redact.rs"]
mod tests;
