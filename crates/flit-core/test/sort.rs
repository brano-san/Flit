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
    v.sort = SortSpec { field: SortField::Size, descending: true };
    sort(&l, &k, v, &mut o);
    assert_eq!(names(&l, &o), ["Dir10", "dir9", "b.txt", "a.rs", "c.md"]);
    v.sort = SortSpec { field: SortField::Type, descending: false };
    sort(&l, &k, v, &mut o);
    assert_eq!(names(&l, &o), ["dir9", "Dir10", "c.md", "a.rs", "b.txt"]);
}

#[test]
fn fast_sort_matches_comparator() {
    let exts = ["", "txt", "TXT", "longextension1", "longextension2", "LONGEXTENSIOn", "abcdefgh", "abcdefghi", "rs"];
    let mut l = Listing::default();
    let mut k = SortKeys::new(CaseMode::Insensitive);
    let mut seed = 7u64;
    for i in 0..3000u64 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let r = seed >> 33;
        let ext = exts[(r % exts.len() as u64) as usize];
        let name = format!("{}{}.{ext}", ["a", "B", "file", "File 0"][(r % 4) as usize], r % 50);
        let attrs = if r.is_multiple_of(7) { attr::DIR } else { 0 };
        let idx = l.push(name.as_bytes(), Meta { size: r % 5, modified: (r % 3) as i64 - 1, attrs, ..Default::default() });
        k.push(l.name(idx));
        let _ = i;
    }
    for field in [SortField::Name, SortField::Size, SortField::Modified, SortField::Type] {
        for descending in [false, true] {
            let view = ViewSpec { sort: SortSpec { field, descending }, show_hidden: true };
            let mut fast = Vec::new();
            sort(&l, &k, view, &mut fast);
            let mut slow: Vec<u32> = (0..l.len() as u32).collect();
            slow.sort_by(|&a, &b| cmp(&l, &k, view.sort, a, b));
            assert_eq!(fast, slow, "{field:?} desc={descending}");
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
    let pos = insert_pos(&l, &k, spec.sort, &o, i);
    o.insert(pos, i);
    assert_eq!(names(&l, &o), ["a", "b"]);
    assert_eq!(find_row(&l, &k, spec.sort, &o, i), Some(1));
    assert_eq!(find_row(&l, &k, spec.sort, &o, 1), None);
}
