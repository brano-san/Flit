use crate::path::{FsPath, PathKind};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Caps {
    pub unix_paths: bool,
}

impl Caps {
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
