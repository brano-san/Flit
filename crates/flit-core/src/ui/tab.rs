use super::field::Field;
use super::{Effect, LoadStats};
use crate::input::Mods;
use crate::listing::{attr, DirChange, DirSnapshot};
use crate::location::{Location, ProviderId};
use crate::name;
use crate::nav::History;
use crate::provider::{GenToken, ProviderRequest, SnapshotSlot, TabId};
use crate::sort::{CaseMode, ViewSpec};
use crate::view::DirView;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

const TYPE_AHEAD_RESET: Duration = Duration::from_millis(1000);
const DIFF_CHUNK: usize = 256;

pub(super) struct Loading {
    pub gen: u64,
    pub since: Instant,
    pub select: Option<Vec<u8>>,
    scanned: u32,
}

pub struct Tab {
    pub id: TabId,
    pub(super) history: History,
    pub(super) gen: u64,
    pub(super) cancel: GenToken,
    pub(super) slot: Arc<SnapshotSlot>,
    pub(super) watching: Option<ProviderId>,
    pub(super) view_spec: ViewSpec,
    pub(super) loading: Option<Loading>,
    pub(super) view: Option<DirView>,
    pub(super) view_loc: Location,
    pub(super) error: Option<String>,
    pub(super) pending_diffs: VecDeque<DirChange>,
    pub(super) crumbs: Vec<(String, Location)>,
    pub stats: LoadStats,
    pub(super) scroll: f32,
    pub(super) scroll_target: f32,
    pub(super) reveal_pending: bool,
    type_ahead: String,
    type_ahead_at: Option<Instant>,
    pub(super) filter: Option<Field>,
}

impl Tab {
    pub(super) fn new(id: TabId, at: Location, view_spec: ViewSpec) -> Self {
        Self {
            id,
            view_loc: at.clone(),
            history: History::new(at),
            gen: 0,
            cancel: GenToken::default(),
            slot: Arc::default(),
            watching: None,
            view_spec,
            loading: None,
            view: None,
            error: None,
            pending_diffs: VecDeque::new(),
            crumbs: Vec::new(),
            stats: LoadStats::default(),
            scroll: 0.0,
            scroll_target: 0.0,
            reveal_pending: false,
            type_ahead: String::new(),
            type_ahead_at: None,
            filter: None,
        }
    }

    pub fn location(&self) -> &Location {
        self.history.current()
    }

    pub fn title(&self) -> String {
        match self.crumbs.last() {
            Some((label, _)) => label.clone(),
            None => name::display(self.location().file_name()).into_owned(),
        }
    }

    pub(super) fn load(&mut self, gen: u64, select: Option<Vec<u8>>, case: CaseMode, now: Instant, fx: &mut Vec<Effect>) {
        self.gen = gen;
        self.cancel.set(gen);
        let loc = self.location().clone();
        if loc != self.view_loc {
            self.reset_filter();
        }
        let provider = loc.provider();
        if let Some(old) = self.watching.take().filter(|p| *p != provider) {
            fx.push(Effect::Provider { provider: old, req: ProviderRequest::Watch { tab: self.id, gen, path: None } });
        }
        self.watching = Some(provider.clone());
        self.loading = Some(Loading { gen, since: now, select, scanned: 0 });
        self.pending_diffs.clear();
        self.error = None;
        self.crumbs = loc.crumbs();
        self.stats = LoadStats::default();
        let path = loc.path().clone();
        fx.push(Effect::Provider { provider: provider.clone(), req: ProviderRequest::Watch { tab: self.id, gen, path: Some(path.clone()) } });
        fx.push(Effect::Provider {
            provider,
            req: ProviderRequest::List { tab: self.id, gen, path, view: self.view_spec, case, out: self.slot.clone(), cancel: self.cancel.clone() },
        });
    }

    pub(super) fn shutdown(&mut self, fx: &mut Vec<Effect>) {
        self.cancel.set(u64::MAX);
        if let Some(p) = self.watching.take() {
            fx.push(Effect::Provider { provider: p, req: ProviderRequest::Watch { tab: self.id, gen: self.gen, path: None } });
        }
    }

    pub(super) fn on_snapshot(&mut self, snap: Arc<DirSnapshot>) -> bool {
        if snap.gen != self.gen {
            return false;
        }
        if self.stats.first.is_none() {
            self.stats.first = Some(snap.elapsed);
        }
        self.stats.entries = snap.listing.len();
        if let Some(e) = &snap.error {
            self.error = Some(e.to_string());
        }
        let done = snap.done;
        let stale_view = snap.view != self.view_spec;
        match &mut self.view {
            Some(v) if v.snap().gen == snap.gen => v.replace(snap),
            _ => {
                self.view = Some(DirView::new(snap));
                self.view_loc = self.location().clone();
                self.scroll = 0.0;
                self.scroll_target = 0.0;
                self.refilter_view();
            }
        }
        let v = self.view.as_mut().unwrap();
        if let Some(l) = self.loading.as_mut() {
            if let Some(name) = &l.select {
                let listing = v.listing();
                let found = (l.scanned..listing.len() as u32).find(|&i| listing.name(i) == name.as_slice() && v.is_visible(i));
                l.scanned = listing.len() as u32;
                if let Some(i) = found {
                    v.clear_selection();
                    v.focus = Some(i);
                    v.anchor = Some(i);
                    v.set_selected(i, true);
                    l.select = None;
                    self.reveal_pending = true;
                }
            }
        }
        if done {
            self.stats.done = Some(v.snap().elapsed);
            self.loading = None;
        }
        done && stale_view
    }

