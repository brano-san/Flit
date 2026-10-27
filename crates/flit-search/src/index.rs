use flit_core::attr;
use flit_core::hash::{self, FxHashMap};
use flit_core::name;
use hashbrown::HashTable;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;

#[derive(Debug, Clone, Copy)]
pub struct IndexRecord<'a> {
    pub id: u64,
    pub parent: u64,
    pub name: &'a [u8],
    pub size: u64,
    pub modified: i64,
    pub attrs: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checkpoint {
    pub journal: u64,
    pub next: i64,
}

pub trait IndexSink {
    fn upsert(&mut self, record: &IndexRecord<'_>);
    fn remove(&mut self, id: u64);
    fn knows(&self, id: u64) -> bool;
    fn progress(&mut self, _done: u64, _total: u64) {}
    fn cancelled(&self) -> bool {
        false
    }
}

pub const NONE: u32 = u32::MAX;
pub const ROOT: u32 = 1 << 30;
pub const KEPT_ATTRS: u32 = attr::DIR | attr::HIDDEN | attr::SYSTEM | attr::READONLY | attr::LINK;
const MAX_DEPTH: usize = 1024;
const MAX_NAME: usize = u16::MAX as usize;
const SAME: u32 = u32::MAX;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Order {
    Name = 0,
    Path = 1,
    Size = 2,
    Modified = 3,
}

pub fn lowercase_into(s: &[u8], out: &mut Vec<u8>) {
    let mut i = 0;
    while i < s.len() {
        let b = s[i];
        if b < 0x80 {
            out.push(b.to_ascii_lowercase());
            i += 1;
            continue;
        }
        let (cp, n) = name::decode(&s[i..]);
        match char::from_u32(cp).filter(|_| !(cp == 0xFFFD && n == 1)) {
            Some(c) => c.to_lowercase().for_each(|l| name::push_cp(out, l as u32)),
            None => out.extend_from_slice(&s[i..i + n]),
        }
        i += n;
    }
}

pub fn lowercase(s: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    lowercase_into(s, &mut out);
    out
}

#[derive(Default, Clone)]
pub struct VolumeIndex {
    pub id: String,
    pub root: Vec<u8>,
    pub root_lower: Vec<u8>,
    pub sep: u8,
    pub epoch: u64,
    pub ids: Vec<u64>,
    pub parents: Vec<u32>,
    pub names: Vec<u8>,
    pub name_off: Vec<u32>,
    pub name_len: Vec<u16>,
    pub lower: Vec<u8>,
    pub lower_off: Vec<u32>,
    pub lower_len: Vec<u16>,
    pub seg_start: Vec<u32>,
    pub seg_entry: Vec<u32>,
    pub sizes: Vec<u64>,
    pub modified: Vec<i64>,
    pub attrs: Vec<u32>,
    pub perms: [Vec<u32>; 4],
    pub ranks: [Vec<u32>; 4],
    by_id: HashTable<u32>,
    orphan_parents: FxHashMap<u32, u64>,
    built: u8,
    removed_now: FxHashMap<u64, u32>,
    orphans: Vec<u32>,
    changed: Vec<u32>,
    changed_bits: Vec<u64>,
    dead_bytes: usize,
    removed_dirs: bool,
    live: usize,
}

impl VolumeIndex {
    pub fn new(id: &str, root: &[u8], sep: u8) -> Self {
        Self { id: id.to_owned(), root: root.to_vec(), root_lower: lowercase(root), sep, ..Default::default() }
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    pub fn live(&self) -> usize {
        self.live
    }

    pub fn pending(&self) -> usize {
        self.changed.len()
    }

    pub fn lookup(&self, id: u64) -> Option<u32> {
        self.by_id.find(hash::hash(&id), |&i| self.ids[i as usize] == id).copied()
    }

    fn index_id(&mut self, i: u32) {
        let ids = &self.ids;
        self.by_id.insert_unique(hash::hash(&ids[i as usize]), i, |&j| hash::hash(&ids[j as usize]));
    }

    fn unindex_id(&mut self, id: u64) -> Option<u32> {
        let ids = &self.ids;
        self.by_id.find_entry(hash::hash(&id), |&i| ids[i as usize] == id).ok().map(|e| e.remove().0)
    }

    pub fn parent_id(&self, i: u32) -> u64 {
        let k = i as usize;
        match self.parents[k] {
            _ if self.attrs[k] & ROOT != 0 => self.ids[k],
            NONE => self.orphan_parents.get(&i).copied().unwrap_or(self.ids[k]),
            p => self.ids[p as usize],
        }
    }

    pub fn has_order(&self, order: Order) -> bool {
        order == Order::Name || self.built & (1 << order as u8) != 0
    }

    pub fn build_order(&mut self, order: Order) {
        if self.has_order(order) {
            return;
        }
        self.commit();
        self.built |= 1 << order as u8;
        self.perms[order as usize] = match order {
            Order::Path => self.path_perm(),
            _ => {
                let mut all: Vec<u32> = (0..self.len() as u32).filter(|&i| self.listed(i)).collect();
                all.par_sort_unstable_by_key(|&i| self.key(order, i));
                all
            }
        };
        self.rank_of(order);
    }

    pub fn reserve(&mut self, records: usize, name_bytes: usize) {
        let ids = &self.ids;
        self.by_id.reserve(records, |&j| hash::hash(&ids[j as usize]));
        self.ids.reserve(records);
        self.parents.reserve(records);
        self.name_off.reserve(records);
        self.name_len.reserve(records);
        self.lower.reserve(name_bytes + records);
        self.lower_off.reserve(records);
        self.lower_len.reserve(records);
        self.seg_start.reserve(records);
        self.seg_entry.reserve(records);
        self.sizes.reserve(records);
        self.modified.reserve(records);
        self.attrs.reserve(records);
        self.changed.reserve(records);
    }

    pub fn shrink(&mut self) {
        let ids = &self.ids;
        self.by_id.shrink_to_fit(|&j| hash::hash(&ids[j as usize]));
        self.ids.shrink_to_fit();
        self.parents.shrink_to_fit();
        self.names.shrink_to_fit();
        self.name_off.shrink_to_fit();
        self.name_len.shrink_to_fit();
        self.lower.shrink_to_fit();
        self.lower_off.shrink_to_fit();
        self.lower_len.shrink_to_fit();
        self.seg_start.shrink_to_fit();
        self.seg_entry.shrink_to_fit();
        self.sizes.shrink_to_fit();
        self.modified.shrink_to_fit();
        self.attrs.shrink_to_fit();
        self.perms.iter_mut().chain(self.ranks.iter_mut()).for_each(Vec::shrink_to_fit);
        self.orphan_parents.shrink_to_fit();
        self.changed.shrink_to_fit();
    }

    #[inline]
    pub fn listed(&self, i: u32) -> bool {
        self.attrs[i as usize] & (attr::DELETED | ROOT) == 0
    }

    #[inline]
    pub fn is_dir(&self, i: u32) -> bool {
        self.attrs[i as usize] & attr::DIR != 0
    }

    #[inline]
    pub fn name(&self, i: u32) -> &[u8] {
        let l = self.name_len[i as usize] as usize;
        match self.name_off[i as usize] {
            SAME => {
                let o = self.lower_off[i as usize] as usize;
                &self.lower[o..o + l]
            }
            o => &self.names[o as usize..o as usize + l],
        }
    }

    #[inline]
    pub fn lower_name(&self, i: u32) -> &[u8] {
        let (o, l) = (self.lower_off[i as usize] as usize, self.lower_len[i as usize] as usize);
        &self.lower[o..o + l]
    }

    #[inline]
    pub fn ext(&self, i: u32) -> &[u8] {
        let n = self.lower_name(i);
        match memchr::memrchr(b'.', n) {
            Some(p) if p + 1 < n.len() && !self.is_dir(i) => &n[p + 1..],
            _ => &[],
        }
    }

    pub fn path_into(&self, i: u32, lower: bool, chain: &mut Vec<u32>, out: &mut Vec<u8>) {
        chain.clear();
        let mut cur = i;
        while chain.len() < MAX_DEPTH {
            if self.attrs[cur as usize] & ROOT != 0 {
                break;
            }
            chain.push(cur);
            let p = self.parents[cur as usize];
            if p == NONE {
                break;
            }
            cur = p;
        }
        out.extend_from_slice(if lower { &self.root_lower } else { &self.root });
        for &c in chain.iter().rev() {
            if out.last() != Some(&self.sep) {
                out.push(self.sep);
            }
            out.extend_from_slice(if lower { self.lower_name(c) } else { self.name(c) });
        }
    }

    pub fn dir_into(&self, i: u32, chain: &mut Vec<u32>, out: &mut Vec<u8>) {
        match self.parents[i as usize] {
            NONE => out.extend_from_slice(&self.root),
            p => self.path_into(p, false, chain, out),
        }
    }

    fn touch(&mut self, i: u32) {
        let (w, b) = (i as usize / 64, i as usize % 64);
        if self.changed_bits.len() <= w {
            self.changed_bits.resize(w + 1, 0);
        }
        if self.changed_bits[w] & (1 << b) == 0 {
            self.changed_bits[w] |= 1 << b;
            self.changed.push(i);
        }
    }

    fn put_name(&mut self, i: usize, name: &[u8]) {
        let name = &name[..name.len().min(MAX_NAME)];
        let at = self.lower.len();
        lowercase_into(name, &mut self.lower);
        let len = (self.lower.len() - at).min(MAX_NAME);
        self.lower.truncate(at + len);
        self.name_len[i] = name.len() as u16;
        if self.lower[at..] == *name {
            self.name_off[i] = SAME;
        } else {
            self.name_off[i] = self.names.len() as u32;
            self.names.extend_from_slice(name);
        }
        self.lower.push(0);
        self.lower_off[i] = at as u32;
        self.lower_len[i] = len as u16;
        self.seg_start.push(at as u32);
        self.seg_entry.push(i as u32);
    }

    fn resolve(&mut self, i: u32, pid: u64) {
        let k = i as usize;
        if self.attrs[k] & ROOT != 0 {
            self.parents[k] = NONE;
            self.orphan_parents.remove(&i);
            return;
        }
        match self.lookup(pid) {
            Some(p) => {
                self.parents[k] = p;
                self.orphan_parents.remove(&i);
            }
            None => {
                self.parents[k] = NONE;
                self.orphan_parents.insert(i, pid);
                self.orphans.push(i);
            }
        }
    }

    fn revive(&mut self, id: u64) -> Option<u32> {
        let i = self.removed_now.remove(&id)?;
        self.attrs[i as usize] &= !attr::DELETED;
        self.index_id(i);
        self.live += (self.attrs[i as usize] & ROOT == 0) as usize;
        Some(i)
    }

    fn resolve_orphans(&mut self) {
        let orphans = std::mem::take(&mut self.orphans);
        for i in orphans {
            let Some(&pid) = self.orphan_parents.get(&i) else { continue };
            if self.attrs[i as usize] & attr::DELETED == 0 && self.parents[i as usize] == NONE {
                self.resolve(i, pid);
                if self.parents[i as usize] != NONE {
                    self.touch(i);
                }
            }
        }
    }

    fn cascade(&mut self) {
        const UNKNOWN: u8 = 0;
        const ALIVE: u8 = 1;
        const DEAD: u8 = 2;
        let n = self.len();
        let mut state = vec![UNKNOWN; n];
        let mut chain = Vec::new();
        for i in 0..n {
            if state[i] != UNKNOWN {
                continue;
            }
            chain.clear();
            let mut cur = i as u32;
            let verdict = loop {
                let c = cur as usize;
                if state[c] != UNKNOWN {
                    break state[c];
                }
                if self.attrs[c] & attr::DELETED != 0 {
                    state[c] = DEAD;
                    break DEAD;
                }
                chain.push(cur);
                let p = self.parents[c];
                if p == NONE || self.attrs[c] & ROOT != 0 || chain.len() > MAX_DEPTH {
                    break ALIVE;
                }
                cur = p;
            };
            for &c in &chain {
                state[c as usize] = verdict;
            }
        }
        for i in 0..n as u32 {
            if state[i as usize] == DEAD && self.attrs[i as usize] & attr::DELETED == 0 {
                let id = self.ids[i as usize];
                self.remove(id);
            }
        }
    }

    fn compact(&mut self) {
        let n = self.len();
        let (names, lower) = (std::mem::take(&mut self.names), std::mem::take(&mut self.lower));
        self.seg_start.clear();
        self.seg_entry.clear();
        let (old_off, old_len) = (std::mem::take(&mut self.name_off), std::mem::take(&mut self.name_len));
        let (old_loff, old_llen) = (std::mem::take(&mut self.lower_off), std::mem::take(&mut self.lower_len));
        self.name_off = vec![0; n];
        self.name_len = vec![0; n];
        self.lower_off = vec![0; n];
        self.lower_len = vec![0; n];
        for i in 0..n {
            if self.attrs[i] & attr::DELETED != 0 {
                continue;
            }
            let (o, l) = (old_off[i], old_len[i]);
            self.name_len[i] = l;
            self.name_off[i] = match o {
                SAME => SAME,
                o => {
                    let at = self.names.len() as u32;
                    self.names.extend_from_slice(&names[o as usize..o as usize + l as usize]);
                    at
                }
            };
            let (o, l) = (old_loff[i] as usize, old_llen[i] as usize);
            let at = self.lower.len() as u32;
            self.lower_off[i] = at;
            self.lower_len[i] = l as u16;
            self.lower.extend_from_slice(&lower[o..o + l]);
            self.lower.push(0);
            self.seg_start.push(at);
            self.seg_entry.push(i as u32);
        }
        self.dead_bytes = 0;
        self.epoch += 1;
    }

    pub fn cmp_name(&self, a: u32, b: u32) -> Ordering {
        self.lower_name(a).cmp(self.lower_name(b)).then_with(|| self.name(a).cmp(self.name(b))).then(a.cmp(&b))
    }

    fn key(&self, order: Order, i: u32) -> (i64, u32) {
        let rank = self.ranks[Order::Name as usize][i as usize];
        match order {
            Order::Size => (self.sizes[i as usize] as i64 ^ i64::MIN, rank),
            _ => (self.modified[i as usize], rank),
        }
    }

    fn merge_into(&mut self, order: Order, fresh: &[u32]) -> usize {
        let o = order as usize;
        let old = std::mem::take(&mut self.perms[o]);
        let ranks = &mut self.ranks[o];
        let mut dropped: Vec<usize> = Vec::with_capacity(self.changed.len());
        for &i in &self.changed {
            if let Some(r) = ranks.get_mut(i as usize).filter(|r| **r != NONE) {
                dropped.push(*r as usize);
                *r = NONE;
            }
        }
        dropped.sort_unstable();
        let mut kept = Vec::with_capacity(old.len());
        let mut at = 0;
        for &d in &dropped {
            kept.extend_from_slice(&old[at..d]);
            at = d + 1;
        }
        kept.extend_from_slice(&old[at..]);
        drop(old);
        let mut first = dropped.first().copied().unwrap_or(usize::MAX);
        let mut out = Vec::with_capacity(kept.len() + fresh.len());
        let mut at = 0;
        for &j in fresh {
            let p = at + kept[at..].partition_point(|&i| self.before(order, i, j));
            out.extend_from_slice(&kept[at..p]);
            first = first.min(out.len());
            out.push(j);
            at = p;
        }
        out.extend_from_slice(&kept[at..]);
        self.perms[o] = out;
        first
    }

    fn before(&self, order: Order, a: u32, b: u32) -> bool {
        match order {
            Order::Name => self.cmp_name(a, b) == Ordering::Less,
            _ => self.key(order, a) < self.key(order, b),
        }
    }

    fn rank_from(&mut self, order: Order, from: usize) {
        let n = self.len();
        let (perm, r) = (&self.perms[order as usize], &mut self.ranks[order as usize]);
        r.resize(n, NONE);
        for (pos, &i) in perm.iter().enumerate().skip(from) {
            r[i as usize] = pos as u32;
        }
    }

    fn rank_of(&mut self, order: Order) {
        let mut r = std::mem::take(&mut self.ranks[order as usize]);
        r.clear();
        r.resize(self.len(), NONE);
        for (pos, &i) in self.perms[order as usize].iter().enumerate() {
            r[i as usize] = pos as u32;
        }
        self.ranks[order as usize] = r;
    }

    fn path_perm(&self) -> Vec<u32> {
        let n = self.len();
        let by_name = &self.perms[Order::Name as usize];
        let slot = |i: u32| -> usize {
            match self.parents[i as usize] {
                NONE => n,
                p if self.attrs[p as usize] & ROOT != 0 => n,
                p => p as usize,
            }
        };
        let mut start = vec![0u32; n + 2];
        for &i in by_name {
            start[slot(i) + 1] += 1;
        }
        for k in 1..start.len() {
            start[k] += start[k - 1];
        }
        let mut fill = start.clone();
        let mut kids = vec![0u32; by_name.len()];
        for &i in by_name {
            let s = slot(i);
            kids[fill[s] as usize] = i;
            fill[s] += 1;
        }
        let children = |s: usize| &kids[start[s] as usize..start[s + 1] as usize];
        let has_children = |i: u32| start[i as usize + 1] > start[i as usize];
        let mut out = Vec::with_capacity(by_name.len());
        let mut stack: Vec<u32> = Vec::new();
        out.extend_from_slice(children(n));
        stack.extend(children(n).iter().rev().filter(|&&c| has_children(c)));
        let mut seen = vec![false; n];
        while let Some(d) = stack.pop() {
            if std::mem::replace(&mut seen[d as usize], true) {
                continue;
            }
            out.extend_from_slice(children(d as usize));
            stack.extend(children(d as usize).iter().rev().filter(|&&c| has_children(c)));
        }
        if out.len() < by_name.len() {
            let mut emitted = vec![false; n];
            out.iter().for_each(|&i| emitted[i as usize] = true);
            out.extend(by_name.iter().filter(|&&i| !emitted[i as usize]));
        }
        out
    }

    pub fn commit(&mut self) -> bool {
        self.resolve_orphans();
        if self.removed_dirs {
            self.cascade();
            self.removed_dirs = false;
        }
        if self.changed.is_empty() {
            return false;
        }
        if self.dead_bytes > (self.lower.len() / 4).max(1 << 20) {
            self.compact();
        }
        let mut fresh: Vec<u32> = self.changed.iter().copied().filter(|&i| self.listed(i)).collect();
        fresh.par_sort_unstable_by(|&a, &b| self.cmp_name(a, b));
        let from = self.merge_into(Order::Name, &fresh);
        self.rank_from(Order::Name, from);
        let lazy: Vec<Order> = [Order::Size, Order::Modified].into_iter().filter(|&o| self.has_order(o)).collect();
        for order in lazy {
            fresh.par_sort_unstable_by_key(|&i| self.key(order, i));
            let from = self.merge_into(order, &fresh);
            self.rank_from(order, from);
        }
        if self.has_order(Order::Path) {
            self.built &= !(1 << Order::Path as u8);
            self.perms[Order::Path as usize] = Vec::new();
            self.ranks[Order::Path as usize] = Vec::new();
        }
        for &i in &self.changed {
            self.changed_bits[i as usize / 64] = 0;
        }
        self.changed.clear();
        self.removed_now.clear();
        true
    }
}

impl IndexSink for VolumeIndex {
    fn knows(&self, id: u64) -> bool {
        self.lookup(id).is_some()
    }

