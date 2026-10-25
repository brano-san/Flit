use super::palette::{Palette, Pick};
use super::sidebar::Places;
use super::{App, Effect};
use crate::library::{self, LibraryOp};
use crate::location::Location;
use crate::path::{FsPath, PathKind};
use std::time::Instant;

const ADD_FOLDER: &str = "Add a folder…";

impl App {
    pub(super) fn library_save(&self, lib: &FsPath) -> Option<FsPath> {
        let l = self.nav.sidebar.library(lib)?;
        l.save.clone().or_else(|| l.folders.first().cloned())
    }

    pub(super) fn library_crumbs(&mut self, k: usize) {
        let loc = self.tabs[k].location().clone();
        if !library::is_library(loc.path()) {
            return;
        }
        let name = self.nav.sidebar.library(loc.path()).map_or_else(|| library::stem(loc.path()), |l| l.name.clone());
        let mut crumbs: Vec<(String, Location)> = loc.parent().map(|p| ("Libraries".to_owned(), p)).into_iter().collect();
        crumbs.push((name, loc));
        self.tabs[k].crumbs = crumbs;
    }

    pub(super) fn places_changed(&mut self, old: &Places, now: Instant) {
        let new = self.nav.sidebar.places.clone();
        let folders = |p: &Places, path: &FsPath| p.libraries.iter().find(|l| l.path == *path).map(|l| (l.folders.clone(), l.name.clone()));
        for k in 0..self.tabs.len() {
            let path = self.tabs[k].location().path().clone();
            if !library::is_library(&path) {
                continue;
            }
            let (before, after) = (folders(old, &path), folders(&new, &path));
            if before.as_ref().map(|b| &b.0) != after.as_ref().map(|a| &a.0) && before.is_some() {
                self.reload(k, now);
            } else if before != after {
                self.library_crumbs(k);
            }
        }
        if let Some(Pick::Library(lib)) = self.nav.palette.as_ref().map(|p| p.pick.clone()) {
            let sel = self.nav.palette.as_ref().map_or(0, |p| p.sel);
            self.nav.palette = Some(self.library_palette(lib));
            if let Some(p) = &mut self.nav.palette {
                p.sel = sel.min(p.matches.len().saturating_sub(1));
            }
        }
    }

    pub(super) fn ask_new_library(&mut self) {
        self.open_pick(Palette::new(Pick::NewLibrary, Vec::new(), ""));
    }

    pub(super) fn current_library(&self) -> Option<FsPath> {
        let p = self.current_location().path();
        library::is_library(p).then(|| p.clone())
    }

    pub(super) fn library_dialog(&mut self, lib: FsPath) {
        if self.nav.sidebar.library(&lib).is_none() {
            return self.status("This library is not loaded yet");
        }
        let p = self.library_palette(lib);
        self.open_pick(p);
    }

    fn library_palette(&self, lib: FsPath) -> Palette {
        let rows = self.nav.sidebar.library(&lib).map(|l| l.folders.iter().map(|f| (f.display().into_owned(), if l.save.as_ref() == Some(f) { "Default save location".to_owned() } else { String::new() })).collect::<Vec<_>>()).unwrap_or_default();
        let mut items = rows;
        items.push((ADD_FOLDER.to_owned(), String::new()));
        Palette::new(Pick::Library(lib), items, "")
    }

    pub(super) fn library_row(&mut self, lib: FsPath, title: &str, remove: bool) {
        if title == ADD_FOLDER {
            return if remove { self.library_dialog(lib) } else { self.open_pick(Palette::new(Pick::LibraryAdd(lib), Vec::new(), "")) };
        }
        let Some(folder) = FsPath::parse(title) else { return };
        let op = if remove { LibraryOp::Remove { library: lib, folder } } else { LibraryOp::SetSave { library: lib, folder } };
        self.effects.push(Effect::Library(op));
    }

    pub(super) fn submit_library(&mut self, pick: Pick, text: &str) {
        match pick {
            Pick::NewLibrary => match super::validate_name(text, true) {
                Ok(name) => self.effects.push(Effect::Library(LibraryOp::Create(name))),
                Err(e) => self.status(e),
            },
            Pick::LibraryAdd(library) => match FsPath::parse(text.trim_matches('"')).filter(|p| matches!(p.kind(), PathKind::Drive | PathKind::Unc | PathKind::Verbatim)) {
                Some(folder) => self.effects.push(Effect::Library(LibraryOp::Add { library, folder })),
                None => self.status("Enter the full path of the folder to include in the library"),
            },
            _ => {}
        }
    }

    pub fn library_failed(&mut self, message: &str) {
        self.status(format!("The library was not changed: {message}"));
    }
}

#[cfg(test)]
#[path = "../../test/ui/libraries.rs"]
mod tests;
