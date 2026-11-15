use super::*;
use crate::temp::Temp;
use crossbeam_channel::{unbounded, Receiver};

fn notify() -> (Notify<u32>, Receiver<(u32, Notice)>) {
    let (tx, rx) = unbounded();
    (Notify::new(move |k, n| tx.send((k, n)).unwrap()).unwrap(), rx)
}

fn settle(rx: &Receiver<(u32, Notice)>, key: u32) -> (HashMap<Vec<u8>, DirChange>, bool) {
    let mut last = HashMap::new();
    let mut rescan = false;
    while let Ok((k, n)) = rx.recv_timeout(Duration::from_millis(400)) {
        if k != key {
            continue;
        }
        match n {
            Notice::Changes(changes) => changes.into_iter().for_each(|c| {
                last.insert(c.name.clone(), c);
            }),
            Notice::Rescan => rescan = true,
        }
    }
    (last, rescan)
}

#[test]
fn changes_carry_final_state_and_metadata() {
    let t = Temp::new("notify");
    t.file("gone", 1);
    t.file("touched", 1);
    let (n, rx) = notify();
    n.watch(1, t.path()).unwrap();
    t.file("new", 3);
    std::fs::write(t.path().join("touched"), b"12345").unwrap();
    std::fs::remove_file(t.path().join("gone")).unwrap();
    t.file("flash", 1);
    std::fs::remove_file(t.path().join("flash")).unwrap();
    std::fs::rename(t.path().join("new"), t.path().join(OsStr::from_bytes(b"re\xFFnamed"))).unwrap();
    let (last, rescan) = settle(&rx, 1);
    assert!(!rescan);
    let kind = |name: &[u8]| last.get(name).map(|c| c.kind.clone());
    assert_eq!(kind(b"gone"), Some(ChangeKind::Removed));
    assert_eq!(kind(b"flash"), Some(ChangeKind::Removed));
    assert_eq!(kind(b"new"), Some(ChangeKind::Removed));
    assert_eq!(kind(b"re\xFFnamed"), Some(ChangeKind::Added));
    assert_eq!(last[&b"re\xFFnamed"[..]].meta.unwrap().size, 3);
    assert_eq!(last[&b"touched"[..]].meta.unwrap().size, 5);
}

#[test]
fn shared_directories_and_unwatch() {
    let t = Temp::new("notify-shared");
    let (n, rx) = notify();
    n.watch(1, t.path()).unwrap();
    n.watch(2, t.path()).unwrap();
    t.file("a", 1);
    let mut keys: Vec<u32> = std::iter::from_fn(|| rx.recv_timeout(Duration::from_millis(400)).ok()).map(|(k, _)| k).collect();
    keys.sort();
    keys.dedup();
    assert_eq!(keys, [1, 2]);
    assert!(n.unwatch(&1));
    assert!(!n.unwatch(&1));
    t.file("b", 1);
    let keys: Vec<u32> = std::iter::from_fn(|| rx.recv_timeout(Duration::from_millis(400)).ok()).map(|(k, _)| k).collect();
    assert!(!keys.is_empty() && keys.iter().all(|&k| k == 2), "{keys:?}");
    assert!(n.unwatch(&2));
    t.file("c", 1);
    assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());
}

#[test]
fn deleting_the_watched_folder_asks_for_a_rescan() {
    let t = Temp::new("notify-self");
    let sub = t.path().join("sub");
    std::fs::create_dir(&sub).unwrap();
    let (n, rx) = notify();
    n.watch(5, &sub).unwrap();
    std::fs::remove_dir(&sub).unwrap();
    assert!(settle(&rx, 5).1);
    assert!(n.watch(6, &sub).is_err());
}

fn event(wd: i32, mask: u32, name: &[u8]) -> Vec<u8> {
    let len = name.len().div_ceil(16) * 16;
    let mut e = Vec::new();
    e.extend(wd.to_ne_bytes());
    e.extend(mask.to_ne_bytes());
    e.extend(0u32.to_ne_bytes());
    e.extend((len as u32).to_ne_bytes());
    e.extend(name);
    e.resize(HEADER + len, 0);
    e
}

#[test]
fn queue_overflow_and_truncated_events_ask_for_a_full_rescan() {
    let whole = event(1, libc::IN_CREATE, b"a.txt");
    let (mut pending, mut overflow) = (HashMap::new(), false);
    parse(&whole, &mut pending, &mut overflow);
    assert!(!overflow && pending[&1].names == [b"a.txt".to_vec()]);
    for buf in [event(-1, libc::IN_Q_OVERFLOW, b""), [whole.clone(), event(1, libc::IN_CREATE, b"b.txt")[..HEADER + 3].to_vec()].concat(), [whole.clone(), vec![0; 5]].concat()] {
        let (mut pending, mut overflow) = (HashMap::new(), false);
        parse(&buf, &mut pending, &mut overflow);
        assert!(overflow, "{buf:?}");
    }
}

#[test]
fn changes_honour_the_dot_hidden_list_and_editing_it_rescans() {
    let t = Temp::new("notify-dothidden");
    std::fs::write(t.path().join(".hidden"), b"secret\n").unwrap();
    let (n, rx) = notify();
    n.watch(4, t.path()).unwrap();
    t.file("secret", 1);
    t.file("plain", 1);
    let (last, rescan) = settle(&rx, 4);
    assert!(!rescan);
    assert_ne!(last[&b"secret"[..]].meta.unwrap().attrs & flit_core::listing::attr::HIDDEN, 0);
    assert_eq!(last[&b"plain"[..]].meta.unwrap().attrs & flit_core::listing::attr::HIDDEN, 0);
    std::fs::write(t.path().join(".hidden"), b"secret\nplain\n").unwrap();
    let (_, rescan) = settle(&rx, 4);
    assert!(rescan);
}

#[test]
fn launcher_names_follow_renames_new_launchers_and_trust_changes() {
    use std::os::unix::fs::PermissionsExt;
    let t = Temp::new("notify-launchers");
    let launcher = |name: &str, mode: u32| {
        let path = t.path().join(name);
        std::fs::write(&path, "[Desktop Entry]\nType=Application\nName=Firefox\nExec=firefox\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
    };
    launcher("firefox.desktop", 0o700);
    let (n, rx) = notify();
    n.watch(1, t.path()).unwrap();
    std::fs::rename(t.path().join("firefox.desktop"), t.path().join("web.desktop")).unwrap();
    launcher("untrusted.desktop", 0o600);
    launcher("later.desktop", 0o600);
    std::fs::set_permissions(t.path().join("later.desktop"), std::fs::Permissions::from_mode(0o700)).unwrap();
    t.file("plain.txt", 3);
    let (last, rescan) = settle(&rx, 1);
    assert!(!rescan);
    assert_eq!(last[&b"web.desktop"[..]].label, Label::Set("Firefox".into()));
    assert_eq!(last[&b"firefox.desktop"[..]].kind, ChangeKind::Removed);
    assert_eq!(last[&b"untrusted.desktop"[..]].label, Label::Clear);
    assert_eq!(last[&b"later.desktop"[..]].label, Label::Set("Firefox".into()));
    assert_eq!(last[&b"plain.txt"[..]].label, Label::Keep);
    std::fs::set_permissions(t.path().join("web.desktop"), std::fs::Permissions::from_mode(0o600)).unwrap();
    let (last, _) = settle(&rx, 1);
    assert_eq!(last[&b"web.desktop"[..]].label, Label::Clear, "a launcher that is no longer trusted loses its name");
}
