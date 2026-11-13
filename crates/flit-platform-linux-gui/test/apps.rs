use super::*;
use crate::temp::Temp;
use std::time::{Duration, SystemTime};
use flit_core::provider::Launch;

const EDITOR: &str = "[Desktop Entry]\nType=Application\nName=Editor\nName[ru]=Редактор\nExec=editor --new %F\nMimeType=text/plain;text/markdown;\n\n[Desktop Action New]\nName=Ignored\nExec=ignored\n";
const VIEWER: &str = "[Desktop Entry]\nType=Application\nName=Viewer\nExec=\"/opt/my viewer/view\" %u --title %c\nMimeType=text/plain;image/png;\nTerminal=true\n";
const GONE: &str = "[Desktop Entry]\nType=Application\nName=Gone\nExec=gone\nHidden=true\nMimeType=text/plain;\n";

fn entry(id: &str, text: &str) -> Entry {
    parse_desktop(id, Path::new(&format!("/usr/share/applications/{id}")), text, "ru_RU.UTF-8").unwrap()
}

#[test]
fn desktop_entries_take_the_localized_name_and_only_the_main_group() {
    let e = entry("editor.desktop", EDITOR);
    assert_eq!((e.name.as_str(), e.exec.as_str()), ("Редактор", "editor --new %F"));
    assert_eq!(e.mimes, ["text/plain", "text/markdown"]);
    assert!(entry("viewer.desktop", VIEWER).terminal);
    assert!(parse_desktop("l.desktop", Path::new("/l"), "[Desktop Entry]\nType=Link\nName=L\nURL=x\n", "").is_none());
}

#[test]
fn exec_field_codes_expand_to_files_and_uris() {
    let files = [PathBuf::from("/tmp/a b.txt"), PathBuf::from("/tmp/c.txt")];
    let d = Path::new("/usr/share/applications/v.desktop");
    assert_eq!(command("editor --new %F", &files, "Editor", d), ["editor", "--new", "/tmp/a b.txt", "/tmp/c.txt"]);
    assert_eq!(command("\"/opt/my viewer/view\" %u --title %c", &files, "Viewer", d), ["/opt/my viewer/view", "file:///tmp/a%20b.txt", "--title", "Viewer"]);
    assert_eq!(command("plain", &files[..1], "P", d), ["plain", "/tmp/a b.txt"]);
    assert_eq!(command("app %i %k 100%%", &[], "A", d), ["app", "/usr/share/applications/v.desktop", "100%"]);
}

#[test]
fn defaults_come_first_and_removed_or_hidden_apps_are_dropped() {
    let entries = vec![entry("editor.desktop", EDITOR), entry("viewer.desktop", VIEWER), entry("gone.desktop", GONE)];
    let mut assoc = Assoc::default();
    parse_mimeapps("[Default Applications]\ntext/plain=viewer.desktop;missing.desktop;\n[Added Associations]\nimage/png=editor.desktop;\n[Removed Associations]\ntext/markdown=editor.desktop;\n", &mut assoc);
    let ids = |mime: &str| handlers(mime, &entries, &assoc).iter().map(|e| e.id.clone()).collect::<Vec<_>>();
    assert_eq!(ids("text/plain"), ["viewer.desktop", "editor.desktop"]);
    assert_eq!(ids("image/png"), ["editor.desktop", "viewer.desktop"]);
    assert!(ids("text/markdown").is_empty());
    assert!(ids("video/mp4").is_empty());
}

#[test]
fn the_open_with_menu_lists_apps_for_every_selected_file() {
    let t = Temp::new("apps");
    let (a, b) = (t.file("a.txt", 1), t.file("b.png", 1));
    let entries = vec![entry("editor.desktop", EDITOR), entry("viewer.desktop", VIEWER)];
    let fp = |p: &Path| crate::xdg::fs_path(p).unwrap();
    if crate::mime::mime_of(b"png").is_none() {
        return;
    }
    let (items, launches) = open_with(&[fp(&a), fp(&b)], &entries, &Assoc::default());
    assert_eq!(items.len(), 3, "the app, a separator and Other application");
    assert_eq!(launches[0], Launch::With { program: fp(Path::new("/usr/share/applications/viewer.desktop")), args: vec![fp(&a), fp(&b)] });
    assert!(matches!(&launches[1], Launch::ChooseApp { apps, files, .. } if apps.len() == 2 && files.len() == 2));
    let (single, _) = open_with(&[fp(&a)], &entries, &Assoc::default());
    assert_eq!(single.len(), 4);
    assert!(matches!(&single[0], MenuItem::Command { keys, .. } if !keys.is_empty()), "the default app is marked");
    assert!(matches!(&single[1], MenuItem::Command { keys, .. } if keys.is_empty()));
    assert!(open_with(&[fp(t.path())], &entries, &Assoc::default()).0.is_empty(), "folders get no Open with");
}

