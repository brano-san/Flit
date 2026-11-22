use std::collections::HashMap;

fn locales(lang: &str) -> Vec<String> {
    let lang = lang.replace('-', "_");
    let (base, modifier) = lang.split_once('@').map_or((lang.as_str(), None), |(b, m)| (b, Some(m)));
    let base = base.split('.').next().unwrap_or_default();
    let short = base.split('_').next().unwrap_or_default();
    if short.is_empty() || matches!(short, "C" | "POSIX") {
        return Vec::new();
    }
    let mut out = Vec::new();
    if let Some(modifier) = modifier {
        out.push(format!("{base}@{modifier}"));
    }
    out.push(base.to_owned());
    if base != short {
        if let Some(modifier) = modifier {
            out.push(format!("{short}@{modifier}"));
        }
        out.push(short.to_owned());
    }
    out
}

fn unescape(text: &str) -> Option<String> {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        out.push(if c == '\\' {
            match chars.next()? {
                's' => ' ',
                'n' => '\n',
                't' => '\t',
                'r' => '\r',
                '\\' => '\\',
                _ => return None,
            }
        } else {
            c
        });
    }
    (!out.trim().is_empty()).then_some(out)
}

pub(crate) fn name(text: &str, lang: &str, system: &str) -> Option<String> {
    let mut fields = HashMap::new();
    let mut inside = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            if inside {
                break;
            }
            inside = line == "[Desktop Entry]";
        } else if inside && !line.starts_with('#') {
            if let Some((key, value)) = line.split_once('=') {
                fields.insert(key.trim(), value.trim());
            }
        }
    }
    if !matches!(fields.get("Type"), Some(&"Application" | &"Link")) {
        return None;
    }
    let base = fields.get("Name").and_then(|v| unescape(v))?;
    locales(lang).into_iter().chain(locales(system)).find_map(|l| fields.get(format!("Name[{l}]").as_str()).and_then(|v| unescape(v))).or(Some(base))
}
