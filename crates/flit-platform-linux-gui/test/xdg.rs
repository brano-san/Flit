use super::*;
use crate::temp::Temp;

const SAMPLE: &str = r#"# This file is written by xdg-user-dirs-update
# If you want to change or add directories, just edit the line you're
XDG_DESKTOP_DIR="$HOME/Desktop"
XDG_DOWNLOAD_DIR="$HOME/Загрузки"
XDG_TEMPLATES_DIR="$HOME/"
XDG_PUBLICSHARE_DIR="/srv/public share"
XDG_DOCUMENTS_DIR="$HOME/My \"Docs\""
XDG_MUSIC_DIR=$HOME/Music
XDG_PICTURES_DIR="$HOMEWORK/Pictures"
XDG_VIDEOS_DIR="relative/Videos"
  XDG_VIDEOS_DIR = "$HOME"
"#;

#[test]
fn user_dirs_expand_home_quotes_and_escapes() {
    let dirs = parse_user_dirs(SAMPLE.as_bytes(), b"/home/ann");
    let get = |k: &str| dirs.iter().rev().find(|(key, _)| key == k).map(|(_, v)| String::from_utf8_lossy(v).into_owned());
    assert_eq!(get("XDG_DESKTOP_DIR").as_deref(), Some("/home/ann/Desktop"));
    assert_eq!(get("XDG_DOWNLOAD_DIR").as_deref(), Some("/home/ann/Загрузки"));
    assert_eq!(get("XDG_TEMPLATES_DIR").as_deref(), Some("/home/ann/"));
    assert_eq!(get("XDG_PUBLICSHARE_DIR").as_deref(), Some("/srv/public share"));
    assert_eq!(get("XDG_DOCUMENTS_DIR").as_deref(), Some("/home/ann/My \"Docs\""));
    assert_eq!(get("XDG_MUSIC_DIR"), None);
    assert_eq!(get("XDG_PICTURES_DIR"), None);
    assert_eq!(get("XDG_VIDEOS_DIR").as_deref(), Some("/home/ann"));
}

#[test]
fn templates_list_files_with_stems_and_extensions() {
    let t = Temp::new("templates");
    t.file("Text Document.txt", 3);
    t.file("Makefile", 1);
    t.file(".hidden.txt", 1);
    t.file("Sheet.tar.gz", 1);
    std::fs::create_dir(t.path().join("Folder")).unwrap();
    let got: Vec<(String, String, String)> = templates_in(t.path()).into_iter().map(|n| (n.label, n.ext, n.stem)).collect();
    assert_eq!(
        got,
        [("Makefile".into(), "".into(), "Makefile".into()), ("Sheet.tar".into(), ".gz".into(), "Sheet.tar".into()), ("Text Document".into(), ".txt".into(), "Text Document".into())]
    );
    let first = templates_in(t.path()).into_iter().find(|n| n.ext == ".txt").unwrap();
    assert_eq!(first.template, Template::Copy(fs_path(&t.path().join("Text Document.txt")).unwrap()));
}

fn under_test_root(p: &Path) -> bool {
    p.as_os_str().as_bytes().starts_with(b"/tmp/flit-")
}

#[test]
fn directories_follow_the_test_environment() {
    let Some(home) = home() else { return };
    assert!(under_test_root(&home), "tests must run with HOME under /tmp/flit-*, got {}", home.display());
    let expect = |var: &str, rel: &str| absolute_var(var).unwrap_or_else(|| home.join(rel)).join("flit");
    assert_eq!(state_dir(), Some(expect("XDG_STATE_HOME", ".local/state")));
    assert_eq!(cache_dir(), Some(expect("XDG_CACHE_HOME", ".cache")));
    assert_eq!(config_dir(), Some(expect("XDG_CONFIG_HOME", ".config")));
    for dir in [state_dir(), cache_dir(), config_dir(), data_home(), runtime_dir()].into_iter().flatten() {
        assert!(under_test_root(&dir), "{}", dir.display());
    }
}

#[test]
fn known_folders_come_from_user_dirs_and_must_exist() {
    let (Some(home), Some(config)) = (home(), config_home()) else { return };
    std::fs::create_dir_all(&config).unwrap();
    let docs = home.join("Мои документы");
    std::fs::create_dir_all(&docs).unwrap();
    std::fs::create_dir_all(home.join("Desktop")).unwrap();
    std::fs::write(config.join("user-dirs.dirs"), "XDG_DOCUMENTS_DIR=\"$HOME/Мои документы\"\nXDG_MUSIC_DIR=\"$HOME/Nowhere\"\n").unwrap();
    assert_eq!(known_folder("shell:Personal"), fs_path(&docs).map(Location::from_local));
    assert_eq!(known_folder("shell:Desktop"), fs_path(&home.join("Desktop")).map(Location::from_local));
    assert_eq!(known_folder("shell:My Music"), None);
    assert_eq!(known_folder("shell:Profile"), fs_path(&home).map(Location::from_local));
    assert_eq!(known_folder("shell:NoSuchFolder"), None);
}

#[test]
fn the_bus_activation_file_round_trips_exactly() {
    let dir = Temp::new("activation");
    let target = dir.path().join("data/dbus-1/services/org.freedesktop.FileManager1.service");
    let saved = dir.path().join("config/flit/file-manager1.backup");
    let ours = activation_text(Path::new("/opt/my apps/flit"));
    assert!(ours.contains("Exec=\"/opt/my apps/flit\" --dbus-activated\n"), "{ours}");
    swap_file(true, &target, &saved, &ours).unwrap();
    assert_eq!(std::fs::read_to_string(&target).unwrap(), ours);
    swap_file(true, &target, &saved, &ours).unwrap();
    swap_file(false, &target, &saved, &ours).unwrap();
    assert!(!target.exists() && !saved.exists());
    let nautilus = "[D-BUS Service]\nName=org.freedesktop.FileManager1\nExec=/usr/bin/nautilus --gapplication-service\n";
    std::fs::write(&target, nautilus).unwrap();
    swap_file(true, &target, &saved, &ours).unwrap();
    assert_eq!(std::fs::read_to_string(&target).unwrap(), ours);
    swap_file(false, &target, &saved, &ours).unwrap();
    assert_eq!(std::fs::read_to_string(&target).unwrap(), nautilus);
    assert!(!saved.exists());
    swap_file(false, &target, &saved, &ours).unwrap();
    assert_eq!(std::fs::read_to_string(&target).unwrap(), nautilus);
}

#[test]
fn a_bus_activation_file_replaced_by_someone_else_is_left_alone() {
    let dir = Temp::new("activation-taken");
    let target = dir.path().join("services/x.service");
    let saved = dir.path().join("x.backup");
    swap_file(true, &target, &saved, "ours").unwrap();
    std::fs::write(&target, "theirs").unwrap();
    swap_file(false, &target, &saved, "ours").unwrap();
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "theirs");
    assert!(!saved.exists());
}

#[test]
fn turning_the_default_manager_on_and_off_restores_the_test_home() {
    let config = config_home().unwrap();
    let list = config.join("mimeapps.list");
    let original = "[Default Applications]\nx-directory/normal=nemo.desktop\n";
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(&list, original).unwrap();
    default_manager(true).unwrap();
    let text = std::fs::read_to_string(&list).unwrap();
    assert!(text.contains("inode/directory=flit.desktop") && text.contains("x-directory/normal=flit.desktop"), "{text}");
    default_manager(false).unwrap();
    assert_eq!(std::fs::read_to_string(&list).unwrap(), original);
    let _ = std::fs::remove_file(&list);
}
