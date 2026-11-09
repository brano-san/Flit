use super::actions::Chord;
use super::edit::EditAction;
use super::field::{Field, FieldKey};
use super::menu::{Cmd, Mark, MenuItem};
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
pub const SLIDERS: [&str; 3] = [settings::SPACING, settings::OPACITY, settings::CACHE];
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
}

impl Btn {
    const ALL: [Btn; 24] = [
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
            Line::Path(..) => room(ITEM_H, fit.body),
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
    Slide(usize),
    Thumb,
    Track,
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
            SettingPick::Text(_) => S::SettingsPagePickText.t(),
            SettingPick::AddPath(_) => S::SettingsPagePickAddPath.t(),
            SettingPick::Export => S::SettingsPagePickExport.t(),
            SettingPick::Import => S::SettingsPagePickImport.t(),
            SettingPick::Reset => S::SettingsPagePickReset.t(),
            SettingPick::ResetKeys => S::SettingsPagePickResetKeys.t(),
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
            SettingPick::Reset | SettingPick::ResetKeys | SettingPick::Capture { .. } => "",
            SettingPick::Color(_) => S::SettingsPageHintColor.t(),
            SettingPick::Font(_) => S::SettingsPageHintFont.t(),
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
    if NESTED.contains(&SETTINGS[i].id) { INDENT } else { 0.0 }
}

fn slider(i: usize) -> bool {
    SLIDERS.contains(&SETTINGS[i].id)
}

fn slider_track(r: Rect, s: impl Fn(f32) -> f32) -> Rect {
    let c = control_rect(r, &s);
    let x = c.x + s(56.0);
    Rect::new(x, c.y + c.h / 2.0 - s(2.0), c.right() - s(8.0) - x, s(4.0))
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

fn slide_value(i: usize, track: Rect, x: f32) -> Option<i64> {
    let Kind::Number { min, max, step, .. } = SETTINGS[i].kind else { return None };
    let frac = ((x - track.x) / track.w.max(1.0)).clamp(0.0, 1.0);
    let raw = min as f32 + frac * (max - min) as f32;
    Some(((raw / step as f32).round() as i64 * step).clamp(min, max))
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

fn paint_group<B: TextBackend>(g: &mut Gfx<B>, t: &Theme, r: Rect, title: &str, bold: FontStyle, s: impl Fn(f32) -> f32) {
    let mb = g.metrics(bold);
    g.text(r.x + s(PAD), (r.bottom() - s(10.0) - mb.descent).round(), title, bold, t.text, r.w - 2.0 * s(PAD));
}

fn step_rects(r: Rect, s: impl Fn(f32) -> f32) -> (Rect, Rect) {
    let c = control_rect(r, &s);
    (Rect::new(c.x, c.y, s(34.0), c.h), Rect::new(c.right() - s(34.0), c.y, s(34.0), c.h))
}

fn remove_rect(r: Rect, s: impl Fn(f32) -> f32) -> Rect {
    Rect::new(r.right() - s(PAD) - s(28.0), r.y + (r.h - s(24.0)) / 2.0, s(28.0), s(24.0))
}

fn glyph_w(btn: &Btn, s: impl Fn(f32) -> f32) -> f32 {
    if matches!(btn, Btn::AddHome | Btn::AddCurrent | Btn::NewTag | Btn::NewEntry | Btn::NewSubmenu) { s(18.0) } else { 0.0 }
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
                Line::Off(i) => format!("off {}", SETTINGS[*i].id),
                Line::Path(i, k) => format!("{} #{k}", SETTINGS[*i].id),
                Line::Buttons(_, b) => b.iter().map(|b| b.label()).collect::<Vec<_>>().join(" | "),
                Line::Tag(k) => self.tag_defs().get(*k).map_or_else(String::new, |t| format!("tag {}", t.name)),
                Line::Entry(at) => self.user_line_text(*at),
                Line::Info(label, value) => format!("{label}: {value}"),
            })
            .collect()
    }

    pub(super) fn open_settings(&mut self, page: Option<Page>) {
        if self.settings.is_some() && page.is_none() {
            return self.close_settings();
        }
        self.nav.palette = None;
        let ui = self.settings.get_or_insert_with(|| SettingsUi::new(page.unwrap_or(Page::Interface), self.scale));
        if let Some(p) = page {
            ui.page = p;
            ui.jump = Some((p, false));
            ui.search.edit.set("");
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
        if self.settings.is_some() {
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
        let focused = t.view.as_ref().and_then(|v| v.focus).map(|i| t.entry_location(i)).filter(|l| l.file_name().to_ascii_lowercase().ends_with(b".json"));
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
            SettingPick::Capture { action, old, chord } => self.capture_choose(action, old, chord, id),
            SettingPick::Font(i) => self.choose_font(i, id),
            _ => {}
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
            SettingPick::Reset | SettingPick::ResetKeys | SettingPick::Capture { .. } | SettingPick::Font(_) => {}
        }
    }

    fn page_lines(&self, page: Page, out: &mut Vec<Line>) {
        for (i, _) in SETTINGS.iter().enumerate().filter(|(_, s)| s.page == page && s.id != settings::LOG_LEVEL) {
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
            }
            Page::About => {
                out.push(Line::Info(S::SettingsPageLineVersion.t(), format!("Flit {}", env!("CARGO_PKG_VERSION"))));
                out.push(Line::Info(OS_NAME, if self.about.os.is_empty() { S::SettingsPageLineUnknown.t().into() } else { self.about.os.clone() }));
                out.push(Line::Buttons(None, &[Btn::CopyAbout]));
                out.push(Line::Group(S::SettingsPageLineHelp.t()));
                out.push(Line::Buttons(None, HELP_BUTTONS));
                out.push(Line::Buttons(None, LINK_BUTTONS));
                out.push(Line::Note(NO_TELEMETRY.t()));
            }
            Page::Interface => {
                out.push(Line::Group(S::UserMenuGroup.t()));
                if self.opts.user_menu.is_empty() {
                    out.push(Line::Note(S::UserMenuEmpty.t()));
                }
                out.extend(self.user_lines().into_iter().map(Line::Entry));
                out.push(Line::Buttons(None, USER_BUTTONS));
            }
            Page::Colors | Page::Files | Page::Data | Page::Input | Page::Windows => {}
        }
    }

