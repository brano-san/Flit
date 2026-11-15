use super::*;

#[test]
fn missing_update_downloader_names_its_package_and_present_downloader_can_finish() {
    let mut data = Vec::new();
    crate::temp::with_tools(&[], || {
        let error = download("https://invalid.example/unused", &mut data, &mut |_, _| true).unwrap_err();
        assert_eq!(error.message, crate::shell::tool_missing("curl").message);
        crate::temp::assert_missing(&error.message, "curl");
        assert!(data.is_empty());
    });
    crate::temp::with_tools(&["curl"], || {
        download("https://invalid.example/unused", &mut data, &mut |_, _| true).unwrap();
    });
    assert!(data.is_empty());
}
