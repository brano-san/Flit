use super::*;
use crate::temp::Temp;

const KDE: &str = "[Desktop Entry]\nType=Service\nMimeType=image/png;image/jpeg;\nActions=resize;half\nX-KDE-Submenu=Images\n\n[Desktop Action resize]\nName=Resize\nName[ru]=Изменить размер\nIcon=transform-scale\nExec=convert %f -resize 50% %f\n\n[Desktop Action half]\nName=Half\nExec=half %F\n";
const NEMO: &str = "[Nemo Action]\nActive=true\nName=Compress _PDF\nExec=<compress.sh %F>\nSelection=notnone\nExtensions=pdf;\n";
const THUNAR: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<actions>\n<action>\n\t<icon>gimp</icon>\n\t<name>Edit in GIMP</name>\n\t<unique-id>1</unique-id>\n\t<command>gimp %F</command>\n\t<patterns>*.jpg;*.png</patterns>\n\t<image-files/>\n</action>\n<action>\n\t<icon>utilities-terminal</icon>\n\t<name>Count &amp; list</name>\n\t<command>ls -l %f | wc -l</command>\n\t<patterns>*</patterns>\n\t<directories/>\n</action>\n</actions>\n";

fn item(path: &str, dir: bool, mime: &str) -> Item {
    Item { path: PathBuf::from(path), dir, removable: false, mime: mime.to_owned() }
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
    assert_eq!((entries.len(), launches.len()), (1, 2), "both actions sit in their X-KDE-Submenu");
    assert!(matches!(&launches[1], Launch::Command { runs, dir: Some(_) } if runs[0].0 == ["half", "/p/a.png"]));
}

fn labels(items: &[MenuItem]) -> Vec<String> {
    items
        .iter()
        .map(|i| match i {
            MenuItem::Command { label, .. } => label.clone(),
            MenuItem::Submenu { label, items } => format!("{label} > {}", labels(items).join(", ")),
            MenuItem::Separator => "--".to_owned(),
            MenuItem::Header(h) => h.clone(),
        })
        .collect()
}

