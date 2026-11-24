use crate::index::{lowercase, Order, VolumeIndex, KEPT_ATTRS, MAX_DEPTH, NONE, ROOT};
use crate::proto::{Sort, SortKey};
use crate::query::{glob, Filter, Pattern, Query, Scope};
use flit_core::hash::FxHashMap;
use flit_core::{attr, query_attrs::SearchPaths, FsPath, Meta};
use flit_proto::listing::EntryBatch;
use memchr::memmem::Finder;
use rayon::prelude::*;
use std::cmp::Ordering;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::atomic::{AtomicU64, AtomicU8};

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
    tail: Vec<u8>,
    keep: Vec<bool>,
}

const UNKNOWN: u8 = 0;
const KNOWN: u8 = 1;
const HIT: u8 = 2;
const ENDS: u8 = 4;

fn has_ext(name: &[u8], list: &[Vec<u8>]) -> bool {
    list.iter().any(|e| match e.as_slice() {
        [] => memchr::memrchr(b'.', name).is_none_or(|p| p + 1 == name.len()),
        e => !e.contains(&b'.') && name.len() > e.len() && name.ends_with(e) && name[name.len() - e.len() - 1] == b'.',
    })
}

fn columnar(f: &Filter) -> bool {
    matches!(f, Filter::Size(..) | Filter::Modified(..))
}

struct FilterMemo<'a> {
    paths: &'a [Vec<AtomicU8>],
    scope: &'a PathScope,
    children: &'a [u32],
    depth: bool,
}

enum Parent {
    Any,
    Root,
    Exact(Vec<u32>),
}

struct PathScope {
    parent: Parent,
    under: Option<Ancestors>,
    excluded: Ancestors,
}

#[derive(Default)]
struct Ancestors(Vec<AtomicU8>);

fn same_path(path: &FsPath, left: &[u8], right: &[u8]) -> bool {
    if path.case_sensitive() {
        left == right
    } else {
        left.eq_ignore_ascii_case(right)
    }
}

fn on_volume(v: &VolumeIndex, path: &FsPath) -> bool {
    let root = v.root.strip_suffix(&[v.sep]).unwrap_or(&v.root);
    path.sep() == v.sep && path.as_bytes().get(..root.len()).is_some_and(|prefix| same_path(path, root, prefix) && path.as_bytes().get(root.len()).is_none_or(|&b| b == v.sep))
}

fn at_volume_root(v: &VolumeIndex, path: &FsPath) -> bool {
    let root = v.root.strip_suffix(&[v.sep]).unwrap_or(&v.root);
    let bytes = path.as_bytes();
    path.sep() == v.sep && same_path(path, root, bytes.strip_suffix(&[v.sep]).unwrap_or(bytes))
}

fn path_ids(v: &VolumeIndex, path: &FsPath, cancel: Cancel<'_>) -> Option<Vec<u32>> {
    let name = lowercase(path.file_name());
    let perm = &v.perms[Order::Name as usize];
    let start = perm.partition_point(|&i| v.lower_name(i) < name.as_slice());
    let root = v.root.strip_suffix(&[v.sep]).unwrap_or(&v.root);
    let components: Vec<_> = path.as_bytes()[root.len()..].rsplit(|&b| b == v.sep).filter(|c| !c.is_empty()).collect();
    let matches = |mut i| {
        for component in &components {
            if at_root(v, i) || !same_path(path, v.name(i), component) {
                return false;
            }
            i = v.parents[i as usize];
        }
        at_root(v, i)
    };
    let mut ids = Vec::new();
    for (n, &i) in perm[start..].iter().take_while(|&&i| v.lower_name(i) == name).enumerate() {
        if n.is_multiple_of(MIN_CHUNK) && cancel() {
            return None;
        }
        if matches(i) {
            ids.push(i);
        }
    }
    Some(ids)
}

