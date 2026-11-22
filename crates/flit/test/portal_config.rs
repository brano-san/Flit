use super::*;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

struct Temp(PathBuf);

impl Temp {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!("flit-portal-test-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn toggling_file_chooser_restores_existing_config_exactly() {
    let temp = Temp::new();
    let config = temp.0.join("portals.conf");
    let backup = temp.0.join("backup.json");
    let original = format!("# user config\r\n[preferred]\r\ndefault=kde\r\n{KEY}=gtk");
    fs::write(&config, &original).unwrap();
    assert!(set(&config, &backup, true, || unreachable!()).unwrap());
    assert_eq!(fs::read_to_string(&config).unwrap(), format!("# user config\r\n[preferred]\r\ndefault=kde\r\n{KEY}=flit"));
    assert!(!set(&config, &backup, true, || unreachable!()).unwrap());
    assert!(set(&config, &backup, false, || unreachable!()).unwrap());
    assert_eq!(fs::read_to_string(&config).unwrap(), original);
    assert!(!backup.exists());
}

#[test]
fn disabled_default_and_external_override_are_untouched() {
    let temp = Temp::new();
    let config = temp.0.join("portals.conf");
    let backup = temp.0.join("backup.json");
    fs::write(&config, format!("[preferred]\n{KEY}=gtk\n")).unwrap();
    assert!(!set(&config, &backup, false, || unreachable!()).unwrap());
    assert!(set(&config, &backup, true, || unreachable!()).unwrap());
    let changed = format!("[preferred]\ndefault=wlr\n{KEY}=kde\n");
    fs::write(&config, &changed).unwrap();
    assert!(!set(&config, &backup, false, || unreachable!()).unwrap());
    assert_eq!(fs::read_to_string(&config).unwrap(), changed);
}

#[test]
fn uninstall_restores_the_saved_configuration_and_removes_its_backup() {
    let temp = Temp::new();
    let config = temp.0.join("portals.conf");
    let backup = temp.0.join("portal-file-chooser.json");
    let original = format!("[preferred]\r\ndefault=kde\r\n{KEY} = gtk;kde;");
    fs::write(&config, &original).unwrap();
    assert!(set(&config, &backup, true, || unreachable!()).unwrap());

    restore_on_uninstall(&config, &backup).unwrap();

    assert_eq!(fs::read_to_string(&config).unwrap(), original);
    assert!(!backup.exists());
}

#[test]
fn uninstall_without_a_backup_removes_only_flit_from_file_chooser_preferences() {
    let temp = Temp::new();
    let config = temp.0.join("portals.conf");
    let backup = temp.0.join("portal-file-chooser.json");
    fs::write(&config, format!("[preferred]\ndefault=kde\n{KEY}=flit;gtk;kde;\n[other]\n{KEY}=flit\n")).unwrap();
    let fallback = format!("[preferred]\ndefault=kde\n{KEY}=gtk;kde;\n[other]\n{KEY}=flit\n");

    restore_on_uninstall(&config, &backup).unwrap();
    restore_on_uninstall(&config, &backup).unwrap();

    assert_eq!(fs::read_to_string(&config).unwrap(), fallback);
}

#[test]
fn uninstall_removes_the_configuration_when_flit_created_it() {
    let temp = Temp::new();
    let config = temp.0.join("portals.conf");
    let backup = temp.0.join("portal-file-chooser.json");
    assert!(set(&config, &backup, true, || Ok("[preferred]\ndefault=kde\n".into())).unwrap());

    restore_on_uninstall(&config, &backup).unwrap();

    assert!(!config.exists());
    assert!(!backup.exists());
}

