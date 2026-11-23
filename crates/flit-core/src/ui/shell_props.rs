use super::{App, Column, Effect, PORTABLE_COLUMNS, SHELL_COLUMNS};
use crate::hash::FxHashMap;
use crate::listing::{attr, ColumnValues, PropKey};
use crate::location::Location;
use crate::path::{FsPath, PathKind};
use crate::provider::TabId;
use crate::sort;
use std::time::{Duration, Instant};

const MAX_ASK: usize = 256;
const BULK: usize = 64;
const BULK_IN_FLIGHT: usize = 2;
pub(super) const RESORT_EVERY: Duration = Duration::from_millis(250);

#[derive(Default)]
pub(super) struct ShellProps {
    gen: u64,
    portable: bool,
    system: bool,
    probe: bool,
    values: FxHashMap<Vec<u8>, ColumnValues>,
    asked: FxHashMap<Vec<u8>, u64>,
    next_ask: u64,
    bulk: Vec<u64>,
    cursor: u32,
    pending: Vec<(Vec<u8>, Option<PropKey>)>,
    applied: Option<Instant>,
}

impl ShellProps {
    pub(super) fn is_elf(&self, gen: u64, name: &[u8]) -> bool {
        self.gen == gen && self.values.get(name).is_some_and(|v| v.elf)
    }

    pub(super) fn is_idle(&self) -> bool {
        self.asked.is_empty() && self.values.is_empty() && self.pending.is_empty()
    }

    pub(super) fn invalidate(&mut self) {
        *self = Self { next_ask: self.next_ask, ..Self::default() };
    }

    pub(super) fn forget(&mut self, name: &[u8], at: Option<u32>) {
        self.values.remove(name);
        self.asked.remove(name);
        self.pending.retain(|(n, _)| n != name);
        if let Some(i) = at {
            self.cursor = self.cursor.min(i);
        }
    }

    fn ask(&mut self, names: &[(Vec<u8>, bool)]) -> u64 {
        self.next_ask += 1;
        for (n, _) in names {
            self.asked.insert(n.clone(), self.next_ask);
        }
        self.next_ask
    }
}

fn readable(attrs: u32) -> bool {
    attrs & (attr::DIR | attr::NESTED | attr::DELETED) == 0
}

impl App {
    pub(super) fn props_dir(&self) -> Option<FsPath> {
        let t = self.tab();
        let Location::Local(dir) = &t.view_loc else { return None };
        let local = matches!(dir.kind(), PathKind::Drive | PathKind::Unc);
        let local = (self.caps.unix_paths && dir.kind() == PathKind::Unix) || (!self.caps.unix_paths && local);
        (local && crate::archive::split(dir).is_none() && !t.view_loc.is_recycle_bin()).then(|| dir.clone())
    }

