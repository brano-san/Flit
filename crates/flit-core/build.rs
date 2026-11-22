use std::collections::BTreeMap;
use std::fmt::Write;
use std::path::Path;

const LANGS: [&str; 7] = ["en", "ru", "de", "fr", "es", "uk", "zh-CN"];

#[derive(Clone)]
enum Entry {
    One(String),
    Forms(Vec<String>),
}

fn unescape(s: &str, at: &str) -> Result<String, String> {
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('"') => out.push('"'),
            Some('\\') => out.push('\\'),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('u') => {
                let hex: String = chars.by_ref().take(4).collect();
                let c = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32).ok_or_else(|| format!("{at}: bad \\u escape"))?;
                out.push(c);
            }
            other => return Err(format!("{at}: unknown escape \\{other:?}")),
        }
    }
    Ok(out)
}

fn string_at(s: &str, at: &str) -> Result<(String, usize), String> {
    if !s.starts_with('"') {
        return Err(format!("{at}: expected a string"));
    }
    let bytes = s.as_bytes();
    let mut i = 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'"' => return Ok((unescape(&s[1..i], at)?, i + 1)),
            _ => i += 1,
        }
    }
    Err(format!("{at}: unterminated string"))
}

fn parse(path: &Path, prefix: &str, out: &mut Vec<(String, Entry)>) -> Result<(), String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut section = String::new();
    for (n, line) in text.lines().enumerate() {
        let at = format!("{}:{}", path.display(), n + 1);
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = name.trim().to_owned();
            continue;
        }
        let (key, value) = if line.starts_with('"') {
            let (k, used) = string_at(line, &at)?;
            let rest = line[used..].trim_start().strip_prefix('=').ok_or_else(|| format!("{at}: expected key = value"))?;
            (k, rest)
        } else {
            let (k, v) = line.split_once('=').ok_or_else(|| format!("{at}: expected key = value"))?;
            (k.trim().to_owned(), v)
        };
        let key = key.as_str();
        if key.is_empty() || !key.split('.').all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')) {
            return Err(format!("{at}: bad key {key}"));
        }
        let full = [prefix, section.as_str(), key].iter().filter(|p| !p.is_empty()).copied().collect::<Vec<_>>().join(".");
        let value = value.trim();
        let entry = if let Some(list) = value.strip_prefix('[') {
            let mut rest = list.trim_start();
            let mut forms = Vec::new();
            while !rest.starts_with(']') {
                let (s, used) = string_at(rest, &at)?;
                forms.push(s);
                rest = rest[used..].trim_start();
                rest = rest.strip_prefix(',').unwrap_or(rest).trim_start();
            }
            if !rest[1..].trim().is_empty() {
                return Err(format!("{at}: text after the list"));
            }
            Entry::Forms(forms)
        } else {
            let (s, used) = string_at(value, &at)?;
            if !value[used..].trim().is_empty() {
                return Err(format!("{at}: text after the string"));
            }
            Entry::One(s)
        };
        out.push((full, entry));
    }
    Ok(())
}

fn load(dir: &Path) -> Result<Vec<(String, Entry)>, String> {
    let error = |e| format!("{}: {e}", dir.display());
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(error)? {
        let path = entry.map_err(error)?.path();
        if path.extension().is_some_and(|x| x == "toml") {
            files.push(path);
        }
    }
    files.sort();
    let mut out = Vec::new();
    for f in files {
        println!("cargo:rerun-if-changed={}", f.display());
        let stem = f.file_stem().and_then(|s| s.to_str()).ok_or_else(|| format!("{}: invalid catalog name", f.display()))?;
        parse(&f, stem, &mut out)?;
    }
    Ok(out)
}

fn ident(key: &str) -> String {
    key.split(['.', '_'])
        .map(|p| {
            let mut c = p.chars();
            c.next().map(|f| f.to_ascii_uppercase().to_string() + c.as_str()).unwrap_or_default()
        })
        .collect()
}

