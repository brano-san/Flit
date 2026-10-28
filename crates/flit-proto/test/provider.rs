use super::*;
use crate::transport::{decode, encode};
use flit_core::listing::{ChangeKind, DirStats};
use flit_core::FsErrorKind;

fn unix(p: &[u8]) -> FsPath {
    FsPath::parse_bytes(p).unwrap()
}

#[test]
fn roundtrip_every_message() {
    let requests = vec![
        Request::List { id: 1, path: unix(b"/home/\xFFuser") },
        Request::Stat { id: 2, path: unix(b"/") },
        Request::Watch { id: 3, path: unix("/tmp/отчёт".as_bytes()) },
        Request::DirSizes { id: 4, dir: unix(b"/var"), items: vec![(0, b"log".to_vec()), (7, b"\xFE".to_vec())] },
        Request::Op { id: 5, op: FileOp::Trash(vec![unix(b"/tmp/a"), unix(b"/tmp/b:c")]), policy: None },
        Request::Op { id: 6, op: FileOp::Copy { items: vec![unix(b"/tmp/\xFF")], dest: unix(b"/home") }, policy: Some(Resolve::Replace) },
        Request::Answer { id: 6, choice: Some(Resolve::KeepBoth), all: true },
        Request::Answer { id: 6, choice: None, all: false },
        Request::Cancel { id: 1 },
        Request::Shutdown,
    ];
    for m in requests {
        let mut buf = Vec::new();
        encode(&m, &mut buf).unwrap();
        assert_eq!(decode::<Request>(&buf[4..]).unwrap(), m);
    }
    let mut batch = EntryBatch::default();
    batch.push(b"\xFF\xFE", Meta { size: 1, attrs: flit_core::attr::HIDDEN, ..Default::default() });
    let err = FsError { kind: FsErrorKind::AccessDenied, code: 13 };
    let events = vec![
        Event::Entries { id: 1, batch },
        Event::ListDone { id: 1, error: Some(err.clone()) },
        Event::Stat { id: 2, result: Ok(Meta { size: 9, modified: -5, created: 1, attrs: 0 }) },
        Event::Stat { id: 2, result: Err(err) },
        Event::Changes { id: 3, changes: vec![DirChange { kind: ChangeKind::Removed, name: b"x".to_vec(), meta: None }] },
        Event::Rescan { id: 3 },
        Event::DirSize { id: 4, idx: 7, stats: DirStats { size: u64::MAX, folders: 3, files: 9, partial: true } },
        Event::OpDone { id: 5, error: None, items: vec![(unix(b"/tmp/a"), unix(b"/home/u/.local/share/Trash/files/a"))] },
        Event::OpProgress { id: 6, progress: OpProgress { stage: flit_core::provider::Stage::Running, bytes: 1, total_bytes: 2, items: 3, total_items: 4 } },
        Event::OpConflict { id: 6, conflict: Conflict { source: unix(b"/tmp/\xFF"), target: unix(b"/home/\xFF"), from: Meta::default(), to: Meta { size: 5, ..Default::default() } } },
    ];
    for m in events {
        let mut buf = Vec::new();
        encode(&m, &mut buf).unwrap();
        assert_eq!(decode::<Event>(&buf[4..]).unwrap(), m);
    }
}

#[test]
fn forged_paths_rejected() {
    let mut buf = Vec::new();
    encode(&(0u8, 1u64, b"../../etc".to_vec()), &mut buf).unwrap();
    assert!(decode::<Request>(&buf[4..]).is_err());
}

