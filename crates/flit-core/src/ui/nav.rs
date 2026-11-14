use super::field::{is_menu_key, text_entries, Field, TextCmd};
use super::filter::FilterRects;
use super::home::Home;
use super::palette::{Palette, Pick};
use super::preview_pane::PaneHit;
use super::settings_page::{SHit, SettingPick};
use super::sidebar::Sidebar;
use super::{App, Focus, Hit};
use crate::input::{Key, Mods, MouseButton};
use crate::location::Location;
use crate::render::Rect;
use std::time::Instant;

const CLICK_SLOP: f32 = 4.0;

#[derive(Default)]
pub(super) struct Nav {
    pub sidebar: Sidebar,
    pub palette: Option<Palette>,
    pub back: Option<Box<Palette>>,
    pub filter: FilterRects,
    pub closed: Vec<Closed>,
    pub home: Home,
    streak: Option<(Instant, f32, f32)>,
    clipboard_text: Option<bool>,
}

pub(super) struct Closed {
    pub history: crate::nav::History,
    pub view: Option<crate::nav::Restore>,
    pub at: Instant,
}

impl Closed {
    pub fn location(&self) -> &Location {
        self.history.current()
    }
}

fn is_field(h: Hit) -> bool {
    matches!(h, Hit::FilterField | Hit::PaletteField | Hit::QuickField | Hit::Preview(PaneHit::Filter) | Hit::Settings(SHit::Search) | Hit::Form(_))
}

impl App {
    pub(super) fn layout_nav(&mut self, top: f32, bottom: f32) {
        self.layout_sidebar(top, bottom);
        self.layout_palette();
    }

    pub(super) fn nav_hit(&self, x: f32, y: f32) -> Option<Hit> {
        self.palette_hit(x, y).or_else(|| self.sidebar_hit(x, y)).or_else(|| self.filter_hit(x, y)).or_else(|| self.home_hit(x, y))
    }

    pub(super) fn field_menu_key(&mut self, key: Key, m: Mods) -> bool {
        if !is_menu_key(key, m) || self.is_renaming() {
            return false;
        }
        let Some(r) = self.text_field_rect() else { return false };
        self.text_field_menu(r.x, r.bottom());
        true
    }

    pub(super) fn clipboard_text_changed(&mut self, text: bool) {
        self.nav.clipboard_text = Some(text);
    }

    pub(super) fn text_on_clipboard(&self) -> bool {
        self.nav.clipboard_text != Some(false)
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
        (self.focus == Focus::Filter && self.filter_insert(s)) || (self.focus == Focus::Sidebar && self.quick_insert(s))
    }

    pub(super) fn nav_ime_area(&self) -> Option<Rect> {
        self.palette_field().or_else(|| (self.focus == Focus::Filter).then_some(self.nav.filter.field)).or_else(|| (self.focus == Focus::Sidebar && self.nav.sidebar.typing).then_some(self.nav.sidebar.field))
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
        let palette_hit = matches!(h, Hit::PaletteRow(_) | Hit::PaletteKeys(_) | Hit::PaletteField | Hit::PaletteBox | Hit::PaletteOutside | Hit::PaletteClose | Hit::PaletteDescribe | Hit::PaletteScroll(_) | Hit::GoButton(_));
        if self.nav.palette.is_some() && !palette_hit {
            self.close_palette();
        }
        if self.go_open() && matches!(button, MouseButton::Back | MouseButton::Forward) {
            self.go_history_step(if button == MouseButton::Back { -1 } else { 1 }, now);
            return true;
        }
        if !matches!(button, MouseButton::Left | MouseButton::Middle | MouseButton::Right) {
            return palette_hit;
        }
        let inspector = h == Hit::Preview(PaneHit::Filter) && button != MouseButton::Middle;
        let ours = palette_hit || inspector || matches!(h, Hit::Side(_) | Hit::SidePin(_) | Hit::SideEmpty | Hit::SideHeader(_) | Hit::HomeItem(_) | Hit::HomeEmpty | Hit::Splitter | Hit::FilterField | Hit::FilterClose | Hit::QuickField | Hit::QuickClear);
        if !ours {
            return false;
        }
        match (button, h) {
            (MouseButton::Left, Hit::PaletteOutside) => self.close_palette(),
            (MouseButton::Middle, Hit::Side(i)) => self.open_side_item(i, true, now),
            (MouseButton::Middle, Hit::HomeItem(i)) => self.open_home_item(i, true, now),
            (MouseButton::Left, Hit::Splitter) => self.begin_splitter_drag(x),
            (MouseButton::Left, Hit::FilterField | Hit::PaletteField | Hit::QuickField | Hit::Preview(_)) => {
                let (clicks, extend) = (self.text_clicks((x, y), clicks, now), self.mods.shift);
                match h {
                    Hit::FilterField => self.filter_click(x, clicks, extend),
                    Hit::PaletteField => self.palette_click(x, clicks, extend),
                    Hit::Preview(_) => {
                        self.focus_text_field(h);
                        if let Some(f) = self.inspector_filter_mut() {
                            f.click(x, clicks, extend);
                        }
                    }
                    _ => self.quick_filter_click(x, clicks, extend),
                }
                self.pressed = h;
            }
            (MouseButton::Right, Hit::FilterField | Hit::PaletteField | Hit::QuickField | Hit::Preview(_)) => {
                self.focus_text_field(h);
                self.text_field_menu(x, y);
            }
            (MouseButton::Left, Hit::PaletteScroll(down)) => self.palette_step(down),
            (MouseButton::Left, Hit::Side(_) | Hit::SidePin(_) | Hit::SideHeader(_) | Hit::HomeItem(_) | Hit::FilterClose | Hit::PaletteRow(_) | Hit::PaletteKeys(_) | Hit::PaletteClose | Hit::PaletteDescribe | Hit::GoButton(_) | Hit::QuickClear) => {
                self.pressed = h;
                self.nav.sidebar.press_y = y;
            }
            (_, Hit::PaletteOutside) => self.close_palette(),
            _ => {}
        }
        self.dirty = true;
        true
    }

