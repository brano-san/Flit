use crate::index::{Order, VolumeIndex, KEPT_ATTRS, ROOT};
use crate::proto::{Sort, SortKey};
use crate::query::{glob, Filter, Pattern, Query, Scope};
use flit_core::{attr, Meta};
use flit_proto::listing::EntryBatch;
use memchr::memmem::Finder;
use rayon::prelude::*;
use std::cmp::Ordering;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering::Relaxed;

const MIN_CHUNK: usize = 4096;
const LIGHT_TASKS: usize = 16;
const SMALL: usize = 1 << 16;
pub const MAX_ROWS: u64 = 1024;
pub const MAX_GROUPED: u64 = 200_000;

pub type Cancel<'a> = &'a (dyn Fn() -> bool + Sync);

pub trait Visibility: Send + Sync {
    fn can_list(&self, v: &VolumeIndex, dir: u32) -> bool;
}

#[derive(Default, Clone)]
pub struct Engine {
    pub volumes: Vec<VolumeIndex>,
    pub version: u64,
}

pub fn order_of(key: SortKey) -> Order {
    match key {
        SortKey::Name => Order::Name,
        SortKey::Path => Order::Path,
        SortKey::Size => Order::Size,
        SortKey::Modified => Order::Modified,
    }
}

#[derive(Default)]
struct Scratch {
    chain: Vec<u32>,
    lower_path: Vec<u8>,
    path: Vec<u8>,
    keep: Vec<bool>,
}

fn has_ext(name: &[u8], list: &[Vec<u8>]) -> bool {
    list.iter().any(|e| match e.as_slice() {
        [] => memchr::memrchr(b'.', name).is_none_or(|p| p + 1 == name.len()),
        e => !e.contains(&b'.') && name.len() > e.len() && name.ends_with(e) && name[name.len() - e.len() - 1] == b'.',
    })
}

fn columnar(f: &Filter) -> bool {
    matches!(f, Filter::Size(..) | Filter::Modified(..))
}

struct Prepared<'q> {
    query: &'q Query,
    finders: Vec<Option<Finder<'q>>>,
    driver: Option<(usize, Finder<'q>, bool)>,
    tasks: usize,
}

impl<'q> Prepared<'q> {
    fn new(query: &'q Query) -> Self {
        let finders = query
            .terms
            .iter()
            .map(|t| match &t.filter {
                Filter::Match(_, Pattern::Text(s)) => Some(Finder::new(s.as_slice())),
                _ => None,
            })
            .collect();
        let driver = query.driver().map(|(k, lit)| (k, Finder::new(lit), matches!(query.terms[k].filter, Filter::Match(_, Pattern::Text(_)))));
        let heavy = query.terms.iter().any(|t| match &t.filter {
            Filter::Match(scope, pattern) => *scope == Scope::Path || matches!(pattern, Pattern::Regex(_)),
            Filter::Ext(_) => true,
            _ => false,
        });
        let tasks = if heavy { rayon::current_num_threads() * 4 } else { LIGHT_TASKS };
        Self { query, finders, driver, tasks }
    }

    fn pattern(&self, k: usize, p: &Pattern, lower: &[u8], original: &[u8]) -> bool {
        match p {
            Pattern::Text(_) => self.finders[k].as_ref().is_some_and(|f| f.find(lower).is_some()),
            Pattern::Glob(g) => glob(g, lower),
            Pattern::Regex(re) => re.is_match(original),
        }
    }

