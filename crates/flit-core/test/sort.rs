use super::*;
use crate::listing::{ColumnValues, Meta, PropKey};

fn sorted(names: &[&str], case: CaseMode) -> Vec<String> {
    let mut v: Vec<&str> = names.to_vec();
    v.sort_by(|a, b| natural_cmp(a.as_bytes(), b.as_bytes(), case));
    v.into_iter().map(String::from).collect()
}

#[test]
fn strcmplogicalw_doc_order() {
    let want = ["2string", "3string", "20string", "st2ring", "st3ring", "st20ring", "string2", "string3", "string20"];
    let mut shuffled = want;
    shuffled.reverse();
    shuffled.swap(1, 6);
    assert_eq!(sorted(&shuffled, CaseMode::Insensitive), want);
}

#[test]
fn numbers_and_classes() {
    assert_eq!(sorted(&["a10", "a9", "a01", "a1", "a", "a.txt", "a 1", "_x", "!y", "9", "B", "b"], CaseMode::Insensitive), ["!y", "_x", "9", "a", "a 1", "a.txt", "a01", "a1", "a9", "a10", "B", "b"]);
    assert_eq!(sorted(&["f100000000000000000000000", "f99999999999999999999999"], CaseMode::Insensitive), ["f99999999999999999999999", "f100000000000000000000000"]);
}

#[test]
fn case_modes() {
    use Ordering::*;
    assert_eq!(natural_cmp(b"abc", b"ABD", CaseMode::Insensitive), Less);
    assert_eq!(natural_cmp(b"Zeta", b"alpha", CaseMode::Insensitive), Greater);
    assert_eq!(natural_cmp(b"Zeta", b"alpha", CaseMode::Sensitive), Greater);
    assert_eq!(natural_cmp(b"a", b"A", CaseMode::Sensitive), Less);
    assert_eq!(natural_cmp(b"A", b"b", CaseMode::Sensitive), Less);
    assert_eq!(natural_cmp("Яблоко".as_bytes(), "арбуз".as_bytes(), CaseMode::Insensitive), Greater);
    assert_eq!(natural_cmp("Арбуз".as_bytes(), "яблоко".as_bytes(), CaseMode::Insensitive), Less);
}

#[test]
fn invalid_names_sort_stably() {
    let mut bad = b"x".to_vec();
    name::push_utf16(&mut bad, &[0xD800]);
    assert_eq!(natural_cmp(&bad, b"y", CaseMode::Insensitive), Ordering::Less);
    assert_eq!(natural_cmp(b"x\xFF", b"x\xFE", CaseMode::Insensitive), Ordering::Greater);
}

#[test]
fn accented_latin_letters_sort_with_their_base_letter() {
    assert_eq!(sorted(&["Zebra", "Äpfel", "Apfel", "Bär", "Ölung", "Ofen", "Pferd"], CaseMode::Insensitive), ["Apfel", "Äpfel", "Bär", "Ofen", "Ölung", "Pferd", "Zebra"]);
    assert_eq!(sorted(&["été", "ecole", "élan", "fin", "Ångström", "zone"], CaseMode::Insensitive), ["Ångström", "ecole", "élan", "été", "fin", "zone"]);
    assert_eq!(sorted(&["Straße", "Strasse", "Strand", "Strauch"], CaseMode::Insensitive), ["Strand", "Strasse", "Straße", "Strauch"]);
    assert_eq!(sorted(&["Łódź", "Lublin", "Lodz", "Mielec", "Ćmielów", "Cieszyn", "Dębica"], CaseMode::Insensitive), ["Cieszyn", "Ćmielów", "Dębica", "Lodz", "Łódź", "Lublin", "Mielec"]);
    assert_eq!(sorted(&["Đà Nẵng", "Hà Nội", "Huế", "Cần Thơ"], CaseMode::Insensitive), ["Cần Thơ", "Đà Nẵng", "Hà Nội", "Huế"]);
}

#[test]
fn cyrillic_yo_and_ukrainian_letters_take_their_alphabet_place() {
    assert_eq!(sorted(&["Ёлка", "Яма", "Ель", "Жук", "ёж", "Дом", "Эхо"], CaseMode::Insensitive), ["Дом", "ёж", "Ёлка", "Ель", "Жук", "Эхо", "Яма"]);
    assert_eq!(sorted(&["Їжак", "Яр", "Іван", "Йод", "Ива", "Ґанок", "Голуб", "Дім", "Євген", "Жовтень"], CaseMode::Insensitive), ["Голуб", "Ґанок", "Дім", "Євген", "Жовтень", "Ива", "Іван", "Їжак", "Йод", "Яр"]);
    assert_eq!(sorted(&["ώρα", "άλφα", "βήτα", "ωμέγα", "αλφάβητο"], CaseMode::Insensitive), ["άλφα", "αλφάβητο", "βήτα", "ωμέγα", "ώρα"]);
}

