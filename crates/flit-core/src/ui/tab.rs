use super::columns::Pane;
use super::tree::TreeState;
use crate::hash::FxHashSet;
use super::field::Field;
use super::details::Columns;
use super::layouts::{LayoutMode, Sizes};
use super::{Effect, LoadStats};
use crate::input::Mods;
use crate::library;
use crate::listing::{attr, DirChange, DirSnapshot, Listing};
use crate::location::{Location, ProviderId};
use crate::name;
use crate::nav::History;
use crate::path::FsPath;
use crate::provider::{GenToken, ProviderRequest, SnapshotSlot, TabId};
use crate::recycle;
use crate::sort::{CaseMode, GroupField, GroupSpec, SortField, SortKeys, SortSpec, ViewSpec};
use crate::view::DirView;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

const TYPE_AHEAD_RESET: Duration = Duration::from_millis(1000);
pub(super) const SUBFOLDERS_MARK: &str = " [...]";

pub(super) struct Loading {
    pub gen: u64,
    pub since: Instant,
    pub select: Option<Vec<u8>>,
    scanned: u32,
    pub note: Option<String>,
}

pub struct Tab {
    pub id: TabId,
    pub(super) history: History,
    pub(super) gen: u64,
    pub(super) cancel: GenToken,
    pub(super) slot: Arc<SnapshotSlot>,
    pub(super) watching: Option<ProviderId>,
    pub(super) view_spec: ViewSpec,
    pub(super) columns: Columns,
    pub(super) mode: LayoutMode,
    pub(super) look: LayoutMode,
    pub(super) sizes: Sizes,
    pub(super) fit_pending: bool,
    pub(super) fit_column: Option<super::Column>,
    pub(super) loading: Option<Loading>,
    pub(super) view: Option<DirView>,
    pub(super) view_loc: Location,
    pub(super) error: Option<String>,
    pub(super) pending_diffs: VecDeque<DirChange>,
    pub(super) crumbs: Vec<(String, Location)>,
    pub(super) trail: Location,
    pub(super) ghosts: Vec<(String, Location)>,
    pub stats: LoadStats,
    pub(super) scroll: f32,
    pub(super) scroll_target: f32,
    pub(super) reveal_pending: bool,
    type_ahead: String,
    type_ahead_at: Option<Instant>,
    pub(super) filter: Option<Field>,
    pub(super) panes: Vec<Pane>,
    pub(super) git: Option<std::sync::Arc<crate::git::GitInfo>>,
    pub(super) zoom: u8,
    pub(super) subfolders: bool,
    pub(super) deep_gen: u64,
    pub(super) tree: TreeState,
    pub(super) hidden_types: FxHashSet<u64>,
    pub(super) retain_filter: bool,
}

impl Tab {
    pub(super) fn new(id: TabId, at: Location, view_spec: ViewSpec) -> Self {
        Self {
            id,
            view_loc: at.clone(),
            trail: at.clone(),
            ghosts: Vec::new(),
            history: History::new(at),
            gen: 0,
            cancel: GenToken::default(),
            slot: Arc::default(),
            watching: None,
            view_spec,
            columns: Columns::default(),
            mode: LayoutMode::default(),
            look: LayoutMode::default(),
            sizes: Sizes::default(),
            fit_pending: false,
            fit_column: None,
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
            panes: Vec::new(),
            git: None,
            zoom: 0,
            subfolders: false,
            deep_gen: 0,
            tree: TreeState::default(),
            hidden_types: FxHashSet::default(),
            retain_filter: false,
        }
    }

    pub fn location(&self) -> &Location {
        self.history.current()
    }

    pub fn title(&self) -> String {
        let mut s = match self.crumbs.last() {
            Some((label, _)) => label.clone(),
            None => name::display(self.location().file_name()).into_owned(),
        };
        if self.subfolders {
            s.push_str(SUBFOLDERS_MARK);
        }
        s
    }

