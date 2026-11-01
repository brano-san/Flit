use super::*;
use crate::temp::Temp;
use std::time::{Duration, SystemTime};

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
    assert_eq!(items.len(), 1);
    assert_eq!(launches, [flit_core::provider::Launch::With { program: fp(Path::new("/usr/share/applications/viewer.desktop")), args: vec![fp(&a), fp(&b)] }]);
    assert_eq!(open_with(&[fp(&a)], &entries, &Assoc::default()).0.len(), 2);
    assert!(open_with(&[fp(t.path())], &entries, &Assoc::default()).0.is_empty(), "folders get no Open with");
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
    for i in 0..count {
        let mime = if i % 3 == 0 { "text/plain;image/png;" } else { "application/pdf;" };
        std::fs::write(apps.join(format!("app{i}.desktop")), format!("[Desktop Entry]\nType=Application\nName=App {i}\nName[de]=Programm {i}\nComment=Something {i}\nIcon=app{i}\nExec=app{i} %F\nMimeType={mime}\nCategories=Utility;\n")).unwrap();
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
