use super::actions::ACTIONS;
use super::describe::description;
use super::{App, Column, Focus, Hit, LayoutMode};
use super::focus::ORDER;
use crate::a11y::{Role, Tree};
use crate::hash;
use crate::i18n::S;
use crate::render::Rect;
use crate::view::Line;
use bumpalo::Bump;
use std::time::Instant;

const TABS: u64 = 2;
const NAV: u64 = 3;
const ADDRESS: u64 = 4;
const COMMANDS: u64 = 5;
const SIDEBAR: u64 = 6;
const FILES: u64 = 7;
const HEADER: u64 = 8;
const PREVIEW: u64 = 9;
const DIALOG: u64 = 10;
const LEAF: u64 = 1 << 63;

const NAV_IDS: [&str; 7] = ["nav.back", "nav.forward", "nav.up", "", "folder.cycle_view", "folder.bookmark", ""];

pub(super) fn leaf(h: Hit) -> u64 {
    hash::hash(&format!("{h:?}")) | LEAF
}

fn action_title(id: &str) -> &'static str {
    ACTIONS.iter().find(|a| a.id == id).map_or("", |a| a.title.t())
}

pub(super) fn union(rects: impl IntoIterator<Item = Rect>) -> Rect {
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for r in rects.into_iter().filter(|r| r.w > 0.0) {
        (x0, y0, x1, y1) = (x0.min(r.x), y0.min(r.y), x1.max(r.right()), y1.max(r.bottom()));
    }
    if x0 > x1 { Rect::default() } else { Rect::new(x0, y0, x1 - x0, y1 - y0) }
}

pub(super) fn put(t: &mut Tree, targets: &mut Vec<(u64, Hit)>, parent: usize, hit: Hit, role: Role, name: &str, r: Rect) -> usize {
    let id = leaf(hit);
    targets.push((id, hit));
    t.add(parent, id, role, name, r)
}

