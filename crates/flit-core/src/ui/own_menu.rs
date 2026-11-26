use super::context::{ContextMenu, CtxHit, MenuKind};
use super::edit::EditAction;
use super::menu::{Cmd, Mark, MenuItem};
use super::popup::{MenuOutcome, MenuView};
use super::widgets::Ghost;
use super::{App, Effect, Hit, MenuOwner, ShellMenu};
use crate::input::{Event, MouseButton};
use crate::provider::Launch;
use crate::render::Rect;
use crate::text::{Gfx, TextBackend};
use crate::theme::Theme;
use std::time::Instant;

const MAX_SHAPES: usize = 24;
const SHAPE_ROW: u8 = 0;
const SHAPE_SEPARATOR: u8 = 1;
const SHAPE_SUBMENU: u8 = 2;

#[derive(Default)]
pub(super) struct Shapes {
    list: Vec<(String, Vec<u8>, f32)>,
    next: Option<String>,
}

impl Shapes {
    pub fn get(&self, key: &str) -> Option<&(String, Vec<u8>, f32)> {
        self.list.iter().find(|s| s.0 == key)
    }

    fn put(&mut self, key: String, rows: Vec<u8>, width: f32) {
        self.list.retain(|s| s.0 != key);
        self.list.insert(0, (key, rows, width));
        self.list.truncate(MAX_SHAPES);
    }
}

pub(super) fn shape_key(m: &ShellMenu) -> String {
    match &m.items[..] {
        [] => "background".into(),
        [one] if one.kind() == crate::path::PathKind::Drive && one.parent().is_none() => "drive".into(),
        [_] if m.kind == MenuKind::Folder => "folder".into(),
        [one] => {
            let name = one.file_name();
            let ext = name.iter().rposition(|&c| c == b'.').map_or(&[][..], |k| &name[k + 1..]);
            format!("file.{}", String::from_utf8_lossy(ext).to_lowercase())
        }
        _ => "multi".into(),
    }
}

fn shape_of(items: &[MenuItem]) -> Vec<u8> {
    items
        .iter()
        .map(|i| match i {
            MenuItem::Separator => SHAPE_SEPARATOR,
            MenuItem::Submenu { .. } => SHAPE_SUBMENU,
            _ => SHAPE_ROW,
        })
        .collect()
}

pub(super) enum View {
    Plain(MenuView),
    Context(Box<ContextMenu>),
}

pub struct OwnMenu {
    items: Vec<MenuItem>,
    at: (f32, f32),
    launches: Vec<Launch>,
    shell: Vec<u32>,
    terminals: Vec<u32>,
    kind: MenuKind,
    view: Option<View>,
    pub(super) shown: Option<Instant>,
    right: Option<f32>,
    dropdown: Option<Rect>,
    pointer: Option<(f32, f32)>,
    pending: bool,
    own: usize,
    shape: Option<(String, Vec<u8>)>,
    floor_w: f32,
    carry: String,
    tail: bool,
}

impl OwnMenu {
    fn new(items: Vec<MenuItem>, at: (f32, f32), kind: MenuKind) -> Self {
        let own = items.len();
        Self { items, at, kind, own, launches: Vec::new(), shell: Vec::new(), terminals: Vec::new(), view: None, shown: None, right: None, dropdown: None, pointer: None, pending: false, shape: None, floor_w: 0.0, carry: String::new(), tail: false }
    }

    fn holds_shell(&self) -> bool {
        self.pending || !self.shell.is_empty()
    }

    fn placeholders(&mut self, rows: &[u8]) {
        if !rows.is_empty() && !self.items.is_empty() {
            self.items.push(MenuItem::Separator);
        }
        self.tail = rows.last() == Some(&SHAPE_SUBMENU);
        self.items.extend(rows.iter().map(|&r| if r == SHAPE_SEPARATOR { MenuItem::Separator } else { MenuItem::Header(String::new()) }));
    }

    fn add_shell(&mut self, items: &[MenuItem], shell: &[u32], terminals: &[u32]) {
        self.items.truncate(self.own);
        if let Some(View::Context(c)) = &self.view {
            self.carry = c.query.text().to_owned();
        }
        if let Some(s) = self.shape.as_mut() {
            s.1 = shape_of(items);
        }
        if !items.is_empty() && !self.items.is_empty() {
            self.items.push(MenuItem::Separator);
        }
        self.tail = matches!(items.last(), Some(MenuItem::Submenu { .. }));
        self.items.extend_from_slice(items);
        self.shell = shell.to_vec();
        self.terminals = terminals.to_vec();
        self.pending = false;
        self.view = None;
    }
}

