use super::*;
use windows::Win32::System::Registry::REG_EXPAND_SZ;

type Seen = (String, &'static str, Option<(u32, Vec<u8>)>, bool);

struct Sandbox(Root, String);

impl Sandbox {
    fn new(name: &str) -> Self {
        let base = format!(r"Software\FlitTest\{}-{name}", std::process::id());
        Sandbox(Root { hive: HKEY_CURRENT_USER, classes: format!(r"{base}\Classes"), backup: format!(r"{base}\Backup") }, base)
    }

    fn get(&self, key: &str, name: &str) -> Option<(u32, Vec<u8>)> {
        read(self.0.hive, &self.0.class(key), name)
    }

    fn put(&self, key: &str, name: &str, kind: u32, data: &[u8]) {
        write(self.0.hive, &self.0.class(key), name, kind, data).unwrap();
    }

    fn snapshot(&self) -> Vec<Seen> {
        entries("x").into_iter().map(|(k, n, _)| (k.clone(), n, self.get(&k, n), exists(self.0.hive, &self.0.class(&k)))).collect()
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        delete_tree(self.0.hive, &self.1);
        if empty(self.0.hive, r"Software\FlitTest") {
            delete_tree(self.0.hive, r"Software\FlitTest");
        }
    }
}

const EXE: &str = r"C:\Apps\Flit\flit.exe";

#[test]
fn default_manager_round_trip_restores_the_previous_values_exactly() {
    let sb = Sandbox::new("manager");
    sb.put(r"Folder\shell\open\command", "", REG_EXPAND_SZ.0, &sz(r"%SystemRoot%\Explorer.exe"));
    sb.put(r"Folder\shell\explore\command", "DelegateExecute", REG_SZ.0, &sz("{11dbb47c-a525-400b-9e80-a54615a090c0}"));
    sb.put(r"Folder\shell\open", "keep", REG_SZ.0, &sz("me"));
    let before = sb.snapshot();
    assert!(set(&sb.0, true, EXE).unwrap());
    assert!(!set(&sb.0, true, EXE).unwrap(), "an unchanged registration writes nothing and tells the shell nothing");
    let text = |k: &str, n: &str| sb.get(k, n).map(|(t, d)| (t, strings(&d).join("")));
    assert_eq!(text(r"Folder\shell\open\command", ""), Some((REG_SZ.0, format!("\"{EXE}\" \"%1\""))));
    assert_eq!(text(r"Folder\shell\explore\command", "DelegateExecute"), Some((REG_SZ.0, String::new())));
    assert_eq!(text(WIN_E, ""), Some((REG_SZ.0, format!("\"{EXE}\" \"{HOME}\""))));
    set(&sb.0, true, r"D:\Moved\flit.exe").unwrap();
    assert_eq!(text(r"Folder\shell\open\command", ""), Some((REG_SZ.0, "\"D:\\Moved\\flit.exe\" \"%1\"".into())));
    assert!(set(&sb.0, false, EXE).unwrap());
    assert_eq!(sb.snapshot(), before);
    assert_eq!(text(r"Folder\shell\open", "keep"), Some((REG_SZ.0, "me".into())));
    assert!(!exists(sb.0.hive, &sb.0.class("CLSID")));
    assert!(!exists(sb.0.hive, &sb.0.backup));
    assert!(!set(&sb.0, false, EXE).unwrap());
    assert_eq!(sb.snapshot(), before);
}

fn plant_old_verbs(sb: &Sandbox, exe: &str) {
    for k in OLD_VERBS {
        sb.put(k, "MUIVerb", REG_SZ.0, &sz("Open in Flit"));
        sb.put(k, "Icon", REG_SZ.0, &sz(&format!("\"{exe}\",0")));
        sb.put(&format!(r"{k}\command"), "", REG_SZ.0, &sz(&format!("\"{exe}\" \"%V\"")));
    }
}

#[test]
fn old_open_in_flit_verbs_are_restored_from_their_backup() {
    let sb = Sandbox::new("old-backup");
    sb.put(r"Directory\Background\shell\Other", "", REG_SZ.0, &sz("other"));
    let backup = sb.0.backup_of(OLD_VERB_BACKUP);
    let created = ["Drive", r"Drive\shell", r"Drive\shell\Flit", r"Drive\shell\Flit\command", r"Directory\shell\Flit", r"Directory\shell\Flit\command", r"Directory\Background\shell\Flit", r"Directory\Background\shell\Flit\command", "Directory\\shell"].map(String::from);
    write(sb.0.hive, &backup, CREATED, REG_MULTI_SZ.0, &multi_sz(&created)).unwrap();
    let absent: Vec<String> = old_verb_entries().iter().map(|(k, n, _)| slot(k, n)).collect();
    write(sb.0.hive, &backup, ABSENT, REG_MULTI_SZ.0, &multi_sz(&absent)).unwrap();
    plant_old_verbs(&sb, EXE);
    assert!(forget_open_in_flit(&sb.0).unwrap());
    for k in OLD_VERBS {
        assert!(!exists(sb.0.hive, &sb.0.class(k)), "{k}");
    }
    assert!(!exists(sb.0.hive, &sb.0.class("Drive")));
    assert!(sb.get(r"Directory\Background\shell\Other", "").is_some());
    assert!(!exists(sb.0.hive, &sb.0.backup));
    assert!(!forget_open_in_flit(&sb.0).unwrap());
}

#[test]
fn old_open_in_flit_verbs_without_a_backup_go_only_when_they_launch_flit() {
    let sb = Sandbox::new("old-bare");
    plant_old_verbs(&sb, r"C:\Moved\FLIT.EXE");
    sb.put(r"Drive\shell\Flit\command", "", REG_SZ.0, &sz("\"C:\\Other\\tool.exe\" \"%V\""));
    assert!(forget_open_in_flit(&sb.0).unwrap());
    assert!(!exists(sb.0.hive, &sb.0.class(r"Directory\shell\Flit")));
    assert!(!exists(sb.0.hive, &sb.0.class(r"Directory\Background\shell\Flit")));
    assert!(sb.get(r"Drive\shell\Flit\command", "").is_some(), "a key another program owns stays");
    assert!(!forget_open_in_flit(&sb.0).unwrap());
}

#[test]
fn only_a_flit_command_counts_as_ours() {
    assert!(launches_flit("\"C:\\Apps\\Flit\\flit.exe\" \"%V\""));
    assert!(!launches_flit("\"C:\\Apps\\Flit\\flit.exe\" \"%1\""));
    assert!(!launches_flit("\"C:\\Apps\\notflit.exe\" \"%V\""));
    assert!(!launches_flit("C:\\Apps\\Flit\\flit.exe %V"));
}

#[test]
fn the_default_manager_backup_survives_the_cleanup() {
    let sb = Sandbox::new("both");
    set(&sb.0, true, EXE).unwrap();
    plant_old_verbs(&sb, EXE);
    forget_open_in_flit(&sb.0).unwrap();
    assert!(exists(sb.0.hive, &sb.0.backup_of(MANAGER_BACKUP)));
    set(&sb.0, false, EXE).unwrap();
    assert!(!exists(sb.0.hive, &sb.0.class("Folder")));
    assert!(!exists(sb.0.hive, &sb.0.backup));
}

#[test]
fn the_real_user_root_points_at_classes() {
    let r = Root::user();
    assert_eq!((r.classes.as_str(), r.backup.as_str()), (r"Software\Classes", r"Software\Flit\Integration"));
}