impl App {
    pub fn accessibility(&mut self) -> Tree {
        let mut targets: Vec<(u64, Hit)> = Vec::new();
        let mut focus: Option<usize> = None;
        let (w, h) = self.size;
        let mut t = Tree::new(self.tab().title(), Rect::new(0.0, 0.0, w, h));
        let l = self.layout.clone();
        let kb = matches!(self.focus, Focus::TabBar | Focus::Nav | Focus::Toolbar).then_some(self.kb);

        let tabs = t.add(0, TABS, Role::TabList, S::A11yRegionTabs.t(), l.title);
        for (i, &(r, _)) in l.tabs.iter().enumerate() {
            let k = put(&mut t, &mut targets, tabs, Hit::Tab(i), Role::Tab, &self.tabs[i].title(), r);
            t.nodes[k].selected = i == self.cur;
            t.nodes[k].help = self.tabs[i].location().display().into_owned();
            if self.focus == Focus::TabBar && kb == Some(i) {
                focus = Some(k);
            }
        }
        let k = put(&mut t, &mut targets, tabs, Hit::NewTab, Role::Button, S::A11yButtonNewTab.t(), l.new_tab);
        t.nodes[k].help = description("tab.new").into();
        if self.focus == Focus::TabBar && kb == Some(l.tabs.len()) {
            focus = Some(k);
        }
        if l.actions.w > 0.0 {
            put(&mut t, &mut targets, tabs, Hit::TabActions, Role::Button, S::A11yButtonTabActions.t(), l.actions);
        }
        let max = if self.maximized { S::A11yButtonRestore } else { S::A11yButtonMaximize };
        for (i, name) in [S::A11yButtonMinimize, max, S::A11yButtonClose].into_iter().enumerate() {
            put(&mut t, &mut targets, tabs, Hit::Caption(i), Role::Button, name.t(), self.panels.caption[i]);
        }
        put(&mut t, &mut targets, tabs, Hit::Menu, Role::Button, S::A11yButtonMainMenu.t(), self.panels.menu);
        put(&mut t, &mut targets, tabs, Hit::QuickToggle, Role::Button, S::A11yButtonQuickAccess.t(), self.panels.quick);

        let nav = t.add(0, NAV, Role::ToolBar, S::A11yRegionNavigation.t(), union(l.nav.iter().copied().chain([l.view, l.bookmark, l.more])));
        let tab = self.tab();
        let enabled = [tab.history.can_back(), tab.history.can_forward(), tab.location().parent().is_some(), true, true, true, true];
        let hits = [Hit::Nav(0), Hit::Nav(1), Hit::Nav(2), Hit::Nav(3), Hit::ViewMode, Hit::Bookmark, Hit::PanelMenu];
        let rects = [l.nav[0], l.nav[1], l.nav[2], l.nav[3], l.view, l.bookmark, l.more];
        for i in 0..hits.len() {
            let (name, help) = match NAV_IDS[i] {
                "" => (if i == 3 { S::TooltipVisited.t() } else { S::TooltipPanelMenu.t() }, String::new()),
                id => (action_title(id), description(id).into()),
            };
            let k = put(&mut t, &mut targets, nav, hits[i], Role::Button, name, rects[i]);
            t.nodes[k].help = help;
            t.nodes[k].enabled = enabled[i];
            if self.focus == Focus::Nav && kb == Some(i) {
                focus = Some(k);
            }
        }

        if l.address.w > 0.0 && !self.search.active {
            let loc = self.tab().location().display().into_owned();
                let a = t.add(0, ADDRESS, Role::ToolBar, S::A11yRegionAddress.t(), l.address);
                t.nodes[a].value = loc;
                t.nodes[a].help = description("nav.address").into();
                let tab = self.tab();
                for (i, r) in l.crumbs.iter().enumerate().filter(|(_, r)| r.w > 0.0) {
                    if let Some((label, target)) = tab.crumb(i) {
                        let k = put(&mut t, &mut targets, a, Hit::Crumb(i), Role::Button, label, *r);
                        t.nodes[k].help = match i < tab.crumbs.len() {
                            true => target.display().into_owned(),
                            false => format!("{}: {}", S::AddressVisitedDeeper.t(), target.display()),
                        };
                    }
                }
        }

        if l.commands.h > 0.0 {
            let bar = t.add(0, COMMANDS, Role::ToolBar, S::A11yRegionCommands.t(), l.commands);
            for (n, (hit, r, label, id, on)) in self.tool_buttons().into_iter().enumerate() {
                let k = put(&mut t, &mut targets, bar, Hit::Tool(hit), Role::Button, label, r);
                t.nodes[k].help = description(id).into();
                t.nodes[k].enabled = on;
                if self.focus == Focus::Toolbar && kb == Some(n) {
                    focus = Some(k);
                }
            }
        }

        let sb = &self.nav.sidebar;
        if sb.rect.w > 0.0 {
            let side = t.add(0, SIDEBAR, Role::Tree, S::A11yRegionSidebar.t(), sb.rect);
            let here = self.tab().location().clone();
            for (i, (it, &r)) in sb.items.iter().zip(&sb.rects).enumerate() {
                let k = put(&mut t, &mut targets, side, Hit::Side(i), Role::TreeItem, &it.label, r);
                t.nodes[k].selected = it.loc == here;
                t.nodes[k].help = it.loc.display().into_owned();
                if self.focus == Focus::Sidebar && sb.cursor == i {
                    focus = Some(k);
                }
            }
        }

        if !self.on_home() && l.list.w > 0.0 {
            let details = l.mode == LayoutMode::Details;
            let files = t.add(0, FILES, if details { Role::DataGrid } else { Role::List }, S::A11yRegionFiles.t(), union([l.header, l.list]));
            if self.focus == Focus::List {
                focus = Some(files);
            }
            if details && l.header.h > 0.0 {
                let head = t.add(files, HEADER, Role::Header, S::A11yRegionColumns.t(), l.header);
                for (c, col) in l.columns.iter().enumerate() {
                    put(&mut t, &mut targets, head, Hit::Header(c), Role::HeaderItem, col.title(), l.cols[c]);
                }
            }
            let tab = &self.tabs[self.cur];
            if let Some(v) = &tab.view {
                let arena = Bump::new();
                let f = self.flow();
                let listing = v.listing();
                for line in self.visible_lines(&f) {
                    let Some(Line::Row(row)) = v.line(line) else { continue };
                    let i = v.rows()[row];
                    let body = self.item_parts(self.line_rect(&f, line)).body;
                    let id = hash::hash(&("row", tab.id, listing.name(i))) | LEAF;
                    targets.push((id, Hit::Row(row)));
                    let k = t.add(files, id, if details { Role::DataItem } else { Role::ListItem }, self.shown_name(listing, i), body);
                    t.nodes[k].selected = v.is_selected(i);
                    if self.focus == Focus::List && v.focus == Some(i) {
                        focus = Some(k);
                    }
                    let cols: Vec<(Column, Rect)> = if details { l.columns.iter().copied().zip(l.cols.iter().copied()).skip(1).collect() } else { Vec::new() };
                    let mut values = Vec::new();
                    for (col, r) in cols {
                        let text = self.cell_text(&arena, listing, i, col);
                        let c = t.add(k, id ^ hash::hash(col.title()), Role::Text, text, Rect::new(r.x, body.y, r.w, body.h));
                        t.nodes[c].help = col.title().into();
                        values.push(format!("{}: {text}", col.title()));
                    }
                    t.nodes[k].value = values.join("; ");
                }
            }
        }

        let pane = self.preview_area();
        if pane.w > 0.0 {
            let k = t.add(0, PREVIEW, Role::Pane, S::A11yRegionPreview.t(), pane);
            if self.focus == Focus::Preview {
                focus = Some(k);
            }
        }

        if let Some(k) = self.settings_a11y(&mut t, &mut targets) {
            focus = Some(k);
        }

        if let Some((title, lines)) = self.dialog_text() {
            let (rect, buttons) = self.dialog_rects();
            let buttons = buttons.to_vec();
            let d = t.add(0, DIALOG, Role::Dialog, title, rect);
            t.nodes[d].help = lines.join(" ");
            focus = Some(d);
            for (k, (label, r)) in self.dialog_buttons().into_iter().zip(buttons).enumerate() {
                let b = put(&mut t, &mut targets, d, Hit::DialogButton(k), Role::Button, label, r);
                if k == self.jobs.sel {
                    focus = Some(b);
                }
            }
        }

        if let Some(k) = self.form_a11y(&mut t, &mut targets) {
            focus = Some(k);
        }

        t.focus = if self.active { focus } else { None };
        self.a11y_targets = targets;
        t
    }

