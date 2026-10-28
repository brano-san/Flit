use crate::i18n::S;
use crate::render::Color;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Theme {
    pub bg: Color,
    pub chrome: Color,
    pub header: Color,
    pub text: Color,
    pub text_dim: Color,
    pub accent: Color,
    pub hover: Color,
    pub selection: Color,
    pub selection_inactive: Color,
    pub selection_text: Color,
    pub focus_ring: Color,
    pub separator: Color,
    pub input_bg: Color,
    pub input_border: Color,
    pub text_selection: Color,
    pub button_hover: Color,
    pub button_pressed: Color,
    pub close_hover: Color,
    pub scrollbar: Color,
    pub scrollbar_hover: Color,
    pub placeholder: Color,
    pub error: Color,
    pub popup: Color,
    pub popup_border: Color,
    pub accent_fill: Color,
    pub teal: Color,
    pub destructive: Color,
}

pub const LIGHT: Theme = Theme {
    bg: Color::rgb(0xFFFFFF),
    chrome: Color::rgb(0xF3F3F3),
    header: Color::rgb(0xFFFFFF),
    text: Color::rgb(0x1B1B1B),
    text_dim: Color::rgb(0x6B6B6B),
    accent: Color::rgb(0x005FB8),
    hover: Color::rgba(0x0000000A),
    selection: Color::rgba(0x005FB826),
    selection_inactive: Color::rgba(0x00000014),
    selection_text: Color::rgb(0x1B1B1B),
    focus_ring: Color::rgba(0x005FB8A0),
    separator: Color::rgba(0x0000001A),
    input_bg: Color::rgb(0xFFFFFF),
    input_border: Color::rgba(0x00000026),
    text_selection: Color::rgba(0x005FB860),
    button_hover: Color::rgba(0x0000000F),
    button_pressed: Color::rgba(0x0000001A),
    close_hover: Color::rgb(0xC42B1C),
    scrollbar: Color::rgba(0x00000040),
    scrollbar_hover: Color::rgba(0x00000080),
    placeholder: Color::rgba(0x0000001A),
    error: Color::rgb(0xC42B1C),
    popup: Color::rgb(0xFFFFFF),
    popup_border: Color::rgba(0x0000002E),
    accent_fill: Color::rgb(0x005FB8),
    teal: Color::rgb(0x8FD3CF),
    destructive: Color::rgb(0xC9555A),
};

pub const DARK: Theme = Theme {
    bg: Color::rgb(0x1C1C1C),
    chrome: Color::rgb(0x202020),
    header: Color::rgb(0x1C1C1C),
    text: Color::rgb(0xF0F0F0),
    text_dim: Color::rgb(0xA0A0A0),
    accent: Color::rgb(0x60CDFF),
    hover: Color::rgba(0xFFFFFF0F),
    selection: Color::rgba(0x60CDFF30),
    selection_inactive: Color::rgba(0xFFFFFF1A),
    selection_text: Color::rgb(0xF0F0F0),
    focus_ring: Color::rgba(0x60CDFFA0),
    separator: Color::rgba(0xFFFFFF17),
    input_bg: Color::rgb(0x2B2B2B),
    input_border: Color::rgba(0xFFFFFF20),
    text_selection: Color::rgba(0x60CDFF60),
    button_hover: Color::rgba(0xFFFFFF14),
    button_pressed: Color::rgba(0xFFFFFF0A),
    close_hover: Color::rgb(0xC42B1C),
    scrollbar: Color::rgba(0xFFFFFF40),
    scrollbar_hover: Color::rgba(0xFFFFFF80),
    placeholder: Color::rgba(0xFFFFFF1A),
    error: Color::rgb(0xFF99A4),
    popup: Color::rgb(0x262626),
    popup_border: Color::rgba(0xFFFFFF33),
    accent_fill: Color::rgb(0x2D6BD3),
    teal: Color::rgb(0x2B6E6B),
    destructive: Color::rgb(0x9E3B3F),
};

pub const MIN_TEXT_CONTRAST: f32 = 4.5;
pub const MIN_ACCENT_CONTRAST: f32 = 3.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Mode {
    #[default]
    System,
    Light,
    Dark,
}