    pub(super) fn pump_shell_props(&mut self, now: Instant) {
        let kinds = self.column_kinds();
        let shell = kinds.contains(&Column::ImageSize);
        let portable = kinds.contains(&Column::Rating) && self.metadata_enabled() && PORTABLE_COLUMNS.iter().any(|&c| self.tab().columns.shown(c));
        let visible_system = shell && SHELL_COLUMNS.iter().any(|&c| self.tab().columns.shown(c));
        let system = visible_system || (shell && self.tab().view_spec.needs_props());
        let tab = self.tab();
        let show_type = tab.look == super::LayoutMode::Cards || (tab.look == super::LayoutMode::Details && tab.columns.shown(Column::Type));
        let probe = !self.caps.unix_paths && show_type && tab.view_loc.path().kind() == PathKind::Drive && !self.network_place(&tab.view_loc);
        if !system && !portable && !probe {
            return;
        }
        let Some(dir) = self.props_dir() else { return };
        let (gen, first, last) = self.visible_rows();
        let k = self.tabs.cur;
        if self.tabs.items[k].props.gen != gen || self.tabs.items[k].props.portable != portable || self.tabs.items[k].props.system != system || self.tabs.items[k].props.probe != probe {
            let next_ask = self.tabs.items[k].props.next_ask;
            self.tabs.items[k].props = ShellProps { gen, portable, system, probe, next_ask, ..Default::default() };
        }
        self.apply_shell_props(k, now);
        let t = &mut self.tabs.items[k];
        let (tab, spec) = (t.id, t.view_spec);
        let Some(v) = t.view.as_ref().filter(|v| v.snap().gen == gen) else { return };
        let l = v.listing();
        let candidate = |i: u32| {
            let e = &l.entries[i as usize];
            probe && e.size >= 52 && e.attrs & (attr::DIR | attr::LINK | attr::NOT_LOCAL | attr::EXEC | attr::STREAM | attr::UNREADABLE) == 0 && crate::name::executable_suffix(l.ext(i))
        };
        if visible_system || portable || probe {
            let rows = v.rows();
            let shown = rows.get(first as usize..(last as usize + 1).min(rows.len())).unwrap_or_default();
            let names: Vec<_> = shown.iter().copied().filter(|&i| readable(l.entries[i as usize].attrs) && (visible_system || portable || candidate(i)) && !t.props.asked.contains_key(l.name(i))).take(MAX_ASK).map(|i| (l.name(i).to_vec(), candidate(i))).collect();
            if !names.is_empty() {
                let ask = t.props.ask(&names);
                self.effects.push(Effect::ShellProps { tab, gen, ask, dir: dir.clone(), names, shell: system, portable });
            }
        }
        if !shell || !spec.needs_props() || !v.snap().done || t.props.bulk.len() >= BULK_IN_FLIGHT {
            return;
        }
        let (mut names, mut i) = (Vec::new(), t.props.cursor);
        while (i as usize) < l.len() && names.len() < BULK {
            if readable(l.entries[i as usize].attrs) && sort::visible(l, i, &spec) && !t.props.asked.contains_key(l.name(i)) {
                names.push((l.name(i).to_vec(), candidate(i)));
            }
            i += 1;
        }
        t.props.cursor = i;
        if !names.is_empty() {
            let ask = t.props.ask(&names);
            t.props.bulk.push(ask);
            self.effects.push(Effect::ShellProps { tab, gen, ask, dir, names, shell: true, portable });
        }
    }

    fn apply_shell_props(&mut self, k: usize, now: Instant) {
        let t = &mut self.tabs.items[k];
        let gen = t.props.gen;
        if t.props.pending.is_empty() {
            return;
        }
        let Some(v) = t.view.as_mut().filter(|v| v.snap().done && v.snap().gen == gen) else { return };
        if v.snap().view.needs_props() {
            if let Some(due) = t.props.applied.map(|at| at + RESORT_EVERY).filter(|&due| now < due) {
                return self.arm_timer(now, due);
            }
            t.props.applied = Some(now);
        }
        let keys = t.props.pending.drain(..).filter_map(|(n, key)| v.find(&n).map(|i| (i, key))).collect();
        v.set_props(keys);
        self.dirty = true;
    }

    pub(super) fn shell_props_due(&self, now: Instant) -> bool {
        let p = &self.tab().props;
        !p.pending.is_empty() && p.applied.is_none_or(|at| now >= at + RESORT_EVERY)
    }

    pub fn on_shell_props(&mut self, tab: TabId, gen: u64, ask: u64, rows: Vec<(Vec<u8>, ColumnValues)>) {
        if let Some(id) = self.parked_of(tab) {
            return self.visit(id, |a| a.on_shell_props(tab, gen, ask, rows));
        }
        let case = self.case;
        let Some(t) = self.tabs.items.iter_mut().find(|t| t.id == tab) else { return };
        if t.props.gen != gen {
            return;
        }
        t.props.bulk.retain(|&b| b != ask);
        for (name, values) in rows {
            if t.props.asked.get(&name) != Some(&ask) {
                continue;
            }
            if t.props.system {
                t.props.pending.push((name.clone(), PropKey::new(&values, case)));
            }
            if values.elf || !values.text.is_empty() {
                t.props.values.insert(name, values);
            }
        }
        self.dirty = true;
    }

    pub(super) fn shell_prop_text(&self, s: &mut impl std::fmt::Write, name: &[u8], col: Column) {
        let Some(k) = SHELL_COLUMNS.iter().chain(&PORTABLE_COLUMNS).position(|&c| c == col) else { return };
        if let Some(text) = self.tab().props.values.get(name).and_then(|v| v.text.get(k)) {
            let _ = s.write_str(text);
        }
    }
}

#[cfg(test)]
#[path = "../../test/ui/shell_props.rs"]
mod tests;
