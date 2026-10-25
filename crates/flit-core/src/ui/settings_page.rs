use super::actions::{chord_text, Chord, ACTIONS};
use super::edit::EditAction;
use super::field::{Field, FieldKey};
use super::menu::Cmd;
use super::palette::{Palette, Pick};
use super::settings::{self, Kind, Page, Picker, Value, SETTINGS};
use super::{centered, App, Effect, Hit, SettingsCmd, G_CLOSE};
use crate::input::{Key, Mods, MouseButton};
use crate::location::{Location, ProviderId};
use crate::open::{OpenTarget, Route};
use crate::path::{FsPath, PathKind};
use crate::provider::{Launch, ProviderRequest};
use crate::render::{Color, Rect};
use crate::tags;
use crate::text::{FontFamily, FontStyle, Gfx, TextBackend};
use crate::theme::Theme;
use std::time::Instant;

const MAX_W: f32 = 960.0;
const NAV_W: f32 = 210.0;
const HEAD_H: f32 = 60.0;
const PAGE_H: f32 = 36.0;
const SEARCH_H: f32 = 32.0;
const SETTING_H: f32 = 60.0;
const GROUP_H: f32 = 40.0;
const NOTE_H: f32 = 32.0;
const ITEM_H: f32 = 32.0;
const BUTTONS_H: f32 = 46.0;
const TAG_H: f32 = 40.0;
const ACTION_H: f32 = 40.0;
const CHIP_W: f32 = 124.0;
const CHIP_SMALL: f32 = 30.0;
const INFO_H: f32 = 28.0;
const CONTROL_W: f32 = 250.0;
const BUTTON_W: f32 = 190.0;
const TAG_BUTTON_W: f32 = 92.0;
const PAD: f32 = 16.0;
const EXPORT_NAME: &str = "flit-settings.json";

const G_MINUS: &str = "\u{E738}";
const G_PLUS: &str = "\u{E710}";
const G_DOWN: &str = "\u{E70D}";
const G_SEARCH: &str = "\u{E721}";
const G_UNDO: &str = "\u{E7A7}";

const DOCS_URL: &str = "https://github.com/brano-san/Flit#readme";
const BUG_URL: &str = "https://github.com/brano-san/Flit/issues/new?labels=bug";
const FEATURE_URL: &str = "https://github.com/brano-san/Flit/issues/new?labels=enhancement";
const REPOSITORY_URL: &str = "https://github.com/brano-san/Flit";
const PRIVACY_URL: &str = "https://github.com/brano-san/Flit#конфиденциальность";
pub(super) const LINKS: [(&str, &str); 5] = [("help.docs", DOCS_URL), ("help.bug", BUG_URL), ("help.feature", FEATURE_URL), ("help.repository", REPOSITORY_URL), ("help.privacy", PRIVACY_URL)];

const LIBRARIES: [(&str, &str); 20] = [
    ("arc-swap", "shared snapshots"),
    ("bumpalo", "frame memory"),
    ("crc32fast", "checksums"),
    ("crossbeam-channel", "threads"),
    ("encoding_rs", "names in old archives"),
    ("flate2", "GZip"),
    ("memchr", "search"),
    ("mimalloc", "memory allocator"),
    ("postcard", "state files"),
    ("rayon", "parallel work"),
    ("regex", "search"),
    ("serde", "serialization"),
    ("serde_json", "settings file"),
    ("sevenz-rust2", "7z archives"),
    ("smallvec", "small lists"),
    ("tar", "TAR archives"),
    ("tracing", "log file"),
    ("tracing-subscriber", "log file"),
    ("unrar-ng", "RAR archives"),
    ("zip", "ZIP archives"),
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Btn {
    AddHome,
    AddCurrent,
    AddTyped,
    NewTag,
    ClearCache,
    OpenFile,
    Export,
    Import,
    Reset,
    CopyAbout,
    CustomizeToolbar,
    Rename,
    Recolor,
    Delete,
    RestoreKeys,
    OpenLog,
    OpenLogs,
    Docs,
    ReportBug,
    RequestFeature,
    Repository,
    Privacy,
}

impl Btn {
    fn label(self) -> &'static str {
        match self {
            Btn::AddHome => "Add the Home page",
            Btn::AddCurrent => "Add the current folder",
            Btn::AddTyped => "Type a path…",
            Btn::NewTag => "New tag",
            Btn::ClearCache => "Clear the cache",
            Btn::OpenFile => "Open the settings file",
            Btn::Export => "Export settings…",
            Btn::Import => "Import settings…",
            Btn::Reset => "Reset all settings",
            Btn::CopyAbout => "Copy version info",
            Btn::CustomizeToolbar => "Customize the toolbar…",
            Btn::Rename => "Rename",
            Btn::Recolor => "Color",
            Btn::Delete => "Delete",
            Btn::RestoreKeys => "Restore all shortcuts",
            Btn::OpenLog => "Open the log file",
            Btn::OpenLogs => "Open the log folder",
            Btn::Docs => "Documentation",
            Btn::ReportBug => "Report a bug",
            Btn::RequestFeature => "Request a feature",
            Btn::Repository => "GitHub repository",
            Btn::Privacy => "Privacy statement",
        }
    }
}

const PATH_BUTTONS: &[Btn] = &[Btn::AddHome, Btn::AddCurrent, Btn::AddTyped];
const TAG_BUTTONS: &[Btn] = &[Btn::Rename, Btn::Recolor, Btn::Delete];
const FILE_BUTTONS: &[Btn] = &[Btn::OpenFile, Btn::Export, Btn::Import];
const LOG_BUTTONS: &[Btn] = &[Btn::OpenLog, Btn::OpenLogs];
const HELP_BUTTONS: &[Btn] = &[Btn::Docs, Btn::ReportBug, Btn::RequestFeature];
const LINK_BUTTONS: &[Btn] = &[Btn::Repository, Btn::Privacy];
pub const LOG_FILE: &str = "flit.log";
const NO_LOGS: &str = "Flit has no log folder in this session";
const NO_TELEMETRY: &str = "Flit has no telemetry and no user identifier: it sends nothing anywhere.";

