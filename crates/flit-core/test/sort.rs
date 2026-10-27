use super::*;
use crate::listing::Meta;

fn sorted(names: &[&str], case: CaseMode) -> Vec<String> {
    let mut v: Vec<&str> = names.to_vec();
    v.sort_by(|a, b| natural_cmp(a.as_bytes(), b.as_bytes(), case));
    v.into_iter().map(String::from).collect()
}

#[test]
fn strcmplogicalw_doc_order() {
    let want = [
        "2string", "3string", "20string", "st2ring", "st3ring", "st20ring", "string2", "string3",
        "string20",
    ];
    let mut shuffled = want;
    shuffled.reverse();
    shuffled.swap(1, 6);
    assert_eq!(sorted(&shuffled, CaseMode::Insensitive), want);
}

#[test]
fn numbers_and_classes() {
    assert_eq!(
        sorted(&["a10", "a9", "a01", "a1", "a", "a.txt", "a 1", "_x", "!y", "9", "B", "b"], CaseMode::Insensitive),
        ["!y", "_x", "9", "a", "a 1", "a.txt", "a01", "a1", "a9", "a10", "B", "b"]
    );
    assert_eq!(
        sorted(&["f100000000000000000000000", "f99999999999999999999999"], CaseMode::Insensitive),
        ["f99999999999999999999999", "f100000000000000000000000"]
    );
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
        k.push(l.name(idx));
    }
    (l, k)
}

#[test]
fn fast_sort_matches_comparator() {
    let (l, k) = synthetic(3000);
    let groups = [GroupField::None, GroupField::Name, GroupField::Type, GroupField::Size, GroupField::Modified, GroupField::Created, GroupField::Tag];
    for field in [SortField::Name, SortField::Size, SortField::Modified, SortField::Created, SortField::Type, SortField::Tag] {
        for (descending, folders) in [(false, Folders::First), (true, Folders::First), (false, Folders::Last), (true, Folders::Mixed)] {
            for (g, &group) in groups.iter().enumerate() {
                let unit = [DateUnit::Year, DateUnit::Month, DateUnit::Day][g % 3];
                let view = ViewSpec {
                    sort: SortSpec { field, descending, folders },
                    group: GroupSpec { field: group, descending: g % 2 == 1, unit },
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
    let by_name = labels(&v);
    assert_eq!(by_name[2], 2);
    assert_eq!(by_name[3], 1);
    assert_eq!(by_name[5], 0x100 + 'ä' as u64);
    v.group.field = GroupField::Size;
    assert_eq!(labels(&v), [1, 3, 7, 2, 0, 2]);
    v.group.field = GroupField::Type;
    assert_eq!(group_id(&l, 0, &v), group_id(&l, 1, &v), "extensions group without regard to case");
    assert_eq!(group_key(&l, 4, &v), (0, true));
    v.group = GroupSpec { field: GroupField::Modified, unit: DateUnit::Day, ..Default::default() };
    assert_ne!(group_key(&l, 0, &v), group_key(&l, 1, &v));
    v.utc_offset_min = -60;
    assert_eq!(group_key(&l, 0, &v), group_key(&l, 1, &v), "both fall on the same local day an hour west of UTC");
    v.group.unit = DateUnit::Month;
    assert_eq!(date_unit_value(l.entries[1].modified, &v), Some(2024 * 12 + 2));
    assert_eq!(date_unit_value(0, &v), None);
    v.group.unit = DateUnit::Year;
    assert_eq!(date_unit_value(l.entries[1].modified, &v), Some(2024));
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