#[test]
fn files_without_handlers_get_no_list_of_every_application() {
    let t = Temp::new("apps-none");
    let blob = t.path().join("blob.flit-unknown-type");
    std::fs::write(&blob, b"\0\x01\x02binary").unwrap();
    let readme = t.path().join("README");
    std::fs::write(&readme, "plain words\n").unwrap();
    let entries = vec![entry("editor.desktop", EDITOR), entry("viewer.desktop", VIEWER)];
    let fp = |p: &Path| crate::xdg::fs_path(p).unwrap();
    let (items, launches) = open_with(&[fp(&blob)], &entries, &Assoc::default());
    assert_eq!(items.len(), 1, "only Other application");
    assert!(matches!(&launches[..], [Launch::ChooseApp { .. }]));
    assert_eq!(open_with(&[fp(&readme)], &entries, &Assoc::default()).0.len(), 4, "text without an extension opens in text editors");
}

#[test]
fn handlers_of_parent_types_follow_the_own_handlers() {
    let entries = vec![entry("editor.desktop", EDITOR), entry("viewer.desktop", VIEWER)];
    let parents = |m: &str| if m == "text/markdown" { vec!["text/plain".to_owned()] } else { Vec::new() };
    let ids: Vec<_> = handlers_by_lineage("text/markdown", &entries, &Assoc::default(), parents).iter().map(|e| e.id.clone()).collect();
    assert_eq!(ids, ["editor.desktop", "viewer.desktop"]);
}

fn backdate(paths: &[PathBuf]) {
    let old = SystemTime::now() - Duration::from_secs(60);
    for p in paths {
        std::fs::File::open(p).unwrap().set_modified(old).unwrap();
    }
}

fn applications(t: &Temp, count: usize) -> (PathBuf, PathBuf) {
    let (data, config) = (t.path().join("data"), t.path().join("config"));
    let apps = data.join("applications");
    std::fs::create_dir_all(apps.join("kde")).unwrap();
    std::fs::create_dir_all(&config).unwrap();
    let langs = ["fr", "es", "it", "pt_BR", "pl", "cs", "ru", "uk", "ja", "ko", "zh_CN", "zh_TW", "nl", "sv", "fi", "hu", "tr", "el", "he", "ar", "ca", "vi", "id", "sr@latin"];
    let translated: String = langs.iter().map(|l| format!("Name[{l}]=App {l}\nGenericName[{l}]=Tool {l}\nComment[{l}]=Does something useful in {l}\nKeywords[{l}]=one;two;three;\n")).collect();
    for i in 0..count {
        let mime = if i % 3 == 0 { "text/plain;image/png;" } else { "application/pdf;" };
        std::fs::write(apps.join(format!("app{i}.desktop")), format!("[Desktop Entry]\nType=Application\nName=App {i}\nName[de]=Programm {i}\nComment=Something {i}\n{translated}Icon=app{i}\nExec=app{i} %F\nMimeType={mime}\nCategories=Utility;\n")).unwrap();
    }
    std::fs::write(apps.join("kde/viewer.desktop"), VIEWER).unwrap();
    std::fs::write(config.join("mimeapps.list"), "[Default Applications]\ntext/plain=kde-viewer.desktop;\n").unwrap();
    backdate(&[apps.clone(), apps.join("kde"), config.join("mimeapps.list")]);
    (data, config)
}

#[test]
fn the_application_catalog_is_kept_until_an_application_folder_or_mimeapps_changes() {
    let t = Temp::new("apps-catalog");
    let (data, config) = applications(&t, 3);
    let c = Catalog::load(std::slice::from_ref(&data), std::slice::from_ref(&config));
    assert_eq!(c.entries.len(), 4);
    assert_eq!(handlers("text/plain", &c.entries, &c.assoc)[0].id, "kde-viewer.desktop");
    assert!(c.fresh());
    std::fs::write(data.join("applications/new.desktop"), EDITOR).unwrap();
    assert!(!c.fresh(), "a new desktop file is seen");
    let c = Catalog::load(std::slice::from_ref(&data), std::slice::from_ref(&config));
    assert_eq!(c.entries.len(), 5);
    assert!(!c.fresh(), "a folder changed within the last seconds is read again");
    backdate(&[data.join("applications")]);
    let c = Catalog::load(std::slice::from_ref(&data), std::slice::from_ref(&config));
    std::fs::write(config.join("mimeapps.list"), "[Default Applications]\ntext/plain=new.desktop;\n").unwrap();
    assert!(!c.fresh(), "mimeapps.list edits are seen");
}