    fn upsert(&mut self, r: &IndexRecord<'_>) {
        let attrs = (r.attrs & KEPT_ATTRS) | if r.parent == r.id { ROOT } else { 0 };
        let found = self.lookup(r.id).or_else(|| self.revive(r.id));
        let i = match found {
            Some(i) => {
                let k = i as usize;
                if self.name(i) != r.name {
                    self.dead_bytes += self.lower_len[k] as usize + 1;
                    self.put_name(k, r.name);
                }
                self.sizes[k] = r.size;
                self.modified[k] = r.modified;
                let was_dir = self.attrs[k] & attr::DIR != 0;
                self.attrs[k] = attrs;
                self.removed_dirs |= was_dir && attrs & attr::DIR == 0;
                if self.parent_id(i) != r.parent || self.parents[k] == NONE {
                    self.resolve(i, r.parent);
                }
                i
            }
            None => {
                let i = self.len() as u32;
                self.ids.push(r.id);
                self.parents.push(NONE);
                self.name_off.push(0);
                self.name_len.push(0);
                self.lower_off.push(0);
                self.lower_len.push(0);
                self.put_name(i as usize, r.name);
                self.sizes.push(r.size);
                self.modified.push(r.modified);
                self.attrs.push(attrs);
                self.index_id(i);
                self.live += (attrs & ROOT == 0) as usize;
                self.resolve(i, r.parent);
                i
            }
        };
        self.touch(i);
    }

    fn remove(&mut self, id: u64) {
        let Some(i) = self.unindex_id(id) else { return };
        self.orphan_parents.remove(&i);
        let k = i as usize;
        self.attrs[k] |= attr::DELETED;
        self.dead_bytes += self.lower_len[k] as usize + 1;
        self.removed_dirs |= self.attrs[k] & attr::DIR != 0;
        self.live -= (self.attrs[k] & ROOT == 0) as usize;
        self.removed_now.insert(id, i);
        self.touch(i);
    }
}

#[cfg(test)]
#[path = "../test/index.rs"]
mod tests;
