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
    let journal = Journal::new(&temp.0);
    let config = temp.0.join("portals.conf");
    let original = format!("# user config\r\n[preferred]\r\ndefault=kde\r\n{KEY}=gtk");
    fs::write(&config, &original).unwrap();
    assert_eq!(configure_at(&journal, &config, true, false, || unreachable!()).unwrap(), Some(true));
    assert_eq!(fs::read_to_string(&config).unwrap(), format!("# user config\r\n[preferred]\r\ndefault=kde\r\n{KEY}=flit"));
    assert_eq!(configure_at(&journal, &config, true, true, || unreachable!()).unwrap(), Some(false));
    assert_eq!(configure_at(&journal, &config, false, false, || unreachable!()).unwrap(), Some(true));
    assert_eq!(fs::read_to_string(&config).unwrap(), original);
    assert!(journal.inspect().unwrap().changes.is_empty());
}

#[test]
fn disabled_default_and_external_override_are_untouched() {
    let temp = Temp::new();
    let journal = Journal::new(&temp.0);
    let config = temp.0.join("portals.conf");
    fs::write(&config, format!("[preferred]\n{KEY}=gtk\n")).unwrap();
    assert_eq!(configure_at(&journal, &config, false, false, || unreachable!()).unwrap(), Some(false));
    assert_eq!(configure_at(&journal, &config, true, false, || unreachable!()).unwrap(), Some(true));
    let changed = format!("[preferred]\ndefault=wlr\n{KEY}=kde\n");
    fs::write(&config, &changed).unwrap();
    assert_eq!(configure_at(&journal, &config, false, false, || unreachable!()).unwrap(), Some(false));
    assert_eq!(fs::read_to_string(&config).unwrap(), changed);
}

#[test]
fn undo_all_restores_the_portal_configuration_and_keeps_it_off_at_the_next_start() {
    let temp = Temp::new();
    let journal = Journal::new(&temp.0);
    let config = temp.0.join("portals.conf");
    let original = format!("[preferred]\r\ndefault=kde\r\n{KEY} = gtk;kde;");
    fs::write(&config, &original).unwrap();
    assert_eq!(configure_at(&journal, &config, true, false, || unreachable!()).unwrap(), Some(true));
    assert_eq!(journal.inspect().unwrap().changes[0].group, Group::Portal);
    journal.undo(&Native, None).unwrap();
    strip(&config).unwrap();
    assert_eq!(fs::read_to_string(&config).unwrap(), original);
    assert_eq!(configure_at(&journal, &config, true, true, || unreachable!()).unwrap(), None);
    assert_eq!(fs::read_to_string(&config).unwrap(), original);
}

#[test]
fn a_hand_edited_configuration_loses_only_flit_from_file_chooser_preferences() {
    let temp = Temp::new();
    let config = temp.0.join("portals.conf");
    fs::write(&config, format!("[preferred]\ndefault=kde\n{KEY}=flit;gtk;kde;\n[other]\n{KEY}=flit\n")).unwrap();
    let fallback = format!("[preferred]\ndefault=kde\n{KEY}=gtk;kde;\n[other]\n{KEY}=flit\n");

    strip(&config).unwrap();
    strip(&config).unwrap();

    assert_eq!(fs::read_to_string(&config).unwrap(), fallback);
}

#[test]
fn turning_off_removes_the_configuration_when_flit_created_it() {
    let temp = Temp::new();
    let journal = Journal::new(&temp.0);
    let config = temp.0.join("xdg-desktop-portal").join("portals.conf");
    assert_eq!(configure_at(&journal, &config, true, false, || Ok("[preferred]\ndefault=kde\n".into())).unwrap(), Some(true));
    assert_eq!(fs::read_to_string(&config).unwrap(), format!("[preferred]\n{KEY}=flit\ndefault=kde\n"));

    journal.undo(&Native, Some(Group::Portal)).unwrap();

    assert!(!config.exists());
}