pub fn commands(items: &[MenuItem]) -> usize {
    items
        .iter()
        .map(|i| match i {
            MenuItem::Command { .. } => 1,
            MenuItem::Submenu { items, .. } => commands(items),
            MenuItem::Header(_) | MenuItem::Separator => 0,
        })
        .sum()
}

impl App {
    pub(super) fn set_menu_disabled(&mut self, command: usize, value: bool) {
        fn set(items: &mut [MenuItem], next: &mut usize, command: usize, value: bool) {
            for item in items {
                match item {
                    MenuItem::Command { disabled, .. } => {
                        if *next == command {
                            *disabled = value;
                        }
                        *next += 1;
                    }
                    MenuItem::Submenu { items, .. } => set(items, next, command, value),
                    _ => {}
                }
            }
        }
        if let Some(menu) = &mut self.own_menu {
            set(&mut menu.items, &mut 0, command, value);
            match &mut menu.view {
                Some(View::Plain(v)) => v.set_disabled(command, value),
                Some(View::Context(c)) => c.set_disabled(command, value),
                None => {}
            }
            self.dirty = true;
        }
    }

    pub(super) fn own_menu_event(&mut self, ev: &Event, now: Instant) -> bool {
        if matches!(ev, Event::Focus(false) | Event::Resized { .. } | Event::ScaleChanged(_)) {
            if self.own_menu.is_some() {
                self.close_own_menu();
            }
            return false;
        }
        if let Event::ShowMenu { items, x, y, launches, kind, shell, pending } = ev {
            if self.dropdown_open() {
                return true;
            }
            if !*pending {
                self.release_shell_menu();
            }
            let key = self.shapes.next.take().filter(|_| *pending);
            let mut m = OwnMenu { launches: launches.clone(), shell: shell.clone(), right: self.menu_right.take(), pending: *pending, ..OwnMenu::new(items.clone(), (*x, *y), *kind) };
            if let Some(key) = key {
                if let Some((_, rows, w)) = self.shapes.get(&key) {
                    m.placeholders(rows);
                    m.floor_w = w * self.scale;
                }
                m.shape = Some((key, Vec::new()));
            }
            self.own_menu = Some(m);
            self.refresh_link_choices();
            self.tip_hide();
            self.dirty = true;
            return true;
        }
        if let Event::ShellItems { items, shell, terminals } = ev {
            if let Some(m) = self.own_menu.as_mut().filter(|m| m.pending) {
                m.add_shell(items, shell, terminals);
                self.dirty = true;
            }
            return true;
        }
        if self.dropdown_open() && matches!(ev, Event::Key { key: crate::input::Key::Tab | crate::input::Key::Escape, pressed: true, .. }) {
            self.close_own_menu();
            return true;
        }
        if let Event::MouseDown { x, y, button, .. } | Event::MouseUp { x, y, button } = ev {
            if *button != MouseButton::Left && self.dropdown_open() && self.over_own_menu(*x, *y) {
                return true;
            }
        }
        if let (Some(m), Event::MouseMove { x, y }) = (self.own_menu.as_mut(), ev) {
            m.pointer = Some((*x, *y));
            if m.view.is_none() {
                return true;
            }
        }
        if let Event::MouseDown { x, y, button: MouseButton::Right, .. } = ev {
            if self.own_menu.as_ref().is_some_and(|m| m.view.is_some()) && !self.over_own_menu(*x, *y) {
                if self.owner_under(*x, *y) {
                    return true;
                }
                self.close_own_menu();
                return false;
            }
        }
        let mods = self.mods;
        let clicks = match *ev {
            Event::MouseDown { x, y, button: MouseButton::Left, clicks } if self.context_menu_open().is_some_and(|c| c.hit(x, y) == Some(CtxHit::Field)) => self.text_clicks((x, y), clicks, now),
            Event::MouseDown { clicks, .. } => clicks,
            _ => 0,
        };
        let pointer = self.own_menu.as_ref().and_then(|m| m.pointer);
        let Some(view) = self.own_menu.as_mut().and_then(|m| m.view.as_mut()) else { return false };
        let outcome = match (view, ev) {
            (View::Plain(v), Event::MouseMove { x, y }) => {
                self.dirty |= v.mouse_move(*x, *y);
                MenuOutcome::Open
            }
            (View::Context(c), Event::MouseMove { x, y }) => {
                self.dirty |= c.mouse_move(*x, *y);
                MenuOutcome::Open
            }
            (View::Plain(v), Event::MouseDown { x, y, .. }) => v.mouse_down(*x, *y),
            (View::Context(c), Event::MouseDown { x, y, button, .. }) => {
                self.dirty = true;
                match c.pick_field_menu(*x, *y) {
                    Some(Some(cmd)) => {
                        let (k, m) = cmd.chord();
                        let (outcome, action) = c.key(k, m);
                        match action {
                            Some(EditAction::Copy(s)) => self.effects.push(Effect::SetClipboard(s)),
                            Some(EditAction::Paste) => self.effects.push(Effect::Paste),
                            _ => {}
                        }
                        outcome
                    }
                    Some(None) => MenuOutcome::Open,
                    None if self.caps.unix_paths && *button == MouseButton::Middle && c.hit(*x, *y) == Some(CtxHit::Field) => {
                        self.paste_after_layout = Some(super::PrimaryPaste::menu(&mut c.query.edit));
                        c.mouse_down(*x, *y, 1, false)
                    }
                    None if *button == MouseButton::Right && c.hit(*x, *y) == Some(CtxHit::Field) => {
                        c.open_field_menu(*x, *y);
                        MenuOutcome::Open
                    }
                    None => c.mouse_down(*x, *y, clicks, mods.shift),
                }
            }
            (View::Plain(v), Event::MouseUp { x, y, .. }) => {
                self.dirty = true;
                v.mouse_up(*x, *y)
            }
            (View::Context(c), Event::MouseUp { x, y, .. }) => {
                self.dirty = true;
                c.mouse_up(*x, *y)
            }
            (View::Plain(v), Event::Key { key, pressed: true, .. }) => v.key(*key),
            (View::Context(c), Event::Key { key, mods: m, pressed: true }) => {
                self.mods = *m;
                let (outcome, action) = c.key(*key, *m);
                match action {
                    Some(EditAction::Copy(s)) => self.effects.push(Effect::SetClipboard(s)),
                    Some(EditAction::Paste) => self.effects.push(Effect::Paste),
                    _ => {}
                }
                self.dirty = true;
                outcome
            }
            (View::Context(c), Event::Char(ch)) => {
                if !mods.ctrl || mods.alt {
                    c.insert(&ch.to_string());
                    self.dirty = true;
                }
                MenuOutcome::Open
            }
            (View::Plain(v), Event::Wheel { dy, .. }) => {
                self.dirty |= v.wheel(pointer, *dy);
                MenuOutcome::Open
            }
            (View::Context(c), Event::Wheel { dy, .. }) => {
                c.wheel(*dy);
                self.dirty = true;
                MenuOutcome::Open
            }
            (_, Event::Key { .. } | Event::Char(_) | Event::Pinch(_)) => MenuOutcome::Open,
            _ => return false,
        };
        match outcome {
            MenuOutcome::Open => self.dirty |= matches!(ev, Event::Key { .. }),
            MenuOutcome::Closed => self.close_own_menu(),
            MenuOutcome::Pin(key, pin, anchor) => self.ask_pin(key, pin, anchor, now),
            MenuOutcome::Toggled(k) => {
                self.dirty = true;
                self.on_event(&Event::MenuCommand(k), now);
            }
            MenuOutcome::Command(k) => self.run_own_menu_command(k, now),
        }
        true
    }