    pub(super) fn load(&mut self, gen: u64, select: Option<Vec<u8>>, case: CaseMode, now: Instant, fx: &mut Vec<Effect>) {
        self.gen = gen;
        self.cancel.set(gen);
        let loc = self.location().clone();
        if loc != self.view_loc {
            if !self.retain_filter {
                self.reset_filter();
            }
            self.tree.reset();
            self.hidden_types.clear();
        }
        self.tree.drop_probes();
        if self.view_spec.sort.field == SortField::Origin && !loc.is_recycle_bin() {
            self.view_spec.sort = SortSpec::default();
        }
        if self.view_spec.group.field == GroupField::Origin && !loc.is_recycle_bin() {
            self.view_spec.group = GroupSpec::default();
        }
        let provider = loc.provider();
        if let Some(old) = self.watching.take().filter(|p| *p != provider) {
            fx.push(Effect::Provider { provider: old, req: ProviderRequest::Watch { tab: self.id, gen, path: None } });
        }
        self.watching = Some(provider.clone());
        self.loading = Some(Loading { gen, since: now, select, scanned: 0, note: None });
        self.pending_diffs.clear();
        self.error = None;
        self.crumbs = loc.crumbs();
        self.follow_trail();
        self.stats = LoadStats::default();
        self.keep_git_root();
        let path = loc.path().clone();
        if loc.is_home_page() {
            fx.push(Effect::Provider { provider, req: ProviderRequest::Watch { tab: self.id, gen, path: None } });
            let empty = DirSnapshot { gen, path, listing: Listing::default(), keys: SortKeys::new(case), order: Vec::new(), view: self.view_spec, done: true, error: None, elapsed: Duration::ZERO };
            return self.slot.publish(Arc::new(empty));
        }
        if self.mode == LayoutMode::Adaptive {
            self.slot.ask_kind(gen);
        }
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
                self.tree.loaded.clear();
                self.apply_shape();
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
            if v.snap().view.deep {
                self.deep_gen = v.snap().gen;
            }
            self.stats.done = Some(v.snap().elapsed);
            self.loading = None;
            self.check_trail();
        }
        done && stale_view
    }

    pub(super) fn apply_diffs(&mut self) -> bool {
        let Some(v) = self.view.as_mut().filter(|v| v.snap().done) else { return false };
        if self.loading.is_some() || self.pending_diffs.is_empty() {
            return false;
        }
        v.apply_all(self.pending_diffs.drain(..));
        self.check_trail();
        true
    }

    pub(super) fn crumb(&self, i: usize) -> Option<&(String, Location)> {
        self.crumbs.get(i).or_else(|| self.ghosts.get(i.checked_sub(self.crumbs.len())?))
    }

    pub(super) fn follow_trail(&mut self) {
        let here = self.location().clone();
        if !std::iter::successors(Some(self.trail.clone()), Location::parent).any(|l| l == here) {
            self.trail = here.clone();
        }
        let mut ghosts = if self.trail == here { Vec::new() } else { self.trail.crumbs() };
        self.ghosts = ghosts.split_off(here.crumbs().len().min(ghosts.len()));
    }

    pub(super) fn drop_trail(&mut self) {
        self.trail = self.location().clone();
        self.ghosts.clear();
    }

    fn check_trail(&mut self) {
        let Some(name) = self.ghosts.first().map(|g| g.1.file_name().to_vec()) else { return };
        let here = self.history.current();
        let Some(v) = self.view.as_mut().filter(|v| v.snap().done && v.snap().error.is_none() && self.view_loc == *here) else { return };
        let folder = |l: &Listing, i: u32| l.entries[i as usize].attrs & (attr::DIR | attr::DELETED) == attr::DIR;
        let exact = v.find(&name).is_some_and(|i| folder(v.listing(), i));
        let wanted = name::display(&name).to_lowercase();
        let l = v.listing();
        if !exact && !(0..l.len() as u32).any(|i| folder(l, i) && name::display(l.name(i)).to_lowercase() == wanted) {
            self.drop_trail();
        }
    }

    pub(super) fn focused_name(&self) -> Option<Vec<u8>> {
        let v = self.view.as_ref()?;
        Some(v.listing().name(v.focus?).to_vec())
    }

    pub(super) fn entry_location(&self, i: u32) -> Location {
        let l = self.view.as_ref().unwrap().listing();
        match self.view_loc.is_recycle_bin() {
            true => recycle::original(l, i).map_or_else(|| self.view_loc.clone(), Location::from_local),
            false => self.entry_dir(i).join(l.name(i)),
        }
    }

    pub(super) fn entry_dir(&self, i: u32) -> Location {
        let folder = self.view.as_ref().filter(|_| library::is_library(self.view_loc.path())).and_then(|v| FsPath::parse_exact(library::folder(v.listing(), i)));
        folder.map_or_else(|| self.view_loc.clone(), Location::from_local)
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

    pub(super) fn move_focus(&mut self, delta: isize, m: Mods) {
        let Some(v) = &self.view else { return };
        let n = v.rows().len();
        if n == 0 {
            return;
        }
        let cur = v.focus.and_then(|f| v.row_of(f)).map(|r| r as isize);
        let base = cur.unwrap_or(if delta < 0 { n as isize } else { -1 });
        let row = base.saturating_add(delta).clamp(0, n as isize - 1) as usize;
        self.focus_row(row, m);
    }

    pub(super) fn typing(&self, now: Instant) -> bool {
        self.type_ahead_at.is_some_and(|t| now.duration_since(t) <= TYPE_AHEAD_RESET)
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
            .map(|&i| {
                let mut rel = Vec::new();
                crate::view::rel_path(l, i, v.separator(), &mut rel);
                (i, rel)
            })
            .collect()
    }
}