fn script(dir: &Path, name: &str) {
    let p = dir.join(name);
    std::fs::write(&p, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn actions_are_grouped_by_where_they_come_from_and_kde_submenus_nest() {
    let t = Temp::new("scripts-groups");
    let data = t.path().join("data");
    std::fs::create_dir_all(data.join("nautilus/scripts")).unwrap();
    script(&data.join("nautilus/scripts"), "Backup");
    let kde = data.join("kio/servicemenus");
    std::fs::create_dir_all(&kde).unwrap();
    std::fs::write(kde.join("resize.desktop"), KDE).unwrap();
    let system = t.path().join("system");
    std::fs::create_dir_all(system.join("kio/servicemenus")).unwrap();
    std::fs::write(system.join("kio/servicemenus/plain.desktop"), "[Desktop Entry]\nType=Service\nMimeType=image/png;\nActions=print\n\n[Desktop Action print]\nName=Print it\nExec=lp %F\n").unwrap();
    std::fs::set_permissions(kde.join("resize.desktop"), std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::create_dir_all(data.join("nemo/actions")).unwrap();
    std::fs::write(data.join("nemo/actions/shrink.nemo_action"), "[Nemo Action]\nName=Shrink\nExec=shrink %F\nSelection=any\nExtensions=png;\n").unwrap();
    let config = t.path().join("config");
    std::fs::create_dir_all(config.join("Thunar")).unwrap();
    std::fs::write(config.join("Thunar/uca.xml"), THUNAR).unwrap();
    let actions = load(&[data, system], Some(&config), "ru_RU.UTF-8");
    let (entries, launches) = menu(&actions, &[item("/p/a.png", false, "image/png")], Path::new("/p"));
    assert_eq!(labels(&entries), ["Backup", "--", "Images > Изменить размер, Half", "Print it", "--", "Shrink", "--", "Edit in GIMP"]);
    let runs: Vec<String> = launches
        .iter()
        .map(|l| match l {
            Launch::Command { runs, .. } => runs[0].0.join(" "),
            _ => String::new(),
        })
        .collect();
    assert_eq!(runs.len(), 6, "one launch per command, in menu order");
    assert!(runs[0].ends_with("Backup a.png"));
    assert_eq!(runs[1..], ["convert /p/a.png -resize 50% /p/a.png", "half /p/a.png", "lp /p/a.png", "shrink /p/a.png", "sh -c gimp '/p/a.png'"]);
}

#[test]
fn a_nemo_separator_joins_the_files_into_one_argument_and_without_it_they_stay_apart() {
    let base = Path::new("/n");
    let two = [PathBuf::from("/d/a.pdf"), PathBuf::from("/d/b.pdf")];
    let joined = nemo_action(&NEMO.replace("Exec=<compress.sh %F>", "Exec=pdfunite %F\nSeparator=,"), base, "").unwrap();
    assert_eq!(commands(&joined, &two, Path::new("/d"))[0].0, ["pdfunite", "/d/a.pdf,/d/b.pdf"]);
    let apart = nemo_action(&NEMO.replace("Exec=<compress.sh %F>", "Exec=pdfunite %F"), base, "").unwrap();
    assert_eq!(commands(&apart, &two, Path::new("/d"))[0].0, ["pdfunite", "/d/a.pdf", "/d/b.pdf"]);
    assert_eq!(commands(&joined, &two[..1], Path::new("/d"))[0].0, ["pdfunite", "/d/a.pdf"]);
    let thunar = thunar_actions(THUNAR);
    let (entries, _) = menu(&thunar, &[item("/d/x.png", false, "image/png")], Path::new("/d"));
    assert_eq!(labels(&entries), ["Edit in GIMP"], "one source alone gets no separators");
}

#[test]
fn unknown_nemo_conditions_hide_the_action() {
    let text = "[Nemo Action]\nName=Open\nExec=open\nExtensions=any;\n";
    assert!(nemo_action(text, Path::new("/actions"), "en").is_some());
    assert!(nemo_action(&format!("{text}Conditions=unknown;"), Path::new("/actions"), "en").is_none());
}

#[test]
fn nemo_menu_requires_every_dependency_and_condition() {
    let load = |more: &str| nemo_with(&format!("{NEMO}{more}"), Path::new("/actions"), "", |p| matches!(p, "tool" | "/opt/app"), |cmd| cmd == "probe --ready", |name| name == "org.example.App", |s, k| (s == "org.example" && k == "theme").then(|| "'dark'".into()));
    let action = load("Dependencies=tool;/opt/app;!missing;\nConditions=exec probe --ready;dbus org.example.App;gsettings org.example theme s eq 'dark';removable;").unwrap();
    let mut selected = item("/media/d/a.pdf", false, "application/pdf");
    assert!(!applies(&action, std::slice::from_ref(&selected)));
    selected.removable = true;
    assert!(applies(&action, &[selected]));
    for reject in ["Dependencies=missing;", "Dependencies=!tool;", "Conditions=desktop;", "Conditions=exec failed;", "Conditions=dbus absent;", "Conditions=gsettings org.example theme s eq 'light';", "Conditions=gsettings org.example theme;"] {
        assert!(load(reject).is_none(), "{reject}");
    }
}

fn nemo_with(text: &str, base: &Path, lang: &str, program: impl Fn(&str) -> bool, exec_ok: impl Fn(&str) -> bool, bus: impl Fn(&str) -> bool, setting: impl Fn(&str, &str) -> Option<String>) -> Option<Action> {
    nemo_action(text, base, lang).filter(|action| action.passes(program, exec_ok, bus, setting))
}

fn checked_setting(condition: &str, actual: &str) -> bool {
    nemo_with(&format!("{NEMO}Conditions=gsettings org.example key{condition};"), Path::new("/actions"), "", |_| false, |_| false, |_| false, |schema, key| (schema == "org.example" && key == "key").then(|| actual.to_owned())).is_some()
}

#[test]
fn nemo_gsettings_checks_boolean_keys_and_compares_typed_values() {
    assert!(checked_setting("", "true\n"));
    assert!(!checked_setting("", "false"));
    assert!(!checked_setting("", "'true'"));
    for (kind, actual, equal, less, greater, ordered) in [
        ("b", "true", "true", "false", "false", false),
        ("s", "'two words'", "'two words'", "'one'", "'zzz'", false),
        ("i", "-2", "-2", "-10", "10", true),
        ("u", "uint32 12", "12", "9", "20", true),
        ("d", "2.5", "2.5", "0.25", "10.5", true),
        ("x", "int64 -9223372036854775807", "-9223372036854775807", "-9223372036854775808", "0", true),
    ] {
        for (op, value, wanted) in [("eq", equal, true), ("eq", less, false), ("ne", equal, false), ("ne", less, true), ("gt", less, ordered), ("lt", greater, ordered), ("lt", less, false), ("gt", greater, false)] {
            let condition = format!(" {kind} {op} {value}");
            assert_eq!(checked_setting(&condition, actual), wanted, "{actual} against{condition}");
        }
    }
    for (condition, actual) in [(" true", "true"), (" b true", "true"), (" b is true", "true"), (" b lt true", "false"), (" i ne 0", "'12'"), (" u ne -1", "uint32 12"), (" i eq 12 extra", "12")] {
        assert!(!checked_setting(condition, actual), "{condition}");
    }
}

#[test]
fn nemo_sidebar_conditions_apply_to_the_file_list() {
    for (condition, visible) in [("sidebar-allow", true), ("sidebar-only", false), ("sidebar-only;sidebar-allow", false)] {
        let actions: Vec<_> = nemo_action(&format!("{NEMO}Conditions={condition};"), Path::new("/actions"), "").into_iter().collect();
        assert_eq!(!names(&actions, &[item("/a.pdf", false, "application/pdf")]).is_empty(), visible, "{condition}");
    }
}

#[test]
fn nemo_exec_kills_a_slow_command_within_the_deadline() {
    let start = std::time::Instant::now();
    let passed = exec_condition("/bin/sleep 5", Path::new("/"));
    let elapsed = start.elapsed();
    assert!(elapsed < std::time::Duration::from_millis(1500), "{elapsed:?}");
    assert!(!passed);
}

#[test]
fn nemo_exec_runs_relative_helpers_from_root_and_checks_their_exit_status() {
    let t = Temp::new("nemo-exec");
    let name = "check arg";
    let path = t.path().join(name);
    std::fs::write(&path, "#!/bin/sh\n[ \"$PWD\" = / ] && [ \"$1\" = yes ]\n").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(exec_condition("<'check arg' yes>", t.path()));
    assert!(!exec_condition("<'check arg' no>", t.path()));
}

#[test]
fn nemo_absolute_dependencies_accept_a_nonexecutable_file() {
    let t = Temp::new("nemo-dependencies");
    let file = t.file("data", 0);
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    for (dep, visible) in [(file.display().to_string(), true), (format!("!{}", file.display()), false), (t.path().join("absent").display().to_string(), false)] {
        let text = format!("{NEMO}Dependencies={dep};");
        assert_eq!(nemo_with(&text, t.path(), "", dependency, |_| false, |_| false, |_, _| None).is_some(), visible, "{dep}");
    }
}

#[test]
fn nemo_removable_uses_the_nearest_mount_of_the_first_selection() {
    let mounts = flit_platform_linux::mounts::parse(b"1 0 8:1 / / rw - ext4 /dev/a rw\n2 1 8:2 / /media/disk rw - vfat /dev/b rw\n3 2 0:1 / /media/disk/remote rw - cifs share rw");
    let local = Path::new("/media/disk");
    let remote = local.join("remote");
    let action = nemo_action(&format!("{NEMO}Extensions=any;\nConditions=removable;"), local, "").unwrap();
    let mut items = [item("/media/disk", true, "inode/directory"), item("/media/disk/remote", true, "inode/directory")];
    items[0].removable = on_removable_mount(local, &mounts);
    items[1].removable = on_removable_mount(&remote, &mounts);
    assert!(items[0].removable);
    assert!(!items[1].removable);
    assert!(applies(&action, &items));
    items.reverse();
    assert!(!applies(&action, &items));
    assert!(!applies(&action, &[]));
}
