use super::{App, Effect, LayoutMode, Tab};
use crate::hash::FxHashSet;
use crate::input::Mods;
use crate::provider::{GenToken, ProviderRequest, SnapshotSlot};
use crate::render::Rect;
use crate::view::{rel_path, Line};
use std::sync::Arc;

pub(super) struct Probe {
    rel: Vec<u8>,
    gen: u64,
    slot: Arc<SnapshotSlot>,
    cancel: GenToken,
    all: bool,
}

#[derive(Default)]
pub(super) struct TreeState {
    pub open: FxHashSet<Vec<u8>>,
    pub loaded: FxHashSet<Vec<u8>>,
    pub probes: Vec<Probe>,
}

impl TreeState {
    pub(super) fn reset(&mut self) {
        self.open.clear();
        self.loaded.clear();
        self.drop_probes();
    }

    pub(super) fn drop_probes(&mut self) {
        for p in self.probes.drain(..) {
            p.cancel.set(u64::MAX);
        }
    }

    fn close(&mut self, rel: &[u8], sep: u8, all: bool) {
        self.open.remove(rel);
        if all {
            self.open.retain(|k| !(k.len() > rel.len() && k.starts_with(rel) && k[rel.len()] == sep));
        }
    }
}

impl Tab {
    pub(super) fn apply_shape(&mut self) {
        let tree = (self.mode == LayoutMode::Tree).then(|| self.tree.open.clone());
        let hidden = self.hidden_types.clone();
        if let Some(v) = &mut self.view {
            v.set_shape(tree, hidden);
        }
    }

    pub(super) fn deep_loaded(&self) -> bool {
        self.view.as_ref().is_some_and(|v| v.snap().gen == self.deep_gen && v.snap().done)
    }
}

impl App {
    pub(super) fn shape_view(&mut self) {
        self.tab_mut().apply_shape();
        self.layout();
        self.dirty = true;
    }

    pub(super) fn tree_slot(&self, c: Rect) -> Option<(usize, bool)> {
        if self.layout.mode != LayoutMode::Tree {
            return None;
        }
        let v = self.tab().view.as_ref()?;
        let line = self.line_at(&self.flow(), c.x + 1.0, c.y + c.h / 2.0)?;
        let Line::Row(r) = v.line(line)? else { return None };
        let i = *v.rows().get(r)?;
        Some((v.depth(i), v.listing().entries[i as usize].is_dir()))
    }

    pub(super) fn chevron_rect(&self, row: usize) -> Option<Rect> {
        let c = self.row_rect(row)?;
        let (depth, dir) = self.tree_slot(c)?;
        let ind = self.tree_indent();
        dir.then(|| Rect::new(c.x + self.s(10.0) + ind * depth as f32, c.y, ind, c.h))
    }

    pub(super) fn on_chevron(&self, x: f32, y: f32, row: usize) -> bool {
        self.chevron_rect(row).is_some_and(|r| r.contains(x, y))
    }

    fn rel_of(&self, i: u32) -> Option<Vec<u8>> {
        let v = self.tab().view.as_ref()?;
        let mut rel = Vec::new();
        rel_path(v.listing(), i, v.separator(), &mut rel);
        Some(rel)
    }

    pub(super) fn is_open_in_tree(&self, i: u32) -> bool {
        self.rel_of(i).is_some_and(|r| self.tab().tree.open.contains(&r))
    }

