use flit_core::input::{CursorIcon, Key, Mods, MouseButton};
use std::time::{Duration, Instant};
use winit::event::{MouseButton as WinitButton, MouseScrollDelta};
use winit::keyboard::{Key as WinitKey, KeyCode, ModifiersState, NamedKey, PhysicalKey};
use winit::cursor::CursorIcon as WinitCursor;
use winit::window::ResizeDirection;

pub(crate) const DOUBLE_CLICK: Duration = Duration::from_millis(500);
const DOUBLE_CLICK_SLOP: f32 = 4.0;
const WHEEL_PIXELS_PER_NOTCH: f32 = 40.0;
const RESIZE_CORNER_FACTOR: f32 = 3.0;

const FUNCTION_KEYS: [NamedKey; 24] = [
    NamedKey::F1,
    NamedKey::F2,
    NamedKey::F3,
    NamedKey::F4,
    NamedKey::F5,
    NamedKey::F6,
    NamedKey::F7,
    NamedKey::F8,
    NamedKey::F9,
    NamedKey::F10,
    NamedKey::F11,
    NamedKey::F12,
    NamedKey::F13,
    NamedKey::F14,
    NamedKey::F15,
    NamedKey::F16,
    NamedKey::F17,
    NamedKey::F18,
    NamedKey::F19,
    NamedKey::F20,
    NamedKey::F21,
    NamedKey::F22,
    NamedKey::F23,
    NamedKey::F24,
];

const LETTER_KEYS: [KeyCode; 26] = [
    KeyCode::KeyA,
    KeyCode::KeyB,
    KeyCode::KeyC,
    KeyCode::KeyD,
    KeyCode::KeyE,
    KeyCode::KeyF,
    KeyCode::KeyG,
    KeyCode::KeyH,
    KeyCode::KeyI,
    KeyCode::KeyJ,
    KeyCode::KeyK,
    KeyCode::KeyL,
    KeyCode::KeyM,
    KeyCode::KeyN,
    KeyCode::KeyO,
    KeyCode::KeyP,
    KeyCode::KeyQ,
    KeyCode::KeyR,
    KeyCode::KeyS,
    KeyCode::KeyT,
    KeyCode::KeyU,
    KeyCode::KeyV,
    KeyCode::KeyW,
    KeyCode::KeyX,
    KeyCode::KeyY,
    KeyCode::KeyZ,
];

const DIGIT_KEYS: [KeyCode; 10] = [
    KeyCode::Digit0,
    KeyCode::Digit1,
    KeyCode::Digit2,
    KeyCode::Digit3,
    KeyCode::Digit4,
    KeyCode::Digit5,
    KeyCode::Digit6,
    KeyCode::Digit7,
    KeyCode::Digit8,
    KeyCode::Digit9,
];

pub(crate) fn key(physical: PhysicalKey, unmodified: &WinitKey) -> Key {
    let logical = match unmodified {
        WinitKey::Named(named) => named_key(named),
        WinitKey::Character(text) => latin_letter(text).map(Key::Char),
        _ => None,
    };
    logical.unwrap_or_else(|| match physical {
        PhysicalKey::Code(code) => key_at_us_position(code),
        PhysicalKey::Unidentified(_) => Key::Other,
    })
}

fn latin_letter(text: &str) -> Option<char> {
    let mut chars = text.chars();
    let c = chars.next()?;
    (chars.next().is_none() && c.is_ascii_alphabetic()).then(|| c.to_ascii_uppercase())
}

fn named_key(named: &NamedKey) -> Option<Key> {
    let key = match named {
        NamedKey::ArrowUp => Key::Up,
        NamedKey::ArrowDown => Key::Down,
        NamedKey::ArrowLeft => Key::Left,
        NamedKey::ArrowRight => Key::Right,
        NamedKey::PageUp => Key::PageUp,
        NamedKey::PageDown => Key::PageDown,
        NamedKey::Home => Key::Home,
        NamedKey::End => Key::End,
        NamedKey::Enter => Key::Enter,
        NamedKey::Escape => Key::Escape,
        NamedKey::Backspace => Key::Backspace,
        NamedKey::Delete => Key::Delete,
        NamedKey::Insert => Key::Insert,
        NamedKey::Tab => Key::Tab,
        NamedKey::BrowserBack => Key::BrowserBack,
        NamedKey::BrowserForward => Key::BrowserForward,
        NamedKey::ContextMenu => Key::Apps,
        other => return FUNCTION_KEYS.iter().position(|f| f == other).map(|i| Key::F(i as u8 + 1)),
    };
    Some(key)
}