#[test]
fn letters_with_marks_keep_case_order_and_numbers_and_follow_plain_letters_on_ties() {
    use Ordering::*;
    assert_eq!(natural_cmp("ä".as_bytes(), b"b", CaseMode::Sensitive), Less);
    assert_eq!(natural_cmp("ä".as_bytes(), "Ä".as_bytes(), CaseMode::Sensitive), Less);
    assert_eq!(natural_cmp("Ä".as_bytes(), b"b", CaseMode::Sensitive), Less);
    assert_eq!(natural_cmp("ё".as_bytes(), "Ё".as_bytes(), CaseMode::Sensitive), Less);
    assert_eq!(natural_cmp("Ё".as_bytes(), "ж".as_bytes(), CaseMode::Sensitive), Less);
    assert_eq!(natural_cmp(b"a", "ä".as_bytes(), CaseMode::Insensitive), Less);
    assert_eq!(natural_cmp("e\u{301}".as_bytes(), b"f", CaseMode::Insensitive), Less);
    assert_eq!(natural_cmp("e\u{301}".as_bytes(), "é".as_bytes(), CaseMode::Insensitive), Less);
    assert_eq!(sorted(&["Ä10", "a9", "Ä2", "a10"], CaseMode::Insensitive), ["Ä2", "a9", "a10", "Ä10"]);
}

#[test]
fn symbols_and_other_scripts_keep_their_place_around_latin_greek_and_cyrillic() {
    assert_eq!(sorted(&["日本", "zeta", "яр", "α", "×x", "שלום", "ÿ"], CaseMode::Insensitive), ["ÿ", "zeta", "×x", "α", "яр", "שלום", "日本"]);
    assert_eq!(sorted(&["b", "_", "a", "1"], CaseMode::Insensitive), ["_", "1", "a", "b"]);
}

fn listing(items: &[(&str, u64, bool)]) -> (Listing, SortKeys) {
    let mut l = Listing::default();
    let mut k = SortKeys::new(CaseMode::Insensitive);
    for &(n, size, dir) in items {
        let attrs = if dir { attr::DIR } else { 0 };
        let i = l.push(n.as_bytes(), Meta { size, modified: size as i64, attrs, ..Default::default() });
        k.push(l.name(i));
    }
    (l, k)
}

fn names(l: &Listing, order: &[u32]) -> Vec<String> {
    order.iter().map(|&i| String::from_utf8_lossy(l.name(i)).into_owned()).collect()
}

#[test]
fn fields_and_folders_first() {
    let (l, k) = listing(&[("b.txt", 30, false), ("Dir10", 0, true), ("a.rs", 20, false), ("dir9", 0, true), ("c.md", 10, false)]);
    let mut o = Vec::new();
    let mut v = ViewSpec::default();
    sort(&l, &k, v, &mut o);
    assert_eq!(names(&l, &o), ["dir9", "Dir10", "a.rs", "b.txt", "c.md"]);
    v.sort = SortSpec { field: SortField::Size, descending: true, ..Default::default() };
    sort(&l, &k, v, &mut o);
    assert_eq!(names(&l, &o), ["Dir10", "dir9", "b.txt", "a.rs", "c.md"]);
    v.sort = SortSpec { field: SortField::Type, descending: false, ..Default::default() };
    sort(&l, &k, v, &mut o);
    assert_eq!(names(&l, &o), ["dir9", "Dir10", "c.md", "a.rs", "b.txt"]);
}

#[test]
fn a_folder_of_umlaut_and_yo_names_lists_them_in_alphabet_order_with_folders_first() {
    let (l, k) = listing(&[("Ölbild.png", 1, false), ("Zug.txt", 2, false), ("Ёлка", 0, true), ("Ель", 0, true), ("Апрель", 0, true), ("Äpfel.txt", 3, false), ("Banane.txt", 4, false)]);
    let mut o = Vec::new();
    let mut v = ViewSpec::default();
    sort(&l, &k, v, &mut o);
    assert_eq!(names(&l, &o), ["Апрель", "Ёлка", "Ель", "Äpfel.txt", "Banane.txt", "Ölbild.png", "Zug.txt"]);
    v.sort.descending = true;
    sort(&l, &k, v, &mut o);
    assert_eq!(names(&l, &o), ["Ель", "Ёлка", "Апрель", "Zug.txt", "Ölbild.png", "Banane.txt", "Äpfel.txt"]);
    assert!(is_sorted(&l, &k, &v, &o));
}

const DAY: i64 = 86_400 * 10_000_000;
const MAR_15_2024: i64 = 133_549_344_000_000_000;
const SIZES: [u64; 8] = [0, 100, 20_000, 2 << 20, 200 << 20, 2 << 30, 5 << 30, 7];

