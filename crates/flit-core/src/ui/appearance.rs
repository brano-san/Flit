use super::menu::Cmd;
use super::palette::{Palette as PickList, Pick};
use super::settings::{Kind, Picker, Value, SETTINGS};
use super::settings_page::SettingPick;
use super::{App, Effect, SettingsCmd};
use crate::theme::{Area, Mode, Palette, Tint, PRESETS};

pub const DEFAULT_FONT: &str = "System default";
const MAX_FONT: usize = 64;

pub struct Look {
    pub mode: Mode,
    pub background: Tint,
    pub areas: [Tint; 5],
    pub font: String,
    pub palette: Palette,
    pub font_missing: bool,
    fonts: Option<Vec<String>>,
    font_pick: Option<usize>,
    system_dark: bool,
}

impl Look {
    pub(super) fn new(system_dark: bool) -> Self {
        Self {
            mode: Mode::System,
            background: Tint::Default,
            areas: [Tint::Default; 5],
            font: String::new(),
            palette: Palette::new(system_dark, Tint::Default, [Tint::Default; 5]),
            font_missing: false,
            fonts: None,
            font_pick: None,
            system_dark,
        }
    }
}

impl Picker {
    pub fn accepts(self, s: &str) -> bool {
        match self {
            Picker::Color => Tint::parse(s).is_some(),
            Picker::Font => s.len() <= MAX_FONT && !s.chars().any(char::is_control),
        }
    }
}

impl App {
    pub fn is_dark(&self) -> bool {
        self.dark
    }

    pub(super) fn set_system_dark(&mut self, dark: bool) {
        self.look.system_dark = dark;
        self.restyle();
    }

    fn restyle(&mut self) {
        let l = &mut self.look;
        l.palette = Palette::new(l.mode.dark(l.system_dark), l.background, l.areas);
        self.dark = l.palette.dark;
        self.dirty = true;
    }

    pub(super) fn set_theme_mode(&mut self, mode: Mode) {
        self.look.mode = mode;
        self.restyle();
    }

    pub(super) fn set_background(&mut self, key: &str) {
        self.look.background = Tint::parse(key).unwrap_or_default();
        self.restyle();
    }

    pub(super) fn set_area_color(&mut self, area: Area, key: &str) {
        self.look.areas[area as usize] = Tint::parse(key).unwrap_or_default();
        self.restyle();
    }

    pub(super) fn set_ui_font(&mut self, name: &str) {
        self.look.font = name.trim().to_owned();
        self.dirty = true;
    }

    pub(super) fn toggle_theme(&mut self) {
        let dark = self.look.mode.dark(self.look.system_dark);
        self.set_theme_mode(if dark { Mode::Light } else { Mode::Dark });
    }

    pub(super) fn color_menu(&mut self, i: usize, x: f32, y: f32) {
        let cur = match SETTINGS[i].get(self) {
            Value::Text(t) => Tint::parse(&t).unwrap_or_default(),
            _ => Tint::Default,
        };
        let mut entries = vec![("Default".to_owned(), Cmd::Setting(i, 0), cur == Tint::Default)];
        entries.extend(PRESETS.iter().enumerate().map(|(k, p)| (p.label.t().to_owned(), Cmd::Setting(i, k + 1), cur == Tint::Preset(k))));
        entries.push(("Custom color…".to_owned(), Cmd::Setting(i, PRESETS.len() + 1), matches!(cur, Tint::Custom(_))));
        let items = self.list_menu(entries);
        self.flit_menu(x, y, items);
    }

    pub(super) fn choose_color(&mut self, i: usize, k: usize) -> bool {
        let key = match k {
            0 => String::new(),
            _ if k <= PRESETS.len() => PRESETS[k - 1].id.to_owned(),
            _ => {
                let cur = match SETTINGS[i].get(self) {
                    Value::Text(t) if t.starts_with('#') => t,
                    _ => String::new(),
                };
                self.open_pick(PickList::new(Pick::Setting(SettingPick::Color(i)), Vec::new(), &cur));
                return false;
            }
        };
        SETTINGS[i].set(self, Value::Text(key))
    }

    pub(super) fn pick_font(&mut self, i: usize) {
        let Some(fonts) = &self.look.fonts else {
            self.look.font_pick = Some(i);
            self.effects.push(Effect::Settings(SettingsCmd::Fonts));
            return;
        };
        let items = std::iter::once(DEFAULT_FONT.to_owned()).chain(fonts.iter().cloned()).map(|f| (f, String::new())).collect();
        self.open_pick(PickList::new(Pick::Setting(SettingPick::Font(i)), items, ""));
    }

    pub fn set_fonts(&mut self, fonts: Vec<String>) {
        self.look.fonts = Some(fonts);
        if let Some(i) = self.look.font_pick.take() {
            self.pick_font(i);
        }
    }

    pub(super) fn choose_font(&mut self, i: usize, id: usize) {
        let name = match id {
            0 => String::new(),
            _ => self.look.fonts.as_ref().and_then(|f| f.get(id - 1)).cloned().unwrap_or_default(),
        };
        if SETTINGS[i].set(self, Value::Text(name)) {
            self.layout();
        }
    }

    pub(super) fn pick_text(&self, i: usize) -> Option<String> {
        let Kind::Pick(picker, get, _) = SETTINGS[i].kind else { return None };
        let v = get(self);
        Some(match picker {
            Picker::Color => match Tint::parse(&v).unwrap_or_default() {
                Tint::Default => "Default".to_owned(),
                t => t.label(),
            },
            Picker::Font if v.is_empty() => DEFAULT_FONT.to_owned(),
            Picker::Font if self.look.font_missing => format!("{v} (not installed)"),
            Picker::Font => v,
        })
    }

    pub(super) fn swatch(&self, i: usize) -> Option<crate::render::Color> {
        let Kind::Pick(Picker::Color, get, _) = SETTINGS[i].kind else { return None };
        Tint::parse(&get(self)).and_then(|t| t.color(self.dark))
    }
}

#[cfg(test)]
#[path = "../../test/ui/appearance.rs"]
mod tests;
