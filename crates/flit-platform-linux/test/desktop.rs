use super::*;
use crate::temp::Temp;
use std::os::unix::fs::symlink;

#[test]
fn desktop_names_follow_locale_country_modifier_and_encoding_fallbacks() {
    let entries = "[Desktop Entry]\nType=Application\nName=Default\nName[sr_RS@latin]=Full\nName[sr_RS]=Country\nName[sr@latin]=Modifier\nName[sr]=Language\nName[ru]=Файрфокс\nName[zh_CN]=浏览器\n";
    assert_eq!(name(entries, "ru", "sr_RS@latin"), Some("Файрфокс".into()));
    assert_eq!(name(entries, "zh-CN", "ru"), Some("浏览器".into()));
    for (removed, expected) in [(Vec::new(), "Full"), (vec!["Name[sr_RS@latin]=Full\n"], "Country"), (vec!["Name[sr_RS@latin]=Full\n", "Name[sr_RS]=Country\n"], "Modifier"), (vec!["Name[sr_RS@latin]=Full\n", "Name[sr_RS]=Country\n", "Name[sr@latin]=Modifier\n"], "Language")] {
        let text = removed.into_iter().fold(entries.to_owned(), |text, entry| text.replace(entry, ""));
        assert_eq!(name(&text, "sr_RS.UTF-8@latin", "ru"), Some(expected.into()));
    }
    assert_eq!(name(entries, "de", "sr_RS.UTF-8@latin"), Some("Full".into()));
    assert_eq!(name(entries, "de", "C.UTF-8"), Some("Default".into()));
    assert_eq!(name(entries, "sr", "ru"), Some("Language".into()));
    assert_eq!(name(entries, "sr_ME", "ru"), Some("Language".into()));
    assert_eq!(name(entries, "sr@other", "ru"), Some("Language".into()));
}

#[test]
fn desktop_names_only_use_the_main_group_and_valid_display_values() {
    let text = "[Desktop Entry]\nType=Link\nName=File\\sManager\\\\Tools\nURL=file:///tmp\n[Desktop Action Other]\nName=Wrong\nName[ru]=Wrong\n";
    assert_eq!(name(text, "ru", ""), Some("File Manager\\Tools".into()));
    for text in ["", "[Other]\nType=Application\nName=Wrong\n", "[Desktop Entry]\nName=Missing type\n", "[Desktop Entry]\nType=Directory\nName=Directory\n", "[Desktop Entry]\nType=Application\nName=\n", "[Desktop Entry]\nType=Application\nName=Invalid\\q\n", "[Desktop Entry]\nType=Application\nName[ru]=Missing default\n"] {
        assert_eq!(name(text, "ru", ""), None, "{text}");
    }
}

#[test]
fn shared_launcher_trust_keeps_installed_real_paths_and_owner_execute_rules() {
    let t = Temp::new("desktop-trust");
    let data = t.path().join("data");
    let installed = data.join("applications/browser.desktop");
    std::fs::create_dir_all(installed.parent().unwrap()).unwrap();
    std::fs::write(&installed, "[Desktop Entry]\nType=Application\nName=Installed\n").unwrap();
    std::fs::set_permissions(&installed, std::fs::Permissions::from_mode(0o600)).unwrap();
    let link = t.path().join("browser.desktop");
    symlink(&installed, &link).unwrap();
    assert!(trusted_launcher(&installed, std::slice::from_ref(&data)));
    assert!(trusted_launcher(&link, std::slice::from_ref(&data)));
    assert_eq!(label(&link, std::slice::from_ref(&data), "en", ""), Some("Installed".into()));
    let copied = t.path().join("copy.desktop");
    std::fs::copy(&installed, &copied).unwrap();
    assert!(!trusted_launcher(&copied, std::slice::from_ref(&data)));
    assert_eq!(label(&copied, std::slice::from_ref(&data), "en", ""), None);
    std::fs::set_permissions(&copied, std::fs::Permissions::from_mode(0o610)).unwrap();
    assert!(!trusted_launcher(&copied, std::slice::from_ref(&data)));
    std::fs::set_permissions(&copied, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(trusted_launcher(&copied, std::slice::from_ref(&data)));
    let executable_link = t.path().join("executable.desktop");
    symlink(&copied, &executable_link).unwrap();
    assert_eq!(label(&executable_link, &[], "en", ""), Some("Installed".into()));
    std::fs::set_permissions(&copied, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(label(&executable_link, &[], "en", ""), None);
    assert!(!trusted(Path::new("/data/applications-other/x.desktop"), Path::new("/tmp/x.desktop"), 0o600, &[PathBuf::from("/data")]));
}
#[test]
fn desktop_label_reads_accept_the_limit_and_reject_larger_or_non_utf8_files() {
    let t = Temp::new("desktop-limit");
    let file = t.path().join("limit.desktop");
    let mut text = b"[Desktop Entry]\nType=Application\nName=At limit\n".to_vec();
    text.resize(MAX_BYTES as usize, b' ');
    std::fs::write(&file, &text).unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(label(&file, &[], "en", ""), Some("At limit".into()));
    text.push(b' ');
    std::fs::write(&file, &text).unwrap();
    assert_eq!(label(&file, &[], "en", ""), None);
    std::fs::write(&file, b"[Desktop Entry]\nType=Application\nName=\xff\n").unwrap();
    assert_eq!(label(&file, &[], "en", ""), None);
}
