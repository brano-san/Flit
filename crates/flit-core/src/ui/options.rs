use super::actions::{Group, ACTIONS};
use super::confirm::Confirmed;
use super::context::MenuKind;
use super::menu::{Cmd, Mark};
use super::palette::{Palette, Pick};
use super::panels::PanelsSession;
use super::sidebar::KnownFolder;
use super::{App, Effect, Hit, LayoutCmd, Route, SettingsCmd, WindowSession};
use crate::i18n::S;
use crate::location::Location;
use crate::open::OpenTarget;
use crate::render::Rect;
use crate::theme::Mode;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

pub const SPACING_DEFAULT: u8 = 40;
pub const SPACING_STEP: i32 = 10;
pub const OPACITY_DEFAULT: u8 = 60;
pub const INSPECTOR_FONT: &str = "Consolas";
pub const INSPECTOR_SIZE: (i64, i64, i64) = (6, 10, 32);
const MAX_LAYOUT_NAME: usize = 64;
const SHOWN_CLOSED: usize = 4;
pub const CONFIG_DEFAULTS: &str = "settings.default.json";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedLayout {
    pub tabs: Vec<Location>,
    pub cur: usize,
    pub panels: Option<PanelsSession>,
    pub quick_access: bool,
    pub quick_width: f32,
}