impl PathScope {
    fn resolve(v: &VolumeIndex, paths: &SearchPaths, cancel: Cancel<'_>) -> Option<Self> {
        let skipped = || Self { parent: Parent::Exact(Vec::new()), under: None, excluded: Ancestors::default() };
        if paths.excludes_branch(&v.root) {
            return Some(skipped());
        }
        let parent = match &paths.parent {
            None => Parent::Any,
            Some(path) if !on_volume(v, path) => return Some(skipped()),
            Some(path) if at_volume_root(v, path) => Parent::Root,
            Some(path) => Parent::Exact(path_ids(v, path, cancel)?),
        };
        if matches!(&parent, Parent::Exact(ids) if ids.is_empty()) {
            return Some(skipped());
        }
        let under = if let Some(path) = &paths.under {
            if at_volume_root(v, path) || FsPath::parse_exact(&v.root).is_some_and(|root| root.is_inside(path)) {
                None
            } else {
                if !on_volume(v, path) {
                    return Some(skipped());
                }
                let ids = path_ids(v, path, cancel)?;
                if ids.is_empty() {
                    return Some(skipped());
                }
                Some(Ancestors::new(v.len(), ids))
            }
        } else {
            None
        };
        let mut ids = Vec::new();
        for path in paths.exclude.iter().filter(|path| on_volume(v, path)) {
            ids.extend(path_ids(v, path, cancel)?);
        }
        Some(Self { parent, under, excluded: Ancestors::new(v.len(), ids) })
    }

    fn skips_volume(&self) -> bool {
        matches!(&self.parent, Parent::Exact(ids) if ids.is_empty())
    }

    fn is_empty(&self) -> bool {
        matches!(self.parent, Parent::Any) && self.under.is_none() && self.excluded.0.is_empty()
    }

    fn rejects(&self, v: &VolumeIndex, i: u32) -> bool {
        let parent = v.parents[i as usize];
        match &self.parent {
            Parent::Any => {}
            Parent::Root if at_root(v, parent) => {}
            Parent::Exact(ids) if ids.contains(&parent) => {}
            _ => return true,
        }
        self.under.as_ref().is_some_and(|scope| !scope.contains(v, parent)) || self.excluded.contains(v, i)
    }
}

fn at_root(v: &VolumeIndex, i: u32) -> bool {
    i == NONE || v.attrs[i as usize] & ROOT != 0
}

impl Ancestors {
    fn new(len: usize, ids: Vec<u32>) -> Self {
        if ids.is_empty() {
            return Self::default();
        }
        let state: Vec<_> = (0..len).map(|_| AtomicU8::new(UNKNOWN)).collect();
        for i in ids {
            state[i as usize].store(KNOWN | HIT, Relaxed);
        }
        Self(state)
    }

    fn contains(&self, v: &VolumeIndex, i: u32) -> bool {
        if self.0.is_empty() {
            return false;
        }
        let mut ancestor = i;
        let mut state = UNKNOWN;
        for _ in 0..MAX_DEPTH {
            if at_root(v, ancestor) {
                state = KNOWN;
                break;
            }
            state = self.0[ancestor as usize].load(Relaxed);
            if state != UNKNOWN {
                break;
            }
            ancestor = v.parents[ancestor as usize];
        }
        if state != UNKNOWN {
            let mut cur = i;
            while cur != ancestor {
                if v.is_dir(cur) {
                    self.0[cur as usize].store(state, Relaxed);
                }
                cur = v.parents[cur as usize];
            }
        }
        state & HIT != 0
    }
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

