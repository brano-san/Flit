use super::*;
use crate::system_journal::{System, Target, Value};
use std::cell::RefCell;
use std::collections::HashMap;

#[derive(Default)]
struct Memory(RefCell<HashMap<Target, Value>>);

impl System for Memory {
    fn read(&self, target: &Target) -> io::Result<Option<Value>> {
        Ok(self.0.borrow().get(target).cloned())
    }

    fn write(&self, target: &Target, value: Option<&Value>) -> io::Result<()> {
        if let Some(value) = value {
            self.0.borrow_mut().insert(target.clone(), value.clone());
        } else {
            self.0.borrow_mut().remove(target);
        }
        Ok(())
    }
}

fn string(value: &Value) -> String {
    let wide: Vec<_> = value.bytes.chunks_exact(2).map(|b| u16::from_le_bytes([b[0], b[1]])).take_while(|&w| w != 0).collect();
    String::from_utf16(&wide).unwrap()
}

#[test]
fn autostart_relocates_and_can_be_disabled() {
    let dir = std::env::temp_dir().join(format!("flit-autostart-{}", std::process::id()));
    let journal = Journal::new(&dir);
    let system = Memory::default();
    let first = autostart_registry(r"C:\Мои программы\Flit\flit.exe", false);
    assert_eq!(string(&first.1), r#""C:\Мои программы\Flit\flit.exe""#);
    journal.apply(&system, Group::Autostart, &[first], false).unwrap();
    let (target, relocated) = autostart_registry(r"D:\Утилиты Flit\flit.exe", false);
    journal.apply(&system, Group::Autostart, &[(target.clone(), relocated)], true).unwrap();
    assert_eq!(string(&system.read(&target).unwrap().unwrap()), r#""D:\Утилиты Flit\flit.exe""#);
    journal.undo(&system, Some(Group::Autostart)).unwrap();
    assert_eq!(system.read(&target).unwrap(), None);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn autostart_desktop_quotes_the_program_and_has_no_file_arguments() {
    let text = autostart_desktop("/home/user/Flit версия 100%/flit", false).unwrap();
    assert!(text.lines().any(|l| l == r#"Exec="/home/user/Flit версия 100%%/flit""#));
    assert!(text.lines().any(|l| l == "X-GNOME-Autostart-enabled=true"));
    assert!(!text.contains("%U"));
    assert!(autostart_desktop("/home/user/Flit\nExec=other", false).is_err());
}


#[test]
fn archive_open_with_entries_use_the_supported_formats_without_setting_defaults() {
    let exe = r"C:\Flit portable\flit.exe";
    let entries = archive_entries(exe, |_| Some("Existing.Archiver".into()));
    let command = Target::Registry { key: r"Software\Classes\Flit.Archive\shell\open\command".into(), name: String::new() };
    assert_eq!(string(&entries.iter().find(|(target, _)| *target == command).unwrap().1), r#""C:\Flit portable\flit.exe" "%1""#);
    for ext in ["zip", "7z", "rar", "tar", "tgz", "gz", "lzh", "jar"] {
        assert!(flit_core::archive::browsable(format!("archive.{ext}").as_bytes()).is_some());
        let key = format!(r"Software\Classes\.{ext}\OpenWithProgids");
        assert!(entries.iter().any(|(target, value)| matches!(target, Target::Registry { key: k, name } if *k == key && name == "Flit.Archive") && string(value).is_empty()));
    }
    assert_eq!(entries.iter().filter(|(target, _)| matches!(target, Target::Registry { key, .. } if key.starts_with(r"Software\Classes\."))).count(), 8);
    assert!(entries.iter().all(|(target, _)| !matches!(target, Target::Registry { name, .. } if name == "AllowSilentDefaultTakeOver")));
    assert!(entries.iter().all(|(target, _)| !matches!(target, Target::Registry { key, name } if key.starts_with(r"Software\Classes\.") && (name.is_empty() || !key.ends_with(r"\OpenWithProgids")))));
}

#[test]
fn linux_open_with_desktop_mime_types_match_supported_archive_types() {
    let desktop = include_str!("../../../assets/flit.desktop");
    let supported = [
        ("zip", "application/zip"),
        ("7z", "application/x-7z-compressed"),
        ("rar", "application/vnd.rar"),
        ("tar", "application/x-tar"),
        ("tgz", "application/x-compressed-tar"),
        ("gz", "application/gzip"),
        ("lzh", "application/x-lha"),
        ("jar", "application/java-archive"),
    ];
    let registered = desktop.lines().find_map(|line| line.strip_prefix("MimeType=")).unwrap();
    let mut actual: Vec<_> = registered.split(';').filter(|mime| !mime.is_empty()).collect();
    let mut expected: Vec<_> = std::iter::once("inode/directory").chain(supported.map(|(_, mime)| mime)).collect();
    actual.sort_unstable();
    expected.sort_unstable();
    assert_eq!(actual, expected);
    assert!(supported.iter().all(|(ext, _)| flit_core::archive::browsable(format!("archive.{ext}").as_bytes()).is_some()));
}

#[test]
fn protocol_registration_passes_one_uri_to_the_restricted_handler() {
    let windows = protocol_registry(r"C:\Flit portable\flit.exe");
    assert_eq!(string(&windows[2].1), r#""C:\Flit portable\flit.exe" --flit-link "%1""#);
    let desktop = protocol_desktop("/tmp/Flit portable/flit").unwrap();
    assert!(desktop.lines().any(|line| line == r#"Exec="/tmp/Flit portable/flit" --flit-link %u"#));
    assert!(desktop.lines().any(|line| line == "MimeType=x-scheme-handler/flit;"));
}

#[test]
fn msix_manifest_matches_archive_and_protocol_registration_and_starts_disabled() {
    let manifest = include_str!("../../../packaging/windows/AppxManifest.xml");
    assert!(manifest.contains("Category=\"windows.fileTypeAssociation\""));
    assert_eq!(manifest.matches("<uap:FileType>").count(), ARCHIVE_TYPES.len());
    for ext in ARCHIVE_TYPES {
        assert!(manifest.contains(&format!("<uap:FileType>.{ext}</uap:FileType>")));
    }
    assert!(manifest.contains("Category=\"windows.protocol\""));
    assert!(manifest.contains("Name=\"flit\" Parameters=\"--flit-link &quot;%1&quot;\""));
    assert!(manifest.contains("<desktop:StartupTask TaskId=\"FlitStartup\" Enabled=\"false\""));
}

#[test]
fn archive_registration_skips_extensions_with_no_other_handler() {
    let entries = archive_entries(r"C:\Flit\flit.exe", |extension| match extension {
        ".zip" | ".7z" => Some("Existing.Archiver".into()),
        ".lzh" => Some("fLiT.aRcHiVe".into()),
        ".tgz" => Some(r"Applications\flit.exe".into()),
        _ => None,
    });
    let extensions: Vec<_> = entries.iter().filter_map(|(target, _)| match target {
        Target::Registry { key, name } if name == "Flit.Archive" => key.strip_prefix(r"Software\Classes\").and_then(|key| key.strip_suffix(r"\OpenWithProgids")),
        _ => None,
    }).collect();
    assert_eq!(extensions, [".zip", ".7z"]);
    assert!(archive_entries(r"C:\Flit\flit.exe", |_| None).is_empty());
}

#[test]
fn appimage_registration_uses_one_stable_launch_path_across_mounts() {
    use std::ffi::OsStr;
    use std::path::Path;

    let dir = std::env::temp_dir().join(format!("flit-appimage-system-{}", std::process::id()));
    let journal = Journal::new(&dir);
    let system = Memory::default();
    let image = Some(OsStr::new("/home/user/Flit 100%.AppImage"));
    let target = Target::File(dir.join("flit.desktop"));
    for (group, desktop) in [(Group::Autostart, (|exe: &str| autostart_desktop(exe, false)) as fn(&str) -> io::Result<String>), (Group::Protocol, protocol_desktop)] {
        for (index, mount) in ["/tmp/.mount_FlitAAA", "/tmp/.mount_FlitBBB"].into_iter().enumerate() {
            let exe = format!("{mount}/usr/bin/flit");
            let launch = launch_path(Path::new(&exe), image, Some(OsStr::new(mount)));
            let bytes = desktop(launch.to_str().unwrap()).unwrap().into_bytes();
            journal.apply(&system, group, &[(target.clone(), Value { kind: 0, bytes })], index != 0).unwrap();
        }
        assert_eq!(journal.inspect().unwrap().changes.len(), 1);
        let bytes = system.read(&target).unwrap().unwrap().bytes;
        let expected = if group == Group::Protocol {
            r#"Exec="/home/user/Flit 100%%.AppImage" --flit-link %u"#
        } else {
            r#"Exec="/home/user/Flit 100%%.AppImage""#
        };
        assert!(String::from_utf8(bytes).unwrap().lines().any(|line| line == expected));
        journal.undo(&system, Some(group)).unwrap();
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn msix_documentation_covers_the_handler_registered_at_install_time() {
    let manifest = include_str!("../../../packaging/windows/AppxManifest.xml");
    assert!(manifest.contains("Category=\"windows.protocol\""));
    assert!(manifest.contains("Name=\"flit\" Parameters="));
    let architecture = include_str!("../../../ARCHITECTURE.md");
    assert!(architecture.contains("Windows регистрирует обработчик `flit:` при установке MSIX"));
    assert!(architecture.contains("`--unregister` не снимает регистрацию из манифеста"));
}


#[test]
fn background_mode_starts_flit_at_sign_in_without_a_window_and_undo_restores_the_original() {
    let dir = std::env::temp_dir().join(format!("flit-autostart-background-{}", std::process::id()));
    let journal = Journal::new(&dir);
    let system = Memory::default();
    let (target, plain) = autostart_registry(r"C:\Flit\flit.exe", false);
    let (_, background) = autostart_registry(r"C:\Flit\flit.exe", true);
    assert_eq!(string(&background), r#""C:\Flit\flit.exe" --background"#);
    journal.apply(&system, Group::Autostart, &[(target.clone(), plain)], false).unwrap();
    journal.apply(&system, Group::Autostart, &[(target.clone(), background.clone())], false).unwrap();
    assert_eq!(system.read(&target).unwrap(), Some(background));
    journal.undo(&system, None).unwrap();
    assert_eq!(system.read(&target).unwrap(), None);
    let desktop = autostart_desktop("/opt/flit/flit", true).unwrap();
    assert!(desktop.lines().any(|l| l == r#"Exec="/opt/flit/flit" --background"#));
    std::fs::remove_dir_all(dir).unwrap();
}
