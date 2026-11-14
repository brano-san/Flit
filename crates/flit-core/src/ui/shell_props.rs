use super::{App, Column, Effect, SHELL_COLUMNS};
use crate::hash::{FxHashMap, FxHashSet};
use crate::location::Location;
use crate::path::{FsPath, PathKind};
use crate::provider::TabId;

const MAX_ASK: usize = 256;

#[derive(Default)]
pub(super) struct ShellProps {
    gen: u64,
    values: FxHashMap<Vec<u8>, Vec<String>>,
    asked: FxHashSet<Vec<u8>>,
}

impl App {
    fn props_dir(&self) -> Option<FsPath> {
        let t = self.tab();
        let Location::Local(dir) = &t.view_loc else { return None };
        let local = matches!(dir.kind(), PathKind::Drive | PathKind::Unc);
        (!self.caps.unix_paths && local && crate::archive::split(dir).is_none() && !t.view_loc.is_recycle_bin()).then(|| dir.clone())
    }

    pub(super) fn request_shell_props(&mut self) {
        let t = self.tab();
        if !SHELL_COLUMNS.iter().any(|&c| t.columns.shown(c)) || !self.column_kinds().contains(&Column::ImageSize) {
            return;
        }
        let Some(dir) = self.props_dir() else { return };
        let (gen, first, last) = self.visible_rows();
        let t = &mut self.tabs[self.cur];
        if t.props.gen != gen {
            t.props = ShellProps { gen, ..Default::default() };
        }
        let Some(v) = &t.view else { return };
        let (l, rows) = (v.listing(), v.rows());
        let shown = rows.get(first as usize..(last as usize + 1).min(rows.len())).unwrap_or_default();
        let names: Vec<Vec<u8>> = shown.iter().filter(|&&i| !l.entries[i as usize].is_dir()).map(|&i| l.name(i).to_vec()).filter(|n| !t.props.asked.contains(n)).take(MAX_ASK).collect();
        if names.is_empty() {
            return;
        }
        t.props.asked.extend(names.iter().cloned());
        let tab = t.id;
        self.effects.push(Effect::ShellProps { tab, gen, dir, names });
    }

    pub fn on_shell_props(&mut self, tab: TabId, gen: u64, rows: Vec<(Vec<u8>, Vec<String>)>) {
        let Some(t) = self.tabs.iter_mut().find(|t| t.id == tab) else { return };
        if t.props.gen != gen {
            return;
        }
        t.props.values.extend(rows);
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
