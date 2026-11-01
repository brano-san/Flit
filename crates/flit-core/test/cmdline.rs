use super::*;

fn places<'a>(items: &'a [String], dir: &'a str) -> Places<'a> {
    Places { items, dir }
}

fn owned(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_owned()).collect()
}

#[test]
fn templates_split_on_spaces_and_quotes_group_words() {
    assert_eq!(split(r#""C:\Program Files\x.exe" -a  {paths}"#, false), [(r"C:\Program Files\x.exe".to_owned(), true), ("-a".into(), false), ("{paths}".into(), false)]);
    assert_eq!(split("echo it's", false), [("echo".to_owned(), false), ("it's".into(), false)], "single quotes are plain text on Windows");
    assert_eq!(split("sh -c 'ls -l' \"\"", true), [("sh".to_owned(), false), ("-c".into(), false), ("ls -l".into(), true), ("".into(), true)]);
}

#[test]
fn placeholders_take_the_selection_the_folder_and_the_first_name() {
    let items = owned(&[r"D:\in\a b.txt", r"D:\in\c.txt"]);
    let p = places(&items, r"D:\in");
    let (program, args) = expand(r#"code --goto {path} {paths} "{paths}" --in={dir} {name} {other}"#, false, &p).unwrap();
    assert_eq!(program, "code");
    assert_eq!(args, owned(&["--goto", r"D:\in\a b.txt", r"D:\in\a b.txt", r"D:\in\c.txt", r"D:\in\a b.txt D:\in\c.txt", r"--in=D:\in", "a b.txt", "{other}"]));
    let none = places(&[], "/home/u/src");
    assert_eq!(expand("tool {paths} {name}", true, &none).unwrap().1, owned(&["/home/u/src", "src"]), "the background stands for the folder itself");
    assert_eq!(expand("  ", true, &none), None);
    assert_eq!(expand("{path}", true, &none).unwrap().0, "/home/u/src", "the program may come from a placeholder");
}

#[test]
fn placeholder_text_inside_a_path_is_never_expanded_again() {
    let items = owned(&["/tmp/{dir} $(rm -rf ~); `x`"]);
    let (_, args) = expand("echo {path}", true, &places(&items, "/tmp")).unwrap();
    assert_eq!(args, items, "a file name is one argument, whatever it contains");
}

#[test]
fn windows_quoting_follows_command_line_to_argv_rules() {
    assert_eq!(quote_windows(r"C:\plain\path"), r"C:\plain\path");
    assert_eq!(quote_windows(""), r#""""#);
    assert_eq!(quote_windows(r"C:\My Folder\"), r#""C:\My Folder\\""#);
    assert_eq!(quote_windows(r#"say "hi""#), r#""say \"hi\"""#);
    assert_eq!(quote_windows(r#"a\"b"#), r#""a\\\"b""#);
    assert_eq!(quote_windows(r"a\\b c"), r#""a\\b c""#, "backslashes before other characters stay as they are");
    assert_eq!(join(&owned(&["-x", r"D:\a b\", ""]), false), r#"-x "D:\a b\\" """#);
}

#[test]
fn posix_quoting_survives_the_split_args_rules() {
    assert_eq!(quote_posix("/home/u/a.txt"), "/home/u/a.txt");
    assert_eq!(quote_posix(""), "''");
    assert_eq!(quote_posix("a b"), "'a b'");
    assert_eq!(quote_posix("it's"), r#"'it'"'"'s'"#);
    assert_eq!(quote_posix(r#"say "hi""#), r#"'say "hi"'"#);
    assert_eq!(quote_posix("'"), r#""'""#);
    for arg in ["a b", "it's", r#"x"'y"#, "", "''", "tab\there", "\u{3000}wide space", "$(rm -rf ~)"] {
        let line = join(&owned(&[arg, "next"]), true);
        assert_eq!(split(&line, true).into_iter().map(|t| t.0).collect::<Vec<_>>(), owned(&[arg, "next"]), "{line}");
    }
}
