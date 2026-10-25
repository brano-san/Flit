use crate::hash::{self, FxHashMap};
use crate::listing::{attr, ChangeKind, DirChange, DirSnapshot, Listing, Meta};
use crate::sort::{self, ViewSpec};
use smallvec::SmallVec;
use std::sync::Arc;

pub struct DirView {
    snap: Arc<DirSnapshot>,
    sel: Vec<u64>,
    sel_count: usize,
    pub focus: Option<u32>,
    pub anchor: Option<u32>,
    names: Option<FxHashMap<u64, SmallVec<[u32; 1]>>>,
}

impl DirView {
    pub fn new(snap: Arc<DirSnapshot>) -> Self {
        Self { snap, sel: Vec::new(), sel_count: 0, focus: None, anchor: None, names: None }
    }

    pub fn snap(&self) -> &Arc<DirSnapshot> {
        &self.snap
    }

    pub fn replace(&mut self, snap: Arc<DirSnapshot>) {
        self.snap = snap;
        self.names = None;
        let n = self.snap.listing.len() as u32;
        self.focus = self.focus.filter(|&f| f < n);
        self.anchor = self.anchor.filter(|&f| f < n);
    }

    #[inline]
    pub fn rows(&self) -> &[u32] {
        &self.snap.order
    }

    #[inline]
    pub fn listing(&self) -> &Listing {
        &self.snap.listing
    }

