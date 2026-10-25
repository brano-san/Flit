use super::*;
use flit_core::ui::SearchSort;
use flit_core::Meta;
use flit_proto::listing::EntryBatch;
use flit_core::tags::TagCell;

#[test]
fn requests_map_to_protocol_messages() {
    let q = SearchRequest::Query { gen: 3, text: "a b".into(), sort: SearchSort { key: SearchKey::Folder, descending: true } };
    assert_eq!(to_message(&q), Some(Message::Query { gen: 3, text: "a b".into(), sort: Sort { key: SortKey::Path, descending: true } }));
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
    let mut w = Worker::<crate::P> { rx, me, out, wake: Arc::new(|| {}), utc_offset_min: 0, remote_failed: None, local: None, links: HashMap::new(), last_query: HashMap::new(), latests: HashMap::new(), tags: cell.clone(), tag_hits: HashMap::new() };
    let latest = Arc::new(AtomicU64::new(1));
    let got = |events: &Receiver<(WindowId, SearchEvent)>| events.try_iter().map(|(_, e)| e).collect::<Vec<_>>();
    w.request(7, latest.clone(), SearchRequest::Query { gen: 1, text: "tag:important".into(), sort: SearchSort::default() });
    let row = SearchRow { name: b"a.txt".to_vec(), dir: br"C:\".to_vec(), meta: Meta::default() };
    assert_eq!(got(&events), [SearchEvent::Done { gen: 1, total: 1 }, SearchEvent::Rows { gen: 1, from: 0, rows: vec![row.clone()] }]);
    w.request(7, latest.clone(), SearchRequest::Rows { gen: 1, from: 0, to: 64 });
    assert_eq!(got(&events), [SearchEvent::Rows { gen: 1, from: 0, rows: vec![row] }]);
    assert!(w.links.is_empty(), "tag queries never reach the index service");
    cell.set(tag_cell(&[r"C:\a.txt", r"C:\b.txt"]).get());
    w.tick();
    assert_eq!(got(&events)[0], SearchEvent::Done { gen: 1, total: 2 });
    latest.store(2, Ordering::Release);
    w.request(7, latest.clone(), SearchRequest::Query { gen: 2, text: "tag:Nope".into(), sort: SearchSort::default() });
    assert_eq!(got(&events), [SearchEvent::Error { message: "No tag named \"Nope\"".into() }, SearchEvent::Done { gen: 2, total: 0 }]);
    w.request(7, latest, SearchRequest::Close);
    assert!(w.tag_hits.is_empty() && w.links.is_empty());
}
