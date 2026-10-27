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

    fn snapshot(&self, what: Integration) -> Vec<Seen> {
        entries(what, "x", "y").into_iter().map(|(k, n, _)| (k.clone(), n, self.get(&k, n), exists(self.0.hive, &self.0.class(&k)))).collect()
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
    let before = sb.snapshot(Integration::DefaultManager);
    set(&sb.0, Integration::DefaultManager, true, EXE, "Open in Flit").unwrap();
    let text = |k: &str, n: &str| sb.get(k, n).map(|(t, d)| (t, strings(&d).join("")));
    assert_eq!(text(r"Folder\shell\open\command", ""), Some((REG_SZ.0, format!("\"{EXE}\" \"%1\""))));
    assert_eq!(text(r"Folder\shell\explore\command", "DelegateExecute"), Some((REG_SZ.0, String::new())));
    assert_eq!(text(WIN_E, ""), Some((REG_SZ.0, format!("\"{EXE}\" \"{HOME}\""))));
    set(&sb.0, Integration::DefaultManager, true, r"D:\Moved\flit.exe", "Open in Flit").unwrap();
    assert_eq!(text(r"Folder\shell\open\command", ""), Some((REG_SZ.0, "\"D:\\Moved\\flit.exe\" \"%1\"".into())));
    set(&sb.0, Integration::DefaultManager, false, EXE, "Open in Flit").unwrap();
    assert_eq!(sb.snapshot(Integration::DefaultManager), before);
    assert_eq!(text(r"Folder\shell\open", "keep"), Some((REG_SZ.0, "me".into())));
    assert!(!exists(sb.0.hive, &sb.0.class("CLSID")));
    assert!(!exists(sb.0.hive, &sb.0.backup));
    set(&sb.0, Integration::DefaultManager, false, EXE, "Open in Flit").unwrap();
    assert_eq!(sb.snapshot(Integration::DefaultManager), before);
}

#[test]
fn open_in_flit_verbs_disappear_completely_when_turned_off() {
    let sb = Sandbox::new("verb");
    sb.put(r"Directory\Background\shell\Other", "", REG_SZ.0, &sz("other"));
    set(&sb.0, Integration::OpenInFlit, true, EXE, "Open in Flit").unwrap();
    assert_eq!(sb.get(r"Directory\shell\Flit", "MUIVerb").map(|(_, d)| strings(&d)), Some(vec!["Open in Flit".to_owned()]));
    assert_eq!(sb.get(r"Directory\Background\shell\Flit\command", "").map(|(_, d)| strings(&d)), Some(vec![format!("\"{EXE}\" \"%V\"")]));
    assert!(sb.get(r"Drive\shell\Flit\command", "").is_some());
    set(&sb.0, Integration::OpenInFlit, false, EXE, "Open in Flit").unwrap();
    for k in VERB_KEYS {
        assert!(!exists(sb.0.hive, &sb.0.class(k)), "{k}");
    }
    assert!(!exists(sb.0.hive, &sb.0.class("Drive")));
    assert!(sb.get(r"Directory\Background\shell\Other", "").is_some());
}

#[test]
fn features_keep_separate_backups() {
    let sb = Sandbox::new("both");
    set(&sb.0, Integration::DefaultManager, true, EXE, "a").unwrap();
    set(&sb.0, Integration::OpenInFlit, true, EXE, "a").unwrap();
    set(&sb.0, Integration::OpenInFlit, false, EXE, "a").unwrap();
    assert!(sb.get(r"Folder\shell\open\command", "").is_some());
    set(&sb.0, Integration::DefaultManager, false, EXE, "a").unwrap();
    assert!(!exists(sb.0.hive, &sb.0.class("Folder")));
}

#[test]
fn the_real_user_root_points_at_classes() {
    let r = Root::user();
    assert_eq!((r.classes.as_str(), r.backup.as_str()), (r"Software\Classes", r"Software\Flit\Integration"));
}
