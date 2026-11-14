use super::menu::Menu;
use super::ops::Then;
use super::App;
use crate::i18n::S;
use crate::install;
use crate::path::FsPath;
use crate::preview::{self, Kind};
use crate::provider::{FileOp, Launch};
use crate::undo::Origin;

const PRINTABLE: [&[u8]; 9] = [b"pdf", b"ps", b"txt", b"png", b"jpg", b"jpeg", b"gif", b"tif", b"tiff"];
const DISK_IMAGES: [&[u8]; 2] = [b"iso", b"img"];

fn is_image(p: &FsPath) -> bool {
    let name = p.file_name();
    preview::kind(&name[name.len() - crate::name::ext_len(name)..]) == Kind::Image
}

fn ext_in(name: &[u8], list: &[&[u8]]) -> bool {
    let ext = name[name.len() - crate::name::ext_len(name)..].to_ascii_lowercase();
    list.iter().any(|e| ext.as_slice() == *e)
}

pub(super) fn printable(name: &[u8]) -> bool {
    ext_in(name, &PRINTABLE)
}

pub(super) fn disk_image(name: &[u8]) -> bool {
    ext_in(name, &DISK_IMAGES)
}

impl App {
    fn selected_file_items(&self) -> Vec<u32> {
        let Some(v) = &self.tab().view else { return Vec::new() };
        self.selected_items().into_iter().filter(|&i| !v.listing().entries[i as usize].is_dir()).collect()
    }

    fn selected_files(&self) -> Vec<FsPath> {
        self.item_paths(&self.selected_file_items()).unwrap_or_default()
    }

    pub(super) fn install_fonts_selection(&mut self) {
        let fonts: Vec<FsPath> = self.selected_files().into_iter().filter(|p| install::is_font(p.file_name())).collect();
        if fonts.is_empty() {
            return self.status(S::OpsExtraSelectFonts.t());
        }
        self.start_op(FileOp::InstallFonts(fonts), Origin::Fresh, Then::Nothing);
    }

    pub(super) fn print_selection(&mut self) {
        let files = self.selected_files();
        if files.is_empty() || !files.iter().all(|p| printable(p.file_name())) {
            return self.status(S::OpsExtraSelectPrintable.t());
        }
        self.launch(Launch::Print(files));
    }

    pub(super) fn mount_image_selection(&mut self) {
        match self.selected_files().as_slice() {
            [one] if disk_image(one.file_name()) => self.launch(Launch::Mount(one.clone())),
            _ => self.status(S::OpsExtraSelectDiskImage.t()),
        }
    }

    pub(super) fn open_file_manager(&mut self) {
        match self.current_location().to_windows().filter(|p| p.kind() == crate::path::PathKind::Unix) {
            Some(dir) => self.launch(Launch::FileManager(dir)),
            None => self.status(S::UiOpsFolderNotShell.t()),
        }
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
        let items = self.selected_file_items();
        let Some(v) = &self.tab().view else { return };
        let print = !items.is_empty() && items.len() == v.selected_count() && items.iter().all(|&i| printable(v.listing().name(i)));
        if items.len() != 1 && !print && !items.iter().all(|&i| install::is_font(v.listing().name(i))) {
            return;
        }
        let files = self.item_paths(&items).unwrap_or_default();
        if !files.is_empty() && files.iter().all(|p| install::is_font(p.file_name())) {
            m.action("file.install_fonts", S::OpsExtraInstallFonts.t());
        }
        if print {
            m.action("file.print", S::ActionsFilePrint.t());
        }
        if let [one] = files.as_slice() {
            if is_image(one) {
                m.action("file.lock_screen", S::OpsExtraLockScreen.t());
                m.action("file.set_backdrop", S::OpsExtraSetBackdrop.t());
            }
            if disk_image(one.file_name()) {
                m.action("file.mount_image", S::ActionsFileMountImage.t());
            }
        }
    }
}
