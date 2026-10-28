use super::*;
use crate::temp::Temp;
use crossbeam_channel::Receiver;
use flit_core::listing::ChangeKind;
use flit_core::Meta;
use flit_proto::transport::write_frame;
use std::collections::HashMap;
use std::io::PipeWriter;
use std::path::Path;

struct Client {
    to: PipeWriter,
    from: Receiver<Event>,
    done: Receiver<Result<(), ProtoError>>,
}

impl Client {
    fn start(trash: Option<Trash>) -> Self {
        let (req_r, mut req_w) = std::io::pipe().unwrap();
        let (mut ev_r, ev_w) = std::io::pipe().unwrap();
        let (done_tx, done) = unbounded();
        std::thread::spawn(move || done_tx.send(serve(req_r, ev_w, "test-build".into(), trash)).unwrap());
        let theirs = read_hello(&mut ev_r, &hello(String::new())).unwrap();
        assert_eq!(theirs.build, "test-build");
        write_frame(&mut req_w, &hello("client".into())).unwrap();
        let (tx, from) = unbounded();
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            while let Ok(ev) = read_frame::<_, Event>(&mut ev_r, &mut buf) {
                ev.validate(false).unwrap();
                if tx.send(ev).is_err() {
                    return;
                }
            }
        });
        Self { to: req_w, from, done }
    }

    fn send(&mut self, r: Request) {
        write_frame(&mut self.to, &r).unwrap();
    }

    fn next(&self) -> Event {
        self.from.recv_timeout(Duration::from_secs(5)).expect("agent answers")
    }

    fn list(&mut self, id: ReqId, dir: &Path) -> (HashMap<Vec<u8>, Meta>, usize, Option<FsError>) {
        self.send(Request::List { id, path: fs_path(dir) });
        let (mut names, mut batches) = (HashMap::new(), 0);
        loop {
            match self.next() {
                Event::Entries { id: got, batch } if got == id => {
                    batches += 1;
                    names.extend(batch.entries(false).unwrap().map(|(n, m)| (n.to_vec(), m)));
                }
                Event::ListDone { id: got, error } if got == id => return (names, batches, error),
                other => panic!("unexpected {other:?}"),
            }
        }
    }
}

fn fs_path(p: &Path) -> FsPath {
    FsPath::parse_bytes(p.as_os_str().as_bytes()).unwrap()
}

#[test]
fn lists_exact_linux_names_with_metadata() {
    let t = Temp::new("agent-list");
    t.file("a:b", 4);
    t.file(".dot", 0);
    std::fs::write(t.path().join(OsStr::from_bytes(b"x\\y\xFF")), b"z").unwrap();
    std::fs::create_dir(t.path().join("dir")).unwrap();
    let mut c = Client::start(None);
    let (names, _, error) = c.list(1, t.path());
    assert_eq!(error, None);
    assert_eq!(names.len(), 4);
    assert_eq!(names[&b"a:b"[..]].size, 4);
    assert_eq!(names[&b".dot"[..]].attrs, flit_core::attr::HIDDEN);
    assert_eq!(names[&b"x\\y\xFF"[..]].size, 1);
    assert_eq!(names[&b"dir"[..]].attrs, flit_core::attr::DIR);
    let (_, _, error) = c.list(2, &t.path().join("missing"));
    assert_eq!(error.unwrap().kind, FsErrorKind::NotFound);
    c.send(Request::List { id: 3, path: FsPath::parse(r"C:\Windows").unwrap() });
    assert_eq!(c.next(), Event::ListDone { id: 3, error: Some(UNAVAILABLE) });
}

#[test]
fn large_folders_stream_in_batches() {
    let t = Temp::new("agent-big");
    for i in 0..10_000 {
        std::fs::File::create(t.path().join(format!("file-{i:05}"))).unwrap();
    }
    let mut c = Client::start(None);
    let (names, batches, error) = c.list(7, t.path());
    assert_eq!((names.len(), error), (10_000, None));
    assert!(batches >= 3, "{batches}");
}

#[test]
fn stat_watch_and_cancel() {
    let t = Temp::new("agent-watch");
    t.file("f", 9);
    let mut c = Client::start(None);
    c.send(Request::Stat { id: 1, path: fs_path(&t.path().join("f")) });
    assert!(matches!(c.next(), Event::Stat { id: 1, result: Ok(Meta { size: 9, .. }) }));
    c.send(Request::Stat { id: 2, path: fs_path(&t.path().join("nope")) });
    assert!(matches!(c.next(), Event::Stat { id: 2, result: Err(FsError { kind: FsErrorKind::NotFound, .. }) }));
    c.send(Request::Watch { id: 3, path: fs_path(t.path()) });
    c.send(Request::Stat { id: 4, path: fs_path(t.path()) });
    assert!(matches!(c.next(), Event::Stat { id: 4, .. }));
    t.file("new", 2);
    match c.next() {
        Event::Changes { id: 3, changes } => assert!(changes.iter().any(|ch| ch.name == b"new" && ch.kind == ChangeKind::Added && ch.meta.unwrap().size == 2)),
        other => panic!("unexpected {other:?}"),
    }
    c.send(Request::Cancel { id: 3 });
    c.send(Request::Stat { id: 5, path: fs_path(t.path()) });
    assert!(matches!(c.next(), Event::Stat { id: 5, .. }));
    t.file("later", 1);
    assert!(c.from.recv_timeout(Duration::from_millis(300)).is_err());
}

