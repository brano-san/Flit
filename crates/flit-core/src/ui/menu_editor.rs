use super::context::{MenuKind, PATH_SEP};
use super::settings_page::SHit;
use super::widgets::{faded, Paint};
use super::{centered, App, Hit};
use crate::render::Rect;
use crate::text::{FontStyle, Gfx, TextBackend};
use crate::theme::Theme;
use crate::i18n::S;

pub const MAX_SEEN: usize = 200;
pub const KINDS: [MenuKind; 3] = [MenuKind::File, MenuKind::Folder, MenuKind::Background];
const PAD: f32 = 16.0;
const GRIP_W: f32 = 18.0;
const INDENT: f32 = 22.0;
const BOX: f32 = 24.0;
const G_GRIP: &str = "\u{E700}";
const G_CLOSED: &str = "\u{E76C}";
const G_OPENED: &str = "\u{E70D}";
const G_PIN: &str = "\u{E718}";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MenuPrefs {
    pub kind: usize,
    pub hidden: [Vec<String>; 3],
    pub order: [Vec<String>; 3],
    pub seen: [Vec<String>; 3],
    pub unfolded: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct MenuRow {
    pub path: String,
    pub depth: usize,
    pub submenu: bool,
    pub open: bool,
    pub hidden: bool,
    pub pinned: bool,
}

impl MenuRow {
    pub fn label(&self) -> &str {
        self.path.rsplit(PATH_SEP).next().unwrap_or(&self.path)
    }
}

fn parent(path: &str) -> Option<&str> {
    path.rsplit_once(PATH_SEP).map(|(p, _)| p)
}

pub fn remember(seen: &mut Vec<String>, latest: Vec<String>) {
    let old = std::mem::replace(seen, latest.clone());
    let mut after: Option<String> = None;
    for p in old {
        if latest.contains(&p) {
            after = Some(p);
            continue;
        }
        if parent(&p).is_some_and(|up| latest.iter().any(|l| l == up)) {
            continue;
        }
        let at = after.as_ref().and_then(|a| seen.iter().position(|s| s == a)).map_or(seen.len(), |k| k + 1);
        seen.insert(at, p.clone());
        after = Some(p);
    }
    seen.truncate(MAX_SEEN);
}

pub(super) fn check_rect(r: Rect, depth: usize, s: impl Fn(f32) -> f32) -> Rect {
    let x = r.x + s(PAD) + s(GRIP_W) + depth as f32 * s(INDENT);
    Rect::new(x, r.y + ((r.h - s(BOX)) / 2.0).round(), s(BOX), s(BOX))
}

pub(super) fn fold_rect(r: Rect, depth: usize, s: impl Fn(f32) -> f32) -> Rect {
    let c = check_rect(r, depth, &s);
    Rect::new(c.right(), c.y, s(BOX), c.h)
}

pub(super) fn star_rect(r: Rect, s: impl Fn(f32) -> f32) -> Rect {
    Rect::new(r.right() - s(PAD) - s(28.0), r.y + ((r.h - s(BOX)) / 2.0).round(), s(28.0), s(BOX))
}

pub(super) fn row_hit(row: &MenuRow, r: Rect, k: usize, (x, y): (f32, f32), s: impl Fn(f32) -> f32) -> SHit {
    if check_rect(r, row.depth, &s).contains(x, y) {
        SHit::Check(k)
    } else if row.submenu && fold_rect(r, row.depth, &s).contains(x, y) {
        SHit::Fold(k)
    } else if !row.submenu && star_rect(r, &s).contains(x, y) {
        SHit::Star(k)
    } else {
        SHit::Line(k)
    }
}

impl App {
    fn builtin_menu_paths(&self, kind: usize) -> Vec<String> {
        let mut paths = Vec::new();
        let mut add = |id: &str, label: S, parent: Option<S>| {
            if self.supports(id) {
                if let Some(parent) = parent {
                    let parent = parent.t().to_owned();
                    if !paths.contains(&parent) { paths.push(parent.clone()); }
                    paths.push(format!("{parent}{PATH_SEP}{}", label.t()));
                } else { paths.push(label.t().to_owned()); }
            }
        };
        if kind == 2 {
            for (id, label) in [("nav.refresh", S::MenuRefresh), ("file.paste", S::MenuPaste), ("app.terminal", S::MenuOpenInTerminal), ("folder.file_manager", S::ActionsFolderFileManager), ("folder.bookmark", S::MenuPinToSidebar)] { add(id, label, None); }
            add("file.new_folder", S::MenuFolder, Some(S::MenuNew));
            add("file.shortcut", S::MenuShortcut, Some(S::MenuNew));
        } else {
            add("item.open", S::MenuOpen, None);
            if kind == 1 {
                add("item.open_tab", S::MenuOpenInNewTab, Some(S::MenuOpenIn));
                add("item.open_window", S::MenuOpenInNewWindow, Some(S::MenuOpenIn));
            }
            for (id, label) in [("file.cut", S::MenuCut), ("file.copy", S::MenuCopy), ("file.shortcut", S::MenuCreateShortcut), ("file.rename", S::MenuRename), ("file.batch_rename", S::MenuBatchRename), ("item.trash", S::MenuDelete), ("file.new_folder_with_selection", S::MenuNewFolderWithSelection)] { add(id, label, None); }
            if kind == 1 {
                for (id, label) in [("file.paste_into", S::MenuPaste), ("item.pin", S::MenuPinToSidebar), ("app.terminal", S::MenuOpenInTerminal), ("file.folder_icon", S::MenuChangeIcon), ("file.folder_icon_reset", S::MenuRestoreDefaultIcon), ("file.flatten", S::MenuFlattenFolder)] { add(id, label, None); }
            } else {
                add("file.hashes", S::MenuHashes, None);
                add("file.run_in_terminal", S::MenuRunInTerminal, None);
                for (id, label) in [("archive.extract", S::MenuExtractFiles), ("archive.extract_here", S::MenuExtractHere), ("archive.smart", S::MenuSmartExtract), ("archive.open", S::MenuOpenAsFolder)] { add(id, label, Some(S::MenuExtract)); }
            }
            add("archive.compress", S::MenuAddToArchive, Some(S::MenuCompress));
            add("tags.new", S::MenuNewTag, Some(S::MenuEditTags));
            add("tags.remove", S::MenuRemoveTags, Some(S::MenuEditTags));
        }
        add("item.copy_path", if self.caps.unix_paths { S::MenuCopyPath } else { S::MenuCopyAsPath }, None);
        add("item.properties", S::MenuProperties, None);
        paths
    }

    fn menu_index(&self) -> usize {
        self.settings.opts.menu.kind.min(KINDS.len() - 1)
    }

    pub(super) fn menu_rows(&self) -> Vec<MenuRow> {
        let k = self.menu_index();
        let m = &self.settings.opts.menu;
        let builtins = self.builtin_menu_paths(k);
        let mut all: Vec<&str> = Vec::new();
        let mut known = std::collections::HashSet::new();
        for p in m.seen[k].iter().chain(&m.hidden[k]).chain(&self.settings.opts.pins[k]).chain(&builtins) {
            if known.insert(p.as_str()) {
                all.push(p);
            }
        }
        let mut tops = Vec::new();
        let mut anchor = -1;
        for p in all.iter().copied().filter(|p| parent(p).is_none_or(|up| !known.contains(up))) {
            if let Some(at) = m.order[k].iter().position(|o| o == p) {
                anchor = at as isize;
            }
            tops.push((anchor, p));
        }
        tops.sort_by_key(|&(anchor, _)| anchor);
        let mut out = Vec::new();
        for (_, p) in tops {
            self.menu_row(p, 0, &all, &mut out);
        }
        out
    }

    fn menu_row(&self, path: &str, depth: usize, all: &[&str], out: &mut Vec<MenuRow>) {
        let k = self.menu_index();
        let m = &self.settings.opts.menu;
        let children: Vec<&str> = all.iter().copied().filter(|c| parent(c) == Some(path)).collect();
        let open = !children.is_empty() && m.unfolded.iter().any(|u| u == path);
        let hidden = m.hidden[k].iter().any(|h| h == path);
        let pinned = self.settings.opts.pins[k].iter().any(|p| p == path);
        out.push(MenuRow { path: path.to_owned(), depth, submenu: !children.is_empty(), open, hidden, pinned });
        if open {
            for c in children {
                self.menu_row(c, depth + 1, all, out);
            }
        }
    }

    pub(super) fn menu_line_text(row: &MenuRow) -> String {
        let fold = match (row.submenu, row.open) {
            (false, _) => "",
            (true, false) => " >",
            (true, true) => " v",
        };
        format!("menu {}[{}] {}{fold}{}", "  ".repeat(row.depth), if row.hidden { ' ' } else { 'x' }, row.label(), if row.pinned { " *" } else { "" })
    }

    pub(super) fn toggle_menu_item(&mut self, path: &str) {
        let k = self.menu_index();
        let hidden = &mut self.settings.opts.menu.hidden[k];
        match hidden.iter().position(|h| h == path) {
            Some(at) => {
                hidden.remove(at);
            }
            None => hidden.push(path.to_owned()),
        }
        self.layout();
        self.dirty = true;
    }

    pub(super) fn pin_menu_item(&mut self, path: &str) {
        let k = self.menu_index();
        let pinned = self.settings.opts.pins[k].iter().any(|p| p == path);
        self.set_pin(KINDS[k], path.to_owned(), !pinned);
        self.layout();
    }

    pub(super) fn fold_menu_item(&mut self, path: &str, open: Option<bool>) {
        let unfolded = &mut self.settings.opts.menu.unfolded;
        let was = unfolded.iter().any(|u| u == path);
        let now = open.unwrap_or(!was);
        unfolded.retain(|u| u != path);
        if now {
            unfolded.push(path.to_owned());
        }
        self.layout();
        self.dirty = true;
    }

    fn menu_tops(&self) -> Vec<String> {
        self.menu_rows().into_iter().filter(|r| r.depth == 0).map(|r| r.path).collect()
    }

    pub(super) fn move_menu_item(&mut self, path: &str, to: usize) {
        let mut tops = self.menu_tops();
        let Some(from) = tops.iter().position(|p| p == path) else { return };
        let to = to.min(tops.len() - 1);
        if to == from {
            return;
        }
        let p = tops.remove(from);
        tops.insert(to, p);
        let k = self.menu_index();
        self.settings.opts.menu.order[k] = tops;
        self.layout();
        self.dirty = true;
    }

    pub(super) fn shift_menu_item(&mut self, path: &str, down: bool) {
        let Some(at) = self.menu_tops().iter().position(|p| p == path) else { return };
        match (down, at) {
            (false, 0) => {}
            (false, at) => self.move_menu_item(path, at - 1),
            (true, at) => self.move_menu_item(path, at + 1),
        }
    }

    pub(super) fn reset_menu(&mut self) {
        let k = self.menu_index();
        self.settings.opts.menu.hidden[k].clear();
        self.settings.opts.menu.order[k].clear();
        self.layout();
        self.dirty = true;
    }

    pub(super) fn remember_menu(&mut self, kind: MenuKind, latest: Vec<String>) {
        if let Some(k) = kind.pins() {
            remember(&mut self.settings.opts.menu.seen[k], latest);
        }
    }

    pub(super) fn draw_menu_row<B: TextBackend>(&self, g: &mut Gfx<B>, t: &Theme, r: Rect, k: usize, row: &MenuRow, (font, icons): (FontStyle, FontStyle)) {
        let sc = self.scale;
        let s = |v: f32| (v * sc).round();
        if row.depth == 0 {
            centered(g, Rect::new(r.x + s(PAD) - s(4.0), r.y, s(GRIP_W), r.h), G_GRIP, icons, t.text_dim.alpha(0x90));
        }
        let c = check_rect(r, row.depth, s);
        let lit = self.lit(Hit::Settings(SHit::Check(k)));
        if lit > 0.0 {
            g.dl.round_rect(c, faded(t.button_hover, lit), s(4.0));
        }
        let inner = Rect::new(c.x + (c.w - s(16.0)) / 2.0, c.y + (c.h - s(16.0)) / 2.0, s(16.0), s(16.0)).snap();
        Paint::new(g, t, sc).checkbox(inner, !row.hidden);
        let f = fold_rect(r, row.depth, s);
        if row.submenu {
            let lit = self.lit(Hit::Settings(SHit::Fold(k)));
            if lit > 0.0 {
                g.dl.round_rect(f, faded(t.button_hover, lit), s(4.0));
            }
            centered(g, f, if row.open { G_OPENED } else { G_CLOSED }, icons, t.text_dim);
        }
        let star = star_rect(r, s);
        let tx = f.right() + s(4.0);
        let ink = if row.hidden { t.text_dim.alpha(0xA0) } else { t.text };
        let y = super::widgets::baseline(g, r, font);
        g.text(tx, y, row.label(), font, ink, (star.x - s(8.0) - tx).max(0.0));
        if !row.submenu {
            let lit = self.lit(Hit::Settings(SHit::Star(k)));
            if lit > 0.0 {
                g.dl.round_rect(star, faded(t.button_hover, lit), s(4.0));
            }
            centered(g, star, G_PIN, icons, if row.pinned { t.accent } else { t.text_dim.alpha(0x70) });
        }
    }
}

#[cfg(test)]
#[path = "../../test/ui/menu_editor.rs"]
mod tests;