    pub fn row_of(&self, i: u32) -> Option<usize> {
        let s = &self.snap;
        match sort::find_row(&s.listing, &s.keys, s.view.sort, &s.order, i) {
            Some(r) => Some(r),
            None => s.order.iter().position(|&p| p == i),
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

    pub fn selected(&self) -> impl Iterator<Item = u32> + '_ {
        self.sel.iter().enumerate().flat_map(|(w, &bits)| {
            (0..64).filter(move |b| bits >> b & 1 != 0).map(move |b| (w * 64 + b) as u32)
        })
    }

    pub fn selected_size(&self) -> u64 {
        let l = self.listing();
        self.selected().map(|i| &l.entries[i as usize]).filter(|e| !e.is_dir() || e.attrs & attr::SIZE_KNOWN != 0).map(|e| e.size).sum()
    }

    fn remove_row(snap: &mut DirSnapshot, i: u32) {
        let row = sort::find_row(&snap.listing, &snap.keys, snap.view.sort, &snap.order, i)
            .or_else(|| snap.order.iter().position(|&p| p == i));
        if let Some(r) = row {
            snap.order.remove(r);
        }
    }

    fn insert_row(snap: &mut DirSnapshot, i: u32) {
        if sort::visible(&snap.listing.entries[i as usize], snap.view.show_hidden) {
            let p = sort::insert_pos(&snap.listing, &snap.keys, snap.view.sort, &snap.order, i);
            snap.order.insert(p, i);
        }
    }

    fn update(&mut self, i: u32, meta: Meta) {
        let snap = Arc::make_mut(&mut self.snap);
        Self::remove_row(snap, i);
        snap.listing.set_meta(i, meta);
        Self::insert_row(snap, i);
    }

    pub fn apply(&mut self, c: &DirChange) {
        let existing = self.find(&c.name);
        match (&c.kind, existing) {
            (ChangeKind::Removed, Some(i)) => {
                let snap = Arc::make_mut(&mut self.snap);
                Self::remove_row(snap, i);
                snap.listing.entries[i as usize].attrs |= attr::DELETED;
                if let Some(m) = &mut self.names {
                    if let Some(v) = m.get_mut(&hash::hash(c.name.as_slice())) {
                        v.retain(|&mut x| x != i);
                    }
                }
                self.set_selected(i, false);
                if self.focus == Some(i) {
                    self.focus = None;
                }
            }
            (ChangeKind::Removed, None) => {}
            (_, Some(i)) => self.update(i, c.meta.unwrap_or_default()),
            (_, None) => {
                let snap = Arc::make_mut(&mut self.snap);
                let i = snap.listing.push(&c.name, c.meta.unwrap_or_default());
                snap.keys.push(snap.listing.name(i));
                Self::insert_row(snap, i);
                if let Some(m) = &mut self.names {
                    m.entry(hash::hash(c.name.as_slice())).or_default().push(i);
                }
            }
        }
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
    }

    pub fn merge_order(&mut self, mut order: Vec<u32>, view: ViewSpec, base_len: usize) {
        let snap = Arc::make_mut(&mut self.snap);
        let l = &snap.listing;
        order.retain(|&i| sort::visible(&l.entries[i as usize], view.show_hidden));
        snap.order = order;
        snap.view = view;
        for i in base_len as u32..snap.listing.len() as u32 {
            Self::insert_row(snap, i);
        }
        let s = &*snap;
        let sorted = s.order.windows(2).all(|w| sort::cmp(&s.listing, &s.keys, view.sort, w[0], w[1]).is_lt());
        if !sorted {
            let mut o = std::mem::take(&mut snap.order);
            sort::sort(&snap.listing, &snap.keys, view, &mut o);
            snap.order = o;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::path::FsPath;
    use crate::sort::{CaseMode, SortField, SortKeys, SortSpec};
    use std::time::Duration;

    pub(crate) fn snap(names: &[&str]) -> Arc<DirSnapshot> {
        let mut listing = Listing::default();
        let mut keys = SortKeys::new(CaseMode::Insensitive);
        for n in names {
            let i = listing.push(n.as_bytes(), Meta { size: n.len() as u64, ..Default::default() });
            keys.push(listing.name(i));
        }
        let view = ViewSpec::default();
        let mut order = Vec::new();
        sort::sort(&listing, &keys, view, &mut order);
        Arc::new(DirSnapshot {
            gen: 1,
            path: FsPath::parse("C:\\t").unwrap(),
            listing,
            keys,
            order,
            view,
            done: true,
            error: None,
            elapsed: Duration::ZERO,
        })
    }

    fn rows(v: &DirView) -> Vec<String> {
        v.rows().iter().map(|&i| String::from_utf8_lossy(v.listing().name(i)).into_owned()).collect()
    }

    fn change(kind: ChangeKind, name: &str, size: u64) -> DirChange {
        DirChange { kind, name: name.into(), meta: Some(Meta { size, ..Default::default() }) }
    }

    #[test]
    fn diffs_keep_order_and_indices() {
        let mut v = DirView::new(snap(&["b", "d", "f"]));
        v.set_selected(1, true);
        v.apply(&change(ChangeKind::Added, "a", 1));
        v.apply(&change(ChangeKind::Added, "e", 1));
        v.apply(&change(ChangeKind::Removed, "b", 0));
        v.apply(&change(ChangeKind::Removed, "zzz", 0));
        v.apply(&change(ChangeKind::Added, "e", 7));
        assert_eq!(rows(&v), ["a", "d", "e", "f"]);
        assert!(v.is_selected(1), "index of 'd' is stable");
        assert_eq!(v.listing().len(), 5, "tombstone kept");
        assert_eq!(v.listing().entries[4].size, 7);
        assert_eq!(v.find(b"b"), None);
        assert_eq!(v.row_of(1), Some(1));
    }

    #[test]
    fn modified_moves_row_under_size_sort() {
        let s = snap(&["aa", "b", "cccc"]);
        let mut v = DirView::new(s);
        let view = ViewSpec { sort: SortSpec { field: SortField::Size, descending: false }, show_hidden: false };
        let mut order = Vec::new();
        sort::sort(v.listing(), &v.snap.keys, view, &mut order);
        let n = v.listing().len();
        v.merge_order(order, view, n);
        assert_eq!(rows(&v), ["b", "aa", "cccc"]);
        v.apply(&change(ChangeKind::Modified, "b", 100));
        assert_eq!(rows(&v), ["aa", "cccc", "b"]);
    }

    #[test]
    fn merge_after_concurrent_changes() {
        let mut v = DirView::new(snap(&["a", "c"]));
        let view = ViewSpec { sort: SortSpec { field: SortField::Name, descending: true }, show_hidden: false };
        let mut order = Vec::new();
        sort::sort(v.listing(), &v.snap.keys, view, &mut order);
        v.apply(&change(ChangeKind::Added, "b", 1));
        v.apply(&change(ChangeKind::Removed, "c", 0));
        v.merge_order(order, view, 2);
        assert_eq!(rows(&v), ["b", "a"]);
    }

    #[test]
    fn selection_ops() {
        let mut v = DirView::new(snap(&["a", "b", "c", "d"]));
        v.select_rows(3, 1);
        assert_eq!(v.selected().collect::<Vec<_>>(), [1, 2, 3]);
        assert_eq!(v.selected_size(), 3);
        v.select_all();
        assert_eq!(v.selected_count(), 4);
        v.set_selected(0, false);
        v.set_selected(200, false);
        assert_eq!(v.selected_count(), 3);
    }
}
