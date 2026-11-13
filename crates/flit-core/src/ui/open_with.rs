use super::palette::{Palette, Pick};
use super::App;
use crate::i18n::S;
use crate::path::FsPath;
use crate::provider::{AppChoice, Launch};

const REMEMBER: usize = 0;
const CUSTOM: usize = 1;
const FIRST_APP: usize = 2;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Chooser {
    pub files: Vec<FsPath>,
    pub mime: String,
    pub apps: Vec<AppChoice>,
    pub remember: bool,
}

impl Chooser {
    pub(super) fn new(files: Vec<FsPath>, mime: String, mut apps: Vec<AppChoice>) -> Self {
        apps.sort_by_cached_key(|a| (a.name.to_lowercase(), a.program.as_bytes().to_vec()));
        apps.dedup_by(|a, b| a.program == b.program);
        Self { files, mime, apps, remember: false }
    }

    pub(super) fn rows(&self) -> Vec<(String, String)> {
        let state = if self.remember { S::OpenWithOn.t() } else { S::OpenWithOff.t() };
        let fixed = [(crate::tr!(S::OpenWithRemember, kind = self.mime), state.to_owned()), (S::OpenWithCustom.t().to_owned(), String::new())];
        fixed.into_iter().chain(self.apps.iter().map(|a| (a.name.clone(), String::new()))).collect()
    }

    pub(super) fn launches(&self, row: usize) -> Vec<Launch> {
        let Some(app) = row.checked_sub(FIRST_APP).and_then(|k| self.apps.get(k)) else { return Vec::new() };
        let default = self.remember.then(|| Launch::SetDefault { program: app.program.clone(), mime: self.mime.clone() });
        default.into_iter().chain([Launch::With { program: app.program.clone(), args: self.files.clone() }]).collect()
    }

    pub(super) fn custom(&self, command: &str) -> Option<Launch> {
        let command = command.trim();
        (!command.is_empty()).then(|| Launch::Custom { command: command.to_owned(), files: self.files.clone(), mime: self.mime.clone(), remember: self.remember })
    }
}

impl App {
    pub(super) fn choose_app(&mut self, files: Vec<FsPath>, mime: String, apps: Vec<AppChoice>) {
        self.show_chooser(Chooser::new(files, mime, apps), "", 0);
    }

    fn show_chooser(&mut self, c: Chooser, text: &str, row: usize) {
        let rows = c.rows();
        let mut p = Palette::new(Pick::OpenWith(Box::new(c)), rows, text);
        p.select_row(row);
        self.nav.palette = None;
        self.open_pick(p);
    }

    pub(super) fn chooser_row(&mut self, c: Chooser, id: usize, row: usize, text: &str) {
        match id {
            REMEMBER => {
                let flipped = Chooser { remember: !c.remember, ..c };
                self.show_chooser(flipped, text, row);
            }
            CUSTOM => self.open_pick(Palette::new(Pick::OpenWithCommand(Box::new(c)), Vec::new(), "")),
            _ => c.launches(id).into_iter().for_each(|l| self.launch(l)),
        }
    }

    pub(super) fn chooser_command(&mut self, c: Chooser, text: &str) {
        match c.custom(text) {
            Some(l) => self.launch(l),
            None => self.open_pick(Palette::new(Pick::OpenWithCommand(Box::new(c)), Vec::new(), "")),
        }
    }
}

#[cfg(test)]
#[path = "../../test/ui/open_with.rs"]
mod tests;
