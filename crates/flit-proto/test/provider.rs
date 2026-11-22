use super::*;
use crate::transport::{decode, encode};
use flit_core::listing::{ChangeKind, DirStats};
use flit_core::provider::PlatformErrorKind;
use flit_core::FsErrorKind;

#[test]
fn typed_links_are_kept_local_and_are_never_dispatched_to_an_old_agent() {
    let dest = FsPath::parse(r"C:\out").unwrap();
    for kind in [flit_core::provider::LinkKind::Symbolic, flit_core::provider::LinkKind::Junction, flit_core::provider::LinkKind::Hard] {
        for op in [FileOp::LinkAs { items: vec![dest.clone()], dest: dest.clone(), kind }, FileOp::PasteLinkAs { dest: dest.clone(), kind }] {
            assert!(!agent_runs(&op));
            assert!(check_report(&op, &[(dest.clone(), dest.clone())]).is_err());
        }
    }
}

fn unix(p: &[u8]) -> FsPath {
    FsPath::parse_bytes(p).unwrap()
}

#[test]
fn roundtrip_every_message() {
    let target = unix(b"/tmp/target");
    let replay = Replay {
        steps: vec![FileOp::Trash(vec![target.clone()])],
        checks: vec![(target, Some(flit_core::undo::Stamp { size: 1, modified: -10, device: 2, file: 3, access: None }))],
        recheck_tree: false,
        captured_ns: Some(1_234_567_890_123_456_789),
    };
    assert!(check_replay(&replay).is_ok());
    assert!(check_replay(&Replay { checks: Vec::new(), ..replay.clone() }).is_err());
    assert!(check_replay(&Replay { steps: vec![FileOp::Delete(vec![unix(b"/tmp/target")])], ..replay.clone() }).is_err());
    let requests = vec![
        Request::List { id: 1, path: unix(b"/home/\xFFuser") },
        Request::Stat { id: 2, path: unix(b"/") },
        Request::Watch { id: 3, path: unix("/tmp/отчёт".as_bytes()) },
        Request::DirSizes { id: 4, dir: unix(b"/var"), items: vec![(0, b"log".to_vec()), (7, b"\xFE".to_vec())] },
        Request::Op { id: 5, op: FileOp::Trash(vec![unix(b"/tmp/a"), unix(b"/tmp/b:c")]), policy: None },
        Request::Op { id: 6, op: FileOp::Copy { items: vec![unix(b"/tmp/\xFF")], dest: unix(b"/home") }, policy: Some(Resolve::Replace) },
        Request::Answer { id: 6, choice: Some(Resolve::KeepBoth), policy: Some(Resolve::ReplaceOlder) },
        Request::Answer { id: 6, choice: None, policy: None },
        Request::Cancel { id: 1 },
        Request::Shutdown,
        Request::Recover { id: 6, choice: Some(Recover::Retry), all: false },
        Request::Recover { id: 6, choice: Some(Recover::Skip), all: true },
        Request::Recover { id: 6, choice: None, all: false },
        Request::Pause { id: 6, paused: true },
        Request::CopyLimit { id: 6, megabytes_per_second: 12 },
        Request::Replay { id: 7, plan: replay.clone() },
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
        Event::Changes { id: 3, changes: vec![DirChange { label: flit_core::listing::Label::Keep, kind: ChangeKind::Removed, name: b"x".to_vec(), meta: None }] },
        Event::Rescan { id: 3 },
        Event::DirSize { id: 4, idx: 7, stats: DirStats { size: u64::MAX, folders: 3, files: 9, partial: true } },
        Event::OpDone { id: 5, error: None, items: vec![(unix(b"/tmp/a"), unix(b"/home/u/.local/share/Trash/files/a"))] },
        Event::OpProgress { id: 6, progress: OpProgress { stage: flit_core::provider::Stage::Running, bytes: 1, total_bytes: 2, items: 3, total_items: 4 } },
        Event::OpConflict { id: 6, conflict: Conflict { source: unix(b"/tmp/\xFF"), target: unix(b"/home/\xFF"), from: Meta::default(), to: Meta { size: 5, ..Default::default() } } },
        Event::OpFailure { id: 6, failure: Failure { path: unix(b"/tmp/\xFF"), error: OpError::new(PlatformErrorKind::AccessDenied, "Permission denied (os error 13)"), fatal: false } },
        Event::OpFailure { id: 6, failure: Failure { path: unix(b"/home"), error: "No space left on device".into(), fatal: true } },
        Event::OpUndo { id: 7, undo: Undo::Checked(replay) },
        Event::OpReplaced { id: 6, targets: vec![(unix(b"/home/a"), None)] },
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
    let change = |name: &[u8], meta: Option<Meta>| Event::Changes { id: 1, changes: vec![DirChange { label: flit_core::listing::Label::Keep, kind: ChangeKind::Added, name: name.to_vec(), meta }] };
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

#[test]
fn failures_must_name_the_operation_paths_and_stay_short() {
    let f = |p: &str| Failure { path: unix(p.as_bytes()), error: "Permission denied".into(), fatal: false };
    let copy = FileOp::Copy { items: vec![unix(b"/s/a")], dest: unix(b"/d") };
    assert!(check_failure(&copy, &f("/s/a/x")).is_ok());
    assert!(check_failure(&copy, &f("/d/a/x")).is_ok());
    assert!(check_failure(&copy, &f("/d")).is_ok());
    assert!(check_failure(&copy, &f("/etc/shadow")).is_err());
    let delete = FileOp::Delete(vec![unix(b"/s/a")]);
    assert!(check_failure(&delete, &f("/s/a/deep")).is_ok() && check_failure(&delete, &f("/s/b")).is_err());
    let restore = FileOp::Restore(vec![(unix(b"/h/Trash/files/a"), unix(b"/tmp/a"))]);
    assert!(check_failure(&restore, &f("/tmp/a")).is_ok());
    assert!(check_failure(&FileOp::Trash(vec![unix(b"/s/a")]), &f("/s/a")).is_err(), "the Trash reports at the end instead");
    assert!(check_failure(&FileOp::Rename { item: unix(b"/s/a"), name: b"b".to_vec() }, &f("/s/a")).is_err());
    let long = Event::OpFailure { id: 1, failure: Failure { error: "x".repeat(MAX_FAILURE_TEXT + 1).into(), ..f("/s/a") } };
    assert!(long.validate(false).is_err());
    assert!(Event::OpFailure { id: 1, failure: f("/s/a") }.validate(true).is_ok());
}

#[test]
fn dropped_file_payloads_never_go_to_the_agent() {
    let dest = unix(b"/tmp");
    let data = std::sync::Arc::new(flit_core::provider::DroppedFile { name: b"text.txt".to_vec(), bytes: b"hello".to_vec() });
    for data in [None, Some(data)] {
        assert!(!agent_runs(&FileOp::PasteDropped { dest: dest.clone(), moving: false, data }));
    }
}
