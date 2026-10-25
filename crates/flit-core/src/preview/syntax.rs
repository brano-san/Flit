use super::Tone;

pub struct Syntax {
    pub name: &'static str,
    exts: &'static [&'static str],
    line: &'static [&'static str],
    block: &'static [(&'static str, &'static str)],
    quotes: &'static [u8],
    char_quote: bool,
    ident_extra: &'static [u8],
    keywords: &'static [&'static str],
    nocase: bool,
    markup: bool,
    diff: bool,
}

const fn syn(name: &'static str, exts: &'static [&'static str], line: &'static [&'static str], block: &'static [(&'static str, &'static str)], quotes: &'static [u8], keywords: &'static [&'static str]) -> Syntax {
    Syntax { name, exts, line, block, quotes, char_quote: false, ident_extra: &[], keywords, nocase: false, markup: false, diff: false }
}

const C_BLOCK: &[(&str, &str)] = &[("/*", "*/")];
const SLASH: &[&str] = &["//"];
const HASH: &[&str] = &["#"];
const DQ: &[u8] = b"\"";
const DQ_SQ: &[u8] = b"\"'";
const DQ_SQ_BT: &[u8] = b"\"'`";

const RUST: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern", "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub",
    "ref", "return", "self", "Self", "static", "struct", "super", "trait", "true", "type", "unsafe", "use", "where", "while",
];
const C: &[&str] = &[
    "#define", "#elif", "#else", "#endif", "#if", "#ifdef", "#ifndef", "#include", "#pragma", "#undef", "auto", "bool", "break", "case", "catch", "char", "class", "const", "constexpr", "continue",
    "default", "delete", "do", "double", "else", "enum", "explicit", "extern", "false", "float", "for", "friend", "goto", "if", "inline", "int", "long", "namespace", "new", "noexcept", "nullptr",
    "operator", "override", "private", "protected", "public", "register", "return", "short", "signed", "sizeof", "static", "struct", "switch", "template", "this", "throw", "true", "try", "typedef",
    "typename", "union", "unsigned", "using", "virtual", "void", "volatile", "while", "cbuffer", "float2", "float3", "float4", "uint", "half", "in", "out", "inout", "uniform",
];
const CSHARP: &[&str] = &[
    "abstract", "as", "async", "await", "base", "bool", "break", "byte", "case", "catch", "char", "class", "const", "continue", "decimal", "default", "delegate", "do", "double", "else", "enum", "event",
    "false", "finally", "float", "for", "foreach", "get", "if", "in", "int", "interface", "internal", "is", "let", "lock", "long", "namespace", "new", "null", "object", "out", "override", "params",
    "private", "protected", "public", "readonly", "record", "ref", "return", "sealed", "set", "static", "string", "struct", "switch", "this", "throw", "true", "try", "typeof", "using", "var", "virtual",
    "void", "while", "yield",
];
const JAVA: &[&str] = &[
    "abstract", "boolean", "break", "byte", "case", "catch", "char", "class", "companion", "const", "continue", "data", "default", "do", "double", "else", "enum", "extends", "false", "final", "finally",
    "float", "for", "fun", "if", "implements", "import", "in", "instanceof", "int", "interface", "is", "long", "new", "null", "object", "override", "package", "private", "protected", "public", "return",
    "short", "static", "super", "switch", "synchronized", "this", "throw", "throws", "true", "try", "val", "var", "void", "when", "while",
];
const GO: &[&str] = &[
    "break", "case", "chan", "const", "continue", "default", "defer", "else", "fallthrough", "false", "for", "func", "go", "goto", "if", "import", "interface", "map", "nil", "package", "range",
    "return", "select", "struct", "switch", "true", "type", "var",
];
const JS: &[&str] = &[
    "async", "await", "break", "case", "catch", "class", "const", "continue", "default", "delete", "do", "else", "enum", "export", "extends", "false", "finally", "for", "from", "function", "if",
    "implements", "import", "in", "instanceof", "interface", "let", "new", "null", "of", "private", "public", "return", "static", "super", "switch", "this", "throw", "true", "try", "type", "typeof",
    "undefined", "var", "void", "while", "yield",
];
const PYTHON: &[&str] = &[
    "False", "None", "True", "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del", "elif", "else", "except", "finally", "for", "from", "global", "if", "import", "in", "is",
    "lambda", "nonlocal", "not", "or", "pass", "raise", "return", "self", "try", "while", "with", "yield",
];
const RUBY: &[&str] = &[
    "begin", "break", "case", "class", "def", "do", "else", "elsif", "end", "ensure", "false", "for", "if", "in", "module", "next", "nil", "not", "require", "rescue", "return", "self", "then", "true",
    "unless", "until", "when", "while", "yield",
];
const PHP: &[&str] = &[
    "array", "as", "break", "case", "class", "const", "continue", "default", "echo", "else", "elseif", "extends", "false", "foreach", "for", "function", "if", "include", "namespace", "new", "null",
    "private", "protected", "public", "require", "return", "static", "switch", "this", "true", "use", "while",
];
const SHELL: &[&str] = &[
    "case", "do", "done", "echo", "elif", "else", "esac", "exit", "export", "fi", "for", "function", "if", "in", "local", "read", "return", "set", "shift", "then", "unset", "until", "while",
];
const POWERSHELL: &[&str] = &[
    "begin", "break", "catch", "class", "continue", "do", "else", "elseif", "end", "exit", "filter", "finally", "for", "foreach", "function", "if", "in", "param", "process", "return", "switch", "throw",
    "trap", "try", "until", "while", "$true", "$false", "$null",
];
const BATCH: &[&str] = &["call", "cd", "copy", "del", "do", "echo", "else", "endlocal", "exist", "exit", "for", "goto", "if", "in", "not", "pause", "rem", "set", "setlocal", "shift", "start"];
const SQL: &[&str] = &[
    "add", "all", "alter", "and", "as", "asc", "begin", "between", "by", "case", "create", "delete", "desc", "distinct", "drop", "else", "end", "exists", "from", "group", "having", "in", "index",
    "inner", "insert", "into", "is", "join", "key", "left", "like", "limit", "not", "null", "on", "or", "order", "outer", "primary", "right", "select", "set", "table", "then", "union", "update",
    "values", "view", "when", "where", "with",
];
const LUA: &[&str] = &["and", "break", "do", "else", "elseif", "end", "false", "for", "function", "goto", "if", "in", "local", "nil", "not", "or", "repeat", "return", "then", "true", "until", "while"];
const LITERALS: &[&str] = &["true", "false", "null", "yes", "no", "on", "off"];
const CSS: &[&str] = &["!important", "@media", "@import", "@font-face", "@keyframes", "@supports"];

pub static SYNTAXES: &[Syntax] = &[
    Syntax { char_quote: true, ..syn("Rust", &["rs"], SLASH, C_BLOCK, DQ_SQ, RUST) },
    Syntax { char_quote: true, ident_extra: b"#", ..syn("C/C++", &["c", "h", "cpp", "cc", "cxx", "hpp", "hh", "hxx", "inl", "ino", "hlsl", "glsl", "fx", "vert", "frag", "m", "mm"], SLASH, C_BLOCK, DQ_SQ, C) },
    Syntax { char_quote: true, ..syn("C#", &["cs", "csx"], SLASH, C_BLOCK, DQ_SQ, CSHARP) },
    Syntax { char_quote: true, ..syn("Java", &["java", "kt", "kts", "scala", "groovy", "gradle", "dart", "swift"], SLASH, C_BLOCK, DQ_SQ, JAVA) },
    Syntax { char_quote: true, ..syn("Go", &["go"], SLASH, C_BLOCK, b"\"`'", GO) },
    syn("JavaScript", &["js", "mjs", "cjs", "jsx", "ts", "mts", "cts", "tsx", "vue", "svelte"], SLASH, C_BLOCK, DQ_SQ_BT, JS),
    syn("Python", &["py", "pyw", "pyi", "gyp"], HASH, &[("\"\"\"", "\"\"\""), ("'''", "'''")], DQ_SQ, PYTHON),
    syn("Ruby", &["rb", "rake", "gemspec"], HASH, &[("=begin", "=end")], DQ_SQ, RUBY),
    Syntax { ident_extra: b"$", ..syn("PHP", &["php"], &["//", "#"], C_BLOCK, DQ_SQ, PHP) },
    Syntax { ident_extra: b"$", ..syn("Shell", &["sh", "bash", "zsh", "ksh", "fish", "bashrc", "zshrc", "profile"], HASH, &[], DQ_SQ, SHELL) },
    Syntax { ident_extra: b"$-", nocase: true, ..syn("PowerShell", &["ps1", "psm1", "psd1"], HASH, &[("<#", "#>")], DQ_SQ, POWERSHELL) },
    Syntax { nocase: true, ..syn("Batch", &["bat", "cmd"], &["::"], &[], DQ, BATCH) },
    Syntax { nocase: true, ..syn("SQL", &["sql"], &["--"], C_BLOCK, DQ_SQ, SQL) },
    syn("Lua", &["lua"], &["--"], &[("--[[", "]]")], DQ_SQ, LUA),
    syn("JSON", &["json", "jsonc", "json5", "geojson", "ipynb", "webmanifest"], SLASH, C_BLOCK, DQ, LITERALS),
    syn("TOML", &["toml", "lock"], HASH, &[], DQ_SQ, LITERALS),
    syn("YAML", &["yml", "yaml"], HASH, &[], DQ_SQ, LITERALS),
    syn("INI", &["ini", "cfg", "conf", "inf", "reg", "url", "desktop", "properties", "gitconfig", "editorconfig", "env"], &[";", "#"], &[], DQ, LITERALS),
    Syntax { markup: true, ..syn("XML", &["xml", "html", "htm", "xhtml", "svg", "xaml", "csproj", "vcxproj", "props", "targets", "plist", "resx", "config", "manifest", "nuspec", "xsd", "xsl"], &[], &[("<!--", "-->")], DQ_SQ, &[]) },
    syn("CSS", &["css", "scss", "less"], &[], C_BLOCK, DQ_SQ, CSS),
    Syntax { diff: true, ..syn("Diff", &["diff", "patch"], &[], &[], &[], &[]) },
];

pub fn by_ext(ext: &str) -> Option<u8> {
    let ext = ext.to_ascii_lowercase();
    SYNTAXES.iter().position(|s| s.exts.contains(&ext.as_str())).map(|i| i as u8)
}

pub fn by_name(name: &str) -> Option<u8> {
    let n = name.to_ascii_lowercase();
    let alias = match n.as_str() {
        "rust" => "rs",
        "python" => "py",
        "javascript" => "js",
        "typescript" => "ts",
        "csharp" | "c#" => "cs",
        "c++" => "cpp",
        "bash" | "shell" | "console" => "sh",
        "powershell" | "pwsh" => "ps1",
        "golang" => "go",
        "kotlin" => "kt",
        "ruby" => "rb",
        "html" => "html",
        other => other,
    };
    by_ext(alias)
}

fn ident(c: u8, extra: &[u8]) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c >= 0x80 || extra.contains(&c)
}

fn is_keyword(s: &Syntax, word: &str) -> bool {
    if s.nocase {
        s.keywords.iter().any(|k| k.eq_ignore_ascii_case(word))
    } else {
        s.keywords.contains(&word)
    }
}

fn push(out: &mut Vec<(u32, u32, Tone)>, a: usize, b: usize, tone: Tone) {
    if a >= b {
        return;
    }
    match out.last_mut() {
        Some(last) if last.2 == tone && last.1 == a as u32 => last.1 = b as u32,
        _ => out.push((a as u32, b as u32, tone)),
    }
}

fn starts(b: &[u8], i: usize, p: &str, nocase: bool) -> bool {
    b.get(i..i + p.len()).is_some_and(|w| if nocase { w.eq_ignore_ascii_case(p.as_bytes()) } else { w == p.as_bytes() })
}

fn char_end(b: &[u8], i: usize) -> usize {
    match b.get(i + 1) {
        Some(b'\\') => (i + 3..b.len().min(i + 12)).find(|&j| b[j] == b'\'').map_or(i, |j| j + 1),
        Some(&c) => {
            let n = match c {
                0xF0.. => 4,
                0xE0.. => 3,
                0xC0.. => 2,
                _ => 1,
            };
            if b.get(i + 1 + n) == Some(&b'\'') { i + 2 + n } else { i }
        }
        None => i,
    }
}

fn string_end(b: &[u8], i: usize, q: u8) -> usize {
    let mut j = i + 1;
    while j < b.len() {
        match b[j] {
            b'\\' if q != b'`' || j + 1 < b.len() => j += 2,
            c if c == q => return j + 1,
            _ => j += 1,
        }
    }
    b.len()
}

pub fn highlight(s: &Syntax, line: &str, state: &mut Option<u8>, out: &mut Vec<(u32, u32, Tone)>) {
    let b = line.as_bytes();
    if s.diff {
        let tone = match b.first() {
            Some(b'+') => Tone::Added,
            Some(b'-') => Tone::Removed,
            Some(b'@') => Tone::Keyword,
            _ => Tone::Plain,
        };
        return push(out, 0, b.len(), tone);
    }
    let mut i = 0;
    let mut tag = false;
    while i < b.len() {
        if let Some(k) = *state {
            let end = s.block[k as usize].1;
            let stop = (i..b.len()).find(|&j| starts(b, j, end, false)).map_or(b.len(), |j| {
                *state = None;
                j + end.len()
            });
            push(out, i, stop, Tone::Comment);
            i = stop;
            continue;
        }
        if let Some(k) = s.block.iter().position(|(open, _)| starts(b, i, open, false)) {
            *state = Some(k as u8);
            push(out, i, i + s.block[k].0.len(), Tone::Comment);
            i += s.block[k].0.len();
            continue;
        }
        if s.line.iter().any(|p| starts(b, i, p, s.nocase)) {
            return push(out, i, b.len(), Tone::Comment);
        }
        let c = b[i];
        if s.markup && c == b'<' {
            let mut j = i + 1;
            while j < b.len() && matches!(b[j], b'/' | b'?' | b'!') {
                j += 1;
            }
            let start = j;
            while j < b.len() && (ident(b[j], b"-:.")) {
                j += 1;
            }
            push(out, i, start, Tone::Plain);
            push(out, start, j, Tone::Keyword);
            tag = true;
            i = j;
            continue;
        }
        if s.markup && c == b'>' {
            tag = false;
        }
        if s.quotes.contains(&c) && (!s.markup || tag) {
            let end = if c == b'\'' && s.char_quote { char_end(b, i) } else { string_end(b, i, c) };
            if end > i {
                push(out, i, end, Tone::String);
                i = end;
                continue;
            }
        }
        let prev_ident = i > 0 && ident(b[i - 1], s.ident_extra);
        if c.is_ascii_digit() && !prev_ident && !s.markup {
            let mut j = i + 1;
            while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'.' || b[j] == b'_') {
                j += 1;
            }
            push(out, i, j, Tone::Number);
            i = j;
            continue;
        }
        if ident(c, s.ident_extra) && !c.is_ascii_digit() {
            let mut j = i + 1;
            while j < b.len() && ident(b[j], s.ident_extra) {
                j += 1;
            }
            let word = &line[i..j];
            push(out, i, j, if !s.markup && is_keyword(s, word) { Tone::Keyword } else { Tone::Plain });
            i = j;
            continue;
        }
        let n = line[i..].chars().next().map_or(1, char::len_utf8);
        push(out, i, i + n, Tone::Plain);
        i += n;
    }
}

#[cfg(test)]
#[path = "../../test/preview/syntax.rs"]
mod tests;
