use crate::path::FsPath;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Back,
    Forward,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct Mods {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
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
    Insert,
    Tab,
    Space,
    F(u8),
    Char(char),
    NumPlus,
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
    Pointer { touch: bool },
    MouseUp { x: f32, y: f32, button: MouseButton },
    Wheel { dx: f32, dy: f32 },
    MouseLeave,
    Key { key: Key, mods: Mods, pressed: bool },
    Char(char),
    Pasted(String),
    Focus(bool),
    ThemeChanged { dark: bool },
    Contrast(Option<crate::theme::SystemColors>),
    Invoke(u64),
    A11yFocus(u64),
    A11ySelect(u64),
    DroppedFiles(Vec<FsPath>),
    CloseRequested,
    ShellVerb(String),
    MenuCommand(usize),
    DevicesChanged,
    Timer,
    ClipboardChanged(u32),
    SessionEnding,
    Hosted(crate::preview::HostEvent),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CursorIcon {
    Default,
    Text,
    Hand,
    ResizeHorizontal,
    ResizeVertical,
    Scroll,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HitArea {
    Client,
    Caption,
    Minimize,
    Maximize,
    Close,
}
