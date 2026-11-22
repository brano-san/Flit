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