fn synthetic(n: u64) -> (Listing, SortKeys) {
    let exts = ["", "txt", "TXT", "longextension1", "longextension2", "LONGEXTENSIOn", "abcdefgh", "abcdefghi", "rs"];
    let mut l = Listing::default();
    let mut k = SortKeys::new(CaseMode::Insensitive);
    let mut seed = 7u64;
    for _ in 0..n {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let r = seed >> 33;
        let ext = exts[(r % exts.len() as u64) as usize];
        let name = format!("{}{}.{ext}", ["a", "B", "file", "File 0", ".dot", "9lives", "_x", "Ägypten"][(r % 8) as usize], r % 50);
        let mut attrs = if r.is_multiple_of(7) { attr::DIR } else { 0 };
        if r.is_multiple_of(13) {
            attrs |= attr::HIDDEN;
        }
        if r.is_multiple_of(26) {
            attrs |= attr::SYSTEM;
        }
        if r.is_multiple_of(5) {
            attrs |= attr::SIZE_KNOWN;
        }
        let modified = if r.is_multiple_of(11) { 0 } else { MAR_15_2024 + (r % 900) as i64 * DAY - DAY / 2 };
        let created = MAR_15_2024 - (r % 400) as i64 * DAY;
        let idx = l.push(name.as_bytes(), Meta { size: SIZES[(r % 8) as usize], modified, created, attrs });
        l.set_tags(idx, [0, 0, 1, 2, 3, 4, 6, 1 << 31][(r % 8) as usize]);
        l.set_accessed(idx, modified / 2 + (r % 3) as i64 * DAY);
        if attrs & attr::DIR != 0 && r.is_multiple_of(3) {
            l.set_stats(idx, crate::listing::DirStats { size: SIZES[(r % 8) as usize], folders: (r % 4) as u32, files: (r % 1200) as u32, partial: false });
        }
        k.push(l.name(idx));
    }
    (l, k)
}

#[test]
fn fast_sort_matches_comparator() {
    let (l, k) = synthetic(3000);
    let groups = [GroupField::None, GroupField::Name, GroupField::Type, GroupField::Size, GroupField::Modified, GroupField::Created, GroupField::Accessed, GroupField::Items, GroupField::Volume, GroupField::FolderUsage, GroupField::Tag];
    for field in [SortField::Name, SortField::Size, SortField::Modified, SortField::Created, SortField::Accessed, SortField::Items, SortField::Folders, SortField::Files, SortField::Volume, SortField::FolderUsage, SortField::Type, SortField::Tag] {
        for (descending, folders) in [(false, Folders::First), (true, Folders::First), (false, Folders::Last), (true, Folders::Mixed), (false, Folders::HiddenLast), (true, Folders::HiddenLast)] {
            for (g, &group) in groups.iter().enumerate() {
                let view = ViewSpec {
                    sort: SortSpec { field, descending, folders },
                    group: GroupSpec { field: group, descending: g % 2 == 1, ..Default::default() },
                    today: crate::fmt::local_days(MAR_15_2024, 180) + 30 * g as i64,
                    show_hidden: true,
                    utc_offset_min: 180,
                    ..Default::default()
                };
                let mut fast = Vec::new();
                sort(&l, &k, view, &mut fast);
                let mut slow: Vec<u32> = (0..l.len() as u32).filter(|&i| visible(&l, i, &view)).collect();
                slow.sort_by(|&a, &b| cmp(&l, &k, &view, a, b));
                assert_eq!(fast, slow, "{field:?} desc={descending} {folders:?} {group:?}");
            }
        }
    }
}

#[test]
fn hidden_and_incremental_insert() {
    let (mut l, mut k) = listing(&[("a", 0, false), ("c", 0, false)]);
    l.entries[1].attrs |= attr::HIDDEN;
    let spec = ViewSpec::default();
    let mut o = Vec::new();
    sort(&l, &k, spec, &mut o);
    assert_eq!(names(&l, &o), ["a"]);
    let i = l.push(b"b", Meta::default());
    k.push(l.name(i));
    let pos = insert_pos(&l, &k, &spec, &o, i);
    o.insert(pos, i);
    assert_eq!(names(&l, &o), ["a", "b"]);
    assert_eq!(find_row(&l, &k, &spec, &o, i), Some(1));
    assert_eq!(find_row(&l, &k, &spec, &o, 1), None);
}

#[test]
fn folders_go_first_last_or_mix_and_created_has_its_own_order() {
    let (mut l, k) = listing(&[("b.txt", 30, false), ("Dir10", 0, true), ("a.rs", 20, false)]);
    for (i, created) in [3, 1, 2].into_iter().enumerate() {
        l.entries[i].created = created;
    }
    let mut o = Vec::new();
    let mut v = ViewSpec::default();
    for (folders, want) in [(Folders::First, ["Dir10", "a.rs", "b.txt"]), (Folders::Last, ["a.rs", "b.txt", "Dir10"]), (Folders::Mixed, ["a.rs", "b.txt", "Dir10"])] {
        v.sort.folders = folders;
        sort(&l, &k, v, &mut o);
        assert_eq!(names(&l, &o), want, "{folders:?}");
    }
    v.sort = SortSpec { field: SortField::Created, descending: false, folders: Folders::Mixed };
    sort(&l, &k, v, &mut o);
    assert_eq!(names(&l, &o), ["Dir10", "a.rs", "b.txt"]);
    v.sort.descending = true;
    sort(&l, &k, v, &mut o);
    assert_eq!(names(&l, &o), ["b.txt", "a.rs", "Dir10"]);
}

