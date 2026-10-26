use super::*;
use crate::temp::Temp;

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