    pub(super) fn apply_diffs(&mut self, budget: Duration) -> bool {
        let Some(v) = self.view.as_mut().filter(|v| v.snap().done) else { return false };
        if self.loading.is_some() || self.pending_diffs.is_empty() {
            return false;
        }
        let (start, pending) = (Instant::now(), &mut self.pending_diffs);
        let mut n = 0;
        v.apply_all(std::iter::from_fn(|| {
            n += 1;
            if n % DIFF_CHUNK == 0 && start.elapsed() >= budget {
                return None;
            }
            pending.pop_front()
        }));
        !self.pending_diffs.is_empty()
    }

    pub(super) fn focused_name(&self) -> Option<Vec<u8>> {
        let v = self.view.as_ref()?;
        Some(v.listing().name(v.focus?).to_vec())
    }

    pub(super) fn entry_location(&self, i: u32) -> Location {
        self.view_loc.join(self.view.as_ref().unwrap().listing().name(i))
    }

    pub(super) fn selection(&self) -> Vec<u32> {
        let Some(v) = &self.view else { return Vec::new() };
        let mut sel: Vec<u32> = v.selected().collect();
        if sel.is_empty() {
            sel.extend(v.focus);
        }
        sel
    }

    pub fn rows(&self) -> usize {
        self.view.as_ref().map_or(0, |v| v.rows().len())
    }

    pub(super) fn max_scroll(&self, row_h: f32, list_h: f32) -> f32 {
        (self.rows() as f32 * row_h - list_h).max(0.0)
    }

    pub(super) fn row_range(&self, row_h: f32, list_h: f32) -> (usize, usize) {
        let n = self.rows();
        let rh = row_h.max(1.0);
        let first = (self.scroll / rh).floor() as usize;
        let last = ((self.scroll + list_h) / rh).ceil() as usize;
        (first.min(n), last.min(n))
    }

    pub(super) fn reveal_focus(&mut self, center: bool, row_h: f32, list_h: f32) {
        let Some(row) = self.view.as_ref().and_then(|v| v.focus.and_then(|f| v.row_of(f))) else { return };
        let (top, bottom) = (row as f32 * row_h, (row + 1) as f32 * row_h);
        let mut s = self.scroll_target;
        if center && (top < s || bottom > s + list_h) {
            s = top - (list_h - row_h) / 2.0;
        } else if top < s {
            s = top;
        } else if bottom > s + list_h {
            s = bottom - list_h;
        }
        self.scroll_target = s.clamp(0.0, self.max_scroll(row_h, list_h));
        self.scroll = self.scroll_target;
    }

    pub(super) fn move_focus(&mut self, delta: isize, m: Mods) {
        let Some(v) = &mut self.view else { return };
        let n = v.rows().len();
        if n == 0 {
            return;
        }
        let cur = v.focus.and_then(|f| v.row_of(f)).map(|r| r as isize);
        let base = cur.unwrap_or(if delta < 0 { n as isize } else { -1 });
        let row = base.saturating_add(delta).clamp(0, n as isize - 1) as usize;
        let i = v.rows()[row];
        if m.shift {
            let anchor_row = v.anchor.and_then(|a| v.row_of(a)).unwrap_or(row);
            if !m.ctrl {
                v.clear_selection();
            }
            v.select_rows(anchor_row, row);
        } else if !m.ctrl {
            v.clear_selection();
            v.set_selected(i, true);
            v.anchor = Some(i);
        }
        v.focus = Some(i);
    }

    pub(super) fn type_ahead(&mut self, c: char, now: Instant) -> bool {
        if self.type_ahead_at.is_some_and(|t| now.duration_since(t) > TYPE_AHEAD_RESET) {
            self.type_ahead.clear();
        }
        self.type_ahead_at = Some(now);
        self.type_ahead.push(c);
        let Some(v) = &mut self.view else { return false };
        let n = v.rows().len();
        if n == 0 {
            return false;
        }
        let first = self.type_ahead.chars().next().unwrap();
        let count = self.type_ahead.chars().count();
        let repeated = count > 1 && self.type_ahead.chars().all(|c| c == first);
        let cur = v.focus.and_then(|f| v.row_of(f));
        let find = |needle: &str, start: usize, v: &DirView| -> Option<usize> {
            (0..n).map(|k| (start + k) % n).find(|&r| name::starts_with_ci(v.listing().name(v.rows()[r]), needle))
        };
        let single = first.to_string();
        let next = cur.map_or(0, |r| r + 1);
        let found = if count == 1 {
            find(&single, next, v)
        } else {
            find(&self.type_ahead, cur.unwrap_or(0), v).or_else(|| if repeated { find(&single, next, v) } else { None })
        };
        let Some(r) = found else { return false };
        let i = v.rows()[r];
        v.clear_selection();
        v.set_selected(i, true);
        v.focus = Some(i);
        v.anchor = Some(i);
        true
    }

    pub(super) fn size_items(&self) -> Vec<(u32, Vec<u8>)> {
        let Some(v) = &self.view else { return Vec::new() };
        let l = v.listing();
        v.rows()
            .iter()
            .filter(|&&i| {
                let e = &l.entries[i as usize];
                e.is_dir() && e.attrs & (attr::LINK | attr::VOLUME) == 0
            })
            .map(|&i| (i, l.name(i).to_vec()))
            .collect()
    }
}
