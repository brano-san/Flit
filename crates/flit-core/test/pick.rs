use super::*;

fn parse(args: &[&str]) -> Option<Request> {
    Request::from_cli(args.iter().map(|arg| arg.as_bytes().to_vec()).collect())
}

#[test]
fn file_picker_cli_preserves_path_and_accepts_either_pattern() {
    let request = parse(&["--multiple", "--filter", "*.png;*.jpg", "a directory"]).unwrap();
    assert_eq!(request.mode, Mode::Open { multiple: true, directories: false });
    assert_eq!(request.path.as_deref(), Some(b"a directory".as_slice()));
    let filter = &request.filters[0];
    assert!(filter.matches(b"photo.JPG"));
    assert!(filter.matches(b"photo.png"));
    assert!(!filter.matches(b"photo.gif"));
}

#[test]
fn picker_cli_rejects_conflicting_or_incomplete_options() {
    assert!(parse(&["--save"]).is_none());
    assert!(parse(&["--save", "photo.png", "--multiple"]).is_none());
    assert!(parse(&["--filter", ";"]).is_none());
    assert!(parse(&["--unknown"]).is_none());
    assert_eq!(parse(&["--", "--path"]).unwrap().path.unwrap(), b"--path");
}

#[test]
fn save_names_cannot_escape_the_chosen_directory() {
    assert!(valid_name(b".hidden", true));
    assert!(valid_name(b"real file.png", false));
    assert!(!valid_name(b"../escape", true));
    assert!(!valid_name(b"..\\escape", false));
    assert!(!valid_name(b"a.txt:stream", false));
}

#[test]
fn file_picker_accepts_character_classes_in_globs() {
    let filter = &parse(&["--filter", "*.[pP][nN][gG];page[0-9][!a-z].txt;[\u{0430}-\u{0432}].txt"]).unwrap().filters[0];
    assert!(filter.matches(b"Screenshot.PNG"));
    assert!(filter.matches(b"PAGE07.TXT"));
    assert!(filter.matches("\u{0411}.txt".as_bytes()));
    assert!(!filter.matches(b"Screenshot.png.bak"));
    assert!(!filter.matches(b"page0a.txt"));
    assert!(!filter.matches(b"pagea7.txt"));
}

#[test]
fn malformed_picker_classes_are_literal() {
    let filter = &parse(&["--filter", "file[.txt;end[]-].txt;[!]a.txt"]).unwrap().filters[0];
    assert!(filter.matches(b"file[.txt"));
    assert!(filter.matches(b"end].txt"));
    assert!(filter.matches(b"end-.txt"));
    assert!(filter.matches(b"[!]a.txt"));
    assert!(!filter.matches(b"ends.txt"));
}

#[test]
fn mime_rules_keep_unclassified_files_and_match_their_parents_when_known() {
    let filter = Filter { label: "Plain text or PNG".into(), patterns: vec!["*.[pP][nN][gG]".to_lowercase()], mime_types: vec!["text/plain".into()] };
    assert!(filter.matches_with_mime(b"README", Some(&["text/plain".into()])));
    assert!(filter.matches_with_mime(b"example.sh", Some(&["application/x-shellscript".into(), "text/plain".into()])));
    assert!(filter.matches_with_mime(b"Image.PNG", Some(&["image/png".into()])));
    assert!(filter.matches_with_mime(b"unknown", None));
    assert!(!filter.matches_with_mime(b"Image.JPG", Some(&["image/jpeg".into()])));
}