#[test]
fn hidden_last_keeps_folders_first_and_sinks_hidden_items_below_everything() {
    let (mut l, k) = listing(&[(".git", 0, true), ("b.txt", 30, false), (".env", 5, false), ("Dir", 0, true), ("a.rs", 20, false)]);
    l.entries[0].attrs |= attr::HIDDEN;
    l.entries[2].attrs |= attr::HIDDEN;
    let mut o = Vec::new();
    let mut v = ViewSpec { show_hidden: true, ..Default::default() };
    v.sort.folders = Folders::HiddenLast;
    for descending in [false, true] {
        v.sort.descending = descending;
        sort(&l, &k, v, &mut o);
        let want = if descending { ["Dir", "b.txt", "a.rs", ".git", ".env"] } else { ["Dir", "a.rs", "b.txt", ".git", ".env"] };
        assert_eq!(names(&l, &o), want);
        for (row, &i) in o.iter().enumerate() {
            assert_eq!(find_row(&l, &k, &v, &o, i), Some(row), "{descending}");
        }
    }
}

#[test]
fn dotfiles_and_protected_system_files_have_their_own_switches() {
    let (mut l, k) = listing(&[("a", 0, false), (".git", 0, true), ("hidden", 0, false), ("pagefile.sys", 0, false)]);
    l.entries[2].attrs |= attr::HIDDEN;
    l.entries[3].attrs |= attr::HIDDEN | attr::SYSTEM;
    let shown = |v: ViewSpec| {
        let mut o = Vec::new();
        sort(&l, &k, v, &mut o);
        names(&l, &o)
    };
    let mut v = ViewSpec::default();
    assert_eq!(shown(v), [".git", "a"]);
    v.hide_dotfiles = true;
    assert_eq!(shown(v), ["a"]);
    v.show_hidden = true;
    assert_eq!(shown(v), ["a", "hidden"]);
    v.show_system = true;
    assert_eq!(shown(v), ["a", "hidden", "pagefile.sys"]);
}

#[test]
fn group_keys_follow_name_type_size_and_local_dates() {
    let (mut l, _) = listing(&[("zeta.TXT", 0, false), ("alpha.txt", 20_000, false), ("7z", 5 << 30, false), ("#tag", 1, false), ("sub", 0, true), ("Ärger.md", 100, false)]);
    l.entries[0].modified = MAR_15_2024 + DAY - 1;
    l.entries[1].modified = MAR_15_2024 + DAY + 1;
    let labels = |v: &ViewSpec| (0..l.len() as u32).map(|i| group_key(&l, i, v).0).collect::<Vec<_>>();
    let mut v = ViewSpec { group: GroupSpec { field: GroupField::Name, ..Default::default() }, ..Default::default() };
    assert_eq!(labels(&v), [2, 0, 3, 3, 2, 3], "A-H, I-P, Q-Z, then the rest");
    v.group.field = GroupField::Size;
    assert_eq!(labels(&v), [6, 4, 0, 5, UNSPECIFIED, 5], "Gigantic first, Empty last, folders without a size unspecified");
    v.group.field = GroupField::Type;
    assert_eq!(group_id(&l, 0, &v), group_id(&l, 1, &v), "extensions group without regard to case");
    assert_eq!(group_key(&l, 4, &v), (0, true));
    v.group = GroupSpec { field: GroupField::Modified, ..Default::default() };
    v.today = crate::fmt::local_days(MAR_15_2024 + DAY + 1, 0);
    assert_eq!(group_key(&l, 1, &v).0, 0, "today");
    assert_eq!(group_key(&l, 0, &v).0, 1, "yesterday");
    v.utc_offset_min = -60;
    assert_eq!(group_key(&l, 0, &v), group_key(&l, 1, &v), "both fall on the same local day an hour west of UTC");
    l.entries[2].modified = 0;
    assert_eq!(group_key(&l, 2, &v).0, UNSPECIFIED, "no date");
    v.today += 400;
    assert_eq!(group_key(&l, 1, &v).0, 5, "long ago");
}

