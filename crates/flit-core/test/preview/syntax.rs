use super::*;

fn tokens(ext: &str, line: &str) -> Vec<(String, Tone)> {
    let s = &SYNTAXES[by_ext(ext).unwrap() as usize];
    let mut out = Vec::new();
    highlight(s, line, &mut None, &mut out);
    out.into_iter().map(|(a, b, t)| (line[a as usize..b as usize].to_owned(), t)).filter(|(_, t)| *t != Tone::Plain).collect()
}

#[test]
fn rust_keywords_strings_numbers_comments() {
    let t = tokens("rs", r#"let s = "a\"b"; // note 42"#);
    assert_eq!(t, [("let".into(), Tone::Keyword), ("\"a\\\"b\"".into(), Tone::String), ("// note 42".into(), Tone::Comment)]);
    assert_eq!(tokens("rs", "fn f<'a>(x: &'a u8) -> u8 { 0x1F }"), [("fn".into(), Tone::Keyword), ("0x1F".into(), Tone::Number)]);
    assert_eq!(tokens("rs", "let c = 'x';"), [("let".into(), Tone::Keyword), ("'x'".into(), Tone::String)]);
}

#[test]
fn block_comments_carry_state() {
    let s = &SYNTAXES[by_ext("c").unwrap() as usize];
    let (mut state, mut out) = (None, Vec::new());
    highlight(s, "int a; /* start", &mut state, &mut out);
    assert!(state.is_some());
    out.clear();
    highlight(s, "still */ return 1;", &mut state, &mut out);
    assert_eq!(state, None);
    assert_eq!(out[0], (0, 8, Tone::Comment));
    assert!(out.iter().any(|&(a, b, t)| &"still */ return 1;"[a as usize..b as usize] == "return" && t == Tone::Keyword));
}

#[test]
fn language_specifics() {
    assert_eq!(tokens("c", "#include <x.h>")[0], ("#include".into(), Tone::Keyword));
    assert_eq!(tokens("sql", "SELECT 1 -- x"), [("SELECT".into(), Tone::Keyword), ("1".into(), Tone::Number), ("-- x".into(), Tone::Comment)]);
    assert_eq!(tokens("lua", "--[[ a ]] x")[0], ("--[[ a ]]".into(), Tone::Comment));
    assert_eq!(tokens("py", "def f(): # c")[0], ("def".into(), Tone::Keyword));
    assert_eq!(tokens("xml", r#"<a href="x">t</a>"#), [("a".into(), Tone::Keyword), ("\"x\"".into(), Tone::String), ("a".into(), Tone::Keyword)]);
    assert_eq!(tokens("diff", "+added"), [("+added".into(), Tone::Added)]);
    assert_eq!(tokens("ps1", "$true -eq $x"), [("$true".into(), Tone::Keyword)]);
    assert_eq!(tokens("json", r#"{"k": true}"#), [("\"k\"".into(), Tone::String), ("true".into(), Tone::Keyword)]);
}

#[test]
fn fence_names_map_to_syntaxes() {
    assert_eq!(by_name("rust"), by_ext("rs"));
    assert_eq!(by_name("TypeScript"), by_ext("ts"));
    assert_eq!(by_name("nope"), None);
}

#[test]
fn non_ascii_is_safe() {
    let line = "let имя = \"строка\"; // коммент";
    let t = tokens("rs", line);
    assert_eq!(t.last().unwrap().1, Tone::Comment);
}
