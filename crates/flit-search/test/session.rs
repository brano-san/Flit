use super::*;
use crate::index::{IndexRecord, IndexSink, VolumeIndex};
use crate::proto::{SortKey, VolumeState};
use flit_core::attr;

fn shared() -> Arc<SharedIndex> {
    let s = SharedIndex::new();
    let mut v = VolumeIndex::new("C:", b"C:\\", b'\\');
    v.upsert(&IndexRecord { id: 5, parent: 5, name: b".", size: 0, modified: 0, attrs: attr::DIR });
    for k in 0..100u64 {
        let name = format!("file{k:03}.txt");
        v.upsert(&IndexRecord { id: 100 + k, parent: 5, name: name.as_bytes(), size: k, modified: 0, attrs: 0 });
    }
    v.commit();
    let mut e = s.write();
    e.volumes.push(v);
    e.version = 1;
    drop(e);
    s
}

const SORT: Sort = Sort { key: SortKey::Name, descending: false };

fn run(session: &mut Session, latest: &AtomicU64, msg: Message) -> Vec<Message> {
    let mut out = Vec::new();
    session.handle(msg, latest, &mut |m| out.push(m));
    out
}

fn query(gen: u64, text: &str) -> Message {
    Message::Query { gen, text: text.into(), conditions: Vec::new(), sort: SORT, paths: Default::default() }
}

