use super::menu::TypeCmd;
use super::details::CHECK;
use super::{App, Column};
use crate::render::Rect;
use crate::view::type_key;
use bumpalo::Bump;
use crate::hash::FxHashMap;

impl App {
    pub(super) fn folder_types(&self) -> Vec<(u64, String, usize)> {
        let Some(v) = &self.tab().view else { return Vec::new() };
        let (l, arena) = (v.listing(), Bump::new());
        let mut seen: FxHashMap<u64, usize> = FxHashMap::default();
        let mut out: Vec<(u64, String, usize)> = Vec::new();
        for &i in &v.snap().order {
            let key = type_key(l, i);
            match seen.get(&key) {
                Some(&k) => out[k].2 += 1,
                None => {
                    seen.insert(key, out.len());
                    out.push((key, self.type_text(&arena, l, i).to_owned(), 1));
                }
            }
        }
        out.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.1.cmp(&b.1)));
        out
    }

    pub(super) fn funnel_rect(&self, header: Rect) -> Rect {
        let w = self.s(22.0);
        Rect::new(header.right() - w - self.s(4.0), header.y, w, header.h)
    }

    pub(super) fn header_check(&self, header: Rect) -> Rect {
        let d = self.s(CHECK);
        Rect::new(header.x + self.s(10.0), header.y + ((header.h - d) / 2.0).round(), d, d)
    }

    pub(super) fn toggle_select_all(&mut self) {
        let all = self.tab().view.as_ref().is_some_and(|v| !v.rows().is_empty() && v.selected_count() == v.rows().len());
        self.run_action(if all { "select.none" } else { "select.all" }, std::time::Instant::now());
    }

    pub(super) fn open_types(&mut self) {
        if self.tab().view.is_none() {
            return;
        }
        let types = self.folder_types();
        let l = &self.layout;
        let at = l.columns.iter().position(|&c| c == Column::Type).map_or((l.list.x, l.list.y), |k| (l.cols[k].x, l.cols[k].bottom()));
        let menu = self.build_types_menu(&types);
        self.flit_menu(at.0, at.1, menu);
    }

    pub(super) fn types_command(&mut self, c: TypeCmd) {
        let all: Vec<u64> = match c {
            TypeCmd::All(false) => self.folder_types().into_iter().map(|t| t.0).collect(),
            _ => Vec::new(),
        };
        let t = self.tab_mut();
        match c {
            TypeCmd::Toggle(key) => {
                if !t.hidden_types.remove(&key) {
                    t.hidden_types.insert(key);
                }
            }
            TypeCmd::All(true) => t.hidden_types.clear(),
            TypeCmd::All(false) => t.hidden_types.extend(all),
        }
        t.apply_shape();
        (t.scroll, t.scroll_target) = (0.0, 0.0);
        self.dirty = true;
    }
}

#[cfg(test)]
#[path = "../../test/ui/types.rs"]
mod tests;