    fn run_own_menu_command(&mut self, k: usize, now: Instant) {
        let Some(m) = self.own_menu.take() else { return };
        self.dirty = true;
        let shell_base = commands(&m.items) - m.shell.len();
        let launch_base = shell_base - m.launches.len();
        match k {
            k if k >= shell_base => self.effects.push(Effect::ShellPick(m.shell.get(k - shell_base).copied())),
            k if k >= launch_base && !m.launches.is_empty() => {
                if let Some(launch) = m.launches.get(k - launch_base).cloned() {
                    self.launch(launch);
                }
            }
            _ => {
                if m.holds_shell() {
                    self.effects.push(Effect::ShellPick(None));
                }
                self.on_event(&Event::MenuCommand(k), now);
            }
        }
    }

    fn release_shell_menu(&mut self) {
        if self.own_menu.as_ref().is_some_and(OwnMenu::holds_shell) {
            self.effects.push(Effect::ShellPick(None));
        }
    }

    pub(super) fn remember_shape_key(&mut self, m: &ShellMenu) {
        self.shapes.next = m.shell.then(|| shape_key(m));
    }

    fn close_own_menu(&mut self) {
        self.release_shell_menu();
        self.own_menu = None;
        self.dismiss_confirm();
        self.dirty = true;
    }