    fn filter_range(&self, v: &VolumeIndex, a: usize, b: usize, sc: &mut Scratch) -> Vec<u32> {
        let attrs = &v.attrs[a..b];
        sc.keep.clear();
        sc.keep.extend(attrs.iter().map(|&x| x & (attr::DELETED | ROOT) == 0));
        for t in &self.query.terms {
            match t.filter {
                Filter::Size(lo, hi) => {
                    for ((k, &s), &x) in sc.keep.iter_mut().zip(&v.sizes[a..b]).zip(attrs) {
                        *k &= (x & attr::DIR == 0 && lo <= s && s < hi) != t.negate;
                    }
                }
                Filter::Modified(lo, hi) => {
                    for (k, &m) in sc.keep.iter_mut().zip(&v.modified[a..b]) {
                        *k &= (lo <= m && m < hi) != t.negate;
                    }
                }
                _ => {}
            }
        }
        let rest = self.query.terms.iter().any(|t| !columnar(&t.filter));
        let mut out = vec![0u32; b - a];
        let mut n = 0;
        if rest {
            let keep = std::mem::take(&mut sc.keep);
            for (j, _) in keep.iter().enumerate().filter(|(_, &k)| k) {
                let i = (a + j) as u32;
                if self.matches(v, i, None, true, sc) {
                    out[n] = i;
                    n += 1;
                }
            }
            sc.keep = keep;
        } else {
            for (j, &k) in sc.keep.iter().enumerate() {
                out[n] = (a + j) as u32;
                n += k as usize;
            }
        }
        out.truncate(n);
        out
    }

    fn matches(&self, v: &VolumeIndex, i: u32, skip: Option<usize>, columns_done: bool, sc: &mut Scratch) -> bool {
        let (mut lower_built, mut path_built) = (false, false);
        let k_ = i as usize;
        for (k, t) in self.query.terms.iter().enumerate() {
            if Some(k) == skip || (columns_done && columnar(&t.filter)) {
                continue;
            }
            let ok = match &t.filter {
                Filter::Match(Scope::Name, p) => self.pattern(k, p, v.lower_name(i), v.name(i)),
                Filter::Match(Scope::Path, p) => {
                    if matches!(p, Pattern::Regex(_)) {
                        if !path_built {
                            sc.path.clear();
                            v.path_into(i, false, &mut sc.chain, &mut sc.path);
                            path_built = true;
                        }
                    } else if !lower_built {
                        sc.lower_path.clear();
                        v.path_into(i, true, &mut sc.chain, &mut sc.lower_path);
                        lower_built = true;
                    }
                    self.pattern(k, p, &sc.lower_path, &sc.path)
                }
                Filter::Ext(list) => !v.is_dir(i) && has_ext(v.lower_name(i), list),
                Filter::Size(lo, hi) => !v.is_dir(i) && (*lo..*hi).contains(&v.sizes[k_]),
                Filter::Modified(lo, hi) => (*lo..*hi).contains(&v.modified[k_]),
                Filter::Tag(_) => false,
            };
            if ok == t.negate {
                return false;
            }
        }
        true
    }
}

fn segment_at(starts: &[u32], from: usize, at: u32) -> usize {
    let mut s = from;
    for _ in 0..8 {
        match starts.get(s + 1) {
            Some(&next) if next <= at => s += 1,
            _ => return s,
        }
    }
    s + starts[s + 1..].partition_point(|&st| st <= at)
}

fn chunks(n: usize, tasks: usize) -> Vec<(usize, usize)> {
    let size = (n / tasks.max(1)).max(MIN_CHUNK);
    (0..n).step_by(size).map(|a| (a, (a + size).min(n))).collect()
}

struct Found {
    hits: Vec<u32>,
    bits: Vec<u64>,
    ranks: (u32, u32),
}

fn mark(part: Vec<u32>, rank: &[u32], bits: &[AtomicU64]) -> (Vec<u32>, u32, u32) {
    let (mut lo, mut hi) = (u32::MAX, 0);
    for &e in &part {
        bits[e as usize / 64].fetch_or(1 << (e % 64), Relaxed);
        lo = lo.min(rank[e as usize]);
        hi = hi.max(rank[e as usize]);
    }
    (part, lo, hi)
}

