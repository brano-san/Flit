use crate::listing::{attr, Entry, Listing};
use crate::name;
use std::cmp::Ordering;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Hash, serde::Serialize, serde::Deserialize)]
pub enum SortField {
    #[default]
    Name,
    Modified,
    Type,
    Size,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Hash, serde::Serialize, serde::Deserialize)]
pub struct SortSpec {
    pub field: SortField,
    pub descending: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Hash, serde::Serialize, serde::Deserialize)]
pub struct ViewSpec {
    pub sort: SortSpec,
    pub show_hidden: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Hash)]
pub enum CaseMode {
    #[default]
    Insensitive,
    Sensitive,
}

const SYMBOLS: u8 = 0x02;
const DIGITS: u8 = 0x48;
const LETTERS: u8 = 0x50;

#[derive(Clone, Debug)]
pub struct SortKeys {
    case: CaseMode,
    blob: Vec<u8>,
    offs: Vec<u32>,
    prefix: Vec<u64>,
}

impl SortKeys {
    pub fn new(case: CaseMode) -> Self {
        Self { case, blob: Vec::new(), offs: vec![0], prefix: Vec::new() }
    }

    pub fn case(&self) -> CaseMode {
        self.case
    }

    pub fn len(&self) -> usize {
        self.prefix.len()
    }

    pub fn is_empty(&self) -> bool {
        self.prefix.is_empty()
    }

    pub fn push(&mut self, name: &[u8]) {
        let start = self.blob.len();
        encode_key(name, self.case, &mut self.blob);
        self.offs.push(self.blob.len() as u32);
        let k = &self.blob[start..];
        let mut p = [0u8; 8];
        let n = k.len().min(8);
        p[..n].copy_from_slice(&k[..n]);
        self.prefix.push(u64::from_be_bytes(p));
    }

    #[inline]
    pub fn key(&self, i: u32) -> &[u8] {
        &self.blob[self.offs[i as usize] as usize..self.offs[i as usize + 1] as usize]
    }

    #[inline]
    fn cmp(&self, a: u32, b: u32) -> Ordering {
        self.prefix[a as usize]
            .cmp(&self.prefix[b as usize])
            .then_with(|| self.key(a).cmp(self.key(b)))
    }
}

#[inline]
fn symbol_rank(b: u8) -> u8 {
    match b {
        0..=0x2F => b,
        0x3A..=0x40 => b - 0x3A + 48,
        0x5B..=0x60 => b - 0x5B + 55,
        _ => b - 0x7B + 61,
    }
}

pub fn encode_key(name: &[u8], case: CaseMode, out: &mut Vec<u8>) {
    let sensitive = case == CaseMode::Sensitive;
    let mut i = 0;
    while i < name.len() {
        let b = name[i];
        if b.is_ascii_digit() {
            let start = i;
            while i < name.len() && name[i].is_ascii_digit() {
                i += 1;
            }
            let run = &name[start..i];
            let sig = &run[run.iter().position(|&d| d != b'0').unwrap_or(run.len())..];
            out.push(DIGITS);
            out.push(sig.len().min(255) as u8);
            out.extend_from_slice(sig);
        } else if b < 0x80 {
            i += 1;
            let lower = b.to_ascii_lowercase();
            out.push(if lower.is_ascii_lowercase() {
                if sensitive {
                    LETTERS + 2 * (lower - b'a') + b.is_ascii_uppercase() as u8
                } else {
                    LETTERS + (lower - b'a')
                }
            } else {
                SYMBOLS + symbol_rank(b)
            });
        } else {
            let (cp, n) = name::decode(&name[i..]);
            i += n;
            let cp = match char::from_u32(cp) {
                Some(c) if !sensitive => c.to_lowercase().next().map_or(cp, |l| l as u32),
                _ => cp,
            };
            name::push_cp(out, cp);
        }
    }
}

pub fn natural_cmp(a: &[u8], b: &[u8], case: CaseMode) -> Ordering {
    let (mut ka, mut kb) = (Vec::new(), Vec::new());
    encode_key(a, case, &mut ka);
    encode_key(b, case, &mut kb);
    ka.cmp(&kb).then_with(|| a.cmp(b))
}

