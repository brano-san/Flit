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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Preedit {
    pub text: String,
    pub cursor: Option<usize>,
}

impl Preedit {
    pub fn new(text: String, cursor: Option<usize>) -> Self {
        let cursor = cursor.filter(|&c| text.is_char_boundary(c));
        Self { text, cursor }
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn caret(&self) -> usize {
        self.cursor.unwrap_or(self.text.len())
    }
}

impl From<&str> for Preedit {
    fn from(text: &str) -> Self {
        Self::new(text.to_owned(), None)
    }
}

impl From<String> for Preedit {
    fn from(text: String) -> Self {
        Self::new(text, None)
    }
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
    Preedit(Preedit),
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
    ShowMenu { items: Vec<crate::ui::MenuItem>, x: f32, y: f32, launches: Vec<crate::provider::Launch>, kind: crate::ui::MenuKind, shell: Vec<u32> },
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
