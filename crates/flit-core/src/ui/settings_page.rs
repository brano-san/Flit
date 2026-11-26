mod layout;
mod paint;

use super::actions::Chord;
use super::edit::EditAction;
use super::field::{Field, FieldKey};
use super::menu::Cmd;
use super::menu_editor::{self, MenuRow};
use super::widgets::Paint;
use super::palette::{Palette, Pick};
use super::fuzzy::fuzzy;
use super::settings::{self, Kind, Page, Picker, Setting, Value, SETTINGS};
use super::a11y::{leaf, put};
use super::setting_editor::BROWSE;
use super::{centered, App, Effect, Glide, Hit, SettingsCmd, G_CLOSE};
use crate::a11y::{Role, Tree};
use crate::i18n::{P, S};
use crate::input::{Key, Mods, MouseButton};
use crate::location::{Location, ProviderId};
use crate::open::{OpenTarget, Route};
use crate::path::FsPath;
use crate::provider::{Launch, ProviderRequest};
use crate::render::{Color, Rect};
use crate::tags;
use crate::text::{FontFamily, FontStyle, Gfx, TextBackend};
use crate::theme::Theme;
use std::time::Instant;

const SETTINGS_ID: u64 = 13;
const LIST_ID: u64 = 15;
const CATEGORIES_ID: u64 = 16;
const NAV_MIN: f32 = 168.0;
const NAV_MAX: f32 = 280.0;
const NAV_TEXT_ROOM: f32 = 88.0;
const RAIL_W: f32 = 48.0;
const RAIL_CONTENT_MIN: f32 = 400.0;
const NAV_ITEM_H: f32 = 34.0;
const NAV_ITEM_MIN: f32 = 24.0;
const NAV_PAD: f32 = 6.0;
const MIN_W: f32 = 560.0;
const MAX_W: f32 = 960.0;
const FIELD_H: f32 = 32.0;
const HINT_H: f32 = 36.0;
const EDGE: f32 = 12.0;
const SETTING_H: f32 = 46.0;
const CONTROL_H: f32 = 30.0;
const ROW_GAP: f32 = 6.0;
const TEXT_GAP: f32 = 3.0;
const TEXT_PAD: f32 = 10.0;
const BUTTON_GAP: f32 = 8.0;
const BUTTON_PAD: f32 = 24.0;
const GROUP_H: f32 = 34.0;
const NOTE_H: f32 = 32.0;
const ITEM_H: f32 = 32.0;
const TAG_H: f32 = 40.0;
const INFO_H: f32 = 28.0;
const CONTROL_MIN: f32 = 120.0;
const CONTROL_MAX: f32 = 320.0;
const LABEL_MIN: f32 = 120.0;
const INFO_LABEL_W: f32 = 200.0;
const BAR_W: f32 = 12.0;
const BAR_THUMB_MIN: f32 = 24.0;
const BUTTON_W: f32 = 190.0;
const TAG_BUTTON_W: f32 = 92.0;
const PAD: f32 = 16.0;
const INDENT: f32 = 22.0;
pub const NESTED: [&str; 9] = ["files.system", "files.dotfiles", "files.sizes_ssd", "files.sizes_hdd", "files.sizes_network", "files.single_click_files", "files.single_click_folders", "files.single_click_columns", "general.startup_pages"];
const NUMBER_DROPDOWNS: [&str; 3] = [settings::SPACING, settings::OPACITY, settings::CACHE];
pub const RESTART: [&str; 0] = [];
pub(super) const EXPORT_NAME: &str = "flit-settings.json";
const OS_NAME: &str = if cfg!(windows) { "Windows" } else { "Linux" };

const G_PLUS: &str = "\u{E710}";
const G_DOWN: &str = "\u{E70D}";
const G_SEARCH: &str = "\u{E721}";
const G_DESCRIBE: &str = "\u{E8A5}";
const G_RESTART: &str = "\u{E72C}";

fn page_glyph(page: Page) -> &'static str {
    match page {
        Page::Interface => "\u{E7F4}",
        Page::Menu => "\u{E8FD}",
        Page::Colors => "\u{E8B9}",
        Page::Files => "\u{E8B7}",
        Page::Data => "\u{EDA2}",
        Page::Input => "\u{E8CB}",
        Page::Windows => "\u{E923}",
        Page::Tags => "\u{E8EC}",
        Page::Advanced => "\u{E713}",
        Page::About => "\u{E946}",
    }
}

const DOCS_URL: &str = "https://github.com/brano-san/Flit#readme";
const BUG_URL: &str = "https://github.com/brano-san/Flit/issues/new?labels=bug";
const FEATURE_URL: &str = "https://github.com/brano-san/Flit/issues/new?labels=enhancement";
const REPOSITORY_URL: &str = "https://github.com/brano-san/Flit";
const PRIVACY_URL: &str = "https://github.com/brano-san/Flit#конфиденциальность";
pub(super) const LINKS: [(&str, &str); 5] = [("help.docs", DOCS_URL), ("help.bug", BUG_URL), ("help.feature", FEATURE_URL), ("help.repository", REPOSITORY_URL), ("help.privacy", PRIVACY_URL)];

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
    Rename,
    Recolor,
    Delete,
    OpenLog,
    OpenLogs,
    Docs,
    ReportBug,
    RequestFeature,
    Repository,
    Privacy,
    NewEntry,
    NewSubmenu,
    Edit,
    Up,
    MenuReset,
    CheckUpdates,
    UndoSystemChanges,
    RegisterArchive,
}

impl Btn {
    const ALL: [Btn; 28] = [
        Btn::AddHome,
        Btn::AddCurrent,
        Btn::AddTyped,
        Btn::NewTag,
        Btn::ClearCache,
        Btn::OpenFile,
        Btn::Export,
        Btn::Import,
        Btn::Reset,
        Btn::CopyAbout,
        Btn::Rename,
        Btn::Recolor,
        Btn::Delete,
        Btn::OpenLog,
        Btn::OpenLogs,
        Btn::Docs,
        Btn::ReportBug,
        Btn::RequestFeature,
        Btn::Repository,
        Btn::Privacy,
        Btn::NewEntry,
        Btn::NewSubmenu,
        Btn::Edit,
        Btn::Up,
        Btn::MenuReset,
        Btn::CheckUpdates,
        Btn::UndoSystemChanges,
        Btn::RegisterArchive,
    ];

    fn label(self) -> &'static str {
        match self {
            Btn::AddHome => S::SettingsPageButtonAddHome.t(),
            Btn::AddCurrent => S::SettingsPageButtonAddCurrent.t(),
            Btn::AddTyped => S::SettingsPageButtonAddTyped.t(),
            Btn::NewTag => S::SettingsPageButtonNewTag.t(),
            Btn::ClearCache => S::SettingsPageButtonClearCache.t(),
            Btn::OpenFile => S::SettingsPageButtonOpenFile.t(),
            Btn::Export => S::SettingsPageButtonExport.t(),
            Btn::Import => S::SettingsPageButtonImport.t(),
            Btn::Reset => S::SettingsPageButtonReset.t(),
            Btn::CopyAbout => S::SettingsPageButtonCopyAbout.t(),
            Btn::Rename => S::SettingsPageButtonRename.t(),
            Btn::Recolor => S::SettingsPageButtonRecolor.t(),
            Btn::Delete => S::SettingsPageButtonDelete.t(),
            Btn::OpenLog => S::SettingsPageButtonOpenLog.t(),
            Btn::OpenLogs => S::SettingsPageButtonOpenLogs.t(),
            Btn::Docs => S::SettingsPageButtonDocs.t(),
            Btn::ReportBug => S::SettingsPageButtonReportBug.t(),
            Btn::RequestFeature => S::SettingsPageButtonRequestFeature.t(),
            Btn::Repository => S::SettingsPageButtonRepository.t(),
            Btn::Privacy => S::SettingsPageButtonPrivacy.t(),
            Btn::NewEntry => S::UserMenuNewEntry.t(),
            Btn::NewSubmenu => S::UserMenuNewSubmenu.t(),
            Btn::Edit => S::UserMenuEdit.t(),
            Btn::Up => S::UserMenuUp.t(),
            Btn::MenuReset => S::MenuEditorReset.t(),
            Btn::CheckUpdates => S::UpdatePageCheck.t(),
            Btn::UndoSystemChanges => S::SettingsPageSystemUndoAll.t(),
            Btn::RegisterArchive => S::SettingsPageSystemRegisterAgain.t(),
        }
    }
}

