use super::*;
use crate::temp::Temp;

const KDE: &str = "[Desktop Entry]\nType=Service\nMimeType=image/png;image/jpeg;\nActions=resize;half\nX-KDE-Submenu=Images\n\n[Desktop Action resize]\nName=Resize\nName[ru]=Изменить размер\nIcon=transform-scale\nExec=convert %f -resize 50% %f\n\n[Desktop Action half]\nName=Half\nExec=half %F\n";
const NEMO: &str = "[Nemo Action]\nActive=true\nName=Compress _PDF\nExec=<compress.sh %F>\nSelection=notnone\nExtensions=pdf;\n";
const THUNAR: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<actions>\n<action>\n\t<icon>gimp</icon>\n\t<name>Edit in GIMP</name>\n\t<unique-id>1</unique-id>\n\t<command>gimp %F</command>\n\t<patterns>*.jpg;*.png</patterns>\n\t<image-files/>\n</action>\n<action>\n\t<icon>utilities-terminal</icon>\n\t<name>Count &amp; list</name>\n\t<command>ls -l %f | wc -l</command>\n\t<patterns>*</patterns>\n\t<directories/>\n</action>\n</actions>\n";

fn item(path: &str, dir: bool, mime: &str) -> Item {
    Item { path: PathBuf::from(path), dir, mime: mime.to_owned() }
}

fn names(actions: &[Action], items: &[Item]) -> Vec<String> {
    actions.iter().filter(|a| applies(a, items)).map(|a| a.name.clone()).collect()
}

#[test]
fn nautilus_scripts_run_with_the_selection_in_their_environment() {
    let t = Temp::new("nautilus-scripts");
    let dir = t.path().join("scripts");
    std::fs::create_dir_all(dir.join("Images")).unwrap();
    let make = |p: PathBuf, mode: u32| {
        std::fs::write(&p, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(mode)).unwrap();
    };
    make(dir.join("Backup"), 0o755);
    make(dir.join("Images/Shrink"), 0o755);
    make(dir.join("notes.txt"), 0o644);
    let scripts = nautilus_scripts(&dir);
    assert_eq!(scripts.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["Backup", "Images / Shrink"]);
    let files = [PathBuf::from("/home/u/a b.txt"), PathBuf::from("/home/u/c.txt")];
    let runs = commands(&scripts[0], &files, Path::new("/home/u"));
    let (argv, env) = &runs[0];
    assert_eq!(argv, &[dir.join("Backup").to_string_lossy().into_owned(), "a b.txt".into(), "c.txt".into()]);
    assert!(env.contains(&("NAUTILUS_SCRIPT_SELECTED_FILE_PATHS".into(), "/home/u/a b.txt\n/home/u/c.txt\n".into())));
    assert!(env.contains(&("NAUTILUS_SCRIPT_CURRENT_URI".into(), "file:///home/u".into())));
    assert_eq!(names(&scripts, &[]), ["Backup", "Images / Shrink"], "scripts are offered on the empty space too");
}

#[test]
fn kde_service_menus_match_their_types_and_expand_their_exec_lines() {
    let actions = service_menu(KDE, "ru_RU.UTF-8");
    assert_eq!(actions.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["Изменить размер", "Half"]);
    let png = [item("/p/a.png", false, "image/png")];
    assert_eq!(names(&actions, &png).len(), 2);
    assert!(names(&actions, &[item("/p/a.txt", false, "text/plain")]).is_empty());
    assert!(names(&actions, &[item("/p/dir", true, "inode/directory")]).is_empty());
    assert!(names(&actions, &[]).is_empty(), "service menus need a selection");
    let two = [PathBuf::from("/p/a.png"), PathBuf::from("/p/b.png")];
    assert_eq!(commands(&actions[0], &two, Path::new("/p")).len(), 2, "%f runs once per file");
    assert_eq!(commands(&actions[1], &two, Path::new("/p"))[0].0, ["half", "/p/a.png", "/p/b.png"]);
    assert!(service_menu("[Desktop Entry]\nType=Application\nActions=x\n[Desktop Action x]\nName=X\nExec=x\n", "").is_empty(), "applications are not service menus");
}

#[test]
fn nemo_actions_and_thunar_custom_actions_are_read() {
    let base = Path::new("/home/u/.local/share/nemo/actions");
    let nemo = nemo_action(NEMO, base, "").unwrap();
    assert_eq!(nemo.name, "Compress PDF");
    assert_eq!(names(std::slice::from_ref(&nemo), &[item("/d/a.PDF", false, "application/pdf")]), ["Compress PDF"]);
    assert!(names(std::slice::from_ref(&nemo), &[item("/d/a.txt", false, "text/plain")]).is_empty());
    assert_eq!(commands(&nemo, &[PathBuf::from("/d/a.pdf")], Path::new("/d"))[0].0, ["/home/u/.local/share/nemo/actions/compress.sh", "/d/a.pdf"]);
    assert!(nemo_action(&NEMO.replace("Active=true", "Active=false"), base, "").is_none());
    let thunar = thunar_actions(THUNAR);
    assert_eq!(thunar.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["Edit in GIMP", "Count & list"]);
    assert_eq!(names(&thunar, &[item("/d/x.png", false, "image/png")]), ["Edit in GIMP"]);
    assert_eq!(names(&thunar, &[item("/d/sub", true, "inode/directory")]), ["Count & list"]);
    assert!(names(&thunar, &[item("/d/x.txt", false, "text/plain")]).is_empty());
    let run = commands(&thunar[0], &[PathBuf::from("/d/it's.png"), PathBuf::from("/d/b.png")], Path::new("/d"));
    assert_eq!(run[0].0, ["sh", "-c", "gimp '/d/it'\\''s.png' '/d/b.png'"]);
}

#[test]
fn only_trusted_user_service_menus_load_and_an_empty_match_adds_no_submenu() {
    let t = Temp::new("service-menus");
    let user = t.path().join("home");
    let menus = user.join("kio/servicemenus");
    std::fs::create_dir_all(&menus).unwrap();
    std::fs::write(menus.join("resize.desktop"), KDE).unwrap();
    assert!(load(std::slice::from_ref(&user), None, "").is_empty(), "a user service menu without the execute bit is ignored, as in Plasma");
    std::fs::set_permissions(menus.join("resize.desktop"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let actions = load(std::slice::from_ref(&user), None, "");
    assert_eq!(actions.len(), 2);
    let (entries, launches) = menu(&actions, &[item("/p/a.txt", false, "text/plain")], Path::new("/p"));
    assert!(entries.is_empty() && launches.is_empty());
    let (entries, launches) = menu(&actions, &[item("/p/a.png", false, "image/png")], Path::new("/p"));
    assert_eq!((entries.len(), launches.len()), (2, 2));
    assert!(matches!(&launches[1], Launch::Command { runs, dir: Some(_) } if runs[0].0 == ["half", "/p/a.png"]));
}
