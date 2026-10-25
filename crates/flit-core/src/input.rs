use crate::path::FsPath;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Back,
    Forward,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Mods {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Key {
    Up,
    Down,
    Left,
    Right,
    PageUp,
    PageDown,
    Home,
    End,
    Enter,
    Escape,
    Backspace,
    Delete,
    Tab,
    Space,
    F(u8),
    Char(char),
    BrowserBack,
    BrowserForward,
    Other,
    Apps,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Resized { width: u32, height: u32 },
    ScaleChanged(f32),
    MouseMove { x: f32, y: f32 },
    MouseDown { x: f32, y: f32, button: MouseButton, clicks: u8 },
    MouseUp { x: f32, y: f32, button: MouseButton },
    Wheel { dx: f32, dy: f32 },
    MouseLeave,
    Key { key: Key, mods: Mods, pressed: bool },
    Char(char),
    Focus(bool),
    ThemeChanged { dark: bool },
    DroppedFiles(Vec<FsPath>),
    CloseRequested,
    ShellVerb(String),
    MenuCommand(usize),
    DevicesChanged,
    Timer,
    ClipboardChanged(u32),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CursorIcon {
    Default,
    Text,
    Hand,
    ResizeHorizontal,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HitArea {
    Client,
    Caption,
    Minimize,
    Maximize,
    Close,
}
