use crate::render::Color;

#[derive(Clone, Copy, Debug)]
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
};

pub fn get(dark: bool) -> &'static Theme {
    if dark { &DARK } else { &LIGHT }
}
