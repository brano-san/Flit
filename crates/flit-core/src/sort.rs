use crate::hash::FxHasher;
use crate::listing::{attr, Entry, Listing};
use crate::{fmt, name, recycle};
use std::cmp::Ordering;
use std::hash::Hasher;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Hash, serde::Serialize, serde::Deserialize)]
pub enum SortField {
    #[default]
    Name,
    Modified,
    Type,
    Size,
    Origin,
    Created,
    Tag,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Hash, serde::Serialize, serde::Deserialize)]
pub enum Folders {
    #[default]
    First,
    Last,
    Mixed,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Hash, serde::Serialize, serde::Deserialize)]
pub struct SortSpec {
    pub field: SortField,
    pub descending: bool,
    pub folders: Folders,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Hash, serde::Serialize, serde::Deserialize)]
pub enum GroupField {
    #[default]
    None,
    Name,
    Type,
    Size,
    Modified,
    Created,
    Origin,
    Tag,
}

impl GroupField {
    pub fn is_date(self) -> bool {
        matches!(self, Self::Modified | Self::Created)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Hash, serde::Serialize, serde::Deserialize)]
pub enum DateUnit {
    Year,
    #[default]
    Month,
    Day,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Hash, serde::Serialize, serde::Deserialize)]
pub struct GroupSpec {
    pub field: GroupField,
    pub descending: bool,
    pub unit: DateUnit,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Hash)]