#[derive(Clone, Debug, PartialEq)]
enum Line {
    Group(&'static str),
    Note(&'static str),
    Setting(usize),
    Path(usize, usize),
    Buttons(Option<usize>, &'static [Btn]),
    Tag(usize),
    Info(&'static str, String),
    Action(usize, usize, bool),
}

impl Line {
    fn height(&self) -> f32 {
        match self {
            Line::Group(_) => GROUP_H,
            Line::Note(_) => NOTE_H,
            Line::Setting(_) => SETTING_H,
            Line::Path(..) => ITEM_H,
            Line::Buttons(..) => BUTTONS_H,
            Line::Tag(_) => TAG_H,
            Line::Info(..) => INFO_H,
            Line::Action(..) => ACTION_H,
        }
    }

    fn count(&self) -> usize {
        match self {
            Line::Action(_, n, custom) => n + 1 + usize::from(*custom),
            _ => self.buttons().len(),
        }
    }

    fn focusable(&self) -> bool {
        !matches!(self, Line::Group(_) | Line::Note(_) | Line::Info(..))
    }

    fn buttons(&self) -> &'static [Btn] {
        match self {
            Line::Buttons(_, b) => b,
            Line::Tag(_) => TAG_BUTTONS,
            _ => &[],
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SHit {
    Outside,
    Panel,
    Close,
    Page(usize),
    Search,
    Line(usize),
    Control(usize),
    Step(usize, bool),
    Remove(usize),
    Button(usize, usize),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum SettingPick {
    Text(usize),
    AddPath(usize),
    Export,
    Import,
    Reset,
    ResetKeys,
    Capture { action: usize, old: Option<Chord>, chord: Option<Chord> },
    Color(usize),
    Font(usize),
}

impl SettingPick {
    pub(super) fn placeholder(&self) -> &'static str {
        match self {
            SettingPick::Text(_) => "New value",
            SettingPick::AddPath(_) => "Full path of a folder, or Home for the Home page",
            SettingPick::Export => "Full path of the file to export the settings and tags to",
            SettingPick::Import => "Full path of the settings file to import",
            SettingPick::Reset => "Reset all settings?",
            SettingPick::ResetKeys => "Restore the default shortcuts?",
            SettingPick::Capture { .. } => "Press the keys of the new shortcut",
            SettingPick::Color(_) => "A color as #RRGGBB, for example #1D2533",
            SettingPick::Font(_) => "Type to filter the installed fonts",
        }
    }

    pub(super) fn empty(&self) -> &'static str {
        match self {
            SettingPick::Text(_) => "Enter saves the value",
            SettingPick::AddPath(_) => "Enter adds the page",
            SettingPick::Export => "Enter writes the file",
            SettingPick::Import => "Enter reads the file and applies it",
            SettingPick::Reset | SettingPick::ResetKeys | SettingPick::Capture { .. } => "",
            SettingPick::Color(_) => "Enter applies the color",
            SettingPick::Font(_) => "No matching fonts",
        }
    }

    pub(super) fn is_input(&self) -> bool {
        !matches!(self, SettingPick::Reset | SettingPick::ResetKeys | SettingPick::Capture { .. } | SettingPick::Font(_))
    }
}

#[derive(Default, Clone, Debug, PartialEq)]
pub struct About {
    pub os: String,
    pub file: Option<FsPath>,
    pub cache_bytes: Option<u64>,
    pub logs: Option<FsPath>,
}

pub(super) struct SettingsUi {
    pub page: Page,
    search: Field,
    searching: bool,
    lines: Vec<Line>,
    rects: Vec<Rect>,
    focus: usize,
    sub: usize,
    scroll: f32,
    content_h: f32,
    reveal: bool,
    panel: Rect,
    nav: Vec<Rect>,
    search_rect: Rect,
    list: Rect,
    close: Rect,
}

impl SettingsUi {
    fn new(page: Page) -> Self {
        Self {
            page,
            search: Field::default(),
            searching: false,
            lines: Vec::new(),
            rects: Vec::new(),
            focus: 0,
            sub: 0,
            scroll: 0.0,
            content_h: 0.0,
            reveal: false,
            panel: Rect::default(),
            nav: Vec::new(),
            search_rect: Rect::default(),
            list: Rect::default(),
            close: Rect::default(),
        }
    }

    pub fn query(&self) -> &str {
        self.search.text().trim()
    }

    fn focusable(&self, from: usize, step: isize) -> Option<usize> {
        let mut k = from as isize;
        loop {
            k += step;
            let line = self.lines.get(usize::try_from(k).ok()?)?;
            if line.focusable() {
                return Some(k as usize);
            }
        }
    }

    fn first_focusable(&self) -> usize {
        self.lines.iter().position(Line::focusable).unwrap_or(0)
    }
}

fn matches(query: &str, words: &str) -> bool {
    query.to_lowercase().split_whitespace().all(|w| words.contains(w))
}

fn switch_rect(r: Rect, s: impl Fn(f32) -> f32) -> Rect {
    Rect::new(r.right() - s(PAD) - s(44.0), r.y + (r.h - s(22.0)) / 2.0, s(44.0), s(22.0))
}

fn control_rect(r: Rect, s: impl Fn(f32) -> f32) -> Rect {
    Rect::new(r.right() - s(PAD) - s(CONTROL_W), r.y + (r.h - s(30.0)) / 2.0, s(CONTROL_W), s(30.0))
}

fn step_rects(r: Rect, s: impl Fn(f32) -> f32) -> (Rect, Rect) {
    let c = control_rect(r, &s);
    (Rect::new(c.x, c.y, s(34.0), c.h), Rect::new(c.right() - s(34.0), c.y, s(34.0), c.h))
}

fn remove_rect(r: Rect, s: impl Fn(f32) -> f32) -> Rect {
    Rect::new(r.right() - s(PAD) - s(28.0), r.y + (r.h - s(24.0)) / 2.0, s(28.0), s(24.0))
}

fn button_rects(line: &Line, r: Rect, s: impl Fn(f32) -> f32) -> Vec<Rect> {
    let buttons = line.buttons();
    let h = s(30.0);
    let y = r.y + (r.h - h) / 2.0;
    match line {
        Line::Action(_, n, custom) => {
            let widths: Vec<f32> = std::iter::repeat_n(s(CHIP_W), *n).chain([s(CHIP_SMALL)]).chain(custom.then(|| s(CHIP_SMALL))).collect();
            let (gap, ch) = (s(6.0), s(26.0));
            let mut x = r.right() - s(PAD) - widths.iter().sum::<f32>() - gap * (widths.len() - 1) as f32;
            widths
                .into_iter()
                .map(|w| {
                    let chip = Rect::new(x, r.y + (r.h - ch) / 2.0, w, ch);
                    x += w + gap;
                    chip
                })
                .collect()
        }
        Line::Tag(_) => {
            let w = s(TAG_BUTTON_W);
            let n = buttons.len();
            (0..n).map(|k| Rect::new(r.right() - s(PAD) - (n - k) as f32 * (w + s(8.0)) + s(8.0), y, w, h)).collect()
        }
        _ => {
            let (w, x0) = (s(BUTTON_W), r.x + if matches!(line, Line::Buttons(Some(_), _)) { s(32.0) } else { s(PAD) });
            (0..buttons.len()).map(|k| Rect::new(x0 + k as f32 * (w + s(8.0)), y, w, h)).collect()
        }
    }
}

fn size_text(bytes: Option<u64>, decimal: bool) -> String {
    let Some(b) = bytes else { return "Not measured yet".into() };
    let mut s = String::new();
    crate::fmt::size_in(&mut s, b, decimal);
    s
}

impl App {
    pub fn settings_open(&self) -> bool {
        self.settings.is_some()
    }

    pub fn set_about(&mut self, about: About) {
        if self.about != about {
            self.about = about;
            self.layout();
            self.dirty = true;
        }
    }

    pub fn settings_page(&self) -> Option<Page> {
        self.settings.as_ref().map(|s| s.page)
    }

    pub fn settings_lines(&self) -> Vec<String> {
        let Some(ui) = &self.settings else { return Vec::new() };
        ui.lines
            .iter()
            .map(|l| match l {
                Line::Group(t) | Line::Note(t) => t.to_string(),
                Line::Setting(i) => SETTINGS[*i].id.to_owned(),
                Line::Path(i, k) => format!("{} #{k}", SETTINGS[*i].id),
                Line::Buttons(_, b) => b.iter().map(|b| b.label()).collect::<Vec<_>>().join(" | "),
                Line::Tag(k) => self.tag_defs().get(*k).map_or_else(String::new, |t| format!("tag {}", t.name)),
                Line::Info(label, value) => format!("{label}: {value}"),
                Line::Action(i, ..) => format!("action {} {}", ACTIONS[*i].id, self.keymap.text(*i)),
            })
            .collect()
    }

    pub(super) fn open_settings(&mut self, page: Option<Page>) {
        if self.settings.is_some() && page.is_none() {
            return self.close_settings();
        }
        if self.editing {
            self.cancel_edit();
        }
        self.nav.palette = None;
        let ui = self.settings.get_or_insert_with(|| SettingsUi::new(page.unwrap_or(Page::General)));
        if let Some(p) = page {
            ui.page = p;
            ui.search.edit.set("");
            ui.scroll = 0.0;
            ui.focus = usize::MAX;
        }
        self.effects.push(Effect::Settings(SettingsCmd::CacheSize));
        self.layout();
        self.dirty = true;
    }

    pub(super) fn close_settings(&mut self) {
        self.settings = None;
        self.layout();
        self.dirty = true;
    }

    fn change_setting(&mut self, i: usize, v: Value) {
        if SETTINGS.get(i).is_some_and(|s| s.set(self, v)) {
            self.layout();
            self.dirty = true;
        }
    }

    pub(super) fn choose_setting(&mut self, i: usize, choice: usize) {
        if matches!(SETTINGS[i].kind, Kind::Pick(..)) {
            if self.choose_color(i, choice) {
                self.layout();
            }
            return;
        }
        self.change_setting(i, Value::Choice(choice));
    }

    fn paths_of(&self, i: usize) -> Vec<crate::location::Location> {
        match SETTINGS[i].get(self) {
            Value::Paths(p) => p,
            _ => Vec::new(),
        }
    }

    fn add_path(&mut self, i: usize, loc: crate::location::Location) {
        let mut paths = self.paths_of(i);
        if paths.contains(&loc) {
            return self.status("This page is already in the list");
        }
        paths.push(loc);
        self.change_setting(i, Value::Paths(paths));
    }

    fn remove_path(&mut self, i: usize, k: usize) {
        let mut paths = self.paths_of(i);
        if k < paths.len() {
            paths.remove(k);
            self.change_setting(i, Value::Paths(paths));
        }
    }

    pub(super) fn open_settings_file(&mut self) {
        match self.about.file.clone() {
            Some(path) => {
                let tab = self.tab().id;
                self.effects.push(Effect::Provider { provider: ProviderId::Local, req: ProviderRequest::Open { tab, path } });
            }
            None => self.status("Flit has no settings file in this session"),
        }
    }

    pub(super) fn open_link(&mut self, id: &str) {
        if let Some((_, url)) = LINKS.iter().find(|(k, _)| *k == id) {
            self.launch(Launch::Run { program: (*url).to_owned(), args: String::new(), dir: None });
        }
    }

    pub(super) fn open_log_file(&mut self) {
        match self.about.logs.as_ref().map(|d| d.join(LOG_FILE.as_bytes())) {
            Some(path) => {
                let tab = self.tab().id;
                self.effects.push(Effect::Provider { provider: ProviderId::Local, req: ProviderRequest::Open { tab, path } });
            }
            None => self.status(NO_LOGS),
        }
    }

    pub(super) fn open_log_folder(&mut self, now: Instant) {
        let Some(dir) = self.about.logs.clone() else { return self.status(NO_LOGS) };
        if self.settings.is_some() {
            self.close_settings();
        }
        self.dispatch(vec![Route { folder: Location::from_local(dir), select: Some(LOG_FILE.as_bytes().to_vec()) }], OpenTarget::NewTab, now);
    }

    fn export_default(&self) -> String {
        let here = self.tab().view_loc.to_windows().filter(|p| matches!(p.kind(), PathKind::Drive | PathKind::Unc | PathKind::Verbatim));
        here.map(|p| p.join(EXPORT_NAME.as_bytes()).display().into_owned()).unwrap_or_default()
    }

    pub(super) fn ask_export_settings(&mut self) {
        let text = self.export_default();
        self.open_pick(Palette::new(Pick::Setting(SettingPick::Export), Vec::new(), &text));
    }

    pub(super) fn ask_import_settings(&mut self) {
        let t = self.tab();
        let focused = t.view.as_ref().and_then(|v| v.focus).map(|i| t.entry_location(i)).filter(|l| l.file_name().to_ascii_lowercase().ends_with(b".json"));
        let text = focused.and_then(|l| l.to_windows()).map(|p| p.display().into_owned()).unwrap_or_else(|| self.export_default());
        self.open_pick(Palette::new(Pick::Setting(SettingPick::Import), Vec::new(), &text));
    }

    pub(super) fn ask_reset_settings(&mut self) {
        let items = vec![("Reset all settings to their defaults".to_owned(), "Tags, pinned folders and folder views stay".to_owned()), ("Cancel".to_owned(), String::new())];
        self.open_pick(Palette::new(Pick::Setting(SettingPick::Reset), items, ""));
    }

    pub(super) fn setting_choose(&mut self, pick: SettingPick, id: usize) {
        match pick {
            SettingPick::Reset if id == 0 => self.reset_settings(),
            SettingPick::ResetKeys if id == 0 => self.restore_all_keys(),
            SettingPick::Capture { action, old, chord } => self.capture_choose(action, old, chord, id),
            SettingPick::Font(i) => self.choose_font(i, id),
            _ => {}
        }
    }

    pub(super) fn ask_reset_keys(&mut self) {
        let items = vec![("Restore the default shortcuts of all commands".to_owned(), "Shortcuts you added or changed are lost".to_owned()), ("Cancel".to_owned(), String::new())];
        self.open_pick(Palette::new(Pick::Setting(SettingPick::ResetKeys), items, ""));
    }

    fn action_line(&self, i: usize) -> Line {
        Line::Action(i, self.keymap.keys(i).len(), self.keymap.is_custom(i))
    }

    fn press_chip(&mut self, i: usize, n: usize, b: usize) {
        match self.keymap.keys(i).get(b).copied() {
            Some(c) if b < n => self.open_capture(i, Some(c)),
            _ if b == n => self.open_capture(i, None),
            _ => self.restore_keys(i),
        }
    }

    pub(super) fn setting_input(&mut self, pick: SettingPick, text: &str) {
        match pick {
            SettingPick::Text(i) => self.change_setting(i, Value::Text(text.to_owned())),
            SettingPick::Color(i) if Picker::Color.accepts(text) => self.change_setting(i, Value::Text(text.to_owned())),
            SettingPick::Color(_) => self.status("Enter a color as # and six hexadecimal digits, for example #1D2533"),
            SettingPick::AddPath(i) => match settings::parse_path(text) {
                Some(loc) => self.add_path(i, loc),
                None => self.status("Enter the full path of a folder, or Home"),
            },
            SettingPick::Export | SettingPick::Import => match FsPath::parse(text.trim_matches('"')).filter(|p| matches!(p.kind(), PathKind::Drive | PathKind::Unc | PathKind::Verbatim)) {
                Some(path) if pick == SettingPick::Export => self.effects.push(Effect::Settings(SettingsCmd::Export { path, text: self.export_text(), replace: false })),
                Some(path) => self.effects.push(Effect::Settings(SettingsCmd::Import(path))),
                None => self.status("Enter the full path of a file"),
            },
            SettingPick::Reset | SettingPick::ResetKeys | SettingPick::Capture { .. } | SettingPick::Font(_) => {}
        }
    }

    fn page_lines(&self, page: Page, out: &mut Vec<Line>) {
        for (i, _) in SETTINGS.iter().enumerate().filter(|(_, s)| s.page == page && s.id != settings::LOG_LEVEL) {
            self.setting_lines(i, out);
        }
        match page {
            Page::Appearance => out.push(Line::Buttons(None, &[Btn::CustomizeToolbar])),
            Page::Tags => {
                if self.tag_defs().is_empty() {
                    out.push(Line::Note("There are no tags yet."));
                }
                out.extend((0..self.tag_defs().len()).map(Line::Tag));
                out.push(Line::Buttons(None, &[Btn::NewTag]));
            }
            Page::Advanced => {
                out.push(Line::Info("Thumbnail cache in use", size_text(self.about.cache_bytes, self.style.decimal)));
                out.push(Line::Buttons(None, &[Btn::ClearCache]));
                out.push(Line::Group("Settings file"));
                out.push(Line::Info("File", self.about.file.as_ref().map_or_else(|| "None in this session".to_owned(), |f| f.display().into_owned())));
                out.push(Line::Buttons(None, FILE_BUTTONS));
                out.push(Line::Buttons(None, &[Btn::Reset]));
                out.push(Line::Group("Log file"));
                if let Some(i) = settings::index(settings::LOG_LEVEL) {
                    self.setting_lines(i, out);
                }
                out.push(Line::Info("Folder", self.about.logs.as_ref().map_or_else(|| "None in this session".to_owned(), |f| f.display().into_owned())));
                out.push(Line::Buttons(None, LOG_BUTTONS));
            }
            Page::About => {
                out.push(Line::Info("Version", format!("Flit {}", env!("CARGO_PKG_VERSION"))));
                out.push(Line::Info("Windows", if self.about.os.is_empty() { "Unknown".into() } else { self.about.os.clone() }));
                out.push(Line::Buttons(None, &[Btn::CopyAbout]));
                out.push(Line::Group("Help"));
                out.push(Line::Buttons(None, HELP_BUTTONS));
                out.push(Line::Buttons(None, LINK_BUTTONS));
                out.push(Line::Note(NO_TELEMETRY));
                out.push(Line::Group("Third-party libraries"));
                out.extend(LIBRARIES.iter().map(|(name, what)| Line::Info(name, (*what).to_owned())));
            }
            Page::Actions => {
                out.push(Line::Note("Enter or a click changes a shortcut, Delete removes it, + adds one, the arrow restores the defaults."));
                out.push(Line::Note("A key runs one command: taking a used key moves it, and clashes in settings.json fall back to the defaults."));
                out.push(Line::Buttons(None, &[Btn::RestoreKeys]));
                out.extend((0..ACTIONS.len()).map(|i| self.action_line(i)));
            }
            Page::General | Page::Files | Page::Layout => {}
        }
    }

    fn setting_lines(&self, i: usize, out: &mut Vec<Line>) {
        if let Kind::Keys(..) = SETTINGS[i].kind {
            return;
        }
        out.push(Line::Setting(i));
        if let Kind::Paths(..) = SETTINGS[i].kind {
            out.extend((0..self.paths_of(i).len()).map(|k| Line::Path(i, k)));
            out.push(Line::Buttons(Some(i), PATH_BUTTONS));
        }
    }

    fn search_lines(&self, query: &str, out: &mut Vec<Line>) {
        for page in Page::ALL {
            let start = out.len();
            for (i, _) in SETTINGS.iter().enumerate().filter(|(_, s)| s.page == page && matches(query, &s.words())) {
                self.setting_lines(i, out);
            }
            let extra: &[&'static [Btn]] = match page {
                Page::Appearance => &[&[Btn::CustomizeToolbar]],
                Page::Tags => &[&[Btn::NewTag]],
                Page::Advanced => &[&[Btn::ClearCache], FILE_BUTTONS, &[Btn::Reset], LOG_BUTTONS],
                Page::About => &[&[Btn::CopyAbout], HELP_BUTTONS, LINK_BUTTONS],
                Page::Actions => &[&[Btn::RestoreKeys]],
                _ => &[],
            };
            if page == Page::Actions {
                out.extend((0..ACTIONS.len()).filter(|&i| matches(query, self.keymap.words(i))).map(|i| self.action_line(i)));
            }
            for b in extra.iter().filter(|b| b.iter().any(|b| matches(query, &b.label().to_lowercase()))) {
                out.push(Line::Buttons(None, b));
            }
            if out.len() > start {
                out.insert(start, Line::Group(page.title()));
            }
        }
        if out.is_empty() {
            out.push(Line::Note("No settings match the search."));
        }
    }

    pub(super) fn layout_settings(&mut self) {
        let Some(mut ui) = self.settings.take() else { return };
        let mut lines = Vec::new();
        match ui.query() {
            "" => self.page_lines(ui.page, &mut lines),
            q => self.search_lines(q, &mut lines),
        }
        ui.lines = lines;
        let (w, h, sc) = (self.size.0, self.size.1, self.scale);
        let s = |v: f32| (v * sc).round();
        let top = self.layout.title.bottom() + s(12.0);
        let pw = s(MAX_W).min(w - s(24.0)).max(s(320.0));
        let x = ((w - pw) / 2.0).round();
        ui.panel = Rect::new(x, top, pw, (h - top - s(12.0)).max(s(200.0)));
        let p = ui.panel;
        let nav_w = s(NAV_W).min(pw * 0.3);
        ui.nav = (0..Page::ALL.len()).map(|k| Rect::new(p.x + s(8.0), p.y + s(HEAD_H) + k as f32 * s(PAGE_H), nav_w - s(16.0), s(PAGE_H))).collect();
        ui.close = Rect::new(p.right() - s(44.0), p.y + s(10.0), s(34.0), s(30.0));
        let cx = p.x + nav_w;
        ui.search_rect = Rect::new(cx + s(PAD), p.y + s(12.0), (p.right() - s(52.0) - cx - s(PAD)).max(s(80.0)), s(SEARCH_H));
        ui.list = Rect::new(cx, p.y + s(HEAD_H), p.right() - cx - s(8.0), p.bottom() - p.y - s(HEAD_H) - s(8.0));
        if ui.focus >= ui.lines.len() || !ui.lines[ui.focus].focusable() {
            ui.focus = ui.first_focusable();
            ui.sub = 0;
        }
        ui.sub = ui.sub.min(ui.lines.get(ui.focus).map_or(0, |l| l.count().saturating_sub(1)));
        let mut y = 0.0;
        let mut offsets = Vec::with_capacity(ui.lines.len());
        for l in &ui.lines {
            offsets.push((y, s(l.height())));
            y += s(l.height());
        }
        ui.content_h = y;
        if std::mem::take(&mut ui.reveal) {
            if let Some(&(fy, fh)) = offsets.get(ui.focus) {
                if fy < ui.scroll {
                    ui.scroll = fy;
                } else if fy + fh > ui.scroll + ui.list.h {
                    ui.scroll = fy + fh - ui.list.h;
                }
            }
        }
        ui.scroll = ui.scroll.clamp(0.0, (ui.content_h - ui.list.h).max(0.0));
        ui.rects = offsets.into_iter().map(|(ly, lh)| Rect::new(ui.list.x, ui.list.y + ly - ui.scroll, ui.list.w, lh)).collect();
        self.settings = Some(ui);
    }

    pub(super) fn settings_hit(&self, x: f32, y: f32) -> Option<Hit> {
        let ui = self.settings.as_ref()?;
        if self.layout.title.contains(x, y) {
            return None;
        }
        let s = |v: f32| (v * self.scale).round();
        let h = |sh| Some(Hit::Settings(sh));
        if !ui.panel.contains(x, y) {
            return h(SHit::Outside);
        }
        if ui.close.contains(x, y) {
            return h(SHit::Close);
        }
        if let Some(k) = ui.nav.iter().position(|r| r.contains(x, y)) {
            return h(SHit::Page(k));
        }
        if ui.search_rect.contains(x, y) {
            return h(SHit::Search);
        }
        if !ui.list.contains(x, y) {
            return h(SHit::Panel);
        }
        let Some(k) = ui.rects.iter().position(|r| r.contains(x, y)) else { return h(SHit::Panel) };
        let (r, line) = (ui.rects[k], &ui.lines[k]);
        if let Some(b) = button_rects(line, r, s).iter().position(|b| b.contains(x, y)) {
            return h(SHit::Button(k, b));
        }
        match line {
            Line::Setting(i) => match SETTINGS[*i].kind {
                Kind::Toggle(..) if switch_rect(r, s).contains(x, y) => h(SHit::Control(k)),
                Kind::Number { .. } => {
                    let (minus, plus) = step_rects(r, s);
                    match (minus.contains(x, y), plus.contains(x, y)) {
                        (true, _) => h(SHit::Step(k, false)),
                        (_, true) => h(SHit::Step(k, true)),
                        _ => h(SHit::Line(k)),
                    }
                }
                Kind::Choice(..) | Kind::Text(..) | Kind::Pick(..) if control_rect(r, s).contains(x, y) => h(SHit::Control(k)),
                _ => h(SHit::Line(k)),
            },
            Line::Path(..) if remove_rect(r, s).contains(x, y) => h(SHit::Remove(k)),
            _ => h(SHit::Line(k)),
        }
    }

    pub(super) fn settings_ime_area(&self) -> Option<Rect> {
        self.settings.as_ref().filter(|s| s.searching && self.nav.palette.is_none()).map(|s| s.search_rect)
    }

    pub(super) fn settings_mouse_down(&mut self, h: Hit, x: f32, button: MouseButton, clicks: u8) -> bool {
        let Hit::Settings(sh) = h else { return false };
        let Some(ui) = &mut self.settings else { return false };
        if self.nav.palette.is_some() {
            self.nav.palette = None;
        }
        self.dirty = true;
        if button != MouseButton::Left {
            return true;
        }
        self.pressed = h;
        match sh {
            SHit::Search => {
                ui.searching = true;
                ui.search.click(x, clicks);
            }
            SHit::Line(k) | SHit::Control(k) | SHit::Step(k, _) | SHit::Remove(k) => {
                ui.searching = false;
                ui.focus = k;
            }
            SHit::Button(k, b) => {
                ui.searching = false;
                (ui.focus, ui.sub) = (k, b);
            }
            _ => {}
        }
        true
    }

    pub(super) fn settings_click(&mut self, sh: SHit, x: f32, y: f32) {
        let Some(ui) = &mut self.settings else { return };
        let line = |k: usize| ui.lines.get(k).cloned();
        match sh {
            SHit::Outside | SHit::Close => self.close_settings(),
            SHit::Page(k) => {
                ui.page = Page::ALL[k];
                ui.search.edit.set("");
                ui.scroll = 0.0;
                ui.focus = usize::MAX;
                ui.searching = false;
                self.layout();
            }
            SHit::Line(k) if !ui.query().is_empty() => {
                if let Some(Line::Setting(i) | Line::Path(i, _) | Line::Buttons(Some(i), _)) = line(k) {
                    self.reveal_setting(i);
                }
            }
            SHit::Control(k) => {
                if let Some(Line::Setting(i)) = line(k) {
                    self.activate_setting(i, Some((x, y)));
                }
            }
            SHit::Step(k, up) => {
                if let Some(Line::Setting(i)) = line(k) {
                    self.step_setting(i, up);
                }
            }
            SHit::Remove(k) => {
                if let Some(Line::Path(i, n)) = line(k) {
                    self.remove_path(i, n);
                }
            }
            SHit::Button(k, b) => {
                if let Some(l) = line(k) {
                    self.press_button(&l, b);
                }
            }
            SHit::Line(_) | SHit::Panel | SHit::Search => {}
        }
        self.dirty = true;
    }

    fn reveal_setting(&mut self, i: usize) {
        let Some(ui) = &mut self.settings else { return };
        ui.page = SETTINGS[i].page;
        ui.search.edit.set("");
        ui.searching = false;
        self.layout();
        let Some(ui) = &mut self.settings else { return };
        if let Some(k) = ui.lines.iter().position(|l| *l == Line::Setting(i)) {
            (ui.focus, ui.sub, ui.reveal) = (k, 0, true);
        }
        self.layout();
    }

    pub(super) fn activate_setting(&mut self, i: usize, at: Option<(f32, f32)>) {
        match (SETTINGS[i].kind, SETTINGS[i].get(self)) {
            (Kind::Toggle(..), Value::Bool(b)) => self.change_setting(i, Value::Bool(!b)),
            (Kind::Choice(choices, ..), Value::Choice(cur)) => {
                let entries = choices.iter().enumerate().map(|(k, (_, label))| ((*label).to_owned(), Cmd::Setting(i, k), k == cur)).collect();
                let items = self.list_menu(entries);
                let (x, y) = self.menu_spot(i, at);
                self.flit_menu(x, y, items);
            }
            (Kind::Pick(Picker::Color, ..), _) => {
                let (x, y) = self.menu_spot(i, at);
                self.color_menu(i, x, y);
            }
            (Kind::Pick(Picker::Font, ..), _) => self.pick_font(i),
            (Kind::Text(..), Value::Text(t)) => self.open_pick(Palette::new(Pick::Setting(SettingPick::Text(i)), Vec::new(), &t)),
            (Kind::Paths(..), _) => self.open_pick(Palette::new(Pick::Setting(SettingPick::AddPath(i)), Vec::new(), "")),
            _ => {}
        }
    }

    fn menu_spot(&self, i: usize, at: Option<(f32, f32)>) -> (f32, f32) {
        let k = self.settings.as_ref().and_then(|ui| ui.lines.iter().position(|l| *l == Line::Setting(i)));
        let r = k.and_then(|k| self.settings.as_ref().map(|ui| control_rect(ui.rects[k], |v| (v * self.scale).round())));
        r.map(|r| (r.x, r.bottom())).or(at).unwrap_or_default()
    }

    fn step_setting(&mut self, i: usize, up: bool) {
        let (Kind::Number { min, max, step, .. }, Value::Number(n)) = (SETTINGS[i].kind, SETTINGS[i].get(self)) else { return };
        let next = if up { (n / step + 1) * step } else { (n - 1).div_euclid(step) * step };
        self.change_setting(i, Value::Number(next.clamp(min, max)));
    }

    fn turn_setting(&mut self, i: usize, back: bool) {
        match (SETTINGS[i].kind, SETTINGS[i].get(self)) {
            (Kind::Choice(c, ..), Value::Choice(k)) => self.choose_setting(i, if back { (k + c.len() - 1) % c.len() } else { (k + 1) % c.len() }),
            (Kind::Number { .. }, _) => self.step_setting(i, !back),
            (Kind::Toggle(..), Value::Bool(b)) if b == back => self.change_setting(i, Value::Bool(!back)),
            _ => {}
        }
    }

    fn press_button(&mut self, line: &Line, b: usize) {
        if let Line::Action(i, n, _) = *line {
            return self.press_chip(i, n, b);
        }
        let Some(&btn) = line.buttons().get(b) else { return };
        let owner = match line {
            Line::Buttons(Some(i), _) => Some(*i),
            _ => None,
        };
        let tag = match line {
            Line::Tag(k) => Some(*k),
            _ => None,
        };
        match (btn, owner, tag) {
            (Btn::AddHome, Some(i), _) => self.add_path(i, crate::location::Location::home_page()),
            (Btn::AddCurrent, Some(i), _) => {
                let here = self.current_location().clone();
                self.add_path(i, here);
            }
            (Btn::AddTyped, Some(i), _) => self.open_pick(Palette::new(Pick::Setting(SettingPick::AddPath(i)), Vec::new(), "")),
            (Btn::NewTag, ..) => self.new_tag(false),
            (Btn::ClearCache, ..) => {
                self.clear_thumbnails();
                self.about.cache_bytes = Some(0);
                self.effects.push(Effect::Settings(SettingsCmd::CacheSize));
                self.layout();
            }
            (Btn::OpenFile, ..) => self.open_settings_file(),
            (Btn::Export, ..) => self.ask_export_settings(),
            (Btn::Import, ..) => self.ask_import_settings(),
            (Btn::Reset, ..) => self.ask_reset_settings(),
            (Btn::RestoreKeys, ..) => self.ask_reset_keys(),
            (Btn::CustomizeToolbar, ..) => self.customize_toolbar(),
            (Btn::CopyAbout, ..) => {
                let text = format!("Flit {}\r\n{}", env!("CARGO_PKG_VERSION"), self.about.os);
                self.effects.push(Effect::SetClipboard(text));
                self.status("Copied the version info");
            }
            (Btn::OpenLog, ..) => self.open_log_file(),
            (Btn::OpenLogs, ..) => self.open_log_folder(Instant::now()),
            (Btn::Docs, ..) => self.open_link("help.docs"),
            (Btn::ReportBug, ..) => self.open_link("help.bug"),
            (Btn::RequestFeature, ..) => self.open_link("help.feature"),
            (Btn::Repository, ..) => self.open_link("help.repository"),
            (Btn::Privacy, ..) => self.open_link("help.privacy"),
            (Btn::Rename, _, Some(k)) => {
                let name = self.tag_defs().get(k).map(|t| t.name.clone()).unwrap_or_default();
                self.open_tag_pick(super::tags::TagPick::Name { tag: Some(k), assign: Vec::new() }, &name);
            }
            (Btn::Recolor, _, Some(k)) => self.open_tag_pick(super::tags::TagPick::Color { tag: Some(k), name: String::new(), assign: Vec::new() }, ""),
            (Btn::Delete, _, Some(k)) => self.open_tag_pick(super::tags::TagPick::Delete(k), ""),
            _ => {}
        }
    }

    fn activate_line(&mut self, k: usize) {
        let Some(ui) = &self.settings else { return };
        let (searching_results, sub) = (!ui.query().is_empty(), ui.sub);
        match ui.lines.get(k).cloned() {
            Some(Line::Setting(i)) if searching_results => self.reveal_setting(i),
            Some(Line::Setting(i)) => self.activate_setting(i, None),
            Some(Line::Path(i, n)) => self.remove_path(i, n),
            Some(l @ (Line::Buttons(..) | Line::Tag(_) | Line::Action(..))) => self.press_button(&l, sub),
            _ => {}
        }
    }

    pub(super) fn settings_key(&mut self, key: Key, m: Mods) -> bool {
        if self.nav.palette.is_some() {
            return false;
        }
        let closes = self.keymap.runs(key, m, "settings.open");
        let Some(ui) = &mut self.settings else { return false };
        self.dirty = true;
        let plain = !m.ctrl && !m.alt;
        let (line, sub) = (ui.lines.get(ui.focus).cloned(), ui.sub);
        match key {
            Key::Escape if !ui.search.text().is_empty() => {
                ui.search.edit.set("");
                ui.focus = usize::MAX;
            }
            Key::Escape => return self.close_settings_handled(),
            _ if closes => return self.close_settings_handled(),
            Key::Char('F') if m.ctrl => {
                ui.searching = true;
                ui.search.edit.select_all();
            }
            Key::Tab if !m.ctrl => ui.searching = !ui.searching,
            Key::PageUp | Key::PageDown if m.ctrl => {
                let at = Page::ALL.iter().position(|p| *p == ui.page).unwrap_or(0);
                let n = Page::ALL.len();
                ui.page = Page::ALL[if key == Key::PageUp { (at + n - 1) % n } else { (at + 1) % n }];
                ui.search.edit.set("");
                (ui.focus, ui.scroll, ui.searching) = (usize::MAX, 0.0, false);
            }
            Key::Up | Key::Down if plain => {
                let step = if key == Key::Up { -1 } else { 1 };
                match (ui.searching, key) {
                    (true, Key::Down) => {
                        ui.searching = false;
                        ui.focus = ui.first_focusable();
                    }
                    (true, _) => {}
                    (false, Key::Up) if ui.focusable(ui.focus, step).is_none() => ui.searching = true,
                    _ => {
                        if let Some(k) = ui.focusable(ui.focus, step) {
                            ui.focus = k;
                        }
                    }
                }
                ui.sub = 0;
                ui.reveal = true;
            }
            Key::Enter if ui.searching => {
                ui.searching = false;
                ui.focus = ui.first_focusable();
                ui.reveal = true;
            }
            _ if ui.searching => {
                if let FieldKey::Edited { changed, action } = ui.search.key(key, m) {
                    if changed {
                        (ui.focus, ui.scroll) = (usize::MAX, 0.0);
                    }
                    match action {
                        EditAction::Copy(s) => self.effects.push(Effect::SetClipboard(s)),
                        EditAction::Paste => self.effects.push(Effect::Paste),
                        _ => {}
                    }
                }
            }
            Key::Backspace => {
                ui.searching = true;
                let _ = ui.search.key(key, m);
            }
            Key::Left | Key::Right if plain => match line {
                Some(Line::Setting(i)) => {
                    self.turn_setting(i, key == Key::Left);
                }
                Some(l @ (Line::Buttons(..) | Line::Tag(_) | Line::Action(..))) => {
                    let n = l.count().max(1);
                    ui.sub = if key == Key::Left { (ui.sub + n - 1) % n } else { (ui.sub + 1) % n };
                }
                _ => {}
            },
            Key::Enter | Key::Space if plain => {
                let k = ui.focus;
                self.activate_line(k);
            }
            Key::Delete if plain => match line {
                Some(Line::Path(i, n)) => self.remove_path(i, n),
                Some(Line::Action(i, n, _)) if sub < n => {
                    if let Some(&c) = self.keymap.keys(i).get(sub) {
                        self.remove_chord(i, c);
                    }
                }
                _ => {}
            },
            _ => return true,
        }
        self.layout();
        true
    }

    fn close_settings_handled(&mut self) -> bool {
        self.close_settings();
        true
    }

    pub(super) fn settings_insert(&mut self, s: &str) -> bool {
        if self.nav.palette.is_some() {
            return false;
        }
        let Some(ui) = &mut self.settings else { return false };
        if s.is_empty() || (s == " " && !ui.searching) {
            return true;
        }
        ui.searching = true;
        ui.search.insert(s);
        (ui.focus, ui.scroll) = (usize::MAX, 0.0);
        self.layout();
        self.dirty = true;
        true
    }

    pub(super) fn settings_wheel(&mut self, dy: f32) -> bool {
        if !matches!(self.hover, Hit::Settings(_)) {
            return false;
        }
        let step = 3.0 * (ITEM_H * self.scale).round();
        if let Some(ui) = &mut self.settings {
            ui.scroll = (ui.scroll - dy * step).clamp(0.0, (ui.content_h - ui.list.h).max(0.0));
        }
        self.layout();
        self.dirty = true;
        true
    }

    fn setting_value_text(&self, i: usize) -> String {
        match (SETTINGS[i].kind, SETTINGS[i].get(self)) {
            (Kind::Choice(c, ..), Value::Choice(k)) => c.get(k).map_or("", |c| c.1).to_owned(),
            (Kind::Number { unit, .. }, Value::Number(n)) => format!("{n} {unit}"),
            (Kind::Text(..), Value::Text(t)) => t,
            (Kind::Paths(..), Value::Paths(p)) => format!("{} pages", p.len()),
            (Kind::Pick(..), _) => self.pick_text(i).unwrap_or_default(),
            _ => String::new(),
        }
    }

    pub(super) fn draw_settings<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme) {
        let Some(mut ui) = self.settings.take() else { return };
        self.paint_settings(g, t, &mut ui);
        self.settings = Some(ui);
    }

    fn paint_settings<B: TextBackend>(&self, g: &mut Gfx<B>, t: &Theme, ui: &mut SettingsUi) {
        let sc = self.scale;
        let s = |v: f32| (v * sc).round();
        let (font, small, bold, head) = (self.ui_font(13.0), self.ui_font(12.0), FontStyle::new(FontFamily::Ui, 14.0 * sc, true), FontStyle::new(FontFamily::Ui, 20.0 * sc, true));
        let icons = self.icon_font(10.0);
        let (s4, s6, s8) = (s(4.0), s(6.0), s(8.0));
        let (w, h) = self.size;
        let top = self.layout.title.bottom();
        g.dl.rect(Rect::new(0.0, top, w, h - top), if self.dark { Color::rgba(0x00000070) } else { Color::rgba(0x00000040) });
        let p = ui.panel;
        g.dl.shadow(Rect::new(p.x, p.y + s8, p.w, p.h), if self.dark { Color::rgba(0x00000090) } else { Color::rgba(0x00000030) }, s8, s(24.0));
        g.dl.round_rect(p, t.bg, s8);
        g.dl.outline(p, t.input_border, s8, 1.0);
        let nav_w = ui.list.x - p.x;
        g.dl.round_rect(Rect::new(p.x, p.y, nav_w, p.h), t.chrome, s8);
        g.dl.rect(Rect::new(p.x + nav_w - s8, p.y, s8, p.h), t.chrome);
        g.dl.rect(Rect::new(p.x + nav_w, p.y, 1.0, p.h), t.separator);
        let mh = g.metrics(head);
        let (mf, ms) = (g.metrics(font), g.metrics(small));
        let mid = |r: Rect, m: crate::text::FontMetrics| (r.y + (r.h + m.ascent - m.descent) / 2.0).round();
        let hover = self.hover;
        centered(g, Rect::new(p.x + s(14.0), p.y + s(14.0), s(24.0), s(30.0)), super::sidebar::G_SETTINGS, self.icon_font(16.0), t.accent);
        g.text(p.x + s(44.0), (p.y + s(14.0) + (s(30.0) + mh.ascent - mh.descent) / 2.0).round(), "Settings", head, t.text, nav_w - s(52.0));
        let searching = !ui.query().is_empty();
        for (k, r) in ui.nav.iter().enumerate() {
            let page = Page::ALL[k];
            if page == ui.page && !searching {
                g.dl.round_rect(*r, t.selection, s4);
                g.dl.rect(Rect::new(r.x, r.y + s(9.0), s(3.0), r.h - s(18.0)), t.accent);
            } else if hover == Hit::Settings(SHit::Page(k)) {
                g.dl.round_rect(*r, t.button_hover, s4);
            }
            g.text(r.x + s(14.0), mid(*r, mf), page.title(), font, t.text, r.w - s(20.0));
        }
        if hover == Hit::Settings(SHit::Close) {
            g.dl.round_rect(ui.close, t.button_hover, s4);
        }
        centered(g, ui.close, G_CLOSE, icons, t.text_dim);
        let sr = ui.search_rect;
        let focused_search = ui.searching && self.active && self.nav.palette.is_none();
        g.dl.round_rect(sr, t.input_bg, s4);
        g.dl.outline(sr, if focused_search { t.accent } else { t.input_border }, s4, if focused_search { sc.round().max(1.0) } else { 1.0 });
        centered(g, Rect::new(sr.x + s4, sr.y, s(24.0), sr.h), G_SEARCH, icons, t.text_dim);
        let inner = Rect::new(sr.x + s(32.0), sr.y, sr.w - s(40.0), sr.h);
        ui.search.draw(g, t, inner, font, focused_search, "Search settings", sc);
        let list = ui.list;
        g.dl.set_clip(Some(list));
        let list_focused = !ui.searching && self.active && self.nav.palette.is_none();
        for (k, (line, &r)) in ui.lines.iter().zip(&ui.rects).enumerate() {
            if r.bottom() < list.y || r.y > list.bottom() {
                continue;
            }
            let focused = list_focused && k == ui.focus;
            let row = Rect::new(r.x + s4, r.y + 1.0, r.w - s8, r.h - 2.0);
            if line.focusable() && !matches!(line, Line::Buttons(..)) {
                if hover == Hit::Settings(SHit::Line(k)) {
                    g.dl.round_rect(row, t.hover, s6);
                }
                if focused {
                    g.dl.outline(row, t.focus_ring, s6, 1.0);
                }
            }
            match line {
                Line::Group(title) => {
                    let mb = g.metrics(bold);
                    g.text(r.x + s(PAD), (r.bottom() - s(10.0) - mb.descent).round(), title, bold, t.text, r.w - 2.0 * s(PAD));
                }
                Line::Note(text) => {
                    g.text(r.x + s(PAD), mid(r, mf), text, font, t.text_dim, r.w - 2.0 * s(PAD));
                }
                Line::Info(label, value) => {
                    g.text(r.x + s(PAD), mid(r, ms), label, small, t.text_dim, s(200.0));
                    g.text(r.x + s(PAD) + s(210.0), mid(r, ms), value, small, t.text, r.w - s(PAD) * 2.0 - s(210.0));
                }
                Line::Setting(i) => {
                    let set = &SETTINGS[*i];
                    let text_w = r.w - s(PAD) * 3.0 - s(CONTROL_W);
                    let ty = r.y + s(10.0) + mf.ascent;
                    g.text(r.x + s(PAD), ty.round(), set.title, font, t.text, text_w);
                    g.text(r.x + s(PAD), (ty + s(20.0)).round(), set.help, small, t.text_dim, text_w);
                    let value = self.setting_value_text(*i);
                    match (set.kind, set.get(self)) {
                        (Kind::Toggle(..), Value::Bool(on)) => {
                            let sw = switch_rect(r, s);
                            let hot = hover == Hit::Settings(SHit::Control(k));
                            let knob = s(14.0);
                            if on {
                                g.dl.round_rect(sw, if hot { t.accent.alpha(0xD0) } else { t.accent }, sw.h / 2.0);
                                g.dl.round_rect(Rect::new(sw.right() - knob - s4, sw.y + (sw.h - knob) / 2.0, knob, knob), t.bg, knob / 2.0);
                            } else {
                                g.dl.round_rect(sw, if hot { t.button_hover } else { t.input_bg }, sw.h / 2.0);
                                g.dl.outline(sw, t.text_dim, sw.h / 2.0, sc.round().max(1.0));
                                g.dl.round_rect(Rect::new(sw.x + s4, sw.y + (sw.h - knob) / 2.0, knob, knob), t.text_dim, knob / 2.0);
                            }
                        }
                        (Kind::Choice(..) | Kind::Text(..) | Kind::Pick(..), _) => {
                            let c = control_rect(r, s);
                            g.dl.round_rect(c, if hover == Hit::Settings(SHit::Control(k)) { t.button_hover } else { t.input_bg }, s4);
                            g.dl.outline(c, t.input_border, s4, 1.0);
                            let mut tx = c.x + s(10.0);
                            if let Some(color) = self.swatch(*i) {
                                let sw = Rect::new(tx, c.y + (c.h - s(16.0)) / 2.0, s(16.0), s(16.0));
                                g.dl.round_rect(sw, color, s4);
                                g.dl.outline(sw, t.input_border, s4, 1.0);
                                tx = sw.right() + s(8.0);
                            }
                            g.text(tx, mid(c, mf), &value, font, t.text, c.right() - s(30.0) - tx);
                            centered(g, Rect::new(c.right() - s(30.0), c.y, s(24.0), c.h), G_DOWN, icons, t.text_dim);
                        }
                        (Kind::Number { .. }, _) => {
                            let c = control_rect(r, s);
                            let (minus, plus) = step_rects(r, s);
                            g.dl.round_rect(c, t.input_bg, s4);
                            g.dl.outline(c, t.input_border, s4, 1.0);
                            for (b, glyph, up) in [(minus, G_MINUS, false), (plus, G_PLUS, true)] {
                                if hover == Hit::Settings(SHit::Step(k, up)) {
                                    g.dl.round_rect(b.inset(s(2.0), s(2.0)), t.button_hover, s4);
                                }
                                centered(g, b, glyph, icons, t.text);
                            }
                            centered(g, Rect::new(minus.right(), c.y, plus.x - minus.right(), c.h), &value, font, t.text);
                        }
                        _ => {
                            let vw = g.measure(&value, small);
                            g.text(r.right() - s(PAD) - vw, mid(r, ms), &value, small, t.text_dim, f32::INFINITY);
                        }
                    }
                }
                Line::Path(i, n) => {
                    let text = self.paths_of(*i).get(*n).map(settings::path_text).unwrap_or_default();
                    g.text(r.x + s(32.0), mid(r, mf), &text, font, t.text, r.w - s(PAD) * 2.0 - s(60.0));
                    let rr = remove_rect(r, s);
                    if hover == Hit::Settings(SHit::Remove(k)) {
                        g.dl.round_rect(rr, t.button_hover, s4);
                    }
                    centered(g, rr, G_CLOSE, self.icon_font(8.0), t.text_dim);
                }
                Line::Tag(n) => {
                    if let Some(tag) = self.tag_defs().get(*n) {
                        let dot = s(12.0);
                        g.dl.round_rect(Rect::new(r.x + s(PAD), r.y + (r.h - dot) / 2.0, dot, dot), Color::rgb(tag.color), dot / 2.0);
                        let count = self.tags().count(*n);
                        let label = format!("{} · {} items", tags::color_name(tag.color), count);
                        let nw = g.text(r.x + s(PAD) + dot + s(10.0), mid(r, mf), &tag.name, font, t.text, s(220.0));
                        g.text(r.x + s(PAD) + dot + s(24.0) + nw, mid(r, ms), &label, small, t.text_dim, s(220.0));
                    }
                }
                Line::Action(i, n, _) => {
                    let a = &ACTIONS[*i];
                    let chips = button_rects(line, r, s);
                    let room = chips.first().map_or(r.right(), |c| c.x) - r.x - s(PAD) * 2.0;
                    let tw = g.text(r.x + s(PAD), mid(r, mf), a.title, font, t.text, room);
                    g.text(r.x + s(PAD) + tw + s(10.0), mid(r, ms), a.id, small, t.text_dim, room - tw - s(10.0));
                    let keys = self.keymap.keys(*i);
                    for (b, chip) in chips.into_iter().enumerate() {
                        let on = focused && ui.sub == b;
                        g.dl.round_rect(chip, if hover == Hit::Settings(SHit::Button(k, b)) { t.button_pressed } else { t.button_hover }, s4);
                        g.dl.outline(chip, if on { t.accent } else { t.input_border }, s4, if on { sc.round().max(1.0) } else { 1.0 });
                        match keys.get(b) {
                            Some(c) if b < *n => centered(g, chip, &chord_text(c), small, t.text),
                            _ if b == *n => centered(g, chip, G_PLUS, self.icon_font(9.0), t.accent),
                            _ => centered(g, chip, G_UNDO, self.icon_font(10.0), t.text_dim),
                        }
                    }
                }
                Line::Buttons(..) => {}
            }
            for (b, (br, btn)) in button_rects(line, r, s).into_iter().zip(line.buttons()).enumerate() {
                let hot = hover == Hit::Settings(SHit::Button(k, b));
                let danger = matches!(btn, Btn::Reset | Btn::Delete | Btn::RestoreKeys);
                g.dl.round_rect(br, if hot { t.button_pressed } else { t.button_hover }, s4);
                g.dl.outline(br, if focused && ui.sub == b { t.accent } else { t.input_border }, s4, if focused && ui.sub == b { sc.round().max(1.0) } else { 1.0 });
                let glyph_w = if matches!(btn, Btn::AddHome | Btn::AddCurrent | Btn::NewTag) { s(18.0) } else { 0.0 };
                if glyph_w > 0.0 {
                    centered(g, Rect::new(br.x + s(8.0), br.y, glyph_w, br.h), G_PLUS, self.icon_font(9.0), t.accent);
                }
                let lw = g.measure(btn.label(), font).min(br.w - s(16.0) - glyph_w);
                g.text((br.x + glyph_w + (br.w - glyph_w - lw) / 2.0).round(), mid(br, mf), btn.label(), font, if danger { t.error } else { t.text }, br.w - s(16.0) - glyph_w);
            }
        }
        g.dl.set_clip(None);
        if ui.content_h > list.h {
            let frac = ui.scroll / (ui.content_h - list.h);
            let th = (list.h * list.h / ui.content_h).max(s(24.0));
            let thumb = Rect::new(list.right() - s(6.0), list.y + (list.h - th) * frac, s(4.0), th);
            g.dl.round_rect(thumb, t.scrollbar, s(2.0));
        }
    }
}

#[cfg(test)]
#[path = "../../test/ui/settings_page.rs"]
mod tests;
