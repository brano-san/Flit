use super::context::{ContextMenu, MenuKind};
use super::edit::EditAction;
use super::menu::MenuItem;
use super::popup::{MenuOutcome, MenuView};
use super::{App, Effect};
use crate::input::Event;
use crate::provider::Launch;
use crate::render::Rect;
use crate::text::{Gfx, TextBackend};
use crate::theme::Theme;
use std::time::Instant;

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
    opened: Instant,
    right: Option<f32>,
    pointer: Option<(f32, f32)>,
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
        if let Event::ShowMenu { items, x, y, launches, kind, shell } = ev {
            self.release_shell_menu();
            self.own_menu = Some(OwnMenu { items: items.clone(), at: (*x, *y), launches: launches.clone(), shell: shell.clone(), kind: *kind, view: None, opened: now, right: self.menu_right.take(), pointer: None });
            self.tip_hide();
            self.dirty = true;
            return true;
        }
        if let (Some(m @ OwnMenu { view: None, .. }), Event::MouseMove { x, y }) = (self.own_menu.as_mut(), ev) {
            m.pointer = Some((*x, *y));
            return true;
        }
        let mods = self.mods;
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
            (View::Context(c), Event::Wheel { dy, .. }) => {
                c.wheel(*dy);
                self.dirty = true;
                MenuOutcome::Open
            }
            (_, Event::Key { .. } | Event::Char(_) | Event::Wheel { .. }) => MenuOutcome::Open,
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
                        if !m.shell.is_empty() {
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
        if self.own_menu.as_ref().is_some_and(|m| !m.shell.is_empty()) {
            self.effects.push(Effect::ShellPick(None));
        }
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
        self.menu_owner = Some(owner);
    }

    pub(super) fn menu_open_from(&self, owner: super::Hit) -> bool {
        self.own_menu.is_some() && self.menu_owner == Some(owner)
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
                c.measure(g);
                if let Some((x, y)) = m.pointer {
                    c.mouse_move(x, y);
                }
                View::Context(Box::new(c))
            }
        });
        let (from, opened, at) = (g.dl.instances.len(), m.opened, m.at);
        match view {
            View::Plain(v) => v.draw(g, t),
            View::Context(c) => c.draw(g, t, active, &preedit),
        }
        self.pop(g, from, opened, at);
    }
}

#[cfg(test)]
#[path = "../../test/ui/own_menu.rs"]
mod tests;
