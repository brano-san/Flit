use super::menu::MenuItem;
use super::popup::{MenuOutcome, MenuView};
use super::App;
use crate::input::Event;
use crate::provider::Launch;
use crate::render::Rect;
use crate::text::{Gfx, TextBackend};
use crate::theme::Theme;
use std::time::Instant;

pub struct OwnMenu {
    items: Vec<MenuItem>,
    at: (f32, f32),
    launches: Vec<Launch>,
    view: Option<MenuView>,
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
        if let Event::ShowMenu { items, x, y, launches } = ev {
            self.own_menu = Some(OwnMenu { items: items.clone(), at: (*x, *y), launches: launches.clone(), view: None, opened: now, right: self.menu_right.take(), pointer: None });
            self.tip_hide();
            self.dirty = true;
            return true;
        }
        if let (Some(m @ OwnMenu { view: None, .. }), Event::MouseMove { x, y }) = (self.own_menu.as_mut(), ev) {
            m.pointer = Some((*x, *y));
            return true;
        }
        let Some(view) = self.own_menu.as_mut().and_then(|m| m.view.as_mut()) else { return false };
        let outcome = match ev {
            Event::MouseMove { x, y } => {
                self.dirty |= view.mouse_move(*x, *y);
                MenuOutcome::Open
            }
            Event::MouseDown { x, y, .. } => view.mouse_down(*x, *y),
            Event::MouseUp { x, y, .. } => view.mouse_up(*x, *y),
            Event::Key { key, pressed: true, .. } => view.key(*key),
            Event::Key { .. } | Event::Char(_) | Event::Wheel { .. } => MenuOutcome::Open,
            Event::Focus(false) | Event::Resized { .. } | Event::ScaleChanged(_) => MenuOutcome::Closed,
            _ => return false,
        };
        match outcome {
            MenuOutcome::Open => self.dirty |= matches!(ev, Event::Key { .. }),
            MenuOutcome::Closed => self.close_own_menu(),
            MenuOutcome::Toggled(k) => {
                self.dirty = true;
                self.on_event(&Event::MenuCommand(k), now);
            }
            MenuOutcome::Command(k) => {
                let menu = self.own_menu.take();
                self.dirty = true;
                let base = menu.as_ref().map_or(0, |m| commands(&m.items) - m.launches.len());
                match menu.and_then(|m| k.checked_sub(base).and_then(|i| m.launches.get(i).cloned())) {
                    Some(launch) => self.launch(launch),
                    None => self.on_event(&Event::MenuCommand(k), now),
                }
            }
        }
        true
    }

    fn close_own_menu(&mut self) {
        self.own_menu = None;
        self.dirty = true;
    }

    pub fn own_menu_open(&self) -> bool {
        self.own_menu.is_some()
    }

    pub(super) fn button_menu(&mut self, owner: super::Hit, x: f32, y: f32, menu: Vec<MenuItem>) {
        self.flit_menu(x, y, menu);
        self.menu_owner = Some(owner);
    }

    pub(super) fn menu_open_from(&self, owner: super::Hit) -> bool {
        self.own_menu.is_some() && self.menu_owner == Some(owner)
    }

    pub(super) fn over_own_menu(&self, x: f32, y: f32) -> bool {
        self.own_menu.as_ref().and_then(|m| m.view.as_ref()).is_some_and(|v| v.contains(x, y))
    }

    pub(super) fn draw_own_menu<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme) {
        let (scale, bounds) = (self.scale, Rect::new(0.0, 0.0, self.size.0, self.size.1));
        let Some(m) = self.own_menu.as_mut() else { return };
        let view = m.view.get_or_insert_with(|| {
            let mut v = MenuView::new(&m.items, m.at.0, m.at.1, bounds, scale, g);
            if let Some(right) = m.right {
                v.align_right(right);
            }
            if let Some((x, y)) = m.pointer {
                v.mouse_move(x, y);
            }
            v
        });
        let (from, opened, at) = (g.dl.instances.len(), m.opened, m.at);
        view.draw(g, t);
        self.pop(g, from, opened, at);
    }
}

#[cfg(test)]
#[path = "../../test/ui/own_menu.rs"]
mod tests;