pub struct ViewSpec {
    pub sort: SortSpec,
    pub group: GroupSpec,
    pub show_hidden: bool,
    pub show_system: bool,
    pub hide_dotfiles: bool,
    pub show_streams: bool,
    pub utc_offset_min: i32,
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
const SIZE_LIMITS: [u64; 5] = [16 << 10, 1 << 20, 128 << 20, 1 << 30, 4 << 30];

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
pub fn visible(l: &Listing, i: u32, v: &ViewSpec) -> bool {
    let a = l.entries[i as usize].attrs;
    a & attr::DELETED == 0
        && (a & attr::HIDDEN == 0 || (v.show_hidden && (v.show_system || a & attr::SYSTEM == 0)))
        && !(v.hide_dotfiles && l.name(i).first() == Some(&b'.'))
        && (a & attr::STREAM == 0 || v.show_streams)
}

fn initial(name: &[u8]) -> u64 {
    if name.is_empty() {
        return 1;
    }
    match char::from_u32(name::decode(name).0) {
        Some(c) if c.is_ascii_digit() => 2,
        Some(c) if c.is_alphabetic() => 0x100 + c.to_lowercase().next().unwrap_or(c) as u64,
        _ => 1,
    }
}

pub fn size_bucket(e: &Entry) -> u64 {
    if e.is_dir() && e.attrs & attr::SIZE_KNOWN == 0 {
        return 0;
    }
    if e.size == 0 {
        return 1;
    }
    2 + SIZE_LIMITS.iter().position(|&lim| e.size < lim).unwrap_or(SIZE_LIMITS.len()) as u64
}

pub fn date_unit_value(ticks: i64, v: &ViewSpec) -> Option<i64> {
    if ticks <= 0 {
        return None;
    }
    let days = fmt::local_days(ticks, v.utc_offset_min);
    Some(match v.group.unit {
        DateUnit::Day => days,
        unit => {
            let (y, m, _) = fmt::civil_from_days(days);
            if unit == DateUnit::Year { y } else { y * 12 + m as i64 - 1 }
        }
    })
}

pub fn first_tag(mask: u32) -> u64 {
    if mask == 0 { 32 } else { mask.trailing_zeros() as u64 }
}

#[inline]
fn tag_key(mask: u32) -> u64 {
    (!mask).reverse_bits() as u64
}

fn date_key(ticks: i64, v: &ViewSpec) -> u64 {
    date_unit_value(ticks, v).map_or(0, |n| n as u64 ^ (1 << 63))
}

pub fn group_key(l: &Listing, i: u32, v: &ViewSpec) -> (u64, bool) {
    let e = &l.entries[i as usize];
    match v.group.field {
        GroupField::None => (0, true),
        GroupField::Name => (initial(l.name(i)), true),
        GroupField::Type if e.is_dir() => (0, true),
        GroupField::Type => (ext_prefix(l.ext(i)).saturating_add(1), l.ext(i).len() <= 8),
        GroupField::Size => (size_bucket(e), true),
        GroupField::Modified => (date_key(e.modified, v), true),
        GroupField::Created => (date_key(e.created, v), true),
        GroupField::Origin => (ext_prefix(recycle::origin(l, i)), recycle::origin(l, i).len() <= 8),
        GroupField::Tag => (first_tag(e.tags), true),
    }
}

pub fn group_bytes(l: &Listing, i: u32, f: GroupField) -> &[u8] {
    match f {
        GroupField::Type => l.ext(i),
        GroupField::Origin => recycle::origin(l, i),
        _ => &[],
    }
}

pub fn group_cmp(l: &Listing, v: &ViewSpec, a: u32, b: u32) -> Ordering {
    if v.group.field == GroupField::None {
        return Ordering::Equal;
    }
    let o = group_key(l, a, v).0.cmp(&group_key(l, b, v).0);
    let o = if v.group.descending { o.reverse() } else { o };
    o.then_with(|| group_tail_cmp(l, v, a, b))
}

#[inline]
fn group_tail_cmp(l: &Listing, v: &ViewSpec, a: u32, b: u32) -> Ordering {
    let f = v.group.field;
    let o = ext_cmp(group_bytes(l, a, f), group_bytes(l, b, f));
    if v.group.descending { o.reverse() } else { o }
}

pub fn group_id(l: &Listing, i: u32, v: &ViewSpec) -> u64 {
    let mut h = FxHasher::default();
    h.write_u64(group_key(l, i, v).0);
    for b in group_bytes(l, i, v.group.field) {
        h.write_u8(b.to_ascii_lowercase());
    }
    h.finish()
}

#[inline]
pub fn cmp(l: &Listing, k: &SortKeys, v: &ViewSpec, a: u32, b: u32) -> Ordering {
    group_cmp(l, v, a, b).then_with(|| item_cmp(l, k, v.sort, a, b))
}

#[inline]
fn item_cmp(l: &Listing, k: &SortKeys, spec: SortSpec, a: u32, b: u32) -> Ordering {
    let (ea, eb) = (&l.entries[a as usize], &l.entries[b as usize]);
    let dirs = match spec.folders {
        Folders::First => eb.is_dir().cmp(&ea.is_dir()),
        Folders::Last => ea.is_dir().cmp(&eb.is_dir()),
        Folders::Mixed => Ordering::Equal,
    };
    if dirs != Ordering::Equal {
        return dirs;
    }
    let o = match spec.field {
        SortField::Name => Ordering::Equal,
        SortField::Size => ea.size.cmp(&eb.size),
        SortField::Modified => ea.modified.cmp(&eb.modified),
        SortField::Created => ea.created.cmp(&eb.created),
        SortField::Type => ext_cmp(l.ext(a), l.ext(b)),
        SortField::Origin => ext_cmp(recycle::origin(l, a), recycle::origin(l, b)),
        SortField::Tag => tag_key(ea.tags).cmp(&tag_key(eb.tags)),
    }
    .then_with(|| k.cmp(a, b))
    .then_with(|| l.name(a).cmp(l.name(b)));
    let o = if spec.descending { o.reverse() } else { o };
    o.then(a.cmp(&b))
}

#[derive(Clone, Copy)]
struct Item {
    gkey: u64,
    primary: u64,
    name: u64,
    name2: u64,
    idx: u32,
    group: u16,
    exact: bool,
    gexact: bool,
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
    let ginv = |x: u64| if view.group.descending { !x } else { x };
    let mut items: Vec<Item> = (0..l.len() as u32)
        .filter(|&i| visible(l, i, &view))
        .map(|i| {
            let e = &l.entries[i as usize];
            let primary = match spec.field {
                SortField::Name => 0,
                SortField::Size => e.size,
                SortField::Modified => e.modified as u64 ^ (1 << 63),
                SortField::Created => e.created as u64 ^ (1 << 63),
                SortField::Type => ext_prefix(l.ext(i)),
                SortField::Origin => ext_prefix(recycle::origin(l, i)),
                SortField::Tag => tag_key(e.tags),
            };
            let exact = match spec.field {
                SortField::Type => l.ext(i).len() <= 8,
                SortField::Origin => recycle::origin(l, i).len() <= 8,
                _ => true,
            };
            let group = match spec.folders {
                Folders::First => !e.is_dir() as u16,
                Folders::Last => e.is_dir() as u16,
                Folders::Mixed => 0,
            };
            let (gkey, gexact) = group_key(l, i, &view);
            let name2 = inv(key_word(k.key(i), 8));
            Item { gkey: ginv(gkey), primary: inv(primary), name: inv(k.prefix[i as usize]), name2, idx: i, group, exact, gexact }
        })
        .collect();
    items.sort_unstable_by(|a, b| {
        a.gkey
            .cmp(&b.gkey)
            .then_with(|| if a.gexact && b.gexact { Ordering::Equal } else { group_tail_cmp(l, &view, a.idx, b.idx) })
            .then_with(|| (a.group, a.primary).cmp(&(b.group, b.primary)))
            .then_with(|| if a.exact && b.exact { (a.name, a.name2).cmp(&(b.name, b.name2)) } else { Ordering::Equal })
            .then_with(|| item_cmp(l, k, spec, a.idx, b.idx))
    });
    out.clear();
    out.extend(items.iter().map(|it| it.idx));
}

pub fn insert_pos(l: &Listing, k: &SortKeys, v: &ViewSpec, order: &[u32], i: u32) -> usize {
    order.partition_point(|&p| cmp(l, k, v, p, i) == Ordering::Less)
}

pub fn find_row(l: &Listing, k: &SortKeys, v: &ViewSpec, order: &[u32], i: u32) -> Option<usize> {
    order.binary_search_by(|&p| cmp(l, k, v, p, i)).ok()
}

#[cfg(test)]
#[path = "../test/sort.rs"]
mod tests;
