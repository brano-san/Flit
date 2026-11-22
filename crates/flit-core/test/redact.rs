use super::*;

#[test]
fn masks_credentials_embedded_in_routes_and_error_text() {
    for scheme in ["smb", "sftp", "dav", "DAVS+SD", "ftp", "ftps"] {
        let address = format!(r#"Unable to open "{scheme}://ann:p%40ss@nas/path?PASSWORD=secret&mode=ro"."#);
        let expected = format!(r#"Unable to open "{scheme}://ann:***@nas/path?PASSWORD=***&mode=ro"."#);
        assert_eq!(passwords(&address), expected);
    }
    assert_eq!(passwords(r#"mount failed: password="abc def" then offline"#), "mount failed: password=*** then offline");
    assert_eq!(passwords(r#"err="password=\"abc def\" then offline""#), r#"err="password=*** then offline""#);
    assert_eq!(passwords("ftp://demo:sec'ret@host/ and password=sec'ret&mode=ro"), "ftp://demo:***@host/ and password=***&mode=ro");
}

#[test]
fn leaves_a_user_without_a_password_and_noncredential_text_intact() {
    let text = "sftp://ann@[fe80::1]:22/ and ftp://host:21/public?not_password=ok";
    assert!(matches!(passwords(text), Cow::Borrowed(s) if s == text));
}

#[test]
fn crash_text_removes_local_identity_and_credentials_from_diagnostics() {
    let raw = r#"at C:\Users\ALICE\src\main.rs and "c:\\users\\Alice\\flit.exe"
user Alice, mail alice+bug@example.org, share \\private-nas\docs
ftp://remote:secret@ftp.example.test password="another secret""#;
    let out = crash_text(raw, r"C:\Users\Alice", "alice", true);
    assert_eq!(
        out,
        r#"at ~\src\main.rs and "~\\flit.exe"
user Alice, mail , share \\<host>\docs
ftp://remote:***@ftp.example.test password=***"#
    );
}

#[test]
fn crash_text_preserves_stack_and_unrelated_names() {
    let raw = "0: flit::app::start at /opt/flit/src/app.rs:60\nalice-helper && malice\n";
    assert_eq!(crash_text(raw, "/", "", false), raw);
    assert_eq!(crash_text("malice failed in /home/alice/src/main.rs:2", "/home/alice", "alice", false), "malice failed in ~/src/main.rs:2");
    assert_eq!(crash_text(r#"//private-a/share UNC:\\private-b\share at "C:\\work\\file.rs" https://public.example/"#, r"C:\Users\Alice", "alice", true), r#"//<host>/share UNC:\\<host>\share at "C:\\work\\file.rs" https://public.example/"#,);
}

#[test]
fn a_common_username_is_hidden_only_as_a_path_component() {
    let text = r"#0 flit.exe: flit::main C:\work\flit\flit.exe C:\Users\flit-local\flit.exe /tmp/flit/flit/file";
    assert_eq!(crash_text(text, r"C:\Users\flit", "flit", true), r"#0 flit.exe: flit::main C:\work\<user>\flit.exe C:\Users\flit-local\flit.exe /tmp/<user>/<user>/file");
    let short = "#0 xy.exe at /tmp/xy/src/xy.rs";
    assert_eq!(crash_text(short, "", "xy", false), short);
}
