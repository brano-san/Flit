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
    let separator = |c| c == '/' || (!path.starts_with('/') && c == '\\');
    let trimmed = path.trim_end_matches(separator);
    let name = trimmed.rsplit(separator).next().unwrap_or(trimmed);
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

fn percent_values(code: char, p: &Places) -> Option<Vec<String>> {
    let first = p.items.first().map_or(p.dir, String::as_str);
    let parent = |path: &str| path.trim_end_matches(|c| c == '/' || (!path.starts_with('/') && c == '\\')).rsplit_once(|c| c == '/' || (!path.starts_with('/') && c == '\\')).map_or_else(|| p.dir.to_owned(), |(dir, _)| {
        if dir.is_empty() { "/".to_owned() }
        else if dir.ends_with(':') { format!("{dir}{}", &path[dir.len()..dir.len() + 1]) }
        else { dir.to_owned() }
    });
    Some(match code {
        'f' => vec![first.to_owned()],
        'F' => if p.items.is_empty() { vec![p.dir.to_owned()] } else { p.items.to_vec() },
        'n' => vec![base_name(first).to_owned()],
        'N' => if p.items.is_empty() { vec![base_name(p.dir).to_owned()] } else { p.items.iter().map(|s| base_name(s).to_owned()).collect() },
        'd' => vec![if p.items.is_empty() { p.dir.to_owned() } else { parent(first) }],
        'D' => if p.items.is_empty() { vec![p.dir.to_owned()] } else { p.items.iter().map(|s| parent(s)).collect() },
        _ => return None,
    })
}

fn percent_fill(token: &str, p: &Places) -> Option<Vec<String>> {
    let mut out = vec![String::new()];
    let mut chars = token.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '{' {
            let tail: String = chars.clone().collect();
            if let Some(name) = NAMES.iter().find(|name| tail.strip_prefix(**name).is_some_and(|rest| rest.starts_with('}'))) {
                for _ in 0..name.len() + 1 { chars.next(); }
                let value = p.value(name);
                for s in &mut out { s.push_str(&value); }
                continue;
            }
        }
        if ch != '%' { for s in &mut out { s.push(ch); } continue; }
        let Some(code) = chars.next() else { for s in &mut out { s.push('%'); } break; };
        if code == '%' { for s in &mut out { s.push('%'); } continue; }
        match percent_values(code, p) {
            Some(values) => {
                if out.len() > 1 && values.len() > 1 { return None; }
                out = out.iter().flat_map(|prefix| values.iter().map(move |value| format!("{prefix}{value}"))).collect();
            }
            None => for s in &mut out { s.push('%'); s.push(code); },
        }
    }
    Some(out)
}

pub fn expand(template: &str, posix: bool, p: &Places) -> Option<(String, Vec<String>)> {
    let mut argv = Vec::new();
    for (token, quoted) in split(template, posix) {
        match (token.as_str(), quoted) {
            ("{paths}", false) if !p.items.is_empty() => argv.extend(p.items.iter().cloned()),
            _ => argv.extend(percent_fill(&token, p)?),
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
