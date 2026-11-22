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
fn make_dockerfile_and_cmake_have_their_own_keywords_and_comments() {
    assert_eq!(tokens("mk", "ifeq ($(OS),Windows_NT) # win"), [("ifeq".into(), Tone::Keyword), ("# win".into(), Tone::Comment)]);
    assert_eq!(tokens("mk", ".PHONY: all")[0], (".PHONY".into(), Tone::Keyword));
    assert_eq!(tokens("dockerfile", "FROM rust:1.80 AS build"), [("FROM".into(), Tone::Keyword), ("1.80".into(), Tone::Number), ("AS".into(), Tone::Keyword)]);
    assert_eq!(tokens("dockerfile", "run echo \"hi\" # x"), [("run".into(), Tone::Keyword), ("\"hi\"".into(), Tone::String), ("# x".into(), Tone::Comment)]);
    assert_eq!(tokens("cmake", "add_executable(flit main.cpp) # app"), [("add_executable".into(), Tone::Keyword), ("# app".into(), Tone::Comment)]);
    assert_eq!(tokens("cmake", "#[[ block ]] set(X 1)")[0], ("#[[ block ]]".into(), Tone::Comment));
}

#[test]
fn well_known_file_names_pick_their_syntax() {
    for (name, ext) in [
        ("Makefile", "mk"),
        ("GNUmakefile", "mk"),
        ("Dockerfile", "dockerfile"),
        ("Dockerfile.dev", "dockerfile"),
        ("build.Dockerfile", "dockerfile"),
        ("CMakeLists.txt", "cmake"),
        ("Gemfile", "rb"),
        ("Jenkinsfile", "groovy"),
        ("PKGBUILD", "sh"),
        (".bash_profile", "sh"),
        (".env.local", "env"),
        (".npmrc", "ini"),
    ] {
        assert_eq!(by_file_name(name), by_ext(ext), "{name}");
    }
    for name in ["notes.txt", "Makefile2", "readme", "docker-compose.txt", ".envrc2"] {
        assert_eq!(by_file_name(name), None, "{name}");
    }
}

#[test]
fn a_shebang_names_the_interpreter() {
    for (text, ext) in [
        ("#!/bin/sh\necho", "sh"),
        ("#!/usr/bin/env bash\n", "sh"),
        ("#! /bin/zsh -f\n", "sh"),
        ("#!/usr/bin/env python3\n", "py"),
        ("#!/usr/bin/python3.12 -u\n", "py"),
        ("#!/usr/bin/env -S deno run --allow-read\n", "js"),
        ("#!/usr/bin/env node", "js"),
        ("#!/usr/bin/env ruby\n", "rb"),
        ("#!/usr/bin/env pwsh\n", "ps1"),
        ("\u{FEFF}#!/bin/bash\n", "sh"),
    ] {
        assert_eq!(by_shebang(text.as_bytes()), by_ext(ext), "{text:?}");
    }
    for text in ["echo #!/bin/sh", "#!/usr/bin/perl\n", "#!\n", "plain text", "#!/usr/bin/env\n", " #!/bin/sh"] {
        assert_eq!(by_shebang(text.as_bytes()), None, "{text:?}");
    }
}

#[test]
fn fence_names_cover_the_new_syntaxes() {
    assert_eq!(by_name("makefile"), by_ext("mk"));
    assert_eq!(by_name("docker"), by_ext("dockerfile"));
    assert_eq!(by_name("Dockerfile"), by_ext("dockerfile"));
    assert_eq!(by_name("cmake"), by_ext("cmake"));
}

#[test]
fn non_ascii_is_safe() {
    let line = "let имя = \"строка\"; // коммент";
    let t = tokens("rs", line);
    assert_eq!(t.last().unwrap().1, Tone::Comment);
}