const PATH_BUTTONS: &[Btn] = &[Btn::AddHome, Btn::AddCurrent, Btn::AddTyped];
const TAG_BUTTONS: &[Btn] = &[Btn::Rename, Btn::Recolor, Btn::Delete];
const ENTRY_BUTTONS: &[Btn] = &[Btn::Up, Btn::Edit, Btn::Delete];
const USER_BUTTONS: &[Btn] = &[Btn::NewEntry, Btn::NewSubmenu];
const FILE_BUTTONS: &[Btn] = &[Btn::OpenFile, Btn::Export, Btn::Import];
const LOG_BUTTONS: &[Btn] = &[Btn::OpenLog, Btn::OpenLogs];
const HELP_BUTTONS: &[Btn] = &[Btn::Docs, Btn::ReportBug, Btn::RequestFeature];
const LINK_BUTTONS: &[Btn] = &[Btn::Repository, Btn::Privacy];
pub const LOG_FILE: &str = "flit.log";
const NO_LOGS: S = S::SettingsPageStatusNoLogs;
const NO_TELEMETRY: S = S::SettingsPageLineNoTelemetry;

#[derive(Clone, Debug, PartialEq)]
enum Line {
    Group(&'static str),
    Note(&'static str),
    Setting(usize),
    Off(usize),
    Path(usize, usize),
    Buttons(Option<usize>, &'static [Btn]),
    Tag(usize),
    Entry(super::user_menu::EntryAt),
    MenuItem(MenuRow),
    Info(&'static str, String),
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) struct Fit {
    pub(super) body: f32,
    pub(super) small: f32,
    bold: f32,
    labels: [f32; Btn::ALL.len()],
    titles: f32,
}

impl Fit {
    fn guess(sc: f32) -> Self {
        let line = |px: f32| (px * 1.35 * sc).ceil();
        Self { body: line(13.0), small: line(12.0), bold: line(14.0), labels: [0.0; Btn::ALL.len()], titles: 0.0 }
    }

    fn nav_w(&self, s: impl Fn(f32) -> f32) -> f32 {
        (self.titles.ceil() + s(NAV_TEXT_ROOM)).clamp(s(NAV_MIN), s(NAV_MAX))
    }

    fn label(&self, b: Btn) -> f32 {
        self.labels[b as usize]
    }
}

fn line_h(m: crate::text::FontMetrics) -> f32 {
    (m.ascent + m.descent).ceil()
}

fn text_baselines(r: Rect, title: crate::text::FontMetrics, help: crate::text::FontMetrics, s: impl Fn(f32) -> f32) -> (f32, f32) {
    let block = line_h(title) + s(TEXT_GAP) + line_h(help);
    let top = r.y + ((r.h - block) / 2.0).max(0.0);
    ((top + title.ascent).round(), (top + line_h(title) + s(TEXT_GAP) + help.ascent).round())
}

impl Line {
    fn height(&self, describe: bool, fit: &Fit, s: impl Fn(f32) -> f32) -> f32 {
        let room = |min: f32, text: f32| s(min).max(text + s(TEXT_PAD));
        let control = s(CONTROL_H) + s(ROW_GAP);
        match self {
            Line::Group(_) => room(GROUP_H, fit.bold + s(TEXT_PAD)),
            Line::Note(_) => room(NOTE_H, fit.body),
            Line::Setting(_) | Line::Off(_) if describe => room(SETTING_H, fit.body + s(TEXT_GAP) + fit.small).max(control),
            Line::Setting(_) | Line::Off(_) => room(0.0, fit.body).max(control),
            Line::Path(..) | Line::MenuItem(_) => room(ITEM_H, fit.body),
            Line::Buttons(..) => 0.0,
            Line::Tag(_) | Line::Entry(_) => room(TAG_H, fit.body).max(control + s(4.0)),
            Line::Info(..) => room(INFO_H, fit.small),
        }
    }

    fn count(&self) -> usize {
        self.buttons().len()
    }

    fn focusable(&self) -> bool {
        !matches!(self, Line::Group(_) | Line::Note(_) | Line::Info(..) | Line::Off(_))
    }

    fn buttons(&self) -> &'static [Btn] {
        match self {
            Line::Buttons(_, b) => b,
            Line::Tag(_) => TAG_BUTTONS,
            Line::Entry(_) => ENTRY_BUTTONS,
            _ => &[],
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SHit {
    Outside,
    Panel,
    Close,
    Describe,
    Search,
    Nav(usize),
    Line(usize),
    Control(usize),
    Thumb,
    Track,
    Step(usize, bool),
    Remove(usize),
    Button(usize, usize),
    Check(usize),
    Star(usize),
    Fold(usize),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum SettingPick {
    Text(usize),
    AddPath(usize),
    Export,
    Import,
    Reset,
    ResetKeys,
    RegisterArchive,
    Capture { action: usize, old: Option<Chord>, chord: Option<Chord> },
    Color(usize),
    Font(usize),
}

impl SettingPick {
    pub(super) fn placeholder(&self) -> &'static str {
        match self {
            SettingPick::Text(_) => S::SettingsPagePickText.t(),
            SettingPick::AddPath(_) => S::SettingsPagePickAddPath.t(),
            SettingPick::Export => S::SettingsPagePickExport.t(),
            SettingPick::Import => S::SettingsPagePickImport.t(),
            SettingPick::Reset => S::SettingsPagePickReset.t(),
            SettingPick::ResetKeys => S::SettingsPagePickResetKeys.t(),
            SettingPick::RegisterArchive => S::SettingsPageSystemArchive.t(),
            SettingPick::Capture { .. } => S::SettingsPagePickCapture.t(),
            SettingPick::Color(_) => S::SettingsPagePickColor.t(),
            SettingPick::Font(_) => S::SettingsPagePickFont.t(),
        }
    }

    pub(super) fn empty(&self) -> &'static str {
        match self {
            SettingPick::Text(_) => S::SettingsPageHintText.t(),
            SettingPick::AddPath(_) => S::SettingsPageHintAddPath.t(),
            SettingPick::Export => S::SettingsPageHintExport.t(),
            SettingPick::Import => S::SettingsPageHintImport.t(),
            SettingPick::Reset | SettingPick::ResetKeys | SettingPick::RegisterArchive | SettingPick::Capture { .. } => "",
            SettingPick::Color(_) => S::SettingsPageHintColor.t(),
            SettingPick::Font(_) => S::SettingsPageHintFont.t(),
        }
    }

    pub(super) fn is_input(&self) -> bool {
        !matches!(self, SettingPick::Reset | SettingPick::ResetKeys | SettingPick::RegisterArchive | SettingPick::Capture { .. } | SettingPick::Font(_))
    }
}

pub type SystemChanges = Result<Vec<(S, String)>, String>;

#[derive(Default, Clone, Debug, PartialEq)]
pub struct About {
    pub os: String,
    pub file: Option<FsPath>,
    pub cache_bytes: Option<u64>,
    pub logs: Option<FsPath>,
    pub missing: Vec<MissingTool>,
    pub system_changes: Option<SystemChanges>,
    pub store_managed: bool,
    pub system_integration_unavailable: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MissingTool {
    pub tool: &'static str,
    pub purpose: S,
    pub package: &'static str,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Zone {
    Search,
    Nav,
    List,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) struct Start {
    pub(super) line: usize,
    pub(super) page: Page,
    pub(super) matches: usize,
}

pub(super) const KNOB: std::time::Duration = std::time::Duration::from_millis(140);

pub(super) struct SettingsUi {
    pub turn: super::address::Turn,
    pub knobs: Vec<(usize, super::widgets::Tween)>,
    pub knob_motion: bool,
    pub page: Page,
    search: Field,
    zone: Zone,
    lines: Vec<Line>,
    starts: Vec<Start>,
    jump: Option<(Page, bool)>,
    held: Option<(Page, f32)>,
    nav: Rect,
    nav_rects: Vec<Rect>,
    nav_at: usize,
    rail: bool,
    sticky: Option<(usize, Rect)>,
    rects: Vec<Rect>,
    focus: usize,
    sub: usize,
    pub(super) scroll: Glide,
    content_h: f32,
    cut: Vec<(usize, Rect)>,
    tip: Option<(usize, (f32, f32))>,
    reveal: bool,
    panel: Rect,
    search_rect: Rect,
    list: Rect,
    bar: Rect,
    describe: Rect,
    close: Rect,
    hints: Rect,
    pub(super) fit: Fit,
    pub(super) browsing: Option<SettingPick>,
}

impl SettingsUi {
    fn new(page: Page, sc: f32) -> Self {
        Self {
            page,
            fit: Fit::guess(sc),
            browsing: None,
            search: Field::default(),
            zone: Zone::List,
            lines: Vec::new(),
            starts: Vec::new(),
            jump: None,
            held: None,
            nav: Rect::default(),
            nav_rects: Vec::new(),
            nav_at: 0,
            rail: false,
            sticky: None,
            rects: Vec::new(),
            focus: 0,
            sub: 0,
            scroll: Glide::default(),
            content_h: 0.0,
            cut: Vec::new(),
            tip: None,
            reveal: false,
            panel: Rect::default(),
            search_rect: Rect::default(),
            list: Rect::default(),
            bar: Rect::default(),
            describe: Rect::default(),
            close: Rect::default(),
            hints: Rect::default(),
            turn: super::address::Turn::default(),
            knobs: Vec::new(),
            knob_motion: false,
        }
    }

    pub fn query(&self) -> &str {
        self.search.text().trim()
    }

    fn searching(&self) -> bool {
        self.zone == Zone::Search
    }

    fn page_at(&self) -> usize {
        self.starts.iter().position(|st| st.page == self.page).unwrap_or(0)
    }

    fn cycle_zone(&mut self, back: bool) {
        let order: &[Zone] = if self.nav_rects.is_empty() { &[Zone::Search, Zone::List] } else { &[Zone::Search, Zone::Nav, Zone::List] };
        let n = order.len();
        let at = order.iter().position(|z| *z == self.zone).unwrap_or(0);
        self.zone = order[if back { (at + n - 1) % n } else { (at + 1) % n }];
        if self.zone == Zone::Nav {
            self.nav_at = self.page_at();
        }
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

fn help_limit(query: &str) -> usize {
    query.split_whitespace().count() + 1
}

pub(super) fn option_matches(query: &str, set: &Setting) -> bool {
    [set.title.t(), set.help.t()].iter().any(|t| fuzzy(query, t).is_some_and(|m| m.ranges.len() <= help_limit(query))) || matches(query, &set.words())
}

fn title_marks(query: &str, text: &str) -> Vec<std::ops::Range<usize>> {
    help_marks(query, text)
}

fn help_marks(query: &str, text: &str) -> Vec<std::ops::Range<usize>> {
    if query.is_empty() {
        return Vec::new();
    }
    fuzzy(query, text).filter(|m| m.ranges.len() <= help_limit(query)).map(|m| m.ranges).unwrap_or_default()
}

fn indent(i: usize) -> f32 {
    if NESTED.contains(&SETTINGS[i].id) {
        INDENT
    } else {
        0.0
    }
}

fn number_dropdown(i: usize) -> bool {
    NUMBER_DROPDOWNS.contains(&SETTINGS[i].id)
}

fn is_dropdown(i: usize) -> bool {
    matches!(SETTINGS[i].kind, Kind::Choice(..)) || number_dropdown(i)
}

fn number_text(value: i64, unit: S, zero: Option<S>) -> String {
    match zero.filter(|_| value == 0) {
        Some(label) => label.t().to_owned(),
        None => format!("{value} {unit}"),
    }
}

fn switch_rect(r: Rect, s: impl Fn(f32) -> f32) -> Rect {
    Rect::new(r.right() - s(PAD) - s(44.0), r.y + (r.h - s(22.0)) / 2.0, s(44.0), s(22.0))
}

fn control_w(r: Rect, s: impl Fn(f32) -> f32) -> f32 {
    let room = r.w - 3.0 * s(PAD);
    (room * 0.42).clamp(s(CONTROL_MIN), s(CONTROL_MAX)).min((room - s(LABEL_MIN)).max(s(CONTROL_MIN)))
}

fn control_rect(r: Rect, s: impl Fn(f32) -> f32) -> Rect {
    let w = control_w(r, &s);
    Rect::new(r.right() - s(PAD) - w, r.y + (r.h - s(30.0)) / 2.0, w, s(30.0))
}

fn label_w(i: usize, r: Rect, s: impl Fn(f32) -> f32) -> f32 {
    let control = match SETTINGS[i].kind {
        Kind::Toggle(..) => switch_rect(r, &s).x,
        _ => control_rect(r, &s).x,
    };
    let restart = if RESTART.contains(&SETTINGS[i].id) { s(24.0) } else { 0.0 };
    (control - s(PAD) - restart - (r.x + s(PAD) + s(indent(i)))).max(0.0)
}

fn info_label_w(r: Rect, s: impl Fn(f32) -> f32) -> f32 {
    s(INFO_LABEL_W).min((r.w * 0.4).round())
}

fn scroll_thumb(list: Rect, content_h: f32, pos: f32, s: impl Fn(f32) -> f32) -> Option<Rect> {
    if content_h <= list.h {
        return None;
    }
    let th = (list.h * list.h / content_h).max(s(BAR_THUMB_MIN)).min(list.h);
    let frac = (pos / (content_h - list.h)).clamp(0.0, 1.0);
    Some(Rect::new(list.right() - s(BAR_W), list.y + (list.h - th) * frac, s(BAR_W), th))
}

const TIP_W: f32 = 380.0;

pub(super) fn wrap<'a, B: TextBackend>(g: &mut Gfx<B>, text: &'a str, font: FontStyle, max_w: f32) -> Vec<&'a str> {
    let mut lines = Vec::new();
    let (mut start, mut end) = (0, 0);
    for word in text.split_whitespace() {
        let at = word.as_ptr() as usize - text.as_ptr() as usize;
        let next = at + word.len();
        if end == start {
            start = at;
        } else if g.measure(&text[start..next], font) > max_w {
            lines.push(&text[start..end]);
            start = at;
        }
        end = next;
    }
    if end > start {
        lines.push(&text[start..end]);
    }
    lines
}

fn step_rects(r: Rect, s: impl Fn(f32) -> f32) -> (Rect, Rect) {
    let c = control_rect(r, &s);
    (Rect::new(c.x, c.y, s(34.0), c.h), Rect::new(c.right() - s(34.0), c.y, s(34.0), c.h))
}

fn remove_rect(r: Rect, s: impl Fn(f32) -> f32) -> Rect {
    Rect::new(r.right() - s(PAD) - s(28.0), r.y + (r.h - s(24.0)) / 2.0, s(28.0), s(24.0))
}

fn glyph_w(btn: &Btn, s: impl Fn(f32) -> f32) -> f32 {
    if matches!(btn, Btn::AddHome | Btn::AddCurrent | Btn::NewTag | Btn::NewEntry | Btn::NewSubmenu) {
        s(18.0)
    } else {
        0.0
    }
}

fn button_rects(line: &Line, r: Rect, fit: &Fit, s: impl Fn(f32) -> f32) -> Vec<Rect> {
    let h = s(CONTROL_H);
    let gap = s(BUTTON_GAP);
    let natural = |btn: &Btn| (fit.label(*btn) + s(BUTTON_PAD) + glyph_w(btn, &s)).ceil();
    match line {
        Line::Tag(_) | Line::Entry(_) => {
            let y = r.y + ((r.h - h) / 2.0).round();
            let mut right = r.right() - s(PAD);
            let mut out: Vec<Rect> = line
                .buttons()
                .iter()
                .rev()
                .map(|btn| {
                    let w = s(TAG_BUTTON_W).max(natural(btn));
                    right -= w;
                    let b = Rect::new(right, y, w, h);
                    right -= gap;
                    b
                })
                .collect();
            out.reverse();
            out
        }
        _ => {
            let left = r.x + if matches!(line, Line::Buttons(Some(_), _)) { s(32.0) } else { s(PAD) };
            let end = r.right() - s(PAD);
            let room = (end - left).max(s(BUTTON_W) / 2.0);
            let (mut x, mut y) = (left, r.y + s(BUTTON_GAP));
            line.buttons()
                .iter()
                .map(|btn| {
                    let w = s(BUTTON_W).min(room).max(natural(btn)).min(room);
                    if x > left && x + w > end {
                        (x, y) = (left, y + h + gap);
                    }
                    let b = Rect::new(x, y, w, h);
                    x += w + gap;
                    b
                })
                .collect()
        }
    }
}

fn buttons_height(line: &Line, w: f32, fit: &Fit, s: impl Fn(f32) -> f32) -> f32 {
    let bottom = button_rects(line, Rect::new(0.0, 0.0, w, 0.0), fit, &s).iter().map(|b| b.bottom()).fold(0.0, f32::max);
    bottom + s(BUTTON_GAP)
}

fn size_text(bytes: Option<u64>, decimal: bool) -> String {
    let Some(b) = bytes else { return S::SettingsPageLineNotMeasured.t().into() };
    let mut s = String::new();
    crate::fmt::size_in(&mut s, b, decimal);
    s
}

impl App {
    pub fn settings_open(&self) -> bool {
        self.settings.screen.is_some()
    }

    pub fn set_about(&mut self, about: About) {
        if self.about != about {
            self.about = about;
            self.layout();
            self.dirty = true;
        }
    }

    pub fn settings_page(&self) -> Option<Page> {
        self.settings.screen.as_ref().map(|s| s.page)
    }

    pub fn settings_lines(&self) -> Vec<String> {
        let Some(ui) = &self.settings.screen else { return Vec::new() };
        ui.lines
            .iter()
            .map(|l| match l {
                Line::Group(t) | Line::Note(t) => t.to_string(),
                Line::Setting(i) => SETTINGS[*i].id.to_owned(),
                Line::Off(i) => format!("off {}", SETTINGS[*i].id),
                Line::Path(i, k) => format!("{} #{k}", SETTINGS[*i].id),
                Line::Buttons(_, b) => b.iter().map(|b| b.label()).collect::<Vec<_>>().join(" | "),
                Line::Tag(k) => self.tag_defs().get(*k).map_or_else(String::new, |t| format!("tag {}", t.name)),
                Line::Entry(at) => self.user_line_text(*at),
                Line::MenuItem(row) => App::menu_line_text(row),
                Line::Info(label, value) => format!("{label}: {value}"),
            })
            .collect()
    }

    pub(super) fn open_settings(&mut self, page: Option<Page>) {
        if self.settings.screen.is_some() && page.is_none() {
            return self.close_settings();
        }
        self.nav.palette = None;
        let ui = self.settings.screen.get_or_insert_with(|| SettingsUi::new(page.unwrap_or(Page::Interface), self.scale));
        if let Some(p) = page {
            ui.page = p;
            ui.jump = Some((p, false));
            ui.search.edit.set("");
            ui.focus = usize::MAX;
        }
        self.effects.push(Effect::Settings(SettingsCmd::CacheSize));
        self.effects.push(Effect::Settings(SettingsCmd::RefreshSystemChanges));
        self.layout();
        self.dirty = true;
    }

    pub(super) fn close_settings(&mut self) {
        self.close_dropdown();
        self.settings.screen = None;
        self.layout();
        self.dirty = true;
    }

    pub(super) fn change_setting(&mut self, i: usize, v: Value) {
        if SETTINGS.get(i).is_some_and(|s| s.set(self, v)) {
            self.layout();
            self.dirty = true;
        }
    }

    pub(super) fn choose_setting(&mut self, i: usize, choice: usize) {
        if SETTINGS[i].id == "appearance.color_scheme" {
            self.set_system_scheme(false);
        }
        self.change_setting(i, Value::Choice(choice));
    }

    pub(super) fn paths_of(&self, i: usize) -> Vec<crate::location::Location> {
        match SETTINGS[i].get(self) {
            Value::Paths(p) => p,
            _ => Vec::new(),
        }
    }

    fn add_path(&mut self, i: usize, loc: crate::location::Location) {
        let mut paths = self.paths_of(i);
        if paths.contains(&loc) {
            return self.status(S::SettingsPageStatusPageListed.t());
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
            None => self.status(S::SettingsPageStatusNoFile.t()),
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
            None => self.status(NO_LOGS.t()),
        }
    }

    pub(super) fn open_log_folder(&mut self, now: Instant) {
        let Some(dir) = self.about.logs.clone() else { return self.status(NO_LOGS.t()) };
        if self.settings.screen.is_some() {
            self.close_settings();
        }
        self.dispatch(vec![Route { folder: Location::from_local(dir), select: Some(LOG_FILE.as_bytes().to_vec()) }], OpenTarget::NewTab, now);
    }

    pub(super) fn ask_export_settings(&mut self) {
        let text = self.export_path().unwrap_or_default();
        self.open_setting_editor(SettingPick::Export, &text);
    }

    pub(super) fn ask_import_settings(&mut self) {
        let t = self.tab();
        let focused = t.view.as_ref().and_then(|v| v.focus).and_then(|i| t.entry_location(i)).filter(|l| l.file_name().to_ascii_lowercase().ends_with(b".json"));
        let text = focused.and_then(|l| l.to_windows()).map(|p| p.display().into_owned()).or_else(|| self.export_path()).unwrap_or_default();
        self.open_setting_editor(SettingPick::Import, &text);
    }

    pub(super) fn ask_reset_settings(&mut self) {
        let items = vec![(S::SettingsPageDialogReset.t().to_owned(), S::SettingsPageDialogResetNote.t().to_owned()), (S::SettingsPageDialogCancel.t().to_owned(), String::new())];
        self.open_pick(Palette::new(Pick::Setting(SettingPick::Reset), items, ""));
    }

    pub(super) fn setting_choose(&mut self, pick: SettingPick, id: usize) {
        match pick {
            SettingPick::Reset if id == 0 => self.reset_settings(),
            SettingPick::ResetKeys if id == 0 => self.restore_all_keys(),
            SettingPick::RegisterArchive if id == 0 => self.effects.push(Effect::Settings(SettingsCmd::RegisterArchive)),
            SettingPick::Capture { action, old, chord } => self.capture_choose(action, old, chord, id),
            SettingPick::Font(i) => self.choose_font(i, id),
            _ => {}
        }
    }

    pub fn offer_archive_registration(&mut self) {
        self.settings.archive_offer_pending = true;
        self.dirty = true;
    }

    pub(super) fn show_pending_archive_offer(&mut self) {
        if self.settings.archive_offer_pending && self.dialogs.form.is_none() && self.nav.palette.is_none() {
            self.settings.archive_offer_pending = false;
            let items = vec![(S::SettingsPageSystemArchivePrompt.t().to_owned(), S::SettingsPageSystemArchivePromptNote.t().to_owned()), (S::SettingsPageDialogCancel.t().to_owned(), String::new())];
            self.open_pick(Palette::new(Pick::Setting(SettingPick::RegisterArchive), items, ""));
            self.effects.push(Effect::Settings(SettingsCmd::ArchiveOfferShown));
        }
    }

    pub(super) fn ask_reset_keys(&mut self) {
        let items = vec![(S::SettingsPageDialogResetKeys.t().to_owned(), S::SettingsPageDialogResetKeysNote.t().to_owned()), (S::SettingsPageDialogCancel.t().to_owned(), String::new())];
        self.open_pick(Palette::new(Pick::Setting(SettingPick::ResetKeys), items, ""));
    }

    pub(super) fn setting_input(&mut self, pick: SettingPick, text: &str) {
        if text == BROWSE {
            return self.browse_for(pick);
        }
        match pick {
            SettingPick::Text(i) => self.change_setting(i, Value::Text(text.to_owned())),
            SettingPick::Color(i) if Picker::Color.accepts(text) => self.change_setting(i, Value::Text(text.to_owned())),
            SettingPick::Color(_) => self.status(S::SettingsPageStatusBadColor.t()),
            SettingPick::AddPath(i) => match settings::parse_path(text) {
                Some(loc) => self.add_path(i, loc),
                None => self.status(S::SettingsPageStatusBadFolder.t()),
            },
            SettingPick::Export | SettingPick::Import => match self.caps.native_path(text) {
                Some(path) if pick == SettingPick::Export => self.effects.push(Effect::Settings(SettingsCmd::Export { path, text: self.export_text(), replace: false })),
                Some(path) => self.effects.push(Effect::Settings(SettingsCmd::Import(path))),
                None => self.status(S::SettingsPageStatusBadFile.t()),
            },
            SettingPick::Reset | SettingPick::ResetKeys | SettingPick::RegisterArchive | SettingPick::Capture { .. } | SettingPick::Font(_) => {}
        }
    }

    fn page_lines(&self, page: Page, out: &mut Vec<Line>) {
        for (i, _) in SETTINGS.iter().enumerate().filter(|(_, s)| s.page == page && page != Page::Menu && s.id != settings::LOG_LEVEL && s.id != settings::CHECK_UPDATES) {
            self.setting_lines(i, out);
        }
        match page {
            Page::Tags => {
                if self.tag_defs().is_empty() {
                    out.push(Line::Note(S::SettingsPageLineNoTags.t()));
                }
                out.extend((0..self.tag_defs().len()).map(Line::Tag));
                out.push(Line::Buttons(None, &[Btn::NewTag]));
            }
            Page::Advanced => {
                out.push(Line::Info(S::SettingsPageLineCache.t(), size_text(self.about.cache_bytes, self.style.decimal)));
                out.push(Line::Buttons(None, &[Btn::ClearCache]));
                out.push(Line::Group(S::SettingsPageLineSettingsFile.t()));
                out.push(Line::Info(S::SettingsPageLineFile.t(), self.about.file.as_ref().map_or_else(|| S::SettingsPageLineNone.t().to_owned(), |f| f.display().into_owned())));
                out.push(Line::Buttons(None, FILE_BUTTONS));
                out.push(Line::Buttons(None, &[Btn::Reset]));
                out.push(Line::Group(S::SettingsPageLineLogFile.t()));
                if let Some(i) = settings::index(settings::LOG_LEVEL) {
                    self.setting_lines(i, out);
                }
                out.push(Line::Info(S::SettingsPageLineFolder.t(), self.about.logs.as_ref().map_or_else(|| S::SettingsPageLineNone.t().to_owned(), |f| f.display().into_owned())));
                out.push(Line::Buttons(None, LOG_BUTTONS));
                out.push(Line::Group(S::SettingsPageSystemTitle.t()));
                match (self.about.system_integration_unavailable, &self.about.system_changes) {
                    (true, _) => out.push(Line::Note(S::SettingsPageSystemUnavailable.t())),
                    (false, Some(Ok(changes))) if changes.is_empty() => out.push(Line::Note(S::SettingsPageSystemEmpty.t())),
                    (false, Some(Ok(changes))) => {
                        out.extend(changes.iter().map(|(kind, target)| Line::Info(kind.t(), target.clone())));
                        out.push(Line::Buttons(None, &[Btn::UndoSystemChanges]));
                    }
                    (false, Some(Err(error))) => out.push(Line::Info(S::SettingsPageSystemReadFailed.t(), error.clone())),
                    (false, None) => out.push(Line::Note(S::SettingsPageSystemLoading.t())),
                }
            }
            Page::About => {
                out.push(Line::Info(S::SettingsPageLineVersion.t(), format!("Flit {}", env!("CARGO_PKG_VERSION"))));
                out.push(Line::Info(OS_NAME, if self.about.os.is_empty() { S::SettingsPageLineUnknown.t().into() } else { self.about.os.clone() }));
                out.push(Line::Buttons(None, &[Btn::CopyAbout]));
                if self.about.system_integration_unavailable {
                    out.push(Line::Note(S::SettingsPageSystemUnavailable.t()));
                } else if self.can_register_archives() {
                    out.push(Line::Buttons(None, &[Btn::RegisterArchive]));
                }
                if !self.about.missing.is_empty() {
                    out.push(Line::Group(S::SettingsPageToolMissing.t()));
                    out.extend(self.about.missing.iter().map(|m| Line::Info(m.tool, crate::tr!(S::SettingsPageToolInstall, what = m.purpose.t(), package = m.package))));
                }
                out.push(Line::Group(S::UpdatePageGroup.t()));
                if self.about.store_managed {
                    out.push(Line::Note(S::UpdateDialogMicrosoftStore.t()));
                } else {
                    if let Some(i) = settings::index(settings::CHECK_UPDATES) {
                        self.setting_lines(i, out);
                    }
                    out.push(Line::Buttons(None, &[Btn::CheckUpdates]));
                }
                out.push(Line::Group(S::SettingsPageLineHelp.t()));
                out.push(Line::Buttons(None, HELP_BUTTONS));
                out.push(Line::Buttons(None, LINK_BUTTONS));
                out.push(Line::Note(NO_TELEMETRY.t()));
            }
            Page::Menu => {
                out.push(Line::Group(S::MenuEditorOwn.t()));
                out.push(Line::Buttons(None, USER_BUTTONS));
                if self.settings.opts.user_menu.is_empty() {
                    out.push(Line::Note(S::UserMenuEmpty.t()));
                }
                out.extend(self.user_lines().into_iter().map(Line::Entry));
                if let Some(i) = settings::index(settings::MENU_KIND) {
                    self.setting_lines(i, out);
                }
                let rows = self.menu_rows();
                match rows.is_empty() {
                    true => out.push(Line::Note(S::MenuEditorEmpty.t())),
                    false => out.extend([Line::Note(S::MenuEditorHint.t()), Line::Note(S::MenuEditorKeys.t())]),
                }
                out.extend(rows.into_iter().map(Line::MenuItem));
                out.push(Line::Buttons(None, &[Btn::MenuReset]));
                out.push(Line::Group(S::MenuEditorOptions.t()));
                for (i, _) in SETTINGS.iter().enumerate().filter(|(_, s)| s.page == Page::Menu && s.id != settings::MENU_KIND) {
                    self.setting_lines(i, out);
                }
            }
            Page::Interface | Page::Colors | Page::Files | Page::Data | Page::Input | Page::Windows => {}
        }
    }

    fn can_register_archives(&self) -> bool {
        !self.caps.unix_paths && !self.about.store_managed && !self.about.system_integration_unavailable
    }

    fn setting_lines(&self, i: usize, out: &mut Vec<Line>) {
        if self.about.store_managed && matches!(SETTINGS[i].id, settings::AUTOSTART | settings::CHECK_UPDATES) {
            return;
        }
        if let Kind::Keys(..) | Kind::Strings(..) | Kind::Menu(..) = SETTINGS[i].kind {
            return;
        }
        if !SETTINGS[i].offered(self.caps) {
            return;
        }
        if !SETTINGS[i].enabled(self) {
            return out.push(Line::Off(i));
        }
        out.push(Line::Setting(i));
        if let Kind::Paths(..) = SETTINGS[i].kind {
            out.extend((0..self.paths_of(i).len()).map(|k| Line::Path(i, k)));
            out.push(Line::Buttons(Some(i), PATH_BUTTONS));
        }
    }

    fn search_lines(&self, query: &str, out: &mut Vec<Line>, starts: &mut Vec<Start>) {
        for page in Page::ALL {
            let start = out.len();
            for (i, _) in SETTINGS.iter().enumerate().filter(|(_, s)| s.page == page && option_matches(query, s)) {
                self.setting_lines(i, out);
            }
            let extra: &[&'static [Btn]] = match page {
                Page::Tags => &[&[Btn::NewTag]],
                Page::Advanced => &[&[Btn::ClearCache], FILE_BUTTONS, &[Btn::Reset], LOG_BUTTONS],
                Page::About if self.about.store_managed => &[&[Btn::CopyAbout], HELP_BUTTONS, LINK_BUTTONS],
                Page::About if self.can_register_archives() => &[&[Btn::CopyAbout], &[Btn::CheckUpdates], HELP_BUTTONS, LINK_BUTTONS, &[Btn::RegisterArchive]],
                Page::About => &[&[Btn::CopyAbout], &[Btn::CheckUpdates], HELP_BUTTONS, LINK_BUTTONS],
                _ => &[],
            };
            for b in extra.iter().filter(|b| b.iter().any(|b| matches(query, &b.label().to_lowercase()))) {
                out.push(Line::Buttons(None, b));
            }
            if out.len() > start {
                let matches = out[start..].iter().filter(|l| matches!(l, Line::Setting(_) | Line::Off(_) | Line::Buttons(None, _))).count();
                starts.push(Start { line: start, page, matches });
                out.insert(start, Line::Group(page.title()));
            }
        }
        if out.is_empty() {
            out.push(Line::Note(S::OptionsNoMatch.t()));
        }
    }

    fn jump_to(&mut self, page: Page) {
        self.hide_settings_tip();
        let Some(ui) = &mut self.settings.screen else { return };
        ui.jump = Some((page, true));
        self.layout();
        self.dirty = true;
    }

    pub(super) fn settings_search(&self) -> Option<&Field> {
        self.settings.screen.as_ref().map(|ui| &ui.search)
    }

    pub(super) fn settings_search_mut(&mut self) -> Option<&mut Field> {
        self.settings.screen.as_mut().map(|ui| &mut ui.search)
    }

    pub(super) fn settings_mouse_down(&mut self, h: Hit, (x, y): (f32, f32), button: MouseButton, clicks: u8, now: Instant) -> bool {
        let Hit::Settings(sh) = h else { return false };
        if self.settings.screen.is_none() {
            return false;
        }
        let clicks = if sh == SHit::Search && button == MouseButton::Left { self.text_clicks((x, y), clicks, now) } else { clicks };
        let Some(ui) = &mut self.settings.screen else { return false };
        if self.nav.palette.is_some() {
            self.nav.palette = None;
        }
        self.dirty = true;
        if sh == SHit::Search && button == MouseButton::Right {
            ui.zone = Zone::Search;
            self.text_field_menu(x, y);
            return true;
        }
        if button != MouseButton::Left {
            return true;
        }
        self.pressed = h;
        match sh {
            SHit::Search => {
                ui.zone = Zone::Search;
                ui.search.click(x, clicks, self.mods.shift);
            }
            SHit::Thumb => {
                let thumb = scroll_thumb(ui.list, ui.content_h, ui.scroll.pos, |v| (v * self.scale).round());
                self.drag = super::Drag::OptionsThumb { grab: thumb.map_or(0.0, |t| y - t.y) };
            }
            SHit::Track => {
                let down = scroll_thumb(ui.list, ui.content_h, ui.scroll.pos, |v| (v * self.scale).round()).is_some_and(|t| y >= t.bottom());
                let page = if down { ui.list.h } else { -ui.list.h };
                ui.scroll.by(page, ui.content_h - ui.list.h);
                self.layout();
            }
            SHit::Line(k) | SHit::Control(k) | SHit::Step(k, _) | SHit::Remove(k) | SHit::Check(k) | SHit::Star(k) | SHit::Fold(k) => {
                ui.zone = Zone::List;
                ui.focus = k;
                if matches!((sh, ui.lines.get(k)), (SHit::Line(_), Some(Line::MenuItem(row))) if row.depth == 0) {
                    self.drag = super::Drag::MenuMove { from: k, to: None };
                }
            }
            SHit::Button(k, b) => {
                ui.zone = Zone::List;
                (ui.focus, ui.sub) = (k, b);
            }
            SHit::Nav(k) => {
                (ui.zone, ui.nav_at) = (Zone::List, k);
            }
            _ => {}
        }
        true
    }

    pub(super) fn settings_click(&mut self, sh: SHit, x: f32, y: f32) {
        let Some(ui) = &mut self.settings.screen else { return };
        let line = |k: usize| ui.lines.get(k).cloned();
        match sh {
            SHit::Outside | SHit::Close => self.close_settings(),
            SHit::Describe => {
                self.settings.opts.describe = !self.settings.opts.describe;
                if let Some(ui) = &mut self.settings.screen {
                    ui.reveal = true;
                }
                self.layout();
            }
            SHit::Nav(k) => {
                if let Some(st) = ui.starts.get(k).copied() {
                    self.jump_to(st.page);
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
            SHit::Check(k) | SHit::Star(k) | SHit::Fold(k) => {
                if let Some(Line::MenuItem(row)) = line(k) {
                    match sh {
                        SHit::Check(_) => self.toggle_menu_item(&row.path),
                        SHit::Star(_) => self.pin_menu_item(&row.path),
                        _ => self.fold_menu_item(&row.path, None),
                    }
                }
            }
            SHit::Line(_) | SHit::Panel | SHit::Search | SHit::Thumb | SHit::Track => {}
        }
        self.dirty = true;
    }

    pub(super) fn drag_settings_thumb(&mut self, y: f32, grab: f32) {
        let sc = self.scale;
        let Some(ui) = &mut self.settings.screen else { return };
        let Some(t) = scroll_thumb(ui.list, ui.content_h, ui.scroll.pos, |v| (v * sc).round()) else { return };
        let frac = ((y - grab - ui.list.y) / (ui.list.h - t.h).max(1.0)).clamp(0.0, 1.0);
        ui.scroll.snap(frac * (ui.content_h - ui.list.h));
        self.layout();
        self.dirty = true;
    }

    pub(super) fn activate_setting(&mut self, i: usize, at: Option<(f32, f32)>) {
        if is_dropdown(i) {
            let anchor = self.settings.screen.as_ref().and_then(|ui| ui.lines.iter().position(|l| *l == Line::Setting(i)).map(|k| (k, ui.rects[k])));
            if let Some((k, r)) = anchor {
                self.show_setting_choices(i, Hit::Settings(SHit::Control(k)), control_rect(r, |v| self.s(v)));
            } else {
                let (x, y) = at.unwrap_or_default();
                let items = self.list_menu(self.setting_dropdown_entries(i));
                self.flit_menu(x, y, items);
            }
            return;
        }
        match (SETTINGS[i].kind, SETTINGS[i].get(self)) {
            (Kind::Toggle(..), Value::Bool(b)) => self.change_setting(i, Value::Bool(!b)),
            (Kind::Pick(Picker::Color, ..), Value::Text(t)) => self.open_setting_editor(SettingPick::Color(i), &t),
            (Kind::Pick(Picker::Font, ..), _) => self.pick_font(i),
            (Kind::Text(..), _) => self.edit_text_setting(i),
            (Kind::Paths(..), _) => self.open_setting_editor(SettingPick::AddPath(i), ""),
            _ => {}
        }
    }

    pub(super) fn show_setting_choices(&mut self, i: usize, owner: Hit, anchor: Rect) {
        self.dropdown_menu(owner, anchor, self.setting_dropdown_entries(i));
    }

    fn setting_dropdown_entries(&self, i: usize) -> Vec<(String, Cmd, bool)> {
        match (SETTINGS[i].kind, SETTINGS[i].get(self)) {
            (Kind::Choice(choices, ..), Value::Choice(cur)) => choices.iter().enumerate().map(|(k, (_, label))| (label.t().to_owned(), Cmd::Setting(i, k), k == cur)).collect(),
            (Kind::Number { min, max, step, unit, zero, .. }, Value::Number(cur)) if number_dropdown(i) => {
                let mut values: Vec<_> = (min..=max).step_by(step as usize).chain([max, cur]).collect();
                values.sort_unstable();
                values.dedup();
                values.into_iter().map(|n| (number_text(n, unit, zero), Cmd::SettingNumber(i, n), n == cur)).collect()
            }
            _ => Vec::new(),
        }
    }

    fn step_setting(&mut self, i: usize, up: bool) {
        let (Kind::Number { min, max, step, .. }, Value::Number(n)) = (SETTINGS[i].kind, SETTINGS[i].get(self)) else { return };
        let next = if up { (n / step + 1) * step } else { (n - 1).div_euclid(step) * step };
        self.change_setting(i, Value::Number(next.clamp(min, max)));
    }

    fn turn_setting(&mut self, i: usize, back: bool) {
        match (SETTINGS[i].kind, SETTINGS[i].get(self)) {
            (Kind::Number { .. }, _) if !number_dropdown(i) => self.step_setting(i, !back),
            (Kind::Toggle(..), Value::Bool(b)) if b == back => self.change_setting(i, Value::Bool(!back)),
            _ => {}
        }
    }

    fn press_button(&mut self, line: &Line, b: usize) {
        let Some(&btn) = line.buttons().get(b) else { return };
        if let Line::Entry(at) = line {
            return match btn {
                Btn::Up => self.raise_user_entry(*at),
                Btn::Edit => self.edit_user_entry(*at),
                _ => self.delete_user_entry(*at),
            };
        }
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
            (Btn::AddTyped, Some(i), _) => self.open_setting_editor(SettingPick::AddPath(i), ""),
            (Btn::NewTag, ..) => self.new_tag(false),
            (Btn::NewEntry, ..) => self.new_user_entry(false),
            (Btn::NewSubmenu, ..) => self.new_user_entry(true),
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
            (Btn::MenuReset, ..) => self.reset_menu(),
            (Btn::CheckUpdates, ..) => self.check_updates(true),
            (Btn::UndoSystemChanges, ..) => self.effects.push(Effect::Settings(SettingsCmd::UndoSystemChanges)),
            (Btn::RegisterArchive, ..) => self.effects.push(Effect::Settings(SettingsCmd::RegisterArchive)),
            (Btn::CopyAbout, ..) => {
                let text = format!("Flit {}\r\n{}", env!("CARGO_PKG_VERSION"), self.about.os);
                self.effects.push(Effect::SetClipboard(text));
                self.status(S::SettingsPageStatusCopied.t());
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
        let Some(ui) = &self.settings.screen else { return };
        let sub = ui.sub;
        match ui.lines.get(k).cloned() {
            Some(Line::Setting(i)) => self.activate_setting(i, None),
            Some(Line::Path(i, n)) => self.remove_path(i, n),
            Some(l @ (Line::Buttons(..) | Line::Tag(_) | Line::Entry(_))) => self.press_button(&l, sub),
            _ => {}
        }
    }

    pub(super) fn settings_key(&mut self, key: Key, m: Mods) -> bool {
        if self.nav.palette.is_some() {
            return false;
        }
        let closes = self.settings.keymap.runs(key, m, "options.open");
        self.hide_settings_tip();
        let Some(ui) = &mut self.settings.screen else { return false };
        self.dirty = true;
        let plain = !m.ctrl && !m.alt;
        let line = ui.lines.get(ui.focus).cloned();
        match key {
            Key::Escape if !ui.search.text().is_empty() => {
                ui.search.edit.set("");
                ui.focus = usize::MAX;
            }
            Key::Escape => return self.close_settings_handled(),
            _ if closes => return self.close_settings_handled(),
            Key::Char('F') if m.ctrl => {
                ui.zone = Zone::Search;
                ui.search.edit.select_all();
            }
            Key::Tab if !m.ctrl => ui.cycle_zone(m.shift),
            Key::PageUp | Key::PageDown if m.ctrl => {
                let at = Page::ALL.iter().position(|p| *p == ui.page).unwrap_or(0);
                let n = Page::ALL.len();
                ui.page = Page::ALL[if key == Key::PageUp { (at + n - 1) % n } else { (at + 1) % n }];
                ui.jump = Some((ui.page, true));
                ui.search.edit.set("");
                (ui.focus, ui.zone) = (usize::MAX, Zone::List);
            }
            Key::Up | Key::Down | Key::Home | Key::End if plain && ui.zone == Zone::Nav => {
                let last = ui.starts.len().saturating_sub(1);
                ui.nav_at = match key {
                    Key::Up => ui.nav_at.saturating_sub(1),
                    Key::Down => (ui.nav_at + 1).min(last),
                    Key::Home => 0,
                    _ => last,
                };
            }
            Key::Enter | Key::Space if plain && ui.zone == Zone::Nav => {
                if let Some(st) = ui.starts.get(ui.nav_at).copied() {
                    ui.jump = Some((st.page, true));
                }
            }
            Key::Right if plain && ui.zone == Zone::Nav => ui.zone = Zone::List,
            Key::Up | Key::Down if plain => {
                let step = if key == Key::Up { -1 } else { 1 };
                match (ui.searching(), key) {
                    (true, Key::Down) => {
                        ui.zone = Zone::List;
                        ui.focus = ui.first_focusable();
                    }
                    (true, _) => {}
                    (false, Key::Up) if ui.focusable(ui.focus, step).is_none() => ui.zone = Zone::Search,
                    _ => {
                        if let Some(k) = ui.focusable(ui.focus, step) {
                            ui.focus = k;
                        }
                    }
                }
                ui.sub = 0;
                ui.reveal = true;
            }
            Key::Enter if ui.searching() => {
                ui.zone = Zone::List;
                ui.focus = ui.first_focusable();
                ui.reveal = true;
            }
            _ if ui.searching() => {
                if let FieldKey::Edited { changed, action } = ui.search.key(key, m) {
                    if changed {
                        ui.focus = usize::MAX;
                        ui.scroll.snap(0.0);
                    }
                    match action {
                        EditAction::Copy(s) => self.effects.push(Effect::SetClipboard(s)),
                        EditAction::Paste => self.effects.push(Effect::Paste),
                        _ => {}
                    }
                }
            }
            Key::Backspace => {
                ui.zone = Zone::Search;
                let _ = ui.search.key(key, m);
            }
            _ if ui.zone == Zone::Nav => return true,
            Key::Up | Key::Down if m.alt && !m.ctrl && matches!(line, Some(Line::MenuItem(_))) => {
                if let Some(Line::MenuItem(row)) = line {
                    self.shift_menu_item(&row.path, key == Key::Down);
                    self.focus_menu_row(&row.path);
                }
            }
            Key::Space if plain && matches!(line, Some(Line::MenuItem(_))) => {
                if let Some(Line::MenuItem(row)) = line {
                    self.toggle_menu_item(&row.path);
                }
            }
            Key::Enter if plain && matches!(line, Some(Line::MenuItem(_))) => match line {
                Some(Line::MenuItem(row)) if row.submenu => self.fold_menu_item(&row.path, None),
                Some(Line::MenuItem(row)) => self.pin_menu_item(&row.path),
                _ => {}
            },
            Key::Left | Key::Right if plain && matches!(&line, Some(Line::MenuItem(row)) if row.submenu) => {
                if let Some(Line::MenuItem(row)) = line {
                    self.fold_menu_item(&row.path, Some(key == Key::Right));
                }
            }
            Key::Left | Key::Right if plain => match line {
                Some(Line::Setting(i)) => {
                    self.turn_setting(i, key == Key::Left);
                }
                Some(l @ (Line::Buttons(..) | Line::Tag(_) | Line::Entry(_))) => {
                    let n = l.count().max(1);
                    ui.sub = if key == Key::Left { (ui.sub + n - 1) % n } else { (ui.sub + 1) % n };
                }
                _ => {}
            },
            Key::Enter | Key::Space if plain => {
                let k = ui.focus;
                self.activate_line(k);
            }
            Key::Delete if plain => {
                if let Some(Line::Path(i, n)) = line {
                    self.remove_path(i, n);
                }
            }
            _ => return true,
        }
        self.layout();
        true
    }

    fn focus_menu_row(&mut self, path: &str) {
        self.layout();
        let Some(ui) = &mut self.settings.screen else { return };
        if let Some(k) = ui.lines.iter().position(|l| matches!(l, Line::MenuItem(row) if row.path == path)) {
            (ui.focus, ui.reveal) = (k, true);
            self.layout();
        }
    }

    fn menu_top_lines(&self) -> Vec<(usize, Rect)> {
        let Some(ui) = &self.settings.screen else { return Vec::new() };
        ui.lines.iter().zip(&ui.rects).enumerate().filter(|(_, (l, _))| matches!(l, Line::MenuItem(row) if row.depth == 0)).map(|(k, (_, r))| (k, *r)).collect()
    }

    pub(super) fn drag_menu_row(&mut self, from: usize, y: f32) {
        let to = self.menu_top_lines().iter().filter(|(_, r)| r.y + r.h / 2.0 < y).count();
        self.drag = super::Drag::MenuMove { from, to: Some(to) };
        self.dirty = true;
    }

    pub(super) fn drop_menu_row(&mut self, from: usize, to: Option<usize>) {
        let tops = self.menu_top_lines();
        let (Some(to), Some(at)) = (to, tops.iter().position(|(k, _)| *k == from)) else { return };
        let Some(Line::MenuItem(row)) = self.settings.screen.as_ref().and_then(|ui| ui.lines.get(from).cloned()) else { return };
        let target = if to > at { to - 1 } else { to };
        self.move_menu_item(&row.path, target);
        self.focus_menu_row(&row.path);
    }

    fn close_settings_handled(&mut self) -> bool {
        self.close_settings();
        true
    }

    pub(super) fn settings_insert(&mut self, s: &str) -> bool {
        if self.nav.palette.is_some() {
            return false;
        }
        let Some(ui) = &mut self.settings.screen else { return false };
        if s.is_empty() || (s == " " && !ui.searching()) {
            return true;
        }
        ui.zone = Zone::Search;
        ui.search.insert(s);
        ui.focus = usize::MAX;
        ui.scroll.snap(0.0);
        self.layout();
        self.dirty = true;
        true
    }

    pub(super) fn settings_pans_at(&self, x: f32, y: f32) -> bool {
        self.settings.screen.as_ref().is_some_and(|ui| ui.list.contains(x, y) && !ui.bar.contains(x, y))
    }

    pub(super) fn touch_pan_settings(&mut self, dy: f32) -> bool {
        self.hide_settings_tip();
        let Some(ui) = &mut self.settings.screen else { return false };
        let was = ui.scroll.pos;
        ui.scroll.snap((was - dy).clamp(0.0, (ui.content_h - ui.list.h).max(0.0)));
        let moved = ui.scroll.pos != was;
        if moved {
            self.layout();
        }
        moved
    }

    pub(super) fn settings_wheel(&mut self, dy: f32) -> bool {
        if !matches!(self.hover, Hit::Settings(_)) {
            return false;
        }
        let step = 3.0 * (ITEM_H * self.scale).round();
        self.hide_settings_tip();
        if let Some(ui) = &mut self.settings.screen {
            ui.scroll.by(-dy * step, ui.content_h - ui.list.h);
        }
        self.layout();
        self.dirty = true;
        true
    }

    pub(super) fn settings_tip(&mut self, x: f32, y: f32) {
        let blocked = self.nav.palette.is_some();
        let Some(ui) = &mut self.settings.screen else { return };
        let over = if blocked || !ui.list.contains(x, y) { None } else { ui.cut.iter().find(|(_, r)| r.contains(x, y)).map(|&(k, _)| k) };
        match (over, ui.tip) {
            (Some(k), Some((was, _))) if k == was => {}
            (Some(k), _) => {
                ui.tip = Some((k, (x, y)));
                self.dirty = true;
            }
            (None, Some(_)) => self.hide_settings_tip(),
            (None, None) => {}
        }
    }

    pub(super) fn hide_settings_tip(&mut self) {
        if let Some(ui) = self.settings.screen.as_mut().filter(|ui| ui.tip.is_some()) {
            ui.tip = None;
            self.dirty = true;
        }
    }
}

#[cfg(test)]
#[path = "../../test/ui/settings_page.rs"]
mod tests;