#[test]
fn item_count_and_percent_buckets() {
    let (mut l, _) = listing(&[("a", 0, true), ("b", 0, true), ("c", 0, true), ("f.txt", 50, false)]);
    l.set_stats(0, crate::listing::DirStats { size: 10, folders: 1, files: 1500, partial: false });
    l.set_stats(1, crate::listing::DirStats { size: 40, folders: 0, files: 0, partial: false });
    assert_eq!((0..4).map(|i| items_bucket(&l, i)).collect::<Vec<_>>(), [0, 4, UNSPECIFIED, UNSPECIFIED]);
    assert_eq!([percent_bucket(95, 100), percent_bucket(5, 100), percent_bucket(1, 0)], [0, 9, UNSPECIFIED]);
    let v = ViewSpec { group: GroupSpec { field: GroupField::FolderUsage, ..Default::default() }, ..Default::default() };
    assert!(!incremental(&v), "folder usage depends on every size");
    assert_eq!(group_key(&l, 3, &v).0, percent_bucket(50, l.total));
}

#[test]
fn tags_sort_by_first_tag_and_group_untagged_last() {
    let (mut l, k) = listing(&[("a", 0, false), ("b", 0, false), ("c", 0, false), ("d", 0, false), ("e", 0, false)]);
    for (i, m) in [0b100, 0, 0b011, 0b010, 0b001].into_iter().enumerate() {
        l.set_tags(i as u32, m);
    }
    let mut o = Vec::new();
    let mut v = ViewSpec { sort: SortSpec { field: SortField::Tag, ..Default::default() }, ..Default::default() };
    sort(&l, &k, v, &mut o);
    assert_eq!(names(&l, &o), ["c", "e", "d", "a", "b"]);
    v.sort.descending = true;
    sort(&l, &k, v, &mut o);
    assert_eq!(names(&l, &o), ["b", "a", "d", "e", "c"]);
    v.group.field = GroupField::Tag;
    assert_eq!((0..5).map(|i| group_key(&l, i, &v).0).collect::<Vec<_>>(), [2, 32, 0, 1, 0]);
    assert_eq!((first_tag(0), first_tag(1 << 31)), (32, 31));
}

#[test]
#[cfg_attr(debug_assertions, ignore)]
fn bench_sort_and_group_100k() {
    let (l, k) = synthetic(100_000);
    let mut o = Vec::new();
    let fields = [SortField::Name, SortField::Size, SortField::Modified, SortField::Type];
    let groups = [GroupField::None, GroupField::Name, GroupField::Type, GroupField::Size, GroupField::Modified];
    for (sort_field, group) in fields.into_iter().flat_map(|f| groups.into_iter().map(move |g| (f, g))) {
        let view = ViewSpec { sort: SortSpec { field: sort_field, ..Default::default() }, group: GroupSpec { field: group, ..Default::default() }, show_hidden: true, ..Default::default() };
        let mut runs: Vec<f64> = (0..9)
            .map(|_| {
                let t = std::time::Instant::now();
                sort(&l, &k, view, &mut o);
                t.elapsed().as_secs_f64() * 1e3
            })
            .collect();
        runs.sort_by(f64::total_cmp);
        println!("sort {sort_field:?} group {group:?}: {:.2} ms median over 100k", runs[4]);
        assert!(runs[4] < 60.0, "sort {sort_field:?} group {group:?} took {:.2} ms", runs[4]);
    }
    let path = crate::path::FsPath::parse(r"C:\x").unwrap();
    for group in [GroupField::None, GroupField::Name, GroupField::Type, GroupField::Modified] {
        let view = ViewSpec { group: GroupSpec { field: group, ..Default::default() }, show_hidden: true, ..Default::default() };
        sort(&l, &k, view, &mut o);
        let snap = crate::listing::DirSnapshot { gen: 1, path: path.clone(), listing: l.clone(), keys: k.clone(), order: o.clone(), view, done: true, error: None, elapsed: Default::default() };
        let mut v = crate::view::DirView::new(std::sync::Arc::new(snap));
        let mut runs: Vec<(f64, f64)> = (0..9)
            .map(|_| {
                let t = std::time::Instant::now();
                v.merge_order(o.clone(), view, l.len());
                let merged = t.elapsed().as_secs_f64() * 1e3;
                let t = std::time::Instant::now();
                v.set_filter("");
                (merged, t.elapsed().as_secs_f64() * 1e3)
            })
            .collect();
        runs.sort_by(|a, b| a.0.total_cmp(&b.0));
        let merge = runs[4].0;
        runs.sort_by(|a, b| a.1.total_cmp(&b.1));
        println!("ui thread, group {group:?}: merge {merge:.2} ms, regroup {:.2} ms median over 100k, {} groups", runs[4].1, v.groups().len());
        assert!(merge < 8.0 && runs[4].1 < 8.0, "merge {merge:.2} ms, regroup {:.2} ms for group {group:?}", runs[4].1);
    }
}
#[test]
fn folders_and_files_sort_by_their_own_counts() {
    use crate::listing::DirStats;
    let (mut l, k) = listing(&[("a", 0, true), ("b", 0, true)]);
    let (a, b) = (0, 1);
    l.set_stats(a, DirStats { size: 0, folders: 5, files: 1, partial: false });
    l.set_stats(b, DirStats { size: 0, folders: 1, files: 9, partial: false });
    let order = |field| {
        let mut out = Vec::new();
        sort(&l, &k, ViewSpec { sort: SortSpec { field, ..Default::default() }, ..Default::default() }, &mut out);
        out
    };
    assert_eq!(order(SortField::Folders), [b, a]);
    assert_eq!(order(SortField::Files), [a, b]);
}

