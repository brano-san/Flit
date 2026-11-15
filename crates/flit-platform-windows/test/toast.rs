use super::*;

#[test]
#[ignore = "displays a real Windows notification; requires an interactive desktop"]
fn native_launch_notification_is_accepted_by_windows() {
    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok().unwrap(); }
    assert!(prepare(), "Flit notification registration failed");
    let notifier = ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(APP_ID)).unwrap();
    let text = flit_core::tr!(flit_core::i18n::S::UserMenuStarted, program = "Flit notification verification");
    let toast = show(0, &text).expect("Windows accepts the native launch notification");
    assert_eq!(notifier.Setting().unwrap(), windows::UI::Notifications::NotificationSetting::Enabled, "Windows has disabled notifications for Flit");
    assert!(toast.Content().unwrap().GetXml().unwrap().to_string_lossy().contains("Flit notification verification"));
    notifier.Hide(&toast).unwrap();
}

#[test]
fn a_notification_shows_the_text_with_markup_characters_kept_literal() {
    let xml = toast_xml("Copying \"a & b\" <2> finished: 3 items");
    assert!(xml.contains("<text>Flit</text><text>Copying &quot;a &amp; b&quot; &lt;2&gt; finished: 3 items</text>"), "{xml}");
    let doc = XmlDocument::new().unwrap();
    doc.LoadXml(&HSTRING::from(xml)).expect("the toast text is well-formed XML");
}

#[test]
fn control_characters_in_a_file_name_do_not_break_the_notification() {
    let doc = XmlDocument::new().unwrap();
    doc.LoadXml(&HSTRING::from(toast_xml("Moving \u{1}odd\u{7}name finished"))).expect("well-formed");
    assert!(toast_xml("line\none").contains("line\none"), "ordinary line breaks stay");
}

#[test]
fn the_app_id_is_registered_per_user_with_a_name_and_an_icon() {
    assert_eq!(registration_key(r"Software\Classes"), r"Software\Classes\AppUserModelId\Flit.FileManager");
    let icon = Path::new(r"C:\Users\me\AppData\Local\Flit\notification.png");
    assert_eq!(registration(icon), [("DisplayName", "Flit".to_owned()), ("IconUri", icon.to_string_lossy().into_owned())]);
}

#[test]
fn the_icon_is_written_once_and_repaired_when_damaged() {
    let dir = crate::test_temp().join(format!("flit-toast-icon-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = keep_icon(&dir).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), ICON);
    let written = std::fs::metadata(&path).unwrap().modified().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    keep_icon(&dir).unwrap();
    assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), written, "an intact icon is left alone");
    std::fs::write(&path, b"broken").unwrap();
    keep_icon(&dir).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), ICON);
    std::fs::remove_dir_all(&dir).unwrap();
}