#[test]
fn query_answers_with_total_and_first_page() {
    let mut s = Session::new(shared(), 0);
    let latest = AtomicU64::new(1);
    let out = run(&mut s, &latest, query(1, "file0"));
    assert_eq!(out[0], Message::QueryDone { gen: 1, total: 100, version: 1 });
    let Message::Rows { gen: 1, from: 0, entries, dirs } = &out[1] else { panic!("{out:?}") };
    assert_eq!(entries.len() as u64, FIRST_PAGE);
    assert_eq!(dirs[0], br"C:\".to_vec());
    let out = run(&mut s, &latest, Message::GetRows { gen: 1, from: 98, to: 120 });
    let [Message::Rows { from: 98, entries, .. }] = out.as_slice() else { panic!("{out:?}") };
    assert_eq!(entries.entries(true).unwrap().map(|(n, _)| n.to_vec()).collect::<Vec<_>>(), [b"file098.txt".to_vec(), b"file099.txt".to_vec()]);
    assert!(run(&mut s, &latest, Message::GetRows { gen: 0, from: 0, to: 10 }).is_empty(), "stale gen");
}

#[test]
fn superseded_queries_are_skipped() {
    let mut s = Session::new(shared(), 0);
    let latest = AtomicU64::new(3);
    assert!(run(&mut s, &latest, query(2, "file")).is_empty());
    assert!(run(&mut s, &latest, Message::GetRows { gen: 2, from: 0, to: 10 }).is_empty());
    assert_eq!(run(&mut s, &latest, query(3, "file05"))[0], Message::QueryDone { gen: 3, total: 10, version: 1 });
}

#[test]
fn bad_queries_report_an_error_and_no_rows() {
    let mut s = Session::new(shared(), 0);
    let latest = AtomicU64::new(1);
    let out = run(&mut s, &latest, query(1, "regex:("));
    assert!(matches!(&out[0], Message::Error { message } if message.contains("regex")));
    assert_eq!(out[1], Message::QueryDone { gen: 1, total: 0, version: 1 });
    assert_eq!(out.len(), 2);
    assert!(run(&mut s, &latest, Message::GetRows { gen: 1, from: 0, to: 10 }).iter().all(|m| !matches!(m, Message::Rows { .. })));
}

#[test]
fn index_changes_refresh_the_result_and_status_is_pushed() {
    let sh = shared();
    let mut s = Session::new(sh.clone(), 0);
    let latest = AtomicU64::new(1);
    run(&mut s, &latest, query(1, "new"));
    sh.set_volume(VolumeInfo { id: "C:".into(), label: String::new(), file_system: "NTFS".into(), files: 101, state: VolumeState::Ready });
    {
        let mut e = sh.write();
        let v = &mut e.volumes[0];
        v.upsert(&IndexRecord { id: 900, parent: 5, name: b"new.txt", size: 1, modified: 0, attrs: 0 });
        v.commit();
        e.version += 1;
    }
    let out = run(&mut s, &latest, Message::GetRows { gen: 1, from: 0, to: 10 });
    assert_eq!(out[0], Message::QueryDone { gen: 1, total: 1, version: 2 }, "stale result is recomputed before rows are served");
    let mut ticked = Vec::new();
    s.tick(&latest, &mut |m| ticked.push(m));
    assert!(matches!(&ticked[..], [Message::IndexStatus { volumes }] if volumes[0].files == 101));
    ticked.clear();
    s.tick(&latest, &mut |m| ticked.push(m));
    assert!(ticked.is_empty());
}

#[test]
fn w3_review_summaries_are_cached_and_refresh_at_the_same_total_after_an_index_change() {
    let shared = shared();
    let open = Arc::new(AtomicU64::new(1));
    let mut s = Session::new(shared.clone(), 0).with_facets(open.clone());
    let latest = AtomicU64::new(1);
    let facets = Message::GetFacets { gen: 1, open: true };
    let folders = Message::GetFolders { gen: 1 };
    run(&mut s, &latest, query(1, "file000"));
    let original_facets = run(&mut s, &latest, facets.clone());
    let original_folders = run(&mut s, &latest, folders.clone());
    let current = s.current.as_ref().unwrap();
    assert!(current.facets.is_some() && current.folders.is_some());
    open.store(0, Ordering::Release);
    assert!(run(&mut s, &latest, facets.clone()).is_empty());
    open.store(1, Ordering::Release);
    assert_eq!(run(&mut s, &latest, facets.clone()), original_facets);
    assert_eq!(run(&mut s, &latest, folders.clone()), original_folders);
    {
        let mut e = shared.write();
        let v = &mut e.volumes[0];
        v.upsert(&IndexRecord { id: 6, parent: 5, name: b"new", size: 0, modified: 0, attrs: attr::DIR });
        v.upsert(&IndexRecord { id: 100, parent: 6, name: b"file000.md", size: 0, modified: 0, attrs: 0 });
        v.commit();
        e.version += 1;
    }
    assert!(run(&mut s, &latest, folders.clone()).is_empty());
    let out = run(&mut s, &latest, Message::GetRows { gen: 1, from: 0, to: 10 });
    assert_eq!(out[0], Message::QueryDone { gen: 1, total: 1, version: 2 });
    assert!(matches!(&run(&mut s, &latest, facets)[..], [Message::Facets { counts, .. }] if counts.extensions == [("md".into(), 1)]));
    assert!(matches!(&run(&mut s, &latest, folders)[..], [Message::Folders { counts, .. }] if counts == &[(br"C:\new".to_vec(), 1)]));
}

#[test]
fn folder_sorted_results_carry_group_starts() {
    let s = SharedIndex::new();
    let mut v = VolumeIndex::new("C:", b"C:\\", b'\\');
    v.upsert(&IndexRecord { id: 5, parent: 5, name: b".", size: 0, modified: 0, attrs: attr::DIR });
    v.upsert(&IndexRecord { id: 6, parent: 5, name: b"a", size: 0, modified: 0, attrs: attr::DIR });
    v.upsert(&IndexRecord { id: 7, parent: 5, name: b"b", size: 0, modified: 0, attrs: attr::DIR });
    for (id, parent, name) in [(10, 6, "x1.txt"), (11, 6, "x2.txt"), (12, 7, "x3.txt"), (13, 5, "x4.txt")] {
        v.upsert(&IndexRecord { id, parent, name: name.as_bytes(), size: 1, modified: 0, attrs: 0 });
    }
    v.commit();
    s.write().volumes.push(v);
    let mut session = Session::new(s, 0);
    let latest = AtomicU64::new(1);
    let by_folder = Sort { key: SortKey::Path, descending: false };
    let out = run(&mut session, &latest, Message::Query { gen: 1, text: "x".into(), conditions: Vec::new(), sort: by_folder, paths: Default::default() });
    assert_eq!(out[0], Message::QueryDone { gen: 1, total: 4, version: 0 });
    assert_eq!(out[1], Message::Groups { gen: 1, starts: vec![0, 1, 3] });
    latest.store(2, Ordering::Release);
    let out = run(&mut session, &latest, query(2, "x"));
    assert!(!out.iter().any(|m| matches!(m, Message::Groups { .. })), "only folder order has groups");
}

#[test]
fn a_removed_volume_leaves_open_results_and_the_status() {
    let shared = shared();
    let mut d = VolumeIndex::new("D:", b"D:\\", b'\\');
    d.upsert(&IndexRecord { id: 5, parent: 5, name: b".", size: 0, modified: 0, attrs: attr::DIR });
    for k in 0..300u64 {
        let name = format!("file{k:03}.dat");
        d.upsert(&IndexRecord { id: 100 + k, parent: 5, name: name.as_bytes(), size: k, modified: 0, attrs: 0 });
    }
    d.commit();
    shared.write().volumes.push(d);
    for id in ["C:", "D:"] {
        shared.set_volume(VolumeInfo { id: id.into(), label: String::new(), file_system: String::new(), files: 0, state: VolumeState::Ready });
    }
    let mut s = Session::new(shared.clone(), 0);
    let latest = AtomicU64::new(1);
    assert_eq!(run(&mut s, &latest, query(1, "file"))[0], Message::QueryDone { gen: 1, total: 400, version: 1 });
    let (seen, _) = shared.status();
    shared.remove_volume("Q:");
    assert_eq!(shared.status().0, seen, "an unknown volume changes nothing");
    shared.remove_volume("C:");
    let (version, volumes) = shared.status();
    assert!(version > seen);
    assert_eq!(volumes.iter().map(|v| v.id.as_str()).collect::<Vec<_>>(), ["D:"]);
    let out = run(&mut s, &latest, Message::GetRows { gen: 1, from: 250, to: 400 });
    assert_eq!(out[0], Message::QueryDone { gen: 1, total: 300, version: 2 });
    let Some(Message::Rows { from: 250, entries, dirs, .. }) = out.last() else { panic!("{out:?}") };
    assert_eq!(entries.len(), 50);
    assert!(dirs.iter().all(|d| d == br"D:\"));
}