#[test]
fn ui_thread_sorts_do_not_wait_for_a_busy_global_pool() {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering as AtomicOrdering};
    use std::time::{Duration, Instant};
    let (l, k) = synthetic(100_000);
    let view = ViewSpec { show_hidden: true, ..Default::default() };
    let usage = ViewSpec { group: GroupSpec { field: GroupField::FolderUsage, ..Default::default() }, ..view };
    let (mut sorted, mut by_usage) = (Vec::new(), Vec::new());
    sort(&l, &k, view, &mut sorted);
    sort(&l, &k, usage, &mut by_usage);
    let path = crate::path::FsPath::parse(r"C:\x").unwrap();
    let snap = crate::listing::DirSnapshot { gen: 1, path, listing: l.clone(), keys: k.clone(), order: sorted.clone(), view, done: true, error: None, elapsed: Default::default() };
    let mut v = crate::view::DirView::new(std::sync::Arc::new(snap));
    v.merge_order(Vec::new(), view, l.len());
    let release = std::sync::Arc::new(AtomicBool::new(false));
    let busy = std::sync::Arc::new(AtomicUsize::new(0));
    let workers = rayon::current_num_threads();
    for _ in 0..workers {
        let (release, busy) = (release.clone(), busy.clone());
        rayon::spawn(move || {
            busy.fetch_add(1, AtomicOrdering::SeqCst);
            let start = Instant::now();
            while !release.load(AtomicOrdering::SeqCst) && start.elapsed() < Duration::from_secs(10) {
                std::thread::sleep(Duration::from_millis(1));
            }
        });
    }
    let start = Instant::now();
    while busy.load(AtomicOrdering::SeqCst) < workers && start.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(1));
    }
    let t = Instant::now();
    v.merge_order(sorted.iter().rev().copied().collect(), view, l.len());
    let resorted = t.elapsed();
    let t = Instant::now();
    v.merge_order(by_usage.clone(), usage, l.len());
    let full = t.elapsed();
    release.store(true, AtomicOrdering::SeqCst);
    assert_eq!(v.rows(), by_usage.as_slice());
    assert!(resorted < Duration::from_secs(2) && full < Duration::from_secs(2), "merge resort {resorted:?}, full sort {full:?} waited for the global pool");
}

#[test]
fn a_swap_at_any_chunk_boundary_breaks_sortedness() {
    let (l, k) = synthetic(40_000);
    let view = ViewSpec { show_hidden: true, ..Default::default() };
    let mut o = Vec::new();
    sort(&l, &k, view, &mut o);
    assert!(is_sorted(&l, &k, &view, &o));
    for threads in 2..=64 {
        let p = o.len().div_ceil(threads) - 1;
        o.swap(p, p + 1);
        assert!(!is_sorted(&l, &k, &view, &o), "swap at {p}");
        o.swap(p, p + 1);
    }
}

#[test]
fn attributes_sort_by_their_explorer_letters_and_plain_items_come_first() {
    let mut l = Listing::default();
    let mut k = SortKeys::new(CaseMode::Insensitive);
    for (n, attrs) in [("system", attr::SYSTEM | attr::ARCHIVE_MARK), ("plain", 0), ("archived", attr::ARCHIVE_MARK), ("readonly", attr::READONLY), ("compressed", attr::ARCHIVE_MARK | attr::COMPRESSED)] {
        let i = l.push(n.as_bytes(), Meta { attrs, ..Default::default() });
        k.push(l.name(i));
    }
    let mut o = Vec::new();
    let mut v = ViewSpec { sort: SortSpec { field: SortField::Attributes, ..Default::default() }, ..Default::default() };
    sort(&l, &k, v, &mut o);
    assert_eq!(names(&l, &o), ["plain", "archived", "compressed", "readonly", "system"]);
    v.sort.descending = true;
    sort(&l, &k, v, &mut o);
    assert_eq!(names(&l, &o), ["system", "readonly", "compressed", "archived", "plain"]);
    o.sort_by(|&a, &b| cmp(&l, &k, &v, a, b));
    assert_eq!(names(&l, &o), ["system", "readonly", "compressed", "archived", "plain"], "the comparison used for live updates agrees");
}

