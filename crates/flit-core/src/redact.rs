use regex::Regex;
use std::borrow::Cow;
use std::sync::LazyLock;

#[allow(clippy::expect_used)]
fn redaction_pattern(pattern: &'static str) -> Regex {
    Regex::new(pattern).expect("invalid built-in redaction pattern")
}

static USERINFO: LazyLock<Regex> = LazyLock::new(|| {
    redaction_pattern(r#"(?i)([a-z][a-z0-9+.-]*://[^/\\:@?#\s<>"]*:)[^/\\\s<>"]+@"#)
});
static PASSWORD_PARAM: LazyLock<Regex> = LazyLock::new(|| {
    redaction_pattern(r##"(?i)(\bpassword[ \t]*=[ \t]*)(?:\\"[^\r\n]*?\\"|"(?:\\.|[^"\\\r\n])*"?|'[^'\r\n]*'?|[^\s&<>"#]+)"##)
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

static EMAIL: LazyLock<Regex> = LazyLock::new(|| {
    redaction_pattern(r"(?i)\w[\w.!#$%&'*+/=?^{}|~\-]*@\w(?:[\w.-]*\w)?")
});
static UNC_HOST: LazyLock<Regex> = LazyLock::new(|| {
    redaction_pattern(r#"(?i)(^|[\s"'(=\[]|\w{2}:)(\\{2,4}(?:\?\\{1,2}UNC\\{1,2})?)[\w.-]+"#)
});
static FORWARD_UNC_HOST: LazyLock<Regex> = LazyLock::new(|| {
    redaction_pattern(r#"(^|[\s"'(=\[])(//)[\w.-]+"#)
});

fn path_end(rest: &str) -> bool {
    rest.chars().next().is_none_or(|c| c.is_whitespace() || "/\\\"')]>".contains(c))
}

pub fn crash_text(text: &str, home: &str, user: &str, windows: bool) -> String {
    let mut text = passwords(text).into_owned();
    let home = home.trim_end_matches(['/', '\\']);
    if !home.is_empty() && !(windows && home.ends_with(':')) {
        let pattern = if windows {
            let parts: Vec<_> = home.split(['\\', '/']).map(regex::escape).collect();
            format!(r"(?i){}", parts.join(r"(?:\\{1,2}|/)"))
        } else {
            regex::escape(home)
        };
        if let Ok(home) = Regex::new(&pattern) {
            text = home.replace_all(&text, |caps: &regex::Captures<'_>| {
                let Some(matched) = caps.get(0) else { return String::new() };
                if path_end(&text[matched.end()..]) { "~".to_owned() } else { matched.as_str().to_owned() }
            }).into_owned();
        }
    }
    let text = UNC_HOST.replace_all(&text, "${1}${2}<host>");
    let text = FORWARD_UNC_HOST.replace_all(&text, "${1}${2}<host>");
    let text = EMAIL.replace_all(&text, "");
    if user.chars().count() < 3 {
        return text.into_owned();
    }
    let case = if windows { "(?i)" } else { "" };
    let pattern = format!(r"{case}([\\/]){}", regex::escape(user));
    Regex::new(&pattern).map_or_else(|_| text.to_string(), |user| {
        user.replace_all(&text, |caps: &regex::Captures<'_>| {
            let Some(matched) = caps.get(0) else { return String::new() };
            if path_end(&text[matched.end()..]) { format!("{}<user>", &caps[1]) } else { matched.as_str().to_owned() }
        }).into_owned()
    })
}

#[cfg(test)]
#[path = "../test/redact.rs"]
mod tests;