    pub(super) fn nav_mouse_move(&mut self, x: f32) -> bool {
        if !is_field(self.pressed) {
            return false;
        }
        let h = self.pressed;
        let Some(f) = self.text_field_mut(h) else { return false };
        f.drag(x);
        self.dirty = true;
        true
    }

    pub(super) fn text_clicks(&mut self, (x, y): (f32, f32), clicks: u8, now: Instant) -> u8 {
        let slop = self.s(CLICK_SLOP);
        let third = clicks == 1 && self.nav.streak.is_some_and(|(at, sx, sy)| now.saturating_duration_since(at) <= self.double_click && (x - sx).abs() <= slop && (y - sy).abs() <= slop);
        self.nav.streak = (clicks == 2).then_some((now, x, y));
        if third {
            3
        } else {
            clicks
        }
    }

    fn focus_text_field(&mut self, h: Hit) {
        match h {
            Hit::FilterField => {
                if self.tab().filter.is_none() {
                    self.open_filter();
                }
                self.focus = Focus::Filter;
            }
            Hit::QuickField => {
                self.nav.sidebar.typing = true;
                self.focus = Focus::Sidebar;
            }
            Hit::Preview(_) if !self.inspector_field_focused() => self.open_inspector_filter(),
            _ => {}
        }
    }

    fn text_field_hit(&self) -> Option<Hit> {
        if let Some(k) = self.form_text_focus() {
            return Some(Hit::Form(k));
        }
        if self.jobs.modal() {
            return None;
        }
        if let Some(p) = &self.nav.palette {
            return (!matches!(p.pick, Pick::Setting(SettingPick::Capture { .. }))).then_some(Hit::PaletteField);
        }
        if self.settings_ime_area().is_some() {
            return Some(Hit::Settings(SHit::Search));
        }
        if self.inspector_field_focused() {
            return Some(Hit::Preview(PaneHit::Filter));
        }
        match self.focus {
            Focus::Filter if self.tab().filter.is_some() => Some(Hit::FilterField),
            Focus::Sidebar if self.nav.sidebar.typing => Some(Hit::QuickField),
            _ => None,
        }
    }

    fn text_field(&self, h: Hit) -> Option<&Field> {
        match h {
            Hit::FilterField => self.tab().filter.as_ref(),
            Hit::PaletteField => self.nav.palette.as_ref().map(|p| &p.field),
            Hit::QuickField => Some(&self.nav.sidebar.filter),
            Hit::Preview(_) => self.inspector_filter(),
            Hit::Settings(_) => self.settings_search(),
            Hit::Form(k) => self.form_text(k),
            _ => None,
        }
    }

    fn text_field_mut(&mut self, h: Hit) -> Option<&mut Field> {
        match h {
            Hit::FilterField => self.tabs[self.cur].filter.as_mut(),
            Hit::PaletteField => self.nav.palette.as_mut().map(|p| &mut p.field),
            Hit::QuickField => Some(&mut self.nav.sidebar.filter),
            Hit::Preview(PaneHit::Filter) => self.inspector_filter_mut(),
            Hit::Settings(SHit::Search) => self.settings_search_mut(),
            Hit::Form(k) => self.form_text_mut(k),
            _ => None,
        }
    }

    fn text_field_rect(&self) -> Option<Rect> {
        match self.text_field_hit()? {
            Hit::FilterField => Some(self.nav.filter.field),
            Hit::PaletteField => self.palette_field(),
            Hit::Preview(_) => Some(self.inspector_filter_rect()),
            Hit::Settings(_) => self.settings_ime_area(),
            Hit::Form(k) => self.form_text_rect(k),
            _ => Some(self.nav.sidebar.field),
        }
    }

    pub(super) fn text_field_menu(&mut self, x: f32, y: f32) {
        let Some(h) = self.text_field_hit() else { return };
        let menu = self.collect(|a, m| {
            if let Some(f) = a.text_field(h) {
                text_entries(m, &f.edit, f.secret, a.text_on_clipboard());
            }
        });
        if !menu.is_empty() {
            self.flit_menu(x, y, menu);
        }
    }

    pub(super) fn text_command(&mut self, cmd: TextCmd, now: Instant) {
        let (key, m) = cmd.chord();
        if self.is_renaming() {
            self.ops_key(key, m);
        } else if self.search.active {
            self.search_key(key, m, now);
        } else if let Some(h) = self.text_field_hit() {
            if matches!(h, Hit::Form(_) | Hit::Settings(_) | Hit::Preview(_)) {
                self.key(key, m, now);
            } else {
                self.nav_key(key, m, now);
            }
        } else if self.focus == Focus::Preview {
            self.inspector_key(key, m);
        }
        self.dirty = true;
    }
}

#[cfg(test)]
#[path = "../../test/ui/nav.rs"]
mod tests;