    fn tree_targets(&self, parents: bool) -> Vec<u32> {
        let t = self.tab();
        let Some(v) = &t.view else { return Vec::new() };
        let l = v.listing();
        let mut out = Vec::new();
        for i in t.selection() {
            if l.entries[i as usize].is_dir() {
                out.push(i);
            } else if parents {
                out.extend(self.parent_of(i));
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    fn parent_of(&self, i: u32) -> Option<u32> {
        let v = self.tab().view.as_ref()?;
        let l = v.listing();
        let folder = l.folder(i);
        if folder.is_empty() {
            return None;
        }
        let sep = v.separator();
        let (up, name) = match folder.iter().rposition(|&b| b == sep) {
            Some(k) => (&folder[..k], &folder[k + 1..]),
            None => (&b""[..], folder),
        };
        (0..l.len() as u32).find(|&p| l.entries[p as usize].attrs & (crate::listing::attr::DIR | crate::listing::attr::DELETED) == crate::listing::attr::DIR && l.name(p) == name && l.folder(p) == up)
    }

    pub(super) fn tree_open(&mut self, all: bool) {
        for i in self.tree_targets(false) {
            if let Some(rel) = self.rel_of(i) {
                self.expand(rel, all);
            }
        }
        self.shape_view();
    }

    pub(super) fn tree_close(&mut self, all: bool) {
        let targets = self.tree_targets(true);
        let sep = self.tab().view.as_ref().map_or(b'\\', |v| v.separator());
        for i in targets {
            if let Some(rel) = self.rel_of(i) {
                self.tab_mut().tree.close(&rel, sep, all);
            }
        }
        self.shape_view();
        self.reveal_focus(false);
    }

    pub(super) fn tree_toggle_row(&mut self, row: usize, all: bool) {
        let Some(i) = self.tab().view.as_ref().and_then(|v| v.rows().get(row).copied()) else { return };
        let Some(rel) = self.rel_of(i) else { return };
        let sep = self.tab().view.as_ref().map_or(b'\\', |v| v.separator());
        if self.tab().tree.open.contains(&rel) {
            self.tab_mut().tree.close(&rel, sep, all);
        } else {
            self.expand(rel, all);
        }
        self.shape_view();
    }

    pub(super) fn tree_key(&mut self, right: bool, m: Mods) -> bool {
        if self.layout.mode != LayoutMode::Tree || m.ctrl || m.alt {
            return false;
        }
        let Some((i, dir)) = self.tab().view.as_ref().and_then(|v| v.focus.filter(|&i| v.is_visible(i)).map(|i| (i, v.listing().entries[i as usize].is_dir()))) else { return false };
        let open = dir && self.is_open_in_tree(i);
        match (right, m.shift) {
            (true, all) if dir && (!open || all) => self.tree_open(all),
            (true, _) => {
                self.tab_mut().move_focus(1, Mods::default());
                self.reveal_focus(false);
            }
            (false, all) if open || (all && dir) => self.tree_close(all),
            (false, all) => {
                if let Some(p) = self.parent_of(i) {
                    if all {
                        if let Some(rel) = self.rel_of(p) {
                            let sep = self.tab().view.as_ref().map_or(b'\\', |v| v.separator());
                            self.tab_mut().tree.close(&rel, sep, true);
                        }
                    } else if let Some(rel) = self.rel_of(p) {
                        self.tab_mut().tree.open.remove(&rel);
                    }
                    if let Some(v) = &mut self.tab_mut().view {
                        v.clear_selection();
                        v.set_selected(p, true);
                        v.focus = Some(p);
                        v.anchor = Some(p);
                    }
                    self.shape_view();
                    self.reveal_focus(false);
                }
            }
        }
        true
    }

    fn expand(&mut self, rel: Vec<u8>, all: bool) {
        let deep = self.tab().deep_loaded();
        let t = self.tab_mut();
        t.tree.open.insert(rel.clone());
        if deep || t.tree.loaded.contains(&rel) {
            if all {
                for child in self.child_dirs(&rel) {
                    self.expand(child, true);
                }
            }
            return;
        }
        if let Some(p) = t.tree.probes.iter_mut().find(|p| p.rel == rel) {
            p.all |= all;
            return;
        }
        let gen = self.gen();
        let t = self.tab_mut();
        let loc = t.view_loc.join(&rel);
        let probe = Probe { rel, gen, slot: Arc::default(), cancel: GenToken::default(), all };
        probe.cancel.set(gen);
        let mut view = t.view_spec;
        (view.deep, view.tree) = (false, false);
        let req = ProviderRequest::List { tab: 0, gen, path: loc.path().clone(), view, case: self.case, out: probe.slot.clone(), cancel: probe.cancel.clone() };
        self.tab_mut().tree.probes.push(probe);
        self.effects.push(Effect::Provider { provider: loc.provider(), req });
    }

    fn child_dirs(&self, rel: &[u8]) -> Vec<Vec<u8>> {
        let Some(v) = &self.tab().view else { return Vec::new() };
        let (l, sep) = (v.listing(), v.separator());
        let mut out = Vec::new();
        let mut path = Vec::new();
        for i in 0..l.len() as u32 {
            let e = &l.entries[i as usize];
            if e.is_dir() && e.attrs & crate::listing::attr::DELETED == 0 && l.folder(i) == rel {
                rel_path(l, i, sep, &mut path);
                out.push(path.clone());
            }
        }
        out
    }

    pub(super) fn reopen_tree(&mut self, k: usize) {
        if k != self.cur || self.tabs[k].mode != LayoutMode::Tree {
            return;
        }
        let mut open: Vec<Vec<u8>> = self.tabs[k].tree.open.iter().cloned().collect();
        open.sort_unstable_by_key(|r| r.len());
        for rel in open {
            self.expand(rel, false);
        }
    }

    pub(super) fn poll_tree(&mut self) -> bool {
        let mut changed = false;
        for k in 0..self.tabs.len() {
            let t = &mut self.tabs[k];
            let mut done = Vec::new();
            t.tree.probes.retain(|p| match p.slot.take().filter(|s| s.gen == p.gen && s.done) {
                Some(snap) => {
                    done.push((p.rel.clone(), snap, p.all));
                    false
                }
                None => true,
            });
            for (rel, snap, all) in done {
                let t = &mut self.tabs[k];
                t.tree.loaded.insert(rel.clone());
                if let Some(v) = &mut t.view {
                    v.graft(&rel, &snap.listing);
                }
                if all && k == self.cur {
                    for child in self.child_dirs(&rel) {
                        self.expand(child, true);
                    }
                }
                changed |= k == self.cur;
            }
            if changed && k == self.cur {
                self.tabs[k].apply_shape();
            }
        }
        if changed {
            self.layout();
        }
        changed
    }

    pub(super) fn toggle_subfolders(&mut self) {
        let on = !self.tab().subfolders;
        let reload = on && !self.tab().deep_loaded();
        self.change_view(|t| {
            t.subfolders = on;
            t.view_spec.deep = on;
        });
        if reload {
            let k = self.cur;
            self.load_tab(k, self.tab().focused_name(), std::time::Instant::now());
        }
        self.shape_view();
    }

    pub(super) fn toggle_kind(&mut self, dirs: bool) {
        self.change_view(|t| {
            let v = &mut t.view_spec;
            if dirs {
                v.hide_dirs = !v.hide_dirs;
            } else {
                v.hide_files = !v.hide_files;
            }
        });
    }
}

#[cfg(test)]
#[path = "../../test/ui/tree.rs"]
mod tests;