impl SavedLayout {
    pub fn session(&self) -> WindowSession {
        WindowSession { tabs: self.tabs.clone(), cur: self.cur, trails: Vec::new(), panels: self.panels.clone() }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CharKey {
    #[default]
    Filter,
    Select,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InspectorAt {
    #[default]
    Auto,
    Right,
    Below,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NewTabShow {
    #[default]
    Current,
    Root,
    Home,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Recents {
    #[default]
    Persistent,
    Session,
    Never,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Thumbs {
    #[default]
    Local,
    Always,
    Never,
}

pub struct Opts {
    pub spacing: u8,
    pub alternating: bool,
    pub inactive_opacity: u8,
    pub compact_paths: bool,
    pub breadcrumbs: bool,
    pub recents: Recents,
    pub thumbs: Thumbs,
    pub sizes_ssd: bool,
    pub sizes_hdd: bool,
    pub sizes_network: bool,
    pub act_on_press: bool,
    pub scroll_beyond: bool,
    pub reverse_scroll: bool,
    pub char_key: CharKey,
    pub inspector_at: InspectorAt,
    pub new_tab: NewTabShow,
    pub inspector_font: String,
    pub inspector_size: u8,
    pub scheme: Mode,
    pub layouts: Vec<String>,
    pub pins: [Vec<String>; 3],
    pub closed_at: Vec<Instant>,
    pub tab_rows: Vec<TabRow>,
    pub describe: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TabRow {
    Open(usize),
    Closed(usize),
}

impl Default for Opts {
    fn default() -> Self {
        Self {
            spacing: SPACING_DEFAULT,
            alternating: true,
            inactive_opacity: OPACITY_DEFAULT,
            compact_paths: true,
            breadcrumbs: true,
            recents: Recents::Persistent,
            thumbs: Thumbs::Local,
            sizes_ssd: true,
            sizes_hdd: false,
            sizes_network: false,
            act_on_press: true,
            scroll_beyond: false,
            reverse_scroll: false,
            char_key: CharKey::Filter,
            inspector_at: InspectorAt::Auto,
            new_tab: NewTabShow::Current,
            inspector_font: INSPECTOR_FONT.to_owned(),
            inspector_size: INSPECTOR_SIZE.1 as u8,
            scheme: Mode::Dark,
            layouts: Vec::new(),
            pins: Default::default(),
            closed_at: Vec::new(),
            tab_rows: Vec::new(),
            describe: true,
        }
    }
}

pub fn remote(loc: &Location) -> bool {
    matches!(loc.path().kind(), crate::path::PathKind::Unc)
}

pub fn mono_px(points: u8) -> f32 {
    f32::from(points) * 1.25
}

pub fn spacing_factor(spacing: u8) -> f32 {
    0.8 + spacing.min(100) as f32 / 200.0
}

pub fn clean_layout_name(name: &str) -> Option<String> {
    let name: String = name.trim().chars().filter(|c| !c.is_control() && !"\\/:*?\"<>|".contains(*c)).take(MAX_LAYOUT_NAME).collect();
    let name = name.trim().trim_end_matches('.').to_owned();
    (!name.is_empty()).then_some(name)
}

pub fn closed_ago(elapsed: Duration) -> String {
    let m = elapsed.as_secs() / 60;
    match m {
        0 => S::MenusClosedNow.t().to_owned(),
        1..=59 => crate::tr!(S::MenusClosedMinutes, n = m),
        _ => crate::tr!(S::MenusClosedHours, n = m / 60),
    }
}

impl App {
    pub(super) fn open_main_menu(&mut self) {
        let r = self.panels.menu;
        let menu = self.collect(|a, m| {
            let title = |id: &str| ACTIONS.iter().find(|x| x.id == id).map_or("", |x| x.title.t());
            m.action("goto.open", title("goto.open"));
            m.action("palette.show", title("palette.show"));
            m.separator();
            m.entry(Cmd::Action("layout.load"), title("layout.load"), String::new(), Mark::None, false, a.opts.layouts.is_empty());
            m.action("layout.save", title("layout.save"));
            m.action("options.open", title("options.open"));
            m.separator();
            m.action("tab.duplicate_window", S::MenuNewWindow.t());
            if a.tabs.len() > 1 {
                m.action("tab.move_to_window", S::MenuMoveTabToWindow.t());
            }
            m.action("window.compact", S::MenuMiniWindow.t());
            m.action("window.fullscreen", S::MenuFullScreen.t());
        });
        self.button_menu(Hit::Menu, r.x, r.bottom(), menu);
    }

    pub(super) fn quick_checked(&self, id: &str) -> Option<bool> {
        let t = self.tab();
        Some(match id {
            "subfolders.toggle" => t.subfolders,
            "inspector.toggle" => self.preview.state.visible,
            "view.extensions" => self.prefs.show_extensions,
            "view.hidden" => t.view_spec.show_hidden,
            "view.system" => self.prefs.show_system,
            _ => return None,
        })
    }

    pub(super) fn panel_menu_at(&mut self, x: f32, y: f32, right: Option<f32>) {
        let menu = self.collect(|a, m| {
            let mut last = None;
            for act in ACTIONS.iter().filter(|x| x.group == Group::Quick) {
                let check = a.quick_checked(act.id);
                let section = match act.id {
                    "subfolders.toggle" => 0,
                    "inspector.toggle" => 1,
                    "panel.split_right" | "panel.split_bottom" => 2,
                    "tab.new" | "tab.close" => 3,
                    "options.open" | "folder.explorer" | "app.terminal" => 4,
                    _ => 5,
                };
                if last.is_some_and(|l| l != section) {
                    m.separator();
                }
                last = Some(section);
                let keys = m.keys.all_text(act.id);
                match check {
                    Some(on) => m.entry(Cmd::Action(act.id), act.title.t(), keys, Mark::Check, on, false),
                    None => m.action(act.id, act.title.t()),
                }
            }
        });
        self.flit_menu(x, y, menu);
        self.menu_right = right;
    }

    pub(super) fn panel_quick_menu(&mut self) {
        let r = self.layout.more;
        let right = self.layout.title.right().max(r.right());
        self.panel_menu_at(r.x, r.bottom(), Some(right.min(self.panel_right())));
        self.menu_owner = Some(Hit::PanelMenu);
    }

    fn panel_right(&self) -> f32 {
        self.layout.list.right().max(self.layout.track.right()).max(self.layout.more.right())
    }

    pub(super) fn quick_commands(&mut self) {
        match self.details.middle_at.take() {
            Some((x, y)) => self.panel_menu_at(x, y, None),
            None => self.panel_quick_menu(),
        }
    }

    pub(super) fn selection_kind(&self) -> MenuKind {
        let t = self.tab();
        let all_dirs = t.view.as_ref().is_some_and(|v| v.selected_count() > 0 && v.selected().all(|i| v.listing().entries[i as usize].is_dir()));
        if all_dirs { MenuKind::Folder } else { MenuKind::File }
    }

    pub(super) fn ask_pin(&mut self, key: String, pin: bool, anchor: Rect, now: Instant) {
        let Some(kind) = self.own_menu_kind() else { return };
        let text = if pin { (S::MenusPinQuestion.t(), S::MenusPinYes.t()) } else { (S::MenusUnpinQuestion.t(), S::MenusUnpinYes.t()) };
        self.ask(anchor, text, false, Confirmed::Pin(kind, key, pin), now);
    }

    pub(super) fn set_pin(&mut self, kind: MenuKind, key: String, pin: bool) {
        let Some(k) = kind.pins() else { return };
        let list = &mut self.opts.pins[k];
        list.retain(|p| *p != key);
        if pin {
            list.insert(0, key);
        }
        let pins = list.clone();
        if let Some(c) = self.context_menu_mut().filter(|c| c.kind == kind) {
            c.set_pins(pins);
        }
        self.dirty = true;
    }

    pub(super) fn step_spacing(&mut self, d: i32) {
        self.set_spacing((self.opts.spacing as i32 + d).clamp(0, 100) as u8);
    }

    pub(super) fn set_spacing(&mut self, v: u8) {
        self.opts.spacing = v.min(100);
        self.layout();
        self.dirty = true;
    }

    pub(super) fn create_config(&mut self) {
        let Some(dir) = self.about.file.as_ref().and_then(|f| f.parent()) else { return self.status(S::SettingsPageStatusNoFile.t()) };
        let path = dir.join(CONFIG_DEFAULTS.as_bytes());
        let text = super::settings::render(super::settings::defaults(), &serde_json::Map::new(), None);
        self.status(crate::tr!(S::MenusConfigCreated, path = path.display()));
        self.effects.push(Effect::Settings(SettingsCmd::Export { path, text, replace: true }));
    }

    pub(super) fn new_tab_location(&self) -> Location {
        match self.opts.new_tab {
            NewTabShow::Current => self.current_location().clone(),
            NewTabShow::Root => Location::home_page(),
            NewTabShow::Home => self.known_folder(KnownFolder::Home).unwrap_or_else(Location::home_page),
        }
    }

    pub(super) fn open_new_tab(&mut self, now: Instant) {
        let folder = self.new_tab_location();
        self.dispatch(vec![Route { folder, select: None }], OpenTarget::NewTab, now);
    }

    pub(super) fn note_closed(&mut self, now: Instant) {
        self.opts.closed_at.push(now);
        let n = self.nav.closed.len();
        if self.opts.closed_at.len() > n {
            let extra = self.opts.closed_at.len() - n;
            self.opts.closed_at.drain(..extra);
        }
    }

    pub(super) fn open_tab_search(&mut self) {
        let now = self.last_frame.unwrap_or_else(Instant::now);
        let mut items = Vec::new();
        let mut descs = Vec::new();
        let mut rows = Vec::new();
        for (i, t) in self.tabs.iter().enumerate() {
            items.push((t.title(), String::new()));
            descs.push(super::tabbar::full_name(t));
            rows.push(TabRow::Open(i));
        }
        let closed = self.nav.closed.len();
        let times = self.opts.closed_at.len();
        for k in (0..closed).rev().take(SHOWN_CLOSED) {
            let loc = &self.nav.closed[k];
            let name = crate::name::display(loc.file_name()).into_owned();
            let title = if name.is_empty() { loc.display().into_owned() } else { name };
            let at = (k + times).checked_sub(closed).and_then(|j| self.opts.closed_at.get(j));
            let path = loc.display().into_owned();
            let desc = match at {
                Some(at) => format!("{path} \u{2022} {}", closed_ago(now.saturating_duration_since(*at))),
                None => path,
            };
            items.push((title, String::new()));
            descs.push(desc);
            rows.push(TabRow::Closed(k));
        }
        let n = items.len();
        let mut p = Palette::new(Pick::Tabs, items, "");
        p.set_descs(descs);
        p.placeholder = Some(crate::tr!(S::MenusSearchTabs, n = n));
        let cur = self.cur;
        self.opts.tab_rows = rows;
        p.select_row(cur);
        self.open_pick(p);
    }

    pub(super) fn tab_list_anchor(&self) -> Rect {
        let a = self.layout.actions;
        if a.w > 0.0 { a } else { Rect::new(self.layout.title.right() - 1.0, self.layout.title.y, 1.0, self.layout.title.h) }
    }

    pub(super) fn tab_row_glyph(&self, row: usize) -> &'static str {
        match self.opts.tab_rows.get(row) {
            Some(TabRow::Open(i)) => self.tabs.get(*i).map_or(super::G_FOLDER, |t| super::tabbar::tab_glyph(t.location())),
            Some(TabRow::Closed(k)) => self.nav.closed.get(*k).map_or(super::G_FOLDER, super::tabbar::tab_glyph),
            None => super::G_FOLDER,
        }
    }

    pub(super) fn tab_row_closed(&self, row: usize) -> bool {
        matches!(self.opts.tab_rows.get(row), Some(TabRow::Closed(_)))
    }

    pub(super) fn choose_tab_row(&mut self, row: usize, now: Instant) {
        match self.opts.tab_rows.get(row).cloned() {
            Some(TabRow::Open(i)) => self.select_tab(i),
            Some(TabRow::Closed(k)) if k < self.nav.closed.len() => {
                let folder = self.nav.closed.remove(k);
                let times = self.opts.closed_at.len();
                if let Some(j) = (k + times).checked_sub(self.nav.closed.len() + 1) {
                    if j < times {
                        self.opts.closed_at.remove(j);
                    }
                }
                self.dispatch(vec![Route { folder, select: None }], OpenTarget::NewTab, now);
            }
            _ => {}
        }
    }

    pub(super) fn ask_save_layout(&mut self) {
        self.open_pick(Palette::new(Pick::SaveLayout, Vec::new(), ""));
    }

    pub(super) fn ask_load_layout(&mut self) {
        if self.opts.layouts.is_empty() {
            return self.status(S::MenusNoLayouts.t());
        }
        let items = self.opts.layouts.iter().map(|n| (n.clone(), String::new())).collect();
        self.open_pick(Palette::new(Pick::LoadLayout, items, ""));
    }

    pub fn saved_layout(&self) -> SavedLayout {
        let s = self.session();
        SavedLayout { tabs: s.tabs, cur: s.cur, panels: s.panels, quick_access: self.nav.sidebar.state.sidebar, quick_width: self.nav.sidebar.state.sidebar_w }
    }

    pub(super) fn save_layout(&mut self, name: &str) {
        let Some(name) = clean_layout_name(name) else { return self.status(S::MenusBadLayoutName.t()) };
        let layout = self.saved_layout();
        if !self.opts.layouts.contains(&name) {
            self.opts.layouts.push(name.clone());
            self.opts.layouts.sort_by_key(|n| n.to_lowercase());
        }
        self.status(crate::tr!(S::MenusLayoutSaved, name = name));
        self.effects.push(Effect::Layout(LayoutCmd::Save(name, Box::new(layout))));
    }

    pub(super) fn load_layout(&mut self, name: &str) {
        self.effects.push(Effect::Layout(LayoutCmd::Load(name.to_owned())));
    }

    pub(super) fn delete_layout(&mut self, name: &str) {
        self.opts.layouts.retain(|n| n != name);
        self.effects.push(Effect::Layout(LayoutCmd::Delete(name.to_owned())));
    }

    pub fn set_layouts(&mut self, names: Vec<String>) {
        if self.opts.layouts != names {
            self.opts.layouts = names;
            self.dirty = true;
        }
    }

    pub fn layouts(&self) -> &[String] {
        &self.opts.layouts
    }

    pub fn apply_layout(&mut self, layout: &SavedLayout, now: Instant) {
        if layout.tabs.is_empty() {
            return;
        }
        self.reset_panels(layout, now);
        let side = &mut self.nav.sidebar.state;
        side.sidebar = layout.quick_access;
        if layout.quick_width > 0.0 {
            side.sidebar_w = layout.quick_width;
            *side = std::mem::take(side).sanitized();
        }
        if let Some(p) = &layout.panels {
            self.restore_panels(p);
        }
        self.strip.rev += 1;
        self.layout();
        self.dirty = true;
    }

    pub(super) fn set_scheme(&mut self, mode: Mode) {
        self.opts.scheme = mode;
        if self.look.mode != Mode::System {
            self.set_theme_mode(mode);
        }
    }

    pub(super) fn set_system_scheme(&mut self, on: bool) {
        self.set_theme_mode(if on { Mode::System } else { self.opts.scheme });
    }

    pub(super) fn thumbs_choice(&self) -> usize {
        match (self.prefs.thumbnails, self.opts.thumbs) {
            (false, _) | (_, Thumbs::Never) => 2,
            (true, Thumbs::Local) => 0,
            (true, Thumbs::Always) => 1,
        }
    }

    pub(super) fn set_thumbs_choice(&mut self, i: usize) {
        self.opts.thumbs = [Thumbs::Local, Thumbs::Always, Thumbs::Never][i.min(2)];
        let on = self.opts.thumbs != Thumbs::Never;
        if self.prefs.thumbnails != on {
            self.set_pref(|p| &mut p.thumbnails, on);
        }
    }

    pub(super) fn set_inspector_font(&mut self, name: String) {
        let name = name.trim();
        self.opts.inspector_font = if name.is_empty() { INSPECTOR_FONT.to_owned() } else { name.to_owned() };
        self.dirty = true;
    }

    pub(super) fn set_inspector_size(&mut self, size: u8) {
        self.opts.inspector_size = size.clamp(INSPECTOR_SIZE.0 as u8, INSPECTOR_SIZE.2 as u8);
        self.dirty = true;
    }

    pub(super) fn set_compact_paths(&mut self, on: bool) {
        self.opts.compact_paths = on;
        self.nav.sidebar.full_paths = !on;
        self.nav.sidebar.rebuild();
        self.dirty = true;
    }

    pub(super) fn sizes_allowed(&self, loc: &Location) -> bool {
        if remote(loc) { self.opts.sizes_network } else { self.opts.sizes_ssd }
    }

    pub(super) fn dim_alpha(&self) -> u8 {
        ((100 - u32::from(self.opts.inactive_opacity.min(100))) * 255 / 100) as u8
    }

    pub(super) fn scheme(&self) -> Mode {
        match self.look.mode {
            Mode::System => self.opts.scheme,
            m => m,
        }
    }
}

#[cfg(test)]
#[path = "../../test/ui/options.rs"]
mod tests;
