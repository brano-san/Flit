use super::*;
use std::path::Path;

fn sources() -> String {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut all = String::new();
    let mut stack: Vec<_> = ["flit-core", "flit", "flit-platform-windows", "flit-platform", "flit-platform-linux-gui", "flit-archive"].iter().map(|c| crates.join(c).join("src")).collect();
    while let Some(dir) = stack.pop() {
        for e in std::fs::read_dir(&dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "rs") {
                all.push_str(&std::fs::read_to_string(&p).unwrap());
                all.push('\n');
            }
        }
    }
    all
}

fn used(src: &str, prefix: &str, name: &str) -> bool {
    let pat = format!("{prefix}::{name}");
    src.match_indices(&pat).any(|(at, _)| !src[at + pat.len()..].starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_'))
}

#[test]
fn every_key_has_a_translation_and_no_file_has_extra_keys() {
    assert!(MISSING.is_empty(), "missing translations: {MISSING:?}");
    assert!(EXTRA.is_empty(), "keys that English does not have: {EXTRA:?}");
    assert!(MISMATCHED.is_empty(), "plural and plain strings mixed up: {MISMATCHED:?}");
}

#[test]
fn every_key_is_used_in_the_code() {
    let src = sources();
    let unused: Vec<&str> = S::ALL.iter().filter(|s| !used(&src, "S", &format!("{s:?}"))).map(|s| s.key()).chain(P::ALL.iter().filter(|p| !used(&src, "P", &format!("{p:?}"))).map(|p| p.key())).collect();
    assert!(unused.is_empty(), "unused keys: {unused:?}");
}

#[test]
fn placeholders_match_in_every_language() {
    for s in S::ALL {
        let en = placeholders(s.en());
        for l in Lang::ALL {
            assert_eq!(placeholders(s.in_lang(l)), en, "{} in {l:?}", s.key());
        }
    }
    for p in P::ALL {
        let en = placeholders(p.forms(Lang::En).last().unwrap());
        assert!(en.contains(&"n"), "{} has no {{n}}", p.key());
        for l in Lang::ALL {
            for f in p.forms(l) {
                assert_eq!(placeholders(f), en, "{} in {l:?}: {f}", p.key());
            }
        }
    }
}

#[test]
fn plural_strings_have_the_forms_of_their_language() {
    for p in P::ALL {
        for l in Lang::ALL {
            assert_eq!(p.forms(l).len(), l.plural_forms(), "{} in {l:?}", p.key());
        }
    }
}

#[test]
fn cldr_plural_rules_for_the_other_languages() {
    let ns = [0u64, 1, 2, 5, 11, 21, 22, 25, 1_000_000, 2_000_000, 1_000_001];
    assert_eq!(ns.map(|n| Lang::Uk.form(n)), ns.map(|n| Lang::Ru.form(n)));
    assert_eq!(ns.map(|n| Lang::De.form(n)), ns.map(|n| Lang::En.form(n)));
    assert_eq!(ns.map(|n| Lang::Fr.form(n)), [0, 0, 2, 2, 2, 2, 2, 2, 1, 1, 2]);
    assert_eq!(ns.map(|n| Lang::Es.form(n)), [2, 0, 2, 2, 2, 2, 2, 2, 1, 1, 2]);
    assert!(ns.iter().all(|&n| Lang::ZhCn.form(n) == 0));
    for l in Lang::ALL {
        assert!(ns.iter().all(|&n| l.form(n) < l.plural_forms()), "{l:?}");
    }
}

#[test]
fn no_string_is_empty_or_padded() {
    for l in Lang::ALL {
        for s in S::ALL {
            let t = s.in_lang(l);
            assert!(!t.is_empty() && t.trim() == t, "{} in {l:?}: {t:?}", s.key());
        }
        for p in P::ALL {
            assert!(p.forms(l).iter().all(|f| !f.is_empty() && f.trim() == *f), "{} in {l:?}", p.key());
        }
    }
}

#[test]
fn russian_plural_rules() {
    let forms: Vec<usize> = [0u64, 1, 2, 4, 5, 11, 12, 14, 15, 21, 22, 25, 101, 111, 112, 1001].iter().map(|&n| Lang::Ru.form(n)).collect();
    assert_eq!(forms, vec![2, 0, 1, 1, 2, 2, 2, 2, 2, 0, 1, 2, 0, 2, 2, 0]);
    assert_eq!([0u64, 1, 2, 21].map(|n| Lang::En.form(n)), [1, 0, 1, 1]);
}

#[test]
fn numbers_follow_the_language() {
    set_lang(Lang::En);
    assert_eq!(num(1234567).to_string(), "1,234,567");
    assert_eq!(num(999).to_string(), "999");
    let mut s = String::new();
    decimal(&mut s, 1.25, 1);
    assert_eq!(s, "1.2");
    set_lang(Lang::Ru);
    assert_eq!(num(1234567).to_string(), "1\u{A0}234\u{A0}567");
    let mut s = String::new();
    decimal(&mut s, 12.5, 1);
    assert_eq!(s, "12,5");
    let cldr = [
        (Lang::En, "1,000", "12,345"),
        (Lang::Ru, "1\u{A0}000", "12\u{A0}345"),
        (Lang::Uk, "1\u{A0}000", "12\u{A0}345"),
        (Lang::De, "1.000", "12.345"),
        (Lang::Fr, "1\u{202F}000", "12\u{202F}345"),
        (Lang::Es, "1000", "12.345"),
        (Lang::ZhCn, "1,000", "12,345"),
    ];
    for (l, four, five) in cldr {
        set_lang(l);
        assert_eq!((num(1000).to_string(), num(12345).to_string()), (four.to_owned(), five.to_owned()), "{l:?}");
    }
    set_lang(Lang::Es);
    assert_eq!(num(1234567).to_string(), "1.234.567");
    set_lang(Lang::En);
}

#[test]
fn fill_replaces_named_placeholders_and_keeps_unknown_ones() {
    assert_eq!(fill("{a} and {b}, {a}", &[("a", &1), ("b", &"x")]), "1 and x, 1");
    assert_eq!(fill("{a} {c}", &[("a", &1)]), "1 {c}");
    assert_eq!(fill("no braces", &[]), "no braces");
}

#[test]
fn language_codes() {
    assert_eq!(Lang::from_code("ru-RU"), Some(Lang::Ru));
    assert_eq!(Lang::from_code("EN"), Some(Lang::En));
    assert_eq!(Lang::from_code("de-DE"), Some(Lang::De));
    assert_eq!(Lang::from_code("fr_CA.UTF-8"), Some(Lang::Fr));
    assert_eq!(Lang::from_code("es"), Some(Lang::Es));
    assert_eq!(Lang::from_code("uk_UA.UTF-8@euro"), Some(Lang::Uk));
    assert_eq!(Lang::from_code("zh-CN"), Some(Lang::ZhCn));
    assert_eq!(Lang::from_code("zh_SG"), Some(Lang::ZhCn));
    assert_eq!(Lang::from_code("zh-TW"), None);
    assert_eq!(Lang::from_code("ja-JP"), None);
    assert_eq!(Lang::from_code("C"), None);
    for l in Lang::ALL {
        assert_eq!(Lang::from_code(l.code()), Some(l));
        assert_eq!(Lang::from_u8(l as u8), l);
    }
}

#[test]
fn language_picker_lists_every_language_in_its_own_name() {
    use crate::ui::settings::{language_of, SETTINGS};
    let Some(crate::ui::settings::Kind::Choice(choices, ..)) = SETTINGS.iter().find(|s| s.id == "general.language").map(|s| s.kind) else { panic!() };
    assert_eq!(choices.len(), Lang::ALL.len() + 1);
    for (k, l) in Lang::ALL.into_iter().enumerate() {
        assert_eq!(choices[k + 1].0, l.code());
        assert_eq!(language_of(k + 1), l);
        let name = choices[k + 1].1;
        assert!(Lang::ALL.iter().all(|&o| name.in_lang(o) == name.en()), "{}", name.key());
    }
}

#[test]
fn every_language_names_this_pc_as_its_unix_term_expects() {
    for l in Lang::ALL.into_iter().filter(|&l| l != Lang::En) {
        assert_ne!(unix_term(S::PathComputer.in_lang(l), l), S::PathComputer.in_lang(l), "{l:?}");
    }
}

#[test]
fn english_errors_are_shown_in_the_chosen_language() {
    set_lang(Lang::Ru);
    for s in S::ALL.into_iter().filter(|s| s.key().starts_with("errors")) {
        let names = placeholders(s.en());
        let sample: Vec<(String, String)> = names.iter().map(|n| (n.to_string(), format!("<{n}7>"))).collect();
        let args: Vec<(&str, &dyn Display)> = sample.iter().map(|(n, v)| (n.as_str(), v as &dyn Display)).collect();
        let en = fill(s.en(), &args);
        assert_eq!(localize(&en), fill(s.in_lang(Lang::Ru), &args), "{}", s.key());
    }
    assert_eq!(localize("C:\\some\\path"), "C:\\some\\path");
    set_lang(Lang::En);
    let any = S::ALL.into_iter().find(|s| s.key().starts_with("errors")).unwrap();
    assert_eq!(localize(any.en()), any.en());
}

#[test]
fn folder_errors_follow_the_language() {
    let e = crate::listing::FsError { kind: crate::listing::FsErrorKind::AccessDenied, code: 5 };
    assert_eq!(e.to_string(), "Access denied (0x00000005)");
    set_lang(Lang::Ru);
    assert_eq!(e.to_string(), "Отказано в доступе (0x00000005)");
    assert_eq!(localize("Access denied (0x00000005)"), "Отказано в доступе (0x00000005)");
    assert!(crate::archive::needs_password(S::ErrorsArchiveWrongPassword.t()));
    set_lang(Lang::En);
}

#[test]
fn relocalize_moves_messages_between_languages() {
    set_lang(Lang::En);
    let status = crate::tr!(S::KeymapStatusAssigned, key = "F2", command = S::SidebarDriveLocal.t());
    let plural = crate::trn!(P::DetailsCountFiles, 3);
    let drive = crate::tr_en!(S::SidebarDriveLabel, name = S::SidebarDriveLocal.en(), letter = 'C');
    set_lang(Lang::Ru);
    let ru = relocalize(&status, Lang::En).into_owned();
    assert_eq!(ru, "F2 теперь запускает «Локальный диск»");
    assert_eq!(relocalize(&plural, Lang::En), "3 файла");
    assert_eq!(relocalize(&drive, Lang::En), drive, "a {{name}} that equals a catalogue string is data and stays");
    let local = crate::ui::sidebar::Drive { path: crate::FsPath::parse(r"C:\").unwrap(), label: drive.clone(), total: 0, free: 0, removable: false, unmounted: false, kind: Some(S::SidebarDriveLocal), remote: None };
    assert_eq!(local.title(), "Локальный диск (C:)");
    assert_eq!(crate::ui::sidebar::Drive { kind: None, ..local }.title(), drive);
    let nested = crate::tr_en!(S::UiQuoted, name = S::SidebarDriveLocal.en());
    assert_eq!(relocalize(&nested, Lang::En), crate::tr!(S::UiQuoted, name = S::SidebarDriveLocal.en()));
    assert_eq!(relocalize("Data (D:)", Lang::En), "Data (D:)");
    assert_eq!(relocalize_static(S::ArchiveOk.en(), Lang::En), S::ArchiveOk.in_lang(Lang::Ru));
    assert_eq!(crate::tr!(S::UiQuoted, name = "a"), "«a»");
    assert_eq!(localize(S::ErrorsSearchUnreadableQuery.en()), S::ErrorsSearchUnreadableQuery.in_lang(Lang::Ru));
    set_lang(Lang::En);
    assert_eq!(relocalize(&ru, Lang::Ru), status);
}

#[test]
fn unix_terms_rename_the_trash_and_the_computer() {
    assert_eq!(unix_term(S::PathRecycleBin.en(), Lang::En), "Trash");
    assert_eq!(unix_term("Move {what} to the Recycle Bin", Lang::En), "Move {what} to the Trash");
    assert_eq!(unix_term(S::PathComputer.in_lang(Lang::Ru), Lang::Ru), "Компьютер");
    assert_eq!(unix_term("Файлы нельзя поместить в «Этот компьютер»", Lang::Ru), "Файлы нельзя поместить в «Компьютер»");
    let same = S::MenuCopy.en();
    assert!(std::ptr::eq(unix_term(same, Lang::En), same));
}

#[test]
fn system_choices_name_the_system_not_windows() {
    for l in Lang::ALL {
        for s in [S::SettingsChoiceLanguageSystem, S::SettingsChoiceDateSystem] {
            assert!(!s.in_lang(l).contains("Windows"), "{l:?}");
        }
    }
}
