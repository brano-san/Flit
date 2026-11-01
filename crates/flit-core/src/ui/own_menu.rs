use super::context::{ContextMenu, MenuKind};
use super::edit::EditAction;
use super::menu::MenuItem;
use super::popup::{MenuOutcome, MenuView};
use super::widgets::Ghost;
use super::{App, Effect, ShellMenu};
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
    kind: MenuKind,
    view: Option<View>,
    pub(super) opened: Instant,
    right: Option<f32>,
    pointer: Option<(f32, f32)>,
    pending: bool,
    own: usize,
    shape: Option<(String, Vec<u8>)>,
    floor_w: f32,
    carry: String,
}

impl OwnMenu {
    fn holds_shell(&self) -> bool {
        self.pending || !self.shell.is_empty()
    }

    fn placeholders(&mut self, rows: &[u8]) {
        if !rows.is_empty() && !self.items.is_empty() {
            self.items.push(MenuItem::Separator);
        }
        self.items.extend(rows.iter().map(|&r| if r == SHAPE_SEPARATOR { MenuItem::Separator } else { MenuItem::Header(String::new()) }));
    }

    fn add_shell(&mut self, items: &[MenuItem], shell: &[u32]) {
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
        self.items.extend_from_slice(items);
        self.shell = shell.to_vec();
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
    pub(super) fn own_menu_event(&mut self, ev: &Event, now: Instant) -> bool {
        if let Event::ShowMenu { items, x, y, launches, kind, shell, pending } = ev {
            self.release_shell_menu();
            let key = self.shapes.next.take().filter(|_| *pending);
            let mut m = OwnMenu { items: items.clone(), at: (*x, *y), launches: launches.clone(), shell: shell.clone(), kind: *kind, view: None, opened: now, right: self.menu_right.take(), pointer: None, pending: *pending, own: items.len(), shape: None, floor_w: 0.0, carry: String::new() };
            if let Some(key) = key {
                if let Some((_, rows, w)) = self.shapes.get(&key) {
                    m.placeholders(rows);
                    m.floor_w = w * self.scale;
                }
                m.shape = Some((key, Vec::new()));
            }
            self.own_menu = Some(m);
            self.tip_hide();
            self.dirty = true;
            return true;
        }
        if let Event::ShellItems { items, shell } = ev {
            if let Some(m) = self.own_menu.as_mut().filter(|m| m.pending) {
                m.add_shell(items, shell);
                self.dirty = true;
            }
            return true;
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
            (View::Context(c), Event::MouseDown { x, y, clicks, .. }) => {
                self.dirty = true;
                c.mouse_down(*x, *y, *clicks)
            }
            (View::Plain(v), Event::MouseUp { x, y, .. }) => v.mouse_up(*x, *y),
            (View::Context(c), Event::MouseUp { x, y, .. }) => c.mouse_up(*x, *y),
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
            (_, Event::Key { .. } | Event::Char(_)) => MenuOutcome::Open,
            (_, Event::Focus(false) | Event::Resized { .. } | Event::ScaleChanged(_)) => MenuOutcome::Closed,
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
            MenuOutcome::Command(k) => {
                let menu = self.own_menu.take();
                self.dirty = true;
                let Some(m) = menu else { return true };
                let total = commands(&m.items);
                let shell_base = total - m.shell.len();
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
        }
        true
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
        self.confirm = None;
        self.dirty = true;
    }

    pub fn own_menu_open(&self) -> bool {
        self.own_menu.is_some()
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
        (self.menu_owner, self.menu_panel) = (Some(owner), self.panels.here);
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

    pub(super) fn menu_open_from(&self, owner: super::Hit) -> bool {
        self.own_menu.is_some() && self.menu_owner == Some(owner) && self.menu_panel == self.panels.here
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
        let pins = self.own_menu.as_ref().and_then(|m| m.kind.pins()).map(|k| self.opts.pins[k].clone());
        let (active, preedit) = (self.active, self.preedit.clone());
        let mut files = Vec::new();
        if let Some(m) = &self.own_menu {
            super::user_menu::file_images(&m.items, &mut files);
        }
        if !files.is_empty() {
            self.request_menu_images(&files, super::context::image_px(scale), |k| g.has_image(k));
        }
        let Some(m) = self.own_menu.as_mut() else { return };
        let view = m.view.get_or_insert_with(|| match m.kind {
            MenuKind::None => {
                let mut v = MenuView::new(&m.items, m.at.0, m.at.1, bounds, scale, g);
                if let Some(right) = m.right {
                    v.align_right(right);
                }
                if let Some((x, y)) = m.pointer {
                    v.mouse_move(x, y);
                }
                View::Plain(v)
            }
            kind => {
                let mut c = ContextMenu::new(&m.items, kind, pins.unwrap_or_default(), m.at, bounds, scale);
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
        let (from, opened, at) = (g.dl.instances.len(), m.opened, m.at);
        let (now, animate) = (self.last_frame.unwrap_or_else(Instant::now), self.animations);
        let glowing = match view {
            View::Plain(v) => {
                let moving = v.settle_glow(now, animate);
                v.draw(g, t);
                moving
            }
            View::Context(c) => {
                let moving = c.settle_glow(now, animate);
                c.draw(g, t, active, &preedit);
                moving
            }
        };
        self.motion |= glowing;
        self.keep_ghost(g, Ghost::Menu, from, at);
        self.pop(g, from, opened, at);
    }
}

#[cfg(test)]
#[path = "../../test/ui/own_menu.rs"]
mod tests;
