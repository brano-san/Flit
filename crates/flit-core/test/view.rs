use super::*;
use crate::path::FsPath;
use crate::sort::{CaseMode, SortField, SortKeys, SortSpec};
use std::time::Duration;

pub(crate) fn snap(names: &[&str]) -> Arc<DirSnapshot> {
    let mut listing = Listing::default();
    let mut keys = SortKeys::new(CaseMode::Insensitive);
    for n in names {
        let i = listing.push(n.as_bytes(), Meta { size: n.len() as u64, ..Default::default() });
        keys.push(listing.name(i));
    }
    let view = ViewSpec::default();
    let mut order = Vec::new();
    sort::sort(&listing, &keys, view, &mut order);
    Arc::new(DirSnapshot {
        gen: 1,
        path: FsPath::parse("C:\\t").unwrap(),
        listing,
        keys,
        order,
        view,
        done: true,
        error: None,
        elapsed: Duration::ZERO,
    })
}

fn rows(v: &DirView) -> Vec<String> {
    v.rows().iter().map(|&i| String::from_utf8_lossy(v.listing().name(i)).into_owned()).collect()
}

fn change(kind: ChangeKind, name: &str, size: u64) -> DirChange {
    DirChange { kind, name: name.into(), meta: Some(Meta { size, ..Default::default() }) }
}

#[test]
fn diffs_keep_order_and_indices() {
    let mut v = DirView::new(snap(&["b", "d", "f"]));
    v.set_selected(1, true);
    v.apply(&change(ChangeKind::Added, "a", 1));
    v.apply(&change(ChangeKind::Added, "e", 1));
    v.apply(&change(ChangeKind::Removed, "b", 0));
    v.apply(&change(ChangeKind::Removed, "zzz", 0));
    v.apply(&change(ChangeKind::Added, "e", 7));
    assert_eq!(rows(&v), ["a", "d", "e", "f"]);
    assert!(v.is_selected(1), "index of 'd' is stable");
    assert_eq!(v.listing().len(), 5, "tombstone kept");
    assert_eq!(v.listing().entries[4].size, 7);
    assert_eq!(v.find(b"b"), None);
    assert_eq!(v.row_of(1), Some(1));
}

#[test]
fn modified_moves_row_under_size_sort() {
    let s = snap(&["aa", "b", "cccc"]);
    let mut v = DirView::new(s);
    let view = ViewSpec { sort: SortSpec { field: SortField::Size, descending: false, ..Default::default() }, show_hidden: false, ..Default::default() };
    let mut order = Vec::new();
    sort::sort(v.listing(), &v.snap.keys, view, &mut order);
    let n = v.listing().len();
    v.merge_order(order, view, n);
    assert_eq!(rows(&v), ["b", "aa", "cccc"]);
    v.apply(&change(ChangeKind::Modified, "b", 100));
    assert_eq!(rows(&v), ["aa", "cccc", "b"]);
}

#[test]
fn merge_after_concurrent_changes() {
    let mut v = DirView::new(snap(&["a", "c"]));
    let view = ViewSpec { sort: SortSpec { field: SortField::Name, descending: true, ..Default::default() }, show_hidden: false, ..Default::default() };
    let mut order = Vec::new();
    sort::sort(v.listing(), &v.snap.keys, view, &mut order);
    v.apply(&change(ChangeKind::Added, "b", 1));
    v.apply(&change(ChangeKind::Removed, "c", 0));
    v.merge_order(order, view, 2);
    assert_eq!(rows(&v), ["b", "a"]);
}

#[test]
fn entries_that_turn_invisible_lose_selection_and_focus() {
    let mut v = DirView::new(snap(&["a", "b", "c"]));
    v.select_all();
    (v.focus, v.anchor) = (Some(1), Some(2));
    v.apply(&DirChange { kind: ChangeKind::Modified, name: "b".into(), meta: Some(Meta { attrs: attr::HIDDEN, ..Default::default() }) });
    assert_eq!(rows(&v), ["a", "c"]);
    assert_eq!((v.selected_count(), v.focus, v.anchor), (2, None, Some(2)));
    v.apply(&change(ChangeKind::Removed, "c", 0));
    assert_eq!(v.selected().collect::<Vec<_>>(), [0]);
    assert_eq!((v.selected_count(), v.anchor), (1, None));
    let shown = ViewSpec { show_hidden: true, ..Default::default() };
    let mut order = Vec::new();
    sort::sort(v.listing(), &v.snap.keys, shown, &mut order);
    v.merge_order(order, shown, 3);
    v.select_all();
    assert_eq!(v.selected_count(), 2);
    let mut order = Vec::new();
    sort::sort(v.listing(), &v.snap.keys, ViewSpec::default(), &mut order);
    v.merge_order(order, ViewSpec::default(), 3);
    assert_eq!((v.selected_count(), v.selected().collect::<Vec<_>>()), (1, vec![0]));
}

#[test]
fn batched_diffs_match_a_full_sort() {
    let names: Vec<String> = (0..300).map(|i| format!("f{i:03}")).collect();
    let mut v = DirView::new(snap(&names.iter().map(String::as_str).collect::<Vec<_>>()));
    let by_size = ViewSpec { sort: SortSpec { field: SortField::Size, descending: true, ..Default::default() }, show_hidden: false, ..Default::default() };
    let mut order = Vec::new();
    sort::sort(v.listing(), &v.snap.keys, by_size, &mut order);
    v.merge_order(order, by_size, names.len());
    let mut changes: Vec<DirChange> = (0..300).step_by(3).map(|i| change(ChangeKind::Removed, &names[i], 0)).collect();
    changes.extend((1..300).step_by(5).map(|i| change(ChangeKind::Modified, &names[i], 1000 - i as u64)));
    changes.extend((0..60).map(|i| change(ChangeKind::Added, &format!("n{i}"), i * 7 % 13)));
    changes.push(change(ChangeKind::Added, "f000", 500));
    changes.push(DirChange { kind: ChangeKind::Modified, name: "f002".into(), meta: Some(Meta { attrs: attr::HIDDEN, ..Default::default() }) });
    v.apply_all(&changes);
    let mut expected = Vec::new();
    sort::sort(v.listing(), &v.snap.keys, by_size, &mut expected);
    assert_eq!(v.rows(), expected.as_slice());
    assert_eq!(v.listing().len(), 381, "modifying a removed name re-adds it");
}

#[test]
fn selection_ops() {
    let mut v = DirView::new(snap(&["a", "b", "c", "d"]));
    v.select_rows(3, 1);
    assert_eq!(v.selected().collect::<Vec<_>>(), [1, 2, 3]);
    assert_eq!(v.selected_size(), 3);
    v.select_all();
    assert_eq!(v.selected_count(), 4);
    v.set_selected(0, false);
    v.set_selected(200, false);
    assert_eq!(v.selected_count(), 3);
}