    fn path_state(&self, k: usize, v: &VolumeIndex, i: u32, dirs: &[AtomicU8], dir: bool, sc: &mut Scratch) -> u8 {
        let Some(finder) = &self.finders[k] else { return KNOWN };
        let needle = finder.needle();
        let last_sep = memchr::memrchr(v.sep, needle);
        let parent = match v.parents[i as usize] {
            _ if v.attrs[i as usize] & ROOT != 0 => UNKNOWN,
            NONE => UNKNOWN,
            p if v.attrs[p as usize] & ROOT != 0 => UNKNOWN,
            p => dirs.get(p as usize).map_or(UNKNOWN, |s| s.load(Relaxed)),
        };
        let name = v.lower_name(i);
        let hit = match (parent, last_sep) {
            (UNKNOWN, _) => {
                sc.tail.clear();
                v.path_into(i, true, &mut sc.chain, &mut sc.tail);
                finder.find(&sc.tail).is_some()
            }
            (s, _) if s & HIT != 0 => true,
            (_, None) => finder.find(name).is_some(),
            (s, Some(at)) => s & ENDS != 0 && name.starts_with(&needle[at + 1..]),
        };
        let ends = dir
            && last_sep.is_some_and(|at| {
                sc.tail.clear();
                v.path_tail_into(i, true, at, &mut sc.chain, &mut sc.tail);
                sc.tail.ends_with(&needle[..at])
            });
        KNOWN | if hit { HIT } else { 0 } | if ends { ENDS } else { 0 }
    }

    fn dir_states(&self, k: usize, v: &VolumeIndex) -> Vec<AtomicU8> {
        let state: Vec<AtomicU8> = (0..v.len()).map(|_| AtomicU8::new(UNKNOWN)).collect();
        let known = |i: u32| state[i as usize].load(Relaxed) & KNOWN != 0;
        chunks(v.len(), self.tasks).into_par_iter().for_each_init(
            || (Vec::new(), Scratch::default()),
            |(stack, sc), (a, b)| {
                for d in a as u32..b as u32 {
                    if !v.is_dir(d) || known(d) {
                        continue;
                    }
                    stack.clear();
                    let mut cur = d;
                    loop {
                        stack.push(cur);
                        let p = v.parents[cur as usize];
                        if p == NONE || v.attrs[cur as usize] & ROOT != 0 || !v.is_dir(p) || known(p) || stack.len() > MAX_DEPTH {
                            break;
                        }
                        cur = p;
                    }
                    for &c in stack.iter().rev() {
                        state[c as usize].store(self.path_state(k, v, c, &state, true, sc), Relaxed);
                    }
                }
            },
        );
        state
    }

    fn path_memo(&self, v: &VolumeIndex) -> Vec<Vec<AtomicU8>> {
        self.query
            .terms
            .iter()
            .enumerate()
            .map(|(k, t)| match &t.filter {
                Filter::Match(Scope::Path, Pattern::Text(_)) if self.driver.is_none() => self.dir_states(k, v),
                _ => Vec::new(),
            })
            .collect()
    }