    pub fn own_menu_open(&self) -> bool {
        self.own_menu.is_some()
    }

    pub(super) fn dropdown_open(&self) -> bool {
        self.own_menu.as_ref().is_some_and(|m| m.dropdown.is_some())
    }

    pub(super) fn close_dropdown(&mut self) {
        if self.dropdown_open() {
            self.close_own_menu();
        }
    }

    pub(super) fn dropdown_menu(&mut self, owner: Hit, anchor: Rect, entries: Vec<(String, Cmd, bool)>) {
        if entries.is_empty() || anchor.w <= 0.0 || anchor.h <= 0.0 {
            return;
        }
        self.release_shell_menu();
        let items = self.list_menu(entries).into_iter().map(|mut item| {
            if let MenuItem::Command { mark, .. } = &mut item {
                *mark = Mark::Radio;
            }
            item
        });
        let menu = OwnMenu::new(items.collect(), (anchor.x, anchor.bottom()), MenuKind::None);
        self.own_menu = Some(OwnMenu { dropdown: Some(anchor), ..menu });
        (self.menu_owner, self.menu_panel) = (Some(MenuOwner::Button(owner)), self.tabs.panels.here);
        self.tip_hide();
        self.dirty = true;
    }

    pub(super) fn dropdown_a11y(&self, tree: &mut crate::a11y::Tree, targets: &mut Vec<(u64, Hit)>) -> Option<usize> {
        use crate::a11y::Role;
        let menu = self.own_menu.as_ref().filter(|m| m.dropdown.is_some())?;
        let Some(View::Plain(view)) = menu.view.as_ref() else { return None };
        let bounds = view.panels().next()?;
        let list = tree.add(0, 1 << 57, Role::List, "", bounds);
        let mut focus = list;
        for (id, name, bounds, selected, hot) in view.dropdown_items() {
            let node = super::a11y::put(tree, targets, list, Hit::DropdownItem(id), Role::ListItem, name, bounds);
            tree.nodes[node].selected = selected;
            if hot {
                focus = node;
            }
        }
        Some(focus)
    }

    pub(super) fn dropdown_a11y_item(&mut self, id: usize, invoke: bool, now: Instant) {
        let Some(View::Plain(view)) = self.own_menu.as_mut().filter(|m| m.dropdown.is_some()).and_then(|m| m.view.as_mut()) else { return };
        if view.focus_dropdown_item(id) {
            self.dirty = true;
            if invoke {
                self.run_own_menu_command(id, now);
            }
        }
    }

    pub(super) fn own_menu_kind(&self) -> Option<MenuKind> {
        self.own_menu.as_ref().map(|m| m.kind)
    }

    pub(super) fn context_menu_open(&self) -> Option<&ContextMenu> {
        match self.own_menu.as_ref()?.view.as_ref()? {
            View::Context(c) => Some(c),
            View::Plain(_) => None,
        }
    }

    pub(super) fn context_menu_mut(&mut self) -> Option<&mut ContextMenu> {
        match self.own_menu.as_mut()?.view.as_mut()? {
            View::Context(c) => Some(c),
            View::Plain(_) => None,
        }
    }

    pub(super) fn button_menu(&mut self, owner: super::Hit, x: f32, y: f32, menu: Vec<MenuItem>) {
        self.flit_menu(x, y, menu);
        (self.menu_owner, self.menu_panel) = (Some(MenuOwner::Button(owner)), self.tabs.panels.here);
    }

    #[cfg(test)]
    pub(super) fn menu_panels(&self) -> Vec<Rect> {
        match self.own_menu.as_ref().and_then(|m| m.view.as_ref()) {
            Some(View::Plain(v)) => v.panels().collect(),
            _ => Vec::new(),
        }
    }

    #[cfg(test)]
    pub(super) fn menu_scroll(&self) -> f32 {
        match self.own_menu.as_ref().and_then(|m| m.view.as_ref()) {
            Some(View::Plain(v)) => v.scrolled(),
            _ => 0.0,
        }
    }

    pub(super) fn menu_open_from(&self, owner: Hit) -> bool {
        if self.own_menu.is_none() || self.menu_panel != self.tabs.panels.here {
            return false;
        }
        match (&self.menu_owner, owner) {
            (Some(MenuOwner::Button(button)), hit) => *button == hit,
            (Some(MenuOwner::SidebarItem { section, loc, label }), Hit::Side(i) | Hit::SidePin(i)) => self.nav.sidebar.items.get(i).is_some_and(|item| item.section == *section && item.loc == *loc && item.label == *label),
            _ => false,
        }
    }

