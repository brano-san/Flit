pub const POSIX: bool = cfg!(not(windows));

const NAMES: [&str; 4] = ["paths", "path", "name", "dir"];

pub struct Places<'a> {
    pub items: &'a [String],
    pub dir: &'a str,
}

impl Places<'_> {
    fn value(&self, name: &str) -> String {
        match name {
            "path" => self.items.first().map_or(self.dir, String::as_str).to_owned(),
            "paths" if self.items.is_empty() => self.dir.to_owned(),
            "paths" => self.items.join(" "),
            "name" => base_name(self.items.first().map_or(self.dir, String::as_str)).to_owned(),
            _ => self.dir.to_owned(),
        }
    }
}

fn base_name(path: &str) -> &str {
    let trimmed = path.trim_end_matches(['/', '\\']);
    let name = trimmed.rsplit(['/', '\\']).next().unwrap_or(trimmed);
    if name.is_empty() { path } else { name }
}

pub fn split(text: &str, posix: bool) -> Vec<(String, bool)> {
    let (mut out, mut cur, mut quote, mut quoted) = (Vec::new(), String::new(), None, false);
    for c in text.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => cur.push(c),
            (None, '"') => (quote, quoted) = (Some('"'), true),
            (None, '\'') if posix => (quote, quoted) = (Some('\''), true),
            (None, c) if c.is_whitespace() => {
                if quoted || !cur.is_empty() {
                    out.push((std::mem::take(&mut cur), quoted));
                }
                quoted = false;
            }
            (None, c) => cur.push(c),
        }
    }
    if quoted || !cur.is_empty() {
        out.push((cur, quoted));
    }
    out
}

fn fill(token: &str, p: &Places) -> String {
    let mut out = String::with_capacity(token.len());
    let mut rest = token;
    while let Some(at) = rest.find('{') {
        out.push_str(&rest[..at]);
        let inner = &rest[at + 1..];
        match NAMES.iter().find(|n| inner.strip_prefix(**n).is_some_and(|t| t.starts_with('}'))) {
            Some(name) => {
                out.push_str(&p.value(name));
                rest = &inner[name.len() + 1..];
            }
            None => {
                out.push('{');
                rest = inner;
            }
        }
    }
    out.push_str(rest);
    out
}

pub fn expand(template: &str, posix: bool, p: &Places) -> Option<(String, Vec<String>)> {
    let mut argv = Vec::new();
    for (token, quoted) in split(template, posix) {
        match (token.as_str(), quoted) {
            ("{paths}", false) if !p.items.is_empty() => argv.extend(p.items.iter().cloned()),
            _ => argv.push(fill(&token, p)),
        }
    }
    let mut argv = argv.into_iter();
    let program = argv.next().filter(|s| !s.trim().is_empty())?;
    Some((program, argv.collect()))
}

pub fn quote_windows(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '\n', '\u{b}', '"']) {
        return arg.to_owned();
    }
    let mut out = String::with_capacity(arg.len() + 2);
    out.push('"');
    let mut slashes = 0;
    for c in arg.chars() {
        match c {
            '\\' => slashes += 1,
            '"' => {
                out.extend(std::iter::repeat_n('\\', slashes * 2 + 1));
                out.push('"');
                slashes = 0;
            }
            c => {
                out.extend(std::iter::repeat_n('\\', slashes));
                out.push(c);
                slashes = 0;
            }
        }
    }
    out.extend(std::iter::repeat_n('\\', slashes * 2));
    out.push('"');
    out
}

pub fn quote_posix(arg: &str) -> String {
    if !arg.is_empty() && !arg.chars().any(|c| c.is_whitespace() || c == '\'' || c == '"') {
        return arg.to_owned();
    }
    let mut out = String::with_capacity(arg.len() + 2);
    for (k, part) in arg.split('\'').enumerate() {
        if k > 0 {
            out.push_str("\"'\"");
        }
        if !part.is_empty() {
            out.push('\'');
            out.push_str(part);
            out.push('\'');
        }
    }
    if out.is_empty() {
        out.push_str("''");
    }
    out
}

pub fn join(args: &[String], posix: bool) -> String {
    let quote = if posix { quote_posix } else { quote_windows };
    args.iter().map(|a| quote(a)).collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
#[path = "../test/cmdline.rs"]
mod tests;
