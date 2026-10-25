use super::*;
use crate::listing::{attr, ChangeKind, DirChange, DirSnapshot, Meta};
use crate::path::FsPath;
use crate::sort::{self, CaseMode, SortField, SortKeys, SortSpec, ViewSpec};
use crate::view::DirView;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn hit(pattern: &str, name: &str) -> bool {
    NameFilter::new(pattern).unwrap().matches_name(name.as_bytes(), &mut String::new())
}

fn snap(names: &[&str]) -> Arc<DirSnapshot> {
    let mut listing = Listing::default();
    let mut keys = SortKeys::new(CaseMode::Insensitive);
    for (k, n) in names.iter().enumerate() {
        let i = listing.push(n.as_bytes(), Meta { size: k as u64, ..Default::default() });
        keys.push(listing.name(i));
    }
    let view = ViewSpec::default();
    let mut order = Vec::new();
    sort::sort(&listing, &keys, view, &mut order);
    Arc::new(DirSnapshot { gen: 1, path: FsPath::parse(r"C:\t").unwrap(), listing, keys, order, view, done: true, error: None, elapsed: Duration::ZERO })
}

fn rows(v: &DirView) -> Vec<String> {
    v.rows().iter().map(|&i| String::from_utf8_lossy(v.listing().name(i)).into_owned()).collect()
}

fn added(name: &str) -> DirChange {
    DirChange { kind: ChangeKind::Added, name: name.into(), meta: Some(Meta::default()) }
}

#[test]
fn substring_ignores_case_in_any_script() {
    assert!(NameFilter::new("").is_none());
    assert!(hit("REP", "annual report.pdf"));
    assert!(hit("rt.P", "annual report.pdf"));
    assert!(!hit("reports", "annual report.pdf"));
    assert!(hit("док", "Мой ДОКУМЕНТ.txt"));
    assert!(hit("ß", "Straße"));
    assert!(!hit("д", "plain ascii"));
    assert!(hit("a\u{FFFD}b", "a\u{FFFD}b"));
    let invalid = [b'x', 0xED, 0xA0, 0x80, b'y'];
    assert!(!NameFilter::new("xy").unwrap().matches_name(&invalid, &mut String::new()));
    assert!(NameFilter::new("x\u{FFFD}").unwrap().matches_name(&invalid, &mut String::new()));
}

#[test]
fn wildcards_match_the_whole_name() {
    assert!(hit("*.TXT", "notes.txt"));
    assert!(!hit("*.txt", "notes.txt.bak"));
    assert!(hit("*.txt*", "notes.txt.bak"));
    assert!(hit("n?tes*", "Notes.md"));
    assert!(!hit("n?tes", "Notes.md"));
    assert!(hit("ф?йл.*", "Файл.doc"));
    assert!(hit("*a*b*c*", "xxaxxbxxcxx"));
    assert!(!hit("*a*b*c*", "xxaxxcxxbxx"));
    assert!(hit("?", "я"));
    assert!(!hit("??", "я"));
    assert!(hit("*", ""));
    assert!(!hit("?", ""));
}

#[test]
fn filtered_rows_drive_selection_and_lookup() {
    let mut v = DirView::new(snap(&["alpha.rs", "beta.txt", "gamma.rs", "delta.md"]));
    v.select_all();
    v.focus = Some(1);
    v.set_filter(".RS");
    assert!(v.is_filtered());
    assert_eq!(rows(&v), ["alpha.rs", "gamma.rs"]);
    assert_eq!((v.total_rows(), v.rows().len()), (4, 2));
    assert_eq!(v.selected().collect::<Vec<_>>(), [0, 2]);
    assert_eq!((v.selected_count(), v.focus), (2, None));
    assert_eq!(v.row_of(2), Some(1));
    assert_eq!(v.row_of(1), None);
    v.clear_selection();
    v.select_all();
    assert_eq!(v.selected_count(), 2);
    v.set_filter("");
    assert!(!v.is_filtered());
    assert_eq!(rows(&v), ["alpha.rs", "beta.txt", "delta.md", "gamma.rs"]);
    assert_eq!(v.selected_count(), 2, "hidden rows do not come back selected");
}

