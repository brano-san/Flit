use super::palette::{Palette, Pick};
use super::settings::{self, Picker, Value, SETTINGS};
use super::settings_page::{wrap, SettingPick};
use super::widgets::{baseline, G_CHECK_MARK, G_CROSS};
use super::{centered, App, Effect, SettingsCmd};
use crate::i18n::{P, S};
use crate::location::Location;
use crate::path::FsPath;
use crate::render::{Color, Rect};
use crate::text::{Gfx, TextBackend};
use crate::theme::{Theme, Tint, PRESETS};

pub(super) const BROWSE: &str = "\u{1}browse";
pub(super) const BACKGROUND_IMAGE: &str = "appearance.background_image";
const INSPECTOR_FONT: &str = "appearance.inspector_font";
const CAN_BROWSE: bool = cfg!(windows);
const NOTE_LINES: usize = 4;
const NOTE_LINE_H: f32 = 20.0;
const SWATCH: f32 = 14.0;
const G_WARNING: &str = "\u{E7BA}";
const MONO_WORDS: [&str; 5] = ["mono", "consol", "code", "courier", "fixed"];
const EXAMPLE_COLOR: &str = "#3A7BD5";
const JSON: [&str; 1] = ["json"];

#[cfg(windows)]
mod example {
    pub const IMAGE: &str = r"C:\Users\you\Pictures\wallpaper.jpg";
    pub const FOLDER: &str = r"C:\Users\you\Documents";
    pub const JSON: &str = r"C:\Users\you\flit-settings.json";
    pub const FONT: &str = "Cascadia Mono";
}

