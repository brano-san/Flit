use super::*;

const DAY_TICKS: i64 = DAY * TICKS_PER_SEC;

fn noon_utc(y: i64, m: u32, d: u32) -> i64 {
    (days_from_civil(y, m, d) * DAY + DAY / 2 + UNIX_EPOCH_SECS) * TICKS_PER_SEC
}

fn clock() -> Clock {
    Clock { now: noon_utc(2024, 3, 5), utc_offset_min: 0 }
}

fn one(text: &str) -> Term {
    let q = parse(text, clock()).unwrap();
    assert_eq!(q.terms.len(), 1, "{text}");
    q.terms.into_iter().next().unwrap()
}

#[test]
fn tokens_quotes_and_modifiers() {
    let q = parse("Foo \"bar baz\" !qux path:src \"ext:rs\" path:\"Program Files\" C:\\Win", clock()).unwrap();
    let shapes: Vec<String> = q
        .terms
        .iter()
        .map(|t| match &t.filter {
            Filter::Match(scope, Pattern::Text(s)) => format!("{}{scope:?}:{}", if t.negate { "!" } else { "" }, String::from_utf8_lossy(s)),
            other => format!("{other:?}"),
        })
        .collect();
    assert_eq!(shapes, ["Name:foo", "Name:bar baz", "!Name:qux", "Path:src", "Name:ext:rs", "Path:program files", "Path:c:\\win"]);
    assert!(parse("   ", clock()).unwrap().terms.is_empty());
    assert!(parse("a\u{0}b", clock()).unwrap().terms.len() == 1);
}

#[test]
fn ext_size_and_regex_filters() {
    assert!(matches!(one("ext:rs;.TXT;").filter, Filter::Ext(e) if e == [b"rs".to_vec(), b"txt".to_vec(), Vec::new()]));
    assert!(matches!(one("size:>1mb").filter, Filter::Size(lo, u64::MAX) if lo == (1 << 20) + 1));
    assert!(matches!(one("size:>=1.5k").filter, Filter::Size(1536, u64::MAX)));
    assert!(matches!(one("size:<10").filter, Filter::Size(0, 10)));
    assert!(matches!(one("size:<=10").filter, Filter::Size(0, 11)));
    assert!(matches!(one("size:1kb..2kb").filter, Filter::Size(1024, 2049)));
    assert!(matches!(one("size:100").filter, Filter::Size(100, 101)));
    assert!(matches!(one("size:2gb..").filter, Filter::Size(lo, u64::MAX) if lo == 2 << 30));
    assert!(parse("size:lots", clock()).is_err());
    assert!(parse("size:1xb", clock()).is_err());
    let Filter::Match(Scope::Name, Pattern::Regex(re)) = one("regex:^IMG_\\d+$").filter else { panic!() };
    assert!(re.is_match(b"img_123") && !re.is_match(b"img_12a"));
    let Filter::Match(Scope::Path, Pattern::Regex(_)) = one("path:regex:src").filter else { panic!() };
    assert!(parse("regex:(", clock()).is_err());
}

#[test]
fn dates_are_local_day_intervals() {
    let day = |y, m, d| (days_from_civil(y, m, d) * DAY + UNIX_EPOCH_SECS) * TICKS_PER_SEC;
    assert!(matches!(one("dm:today").filter, Filter::Modified(lo, hi) if lo == day(2024, 3, 5) && hi == day(2024, 3, 6)));
    assert!(matches!(one("dm:yesterday").filter, Filter::Modified(lo, hi) if lo == day(2024, 3, 4) && hi == day(2024, 3, 5)));
    assert!(matches!(one("dm:2024-02").filter, Filter::Modified(lo, hi) if lo == day(2024, 2, 1) && hi == day(2024, 3, 1)));
    assert!(matches!(one("dm:2023").filter, Filter::Modified(lo, hi) if lo == day(2023, 1, 1) && hi == day(2024, 1, 1)));
    assert!(matches!(one("dm:2023-12").filter, Filter::Modified(_, hi) if hi == day(2024, 1, 1)));
    assert!(matches!(one("dm:>2024/01/31").filter, Filter::Modified(lo, i64::MAX) if lo == day(2024, 2, 1)));
    assert!(matches!(one("dm:2024-01-01..2024-01-31").filter, Filter::Modified(lo, hi) if lo == day(2024, 1, 1) && hi == day(2024, 2, 1)));
    let east = Clock { utc_offset_min: 180, ..clock() };
    let Filter::Modified(lo, _) = parse("dm:today", east).unwrap().terms[0].filter else { panic!() };
    assert_eq!(lo, day(2024, 3, 5) - 3 * 3600 * TICKS_PER_SEC);
    let late = Clock { now: noon_utc(2024, 3, 5) + DAY_TICKS / 2 - TICKS_PER_SEC, utc_offset_min: 60 };
    let Filter::Modified(lo, _) = parse("dm:today", late).unwrap().terms[0].filter else { panic!() };
    assert_eq!(lo, day(2024, 3, 6) - 3600 * TICKS_PER_SEC, "local day already rolled over");
    for bad in ["dm:2024-13", "dm:soon", "dm:24-01-01", "dm:2024-02-40"] {
        assert!(parse(bad, clock()).is_err(), "{bad}");
    }
}

#[test]
fn glob_matches_whole_names() {
    let g = |p: &str, s: &str| glob(p.as_bytes(), s.as_bytes());
    assert!(g("*.txt", "notes.txt") && !g("*.txt", "notes.txt.bak"));
    assert!(g("a?c", "abc") && g("a?c", "aжc") && !g("a?c", "ac"));
    assert!(g("*", "") && g("*", "anything") && !g("", "x") && g("", ""));
    assert!(g("*re*rt*", "my report final") && !g("*re*rt", "report final"));
    assert!(g("f*1*.txt", "file100.txt") && g("**a", "bba"));
    let Filter::Match(Scope::Name, Pattern::Glob(_)) = one("*.TXT").filter else { panic!() };
    let Filter::Match(_, Pattern::Text(_)) = one("\"*.txt\"").filter else { panic!() };
}

#[test]
fn driver_is_the_longest_positive_name_literal() {
    let q = parse("ab !longer_negated path:longest_in_path f*abcd?x ext:rs", clock()).unwrap();
    assert_eq!(q.driver().map(|(k, l)| (k, l.to_vec())), Some((3, b"abcd".to_vec())));
    assert!(parse("ext:rs size:>1", clock()).unwrap().driver().is_none());
    assert!(parse("path:x", clock()).unwrap().needs_path());
}
