use super::*;
use flit_core::ui::SearchSort;
use flit_core::Meta;
use flit_proto::listing::EntryBatch;
use flit_core::tags::TagCell;

#[test]
fn requests_map_to_protocol_messages() {
    let q = SearchRequest::Query { gen: 3, text: "a b".into(), conditions: Vec::new(), sort: SearchSort { key: SearchKey::Folder, descending: true }, scope: SearchScope::All, paths: Default::default() };
    assert_eq!(to_message(&q), Some(Message::Query { gen: 3, text: "a b".into(), conditions: Vec::new(), sort: Sort { key: SortKey::Path, descending: true }, paths: Default::default() }));
    let docs = flit_core::Location::parse(r"C:\Users\Jo Doe").unwrap();
    let q = SearchRequest::Query { gen: 4, text: "x".into(), conditions: Vec::new(), sort: SearchSort::default(), scope: SearchScope::Indexed(docs), paths: Default::default() };
    assert!(matches!(to_message(&q), Some(Message::Query { text, conditions, paths, .. })
        if text == "x" && conditions.is_empty() && paths.under.as_ref().unwrap().as_bytes() == br"C:\Users\Jo Doe"));
    let root = flit_core::Location::parse(r"D:\").unwrap();
    let q = SearchRequest::Query { gen: 5, text: String::new(), conditions: Vec::new(), sort: SearchSort::default(), scope: SearchScope::Indexed(root), paths: Default::default() };
    assert!(matches!(to_message(&q), Some(Message::Query { text, conditions, paths, .. })
        if text.is_empty() && conditions.is_empty() && paths.under.as_ref().unwrap().as_bytes() == br"D:\"));
    assert_eq!(to_message(&SearchRequest::Rows { gen: 3, from: 64, to: 128 }), Some(Message::GetRows { gen: 3, from: 64, to: 128 }));
    assert_eq!(to_message(&SearchRequest::Close), None);
}

#[test]
fn replies_are_validated_before_reaching_the_ui() {
    let mut ok = EntryBatch::default();
    ok.push(b"a.txt", Meta { size: 1, ..Default::default() });
    let ev = to_event(Message::Rows { gen: 1, from: 5, entries: ok.clone(), dirs: vec![br"C:\d".to_vec()] }, SearchBackend::Service, &None);
    assert_eq!(ev, Some(SearchEvent::Rows { gen: 1, from: 5, rows: vec![SearchRow { name: b"a.txt".to_vec(), dir: br"C:\d".to_vec(), meta: Meta { size: 1, ..Default::default() } }] }));
    let missing = to_event(Message::Rows { gen: 1, from: 0, entries: ok, dirs: Vec::new() }, SearchBackend::Service, &None);
    assert!(matches!(missing, Some(SearchEvent::Error { .. })));
    let mut sneaky = EntryBatch::default();
    sneaky.push(br"..\evil", Meta::default());
    let bad = to_event(Message::Rows { gen: 1, from: 0, entries: sneaky, dirs: vec![br"C:\".to_vec()] }, SearchBackend::Service, &None);
    assert!(matches!(bad, Some(SearchEvent::Error { message }) if message.contains("invalid")));
    let mut internal = EntryBatch::default();
    internal.push(b"x", Meta { attrs: flit_core::attr::DELETED, ..Default::default() });
    assert!(matches!(to_event(Message::Rows { gen: 1, from: 0, entries: internal, dirs: vec![b"C:".to_vec()] }, SearchBackend::Service, &None), Some(SearchEvent::Error { .. })));
}

#[test]
fn status_and_errors_translate() {
    let volumes = vec![
        VolumeInfo { id: "C:".into(), label: String::new(), file_system: "NTFS".into(), files: 9, state: VolumeState::Scanning { progress: 0.25 } },
        VolumeInfo { id: "D:".into(), label: String::new(), file_system: "FAT32".into(), files: 0, state: VolumeState::Unsupported },
    ];
    let note = Some("flit-indexd is not reachable".to_string());
    let Some(SearchEvent::Status { backend, volumes, note: n }) = to_event(Message::IndexStatus { volumes }, SearchBackend::Fallback, &note) else { panic!() };
    assert_eq!(backend, SearchBackend::Fallback);
    assert_eq!(n, note);
    assert_eq!(volumes[0], IndexVolume { id: "C:".into(), files: 9, state: IndexState::Scanning(0.25) });
    assert!(matches!(volumes[1].state, IndexState::Error(_)));
    assert_eq!(to_event(Message::QueryDone { gen: 2, total: 7, version: 0 }, SearchBackend::Service, &None), Some(SearchEvent::Done { gen: 2, total: 7, version: 0 }));
    assert_eq!(to_event(Message::GetRows { gen: 1, from: 0, to: 1 }, SearchBackend::Service, &None), None);
}

fn tag_cell(files: &[&str]) -> TagCell {
    let mut store = flit_core::tags::TagStore::default();
    let mut seen = flit_core::hash::FxHashMap::default();
    for (k, p) in files.iter().enumerate() {
        let key = flit_core::tags::FileKey { volume: 1, id: k as u64 };
        store.update(key, flit_core::FsPath::parse(p).unwrap(), |_| 1);
        seen.insert(key, (Meta::default(), true));
    }
    let cell = TagCell::default();
    cell.set(Arc::new(flit_core::tags::Tags::new(files.len() as u64, &store, &seen)));
    cell
}

#[test]
fn tag_queries_are_answered_from_the_tag_store_and_follow_its_changes() {
    let (me, rx) = unbounded();
    let (out, events) = unbounded();
    let cell = tag_cell(&[r"C:\a.txt"]);
    let mut w = Worker::<crate::P>::new(rx, me, out, Arc::new(|| {}), 0, cell.clone());
    let latest = Arc::new(AtomicU64::new(1));
    let got = |events: &Receiver<(WindowId, SearchEvent)>| events.try_iter().map(|(_, e)| e).collect::<Vec<_>>();
    w.request(7, latest.clone(), SearchRequest::Query { gen: 1, text: "tag:important".into(), conditions: Vec::new(), sort: SearchSort::default(), scope: SearchScope::All, paths: Default::default() });
    let row = SearchRow { name: b"a.txt".to_vec(), dir: br"C:\".to_vec(), meta: Meta::default() };
    assert_eq!(got(&events), [SearchEvent::Done { gen: 1, total: 1, version: 1 }, SearchEvent::Rows { gen: 1, from: 0, rows: vec![row.clone()] }]);
    w.request(7, latest.clone(), SearchRequest::Rows { gen: 1, from: 0, to: 64 });
    assert_eq!(got(&events), [SearchEvent::Rows { gen: 1, from: 0, rows: vec![row] }]);
    assert!(w.links.is_empty(), "tag queries never reach the index service");
    cell.set(tag_cell(&[r"C:\a.txt", r"C:\b.txt"]).get());
    w.tick();
    assert_eq!(got(&events)[0], SearchEvent::Done { gen: 1, total: 2, version: 2 });
    latest.store(2, Ordering::Release);
    w.request(7, latest.clone(), SearchRequest::Query { gen: 2, text: "tag:Nope".into(), conditions: Vec::new(), sort: SearchSort::default(), scope: SearchScope::All, paths: Default::default() });
    assert_eq!(got(&events), [SearchEvent::Error { message: "No tag named \"Nope\"".into() }, SearchEvent::Done { gen: 2, total: 0, version: 0 }]);
    w.request(7, latest, SearchRequest::Close);
    assert!(w.hits.is_empty() && w.links.is_empty());
}

#[test]
fn tag_queries_in_a_folder_keep_only_its_files() {
    let (me, rx) = unbounded();
    let (out, events) = unbounded();
    let cell = tag_cell(&[r"C:\Docs\a.txt", r"C:\Docs\Sub\b.txt", r"C:\Other\c.txt"]);
    let mut w = Worker::<crate::P>::new(rx, me, out, Arc::new(|| {}), 0, cell);
    let docs = flit_core::Location::parse(r"C:\Docs").unwrap();
    let by_folder = SearchSort { key: SearchKey::Folder, descending: false };
    w.request(1, Arc::new(AtomicU64::new(1)), SearchRequest::Query { gen: 1, text: "tag:".into(), conditions: Vec::new(), sort: by_folder, scope: SearchScope::Indexed(docs), paths: Default::default() });
    let got: Vec<SearchEvent> = events.try_iter().map(|(_, e)| e).collect();
    assert_eq!(got[0], SearchEvent::Done { gen: 1, total: 2, version: 3 });
    assert_eq!(got[1], SearchEvent::Groups { gen: 1, starts: vec![0, 1] });
    assert!(matches!(&got[2], SearchEvent::Rows { rows, .. } if rows.len() == 2));
}

#[test]
fn indexing_off_answers_without_an_index() {
    let (me, rx) = unbounded();
    let (out, events) = unbounded();
    let mut w = Worker::<crate::P>::new(rx, me, out, Arc::new(|| {}), 0, TagCell::default());
    let latest = Arc::new(AtomicU64::new(1));
    w.request(2, latest.clone(), SearchRequest::Indexing(false));
    assert_eq!(events.try_iter().map(|(_, e)| e).collect::<Vec<_>>(), [SearchEvent::Status { backend: SearchBackend::Off, volumes: Vec::new(), note: None }]);
    w.request(2, latest.clone(), SearchRequest::Query { gen: 1, text: "report".into(), conditions: Vec::new(), sort: SearchSort::default(), scope: SearchScope::All, paths: Default::default() });
    assert_eq!(events.try_iter().map(|(_, e)| e).collect::<Vec<_>>().last(), Some(&SearchEvent::Done { gen: 1, total: 0, version: 0 }));
    assert!(w.links.is_empty() && w.local.is_none(), "no service connection and no in-process indexer");
    w.request(2, latest, SearchRequest::Indexing(false));
    assert!(events.try_iter().next().is_none(), "repeating the same setting does nothing");
}

#[test]
fn walk_scopes_stream_found_events_and_serve_rows() {
    let dir = crate::find::tests::Scratch::new("search", &["one.txt", "sub/two.txt", "sub/skip.md"]);
    let (me, rx) = unbounded();
    let (out, events) = unbounded();
    let mut w = Worker::<crate::P>::new(rx, me, out, Arc::new(|| {}), 0, TagCell::default());
    let folder = flit_core::Location::from_local(dir.path());
    let latest = Arc::new(AtomicU64::new(1));
    w.request(3, latest.clone(), SearchRequest::Query { gen: 1, text: "ext:txt".into(), conditions: Vec::new(), sort: SearchSort::default(), scope: SearchScope::Walk(folder), paths: Default::default() });
    assert!(w.links.is_empty(), "walks never reach the index");
    let Ok(Cmd::Finished(3, 1)) = w.rx.recv_timeout(Duration::from_secs(10)) else { panic!("the walk did not finish") };
    w.publish(3, 1);
    let got: Vec<SearchEvent> = events.try_iter().map(|(_, e)| e).collect();
    assert_eq!(got[0], SearchEvent::Found { gen: 1, total: 2, folders: 2, done: true, source: SearchSource::Walk });
    let SearchEvent::Rows { rows, .. } = &got[1] else { panic!("{got:?}") };
    assert_eq!(rows.iter().map(|r| r.name.as_slice()).collect::<Vec<_>>(), [b"one.txt".as_slice(), b"two.txt"]);
    w.request(3, latest.clone(), SearchRequest::Rows { gen: 1, from: 1, to: 64 });
    assert!(matches!(&events.try_iter().map(|(_, e)| e).collect::<Vec<_>>()[..], [SearchEvent::Rows { from: 1, rows, .. }] if rows.len() == 1));
    assert!(!w.hits[&3].running());
}

#[test]
fn unix_folders_scope_queries_with_slashes() {
    let scoped = |folder: &str| {
        let q = SearchRequest::Query { gen: 1, text: "x".into(), conditions: Vec::new(), sort: SearchSort::default(), scope: SearchScope::Indexed(flit_core::Location::parse(folder).unwrap()), paths: Default::default() };
        match to_message(&q) {
            Some(Message::Query { text, paths, .. }) if text == "x" => paths.under.unwrap(),
            other => panic!("{other:?}"),
        }
    };
    assert_eq!(scoped("/home/jo doe").as_bytes(), b"/home/jo doe");
    assert_eq!(scoped("/").as_bytes(), b"/");
    assert_eq!(scoped(r#"/home/has"quotes"#).as_bytes(), br#"/home/has"quotes"#);
}

#[test]
fn w3_review_regex_at_the_end_of_an_or_query_keeps_the_indexed_folder_scope() {
    let dir = flit_core::Location::parse(r"C:\Docs").unwrap();
    let q = SearchRequest::Query {
        gen: 1, text: "other OR regex:^a.*b$".into(), conditions: vec!["size:>4".into()],
        sort: SearchSort::default(), scope: SearchScope::Indexed(dir), paths: Default::default(),
    };
    let Some(Message::Query { text, conditions, paths, .. }) = to_message(&q) else { panic!() };
    let mut q = query::parse_with_conditions(&text, &conditions, Clock::system(0)).unwrap();
    q.paths = paths;
    for (name, path, size, matches) in [
        ("axb", r"C:\Docs\axb", 5, true),
        ("other", r"C:\Docs\Other", 5, true),
        ("axb", r"C:\Docs2\axb", 5, false),
        ("other", r"D:\Docs\Other", 5, false),
        ("axb", r"C:\Docs\axb", 4, false),
    ] {
        let entry = query::Plain { name: name.as_bytes(), lower_name: name.as_bytes(), path: path.as_bytes(),
            lower_path: &lowercase(path.as_bytes()), size, modified: 0, dir: false };
        assert_eq!(q.matches(&entry, &|_| false), matches, "{path} ({size})");
    }
    assert_eq!(query::parse_with_conditions(" é regex:(", &conditions, Clock::system(0)).unwrap_err().position, 4);
}

#[cfg(target_os = "linux")]
mod service {
    use super::*;
    use flit_platform_linux::sock::{self, UnixConn};
    use flit_platform_linux::WalkIndex;
    use flit_proto::transport::{read_frame, read_hello};
    use std::path::{Path, PathBuf};

    struct Dir(PathBuf);

    impl Dir {
        fn new(tag: &str) -> Self {
            let root = std::env::temp_dir().join(format!("flit-svc-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(root.join("files")).unwrap();
            std::fs::write(root.join("files/report.txt"), b"x").unwrap();
            Self(root)
        }

        fn socket(&self) -> PathBuf {
            self.0.join(sock::SOCKET)
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    enum Mode {
        Answer,
        CrashAfterQuery,
    }

    fn fake_service(path: &Path, mode: Mode) {
        let listener = sock::bind_at(path, sock::uid()).unwrap();
        let mode = Arc::new(mode);
        std::thread::spawn(move || {
            sock::serve_on(
                listener,
                sock::uid(),
                4,
                Arc::new(move |mut conn: UnixConn| {
                    let hello = Hello::new(Protocol::Index, VERSION);
                    if read_hello(&mut conn, &hello).is_err() || write_frame(&mut conn, &hello).is_err() {
                        return;
                    }
                    let mut buf = Vec::new();
                    while let Ok(Message::Query { gen, .. }) = read_frame::<_, Message>(&mut conn, &mut buf) {
                        match *mode {
                            Mode::Answer => {
                                let _ = write_frame(&mut conn, &Message::QueryDone { gen, total: 42, version: 0 });
                            }
                            Mode::CrashAfterQuery => return,
                        }
                    }
                }),
            )
        });
    }

    fn worker(dir: &Dir) -> (Worker<crate::P>, Receiver<(WindowId, SearchEvent)>) {
        let (me, rx) = unbounded();
        let (out, events) = unbounded();
        let mut w = Worker::<crate::P>::new(rx, me, out, Arc::new(|| {}), 0, TagCell::default());
        let socket = dir.socket();
        w.connect = Box::new(move || sock::connect_at(&socket, sock::uid(), sock::CONNECT_TIMEOUT));
        let files = dir.0.join("files");
        w.fallback = Box::new(move || Ok((WalkIndex::with_roots(vec![("files".into(), files.clone(), "ext4".into())]), None)));
        (w, events)
    }

    fn query(w: &mut Worker<crate::P>, latest: &Arc<AtomicU64>) {
        w.request(1, latest.clone(), SearchRequest::Query { gen: 1, text: "report".into(), conditions: Vec::new(), sort: SearchSort::default(), scope: SearchScope::All, paths: Default::default() });
    }

    fn status(got: &[SearchEvent]) -> Option<(SearchBackend, Option<String>)> {
        got.iter().find_map(|e| match e {
            SearchEvent::Status { backend, note, .. } => Some((*backend, note.clone())),
            _ => None,
        })
    }

    fn local_answer(w: &mut Worker<crate::P>, events: &Receiver<(WindowId, SearchEvent)>, got: &mut Vec<SearchEvent>) -> bool {
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            w.tick();
            got.extend(events.try_iter().map(|(_, e)| e));
            if got.iter().any(|e| matches!(e, SearchEvent::Done { gen: 1, total: 1, .. })) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    }

    #[test]
    fn a_running_service_answers_the_search_without_an_index_in_flit() {
        let dir = Dir::new("answer");
        fake_service(&dir.socket(), Mode::Answer);
        let (mut w, events) = worker(&dir);
        let latest = Arc::new(AtomicU64::new(1));
        query(&mut w, &latest);
        let mut got: Vec<SearchEvent> = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !got.contains(&SearchEvent::Done { gen: 1, total: 42, version: 0 }) && Instant::now() < deadline {
            if let Ok((_, e)) = events.recv_timeout(Duration::from_millis(100)) {
                got.push(e);
            }
        }
        assert_eq!(status(&got), Some((SearchBackend::Service, None)), "{got:?}");
        assert!(got.contains(&SearchEvent::Done { gen: 1, total: 42, version: 0 }), "{got:?}");
        assert!(w.local.is_none(), "no second index runs inside Flit");
    }

    #[test]
    fn a_service_that_dies_mid_search_hands_the_search_to_the_index_in_flit() {
        let dir = Dir::new("crash");
        fake_service(&dir.socket(), Mode::CrashAfterQuery);
        let (mut w, events) = worker(&dir);
        let latest = Arc::new(AtomicU64::new(1));
        query(&mut w, &latest);
        assert_eq!(status(&events.try_iter().map(|(_, e)| e).collect::<Vec<_>>()), Some((SearchBackend::Service, None)));
        let Ok(Cmd::Lost(window, why)) = w.rx.recv_timeout(Duration::from_secs(10)) else { panic!("the dead service was not noticed") };
        w.lost(window, why);
        let mut got: Vec<SearchEvent> = events.try_iter().map(|(_, e)| e).collect();
        let (backend, note) = status(&got).unwrap();
        assert_eq!(backend, SearchBackend::Fallback);
        assert!(note.is_some_and(|n| n.contains("closed")));
        assert!(local_answer(&mut w, &events, &mut got), "the same query is answered by the index in Flit: {got:?}");
    }

    #[test]
    fn without_a_service_search_works_as_before() {
        let dir = Dir::new("absent");
        let (mut w, events) = worker(&dir);
        let latest = Arc::new(AtomicU64::new(1));
        let started = Instant::now();
        query(&mut w, &latest);
        assert!(started.elapsed() < Duration::from_secs(1), "a missing service costs no waiting");
        let mut got: Vec<SearchEvent> = events.try_iter().map(|(_, e)| e).collect();
        let (backend, note) = status(&got).unwrap();
        assert_eq!(backend, SearchBackend::Fallback);
        assert!(note.is_some_and(|n| n.contains("not reachable")));
        assert!(local_answer(&mut w, &events, &mut got), "{got:?}");
    }

    #[test]
    fn a_socket_others_may_open_is_ignored() {
        let dir = Dir::new("open");
        fake_service(&dir.socket(), Mode::Answer);
        std::fs::set_permissions(dir.socket().parent().unwrap(), std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let (mut w, events) = worker(&dir);
        let latest = Arc::new(AtomicU64::new(1));
        query(&mut w, &latest);
        let mut got: Vec<SearchEvent> = events.try_iter().map(|(_, e)| e).collect();
        assert_eq!(status(&got).map(|s| s.0), Some(SearchBackend::Fallback));
        assert!(local_answer(&mut w, &events, &mut got), "{got:?}");
    }

    #[test]
    fn a_service_that_never_greets_is_given_up_after_the_hello_timeout() {
        let dir = Dir::new("mute");
        let _listener = sock::bind_at(&dir.socket(), sock::uid()).unwrap();
        let (mut w, events) = worker(&dir);
        let latest = Arc::new(AtomicU64::new(1));
        let started = Instant::now();
        query(&mut w, &latest);
        let waited = started.elapsed();
        assert!(waited >= HELLO_TIMEOUT && waited < HELLO_TIMEOUT * 3, "{waited:?}");
        let mut got: Vec<SearchEvent> = events.try_iter().map(|(_, e)| e).collect();
        assert_eq!(status(&got).map(|s| s.0), Some(SearchBackend::Fallback));
        assert!(local_answer(&mut w, &events, &mut got), "{got:?}");
    }

    fn query_gen(w: &mut Worker<crate::P>, latest: &Arc<AtomicU64>, gen: u64) {
        latest.store(gen, Ordering::Release);
        w.request(1, latest.clone(), SearchRequest::Query { gen, text: "report".into(), conditions: Vec::new(), sort: SearchSort::default(), scope: SearchScope::All, paths: Default::default() });
    }

    fn counted(w: &mut Worker<crate::P>, dir: &Dir) -> Arc<std::sync::atomic::AtomicUsize> {
        let tries = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (socket, n) = (dir.socket(), tries.clone());
        w.connect = Box::new(move || {
            n.fetch_add(1, Ordering::AcqRel);
            sock::connect_at(&socket, sock::uid(), sock::CONNECT_TIMEOUT)
        });
        tries
    }

    #[test]
    fn a_service_started_later_takes_over_at_the_next_search_after_the_pause() {
        let dir = Dir::new("later");
        let (mut w, events) = worker(&dir);
        w.retry = Duration::ZERO;
        let latest = Arc::new(AtomicU64::new(1));
        query_gen(&mut w, &latest, 1);
        assert_eq!(status(&events.try_iter().map(|(_, e)| e).collect::<Vec<_>>()).map(|s| s.0), Some(SearchBackend::Fallback));
        fake_service(&dir.socket(), Mode::Answer);
        query_gen(&mut w, &latest, 2);
        let mut got: Vec<SearchEvent> = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !got.contains(&SearchEvent::Done { gen: 2, total: 42, version: 0 }) && Instant::now() < deadline {
            if let Ok((_, e)) = events.recv_timeout(Duration::from_millis(100)) {
                got.push(e);
            }
        }
        assert_eq!(status(&got), Some((SearchBackend::Service, None)), "{got:?}");
        assert!(got.contains(&SearchEvent::Done { gen: 2, total: 42, version: 0 }), "{got:?}");
        assert!(w.remote_failed.is_none());
    }

    #[test]
    fn a_failed_service_is_not_asked_again_before_the_pause() {
        let dir = Dir::new("pause");
        let (mut w, events) = worker(&dir);
        let tries = counted(&mut w, &dir);
        let latest = Arc::new(AtomicU64::new(1));
        for gen in 1..=3 {
            query_gen(&mut w, &latest, gen);
        }
        assert_eq!(tries.load(Ordering::Acquire), 1, "one attempt per minute at most");
        assert!(events.try_iter().all(|(_, e)| !matches!(e, SearchEvent::Status { backend: SearchBackend::Service, .. })));
        w.retry = Duration::ZERO;
        w.request(1, latest.clone(), SearchRequest::Rows { gen: 3, from: 0, to: 64 });
        assert_eq!(tries.load(Ordering::Acquire), 1, "page requests never wait for the service");
        query_gen(&mut w, &latest, 4);
        assert_eq!(tries.load(Ordering::Acquire), 2, "a new search after the pause asks again");
    }
}
