use crate::path::{FsPath, PathKind};

pub const WINDOWS_ONLY_ACTIONS: &[&str] = &[
    "file.shortcut",
    "file.new_stream",
    "view.streams",
    "file.folder_icon",
    "file.folder_icon_reset",
    "libraries.new",
    "libraries.restore",
    "library.locations",
    "app.terminal_admin",
    "file.share",
    "file.install_fonts",
    "file.lock_screen",
    "item.shell_properties",
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Caps {
    pub unix_paths: bool,
}

impl Caps {
    pub fn supports(&self, action: &str) -> bool {
        !self.unix_paths || !WINDOWS_ONLY_ACTIONS.contains(&action)
    }

    pub fn is_native(&self, p: &FsPath) -> bool {
        match p.kind() {
            PathKind::Unix => self.unix_paths,
            PathKind::Drive | PathKind::Unc | PathKind::Verbatim => !self.unix_paths,
            PathKind::Computer | PathKind::RecycleBin | PathKind::Home => false,
        }
    }

    pub fn native_path(&self, text: &str) -> Option<FsPath> {
        FsPath::parse(text).filter(|p| self.is_native(p))
    }
}

#[cfg(test)]
#[path = "../test/caps.rs"]
mod tests;