    fn setting_lines(&self, i: usize, out: &mut Vec<Line>) {
        if let Kind::Keys(..) | Kind::Strings(..) | Kind::Menu(..) = SETTINGS[i].kind {
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
                Page::About => &[&[Btn::CopyAbout], HELP_BUTTONS, LINK_BUTTONS],
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

    pub(super) fn layout_settings(&mut self) {
        let Some(mut ui) = self.settings.take() else { return };
        let mut lines = Vec::new();
        let mut starts = Vec::new();
        match ui.query().to_owned().as_str() {
            "" => {
                for page in Page::ALL {
                    starts.push(Start { line: lines.len(), page, matches: 0 });
                    lines.push(Line::Group(page.title()));
                    self.page_lines(page, &mut lines);
                }
            }
            q => self.search_lines(q, &mut lines, &mut starts),
        }
        ui.lines = lines;
        ui.starts = starts;
        let (w, h, sc, describe) = (self.size.0, self.size.1, self.scale, self.opts.describe);
        let s = |v: f32| (v * sc).round();
        let left = self.panels.left;
        let area = (w - left).max(s(200.0));
        let room_w = area - 2.0 * s(EDGE);
        let want = (w / 2.0).round().max(s(MIN_W)).min(s(MAX_W));
        let full = ui.fit.nav_w(s);
        let nav_w = match room_w {
            r if r - full >= s(MIN_W) => full,
            r if r - s(RAIL_W) >= s(RAIL_CONTENT_MIN) => s(RAIL_W),
            _ => 0.0,
        };
        ui.rail = nav_w == s(RAIL_W);
        let pw = want.min(room_w - nav_w).max(s(320.0));
        let total = pw + nav_w;
        let mut y = 0.0;
        let mut offsets = Vec::with_capacity(ui.lines.len());
        for l in &ui.lines {
            let lh = match l {
                Line::Buttons(..) => buttons_height(l, pw, &ui.fit, s),
                _ => l.height(describe, &ui.fit, s),
            };
            offsets.push((y, lh));
            y += lh;
        }
        ui.content_h = y;
        let x = (left + (area - total) / 2.0).round().min(w - total).max(0.0);
        let fixed = s(8.0) + s(FIELD_H) + s(6.0) + s(HINT_H);
        let top = super::palette::popup_top(sc, h, fixed + s(SETTING_H) * 4.0);
        let list_h = (h - s(EDGE) - fixed - top).max(0.0);
        ui.search_rect = Rect::new(x + s(8.0), top + s(8.0), total - s(16.0) - 2.0 * (s(FIELD_H) + s(4.0)), s(FIELD_H));
        ui.describe = Rect::new(ui.search_rect.right() + s(4.0), ui.search_rect.y, s(FIELD_H), s(FIELD_H));
        ui.close = Rect::new(ui.describe.right() + s(4.0), ui.search_rect.y, s(FIELD_H), s(FIELD_H));
        let body = ui.search_rect.bottom() + s(6.0);
        ui.nav = Rect::new(x, body, nav_w, list_h);
        ui.list = Rect::new(x + nav_w, body, pw, list_h);
        ui.hints = Rect::new(x, ui.list.bottom(), total, s(HINT_H));
        ui.panel = Rect::new(x, top, total, ui.hints.bottom() - top);
        let max = (ui.content_h - ui.list.h).max(0.0);
        if let Some((p, smooth)) = ui.jump.take() {
            if let Some(&Start { line: k, .. }) = ui.starts.iter().find(|st| st.page == p) {
                let to = offsets[k].0.min(max);
                if smooth {
                    ui.scroll.target = to;
                } else {
                    ui.scroll.snap(to);
                }
                ui.held = Some((p, to));
                ui.focus = ui.focusable(k, 1).unwrap_or(k);
                ui.sub = 0;
            }
        }
        if ui.focus >= ui.lines.len() || !ui.lines[ui.focus].focusable() {
            ui.focus = ui.first_focusable();
            ui.sub = 0;
        }
        ui.sub = ui.sub.min(ui.lines.get(ui.focus).map_or(0, |l| l.count().saturating_sub(1)));
        let group_h = Line::Group("").height(describe, &ui.fit, s);
        if std::mem::take(&mut ui.reveal) {
            if let Some(&(fy, fh)) = offsets.get(ui.focus) {
                let lead = if matches!(ui.lines[ui.focus], Line::Group(_)) { 0.0 } else { group_h };
                if fy - lead < ui.scroll.target {
                    ui.scroll.target = (fy - lead).max(0.0);
                } else if fy + fh > ui.scroll.target + ui.list.h {
                    ui.scroll.target = fy + fh - ui.list.h;
                }
            }
        }
        ui.scroll.clamp(max);
        ui.page = match ui.held {
            Some((p, at)) if (ui.scroll.target - at).abs() < 0.5 => p,
            _ => {
                ui.held = None;
                let target = ui.scroll.target;
                ui.starts.iter().rev().find(|st| offsets[st.line].0 <= target + 0.5).or(ui.starts.first()).map_or(ui.page, |st| st.page)
            }
        };
        let n = ui.starts.len();
        ui.nav_rects = if nav_w > 0.0 && n > 0 {
            let item_h = ((list_h - 2.0 * s(NAV_PAD)) / n as f32).floor().clamp(s(NAV_ITEM_MIN), s(NAV_ITEM_H));
            (0..n).map(|k| Rect::new(ui.nav.x + s(NAV_PAD), ui.nav.y + s(NAV_PAD) + k as f32 * item_h, nav_w - 2.0 * s(NAV_PAD), item_h)).collect()
        } else {
            Vec::new()
        };
        ui.nav_at = ui.nav_at.min(n.saturating_sub(1));
        if ui.zone == Zone::Nav && ui.nav_rects.is_empty() {
            ui.zone = Zone::List;
        }
        ui.bar = if ui.content_h > ui.list.h { Rect::new(ui.list.right() - s(BAR_W), ui.list.y, s(BAR_W), ui.list.h) } else { Rect::default() };
        ui.rects = offsets.into_iter().map(|(ly, lh)| Rect::new(ui.list.x, ui.list.y + ly - ui.scroll.pos, ui.list.w, lh)).collect();
        ui.sticky = ui.starts.iter().rposition(|st| ui.rects[st.line].y <= ui.list.y + 0.5).filter(|&at| ui.rects[ui.starts[at].line].y < ui.list.y - 0.5).map(|at| {
            let line = ui.starts[at].line;
            let next = ui.starts.get(at + 1).map_or(f32::INFINITY, |st| ui.rects[st.line].y);
            (line, Rect::new(ui.list.x, ui.list.y.min(next - group_h), ui.list.w, group_h))
        });
        self.settings = Some(ui);
    }

    pub(super) fn settings_hit(&self, x: f32, y: f32) -> Option<Hit> {
        let ui = self.settings.as_ref()?;
        if self.in_title(y) {
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
        if ui.describe.contains(x, y) {
            return h(SHit::Describe);
        }
        if ui.search_rect.contains(x, y) {
            return h(SHit::Search);
        }
        if let Some(k) = ui.nav_rects.iter().position(|r| r.contains(x, y)) {
            return h(SHit::Nav(k));
        }
        if !ui.list.contains(x, y) {
            return h(SHit::Panel);
        }
        if ui.bar.contains(x, y) {
            let on_thumb = scroll_thumb(ui.list, ui.content_h, ui.scroll.pos, s).is_some_and(|t| y >= t.y && y < t.bottom());
            return h(if on_thumb { SHit::Thumb } else { SHit::Track });
        }
        if ui.sticky.is_some_and(|(_, r)| r.contains(x, y)) {
            return h(SHit::Panel);
        }
        let Some(k) = ui.rects.iter().position(|r| r.contains(x, y)) else { return h(SHit::Panel) };
        let (r, line) = (ui.rects[k], &ui.lines[k]);
        if let Some(b) = button_rects(line, r, &ui.fit, s).iter().position(|b| b.contains(x, y)) {
            return h(SHit::Button(k, b));
        }
        match line {
            Line::Setting(i) => match SETTINGS[*i].kind {
                Kind::Toggle(..) if switch_rect(r, s).contains(x, y) => h(SHit::Control(k)),
                Kind::Number { .. } if slider(*i) => {
                    let grip = slider_track(r, s).inset(-s(8.0), -s(10.0));
                    if grip.contains(x, y) { h(SHit::Slide(k)) } else { h(SHit::Line(k)) }
                }
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

    pub(super) fn settings_a11y(&self, t: &mut Tree, targets: &mut Vec<(u64, Hit)>) -> Option<usize> {
        let ui = self.settings.as_ref()?;
        let s = |v: f32| (v * self.scale).round();
        let at = Hit::Settings;
        let d = t.add(0, SETTINGS_ID, Role::Dialog, S::SettingsPageLineTitle.t(), ui.panel);
        put(t, targets, d, at(SHit::Close), Role::Button, S::A11yButtonClose.t(), ui.close);
        let search = put(t, targets, d, at(SHit::Search), Role::Edit, S::SettingsPageLineSearch.t(), ui.search_rect);
        t.nodes[search].value = ui.search.text().to_owned();
        let searching = !ui.query().is_empty();
        let describe = put(t, targets, d, at(SHit::Describe), Role::Button, S::OptionsDescriptions.t(), ui.describe);
        t.nodes[describe].selected = self.opts.describe;
        let mut focus = if ui.searching() { search } else { d };
        if !ui.nav_rects.is_empty() {
            let nav = t.add(d, CATEGORIES_ID, Role::List, S::SettingsPageLineCategories.t(), ui.nav);
            for (k, (&r, st)) in ui.nav_rects.iter().zip(&ui.starts).enumerate() {
                let n = put(t, targets, nav, at(SHit::Nav(k)), Role::ListItem, st.page.title(), r);
                t.nodes[n].selected = st.page == ui.page;
                if searching {
                    t.nodes[n].value = crate::trn!(P::SettingsPageMatches, st.matches);
                }
                if ui.zone == Zone::Nav && k == ui.nav_at {
                    focus = n;
                }
            }
        }
        let list = t.add(d, LIST_ID, Role::Group, if searching { S::SettingsPageLineSearch.t() } else { ui.page.title() }, ui.list);
        let mut section = list;
        for (k, (line, &r)) in ui.lines.iter().zip(&ui.rects).enumerate() {
            if r.bottom() < ui.list.y || r.y > ui.list.bottom() {
                continue;
            }
            let id = leaf(at(SHit::Line(k)));
            let row = match line {
                Line::Group(title) => {
                    section = t.add(list, id, Role::Group, *title, r);
                    None
                }
                Line::Note(text) => Some(t.add(section, id, Role::Text, *text, r)),
                Line::Info(label, value) => {
                    let n = t.add(section, id, Role::Text, *label, r);
                    t.nodes[n].value = value.clone();
                    Some(n)
                }
                Line::Setting(i) => {
                    let set = &SETTINGS[*i];
                    let (role, hit) = match set.kind {
                        Kind::Toggle(..) => (Role::CheckBox, SHit::Control(k)),
                        Kind::Choice(..) => (Role::ComboBox, SHit::Control(k)),
                        Kind::Text(..) | Kind::Pick(..) => (Role::Button, SHit::Control(k)),
                        _ => (Role::Group, SHit::Line(k)),
                    };
                    let n = put(t, targets, section, at(hit), role, set.title.t(), r);
                    t.nodes[n].help = if RESTART.contains(&set.id) { format!("{} {}", set.help.t(), S::OptionsRestart.t()) } else { set.help.t().to_owned() };
                    t.nodes[n].value = self.setting_value_text(*i);
                    t.nodes[n].selected = set.get(self) == Value::Bool(true);
                    if let (Kind::Number { .. }, false) = (set.kind, slider(*i)) {
                        let (minus, plus) = step_rects(r, s);
                        put(t, targets, n, at(SHit::Step(k, false)), Role::Button, S::A11yButtonDecrease.t(), minus);
                        put(t, targets, n, at(SHit::Step(k, true)), Role::Button, S::A11yButtonIncrease.t(), plus);
                    }
                    Some(n)
                }
                Line::Off(i) => {
                    let set = &SETTINGS[*i];
                    let role = match set.kind {
                        Kind::Toggle(..) => Role::CheckBox,
                        Kind::Choice(..) => Role::ComboBox,
                        _ => Role::Group,
                    };
                    let n = t.add(section, id, role, set.title.t(), r);
                    t.nodes[n].help = self.setting_help(line).into_owned();
                    t.nodes[n].value = self.setting_value_text(*i);
                    t.nodes[n].selected = set.get(self) == Value::Bool(true);
                    t.nodes[n].enabled = false;
                    Some(n)
                }
                Line::Path(i, n) => {
                    let p = t.add(section, id, Role::Text, self.paths_of(*i).get(*n).map(settings::path_text).unwrap_or_default(), r);
                    put(t, targets, p, at(SHit::Remove(k)), Role::Button, S::A11yButtonRemove.t(), remove_rect(r, s));
                    Some(p)
                }
                Line::Tag(n) => Some(t.add(section, id, Role::Group, self.tag_defs().get(*n).map_or("", |tag| tag.name.as_str()), r)),
                Line::Entry(e) => Some(t.add(section, id, Role::Group, self.user_entry_name(*e), r)),
                Line::Buttons(..) => None,
            };
            let mut subs = Vec::new();
            for (b, br) in button_rects(line, r, &ui.fit, s).into_iter().enumerate() {
                let name = line.buttons().get(b).map_or("", |btn| btn.label());
                subs.push(put(t, targets, row.unwrap_or(section), at(SHit::Button(k, b)), Role::Button, name, br));
            }
            if ui.zone == Zone::List && k == ui.focus {
                focus = subs.get(ui.sub).copied().or(row).unwrap_or(focus);
            }
        }
        Some(focus)
    }

    pub(super) fn settings_a11y_focus(&mut self, sh: SHit) {
        let Some(ui) = &mut self.settings else { return };
        match sh {
            SHit::Search => ui.zone = Zone::Search,
            SHit::Nav(k) => (ui.zone, ui.nav_at) = (Zone::Nav, k),
            SHit::Line(k) | SHit::Control(k) | SHit::Slide(k) | SHit::Step(k, _) | SHit::Remove(k) => (ui.zone, ui.focus, ui.sub) = (Zone::List, k, 0),
            SHit::Button(k, b) => (ui.zone, ui.focus, ui.sub) = (Zone::List, k, b),
            _ => {}
        }
    }

    pub(super) fn settings_ime_area(&self) -> Option<Rect> {
        self.settings.as_ref().filter(|s| s.searching() && self.nav.palette.is_none()).map(|s| s.search_rect)
    }

    pub(super) fn settings_nav_tip(&self, k: usize) -> Option<(Rect, String)> {
        let ui = self.settings.as_ref().filter(|ui| ui.rail)?;
        let (r, st) = (*ui.nav_rects.get(k)?, ui.starts.get(k)?);
        let text = match ui.query() {
            "" => st.page.title().to_owned(),
            _ => format!("{} · {}", st.page.title(), crate::trn!(P::SettingsPageMatches, st.matches)),
        };
        Some((r, text))
    }

    fn jump_to(&mut self, page: Page) {
        self.hide_settings_tip();
        let Some(ui) = &mut self.settings else { return };
        ui.jump = Some((page, true));
        self.layout();
        self.dirty = true;
    }

    pub(super) fn settings_mouse_down(&mut self, h: Hit, (x, y): (f32, f32), button: MouseButton, clicks: u8) -> bool {
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
                ui.zone = Zone::Search;
                ui.search.click(x, clicks);
            }
            SHit::Slide(k) => {
                (ui.zone, ui.focus) = (Zone::List, k);
                self.drag = super::Drag::Slide(k);
                self.slide_setting(k, x);
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
            SHit::Line(k) | SHit::Control(k) | SHit::Step(k, _) | SHit::Remove(k) => {
                ui.zone = Zone::List;
                ui.focus = k;
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
        let Some(ui) = &mut self.settings else { return };
        let line = |k: usize| ui.lines.get(k).cloned();
        match sh {
            SHit::Outside | SHit::Close => self.close_settings(),
            SHit::Describe => {
                self.opts.describe = !self.opts.describe;
                if let Some(ui) = &mut self.settings {
                    ui.reveal = true;
                }
                self.layout();
            }
            SHit::Slide(k) => self.slide_setting(k, x),
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
            SHit::Line(_) | SHit::Panel | SHit::Search | SHit::Thumb | SHit::Track => {}
        }
        self.dirty = true;
    }

    pub(super) fn slide_setting(&mut self, k: usize, x: f32) {
        let Some(ui) = &self.settings else { return };
        let (Some(&Line::Setting(i)), Some(&r)) = (ui.lines.get(k), ui.rects.get(k)) else { return };
        let track = slider_track(r, |v| (v * self.scale).round());
        if let Some(v) = slide_value(i, track, x).filter(|&v| SETTINGS[i].get(self) != Value::Number(v)) {
            self.change_setting(i, Value::Number(v));
        }
        self.dirty = true;
    }

    pub(super) fn drag_settings_thumb(&mut self, y: f32, grab: f32) {
        let sc = self.scale;
        let Some(ui) = &mut self.settings else { return };
        let Some(t) = scroll_thumb(ui.list, ui.content_h, ui.scroll.pos, |v| (v * sc).round()) else { return };
        let frac = ((y - grab - ui.list.y) / (ui.list.h - t.h).max(1.0)).clamp(0.0, 1.0);
        ui.scroll.snap(frac * (ui.content_h - ui.list.h));
        self.layout();
        self.dirty = true;
    }

    pub(super) fn activate_setting(&mut self, i: usize, at: Option<(f32, f32)>) {
        match (SETTINGS[i].kind, SETTINGS[i].get(self)) {
            (Kind::Toggle(..), Value::Bool(b)) => self.change_setting(i, Value::Bool(!b)),
            (Kind::Choice(choices, ..), Value::Choice(cur)) => {
                let entries = choices.iter().enumerate().map(|(k, (_, label))| (label.t().to_owned(), Cmd::Setting(i, k), k == cur)).collect();
                let items = self.list_menu(entries).into_iter().map(|mut it| {
                    if let MenuItem::Command { mark, .. } = &mut it {
                        *mark = Mark::Radio;
                    }
                    it
                });
                let (x, y) = self.menu_spot(i, at);
                let k = self.settings.as_ref().and_then(|ui| ui.lines.iter().position(|l| *l == Line::Setting(i)));
                match k.zip(self.settings.as_ref()).map(|(k, ui)| (k, control_rect(ui.rects[k], |v| (v * self.scale).round()))) {
                    Some((k, r)) => {
                        self.button_menu(Hit::Settings(SHit::Control(k)), r.x, r.bottom() + self.s(4.0), items.collect());
                        self.menu_right = Some(r.right());
                    }
                    None => self.flit_menu(x, y, items.collect()),
                }
            }
            (Kind::Pick(Picker::Color, ..), Value::Text(t)) => self.open_setting_editor(SettingPick::Color(i), &t),
            (Kind::Pick(Picker::Font, ..), _) => self.pick_font(i),
            (Kind::Text(..), _) => self.edit_text_setting(i),
            (Kind::Paths(..), _) => self.open_setting_editor(SettingPick::AddPath(i), ""),
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
        let Some(ui) = &self.settings else { return };
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
        let closes = self.keymap.runs(key, m, "options.open");
        self.hide_settings_tip();
        let Some(ui) = &mut self.settings else { return false };
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

    fn close_settings_handled(&mut self) -> bool {
        self.close_settings();
        true
    }

    pub(super) fn settings_insert(&mut self, s: &str) -> bool {
        if self.nav.palette.is_some() {
            return false;
        }
        let Some(ui) = &mut self.settings else { return false };
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

    pub(super) fn settings_wheel(&mut self, dy: f32) -> bool {
        if !matches!(self.hover, Hit::Settings(_)) {
            return false;
        }
        let step = 3.0 * (ITEM_H * self.scale).round();
        self.hide_settings_tip();
        if let Some(ui) = &mut self.settings {
            ui.scroll.by(-dy * step, ui.content_h - ui.list.h);
        }
        self.layout();
        self.dirty = true;
        true
    }

    fn setting_help(&self, line: &Line) -> std::borrow::Cow<'static, str> {
        match line {
            Line::Off(i) if settings::NEEDS_BUILTIN_ARCHIVES.contains(&SETTINGS[*i].id) => format!("{} {}", S::SettingsPageLineNeedsBuiltinArchives.t(), SETTINGS[*i].help.t()).into(),
            Line::Off(i) => SETTINGS[*i].help.t().into(),
            Line::Setting(i) => SETTINGS[*i].help.t().into(),
            _ => "".into(),
        }
    }

    fn setting_value_text(&self, i: usize) -> String {
        match (SETTINGS[i].kind, SETTINGS[i].get(self)) {
            (Kind::Choice(c, ..), Value::Choice(k)) => c.get(k).map_or("", |c| c.1.t()).to_owned(),
            (Kind::Number { zero: Some(off), .. }, Value::Number(0)) => off.t().to_owned(),
            (Kind::Number { unit, .. }, Value::Number(n)) => format!("{n} {unit}"),
            (Kind::Text(..), _) => self.text_value(i),
            (Kind::Paths(..), Value::Paths(p)) => crate::trn!(P::SettingsPagePages, p.len()),
            (Kind::Pick(..), _) => self.pick_text(i).unwrap_or_default(),
            _ => String::new(),
        }
    }

    pub(super) fn fit_settings<B: TextBackend>(&mut self, g: &mut Gfx<B>) {
        if self.settings.is_none() {
            return;
        }
        let font = self.ui_font(13.0);
        let fit = Fit {
            body: line_h(g.metrics(font)),
            small: line_h(g.metrics(self.ui_font(12.0))),
            bold: line_h(g.metrics(FontStyle::new(FontFamily::Ui, 14.0 * self.scale, true))),
            labels: Btn::ALL.map(|b| g.measure(b.label(), font)),
            titles: Page::ALL.iter().map(|p| g.measure(p.title(), font)).fold(0.0, f32::max),
        };
        if let Some(ui) = self.settings.as_mut().filter(|ui| ui.fit != fit) {
            ui.fit = fit;
            self.layout_settings();
        }
    }

    pub(super) fn draw_settings<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme) {
        let Some(mut ui) = self.settings.take() else { return };
        let open = match self.menu_owner {
            Some(Hit::Settings(SHit::Control(k))) if self.own_menu_open() => Some(k),
            _ => None,
        };
        if ui.turn.aim(open, self.animations, self.last_frame.unwrap_or_else(Instant::now)) {
            self.motion = true;
        }
        ui.knob_motion = false;
        if ui.lines.iter().any(|l| matches!(l, Line::Entry(_))) {
            self.request_user_line_images(g);
        }
        self.paint_settings(g, t, &mut ui);
        self.motion |= ui.knob_motion;
        self.settings = Some(ui);
    }

    fn paint_settings<B: TextBackend>(&self, g: &mut Gfx<B>, t: &Theme, ui: &mut SettingsUi) {
        let sc = self.scale;
        let s = |v: f32| (v * sc).round();
        let (font, small, bold) = (self.ui_font(13.0), self.ui_font(12.0), FontStyle::new(FontFamily::Ui, 14.0 * sc, true));
        let icons = self.icon_font(10.0);
        let (s4, s6, s8) = (s(4.0), s(6.0), s(8.0));
        let p = ui.panel;
        Paint::new(g, t, sc).frame(p);
        let (mf, ms) = (g.metrics(font), g.metrics(small));
        let mid = |r: Rect, m: crate::text::FontMetrics| (r.y + (r.h + m.ascent - m.descent) / 2.0).round();
        let hover = self.hover;
        let query = ui.query().to_owned();
        let describe = self.opts.describe;
        let sr = ui.search_rect;
        let focused_search = ui.searching() && self.active && self.nav.palette.is_none();
        Paint::new(g, t, sc).field(sr, focused_search);
        centered(g, Rect::new(sr.x + s4, sr.y, s(24.0), sr.h), G_SEARCH, icons, t.text_dim);
        let inner = Rect::new(sr.x + s(32.0), sr.y, sr.w - s(40.0), sr.h);
        ui.search.draw(g, t, inner, font, focused_search, S::OptionsPlaceholder.t(), sc, Some(&self.preedit));
        let big_icons = self.icon_font(12.0);
        Paint::new(g, t, sc).toggle_button(ui.describe, G_DESCRIBE, big_icons, describe, self.lit(Hit::Settings(SHit::Describe)));
        Paint::new(g, t, sc).icon_button(ui.close, G_CLOSE, icons, self.button(Hit::Settings(SHit::Close), false, true), t.text_dim);
        let hints = ui.hints;
        {
            let mut w = Paint::new(g, t, sc);
            w.separator(hints.x + s(12.0), hints.y, hints.w - s(24.0));
            let keys = [S::WidgetsKeyUp.t(), S::WidgetsKeyDown.t()];
            let all: [(&[&str], &str); 4] = [(&[S::WidgetsKeyTab.t()], S::SettingsPageLineSwitchArea.t()), (&keys, S::WidgetsHintNavigate.t()), (&[S::WidgetsKeyEnter.t()], S::WidgetsHintUse.t()), (&[S::WidgetsKeyEscape.t()], S::WidgetsHintDismiss.t())];
            w.hints_within(hints, &all[usize::from(ui.nav_rects.is_empty())..], small);
        }
        self.paint_settings_nav(g, t, ui, (font, small), (mf, ms));
        let list = ui.list;
        g.dl.set_clip(Some(list));
        let mut cut = Vec::new();
        let list_focused = ui.zone == Zone::List && self.active && self.nav.palette.is_none();
        for (k, (line, &r)) in ui.lines.iter().zip(&ui.rects).enumerate() {
            if r.bottom() < list.y || r.y > list.bottom() {
                continue;
            }
            let focused = list_focused && k == ui.focus;
            let row = Rect::new(r.x + s4, r.y + 1.0, r.w - s8, r.h - 2.0);
            if line.focusable() && !matches!(line, Line::Buttons(..)) {
                let lit = self.lit(Hit::Settings(SHit::Line(k)));
                if lit > 0.0 {
                    g.dl.round_rect(row, super::widgets::faded(t.hover, lit), s6);
                }
                if focused {
                    g.dl.outline(row, t.accent, s6, sc.round().max(1.0));
                }
            }
            match line {
                Line::Group(title) => paint_group(g, t, r, title, bold, s),
                Line::Note(text) => {
                    g.text(r.x + s(PAD), mid(r, mf), text, font, t.text_dim, r.w - 2.0 * s(PAD));
                }
                Line::Info(label, value) => {
                    let lw = info_label_w(r, s);
                    g.text(r.x + s(PAD), mid(r, ms), label, small, t.text_dim, lw);
                    g.text(r.x + s(PAD) + lw + s(10.0), mid(r, ms), value, small, t.text, r.w - s(PAD) * 2.0 - lw - s(10.0));
                }
                Line::Setting(i) | Line::Off(i) => {
                    let set = &SETTINGS[*i];
                    let tx = r.x + s(PAD) + s(indent(*i));
                    let restart = RESTART.contains(&set.id);
                    let text_w = label_w(*i, r, s);
                    let (tm, hm) = (title_marks(&query, set.title.t()), help_marks(&query, set.help.t()));
                    let (ty, help_y) = if describe { text_baselines(r, mf, ms, s) } else { (mid(r, mf), 0.0) };
                    Paint::new(g, t, sc).marked((tx, ty), set.title.t(), &tm, font, (t.text, t.accent), text_w);
                    if describe {
                        let help = self.setting_help(line);
                        let marks = if matches!(line, Line::Setting(_)) { hm } else { Vec::new() };
                        Paint::new(g, t, sc).marked((tx, help_y), &help, &marks, small, (t.text_dim, t.accent), text_w);
                        if g.measure(&help, small) > text_w {
                            cut.push((k, Rect::new(tx, help_y - ms.ascent, text_w, ms.ascent + ms.descent)));
                        }
                    }
                    if restart {
                        let c = control_rect(r, s);
                        centered(g, Rect::new(c.x - s(24.0), r.y, s(20.0), r.h), G_RESTART, icons, t.text_dim);
                    }
                    let value = self.setting_value_text(*i);
                    match (set.kind, set.get(self)) {
                        (Kind::Number { min, max, .. }, Value::Number(n)) if slider(*i) => {
                            let c = control_rect(r, s);
                            g.text(c.x, mid(c, ms), &value, small, t.text_dim, s(52.0));
                            let frac = if max > min { (n - min) as f32 / (max - min) as f32 } else { 0.0 };
                            let hot = if self.drag == super::Drag::Slide(k) { 1.0 } else { self.lit(Hit::Settings(SHit::Slide(k))) };
                            Paint::new(g, t, sc).slider(slider_track(r, s), frac, hot);
                        }
                        (Kind::Toggle(..), Value::Bool(on)) => {
                            let hot = self.lit(Hit::Settings(SHit::Control(k)));
                            let to = if on { 1.0 } else { 0.0 };
                            let now = self.last_frame.unwrap_or_else(Instant::now);
                            let knob = match ui.knobs.iter().position(|(x, _)| x == i) {
                                Some(n) => n,
                                None => {
                                    ui.knobs.push((*i, super::widgets::Tween::at(to)));
                                    ui.knobs.len() - 1
                                }
                            };
                            ui.knob_motion |= ui.knobs[knob].1.aim(to, self.animations, now, KNOB);
                            Paint::new(g, t, sc).switch(switch_rect(r, s), ui.knobs[knob].1.value(now, KNOB), hot, self.icon_font(8.0));
                        }
                        (Kind::Choice(..), _) => {
                            let h = Hit::Settings(SHit::Control(k));
                            let angle = ui.turn.angle(k, self.last_frame.unwrap_or_else(Instant::now));
                            Paint::new(g, t, sc).picker(control_rect(r, s), &value, font, angle, self.menu_open_from(h), hover == h);
                        }
                        (Kind::Text(..) | Kind::Pick(..), _) => {
                            let c = control_rect(r, s);
                            g.dl.round_rect(c, crate::theme::mix(t.input_bg, t.button_hover, self.lit(Hit::Settings(SHit::Control(k)))), s4);
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
                            let lit = [false, true].map(|up| self.lit(Hit::Settings(SHit::Step(k, up))));
                            Paint::new(g, t, sc).stepper(control_rect(r, s), &value, font, icons, lit);
                        }
                        _ => {
                            let c = control_rect(r, s);
                            let vw = g.measure(&value, small).min(c.w);
                            g.text(c.right() - vw, mid(r, ms), &value, small, t.text_dim, c.w);
                        }
                    }
                    if let Line::Off(_) = line {
                        g.dl.rect(row, t.bg.alpha(0xA0));
                    }
                }
                Line::Path(i, n) => {
                    let text = self.paths_of(*i).get(*n).map(settings::path_text).unwrap_or_default();
                    g.text(r.x + s(32.0), mid(r, mf), &text, font, t.text, r.w - s(PAD) * 2.0 - s(60.0));
                    let rr = remove_rect(r, s);
                    let lit = self.lit(Hit::Settings(SHit::Remove(k)));
                    if lit > 0.0 {
                        g.dl.round_rect(rr, super::widgets::faded(t.button_hover, lit), s4);
                    }
                    centered(g, rr, G_CLOSE, self.icon_font(8.0), t.text_dim);
                }
                Line::Tag(n) => {
                    if let Some(tag) = self.tag_defs().get(*n) {
                        let dot = s(12.0);
                        g.dl.round_rect(Rect::new(r.x + s(PAD), r.y + (r.h - dot) / 2.0, dot, dot), Color::rgb(tag.color), dot / 2.0);
                        let count = self.tags().count(*n);
                        let label = crate::trn!(P::SettingsPageTagItems, count, color = tags::color_name(tag.color));
                        let limit = button_rects(line, r, &ui.fit, s).first().map_or(r.right(), |b| b.x) - s(BUTTON_GAP);
                        let nx = r.x + s(PAD) + dot + s(10.0);
                        let nw = g.text(nx, mid(r, mf), &tag.name, font, t.text, s(220.0).min(limit - nx).max(0.0));
                        let lx = nx + nw + s(14.0);
                        g.text(lx, mid(r, ms), &label, small, t.text_dim, s(220.0).min(limit - lx).max(0.0));
                    }
                }
                Line::Entry(at) => self.draw_user_line(g, t, r, *at, (font, small, icons)),
                Line::Buttons(..) => {}
            }
            for (b, (br, btn)) in button_rects(line, r, &ui.fit, s).into_iter().zip(line.buttons()).enumerate() {
                let hot = self.lit(Hit::Settings(SHit::Button(k, b)));
                let danger = matches!(btn, Btn::Reset | Btn::Delete);
                g.dl.round_rect(br, crate::theme::mix(t.button_hover, t.button_pressed, hot), s4);
                g.dl.outline(br, if focused && ui.sub == b { t.accent } else { t.input_border }, s4, if focused && ui.sub == b { sc.round().max(1.0) } else { 1.0 });
                let glyph_w = glyph_w(btn, s);
                if glyph_w > 0.0 {
                    centered(g, Rect::new(br.x + s(8.0), br.y, glyph_w, br.h), G_PLUS, self.icon_font(9.0), t.accent);
                }
                let lw = g.measure(btn.label(), font).min(br.w - s(16.0) - glyph_w);
                g.text((br.x + glyph_w + (br.w - glyph_w - lw) / 2.0).round(), mid(br, mf), btn.label(), font, if danger { t.error } else { t.text }, br.w - s(16.0) - glyph_w);
            }
        }
        if let Some((k, r)) = ui.sticky {
            g.dl.rect(r, t.popup);
            if let Some(Line::Group(title)) = ui.lines.get(k) {
                paint_group(g, t, r, title, bold, s);
            }
            Paint::new(g, t, sc).separator(r.x + s(12.0), r.bottom() - 1.0, r.w - s(24.0));
        }
        g.dl.set_clip(None);
        if let Some(th) = scroll_thumb(list, ui.content_h, ui.scroll.pos, s) {
            let hot = matches!(hover, Hit::Settings(SHit::Thumb | SHit::Track)) || matches!(self.drag, super::Drag::OptionsThumb { .. });
            let bar = if hot { s(6.0) } else { s(4.0) };
            g.dl.round_rect(Rect::new(th.right() - s(3.0) - bar, th.y, bar, th.h), if hot { t.scrollbar_hover } else { t.scrollbar }, bar / 2.0);
        }
        ui.cut = cut;
        if let Some((k, (x, y))) = ui.tip {
            if let Some(line @ (Line::Setting(_) | Line::Off(_))) = ui.lines.get(k) {
                self.paint_tip(g, t, &self.setting_help(line), (x, y));
            }
        }
    }

    fn paint_settings_nav<B: TextBackend>(&self, g: &mut Gfx<B>, t: &Theme, ui: &SettingsUi, (font, small): (FontStyle, FontStyle), (mf, ms): (crate::text::FontMetrics, crate::text::FontMetrics)) {
        if ui.nav.w <= 0.0 {
            return;
        }
        let s = |v: f32| (v * self.scale).round();
        let nav = ui.nav;
        g.dl.rect(Rect::new(nav.right() - 1.0, nav.y + s(NAV_PAD), 1.0, (nav.h - 2.0 * s(NAV_PAD)).max(0.0)), t.separator);
        let icons = self.icon_font(13.0);
        let mid = |r: Rect, m: crate::text::FontMetrics| (r.y + (r.h + m.ascent - m.descent) / 2.0).round();
        let focused = ui.zone == Zone::Nav && self.active && self.nav.palette.is_none();
        let searching = !ui.query().is_empty();
        g.dl.set_clip(Some(nav));
        for (k, (&r, st)) in ui.nav_rects.iter().zip(&ui.starts).enumerate() {
            let glow = self.lit(Hit::Settings(SHit::Nav(k)));
            if st.page == ui.page && glow < 1.0 {
                g.dl.round_rect(r, t.selection, s(4.0));
            }
            if glow > 0.0 {
                g.dl.round_rect(r, super::widgets::faded(t.accent_fill, glow), s(4.0));
            }
            if focused && ui.nav_at == k {
                g.dl.outline(r, t.focus_ring, s(4.0), 1.0);
            }
            let ink = if glow >= 0.5 { crate::theme::ink_on(t.accent_fill) } else { t.text };
            let mark = if glow >= 0.5 { ink } else { t.accent };
            if ui.rail {
                centered(g, r, page_glyph(st.page), icons, mark);
                continue;
            }
            let icon = Rect::new(r.x + s(6.0), r.y, s(22.0), r.h);
            centered(g, icon, page_glyph(st.page), icons, mark);
            let tx = icon.right() + s(8.0);
            let mut right = r.right() - s(8.0);
            if searching {
                let count = st.matches.to_string();
                let cw = g.measure(&count, small);
                g.text(right - cw, mid(r, ms), &count, small, if glow >= 0.5 { ink } else { t.text_dim }, cw + 1.0);
                right -= cw + s(8.0);
            }
            g.text(tx, mid(r, mf), st.page.title(), font, ink, (right - tx).max(0.0));
        }
        g.dl.set_clip(None);
    }

    fn paint_tip<B: TextBackend>(&self, g: &mut Gfx<B>, t: &Theme, text: &str, (x, y): (f32, f32)) {
        let sc = self.scale;
        let font = self.ui_font(12.0);
        let m = g.metrics(font);
        let (pad, line_h, edge) = (self.s(8.0), (self.s(12.0) * 1.45).round(), self.s(8.0));
        let max_w = self.s(TIP_W).min(self.size.0 - 2.0 * edge - 2.0 * pad);
        let lines = wrap(g, text, font, max_w);
        let w = lines.iter().map(|l| g.measure(l, font)).fold(0.0, f32::max).ceil() + 2.0 * pad;
        let h = lines.len() as f32 * line_h + 2.0 * pad;
        let below = y + self.s(20.0);
        let ty = if below + h > self.size.1 - edge { y - self.s(6.0) - h } else { below };
        let b = Rect::new((x + self.s(12.0)).min(self.size.0 - w - edge).max(edge), ty.max(edge), w, h);
        Paint::new(g, t, sc).frame(b);
        for (n, l) in lines.iter().enumerate() {
            let base = b.y + pad + n as f32 * line_h + (line_h + m.ascent - m.descent) / 2.0;
            g.text(b.x + pad, base.round(), l, font, t.text, w - 2.0 * pad);
        }
    }

    pub(super) fn settings_tip(&mut self, x: f32, y: f32) {
        let blocked = self.nav.palette.is_some();
        let Some(ui) = &mut self.settings else { return };
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
        if let Some(ui) = self.settings.as_mut().filter(|ui| ui.tip.is_some()) {
            ui.tip = None;
            self.dirty = true;
        }
    }
}

#[cfg(test)]
#[path = "../../test/ui/settings_page.rs"]
mod tests;