fn scan(v: &VolumeIndex, p: &Prepared<'_>, rank: &[u32], access: Option<&dyn Visibility>, cancel: Cancel<'_>) -> Option<Found> {
    let bits: Vec<AtomicU64> = (0..v.len().div_ceil(64)).map(|_| AtomicU64::new(0)).collect();
    let visible = |i: u32| access.is_none_or(|a| a.can_list(v, v.parents[i as usize]));
    let parts: Vec<Option<(Vec<u32>, u32, u32)>> = match &p.driver {
        Some((k, finder, exact)) => {
            let skip = exact.then_some(*k);
            let ns = v.seg_start.len();
            chunks(ns, p.tasks)
                .par_iter()
                .map_init(Scratch::default, |sc, &(a, b)| {
                    if cancel() {
                        return None;
                    }
                    let starts = &v.seg_start[a..b];
                    let lo = starts[0] as usize;
                    let hi = if b < ns { v.seg_start[b] as usize } else { v.lower.len() };
                    let mut out = Vec::new();
                    let (mut s, mut end) = (0, 0u32);
                    for pos in finder.find_iter(&v.lower[lo..hi]) {
                        let at = (lo + pos) as u32;
                        if at < end {
                            continue;
                        }
                        s = segment_at(starts, s, at);
                        end = starts.get(s + 1).copied().unwrap_or(u32::MAX);
                        let e = v.seg_entry[a + s];
                        if v.lower_off[e as usize] == starts[s] && v.listed(e) && p.matches(v, e, skip, false, sc) && visible(e) {
                            out.push(e);
                        }
                    }
                    Some(mark(out, rank, &bits))
                })
                .collect()
        }
        None => chunks(v.len(), p.tasks)
            .par_iter()
            .map_init(Scratch::default, |sc, &(a, b)| {
                if cancel() {
                    return None;
                }
                let mut hits = p.filter_range(v, a, b, sc);
                if access.is_some() {
                    hits.retain(|&i| visible(i));
                }
                Some(mark(hits, rank, &bits))
            })
            .collect(),
    };
    let mut found = Found { hits: Vec::with_capacity(parts.iter().flatten().map(|p| p.0.len()).sum()), bits: Vec::new(), ranks: (u32::MAX, 0) };
    for part in parts {
        let (hits, lo, hi) = part?;
        found.hits.extend(hits);
        found.ranks = (found.ranks.0.min(lo), found.ranks.1.max(hi));
    }
    found.bits = bits.into_iter().map(AtomicU64::into_inner).collect();
    Some(found)
}

enum Kind {
    List(Vec<u32>),
    Perm(Option<Vec<u64>>),
}

struct Stream {
    vol: usize,
    kind: Kind,
    pos: usize,
}

impl Stream {
    fn peek(&mut self, v: &VolumeIndex, order: Order, descending: bool) -> Option<u32> {
        match &self.kind {
            Kind::List(l) => l.get(self.pos).copied(),
            Kind::Perm(bits) => {
                let perm = &v.perms[order as usize];
                while self.pos < perm.len() {
                    let i = if descending { perm[perm.len() - 1 - self.pos] } else { perm[self.pos] };
                    if bits.as_ref().is_none_or(|b| b.get(i as usize / 64).is_some_and(|w| w & (1 << (i % 64)) != 0)) {
                        return Some(i);
                    }
                    self.pos += 1;
                }
                None
            }
        }
    }
}

pub struct ResultSet {
    pub total: u64,
    pub sort: Sort,
    streams: Vec<Stream>,
    rows: Vec<(u32, u32)>,
}

impl ResultSet {
    pub fn materialized(&self) -> usize {
        self.rows.len()
    }

    fn ensure(&mut self, e: &Engine, to: usize) {
        let order = order_of(self.sort.key);
        let desc = self.sort.descending;
        while self.rows.len() < to {
            let mut best: Option<(usize, u32)> = None;
            for k in 0..self.streams.len() {
                let vol = self.streams[k].vol;
                let Some(i) = self.streams[k].peek(&e.volumes[vol], order, desc) else { continue };
                let better = match best {
                    None => true,
                    Some((bk, bi)) => {
                        let c = e.cmp(order, (vol, i), (self.streams[bk].vol, bi));
                        if desc { c == Ordering::Greater } else { c == Ordering::Less }
                    }
                };
                if better {
                    best = Some((k, i));
                }
            }
            let Some((k, i)) = best else { break };
            self.rows.push((self.streams[k].vol as u32, i));
            self.streams[k].pos += 1;
        }
    }
}

impl Engine {
    pub fn total(&self) -> u64 {
        self.volumes.iter().map(|v| v.live() as u64).sum()
    }

