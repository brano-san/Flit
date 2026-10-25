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