#[test]
fn hostile_events_rejected() {
    let change = |name: &[u8], meta: Option<Meta>| Event::Changes { id: 1, changes: vec![DirChange { kind: ChangeKind::Added, name: name.to_vec(), meta }] };
    for evil in [&b".."[..], b".", b"", b"a/b", b"nul\0"] {
        assert!(change(evil, None).validate(false).is_err(), "{evil:?}");
    }
    for evil in [&br"..\x"[..], b"a:b"] {
        assert!(change(evil, None).validate(false).is_ok(), "{evil:?}");
        assert!(change(evil, None).validate(true).is_err(), "{evil:?}");
    }
    let tombstone = Meta { attrs: flit_core::attr::DELETED, ..Default::default() };
    assert!(change(b"ok", Some(tombstone)).validate(false).is_err());
    assert!(Event::Stat { id: 2, result: Ok(tombstone) }.validate(false).is_err());
    let mut batch = EntryBatch::default();
    batch.push(br"a\b", Meta::default());
    assert!(Event::Entries { id: 3, batch: batch.clone() }.validate(false).is_ok());
    assert!(Event::Entries { id: 3, batch }.validate(true).is_err());
    assert!(change(b"ok", Some(Meta { attrs: flit_core::attr::DIR | flit_core::attr::HIDDEN, ..Default::default() })).validate(true).is_ok());
    assert!(Event::Rescan { id: 4 }.validate(true).is_ok());
}

#[test]
fn reports_must_stay_inside_the_operation() {
    let pairs = |v: &[(&str, &str)]| v.iter().map(|&(a, b)| (unix(a.as_bytes()), FsPath::parse(b).unwrap())).collect::<Vec<_>>();
    let copy = FileOp::Copy { items: vec![unix(b"/home/u/a"), unix(b"/home/u/d")], dest: unix(b"/tmp/x") };
    assert!(check_report(&copy, &pairs(&[("/home/u/a", "/tmp/x/a - Copy"), ("/home/u/d/inner", "/tmp/x/d/inner")])).is_ok());
    assert!(check_report(&copy, &pairs(&[("/home/u/a", "/tmp/xy/a")])).is_err());
    assert!(check_report(&copy, &pairs(&[("/home/u/ab", "/tmp/x/ab")])).is_err());
    assert!(check_report(&copy, &pairs(&[("/home/u/a", r"C:\Windows")])).is_err());
    let trash = FileOp::Trash(vec![unix(b"/tmp/a")]);
    assert!(check_report(&trash, &pairs(&[("/tmp/a", "/home/u/.local/share/Trash/files/a.2")])).is_ok());
    assert!(check_report(&trash, &pairs(&[("/tmp/a", "/home/u/.ssh")])).is_err());
    let rename = FileOp::Rename { item: unix(b"/a/b"), name: b"c".to_vec() };
    assert!(check_report(&rename, &pairs(&[("/a/b", "/a/c")])).is_ok());
    assert!(check_report(&rename, &pairs(&[("/a/b", "/etc/c")])).is_err());
    let root = FileOp::NewFolder { dir: unix(b"/"), name: b"n".to_vec() };
    assert!(check_report(&root, &pairs(&[("/n", "/n")])).is_ok());
    let restore = FileOp::Restore(vec![(unix(b"/h/Trash/files/a"), unix(b"/tmp/a"))]);
    assert!(check_report(&restore, &pairs(&[("/h/Trash/files/a", "/tmp/a (2)")])).is_ok());
    assert!(check_report(&FileOp::EmptyBin, &[]).is_ok());
    assert!(check_report(&FileOp::EmptyBin, &pairs(&[("/a", "/b")])).is_err());
}

#[test]
fn conflicts_must_name_the_operation_paths() {
    let copy = FileOp::Copy { items: vec![unix(b"/s/a")], dest: unix(b"/d") };
    let c = |s: &str, t: &str| Conflict { source: unix(s.as_bytes()), target: unix(t.as_bytes()), from: Meta::default(), to: Meta::default() };
    assert!(check_conflict(&copy, &c("/s/a/x", "/d/a/x")).is_ok());
    assert!(check_conflict(&copy, &c("/s/b", "/d/b")).is_err());
    assert!(check_conflict(&copy, &c("/s/a", "/e/a")).is_err());
    assert!(check_conflict(&FileOp::Delete(vec![unix(b"/s/a")]), &c("/s/a", "/s/a")).is_err());
    let tomb = Event::OpConflict { id: 1, conflict: Conflict { to: Meta { attrs: flit_core::attr::DELETED, ..Default::default() }, ..c("/s/a", "/d/a") } };
    assert!(tomb.validate(false).is_err());
}