    fn cmp(&self, order: Order, (va, a): (usize, u32), (vb, b): (usize, u32)) -> Ordering {
        let (x, y) = (&self.volumes[va], &self.volumes[vb]);
        let by_name = || x.lower_name(a).cmp(y.lower_name(b)).then_with(|| x.name(a).cmp(y.name(b))).then(va.cmp(&vb)).then(a.cmp(&b));
        match order {
            Order::Name => by_name(),
            Order::Path => x.root_lower.cmp(&y.root_lower).then(va.cmp(&vb)).then(x.ranks[1][a as usize].cmp(&y.ranks[1][b as usize])),
            Order::Size => x.sizes[a as usize].cmp(&y.sizes[b as usize]).then_with(by_name),
            Order::Modified => x.modified[a as usize].cmp(&y.modified[b as usize]).then_with(by_name),
        }
    }

    pub fn search(&self, query: &Query, sort: Sort, cancel: Cancel<'_>) -> Option<ResultSet> {
        self.search_as(query, sort, None, cancel)
    }

    pub fn search_as(&self, query: &Query, sort: Sort, access: Option<&dyn Visibility>, cancel: Cancel<'_>) -> Option<ResultSet> {
        let order = order_of(sort.key);
        let mut streams = Vec::new();
        let mut total = 0u64;
        let p = Prepared::new(query);
        for (vol, v) in self.volumes.iter().enumerate() {
            let (kind, pos) = if query.terms.is_empty() && access.is_none() {
                total += v.perms[order as usize].len() as u64;
                (Kind::Perm(None), 0)
            } else {
                let rank = &v.ranks[order as usize];
                let Found { mut hits, bits, ranks } = scan(v, &p, rank, access, cancel)?;
                total += hits.len() as u64;
                if hits.len() <= SMALL {
                    hits.par_sort_unstable_by_key(|&i| rank[i as usize]);
                    if sort.descending {
                        hits.reverse();
                    }
                    (Kind::List(hits), 0)
                } else {
                    let n = v.perms[order as usize].len();
                    let first = match ranks {
                        (_, hi) if hi as usize >= n => 0,
                        (_, hi) if sort.descending => n - 1 - hi as usize,
                        (lo, _) => lo as usize,
                    };
                    (Kind::Perm(Some(bits)), first)
                }
            };
            streams.push(Stream { vol, kind, pos });
        }
        if cancel() {
            return None;
        }
        let mut rs = ResultSet { total, sort, streams, rows: Vec::new() };
        rs.ensure(self, 1);
        Some(rs)
    }

    pub fn rows(&self, rs: &mut ResultSet, from: u64, to: u64) -> (EntryBatch, Vec<Vec<u8>>) {
        let to = to.min(from.saturating_add(MAX_ROWS)).min(rs.total);
        let mut batch = EntryBatch::default();
        let mut dirs = Vec::new();
        if from >= to {
            return (batch, dirs);
        }
        rs.ensure(self, to as usize);
        let mut chain = Vec::new();
        let end = (to as usize).min(rs.rows.len());
        for &(vol, i) in rs.rows.get(from as usize..end).unwrap_or(&[]) {
            let v = &self.volumes[vol as usize];
            let k = i as usize;
            batch.push(v.name(i), Meta { size: v.sizes[k], modified: v.modified[k], created: 0, attrs: v.attrs[k] & KEPT_ATTRS });
            let mut dir = Vec::new();
            v.dir_into(i, &mut chain, &mut dir);
            dirs.push(dir);
        }
        (batch, dirs)
    }

    pub fn groups(&self, rs: &mut ResultSet) -> Option<Vec<u64>> {
        if rs.sort.key != SortKey::Path || rs.total > MAX_GROUPED || rs.total == 0 {
            return None;
        }
        rs.ensure(self, rs.total as usize);
        let parent = |&(vol, i): &(u32, u32)| (vol, self.volumes[vol as usize].parents[i as usize]);
        let mut starts = vec![0];
        starts.extend(rs.rows.windows(2).enumerate().filter(|(_, w)| parent(&w[0]) != parent(&w[1])).map(|(k, _)| k as u64 + 1));
        Some(starts)
    }

    pub fn volume_mut(&mut self, id: &str) -> Option<&mut VolumeIndex> {
        self.volumes.iter_mut().find(|v| v.id == id)
    }
}

#[cfg(test)]
#[path = "../test/engine.rs"]
mod tests;