#[test]
fn folder_sizes_refuse_forged_names() {
    let t = Temp::new("agent-sizes");
    std::fs::create_dir_all(t.path().join("a/b")).unwrap();
    t.file("a/b/x", 1000);
    t.file("a/y", 24);
    let mut c = Client::start(None);
    c.send(Request::DirSizes { id: 9, dir: fs_path(t.path()), items: vec![(3, b"a".to_vec()), (4, b"../etc".to_vec()), (5, b"..".to_vec())] });
    let mut got: Vec<(u32, u64)> = (0..3)
        .map(|_| match c.next() {
            Event::DirSize { id: 9, idx, stats } => (idx, stats.size),
            other => panic!("unexpected {other:?}"),
        })
        .collect();
    got.sort();
    assert_eq!(got, [(3, 1024), (4, 0), (5, 0)]);
}

impl Client {
    fn op(&mut self, id: ReqId, op: FileOp) {
        self.send(Request::Op { id, op, policy: None });
    }

    fn done(&self, id: ReqId) -> (Option<String>, Vec<(FsPath, FsPath)>) {
        loop {
            match self.next() {
                Event::OpProgress { id: got, .. } if got == id => {}
                Event::OpDone { id: got, error, items } if got == id => return (error, items),
                other => panic!("unexpected {other:?}"),
            }
        }
    }
}

#[test]
fn trash_operation_moves_to_the_freedesktop_trash_and_back() {
    let t = Temp::new("agent-trash");
    let victim = t.file("victim", 3);
    let mut c = Client::start(Some(Trash::new(t.path().join("data/Trash"), unsafe { libc::getuid() })));
    c.op(1, FileOp::Trash(vec![fs_path(&victim)]));
    let trashed = fs_path(&t.path().join("data/Trash/files/victim"));
    assert_eq!(c.done(1), (None, vec![(fs_path(&victim), trashed.clone())]));
    assert!(!victim.exists());
    c.op(2, FileOp::Trash(vec![fs_path(&t.path().join("gone")), fs_path(&t.path().join("gone2"))]));
    let (Some(e), items) = c.done(2) else { panic!() };
    assert!(e.starts_with("2 items failed. Cannot move") && items.is_empty(), "{e}");
    c.op(3, FileOp::Restore(vec![(trashed.clone(), fs_path(&victim))]));
    assert_eq!(c.done(3), (None, vec![(trashed, fs_path(&victim))]));
    assert!(victim.exists());
    c.op(4, FileOp::Delete(vec![fs_path(&t.path().join("data"))]));
    assert_eq!(c.done(4).0, None);
    assert!(!t.path().join("data").exists());
}

#[test]
fn conflicts_are_asked_over_the_channel_and_cancel_stops_the_operation() {
    let t = Temp::new("agent-ask");
    t.file("a", 1);
    std::fs::create_dir(t.path().join("dst")).unwrap();
    t.file("dst/a", 2);
    let mut c = Client::start(None);
    let copy = FileOp::Copy { items: vec![fs_path(&t.path().join("a"))], dest: fs_path(&t.path().join("dst")) };
    c.op(1, copy.clone());
    let conflict = loop {
        match c.next() {
            Event::OpProgress { .. } => {}
            Event::OpConflict { id: 1, conflict } => break conflict,
            other => panic!("unexpected {other:?}"),
        }
    };
    assert_eq!((conflict.target, conflict.to.size), (fs_path(&t.path().join("dst/a")), 2));
    c.send(Request::Answer { id: 1, choice: Some(flit_core::provider::Resolve::KeepBoth), all: false });
    assert_eq!(c.done(1), (None, vec![(fs_path(&t.path().join("a")), fs_path(&t.path().join("dst/a (2)")))]));
    c.op(2, copy);
    while !matches!(c.next(), Event::OpConflict { id: 2, .. }) {}
    c.send(Request::Cancel { id: 2 });
    let (error, items) = c.done(2);
    assert_eq!((error.as_deref(), items), (Some(flit_platform::CANCELLED), Vec::new()));
    c.send(Request::Op { id: 3, op: FileOp::Copy { items: vec![fs_path(&t.path().join("a"))], dest: fs_path(&t.path().join("dst")) }, policy: Some(flit_core::provider::Resolve::Skip) });
    assert_eq!(c.done(3), (None, Vec::new()), "a policy answers without asking");
}

#[test]
fn shutdown_eof_and_garbage_end_the_session() {
    let mut c = Client::start(None);
    c.send(Request::Shutdown);
    assert!(c.done.recv_timeout(Duration::from_secs(5)).unwrap().is_ok());
    let c = Client::start(None);
    drop(c.to);
    assert!(c.done.recv_timeout(Duration::from_secs(5)).unwrap().is_ok());
    let (req_r, mut req_w) = std::io::pipe().unwrap();
    let (_ev_r, ev_w) = std::io::pipe().unwrap();
    req_w.write_all("w\0s\0l\0:\0 \0".as_bytes()).unwrap();
    drop(req_w);
    assert!(matches!(serve(req_r, ev_w, String::new(), None), Err(ProtoError::NotFlit)));
}