    fn owner_under(&self, x: f32, y: f32) -> bool {
        match self.hit(x, y) {
            super::Hit::TabClose(i) => self.menu_open_from(super::Hit::Tab(i)),
            h => self.menu_open_from(h),
        }
    }

    pub(super) fn over_own_menu(&self, x: f32, y: f32) -> bool {
        match self.own_menu.as_ref().and_then(|m| m.view.as_ref()) {
            Some(View::Plain(v)) => v.contains(x, y),
            Some(View::Context(c)) => c.contains(x, y),
            None => false,
        }
    }

    pub(super) fn draw_own_menu<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme) {
        let (scale, bounds) = (self.scale, Rect::new(0.0, 0.0, self.size.0, self.size.1));
        let pins = self.own_menu.as_ref().and_then(|m| m.kind.pins()).map(|k| self.settings.opts.pins[k].clone());
        let arrangement = self.own_menu.as_ref().filter(|m| m.view.is_none()).and_then(|m| m.kind.pins()).map(|k| (self.settings.opts.menu.hidden[k].clone(), self.settings.opts.menu.order[k].clone()));
        let mut seen = None;
        let (active, preedit) = (self.active, self.preedit.clone());
        let mut files = Vec::new();
        if let Some(m) = &self.own_menu {
            super::user_menu::file_images(&m.items, &mut files);
        }
        if !files.is_empty() {
            self.request_menu_images(&files, super::context::image_px(scale), |k| g.has_image(k));
        }
        let roles = self.own_menu.as_ref().filter(|m| m.view.is_none()).map(|_| super::context::roles(&self.menu)).unwrap_or_default();
        let terminal_fallback = self.own_menu.as_ref().filter(|m| m.view.is_none() && m.kind == MenuKind::Folder && !m.terminals.is_empty()).and_then(|_| self.menu.iter().position(|cmd| matches!(cmd, Cmd::Action("app.terminal"))));
        let Some(m) = self.own_menu.as_mut() else { return };
        let view = m.view.get_or_insert_with(|| match m.kind {
            MenuKind::None => {
                let mut v = MenuView::new(&m.items, m.at.0, m.at.1, bounds, scale, g);
                if let Some(right) = m.right {
                    v.align_right(right);
                }
                if let Some(anchor) = m.dropdown {
                    v.align_dropdown(anchor);
                }
                if let Some((x, y)) = m.pointer {
                    v.mouse_move(x, y);
                }
                View::Plain(v)
            }
            kind => {
                let mut c = ContextMenu::new(&m.items, kind, pins.unwrap_or_default(), m.at, bounds, scale);
                c.set_roles(roles);
                if m.items.len() > m.own {
                    c.lift_shell(m.own, m.tail);
                }
                if let Some((hidden, order)) = &arrangement {
                    if let Some(fallback) = terminal_fallback {
                        let base = commands(&m.items[..m.own]);
                        let terminals = m.shell.iter().enumerate().filter(|(_, id)| m.terminals.contains(id)).map(|(at, _)| base + at);
                        c.prefer_shell_terminal(fallback, terminals, hidden);
                    }
                    c.arrange(hidden, order);
                }
                if !m.pending {
                    seen = Some(c.seen());
                }
                c.floor_w = m.floor_w;
                c.measure(g);
                if !m.carry.is_empty() {
                    c.insert(&std::mem::take(&mut m.carry));
                }
                if let Some((x, y)) = m.pointer {
                    c.mouse_move(x, y);
                }
                if !m.pending {
                    if let Some((key, rows)) = m.shape.take() {
                        self.shapes.put(key, rows, c.width() / scale);
                    }
                }
                View::Context(Box::new(c))
            }
        });
        let (from, at) = (g.dl.instances.len(), m.at);
        let (now, animate) = (self.last_frame.unwrap_or_else(Instant::now), self.animations);
        let since = *m.shown.get_or_insert(now);
        let moving = match view {
            View::Plain(v) => {
                let moving = v.settle_glow(now, animate) | v.settle_unfold(since, now, animate);
                v.draw(g, t);
                moving
            }
            View::Context(c) => {
                let moving = c.settle_glow(now, animate) | c.settle_unfold(since, now, animate);
                c.draw(g, t, active, &preedit);
                moving
            }
        };
        self.motion |= moving;
        self.keep_ghost(g, Ghost::Menu, from, at);
        if let (Some(latest), Some(kind)) = (seen, self.own_menu_kind()) {
            self.remember_menu(kind, latest);
        }
    }
}

#[cfg(test)]
#[path = "../../test/ui/own_menu.rs"]
mod tests;
