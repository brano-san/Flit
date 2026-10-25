use super::filter::FilterRects;
use super::home::Home;
use super::palette::Palette;
use super::sidebar::Sidebar;
use super::{App, Focus, Hit};
use crate::input::{Key, Mods, MouseButton};
use crate::location::Location;
use crate::render::Rect;
use std::time::Instant;

#[derive(Default)]
pub(super) struct Nav {
    pub sidebar: Sidebar,
    pub palette: Option<Palette>,
    pub filter: FilterRects,
    pub closed: Vec<Location>,
    pub home: Home,
}

impl App {
    pub(super) fn layout_nav(&mut self) {
        let (top, bottom) = (self.layout.commands.bottom(), self.layout.status.y);
        self.layout_sidebar(top, bottom);
        self.layout_palette(self.layout.title.bottom());
    }

    pub(super) fn nav_hit(&self, x: f32, y: f32) -> Option<Hit> {
        self.palette_hit(x, y).or_else(|| self.sidebar_hit(x, y)).or_else(|| self.filter_hit(x, y)).or_else(|| self.home_hit(x, y))
    }

    pub(super) fn nav_key(&mut self, key: Key, m: Mods, now: Instant) -> bool {
        if self.nav.palette.is_some() {
            return self.palette_key(key, m, now);
        }
        let handled = match self.focus {
            Focus::Filter => self.filter_key(key, m),
            Focus::Sidebar => self.sidebar_key(key, m, now),
            _ => false,
        };
        handled || self.run_bound(key, m, now)
    }

    pub(super) fn nav_insert(&mut self, s: &str) -> bool {
        if self.nav.palette.is_some() {
            return self.palette_insert(s);
        }
        self.focus == Focus::Filter && self.filter_insert(s)
    }

    pub(super) fn nav_ime_area(&self) -> Option<Rect> {
        self.palette_field().or_else(|| (self.focus == Focus::Filter).then_some(self.nav.filter.field))
    }

    pub(super) fn nav_wheel(&mut self, dy: f32) -> bool {
        if self.nav.palette.is_some() {
            self.palette_wheel(dy);
            return true;
        }
        if matches!(self.hover, Hit::Side(_) | Hit::SidePin(_) | Hit::SideEmpty | Hit::SideHeader(_)) {
            self.sidebar_wheel(dy);
            return true;
        }
        if matches!(self.hover, Hit::HomeItem(_) | Hit::HomeEmpty) {
            self.home_wheel(dy);
            return true;
        }
        false
    }

    pub(super) fn nav_mouse_down(&mut self, h: Hit, (x, y): (f32, f32), button: MouseButton, clicks: u8, now: Instant) -> bool {
        let palette_hit = matches!(h, Hit::PaletteRow(_) | Hit::PaletteField | Hit::PaletteBox | Hit::PaletteOutside);
        if self.nav.palette.is_some() && !palette_hit {
            self.close_palette();
        }
        if !matches!(button, MouseButton::Left | MouseButton::Middle | MouseButton::Right) {
            return false;
        }
        let ours = palette_hit || matches!(h, Hit::Side(_) | Hit::SidePin(_) | Hit::SideEmpty | Hit::SideHeader(_) | Hit::SideSettings | Hit::HomeItem(_) | Hit::HomeEmpty | Hit::Splitter | Hit::FilterField | Hit::FilterClose);
        if !ours {
            return false;
        }
        if self.editing {
            self.cancel_edit();
        }
        match (button, h) {
            (MouseButton::Left, Hit::PaletteOutside) => self.close_palette(),
            (MouseButton::Middle, Hit::Side(i)) => self.open_side_item(i, true, now),
            (MouseButton::Middle, Hit::HomeItem(i)) => self.open_home_item(i, true, now),
            (MouseButton::Left, Hit::Splitter) => self.begin_splitter_drag(x),
            (MouseButton::Left, Hit::FilterField) => self.filter_click(x, clicks),
            (MouseButton::Left, Hit::PaletteField) => self.palette_click(x, clicks),
            (MouseButton::Left, Hit::Side(_) | Hit::SidePin(_) | Hit::SideHeader(_) | Hit::SideSettings | Hit::HomeItem(_) | Hit::FilterClose | Hit::PaletteRow(_)) => {
                self.pressed = h;
                self.nav.sidebar.press_y = y;
            }
            (_, Hit::PaletteOutside) => self.close_palette(),
            _ => {}
        }
        self.dirty = true;
        true
    }
}

#[cfg(test)]
#[path = "../../test/ui/nav.rs"]
mod tests;
