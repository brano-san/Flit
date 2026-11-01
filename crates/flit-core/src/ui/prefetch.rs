use super::{App, Drag, Effect, Hit, MenuKind, ShellMenu, TabId};

const MAX_ITEMS: usize = 256;
const WARM_SCAN: usize = 4096;

#[derive(Default)]
pub(super) struct Prefetch {
    key: Option<Key>,
    pub sent: Option<ShellMenu>,
    warmed: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Over {
    Item(u32),
    Empty,
    Keyboard,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Key {
    tab: TabId,
    gen: u64,
    sel: u64,
    focus: Option<u32>,
    over: Over,
}

impl App {
    pub(super) fn shell_target(&self, items: &[u32], on_items: bool, extended: bool) -> Result<ShellMenu, &'static str> {
        let Some(dir) = self.tab().view_loc.to_windows() else { return Err(crate::i18n::S::UiOpsFolderNotShell.t()) };
        let items = self.item_paths(items)?;
        let kind = if on_items { self.selection_kind() } else { MenuKind::Background };
        Ok(ShellMenu { dir, items, x: 0.0, y: 0.0, extended, menu: Vec::new(), inline: self.shell_items_inline || extended, shell: true, kind })
    }

    pub(super) fn push_shell_menu(&mut self, menu: ShellMenu) {
        self.remember_shape_key(&menu);
        self.prefetch.key = None;
        self.prefetch.sent = None;
        self.effects.push(Effect::ShellMenu(menu));
    }

    pub(super) fn prefetch_shell(&mut self) {
        if self.own_menu.is_some() {
            return;
        }
        self.warm_shell();
        let key = self.prefetch_key();
        if key == self.prefetch.key {
            return;
        }
        self.prefetch.key = key;
        let target = key.and_then(|k| self.prefetch_target(k.over));
        if target != self.prefetch.sent {
            self.prefetch.sent.clone_from(&target);
            self.effects.push(Effect::ShellPrefetch(target));
        }
    }

    fn prefetch_key(&self) -> Option<Key> {
        let t = self.tab();
        let busy = self.drag != Drag::None || self.marquee.is_some() || self.pressed != Hit::None || (t.scroll - t.scroll_target).abs() > 0.5 || self.search.active || !self.active;
        if busy {
            return None;
        }
        let v = t.view.as_ref()?;
        let over = match self.hover {
            Hit::Row(r) => Over::Item(*v.rows().get(r)?),
            Hit::ListEmpty | Hit::Group(_) => Over::Empty,
            _ => Over::Keyboard,
        };
        Some(Key { tab: t.id, gen: v.snap().gen, sel: v.selection_rev(), focus: v.focus, over })
    }

    fn prefetch_target(&self, over: Over) -> Option<ShellMenu> {
        if self.in_bin() || self.archive_view().is_some() {
            return None;
        }
        let v = self.tab().view.as_ref()?;
        let selection = || v.selected().take(MAX_ITEMS + 1).collect::<Vec<u32>>();
        let items = match over {
            Over::Item(i) if v.is_selected(i) => selection(),
            Over::Item(i) => vec![i],
            Over::Empty => Vec::new(),
            Over::Keyboard if v.focus.is_some_and(|f| v.is_selected(f)) => selection(),
            Over::Keyboard => Vec::new(),
        };
        if items.len() > MAX_ITEMS {
            return None;
        }
        let on_items = !items.is_empty();
        self.shell_target(&items, on_items, false).ok()
    }

    fn warm_shell(&mut self) {
        if self.prefetch.warmed || self.tab().view.is_none() {
            return;
        }
        self.prefetch.warmed = true;
        if self.in_bin() || self.archive_view().is_some() {
            return;
        }
        let Some(v) = self.tab().view.as_ref() else { return };
        let l = v.listing();
        let rows = &v.rows()[..v.rows().len().min(WARM_SCAN)];
        let folder = rows.iter().copied().find(|&i| l.entries[i as usize].is_dir());
        let files = || rows.iter().copied().filter(|&i| !l.entries[i as usize].is_dir());
        let file = files().find(|&i| l.name(i).to_ascii_lowercase().ends_with(b".txt")).or_else(|| files().next());
        let mut menus: Vec<ShellMenu> = [folder, file].into_iter().flatten().filter_map(|i| self.shell_target(&[i], true, false).ok()).collect();
        menus.extend(self.shell_target(&[], false, false).ok());
        if !menus.is_empty() {
            self.effects.push(Effect::ShellWarm(menus));
        }
    }
}

impl ShellMenu {
    pub fn same_target(&self, other: &ShellMenu) -> bool {
        self.dir == other.dir && self.items == other.items && self.extended == other.extended && self.inline == other.inline
    }
}

#[cfg(test)]
#[path = "../../test/ui/prefetch.rs"]
mod tests;
