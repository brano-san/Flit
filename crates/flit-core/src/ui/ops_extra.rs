use super::menu::Menu;
use super::ops::Then;
use super::App;
use crate::i18n::S;
use crate::install;
use crate::path::FsPath;
use crate::preview::{self, Kind};
use crate::provider::FileOp;
use crate::undo::Origin;

fn is_image(p: &FsPath) -> bool {
    let name = p.file_name();
    preview::kind(&name[name.len() - crate::name::ext_len(name)..]) == Kind::Image
}

impl App {
    fn selected_files(&self) -> Vec<FsPath> {
        let Some(v) = &self.tab().view else { return Vec::new() };
        let files: Vec<u32> = self.selected_items().into_iter().filter(|&i| !v.listing().entries[i as usize].is_dir()).collect();
        self.item_paths(&files).unwrap_or_default()
    }

    pub(super) fn install_fonts_selection(&mut self) {
        let fonts: Vec<FsPath> = self.selected_files().into_iter().filter(|p| install::is_font(p.file_name())).collect();
        if fonts.is_empty() {
            return self.status(S::OpsExtraSelectFonts.t());
        }
        self.start_op(FileOp::InstallFonts(fonts), Origin::Fresh, Then::Nothing);
    }

    pub(super) fn lock_screen_selection(&mut self) {
        match self.selected_files().as_slice() {
            [one] if is_image(one) => self.start_op(FileOp::LockScreen(one.clone()), Origin::Fresh, Then::Nothing),
            _ => self.status(S::OpsExtraSelectImage.t()),
        }
    }

    pub(super) fn backdrop_selection(&mut self) {
        match self.selected_files().as_slice() {
            [one] if is_image(one) => self.set_backdrop(one.display().into_owned()),
            _ => self.status(S::OpsExtraSelectBackdrop.t()),
        }
    }

    pub(super) fn media_entries(&self, m: &mut Menu) {
        let files = self.selected_files();
        if !files.is_empty() && files.iter().all(|p| install::is_font(p.file_name())) {
            m.action("file.install_fonts", S::OpsExtraInstallFonts.t());
        }
        if let [one] = files.as_slice() {
            if is_image(one) {
                m.action("file.lock_screen", S::OpsExtraLockScreen.t());
                m.action("file.set_backdrop", S::OpsExtraSetBackdrop.t());
            }
        }
    }
}