#[test]
fn relabeling_the_last_sort_key_preserves_earlier_keys_and_future_pushes() {
    for case in [CaseMode::Sensitive, CaseMode::Insensitive] {
        let mut keys = SortKeys::new(case);
        keys.relabel_last(b"empty");
        assert!(keys.is_empty());
        keys.push(b"first");
        keys.push(b"long-filename.desktop");
        let original = keys.key(0).to_vec();
        for label in [b"A".as_slice(), b"Translated Application 123", b"", b"Zebra"] {
            keys.relabel_last(label);
            assert_eq!(keys.len(), 2);
            assert_eq!(keys.key(0), original);
            let mut expected = SortKeys::new(case);
            expected.push(b"first");
            expected.push(label);
            assert_eq!(keys.key(1), expected.key(1));
            assert_eq!(keys.cmp(0, 1), expected.cmp(0, 1));
        }
        keys.push(b"after");
        let mut expected = SortKeys::new(case);
        for name in [b"first".as_slice(), b"Zebra", b"after"] {
            expected.push(name);
        }
        for i in 0..3 {
            assert_eq!(keys.key(i), expected.key(i));
        }
    }
}

type Media<'a> = (&'a str, Option<(u32, u32)>, Option<u64>, &'a str, &'a str);

fn media_listing() -> (Listing, SortKeys) {
    let (mut l, k) = listing(&[
        ("album", 0, true),
        ("a.jpg", 1, false),
        ("b.jpg", 1, false),
        ("c.txt", 1, false),
        ("d.jpg", 1, false),
        ("e.mp3", 1, false),
        ("f.mp3", 1, false),
        ("g.mp3", 1, false),
        ("h.mp3", 1, false),
        ("w.dll", 1, false),
        ("x.exe", 1, false),
        ("y.exe", 1, false),
        ("z.dll", 1, false),
    ]);
    let props: [Media; 11] = [
        ("a.jpg", Some((4000, 3000)), None, "", ""),
        ("b.jpg", Some((640, 480)), None, "", ""),
        ("d.jpg", Some((480, 640)), None, "", ""),
        ("e.mp3", None, Some(3), "nina Simone", ""),
        ("f.mp3", None, Some(45), "Abba", ""),
        ("g.mp3", None, Some(12), "artist 10", ""),
        ("h.mp3", None, Some(7), "Artist 9", ""),
        ("w.dll", None, None, "", "not a version"),
        ("x.exe", None, None, "", "10.0.1"),
        ("y.exe", None, None, "", "9.9.9"),
        ("z.dll", None, None, "", "10.0.1.2"),
    ];
    for (name, dims, minutes, artist, version) in props {
        let i = (0..l.len() as u32).find(|&i| l.name(i) == name.as_bytes()).unwrap();
        let text = vec![dims.map(|(w, h)| format!("{w} x {h}")).unwrap_or_default(), minutes.map(|m| format!("{m}:00")).unwrap_or_default(), artist.to_owned(), version.to_owned()];
        let values = ColumnValues { elf: false, text, width: dims.map(|d| d.0), height: dims.map(|d| d.1), duration: minutes.map(|m| m * 600_000_000) };
        if let Some(key) = PropKey::new(&values, CaseMode::Insensitive) {
            l.props.insert(i, key);
        }
    }
    (l, k)
}

fn by(l: &Listing, k: &SortKeys, field: SortField, descending: bool) -> Vec<String> {
    let view = ViewSpec { sort: SortSpec { field, descending, ..Default::default() }, ..Default::default() };
    let mut fast = Vec::new();
    sort(l, k, view, &mut fast);
    let mut slow: Vec<u32> = (0..l.len() as u32).collect();
    slow.sort_by(|&a, &b| cmp(l, k, &view, a, b));
    assert_eq!(fast, slow, "{field:?} desc={descending}");
    assert!(is_sorted(l, k, &view, &fast));
    names(l, &fast)
}

#[test]
fn shell_property_columns_sort_by_value_and_files_without_one_go_last_both_ways() {
    let (l, k) = media_listing();
    let rest = |shown: &[&str], descending: bool| {
        let mut others: Vec<String> = ["a.jpg", "b.jpg", "c.txt", "d.jpg", "e.mp3", "f.mp3", "g.mp3", "h.mp3", "w.dll", "x.exe", "y.exe", "z.dll"].iter().filter(|n| !shown.contains(n)).map(|n| n.to_string()).collect();
        if descending {
            others.reverse();
        }
        let mut want = vec!["album".to_owned()];
        want.extend(shown.iter().map(|s| s.to_string()));
        want.extend(others);
        want
    };
    let cases: [(SortField, &[&str]); 4] =
        [(SortField::Dimensions, &["d.jpg", "b.jpg", "a.jpg"]), (SortField::Duration, &["e.mp3", "h.mp3", "g.mp3", "f.mp3"]), (SortField::Artist, &["f.mp3", "h.mp3", "g.mp3", "e.mp3"]), (SortField::Version, &["y.exe", "x.exe", "z.dll"])];
    for (field, shown) in cases {
        assert_eq!(by(&l, &k, field, false), rest(shown, false), "{field:?}");
        let mut reversed = shown.to_vec();
        reversed.reverse();
        assert_eq!(by(&l, &k, field, true), rest(&reversed, true), "{field:?} descending");
    }
}