#[cfg(not(windows))]
mod example {
    pub const IMAGE: &str = "/home/you/Pictures/wallpaper.jpg";
    pub const FOLDER: &str = "/home/you/Documents";
    pub const JSON: &str = "/home/you/flit-settings.json";
    pub const FONT: &str = "DejaVu Sans Mono";
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Tone {
    Plain,
    Good,
    Warn,
    Bad,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Note {
    pub text: String,
    pub tone: Tone,
    pub swatch: Option<Color>,
}

impl Note {
    fn plain(text: impl Into<String>) -> Self {
        Self { text: text.into(), tone: Tone::Plain, swatch: None }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Editor {
    Color(usize),
    Image(usize),
    Font(usize),
    Text(usize),
    Folder(usize),
    Export,
    Import,
}

fn editor(pick: &SettingPick) -> Option<Editor> {
    Some(match *pick {
        SettingPick::Color(i) => Editor::Color(i),
        SettingPick::Text(i) if SETTINGS[i].id == BACKGROUND_IMAGE => Editor::Image(i),
        SettingPick::Text(i) if SETTINGS[i].id == INSPECTOR_FONT => Editor::Font(i),
        SettingPick::Text(i) => Editor::Text(i),
        SettingPick::AddPath(i) => Editor::Folder(i),
        SettingPick::Export => Editor::Export,
        SettingPick::Import => Editor::Import,
        _ => return None,
    })
}

fn example(e: Editor) -> Option<&'static str> {
    Some(match e {
        Editor::Color(_) => EXAMPLE_COLOR,
        Editor::Image(_) => example::IMAGE,
        Editor::Font(_) => example::FONT,
        Editor::Folder(_) => example::FOLDER,
        Editor::Export | Editor::Import => example::JSON,
        Editor::Text(_) => return None,
    })
}

fn extension(p: &FsPath) -> String {
    let name = p.file_name();
    crate::name::display(&name[name.len() - crate::name::ext_len(name)..]).to_ascii_lowercase()
}

fn is_image(p: &FsPath) -> bool {
    crate::preview::IMAGES.contains(&extension(p).as_str())
}

fn monospaced_first(fonts: &[String]) -> Vec<String> {
    let mono = |f: &String| MONO_WORDS.iter().any(|w| f.to_ascii_lowercase().contains(w));
    let mut out: Vec<String> = fonts.iter().filter(|f| mono(f)).cloned().collect();
    out.extend(fonts.iter().filter(|f| !mono(f)).cloned());
    out
}

impl App {
    pub(super) fn open_setting_editor(&mut self, pick: SettingPick, text: &str) {
        let (rows, values): (Vec<(String, String)>, Vec<String>) = self.suggestions(&pick).into_iter().map(|(title, aside, value)| ((title, aside), value)).unzip();
        let placeholder = editor(&pick).and_then(example);
        let mut p = Palette::new(Pick::Setting(pick), rows, "");
        p.set_descs(values);
        p.field.edit.set(text);
        p.field.edit.select_all();
        p.placeholder = placeholder.map(|e| crate::tr!(S::SettingsPageGuideExample, example = e));
        self.open_pick(p);
    }

    pub(super) fn edit_text_setting(&mut self, i: usize) {
        if SETTINGS[i].id == INSPECTOR_FONT && self.look.fonts.is_none() {
            self.look.font_pick = Some(i);
            self.effects.push(Effect::Settings(SettingsCmd::Fonts));
            return;
        }
        let text = match SETTINGS[i].get(self) {
            Value::Text(t) => t,
            _ => String::new(),
        };
        self.open_setting_editor(SettingPick::Text(i), &text);
    }

    fn suggestions(&self, pick: &SettingPick) -> Vec<(String, String, String)> {
        let row = |title: &str, aside: String, value: String| (title.to_owned(), aside, value);
        let browse = |title: S| CAN_BROWSE.then(|| row(title.t(), String::new(), BROWSE.to_owned()));
        match editor(pick) {
            Some(Editor::Color(_)) => std::iter::once(row(S::AppearanceDefault.t(), String::new(), String::new()))
                .chain(PRESETS.iter().enumerate().map(|(k, p)| row(p.label.t(), Tint::Preset(k).color(self.dark).map_or_else(String::new, |c| format!("#{:06X}", c.0 & 0xFF_FFFF)), p.id.to_owned())))
                .collect(),
            Some(Editor::Image(_)) => {
                let selected = self.focused_file().filter(is_image).map(|p| row(S::SettingsPageRowSelected.t(), String::from_utf8_lossy(p.file_name()).into_owned(), p.display().into_owned()));
                browse(S::SettingsPageRowBrowse).into_iter().chain(selected).chain(Some(row(S::SettingsPageGuideNoImage.t(), String::new(), String::new()))).collect()
            }
            Some(Editor::Font(_)) => {
                let fonts = self.look.fonts.as_deref().map(monospaced_first).unwrap_or_default();
                fonts.into_iter().map(|f| (f.clone(), String::new(), f)).collect()
            }
            Some(Editor::Folder(i)) => {
                let listed = self.paths_of(i);
                let mut seen: Vec<Location> = Vec::new();
                let mut out: Vec<(String, String, String)> = browse(S::SettingsPageRowBrowseFolder).into_iter().collect();
                let home = std::iter::once((S::SettingsPageRowHome.t(), Location::home_page()));
                let here = std::iter::once((S::SettingsPageRowCurrentFolder.t(), self.current_location().clone()));
                let tabs = self.tabs.iter().map(|t| (S::SettingsPageRowOpenTab.t(), t.location().clone()));
                for (title, loc) in home.chain(here).chain(tabs) {
                    if listed.contains(&loc) || seen.contains(&loc) {
                        continue;
                    }
                    let text = settings::path_text(&loc);
                    out.push(row(title, text.clone(), text));
                    seen.push(loc);
                }
                out
            }
            Some(Editor::Import) => {
                let file = self.focused_file().filter(|p| extension(p) == "json").map(|p| row(S::SettingsPageRowSelected.t(), String::from_utf8_lossy(p.file_name()).into_owned(), p.display().into_owned()));
                browse(S::SettingsPageRowBrowse).into_iter().chain(file).chain(self.export_suggestion()).collect()
            }
            Some(Editor::Export) => self.export_suggestion().into_iter().collect(),
            Some(Editor::Text(_)) | None => Vec::new(),
        }
    }

    fn export_suggestion(&self) -> Option<(String, String, String)> {
        let path = self.export_path()?;
        Some((S::SettingsPageRowSuggested.t().to_owned(), path.clone(), path))
    }

    fn focused_file(&self) -> Option<FsPath> {
        let t = self.tab();
        let i = t.view.as_ref()?.focus?;
        t.entry_location(i).to_windows().filter(|p| self.caps.is_native(p))
    }

    pub(super) fn setting_notes(&self, pick: &SettingPick, text: &str) -> Vec<Note> {
        let Some(e) = editor(pick) else { return Vec::new() };
        let (tone, verdict, swatch) = self.verdict(e, text.trim());
        vec![Note::plain(self.expectation(e)), self.current_note(e), Note { text: verdict, tone, swatch }]
    }

    pub(super) fn setting_accepts(&self, pick: &SettingPick, text: &str) -> bool {
        text == BROWSE || editor(pick).is_none_or(|e| self.verdict(e, text.trim()).0 != Tone::Bad)
    }

    pub(super) fn note_lines(pick: &Pick) -> usize {
        match pick {
            Pick::Setting(p) if editor(p).is_some() => NOTE_LINES,
            _ => 0,
        }
    }

    pub(super) fn editor_swatch(&self, pick: &Pick, value: &str) -> Option<Option<Color>> {
        match pick {
            Pick::Setting(SettingPick::Color(_)) => Some(Tint::parse(value).and_then(|t| t.color(self.dark))),
            _ => None,
        }
    }

    fn expectation(&self, e: Editor) -> String {
        match e {
            Editor::Color(_) => crate::tr!(S::SettingsPageGuideColor, example = EXAMPLE_COLOR),
            Editor::Image(_) => crate::tr!(S::SettingsPageGuideImage, example = example::IMAGE),
            Editor::Font(_) => crate::tr!(S::SettingsPageGuideFont, example = example::FONT, default = super::options::INSPECTOR_FONT),
            Editor::Text(i) => SETTINGS[i].help.t().to_owned(),
            Editor::Folder(_) => crate::tr!(S::SettingsPageGuideFolder, example = example::FOLDER),
            Editor::Export => crate::tr!(S::SettingsPageGuideExport, example = example::JSON),
            Editor::Import => crate::tr!(S::SettingsPageGuideImport, example = example::JSON),
        }
    }

    fn current_note(&self, e: Editor) -> Note {
        let (value, swatch) = match e {
            Editor::Color(i) => (self.pick_text(i).unwrap_or_default(), self.swatch(i)),
            Editor::Image(i) | Editor::Font(i) | Editor::Text(i) => (self.text_value(i), None),
            Editor::Folder(i) => (crate::trn!(P::SettingsPagePages, self.paths_of(i).len()), None),
            Editor::Export | Editor::Import => (self.about.file.as_ref().map_or_else(|| S::SettingsPageLineNone.t().to_owned(), |f| f.display().into_owned()), None),
        };
        Note { text: crate::tr!(S::SettingsPageGuideCurrent, value = value), tone: Tone::Plain, swatch }
    }

    pub(super) fn text_value(&self, i: usize) -> String {
        let text = match SETTINGS[i].get(self) {
            Value::Text(t) => t,
            _ => String::new(),
        };
        match () {
            _ if text.is_empty() => S::SettingsPageGuideEmpty.t().to_owned(),
            _ if SETTINGS[i].id == BACKGROUND_IMAGE && self.look.backdrop_failed => crate::tr!(S::SettingsPageGuideUnreadable, path = text),
            _ => text,
        }
    }

    fn verdict(&self, e: Editor, text: &str) -> (Tone, String, Option<Color>) {
        let good = |s: String| (Tone::Good, s, None);
        let bad = |s: String| (Tone::Bad, s, None);
        match e {
            Editor::Color(_) => match Tint::parse(text) {
                Some(Tint::Default) => good(crate::tr!(S::SettingsPageGuidePreview, color = S::AppearanceDefault.t())),
                Some(t) => (Tone::Good, crate::tr!(S::SettingsPageGuidePreview, color = t.label()), t.color(self.dark)),
                None => bad(S::SettingsPageStatusBadColor.t().to_owned()),
            },
            Editor::Image(_) if text.is_empty() => good(S::SettingsPageGuideNoImage.t().to_owned()),
            Editor::Image(_) => match self.caps.native_path(text) {
                None => bad(crate::tr!(S::SettingsPageGuideFullPath, example = example::IMAGE)),
                Some(p) if !is_image(&p) => bad(S::SettingsPageGuideImageType.t().to_owned()),
                Some(_) => good(S::SettingsPageGuideImageOk.t().to_owned()),
            },
            Editor::Font(_) if text.is_empty() => good(crate::tr!(S::SettingsPageGuideFontDefault, default = super::options::INSPECTOR_FONT)),
            Editor::Font(_) if !Picker::Font.accepts(text) => bad(S::SettingsPageGuideFontBad.t().to_owned()),
            Editor::Font(_) => match &self.look.fonts {
                Some(fonts) if !fonts.iter().any(|f| f.eq_ignore_ascii_case(text)) => (Tone::Warn, crate::tr!(S::SettingsPageGuideFontMissing, default = super::options::INSPECTOR_FONT), None),
                Some(_) => good(S::SettingsPageGuideFontOk.t().to_owned()),
                None => good(S::SettingsPageHintText.t().to_owned()),
            },
            Editor::Text(_) => good(S::SettingsPageHintText.t().to_owned()),
            Editor::Folder(i) => match settings::parse_path(text) {
                None => bad(S::SettingsPageStatusBadFolder.t().to_owned()),
                Some(l) if self.paths_of(i).contains(&l) => bad(S::SettingsPageStatusPageListed.t().to_owned()),
                Some(l) => good(crate::tr!(S::SettingsPageGuideFolderOk, path = settings::path_text(&l))),
            },
            Editor::Export | Editor::Import => match self.caps.native_path(text) {
                None => bad(crate::tr!(S::SettingsPageGuideFullPath, example = example::JSON)),
                Some(p) if extension(&p) != "json" => (Tone::Warn, S::SettingsPageGuideNotJson.t().to_owned(), None),
                Some(_) if e == Editor::Export => good(S::SettingsPageHintExport.t().to_owned()),
                Some(_) => good(S::SettingsPageHintImport.t().to_owned()),
            },
        }
    }

    pub(super) fn browse_for(&mut self, pick: SettingPick) {
        let (exts, folder): (&'static [&'static str], bool) = match editor(&pick) {
            Some(Editor::Image(_)) => (crate::preview::IMAGES, false),
            Some(Editor::Import) => (&JSON, false),
            Some(Editor::Folder(_)) => (&[], true),
            _ => return,
        };
        let start = match editor(&pick) {
            Some(Editor::Image(_)) => FsPath::parse(&self.look.backdrop_path).filter(|_| !self.look.backdrop_path.is_empty()).and_then(|p| p.parent()),
            _ => None,
        };
        let start = start.or_else(|| self.current_location().to_windows().filter(|p| self.caps.is_native(p)));
        if let Some(ui) = &mut self.settings {
            ui.browsing = Some(pick);
            self.effects.push(Effect::Settings(SettingsCmd::Browse { exts, folder, start }));
        }
    }

    pub fn on_browsed(&mut self, path: Option<FsPath>) {
        let Some(pick) = self.settings.as_mut().and_then(|ui| ui.browsing.take()) else { return };
        let Some(path) = path else { return };
        let text = path.display().into_owned();
        match self.setting_accepts(&pick, &text) {
            true => self.setting_input(pick, &text),
            false => self.open_setting_editor(pick, &text),
        }
        self.dirty = true;
    }

    pub(super) fn note_h(&self) -> f32 {
        let small = self.settings.as_ref().map_or_else(|| (12.0 * 1.35 * self.scale).ceil(), |ui| ui.fit.small);
        self.s(NOTE_LINE_H).max(small + self.s(4.0))
    }

    pub(super) fn paint_swatch<B: TextBackend>(&self, g: &mut Gfx<B>, t: &Theme, r: Rect, color: Option<Color>) -> f32 {
        let side = self.s(SWATCH);
        let sw = Rect::new(r.x, (r.y + (r.h - side) / 2.0).round(), side, side);
        let radius = self.s(3.0);
        if let Some(c) = color {
            g.dl.round_rect(sw, c, radius);
        }
        g.dl.outline(sw, t.input_border, radius, 1.0);
        side + self.s(8.0)
    }

    pub(super) fn draw_notes<B: TextBackend>(&self, g: &mut Gfx<B>, t: &Theme, p: &Palette, band: Rect) {
        let Pick::Setting(sp) = &p.pick else { return };
        let text = p.chosen(p.sel).filter(|v| *v != BROWSE).unwrap_or(p.field.text());
        let notes = self.setting_notes(sp, text);
        let Some((expect, rest)) = notes.split_first() else { return };
        let (font, icons, lh) = (self.ui_font(12.0), self.icon_font(10.0), self.note_h());
        let (x0, right) = (band.x + self.s(16.0), band.right() - self.s(16.0));
        let row = |k: usize| Rect::new(x0, band.y + k as f32 * lh, right - x0, lh);
        let first = wrap(g, &expect.text, font, right - x0).first().copied().unwrap_or(&expect.text[..0]);
        let end = (first.as_ptr() as usize - expect.text.as_ptr() as usize + first.len()).min(expect.text.len());
        let tail = expect.text[end..].trim_start();
        for (k, line) in [first, tail].into_iter().enumerate() {
            let r = row(k);
            let y = baseline(g, r, font);
            g.text(r.x, y, line, font, t.text_dim, r.w);
        }
        for (k, n) in rest.iter().enumerate() {
            let r = row(k + 2);
            let mut x = r.x;
            let mark = match n.tone {
                Tone::Good => Some((G_CHECK_MARK, t.accent)),
                Tone::Warn => Some((G_WARNING, t.text)),
                Tone::Bad => Some((G_CROSS, t.error)),
                Tone::Plain => None,
            };
            if let Some((glyph, ink)) = mark {
                centered(g, Rect::new(x, r.y, self.s(SWATCH), r.h), glyph, icons, ink);
                x += self.s(SWATCH) + self.s(6.0);
            }
            if n.swatch.is_some() {
                x += self.paint_swatch(g, t, Rect::new(x, r.y, 0.0, r.h), n.swatch);
            }
            let ink = match n.tone {
                Tone::Plain => t.text_dim,
                Tone::Bad => t.error,
                Tone::Good | Tone::Warn => t.text,
            };
            let y = baseline(g, r, font);
            g.text(x, y, &n.text, font, ink, (right - x).max(0.0));
        }
    }

    pub(super) fn export_path(&self) -> Option<String> {
        let here = self.tab().view_loc.to_windows().filter(|p| self.caps.is_native(p))?;
        Some(here.join(super::settings_page::EXPORT_NAME.as_bytes()).display().into_owned())
    }
}

#[cfg(test)]
#[path = "../../test/ui/setting_editor.rs"]
mod tests;
