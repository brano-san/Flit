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
fn the_desktop_entry_runs_this_executable_and_handles_folders() {
    let text = desktop_text(Path::new("/opt/my apps/100%/a\"b$c\\d"));
    assert!(text.contains("\nExec=\"/opt/my apps/100%%/a\\\\\"b\\\\$c\\\\\\\\d\" %U\n"), "{text}");
    assert!(text.contains("\nMimeType=inode/directory;application/zip;") && text.starts_with("[Desktop Entry]\n"), "{text}");
    assert_eq!(text.lines().filter(|l| l.starts_with("Exec=")).count(), 2);
    assert!(text.lines().all(|l| !l.starts_with("Exec=flit")), "{text}");
}

#[test]
fn the_desktop_entry_announces_startup_and_offers_a_new_window() {
    let text = desktop_text(Path::new("/usr/bin/flit"));
    let main = text.split("\n[Desktop Action").next().unwrap();
    assert!(main.contains("\nStartupNotify=true\n") && main.contains("\nActions=new-window;\n"), "{text}");
    assert!(main.contains("\nExec=\"/usr/bin/flit\" %U\n"), "the main entry still takes files: {text}");
    let action = text.split("[Desktop Action new-window]\n").nth(1).unwrap();
    assert!(action.contains("\nExec=\"/usr/bin/flit\" --new-window\n") && action.starts_with("Name=New window\n"), "{text}");
    assert!(action.contains("\nName[ru]=Новое окно\n"), "{text}");
}

#[test]
fn the_desktop_entry_lands_in_the_test_data_home_and_goes_away() {
    let entry = data_home().unwrap().join("applications/flit.desktop");
    desktop_entry(true).unwrap();
    let text = std::fs::read_to_string(&entry).unwrap();
    assert!(under_test_root(&entry) && text.contains(&format!("Exec=\"{}\" %U", std::env::current_exe().unwrap().display())), "{text}");
    desktop_entry(false).unwrap();
    assert!(!entry.exists());
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

#[test]
fn new_offers_files_documents_and_a_script_unless_templates_already_do() {
    let summary = |v: Vec<NewTemplate>| v.into_iter().map(|n| (n.label, n.ext, n.stem, n.template)).collect::<Vec<_>>();
    let own = |label: &str, ext: &str, stem: &str, template: Template| (label.to_owned(), ext.to_owned(), stem.to_owned(), template);
    let builtins = [
        own("Empty file", "", "New file", Template::Empty),
        own("Text document", ".txt", "New text document", Template::Empty),
        own("Markdown document", ".md", "New Markdown document", Template::Empty),
        own("Shell script", ".sh", "New shell script", Template::Data(b"#!/bin/sh\n\n".to_vec())),
    ];
    assert_eq!(summary(with_builtins(Vec::new())), builtins);
    let t = Temp::new("builtin-templates");
    t.file("Makefile", 4);
    let report = t.file("Report.odt", 9);
    let mut expected = builtins.to_vec();
    expected.extend([own("Makefile", "", "Makefile", Template::Copy(fs_path(&t.path().join("Makefile")).unwrap())), own("Report", ".odt", "Report", Template::Copy(fs_path(&report).unwrap()))]);
    assert_eq!(summary(with_builtins(templates_in(t.path()))), expected);
    let labels = |dir: &Temp| with_builtins(templates_in(dir.path())).into_iter().map(|n| n.label).collect::<Vec<String>>();
    let u = Temp::new("covering-templates");
    u.file("Empty Document", 0);
    u.file("TEXT DOCUMENT.md", 2);
    assert_eq!(labels(&u), ["Markdown document", "Shell script", "Empty Document", "TEXT DOCUMENT"]);
    let v = Temp::new("empty-templates");
    v.file("Notes.md", 0);
    v.file("empty.sh", 0);
    assert_eq!(labels(&v), ["Empty file", "Text document", "Shell script", "empty", "Notes"]);
    let w = Temp::new("script-template");
    w.file("Shell Script.sh", 5);
    assert_eq!(labels(&w), ["Empty file", "Text document", "Markdown document", "Shell Script"]);
}

#[test]
fn user_folders_tell_their_type_and_other_folders_do_not() {
    use flit_core::desktop_ini::FolderKind;
    let dirs = |key: &str| match key {
        "XDG_PICTURES_DIR" => Some(PathBuf::from("/home/ann/Bilder")),
        "XDG_MUSIC_DIR" => Some(PathBuf::from("/home/ann/Musik")),
        "XDG_DOCUMENTS_DIR" => Some(PathBuf::from("/home/ann/Dokumente")),
        "XDG_VIDEOS_DIR" => Some(PathBuf::from("/home/ann/Videos")),
        _ => None,
    };
    assert_eq!(folder_kind_with(Path::new("/home/ann/Bilder"), dirs), Some(FolderKind::Pictures));
    assert_eq!(folder_kind_with(Path::new("/home/ann/Musik"), dirs), Some(FolderKind::Music));
    assert_eq!(folder_kind_with(Path::new("/home/ann/Dokumente"), dirs), Some(FolderKind::Documents));
    assert_eq!(folder_kind_with(Path::new("/home/ann/Videos"), dirs), Some(FolderKind::Videos));
    assert_eq!(folder_kind_with(Path::new("/home/ann/Bilder/2024"), dirs), None, "subfolders keep their own type");
    assert_eq!(folder_kind_with(Path::new("/home/ann/Downloads"), dirs), None);
}

#[test]
fn built_in_templates_carry_their_string_keys_and_files_keep_their_own_names() {
    let t = Temp::new("named-templates");
    t.file("Report.odt", 9);
    let all = with_builtins(templates_in(t.path()));
    let keys: Vec<Option<TemplateNames>> = all.iter().map(|n| n.names).collect();
    assert_eq!(keys[1], Some(TemplateNames { label: S::MenuTextDocument, stem: S::MenuNewTextDocument }), "the text document is renamed when the language changes");
    assert_eq!(keys.iter().filter(|k| k.is_some()).count(), 4);
    assert_eq!((all[4].label(), all[4].stem(), all[4].names), ("Report", "Report", None), "a file in Templates keeps its file name");
}