#[test]
fn versions_compare_as_numbers_and_text_that_is_no_version_has_none() {
    assert!(crate::listing::version_number("10.0.1") > crate::listing::version_number("9.9.9"));
    assert_eq!(crate::listing::version_number("10.0.1"), crate::listing::version_number("10.0.1.0"));
    assert!(crate::listing::version_number("10.0.1.2") > crate::listing::version_number("10.0.1"));
    for bad in ["", "1.2.3.4.5", "1..2", "v1.2", "1.70000", "1.2 beta", "-1"] {
        assert_eq!(crate::listing::version_number(bad), None, "{bad}");
    }
}

#[test]
fn shell_property_groups_split_pictures_media_artists_and_major_versions() {
    let (mut l, _) = listing(&[("p", 1, false)]);
    let mut key = |field: GroupField, values: ColumnValues| {
        l.props.clear();
        if let Some(k) = PropKey::new(&values, CaseMode::Insensitive) {
            l.props.insert(0, k);
        }
        let view = ViewSpec { group: GroupSpec { field, ..Default::default() }, ..Default::default() };
        group_key(&l, 0, &view).0
    };
    let picture = |w: u32, h: u32| ColumnValues { text: vec!["x".into()], width: Some(w), height: Some(h), ..Default::default() };
    let picture_groups: Vec<u64> = [(639, 480), (640, 480), (1920, 1079), (1920, 1080), (3999, 3000), (4000, 3000)].iter().map(|&(w, h)| key(GroupField::Dimensions, picture(w, h))).collect();
    assert_eq!(picture_groups, [3, 2, 2, 1, 1, 0]);
    let clip = |seconds: u64| ColumnValues { text: vec![String::new(), "x".into()], duration: Some(seconds * 10_000_000), ..Default::default() };
    let clip_groups: Vec<u64> = [299, 300, 1799, 1800].iter().map(|&s| key(GroupField::Duration, clip(s))).collect();
    assert_eq!(clip_groups, [2, 1, 1, 0]);
    let program = |v: &str| ColumnValues { text: vec![String::new(), String::new(), String::new(), v.into()], ..Default::default() };
    assert_eq!(key(GroupField::Version, program("10.0.19041.1")), 10);
    assert_eq!(key(GroupField::Version, program("9.9")), 9);
    for field in [GroupField::Dimensions, GroupField::Duration, GroupField::Artist, GroupField::Version] {
        assert_eq!(key(field, ColumnValues::default()), UNSPECIFIED, "{field:?} without a value");
    }
    assert_eq!(key(GroupField::Dimensions, picture(0, 480)), UNSPECIFIED);
}

#[test]
fn fast_sort_matches_comparator_for_shell_properties() {
    let (mut l, k) = synthetic(3000);
    for i in 0..l.len() as u32 {
        let r = u64::from(i).wrapping_mul(2654435761) >> 7;
        let values = ColumnValues {
            elf: false,
            text: vec![
                if r % 3 == 0 { String::new() } else { "d".into() },
                if r % 4 == 0 { String::new() } else { "t".into() },
                ["", "Abba", "abba", "Artist 10", "artist 9", "Ölbild long artist name"][(r % 6) as usize].into(),
                ["", "1.2", "10.0.1", "9.9.9", "10.0.1.2", "bad"][(r % 6) as usize].into(),
            ],
            width: (r % 3 != 0).then_some((r % 50) as u32 * 10),
            height: (r % 3 != 0).then_some((r % 7) as u32 * 100),
            duration: (r % 4 != 0).then_some((r % 40) * 600_000_000),
        };
        if let Some(key) = PropKey::new(&values, CaseMode::Insensitive) {
            l.props.insert(i, key);
        }
    }
    for field in [SortField::Dimensions, SortField::Duration, SortField::Artist, SortField::Version] {
        for group in [GroupField::None, GroupField::Dimensions, GroupField::Duration, GroupField::Artist, GroupField::Version] {
            for (descending, folders) in [(false, Folders::First), (true, Folders::First), (false, Folders::Mixed), (true, Folders::Last)] {
                let view = ViewSpec { sort: SortSpec { field, descending, folders }, group: GroupSpec { field: group, ..Default::default() }, show_hidden: true, ..Default::default() };
                let mut fast = Vec::new();
                sort(&l, &k, view, &mut fast);
                let mut slow: Vec<u32> = (0..l.len() as u32).filter(|&i| visible(&l, i, &view)).collect();
                slow.sort_by(|&a, &b| cmp(&l, &k, &view, a, b));
                assert_eq!(fast, slow, "{field:?} desc={descending} {folders:?} {group:?}");
            }
        }
    }
}
