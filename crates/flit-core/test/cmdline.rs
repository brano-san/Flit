use super::*;

#[test]
fn an_argument_takes_at_most_one_list_placeholder() {
    let two = owned(&["/tmp/a", "/tmp/b"]);
    assert!(expand("tool %F%N", true, &places(&two, "/tmp")).is_none());
    assert!(expand("tool %F=%D", true, &places(&two, "/tmp")).is_none());
    assert_eq!(expand("tool %F%F", true, &places(&two[..1], "/tmp")).unwrap().1, owned(&["/tmp/a/tmp/a"]));
    assert_eq!(expand("tool %F %N", true, &places(&two, "/tmp")).unwrap().1, owned(&["/tmp/a", "/tmp/b", "a", "b"]));
    assert_eq!(expand("tool --in=%f%n", true, &places(&two, "/tmp")).unwrap().1, owned(&["--in=/tmp/aa"]));
}

#[test]
fn folder_placeholders_keep_roots_and_shares_whole() {
    assert_eq!(expand("t %d", true, &places(&owned(&["/file"]), "/")).unwrap().1, owned(&["/"]));
    assert_eq!(expand("t %d", false, &places(&owned(&[r"C:\file"]), r"C:\")).unwrap().1, owned(&[r"C:\"]));
    assert_eq!(expand("t %d", false, &places(&owned(&[r"\\server\share\dir\file.txt"]), r"C:\")).unwrap().1, owned(&[r"\\server\share\dir"]));
    assert_eq!(expand("t %D", false, &places(&owned(&[r"D:\a\b\", r"D:\c"]), r"C:\")).unwrap().1, owned(&[r"D:\a", r"D:\"]));
}

#[test]
fn doubled_percent_signs_stay_literal_next_to_placeholder_letters() {
    assert_eq!(expand("cmd /c echo %%DATE%% %%f", false, &places(&owned(&[r"C:\x.txt"]), r"C:\")).unwrap().1, owned(&["/c", "echo", "%DATE%", "%f"]));
}

#[test]
fn percent_placeholders_preserve_literal_unix_backslashes() {
    let files = vec![r"/home/a\b.txt".to_owned()];
    assert_eq!(expand("app %n %d", true, &Places { items: &files, dir: "/home" }).unwrap().1, [r"a\b.txt", "/home"]);
}

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
fn percent_placeholders_keep_each_selected_item_as_a_literal_argument() {
    let items = owned(&["/tmp/a b;$(echo bad).txt", "/other/{dir}%n.txt"]);
    let (_, args) = expand("tool %f %F %n %N %d %D %% --name=%N", true, &places(&items, "/tmp")).unwrap();
    assert_eq!(args, owned(&["/tmp/a b;$(echo bad).txt", "/tmp/a b;$(echo bad).txt", "/other/{dir}%n.txt", "a b;$(echo bad).txt", "a b;$(echo bad).txt", "{dir}%n.txt", "/tmp", "/tmp", "/other", "%", "--name=a b;$(echo bad).txt", "--name={dir}%n.txt"]));
    assert_eq!(expand("tool %d %F %n", true, &places(&[], "/tmp")).unwrap().1, owned(&["/tmp", "/tmp", "tmp"]));
    assert_eq!(expand("tool %d", false, &places(&owned(&[r"C:\a.txt"]), r"C:\")).unwrap().1, owned(&[r"C:\"]));
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