#[test]
fn diffs_resorts_and_sizes_respect_the_filter() {
    let mut v = DirView::new(snap(&["a1", "b1", "a2"]));
    v.set_filter("a");
    v.apply_all([added("a0"), added("zz"), DirChange { kind: ChangeKind::Removed, name: "a2".into(), meta: None }]);
    assert_eq!(rows(&v), ["a0", "a1"]);
    v.apply(&DirChange { kind: ChangeKind::Modified, name: "a0".into(), meta: Some(Meta { attrs: attr::HIDDEN, ..Default::default() }) });
    assert_eq!(rows(&v), ["a1"]);
    let by_size = ViewSpec { sort: SortSpec { field: SortField::Size, descending: true, ..Default::default() }, show_hidden: true, ..Default::default() };
    let mut order = Vec::new();
    sort::sort(v.listing(), &v.snap().keys, by_size, &mut order);
    let base = v.listing().len();
    v.apply(&added("a9"));
    v.merge_order(order, by_size, base);
    v.set_dir_size(0, 5_000_000);
    v.set_dir_size(3, 1_000_000);
    v.set_dir_size(5, 3_000_000);
    assert_eq!(rows(&v), ["a1", "a9", "a0"]);
    assert_eq!(v.total_rows(), 5);
}

#[test]
fn growing_snapshots_keep_the_filter() {
    let mut v = DirView::new(snap(&["x1", "y1"]));
    v.set_filter("y");
    v.replace(snap(&["x1", "y1", "y2", "x2"]));
    assert_eq!(rows(&v), ["y1", "y2"]);
    v.replace(snap(&["q"]));
    assert!(rows(&v).is_empty());
}

#[test]
fn narrowing_reuses_previous_matches() {
    let names: Vec<String> = (0..2000).map(|i| format!("Item {i} {}", if i % 3 == 0 { "Ёлка" } else { "x" })).collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let mut typed = DirView::new(snap(&refs));
    let mut direct = DirView::new(snap(&refs));
    for text in ["1", "12", "12 ", "12 ё", "1", "", "*7", "*7*"] {
        typed.set_filter(text);
        direct.set_filter("");
        direct.set_filter(text);
        assert_eq!(typed.rows(), direct.rows(), "{text:?}");
    }
}

#[test]
fn filters_100k_entries_instantly() {
    let mut listing = Listing::default();
    let mut keys = SortKeys::new(CaseMode::Insensitive);
    for k in 0..100_000u32 {
        let name = match k % 4 {
            0 => format!("file{k}.txt"),
            1 => format!("IMG_{:05}.JPG", k),
            2 => format!("Документ {k} (копия).docx"),
            _ => format!("Report-{k}-final.pdf"),
        };
        let i = listing.push(name.as_bytes(), Meta::default());
        keys.push(listing.name(i));
    }
    let view = ViewSpec::default();
    let mut order = Vec::new();
    sort::sort(&listing, &keys, view, &mut order);
    let snap = Arc::new(DirSnapshot { gen: 1, path: FsPath::parse(r"C:\big").unwrap(), listing, keys, order, view, done: true, error: None, elapsed: Duration::ZERO });
    let mut v = DirView::new(snap);
    v.select_all();
    let mut worst = Duration::ZERO;
    for text in ["1", "12", "123", "1234", "", "копия", "*.jpg", "report"] {
        let t = Instant::now();
        v.set_filter(text);
        worst = worst.max(t.elapsed());
    }
    assert_eq!(v.rows().len(), 25_000);
    assert!(v.selected().all(|i| v.listing().name(i).starts_with(b"Report-")));
    v.select_all();
    assert_eq!(v.selected_count(), 25_000);
    if !cfg!(debug_assertions) {
        assert!(worst < Duration::from_millis(30), "slowest keystroke took {worst:?}");
    }
}