impl Mode {
    pub fn dark(self, system_dark: bool) -> bool {
        match self {
            Mode::System => system_dark,
            Mode::Light => false,
            Mode::Dark => true,
        }
    }
}

pub struct Preset {
    pub id: &'static str,
    pub label: S,
    light: u32,
    dark: u32,
}

const fn preset(id: &'static str, label: S, light: u32, dark: u32) -> Preset {
    Preset { id, label, light, dark }
}

pub static PRESETS: [Preset; 12] = [
    preset("gold", S::ThemePresetGold, 0xFBF1D3, 0x2E2812),
    preset("orange", S::ThemePresetOrange, 0xFCE8D8, 0x33220F),
    preset("red", S::ThemePresetRed, 0xFBE1DE, 0x361818),
    preset("rose", S::ThemePresetRose, 0xFAE3EE, 0x341A27),
    preset("violet", S::ThemePresetViolet, 0xEDE6F9, 0x261F38),
    preset("blue", S::ThemePresetBlue, 0xE0ECFA, 0x142439),
    preset("seafoam", S::ThemePresetSeafoam, 0xDCF2EE, 0x0F2C29),
    preset("green", S::ThemePresetGreen, 0xE2F1DE, 0x16291A),
    preset("grey", S::ThemePresetGrey, 0xECECEC, 0x2A2A2A),
    preset("overcast", S::ThemePresetOvercast, 0xE3E7EB, 0x262C31),
    preset("storm", S::ThemePresetStorm, 0xD9DFE8, 0x1D2533),
    preset("camouflage", S::ThemePresetCamouflage, 0xE5E8D6, 0x252A19),
];

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Tint {
    #[default]
    Default,
    Preset(usize),
    Custom(u32),
}

impl Tint {
    pub fn parse(s: &str) -> Option<Tint> {
        let s = s.trim();
        if s.is_empty() {
            return Some(Tint::Default);
        }
        if let Some(hex) = s.strip_prefix('#') {
            return (hex.len() == 6 && hex.bytes().all(|b| b.is_ascii_hexdigit())).then(|| Tint::Custom(u32::from_str_radix(hex, 16).unwrap_or(0)));
        }
        PRESETS.iter().position(|p| p.id.eq_ignore_ascii_case(s)).map(Tint::Preset)
    }

    pub fn key(self) -> String {
        match self {
            Tint::Default => String::new(),
            Tint::Preset(i) => PRESETS[i].id.to_owned(),
            Tint::Custom(rgb) => format!("#{rgb:06X}"),
        }
    }

    pub fn label(self) -> String {
        match self {
            Tint::Default => String::new(),
            Tint::Preset(i) => PRESETS[i].label.t().to_owned(),
            Tint::Custom(rgb) => format!("#{rgb:06X}"),
        }
    }