#[test]
#[ignore]
fn bench_the_open_with_menu_for_six_hundred_applications() {
    let t = Temp::new("apps-bench");
    let (data, config) = applications(&t, 600);
    let file = t.file("note.txt", 1);
    let files = [crate::xdg::fs_path(&file).unwrap()];
    let runs = 50;
    let started = std::time::Instant::now();
    for _ in 0..runs {
        let c = Catalog::load(std::slice::from_ref(&data), std::slice::from_ref(&config));
        assert!(!open_with(&files, &c.entries, &c.assoc).0.is_empty());
    }
    let parsed = started.elapsed() / runs;
    let c = Catalog::load(std::slice::from_ref(&data), std::slice::from_ref(&config));
    let started = std::time::Instant::now();
    for _ in 0..runs {
        assert!(c.fresh());
        assert!(!open_with(&files, &c.entries, &c.assoc).0.is_empty());
    }
    let cached = started.elapsed() / runs;
    eprintln!("open with for 600 applications: parsing every time {parsed:?}, cached catalog {cached:?}");
}

#[test]
fn launching_a_desktop_file_runs_its_exec_line() {
    let t = Temp::new("apps-launch");
    let out = t.path().join("ran");
    let desktop = t.path().join("touch.desktop");
    std::fs::write(&desktop, format!("[Desktop Entry]\nType=Application\nName=T\nExec=sh -c 'echo \"$0\" > {}' %f\n", out.display())).unwrap();
    launch(&desktop, &[PathBuf::from("/tmp/given file")], Some("tok")).unwrap();
    let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !out.exists() && std::time::Instant::now() < until {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    std::thread::sleep(std::time::Duration::from_millis(50));
    assert_eq!(std::fs::read_to_string(&out).unwrap().trim(), "/tmp/given file");
}

#[test]
fn desktop_keys_hide_entries_that_cannot_run_or_belong_to_other_desktops() {
    let t = Temp::new("apps-keys");
    let (data, config) = (t.path().join("data"), t.path().join("config"));
    let apps = data.join("applications");
    std::fs::create_dir_all(&apps).unwrap();
    std::fs::create_dir_all(&config).unwrap();
    let app = |name: &str, extra: &str| std::fs::write(apps.join(format!("{name}.desktop")), format!("[Desktop Entry]\nType=Application\nName={name}\nExec={name} %f\nMimeType=text/plain;\nIcon=accessories-text-editor\nPath=/tmp\n{extra}")).unwrap();
    app("plain", "");
    app("missing", "TryExec=/nonexistent/flit-no-such-binary\n");
    app("present", "TryExec=sh\n");
    app("gnomeonly", "OnlyShowIn=GNOME;\n");
    app("notkde", "NotShowIn=KDE;\n");
    app("hiddenmenu", "NoDisplay=true\n");
    app("chosen", "NoDisplay=true\n");
    std::fs::write(config.join("mimeapps.list"), "[Added Associations]\ntext/plain=chosen.desktop;missing.desktop;\n").unwrap();
    let ids = |desktops: &[&str]| {
        let c = Catalog::load_for(std::slice::from_ref(&data), std::slice::from_ref(&config), &desktops.iter().map(|d| d.to_string()).collect::<Vec<_>>());
        let mut ids: Vec<String> = handlers("text/plain", &c.entries, &c.assoc).iter().map(|e| e.id.trim_end_matches(".desktop").to_owned()).collect();
        ids.sort();
        ids
    };
    assert_eq!(ids(&["KDE"]), ["chosen", "plain", "present"]);
    assert_eq!(ids(&["ubuntu", "GNOME"]), ["chosen", "gnomeonly", "notkde", "plain", "present"]);
    let e = entry("x.desktop", "[Desktop Entry]\nType=Application\nName=X\nExec=x\nPath=/srv/w\nIcon=x-icon\nNoDisplay=true\nOnlyShowIn=XFCE;LXQt;\n");
    assert_eq!((e.dir.as_str(), e.icon.as_str(), e.no_display), ("/srv/w", "x-icon", true));
    assert!(shown_in(&e, &["LXQt".into()]) && !shown_in(&e, &["GNOME".into()]) && !shown_in(&e, &[]));
}

#[test]
fn single_file_field_codes_start_one_process_per_file() {
    let files = [PathBuf::from("/tmp/a"), PathBuf::from("/tmp/b c")];
    let d = Path::new("/x.desktop");
    assert_eq!(commands("view %f", &files, "V", d), [vec!["view", "/tmp/a"], vec!["view", "/tmp/b c"]]);
    assert_eq!(commands("view %u", &files, "V", d), [vec!["view", "file:///tmp/a"], vec!["view", "file:///tmp/b%20c"]]);
    assert_eq!(commands("view %F", &files, "V", d), [vec!["view", "/tmp/a", "/tmp/b c"]]);
    assert_eq!(commands("view %f", &files[..1], "V", d), [vec!["view", "/tmp/a"]]);
}

fn wait_for(path: &Path) -> String {
    let until = std::time::Instant::now() + Duration::from_secs(5);
    while !path.exists() && std::time::Instant::now() < until {
        std::thread::sleep(Duration::from_millis(10));
    }
    std::thread::sleep(Duration::from_millis(50));
    std::fs::read_to_string(path).unwrap_or_default()
}

#[test]
fn launching_runs_in_the_entry_path_with_the_token_once_per_file() {
    let t = Temp::new("apps-path");
    let work = t.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    let desktop = t.path().join("each.desktop");
    std::fs::write(&desktop, format!("[Desktop Entry]\nType=Application\nName=E\nPath={}\nExec=sh -c 'echo \"$PWD $XDG_ACTIVATION_TOKEN\" > \"$(basename \"$0\").out\"' %f\n", work.display())).unwrap();
    launch(&desktop, &[PathBuf::from("/tmp/one"), PathBuf::from("/tmp/two")], Some("tok_TIME1")).unwrap();
    assert_eq!(wait_for(&work.join("one.out")).trim(), format!("{} tok_TIME1", work.display()));
    assert_eq!(wait_for(&work.join("two.out")).trim(), format!("{} tok_TIME1", work.display()));
}

#[test]
fn a_remembered_custom_command_becomes_a_user_entry_and_the_default() {
    let t = Temp::new("apps-custom");
    let (apps, config) = (t.path().join("applications"), t.path().join("config"));
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(config.join("mimeapps.list"), "[Default Applications]\nimage/png=eog.desktop\n").unwrap();
    assert_eq!(with_files("gimp --new"), "gimp --new %F");
    assert_eq!(with_files("gimp %U"), "gimp %U");
    assert_eq!(user_app_text("/usr/bin/gimp %F", "image/png"), "[Desktop Entry]\nType=Application\nName=gimp\nExec=/usr/bin/gimp %F\nMimeType=image/png;\nNoDisplay=true\nTerminal=false\n");
    let out = t.path().join("ran");
    let command = format!("sh -c 'echo \"$1\" > {}' sh", out.display());
    run_custom_in(&apps, &config, &command, &[PathBuf::from("/tmp/pic.png")], "image/png", true, None).unwrap();
    assert_eq!(wait_for(&out).trim(), "/tmp/pic.png");
    let made: Vec<_> = std::fs::read_dir(&apps).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    assert!(made.len() == 1 && made[0].starts_with("userapp-sh-") && made[0].ends_with(".desktop"), "{made:?}");
    let list = std::fs::read_to_string(config.join("mimeapps.list")).unwrap();
    assert_eq!(flit_core::mimeapps::default_app(&list, "image/png").as_deref(), Some(made[0].as_str()));
    assert_eq!(flit_core::mimeapps::apps_in(&list, flit_core::mimeapps::ADDED, "image/png"), [made[0].as_str()]);
    let c = Catalog::load_for(&[t.path().to_path_buf()], std::slice::from_ref(&config), &[]);
    assert_eq!(handlers("image/png", &c.entries, &c.assoc)[0].id, made[0], "NoDisplay user entries still open their own type");
    assert!(choices(&c.entries).is_empty(), "the chooser does not list user entries");
    assert_eq!(desktop_id(&apps.join("kde/a.desktop"), &[t.path().to_path_buf()]), "kde-a.desktop");
}