fn lit(s: &str) -> String {
    format!("{s:?}")
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("lang");
    println!("cargo:rerun-if-changed={}", root.display());
    let langs: Vec<_> = LANGS
        .iter()
        .map(|l| {
            let dir = root.join(l);
            println!("cargo:rerun-if-changed={}", dir.display());
            load(&dir)
        })
        .collect::<Result<_, _>>()?;
    let source = &langs[0];
    let mut seen = BTreeMap::new();
    for (k, _) in source {
        if let Some(prev) = seen.insert(ident(k), k.clone()) {
            return Err(format!("keys {prev} and {k} map to the same name").into());
        }
    }
    let singles: Vec<&(String, Entry)> = source.iter().filter(|(_, e)| matches!(e, Entry::One(_))).collect();
    let plurals: Vec<&(String, Entry)> = source.iter().filter(|(_, e)| matches!(e, Entry::Forms(_))).collect();
    let mut code = String::new();
    code.push_str("#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, serde::Serialize, serde::Deserialize)]\n#[repr(u16)]\npub enum S {\n");
    for (k, _) in &singles {
        let _ = writeln!(code, "    {},", ident(k));
    }
    code.push_str("}\n\n#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]\n#[repr(u16)]\npub enum P {\n");
    for (k, _) in &plurals {
        let _ = writeln!(code, "    {},", ident(k));
    }
    code.push_str("}\n\n");
    let all = |v: &[&(String, Entry)], e: &str| v.iter().map(|(k, _)| format!("{e}::{}", ident(k))).collect::<Vec<_>>().join(", ");
    let _ = writeln!(code, "impl S {{\n    pub const COUNT: usize = {};\n    pub const ALL: [S; {}] = [{}];\n}}\n", singles.len(), singles.len(), all(&singles, "S"));
    let _ = writeln!(code, "impl P {{\n    pub const COUNT: usize = {};\n    pub const ALL: [P; {}] = [{}];\n}}\n", plurals.len(), plurals.len(), all(&plurals, "P"));
    let _ = writeln!(code, "pub static S_KEYS: [&str; {}] = [{}];", singles.len(), singles.iter().map(|(k, _)| lit(k)).collect::<Vec<_>>().join(", "));
    let _ = writeln!(code, "pub static P_KEYS: [&str; {}] = [{}];", plurals.len(), plurals.iter().map(|(k, _)| lit(k)).collect::<Vec<_>>().join(", "));
    let mut s_tables = Vec::new();
    let mut p_tables = Vec::new();
    let mut missing = Vec::new();
    let mut extra = Vec::new();
    let mut mismatched = Vec::new();
    for (li, lang) in langs.iter().enumerate() {
        let map: BTreeMap<&str, &Entry> = lang.iter().map(|(k, e)| (k.as_str(), e)).collect();
        for (k, _) in lang {
            if !source.iter().any(|(s, _)| s == k) {
                extra.push(format!("{}:{k}", LANGS[li]));
            }
        }
        let mut row = Vec::new();
        for (k, e) in &singles {
            let Entry::One(en) = e else { unreachable!() };
            row.push(match map.get(k.as_str()) {
                Some(Entry::One(t)) => lit(t),
                Some(Entry::Forms(_)) => {
                    mismatched.push(format!("{}:{k}", LANGS[li]));
                    lit(en)
                }
                None => {
                    missing.push(format!("{}:{k}", LANGS[li]));
                    lit(en)
                }
            });
        }
        s_tables.push(format!("[{}]", row.join(", ")));
        let mut row = Vec::new();
        for (k, e) in &plurals {
            let Entry::Forms(en) = e else { unreachable!() };
            let forms = match map.get(k.as_str()) {
                Some(Entry::Forms(t)) => t.clone(),
                Some(Entry::One(_)) => {
                    mismatched.push(format!("{}:{k}", LANGS[li]));
                    en.clone()
                }
                None => {
                    missing.push(format!("{}:{k}", LANGS[li]));
                    en.clone()
                }
            };
            row.push(format!("&[{}]", forms.iter().map(|f| lit(f)).collect::<Vec<_>>().join(", ")));
        }
        p_tables.push(format!("[{}]", row.join(", ")));
    }
    let _ = writeln!(code, "static S_TABLE: [[&str; {}]; {}] = [{}];", singles.len(), LANGS.len(), s_tables.join(", "));
    let _ = writeln!(code, "static P_TABLE: [[&[&str]; {}]; {}] = [{}];", plurals.len(), LANGS.len(), p_tables.join(", "));
    let list = |v: &[String]| v.iter().map(|s| lit(s)).collect::<Vec<_>>().join(", ");
    let _ = writeln!(code, "pub static MISSING: &[&str] = &[{}];", list(&missing));
    let _ = writeln!(code, "pub static EXTRA: &[&str] = &[{}];", list(&extra));
    let _ = writeln!(code, "pub static MISMATCHED: &[&str] = &[{}];", list(&mismatched));
    let out = Path::new(&std::env::var("OUT_DIR")?).join("strings.rs");
    std::fs::write(out, code)?;
    Ok(())
}