#[inline]
fn ext_cmp(a: &[u8], b: &[u8]) -> Ordering {
    a.iter().map(u8::to_ascii_lowercase).cmp(b.iter().map(u8::to_ascii_lowercase))
}

#[inline]
pub fn visible(e: &Entry, show_hidden: bool) -> bool {
    e.attrs & attr::DELETED == 0 && (show_hidden || e.attrs & attr::HIDDEN == 0)
}

#[inline]
pub fn cmp(l: &Listing, k: &SortKeys, spec: SortSpec, a: u32, b: u32) -> Ordering {
    let (ea, eb) = (&l.entries[a as usize], &l.entries[b as usize]);
    let dirs = eb.is_dir().cmp(&ea.is_dir());
    if dirs != Ordering::Equal {
        return dirs;
    }
    let o = match spec.field {
        SortField::Name => Ordering::Equal,
        SortField::Size => ea.size.cmp(&eb.size),
        SortField::Modified => ea.modified.cmp(&eb.modified),
        SortField::Type => ext_cmp(l.ext(a), l.ext(b)),
    }
    .then_with(|| k.cmp(a, b))
    .then_with(|| l.name(a).cmp(l.name(b)));
    let o = if spec.descending { o.reverse() } else { o };
    o.then(a.cmp(&b))
}

#[derive(Clone, Copy)]
struct Item {
    group: u16,
    exact: bool,
    idx: u32,
    primary: u64,
    name: u64,
    name2: u64,
}

#[inline]
fn key_word(key: &[u8], from: usize) -> u64 {
    let mut p = [0u8; 8];
    if let Some(rest) = key.get(from..) {
        let n = rest.len().min(8);
        p[..n].copy_from_slice(&rest[..n]);
    }
    u64::from_be_bytes(p)
}

fn ext_prefix(ext: &[u8]) -> u64 {
    let mut p = [0u8; 8];
    for (d, s) in p.iter_mut().zip(ext) {
        *d = s.to_ascii_lowercase();
    }
    u64::from_be_bytes(p)
}

pub fn sort(l: &Listing, k: &SortKeys, view: ViewSpec, out: &mut Vec<u32>) {
    let _s = tracing::trace_span!("sort", n = l.len()).entered();
    let spec = view.sort;
    let inv = |x: u64| if spec.descending { !x } else { x };
    let mut items: Vec<Item> = (0..l.len() as u32)
        .filter(|&i| visible(&l.entries[i as usize], view.show_hidden))
        .map(|i| {
            let e = &l.entries[i as usize];
            let primary = match spec.field {
                SortField::Name => 0,
                SortField::Size => e.size,
                SortField::Modified => e.modified as u64 ^ (1 << 63),
                SortField::Type => ext_prefix(l.ext(i)),
            };
            let exact = spec.field != SortField::Type || l.ext(i).len() <= 8;
            let name2 = inv(key_word(k.key(i), 8));
            Item { group: !e.is_dir() as u16, exact, idx: i, primary: inv(primary), name: inv(k.prefix[i as usize]), name2 }
        })
        .collect();
    items.sort_unstable_by(|a, b| {
        (a.group, a.primary)
            .cmp(&(b.group, b.primary))
            .then_with(|| if a.exact && b.exact { (a.name, a.name2).cmp(&(b.name, b.name2)) } else { Ordering::Equal })
            .then_with(|| cmp(l, k, spec, a.idx, b.idx))
    });
    out.clear();
    out.extend(items.iter().map(|it| it.idx));
}

pub fn insert_pos(l: &Listing, k: &SortKeys, spec: SortSpec, order: &[u32], i: u32) -> usize {
    order.partition_point(|&p| cmp(l, k, spec, p, i) == Ordering::Less)
}

pub fn find_row(l: &Listing, k: &SortKeys, spec: SortSpec, order: &[u32], i: u32) -> Option<usize> {
    order.binary_search_by(|&p| cmp(l, k, spec, p, i)).ok()
}

#[cfg(test)]
mod tests {
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
}