    fn filter_range(&self, v: &VolumeIndex, a: usize, b: usize, memo: &FilterMemo<'_>, sc: &mut Scratch) -> Vec<u32> {
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
        let rest = !memo.scope.is_empty() || self.query.terms.iter().any(|t| !columnar(&t.filter));
        let mut out = vec![0u32; b - a];
        let mut n = 0;
        if rest {
            let keep = std::mem::take(&mut sc.keep);
            for (j, _) in keep.iter().enumerate().filter(|(_, &k)| k) {
                let i = (a + j) as u32;
                if self.matches(v, i, None, true, memo, sc) {
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

    fn matches(&self, v: &VolumeIndex, i: u32, skip: Option<usize>, columns_done: bool, memo: &FilterMemo<'_>, sc: &mut Scratch) -> bool {
        if !memo.scope.is_empty() && memo.scope.rejects(v, i) {
            return false;
        }
        let (mut lower_built, mut path_built) = (false, false);
        let k_ = i as usize;
        for (k, t) in self.query.terms.iter().enumerate() {
            if Some(k) == skip || (columns_done && columnar(&t.filter)) {
                continue;
            }
            let ok = match &t.filter {
                Filter::Match(Scope::Name, p) => self.pattern(k, p, v.lower_name(i), v.name(i)),
                Filter::Match(Scope::Path, Pattern::Text(_)) => self.path_state(k, v, i, &memo.paths[k], false, sc) & HIT != 0,
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
                Filter::Tag(_) | Filter::Rating(..) | Filter::Comment(_) | Filter::PortableTag(_) => false,
                Filter::Directory(dir) => v.is_dir(i) == *dir,
                Filter::Attributes(mask) => v.attrs[k_] & mask == *mask,
                Filter::Children(lo, hi) => v.is_dir(i) && memo.children.get(k_).is_some_and(|&n| (*lo..*hi).contains(&u64::from(n))),
                Filter::Depth(lo, hi) => (*lo..*hi).contains(&u64::from(depth(v, i))),
                Filter::Created(..) | Filter::Accessed(..) => false,
                Filter::Any(queries) => {
                    if queries.iter().any(Query::needs_path) {
                        if !lower_built {
                            sc.lower_path.clear();
                            v.path_into(i, true, &mut sc.chain, &mut sc.lower_path);
                            lower_built = true;
                        }
                        if !path_built {
                            sc.path.clear();
                            v.path_into(i, false, &mut sc.chain, &mut sc.path);
                            path_built = true;
                        }
                    }
                    let plain = crate::query::Plain { name: v.name(i), lower_name: v.lower_name(i), path: &sc.path, lower_path: &sc.lower_path, size: v.sizes[k_], modified: v.modified[k_], dir: v.is_dir(i) };
                    let facts = crate::query::Facts { attrs: v.attrs[k_], children: memo.children.get(k_).copied(), depth: memo.depth.then(|| depth(v, i)), ..Default::default() };
                    queries.iter().any(|q| q.matches_with(&plain, facts, &|_| false, &|_| false))
                }
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

fn depth(v: &VolumeIndex, mut i: u32) -> u32 {
    let mut n = 0;
    while i != NONE && v.attrs[i as usize] & ROOT == 0 && n < MAX_DEPTH as u32 {
        i = v.parents[i as usize];
        n += 1;
    }
    n
}

fn scan(v: &VolumeIndex, p: &Prepared<'_>, scope: &PathScope, access: Option<&dyn Visibility>, cancel: Cancel<'_>) -> Option<Vec<Vec<u32>>> {
    let visible = |i: u32| access.is_none_or(|a| a.can_list(v, v.parents[i as usize]));
    let paths = p.path_memo(v);
    let children = if p.query.uses(&|f| matches!(f, Filter::Children(..))) { Some(v.child_counts(cancel)?) } else { None };
    let children = children.as_deref().map_or(&[][..], Vec::as_slice);
    let memo = &FilterMemo { paths: &paths, scope, children, depth: p.query.uses(&|f| matches!(f, Filter::Depth(..))) };
    let parts: Vec<Option<Vec<u32>>> = match &p.driver {
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
                        if v.lower_off[e as usize] == starts[s] && v.listed(e) && p.matches(v, e, skip, false, memo, sc) && visible(e) {
                            out.push(e);
                        }
                    }
                    Some(out)
                })
                .collect()
        }
        None => chunks(v.len(), p.tasks)
            .par_iter()
            .map_init(Scratch::default, |sc, &(a, b)| {
                if cancel() {
                    return None;
                }
                let mut hits = p.filter_range(v, a, b, memo, sc);
                if access.is_some() {
                    hits.retain(|&i| visible(i));
                }
                Some(hits)
            })
            .collect(),
    };
    parts.into_iter().collect()
}

fn ranked_bits(parts: Vec<Vec<u32>>, rank: &[u32], n: usize, descending: bool) -> (Vec<u64>, usize) {
    let bits: Vec<_> = (0..rank.len().div_ceil(64)).map(|_| AtomicU64::new(0)).collect();
    let initial = if descending { 0u32 } else { u32::MAX };
    let earlier = |a: u32, b| if descending { a.max(b) } else { a.min(b) };
    let first = parts
        .into_par_iter()
        .map(|part| {
            let mut first = initial;
            let (mut word, mut mask) = (0, 0);
            for i in part {
                let next = i as usize / 64;
                if next != word {
                    if mask != 0 {
                        bits[word].fetch_or(mask, Relaxed);
                    }
                    word = next;
                    mask = 0;
                }
                mask |= 1 << (i % 64);
                first = earlier(first, rank[i as usize]);
            }
            if mask != 0 {
                bits[word].fetch_or(mask, Relaxed);
            }
            first
        })
        .reduce(|| initial, earlier) as usize;
    let pos = if first >= n {
        0
    } else if descending {
        n - 1 - first
    } else {
        first
    };
    (bits.into_iter().map(AtomicU64::into_inner).collect(), pos)
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
                        if desc {
                            c == Ordering::Greater
                        } else {
                            c == Ordering::Less
                        }
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
    pub fn has_order(&self, order: Order) -> bool {
        self.volumes.iter().all(|v| v.has_order(order))
    }

    pub fn build_order(&mut self, order: Order) {
        self.volumes.par_iter_mut().for_each(|v| v.build_order(order));
    }

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
        if !self.has_order(order) {
            return None;
        }
        let mut streams = Vec::new();
        let mut total = 0u64;
        let p = Prepared::new(query);
        for (vol, v) in self.volumes.iter().enumerate() {
            let scope = PathScope::resolve(v, &query.paths, cancel)?;
            if scope.skips_volume() {
                continue;
            }
            let before = total;
            let (kind, pos) = if query.terms.is_empty() && access.is_none() {
                let perm = &v.perms[order as usize];
                if scope.is_empty() {
                    total += perm.len() as u64;
                    (Kind::Perm(None), 0)
                } else {
                    let mut bits = vec![0; v.len().div_ceil(64)];
                    for (n, &i) in perm.iter().enumerate() {
                        if n.is_multiple_of(MIN_CHUNK) && cancel() {
                            return None;
                        }
                        if !scope.rejects(v, i) {
                            bits[i as usize / 64] |= 1 << (i % 64);
                            total += 1;
                        }
                    }
                    (Kind::Perm(Some(bits)), 0)
                }
            } else {
                let rank = &v.ranks[order as usize];
                let parts = scan(v, &p, &scope, access, cancel)?;
                if cancel() {
                    return None;
                }
                let count = parts.iter().map(Vec::len).sum::<usize>();
                total += count as u64;
                if count <= SMALL {
                    let mut hits = Vec::with_capacity(count);
                    hits.extend(parts.into_iter().flatten());
                    hits.par_sort_unstable_by_key(|&i| rank[i as usize]);
                    if sort.descending {
                        hits.reverse();
                    }
                    (Kind::List(hits), 0)
                } else {
                    let n = v.perms[order as usize].len();
                    let (bits, pos) = ranked_bits(parts, rank, n, sort.descending);
                    (Kind::Perm(Some(bits)), pos)
                }
            };
            if total > before {
                streams.push(Stream { vol, kind, pos });
            }
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
        let mut chain = Vec::new();
        let mut push = |v: &VolumeIndex, i: u32| {
            let k = i as usize;
            batch.push(v.name(i), Meta { size: v.sizes[k], modified: v.modified[k], created: 0, attrs: v.attrs[k] & KEPT_ATTRS });
            let mut dir = Vec::new();
            v.dir_into(i, &mut chain, &mut dir);
            dirs.push(dir);
        };
        let (from, to) = (from as usize, to as usize);
        let direct = if let [stream] = &rs.streams[..] {
            let volume = &self.volumes[stream.vol];
            match &stream.kind {
                Kind::List(ids) => Some((volume, &ids[from..to], false)),
                Kind::Perm(None) => {
                    let perm = &volume.perms[order_of(rs.sort.key) as usize];
                    let to = to.min(perm.len());
                    let from = from.min(to);
                    if rs.sort.descending {
                        Some((volume, &perm[perm.len() - to..perm.len() - from], true))
                    } else {
                        Some((volume, &perm[from..to], false))
                    }
                }
                Kind::Perm(Some(_)) => None,
            }
        } else {
            None
        };
        if let Some((volume, ids, reverse)) = direct {
            if reverse {
                ids.iter().rev().for_each(|&i| push(volume, i));
            } else {
                ids.iter().for_each(|&i| push(volume, i));
            }
        } else {
            rs.ensure(self, to);
            let end = to.min(rs.rows.len());
            for &(vol, i) in rs.rows.get(from..end).unwrap_or(&[]) {
                push(&self.volumes[vol as usize], i);
            }
        }
        (batch, dirs)
    }

    pub fn result_folders(&self, rs: &ResultSet, cancel: Cancel<'_>) -> Option<flit_core::query_attrs::FolderCounts> {
        let mut counts = FxHashMap::<(usize, u32), (u32, u64)>::default();
        for stream in &rs.streams {
            let v = &self.volumes[stream.vol];
            let visit = |(at, i): (usize, u32)| {
                if at.is_multiple_of(4096) && cancel() {
                    return None;
                }
                let parent = v.parents[i as usize];
                counts.entry((stream.vol, parent)).or_insert((i, 0)).1 += 1;
                Some(())
            };
            match &stream.kind {
                Kind::List(list) => list.iter().copied().enumerate().try_for_each(visit)?,
                Kind::Perm(bits) => v.perms[order_of(rs.sort.key) as usize].iter().copied().enumerate().try_for_each(|(at, i)| {
                    if at.is_multiple_of(4096) && cancel() {
                        return None;
                    }
                    if bits.as_ref().is_none_or(|b| b.get(i as usize / 64).is_some_and(|w| w & (1 << (i % 64)) != 0)) {
                        counts.entry((stream.vol, v.parents[i as usize])).or_insert((i, 0)).1 += 1;
                    }
                    Some(())
                })?,
            }
        }
        const MAX_FOLDERS: usize = 256;
        let key = |&((vol, parent), (_, count)): &((usize, u32), (u32, u64))| (std::cmp::Reverse(count), vol, parent);
        let mut counts: Vec<_> = counts.into_iter().collect();
        if counts.len() > MAX_FOLDERS {
            counts.select_nth_unstable_by_key(MAX_FOLDERS, key);
            counts.truncate(MAX_FOLDERS);
        }
        let mut chain = Vec::new();
        let folders = counts
            .into_iter()
            .map(|((vol, _), (i, count))| {
                let mut path = Vec::new();
                self.volumes[vol].dir_into(i, &mut chain, &mut path);
                (path, count)
            })
            .collect();
        Some(flit_core::query_attrs::finish_folder_counts(folders))
    }

    pub fn result_facets(&self, rs: &ResultSet, offset: i32, cancel: Cancel<'_>) -> Option<flit_core::query_attrs::Facets> {
        let mut facets = flit_core::query_attrs::FacetCounts::default();
        for stream in &rs.streams {
            let v = &self.volumes[stream.vol];
            let mut add = |i: u32| {
                let k = i as usize;
                facets.add(v.name(i), Meta { size: v.sizes[k], modified: v.modified[k], attrs: v.attrs[k], created: 0 }, offset);
            };
            match &stream.kind {
                Kind::List(ids) => {
                    for (n, &i) in ids.iter().enumerate() {
                        if n.is_multiple_of(4096) && cancel() {
                            return None;
                        }
                        add(i);
                    }
                }
                Kind::Perm(bits) => {
                    for (n, &i) in v.perms[order_of(rs.sort.key) as usize].iter().enumerate() {
                        if n.is_multiple_of(4096) && cancel() {
                            return None;
                        }
                        if bits.as_ref().is_none_or(|b| b.get(i as usize / 64).is_some_and(|w| w & (1 << (i % 64)) != 0)) {
                            add(i);
                        }
                    }
                }
            }
        }
        (!cancel()).then(|| facets.finish())
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
