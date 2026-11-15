use super::{App, Column, Effect, SHELL_COLUMNS};
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
    values: FxHashMap<Vec<u8>, Vec<String>>,
    asked: FxHashMap<Vec<u8>, u64>,
    next_ask: u64,
    bulk: Vec<u64>,
    cursor: u32,
    pending: Vec<(Vec<u8>, Option<PropKey>)>,
    applied: Option<Instant>,
}

impl ShellProps {
    pub(super) fn is_idle(&self) -> bool {
        self.asked.is_empty() && self.values.is_empty() && self.pending.is_empty()
    }

    pub(super) fn forget(&mut self, name: &[u8], at: Option<u32>) {
        self.values.remove(name);
        self.asked.remove(name);
        self.pending.retain(|(n, _)| n != name);
        if let Some(i) = at {
            self.cursor = self.cursor.min(i);
        }
    }

    fn ask(&mut self, names: &[Vec<u8>]) -> u64 {
        self.next_ask += 1;
        for n in names {
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
        (!self.caps.unix_paths && local && crate::archive::split(dir).is_none() && !t.view_loc.is_recycle_bin()).then(|| dir.clone())
    }

    pub(super) fn pump_shell_props(&mut self, now: Instant) {
        if !self.column_kinds().contains(&Column::ImageSize) {
            return;
        }
        let Some(dir) = self.props_dir() else { return };
        let (gen, first, last) = self.visible_rows();
        let k = self.cur;
        if self.tabs[k].props.gen != gen {
            self.tabs[k].props = ShellProps { gen, ..Default::default() };
        }
        self.apply_shell_props(k, now);
        let t = &mut self.tabs[k];
        let (tab, spec) = (t.id, t.view_spec);
        let Some(v) = &t.view else { return };
        let l = v.listing();
        if SHELL_COLUMNS.iter().any(|&c| t.columns.shown(c)) {
            let rows = v.rows();
            let shown = rows.get(first as usize..(last as usize + 1).min(rows.len())).unwrap_or_default();
            let names: Vec<Vec<u8>> = shown.iter().filter(|&&i| readable(l.entries[i as usize].attrs)).map(|&i| l.name(i)).filter(|n| !t.props.asked.contains_key(*n)).take(MAX_ASK).map(<[u8]>::to_vec).collect();
            if !names.is_empty() {
                let ask = t.props.ask(&names);
                self.effects.push(Effect::ShellProps { tab, gen, ask, dir: dir.clone(), names });
            }
        }
        if !spec.needs_props() || !v.snap().done || t.props.bulk.len() >= BULK_IN_FLIGHT {
            return;
        }
        let (mut names, mut i) = (Vec::new(), t.props.cursor);
        while (i as usize) < l.len() && names.len() < BULK {
            if readable(l.entries[i as usize].attrs) && sort::visible(l, i, &spec) && !t.props.asked.contains_key(l.name(i)) {
                names.push(l.name(i).to_vec());
            }
            i += 1;
        }
        t.props.cursor = i;
        if !names.is_empty() {
            let ask = t.props.ask(&names);
            t.props.bulk.push(ask);
            self.effects.push(Effect::ShellProps { tab, gen, ask, dir, names });
        }
    }

    fn apply_shell_props(&mut self, k: usize, now: Instant) {
        let t = &mut self.tabs[k];
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
        let Some(t) = self.tabs.iter_mut().find(|t| t.id == tab) else { return };
        if t.props.gen != gen {
            return;
        }
        t.props.bulk.retain(|&b| b != ask);
        for (name, values) in rows {
            if t.props.asked.get(&name) != Some(&ask) {
                continue;
            }
            let key = PropKey::new(&values, case);
            t.props.values.insert(name.clone(), values.text);
            t.props.pending.push((name, key));
        }
        self.dirty = true;
    }

    pub(super) fn shell_prop_text(&self, s: &mut impl std::fmt::Write, name: &[u8], col: Column) {
        let Some(k) = SHELL_COLUMNS.iter().position(|&c| c == col) else { return };
        if let Some(text) = self.tab().props.values.get(name).and_then(|v| v.get(k)) {
            let _ = s.write_str(text);
        }
    }
}

#[cfg(test)]
#[path = "../../test/ui/shell_props.rs"]
mod tests;
