use crate::filter::NameFilter;
use crate::hash::{self, FxHashMap};
use crate::listing::{attr, ChangeKind, DirChange, DirSnapshot, Listing, Meta};
use crate::sort::{self, GroupField, ViewSpec};
use smallvec::SmallVec;
use std::borrow::Borrow;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Group {
    pub first: u32,
    pub start: u32,
    pub line: u32,
    pub count: u32,
    pub collapsed: bool,
    pub id: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Line {
    Header(usize),
    Row(usize),
}

pub struct DirView {
    snap: Arc<DirSnapshot>,
    sel: Vec<u64>,
    sel_count: usize,
    pub focus: Option<u32>,
    pub anchor: Option<u32>,
    names: Option<FxHashMap<u64, SmallVec<[u32; 1]>>>,
    filter: Option<NameFilter>,
    groups: Vec<Group>,
    collapsed: Vec<u64>,
    shown: Option<Vec<u32>>,
}

impl DirView {
    pub fn new(snap: Arc<DirSnapshot>) -> Self {
        let mut v = Self { snap, sel: Vec::new(), sel_count: 0, focus: None, anchor: None, names: None, filter: None, groups: Vec::new(), collapsed: Vec::new(), shown: None };
        v.regroup();
        v
    }

    pub fn snap(&self) -> &Arc<DirSnapshot> {
        &self.snap
    }

    pub fn replace(&mut self, snap: Arc<DirSnapshot>) {
        self.snap = snap;
        self.names = None;
        self.rows_changed();
        self.prune();
    }

    #[inline]
    pub fn rows(&self) -> &[u32] {
        match &self.shown {
            Some(s) => s,
            None => self.base_rows(),
        }
    }

    #[inline]
    pub fn base_rows(&self) -> &[u32] {
        match &self.filter {
            Some(f) => f.rows(),
            None => &self.snap.order,
        }
    }

    pub fn groups(&self) -> &[Group] {
        &self.groups
    }

    pub fn lines(&self) -> usize {
        self.rows().len() + self.groups.len()
    }

    pub fn line(&self, n: usize) -> Option<Line> {
        if n >= self.lines() {
            return None;
        }
        if self.groups.is_empty() {
            return Some(Line::Row(n));
        }
        let g = self.groups.partition_point(|gr| gr.line as usize <= n) - 1;
        let gr = &self.groups[g];
        Some(match n - gr.line as usize {
            0 => Line::Header(g),
            k => Line::Row(gr.start as usize + k - 1),
        })
    }

    pub fn line_of_row(&self, r: usize) -> usize {
        r + self.groups.partition_point(|gr| gr.start as usize <= r)
    }

    pub fn row_at_line(&self, n: usize) -> usize {
        match self.line(n) {
            Some(Line::Row(r)) => r,
            Some(Line::Header(g)) => self.groups[g].start as usize,
            None => self.rows().len(),
        }
    }

    pub fn toggle_group(&mut self, g: usize) {
        let Some(id) = self.groups.get(g).map(|gr| gr.id) else { return };
        match self.collapsed.iter().position(|&c| c == id) {
            Some(k) => {
                self.collapsed.swap_remove(k);
            }
            None => self.collapsed.push(id),
        }
        self.regroup();
        self.prune();
    }

    fn regroup(&mut self) {
        self.groups.clear();
        self.shown = None;
        let view = self.snap.view;
        let field = view.group.field;
        if field == GroupField::None {
            return;
        }
        let l = &self.snap.listing;
        let base = match &self.filter {
            Some(f) => f.rows(),
            None => &self.snap.order,
        };
        let fold = !self.collapsed.is_empty();
        let mut shown = Vec::new();
        let mut r = 0;
        while r < base.len() {
            let first = base[r];
            let (key, bytes) = (sort::group_key(l, first, &view).0, sort::group_bytes(l, first, field));
            let same = |&i: &u32| sort::group_key(l, i, &view).0 == key && sort::group_bytes(l, i, field).eq_ignore_ascii_case(bytes);
            let end = r + 1 + base[r + 1..].partition_point(same);
            let id = sort::group_id(l, first, &view);
            let collapsed = fold && self.collapsed.contains(&id);
            let start = if fold { shown.len() } else { r };
            if fold && !collapsed {
                shown.extend_from_slice(&base[r..end]);
            }
            let line = (start + self.groups.len()) as u32;
            self.groups.push(Group { first, start: start as u32, line, count: (end - r) as u32, collapsed, id });
            r = end;
        }
        if fold {
            self.shown = Some(shown);
        }
    }

    fn rows_changed(&mut self) {
        self.refilter();
        self.regroup();
    }

    #[inline]
    pub fn listing(&self) -> &Listing {
        &self.snap.listing
    }

    pub fn row_of(&self, i: u32) -> Option<usize> {
        let s = &self.snap;
        match sort::find_row(&s.listing, &s.keys, &s.view, self.rows(), i) {
            Some(r) => Some(r),
            None => self.rows().iter().position(|&p| p == i),
        }
    }

    pub fn find(&mut self, name: &[u8]) -> Option<u32> {
        let l = &self.snap.listing;
        let names = self.names.get_or_insert_with(|| {
            let mut m = FxHashMap::default();
            m.reserve(l.len());
            for i in 0..l.len() as u32 {
                if l.entries[i as usize].attrs & attr::DELETED == 0 {
                    m.entry(hash::hash(l.name(i))).or_insert_with(SmallVec::new).push(i);
                }
            }
            m
        });
        names.get(&hash::hash(name))?.iter().copied().find(|&i| l.name(i) == name)
    }

    pub(crate) fn is_visible(&self, i: u32) -> bool {
        let (l, v) = (&self.snap.listing, &self.snap.view);
        (i as usize) < l.len()
            && sort::visible(l, i, v)
            && self.filter.as_ref().is_none_or(|f| f.matched(i))
            && (self.groups.is_empty() || self.collapsed.is_empty() || !self.collapsed.contains(&sort::group_id(l, i, v)))
    }

    #[inline]
    pub fn is_selected(&self, i: u32) -> bool {
        self.sel.get(i as usize / 64).is_some_and(|w| w >> (i % 64) & 1 != 0)
    }

    pub fn set_selected(&mut self, i: u32, on: bool) {
        let w = i as usize / 64;
        if w >= self.sel.len() {
            if !on {
                return;
            }
            self.sel.resize(w + 1, 0);
        }
        let bit = 1u64 << (i % 64);
        let was = self.sel[w] & bit != 0;
        if was != on {
            self.sel[w] ^= bit;
            if on { self.sel_count += 1 } else { self.sel_count -= 1 }
        }
    }

    pub fn clear_selection(&mut self) {
        self.sel.clear();
        self.sel_count = 0;
    }

    pub fn select_all(&mut self) {
        self.clear_selection();
        for r in 0..self.rows().len() {
            let i = self.rows()[r];
            self.set_selected(i, true);
        }
    }

    pub fn invert_selection(&mut self) {
        for r in 0..self.rows().len() {
            let i = self.rows()[r];
            let on = !self.is_selected(i);
            self.set_selected(i, on);
        }
    }

    pub fn select_rows(&mut self, a: usize, b: usize) {
        let (lo, hi) = (a.min(b), a.max(b).min(self.rows().len().saturating_sub(1)));
        for r in lo..=hi {
            let i = self.rows()[r];
            self.set_selected(i, true);
        }
    }

    pub fn selected_count(&self) -> usize {
        self.sel_count
    }

    fn marked(&self) -> impl Iterator<Item = u32> + '_ {
        self.sel.iter().enumerate().flat_map(|(w, &bits)| {
            (0..64).filter(move |b| bits >> b & 1 != 0).map(move |b| (w * 64 + b) as u32)
        })
    }

    pub fn selected(&self) -> impl Iterator<Item = u32> + '_ {
        self.marked().filter(|&i| self.is_visible(i))
    }

    pub fn selected_size(&self) -> u64 {
        let l = self.listing();
        self.selected().map(|i| &l.entries[i as usize]).filter(|e| !e.is_dir() || e.attrs & attr::SIZE_KNOWN != 0).map(|e| e.size).sum()
    }

    fn prune(&mut self) {
        let gone: Vec<u32> = self.marked().chain(self.focus).chain(self.anchor).filter(|&i| !self.is_visible(i)).collect();
        for i in gone {
            self.set_selected(i, false);
            if self.focus == Some(i) {
                self.focus = None;
            }
            if self.anchor == Some(i) {
                self.anchor = None;
            }
        }
    }

    fn update(&mut self, i: u32, meta: Meta) {
        let snap = Arc::make_mut(&mut self.snap);
        let row = sort::find_row(&snap.listing, &snap.keys, &snap.view, &snap.order, i).or_else(|| snap.order.iter().position(|&p| p == i));
        if let Some(r) = row {
            snap.order.remove(r);
        }
        snap.listing.set_meta(i, meta);
        if sort::visible(&snap.listing, i, &snap.view) {
            let p = sort::insert_pos(&snap.listing, &snap.keys, &snap.view, &snap.order, i);
            snap.order.insert(p, i);
        }
    }

    pub fn apply(&mut self, c: &DirChange) {
        self.apply_all([c]);
    }

    pub(crate) fn apply_all<C: Borrow<DirChange>>(&mut self, changes: impl IntoIterator<Item = C>) {
        let mut touched = Vec::new();
        for c in changes {
            let c = c.borrow();
            let existing = self.find(&c.name);
            if existing.is_none() && c.kind == ChangeKind::Removed {
                continue;
            }
            let snap = Arc::make_mut(&mut self.snap);
            let i = match existing {
                Some(i) if c.kind == ChangeKind::Removed => {
                    snap.listing.entries[i as usize].attrs |= attr::DELETED;
                    if let Some(v) = self.names.as_mut().and_then(|m| m.get_mut(&hash::hash(c.name.as_slice()))) {
                        v.retain(|&mut x| x != i);
                    }
                    i
                }
                Some(i) => {
                    snap.listing.set_meta(i, c.meta.unwrap_or_default());
                    i
                }
                None => {
                    let i = snap.listing.push(&c.name, c.meta.unwrap_or_default());
                    snap.keys.push(snap.listing.name(i));
                    if let Some(m) = &mut self.names {
                        m.entry(hash::hash(c.name.as_slice())).or_default().push(i);
                    }
                    i
                }
            };
            touched.push(i);
        }
        if !touched.is_empty() {
            self.reorder(touched);
            self.rows_changed();
            self.prune();
        }
    }

    fn reorder(&mut self, mut touched: Vec<u32>) {
        let snap = Arc::make_mut(&mut self.snap);
        let (l, k, view) = (&snap.listing, &snap.keys, snap.view);
        let mut moved = vec![false; l.len()];
        for &i in &touched {
            moved[i as usize] = true;
        }
        snap.order.retain(|&i| !moved[i as usize]);
        touched.retain(|&i| sort::visible(l, i, &view));
        touched.sort_unstable_by(|&a, &b| sort::cmp(l, k, &view, a, b));
        touched.dedup();
        let mut order = Vec::with_capacity(snap.order.len() + touched.len());
        let mut rest = &snap.order[..];
        for &i in &touched {
            let p = rest.partition_point(|&o| sort::cmp(l, k, &view, o, i).is_lt());
            order.extend_from_slice(&rest[..p]);
            order.push(i);
            rest = &rest[p..];
        }
        order.extend_from_slice(rest);
        snap.order = order;
    }

    pub fn set_dir_size(&mut self, i: u32, size: u64) {
        let Some(e) = self.listing().entries.get(i as usize) else { return };
        let mut m = e.meta();
        if m.attrs & attr::DELETED != 0 {
            return;
        }
        m.size = size;
        m.attrs |= attr::SIZE_KNOWN;
        self.update(i, m);
        self.rows_changed();
        self.prune();
    }

    pub fn merge_order(&mut self, mut order: Vec<u32>, view: ViewSpec, base_len: usize) {
        if view.group != self.snap.view.group {
            self.collapsed.clear();
        }
        let snap = Arc::make_mut(&mut self.snap);
        let l = &snap.listing;
        order.retain(|&i| sort::visible(l, i, &view));
        let sorted = order.windows(2).all(|w| sort::cmp(l, &snap.keys, &view, w[0], w[1]).is_lt());
        snap.order = order;
        snap.view = view;
        if sorted {
            self.reorder((base_len as u32..self.snap.listing.len() as u32).collect());
        } else {
            let mut o = std::mem::take(&mut snap.order);
            sort::sort(&snap.listing, &snap.keys, view, &mut o);
            snap.order = o;
        }
        self.rows_changed();
        self.prune();
    }

    pub fn set_filter(&mut self, text: &str) {
        let prev = self.filter.take();
        self.filter = NameFilter::new(text);
        if let (Some(f), Some(p)) = (&mut self.filter, &prev) {
            if f.narrows(p) {
                f.seed_from(p, &self.snap.listing);
            }
        }
        self.rows_changed();
        self.prune();
    }

    pub fn is_filtered(&self) -> bool {
        self.filter.is_some()
    }

    pub fn total_rows(&self) -> usize {
        self.snap.order.len()
    }

    fn refilter(&mut self) {
        if let Some(f) = &mut self.filter {
            f.sync(&self.snap.listing, &self.snap.order);
        }
    }
}

#[cfg(test)]
#[path = "../test/view.rs"]
mod tests;