    pub fn color(self, dark: bool) -> Option<Color> {
        match self {
            Tint::Default => None,
            Tint::Preset(i) => Some(Color::rgb(if dark { PRESETS[i].dark } else { PRESETS[i].light })),
            Tint::Custom(rgb) => Some(Color::rgb(rgb)),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Area {
    Address,
    Toolbar,
    Sidebar,
    Files,
    Pane,
}

impl Area {
    pub const ALL: [Area; 5] = [Area::Address, Area::Toolbar, Area::Sidebar, Area::Files, Area::Pane];
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub dark: bool,
    pub base: Theme,
    areas: [Theme; 5],
}

impl Palette {
    pub fn new(dark: bool, background: Tint, areas: [Tint; 5]) -> Self {
        let base = match background.color(dark) {
            None => if dark { DARK } else { LIGHT },
            Some(bg) => derive(bg),
        };
        let dark = is_dark(base.bg);
        let areas = Area::ALL.map(|a| match areas[a as usize].color(dark) {
            None => base,
            Some(c) if a == Area::Address => Theme { input_bg: c, ..surface(c) },
            Some(c) => surface(c),
        });
        Self { dark, base, areas }
    }

    pub fn area(&self, a: Area) -> &Theme {
        &self.areas[a as usize]
    }

    pub fn high_contrast(c: &SystemColors) -> Self {
        let base = high_contrast(c);
        Self { dark: is_dark(base.bg), base, areas: [base; 5] }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SystemColors {
    pub window: Color,
    pub window_text: Color,
    pub highlight: Color,
    pub highlight_text: Color,
    pub hotlight: Color,
    pub gray_text: Color,
    pub btn_face: Color,
}

pub fn high_contrast(c: &SystemColors) -> Theme {
    Theme {
        bg: c.window,
        chrome: c.btn_face,
        header: c.window,
        text: c.window_text,
        text_dim: c.window_text,
        accent: c.hotlight,
        hover: c.highlight.alpha(0x40),
        selection: c.highlight,
        selection_inactive: c.highlight,
        selection_text: c.highlight_text,
        focus_ring: c.window_text,
        separator: c.window_text,
        input_bg: c.window,
        input_border: c.window_text,
        text_selection: c.highlight,
        button_hover: c.highlight.alpha(0x40),
        button_pressed: c.highlight.alpha(0x80),
        close_hover: c.highlight,
        scrollbar: c.window_text,
        scrollbar_hover: c.hotlight,
        placeholder: c.gray_text,
        error: c.hotlight,
        popup: c.window,
        popup_border: c.window_text,
        accent_fill: c.highlight,
        teal: c.highlight,
        destructive: c.highlight,
    }
}

pub fn derive(bg: Color) -> Theme {
    let dark = is_dark(bg);
    let base = if dark { DARK } else { LIGHT };
    let pole = if dark { WHITE } else { BLACK };
    let chrome = mix(bg, pole, if dark { 0.018 } else { 0.047 });
    let accent = readable(base.accent, chrome, pole, MIN_ACCENT_CONTRAST);
    Theme {
        bg,
        header: bg,
        chrome,
        input_bg: mix(bg, WHITE, if dark { 0.06 } else { 0.7 }),
        text: readable(base.text, chrome, pole, 7.0),
        selection_text: readable(base.text, chrome, pole, 7.0),
        text_dim: readable(base.text_dim, chrome, pole, MIN_TEXT_CONTRAST),
        accent,
        selection: accent.alpha(alpha(base.selection)),
        focus_ring: accent.alpha(alpha(base.focus_ring)),
        text_selection: accent.alpha(alpha(base.text_selection)),
        error: readable(base.error, chrome, pole, MIN_TEXT_CONTRAST),
        popup: mix(bg, WHITE, if dark { 0.06 } else { 0.7 }),
        ..base
    }
}

fn surface(c: Color) -> Theme {
    Theme { chrome: c, header: c, ..derive(c) }
}

const WHITE: Color = Color::rgb(0xFFFFFF);
const BLACK: Color = Color::rgb(0x000000);

fn alpha(c: Color) -> u8 {
    (c.0 >> 24) as u8
}

fn channels(c: Color) -> [f32; 3] {
    [c.0 & 0xFF, (c.0 >> 8) & 0xFF, (c.0 >> 16) & 0xFF].map(|v| v as f32)
}

pub fn mix(a: Color, b: Color, t: f32) -> Color {
    let (x, y) = (channels(a), channels(b));
    let [r, g, bl] = [0, 1, 2].map(|i| (x[i] + (y[i] - x[i]) * t).round().clamp(0.0, 255.0) as u32);
    Color(r | g << 8 | bl << 16 | (a.0 & 0xFF00_0000))
}

pub fn luminance(c: Color) -> f32 {
    let lin = |v: f32| {
        let s = v / 255.0;
        if s <= 0.040_45 { s / 12.92 } else { ((s + 0.055) / 1.055).powf(2.4) }
    };
    let [r, g, b] = channels(c).map(lin);
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

pub fn contrast(a: Color, b: Color) -> f32 {
    let (x, y) = (luminance(a), luminance(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

pub fn is_dark(bg: Color) -> bool {
    contrast(WHITE, bg) > contrast(BLACK, bg)
}

pub fn ink_on(fill: Color) -> Color {
    if is_dark(fill) { WHITE } else { BLACK }
}

fn readable(c: Color, bg: Color, pole: Color, min: f32) -> Color {
    (0..=20).map(|k| mix(c, pole, k as f32 / 20.0)).find(|&m| contrast(m, bg) >= min).unwrap_or(pole)
}

#[cfg(test)]
#[path = "../test/theme.rs"]
mod tests;