fn key_at_us_position(code: KeyCode) -> Key {
    if let Some(i) = LETTER_KEYS.iter().position(|&k| k == code) {
        return Key::Char((b'A' + i as u8) as char);
    }
    if let Some(i) = DIGIT_KEYS.iter().position(|&k| k == code) {
        return Key::Char((b'0' + i as u8) as char);
    }
    match code {
        KeyCode::Space => Key::Space,
        KeyCode::Backquote => Key::Char('`'),
        KeyCode::Comma => Key::Char(','),
        KeyCode::Period => Key::Char('.'),
        KeyCode::Equal => Key::Char('+'),
        KeyCode::Minus | KeyCode::NumpadSubtract => Key::Char('-'),
        KeyCode::NumpadAdd => Key::NumPlus,
        _ => Key::Other,
    }
}

pub(crate) fn mods(state: ModifiersState) -> Mods {
    Mods { ctrl: state.control_key(), shift: state.shift_key(), alt: state.alt_key() }
}

pub(crate) fn mouse_button(button: WinitButton) -> Option<MouseButton> {
    match button {
        WinitButton::Left => Some(MouseButton::Left),
        WinitButton::Right => Some(MouseButton::Right),
        WinitButton::Middle => Some(MouseButton::Middle),
        WinitButton::Back => Some(MouseButton::Back),
        WinitButton::Forward => Some(MouseButton::Forward),
        _ => None,
    }
}

pub(crate) fn wheel(delta: MouseScrollDelta, scale: f32) -> (f32, f32) {
    match delta {
        MouseScrollDelta::LineDelta(x, y) => (-x, y),
        MouseScrollDelta::PixelDelta(p) => {
            let notch = WHEEL_PIXELS_PER_NOTCH * scale;
            (-p.x as f32 / notch, p.y as f32 / notch)
        }
        _ => (0.0, 0.0),
    }
}

pub(crate) fn cursor(icon: CursorIcon) -> WinitCursor {
    match icon {
        CursorIcon::Default => WinitCursor::Default,
        CursorIcon::Text => WinitCursor::Text,
        CursorIcon::Hand => WinitCursor::Pointer,
        CursorIcon::ResizeHorizontal => WinitCursor::EwResize,
        CursorIcon::ResizeVertical | CursorIcon::Scroll => WinitCursor::NsResize,
    }
}

pub(crate) fn resize_edge(x: f32, y: f32, (w, h): (f32, f32), border: f32) -> Option<ResizeDirection> {
    let corner = border * RESIZE_CORNER_FACTOR;
    let (left, right, top, bottom) = (x < border, x >= w - border, y < border, y >= h - border);
    let (near_left, near_right, near_top, near_bottom) = (x < corner, x >= w - corner, y < corner, y >= h - corner);
    let direction = match () {
        _ if (top && near_left) || (left && near_top) => ResizeDirection::NorthWest,
        _ if (top && near_right) || (right && near_top) => ResizeDirection::NorthEast,
        _ if (bottom && near_left) || (left && near_bottom) => ResizeDirection::SouthWest,
        _ if (bottom && near_right) || (right && near_bottom) => ResizeDirection::SouthEast,
        _ if top => ResizeDirection::North,
        _ if bottom => ResizeDirection::South,
        _ if left => ResizeDirection::West,
        _ if right => ResizeDirection::East,
        _ => return None,
    };
    Some(direction)
}

#[derive(Default)]
pub(crate) struct Clicks {
    last: Option<(MouseButton, Instant, f32, f32, u8)>,
}

impl Clicks {
    pub(crate) fn press(&mut self, button: MouseButton, at: Instant, x: f32, y: f32, scale: f32) -> u8 {
        let slop = DOUBLE_CLICK_SLOP * scale;
        let clicks = match self.last {
            Some((b, t, lx, ly, 1)) if b == button && at.duration_since(t) <= DOUBLE_CLICK && (x - lx).abs() <= slop && (y - ly).abs() <= slop => 2,
            _ => 1,
        };
        self.last = Some((button, at, x, y, clicks));
        clicks
    }

    pub(crate) fn reset(&mut self) {
        self.last = None;
    }
}

#[cfg(test)]
#[path = "../test/input.rs"]
mod tests;
