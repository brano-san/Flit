use super::*;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

struct Temp(PathBuf);

impl Temp {
    fn new() -> Self {
        let n = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let path = std::env::temp_dir().join(format!("flit-system-journal-{}-{n}", std::process::id()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Mock {
    journal: PathBuf,
    values: RefCell<HashMap<Target, Value>>,
    fail: Cell<bool>,
    keys: RefCell<HashSet<String>>,
}

impl Mock {
    fn new(dir: &Path) -> Self {
        Self { journal: dir.join(JOURNAL), values: RefCell::default(), fail: Cell::new(false), keys: RefCell::default() }
    }
}

impl System for Mock {
    fn read(&self, target: &Target) -> io::Result<Option<Value>> {
        Ok(self.values.borrow().get(target).cloned())
    }

    fn write(&self, target: &Target, value: Option<&Value>) -> io::Result<()> {
        let recorded = load(&self.journal)?;
        assert!(recorded.changes.iter().any(|c| &c.target == target));
        if self.fail.get() {
            return Err(io::Error::other("write rejected"));
        }
        if let Some(value) = value {
            if let Target::Registry { key, .. } = target {
                let mut path = key.as_str();
                loop {
                    self.keys.borrow_mut().insert(path.to_owned());
                    let Some((parent, _)) = path.rsplit_once('\\') else { break };
                    path = parent;
                }
            }
            self.values.borrow_mut().insert(target.clone(), value.clone());
        } else {
            self.values.borrow_mut().remove(target);
        }
        Ok(())
    }

    fn registry_key_exists(&self, key: &str) -> io::Result<bool> {
        Ok(self.keys.borrow().contains(key))
    }

    fn remove_empty_registry_key(&self, key: &str) -> io::Result<()> {
        let prefix = format!(r"{key}\");
        let mut keys = self.keys.borrow_mut();
        if !keys.iter().any(|path| path.starts_with(&prefix))
            && !self.values.borrow().keys().any(|target| matches!(target, Target::Registry { key: path, .. } if path == key))
        {
            keys.remove(key);
        }
        Ok(())
    }
}

fn key(name: &str) -> Target {
    Target::Registry { key: r"Software\Flit\Test".into(), name: name.into() }
}

fn value(n: u8) -> Value {
    Value { kind: 3, bytes: vec![n] }
}

#[test]
fn system_journal_records_previous_before_write_and_restores_in_reverse_order() {
    let dir = Temp::new();
    let sys = Mock::new(&dir.0);
    let journal = Journal::new(&dir.0);
    let a = key("a");
    let b = key("b");
    sys.values.borrow_mut().insert(a.clone(), value(1));
    journal.apply(&sys, Group::Autostart, &[(a.clone(), value(2))], false).unwrap();
    journal.apply(&sys, Group::Autostart, &[(a.clone(), value(3))], false).unwrap();
    journal.apply(&sys, Group::Archive, &[(b.clone(), value(4))], false).unwrap();
    assert_eq!(journal.inspect().unwrap().changes[0].previous, Some(value(1)));
    journal.undo(&sys, Some(Group::Archive)).unwrap();
    assert_eq!(sys.read(&b).unwrap(), None);
    assert_eq!(sys.read(&a).unwrap(), Some(value(3)));
    journal.undo(&sys, None).unwrap();
    assert_eq!(sys.read(&a).unwrap(), Some(value(1)));
    assert!(journal.inspect().unwrap().changes.is_empty());
}

#[test]
fn system_journal_never_overwrites_an_unreadable_file() {
    let dir = Temp::new();
    let sys = Mock::new(&dir.0);
    let journal = Journal::new(&dir.0);
    fs::write(&sys.journal, b"{ broken").unwrap();
    assert!(journal.apply(&sys, Group::Archive, &[(key("test"), value(1))], false).is_err());
    assert!(journal.undo(&sys, None).is_err());
    assert!(sys.values.borrow().is_empty());
    assert_eq!(fs::read(&sys.journal).unwrap(), b"{ broken");
}

#[test]
fn system_journal_recovers_from_failed_write_and_preserves_external_edit() {
    let dir = Temp::new();
    let sys = Mock::new(&dir.0);
    let journal = Journal::new(&dir.0);
    let a = key("a");
    sys.fail.set(true);
    assert!(journal.apply(&sys, Group::Autostart, &[(a.clone(), value(2))], false).is_err());
    assert_eq!(journal.inspect().unwrap().changes.len(), 1);
    journal.undo(&sys, Some(Group::Autostart)).unwrap();
    sys.fail.set(false);
    journal.apply(&sys, Group::Autostart, &[(a.clone(), value(3))], false).unwrap();
    sys.values.borrow_mut().insert(a.clone(), value(7));
    journal.undo(&sys, None).unwrap();
    assert_eq!(sys.read(&a).unwrap(), Some(value(7)));
    journal.apply(&sys, Group::Autostart, &[(a.clone(), value(3))], true).unwrap();
    assert_eq!(sys.read(&a).unwrap(), Some(value(7)));
}

#[test]
fn system_journal_lock_blocks_another_handle_until_drop() {
    let dir = Temp::new();
    let journal = Journal::new(&dir.0);
    let lock = journal.lock().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let path = dir.0.clone();
    let worker = std::thread::spawn(move || {
        tx.send(Journal::new(&path).inspect().unwrap().changes.len()).unwrap();
    });
    assert!(rx.recv_timeout(Duration::from_millis(80)).is_err());
    drop(lock);
    assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), 0);
    worker.join().unwrap();
}


#[test]
fn system_journal_restores_the_linux_uri_handler_without_reverting_other_preferences() {
    let dir = Temp::new();
    let path = dir.0.join("mimeapps.list");
    fs::write(&path, "[Default Applications]\nx-scheme-handler/flit=previous.desktop;\nimage/png=old.desktop;\n").unwrap();
    let target = Target::MimeDefault(path.clone());
    let journal = Journal::new(&dir.0);
    journal.apply(&Native, Group::Protocol, &[(target, Value { kind: 0, bytes: b"flit-uri.desktop;".to_vec() })], false).unwrap();
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.contains("x-scheme-handler/flit=flit-uri.desktop;"));
    fs::write(&path, text.replace("image/png=old.desktop;", "image/png=new.desktop;")).unwrap();
    journal.undo(&Native, Some(Group::Protocol)).unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "[Default Applications]\nx-scheme-handler/flit=previous.desktop;\nimage/png=new.desktop;\n");
}

#[test]
fn system_journal_does_not_reactivate_changes_after_an_incomplete_unregister() {
    let dir = Temp::new();
    let sys = Mock::new(&dir.0);
    let journal = Journal::new(&dir.0);
    let target = key("autostart");
    journal.apply(&sys, Group::Autostart, &[(target.clone(), value(1))], false).unwrap();
    sys.fail.set(true);
    assert!(journal.undo(&sys, None).is_err());
    sys.fail.set(false);
    journal.apply(&sys, Group::Autostart, &[(target.clone(), value(2))], true).unwrap();
    assert_eq!(sys.read(&target).unwrap(), Some(value(1)));
    journal.undo(&sys, None).unwrap();
    assert_eq!(sys.read(&target).unwrap(), None);
}

#[test]
fn system_journal_removes_only_its_own_empty_registry_keys_on_undo() {
    let dir = Temp::new();
    let sys = Mock::new(&dir.0);
    let journal = Journal::new(&dir.0);
    let reg = |key: &str, name: &str| Target::Registry { key: key.into(), name: name.into() };
    let protocol = r"Software\Classes\flit";
    let command = r"Software\Classes\flit\shell\open\command";
    let zip = r"Software\Classes\.zip";
    let zip_open_with = r"Software\Classes\.zip\OpenWithProgids";
    let seven_zip_open_with = r"Software\Classes\.7z\OpenWithProgids";
    let rar = r"Software\Classes\.rar";
    let rar_open_with = r"Software\Classes\.rar\OpenWithProgids";
    let archive = r"Software\Classes\Flit.Archive";
    let external = reg(seven_zip_open_with, "Other.Archiver");
    sys.keys.borrow_mut().extend(["Software".into(), r"Software\Classes".into(), zip.into()]);
    journal.apply(&sys, Group::Protocol, &[(reg(protocol, ""), value(1)), (reg(command, ""), value(2))], false).unwrap();
    journal.apply(&sys, Group::Archive, &[
        (reg(archive, ""), value(3)),
        (reg(zip_open_with, "Flit.Archive"), value(4)),
        (reg(seven_zip_open_with, "Flit.Archive"), value(5)),
        (reg(rar_open_with, "Flit.Archive"), value(6)),
    ], false).unwrap();
    sys.values.borrow_mut().insert(external.clone(), value(8));
    let journal = Journal::new(&dir.0);
    journal.undo(&sys, None).unwrap();
    for key in [protocol, r"Software\Classes\flit\shell", command, zip_open_with, rar, rar_open_with, archive] {
        assert!(!sys.registry_key_exists(key).unwrap(), "{key} is still present");
    }
    for key in [zip, seven_zip_open_with, r"Software\Classes"] {
        assert!(sys.registry_key_exists(key).unwrap(), "{key} was removed");
    }
    assert_eq!(sys.read(&external).unwrap(), Some(value(8)));
    assert_eq!(sys.values.borrow().len(), 1);
}

#[test]
fn unshown_archive_offer_remains_eligible() {
    let dir = Temp::new();
    let journal = Journal::new(&dir.0);
    assert!(journal.offer_archive_registration().unwrap());
    assert!(journal.offer_archive_registration().unwrap());
    journal.mark_archive_offer_shown().unwrap();
    assert!(!Journal::new(&dir.0).offer_archive_registration().unwrap());
}