    fn a11y_hit(&self, id: u64) -> Option<Hit> {
        self.a11y_targets.iter().find(|(k, _)| *k == id).map(|&(_, h)| h)
    }

    pub(super) fn a11y_invoke(&mut self, id: u64, now: Instant) {
        match self.a11y_hit(id) {
            Some(Hit::Form(k)) => self.form_a11y_invoke(k),
            Some(Hit::FormFlag(k, j)) => self.form_a11y_flag(k, j, true),
            Some(h) => self.activate(h, now),
            None => {}
        }
    }

    pub(super) fn a11y_focus(&mut self, id: u64) {
        let Some(h) = self.a11y_hit(id) else { return };
        self.dirty = true;
        match h {
            Hit::Settings(sh) => self.settings_a11y_focus(sh),
            Hit::Form(k) => self.form_a11y_focus(k),
            Hit::FormFlag(k, j) => self.form_a11y_flag(k, j, false),
            Hit::DialogButton(k) => self.jobs.sel = k,
            Hit::Row(row) => {
                self.focus = Focus::List;
                if let Some(v) = &mut self.tabs[self.cur].view {
                    v.focus = v.rows().get(row).copied();
                }
            }
            _ => {
                let Some((region, n)) = ORDER.into_iter().find_map(|r| self.focus_targets(r).iter().position(|&(k, _)| k == h).map(|n| (r, n))) else { return };
                if region == Focus::Address {
                    return self.enter_focus(region);
                }
                self.focus = region;
                self.kb = n;
                if region == Focus::Sidebar {
                    self.nav.sidebar.cursor = n;
                }
            }
        }
    }

    pub(super) fn a11y_select(&mut self, id: u64, now: Instant) {
        match self.a11y_hit(id) {
            Some(Hit::Row(row)) => {
                self.focus = Focus::List;
                self.tabs[self.cur].focus_row(row, crate::input::Mods::default());
                self.dirty = true;
            }
            Some(_) => self.a11y_invoke(id, now),
            None => {}
        }
    }
}

#[cfg(test)]
#[path = "../../test/ui/a11y.rs"]
mod tests;
