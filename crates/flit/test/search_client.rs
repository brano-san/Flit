use super::*;
use flit_core::ui::SearchSort;
use flit_core::Meta;
use flit_proto::listing::EntryBatch;
use flit_core::tags::TagCell;

#[test]
fn requests_map_to_protocol_messages() {
    let q = SearchRequest::Query { gen: 3, text: "a b".into(), sort: SearchSort { key: SearchKey::Folder, descending: true }, scope: SearchScope::All };
    assert_eq!(to_message(&q), Some(Message::Query { gen: 3, text: "a b".into(), sort: Sort { key: SortKey::Path, descending: true } }));
    let docs = flit_core::Location::parse(r"C:\Users\Jo Doe").unwrap();
    let q = SearchRequest::Query { gen: 4, text: "x".into(), sort: SearchSort::default(), scope: SearchScope::Indexed(docs) };
    assert!(matches!(to_message(&q), Some(Message::Query { text, .. }) if text == r#"x path:"C:\Users\Jo Doe\""#));
    let root = flit_core::Location::parse(r"D:\").unwrap();
    let q = SearchRequest::Query { gen: 5, text: String::new(), sort: SearchSort::default(), scope: SearchScope::Indexed(root) };
    assert!(matches!(to_message(&q), Some(Message::Query { text, .. }) if text == r#" path:"D:\""#));
    assert_eq!(query::parse(r#"x path:"C:\Users\Jo Doe\""#, Clock::system(0)).unwrap().terms.len(), 2);
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
    assert_eq!(to_event(Message::QueryDone { gen: 2, total: 7 }, SearchBackend::Service, &None), Some(SearchEvent::Done { gen: 2, total: 7 }));
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
    w.request(7, latest.clone(), SearchRequest::Query { gen: 1, text: "tag:important".into(), sort: SearchSort::default(), scope: SearchScope::All });
    let row = SearchRow { name: b"a.txt".to_vec(), dir: br"C:\".to_vec(), meta: Meta::default() };
    assert_eq!(got(&events), [SearchEvent::Done { gen: 1, total: 1 }, SearchEvent::Rows { gen: 1, from: 0, rows: vec![row.clone()] }]);
    w.request(7, latest.clone(), SearchRequest::Rows { gen: 1, from: 0, to: 64 });
    assert_eq!(got(&events), [SearchEvent::Rows { gen: 1, from: 0, rows: vec![row] }]);
    assert!(w.links.is_empty(), "tag queries never reach the index service");
    cell.set(tag_cell(&[r"C:\a.txt", r"C:\b.txt"]).get());
    w.tick();
    assert_eq!(got(&events)[0], SearchEvent::Done { gen: 1, total: 2 });
    latest.store(2, Ordering::Release);
    w.request(7, latest.clone(), SearchRequest::Query { gen: 2, text: "tag:Nope".into(), sort: SearchSort::default(), scope: SearchScope::All });
    assert_eq!(got(&events), [SearchEvent::Error { message: "No tag named \"Nope\"".into() }, SearchEvent::Done { gen: 2, total: 0 }]);
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
    w.request(1, Arc::new(AtomicU64::new(1)), SearchRequest::Query { gen: 1, text: "tag:".into(), sort: by_folder, scope: SearchScope::Indexed(docs) });
    let got: Vec<SearchEvent> = events.try_iter().map(|(_, e)| e).collect();
    assert_eq!(got[0], SearchEvent::Done { gen: 1, total: 2 });
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
    w.request(2, latest.clone(), SearchRequest::Query { gen: 1, text: "report".into(), sort: SearchSort::default(), scope: SearchScope::All });
    assert_eq!(events.try_iter().map(|(_, e)| e).collect::<Vec<_>>().last(), Some(&SearchEvent::Done { gen: 1, total: 0 }));
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
    w.request(3, latest.clone(), SearchRequest::Query { gen: 1, text: "ext:txt".into(), sort: SearchSort::default(), scope: SearchScope::Walk(folder) });
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
        let q = SearchRequest::Query { gen: 1, text: "x".into(), sort: SearchSort::default(), scope: SearchScope::Indexed(flit_core::Location::parse(folder).unwrap()) };
        match to_message(&q) {
            Some(Message::Query { text, .. }) => text,
            other => panic!("{other:?}"),
        }
    };
    assert_eq!(scoped("/home/jo doe"), r#"x path:"/home/jo doe/""#);
    assert_eq!(scoped("/"), r#"x path:"/""#);
}
