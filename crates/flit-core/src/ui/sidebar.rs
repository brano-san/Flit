use super::tags::G_TAG;
use super::{centered, App, Drag, Effect, Focus, Hit, Route, G_DRIVE, G_FOLDER};
use crate::input::{Key, Mods};
use crate::location::Location;
use crate::open::OpenTarget;
use crate::path::{FsPath, PathKind};
use crate::render::{Color, Rect};
use crate::tags::Tags;
use crate::text::{Gfx, TextBackend};
use crate::theme::Theme;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Instant;

pub const SIDEBAR_MIN_W: f32 = 160.0;
pub const SIDEBAR_MAX_W: f32 = 480.0;
const ROW_H: f32 = 28.0;
const DRIVE_H: f32 = 38.0;
const HEADER_H: f32 = 28.0;
const PIN_W: f32 = 22.0;

const G_HOME: &str = "\u{E80F}";
const G_DESKTOP: &str = "\u{E7F4}";
const G_DOWNLOADS: &str = "\u{E896}";
const G_DOCUMENTS: &str = "\u{E8A5}";
const G_PICTURES: &str = "\u{E8B9}";
const G_MUSIC: &str = "\u{E8D6}";
const G_VIDEOS: &str = "\u{E714}";
const G_LINUX: &str = "\u{E756}";
const G_RECYCLE_BIN: &str = "\u{E74D}";
const G_PIN: &str = "\u{E718}";
const G_UNPIN: &str = "\u{E77A}";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum KnownFolder {
    Home,
    Desktop,
    Downloads,
    Documents,
    Pictures,
    Music,
    Videos,
}

impl KnownFolder {
    pub const ALL: [KnownFolder; 7] = [Self::Home, Self::Desktop, Self::Downloads, Self::Documents, Self::Pictures, Self::Music, Self::Videos];

    pub fn label(self) -> &'static str {
        match self {
            Self::Home => "Home",
            Self::Desktop => "Desktop",
            Self::Downloads => "Downloads",
            Self::Documents => "Documents",
            Self::Pictures => "Pictures",
            Self::Music => "Music",
            Self::Videos => "Videos",
        }
    }

    pub fn shell_name(self) -> &'static str {
        match self {
            Self::Home => "shell:Profile",
            Self::Desktop => "shell:Desktop",
            Self::Downloads => "shell:Downloads",
            Self::Documents => "shell:Personal",
            Self::Pictures => "shell:My Pictures",
            Self::Music => "shell:My Music",
            Self::Videos => "shell:My Video",
        }
    }

    fn glyph(self) -> &'static str {
        match self {
            Self::Home => G_HOME,
            Self::Desktop => G_DESKTOP,
            Self::Downloads => G_DOWNLOADS,
            Self::Documents => G_DOCUMENTS,
            Self::Pictures => G_PICTURES,
            Self::Music => G_MUSIC,
            Self::Videos => G_VIDEOS,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Drive {
    pub path: FsPath,
    pub label: String,
    pub total: u64,
    pub free: u64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Places {
    pub folders: Vec<(KnownFolder, Location)>,
    pub drives: Vec<Drive>,
    pub wsl: Vec<String>,
    pub recycle_bin: bool,
    pub templates: Vec<super::NewTemplate>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NavState {
    pub pinned: Vec<Location>,
    pub sidebar_w: f32,
    pub sidebar: bool,
}

impl Default for NavState {
    fn default() -> Self {
        Self { pinned: Vec::new(), sidebar_w: 220.0, sidebar: true }
    }
}

impl NavState {
    pub fn sanitized(mut self) -> Self {
        if !(self.sidebar_w.is_finite() && (SIDEBAR_MIN_W..=SIDEBAR_MAX_W).contains(&self.sidebar_w)) {
            self.sidebar_w = Self::default().sidebar_w;
        }
        let mut seen = Vec::with_capacity(self.pinned.len());
        self.pinned.retain(|l| {
            let fresh = !seen.contains(l);
            seen.push(l.clone());
            fresh
        });
        self
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Section {
    Pinned,
    Folders,
    Drives,
    Wsl,
    Tags,
}

impl Section {
    fn title(self) -> &'static str {
        match self {
            Section::Pinned => "Pinned",
            Section::Folders => "Folders",
            Section::Drives => "Drives",
            Section::Wsl => "WSL",
            Section::Tags => "Tags",
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct Item {
    section: Section,
    pub label: String,
    glyph: &'static str,
    pub loc: Location,
    pub usage: Option<f32>,
}

#[derive(Default)]
pub(super) struct Sidebar {
    pub state: NavState,
    pub places: Arc<Places>,
    pub items: Vec<Item>,
    pub rects: Vec<Rect>,
    headers: Vec<(Rect, Section)>,
    hint: Option<Rect>,
    pub rect: Rect,
    pub splitter: Rect,
    scroll: f32,
    content_h: f32,
    pub cursor: usize,
    pub press_y: f32,
    pub tags: Arc<Tags>,
}

fn pinned_label(loc: &Location) -> String {
    loc.crumbs().pop().map(|(label, _)| label).unwrap_or_default()
}

fn pinned_glyph(loc: &Location) -> &'static str {
    match loc {
        Location::Wsl { path, .. } if path.is_root() => G_LINUX,
        _ if loc.path().kind() == PathKind::Computer || (loc.is_root() && loc.path().kind() == PathKind::Drive) => G_DRIVE,
        _ => G_FOLDER,
    }
}

impl Sidebar {
    fn rebuild(&mut self) {
        let mut items: Vec<Item> = self
            .state
            .pinned
            .iter()
            .map(|l| Item { section: Section::Pinned, label: pinned_label(l), glyph: pinned_glyph(l), loc: l.clone(), usage: None })
            .collect();
        let p = &self.places;
        items.extend(p.folders.iter().map(|(k, l)| Item { section: Section::Folders, label: k.label().to_owned(), glyph: k.glyph(), loc: l.clone(), usage: None }));
        if p.recycle_bin {
            let label = crate::path::RECYCLE_BIN_LABEL.to_owned();
            items.push(Item { section: Section::Folders, label, glyph: G_RECYCLE_BIN, loc: Location::recycle_bin(), usage: None });
        }
        items.extend(p.drives.iter().map(|d| Item {
            section: Section::Drives,
            label: d.label.clone(),
            glyph: G_DRIVE,
            loc: Location::Local(d.path.clone()),
            usage: (d.total > 0).then(|| (d.total.saturating_sub(d.free)) as f32 / d.total as f32),
        }));
        items.extend(p.wsl.iter().filter_map(|d| {
            let loc = Location::parse(&format!("wsl://{d}/"))?;
            Some(Item { section: Section::Wsl, label: d.clone(), glyph: G_LINUX, loc, usage: None })
        }));
        items.extend(self.tags.defs.iter().map(|t| Item { section: Section::Tags, label: t.name.clone(), glyph: G_TAG, loc: Location::computer(), usage: None }));
        self.items = items;
        self.cursor = self.cursor.min(self.items.len().saturating_sub(1));
    }

    pub fn tag_of(&self, i: usize) -> Option<usize> {
        let first = self.items.iter().position(|it| it.section == Section::Tags)?;
        (self.items.get(i)?.section == Section::Tags).then(|| i - first)
    }

    fn is_pinned(&self, loc: &Location) -> bool {
        self.state.pinned.contains(loc)
    }

    pub fn pin_rect(&self, r: Rect, scale: f32) -> Rect {
        let w = (PIN_W * scale).round();
        Rect::new(r.right() - w - (4.0 * scale).round(), r.y + ((r.h - w) / 2.0).round(), w, w)
    }
}

impl App {
    pub fn set_places(&mut self, places: Arc<Places>) {
        self.nav.sidebar.places = places;
        self.nav.sidebar.rebuild();
        self.layout();
        self.dirty = true;
    }

    pub fn set_tags(&mut self, tags: Arc<Tags>) {
        self.nav.sidebar.tags = tags;
        self.nav.sidebar.rebuild();
        self.layout();
        self.dirty = true;
    }

    pub fn set_pinned(&mut self, pinned: Vec<Location>) {
        if self.nav.sidebar.state.pinned != pinned {
            self.nav.sidebar.state.pinned = pinned;
            self.nav.sidebar.rebuild();
            self.layout();
            self.dirty = true;
        }
    }

    pub fn nav_state(&self) -> NavState {
        self.nav.sidebar.state.clone()
    }

    pub fn set_nav_state(&mut self, state: NavState) {
        self.nav.sidebar.state = state.sanitized();
        self.nav.sidebar.rebuild();
        self.layout();
        self.dirty = true;
    }

    pub(super) fn sidebar_width(&self) -> f32 {
        let st = &self.nav.sidebar.state;
        if st.sidebar {
            self.s(st.sidebar_w).min((self.size.0 / 2.0).round())
        } else {
            0.0
        }
    }

    pub(super) fn toggle_sidebar(&mut self) {
        let st = &mut self.nav.sidebar.state;
        st.sidebar = !st.sidebar;
        if !st.sidebar && self.focus == Focus::Sidebar {
            self.focus = Focus::List;
        }
        self.layout();
        self.dirty = true;
    }

    pub(super) fn cycle_sidebar_focus(&mut self) {
        if self.focus == Focus::Sidebar {
            self.focus = Focus::List;
        } else {
            if !self.nav.sidebar.state.sidebar {
                self.toggle_sidebar();
            }
            self.editing = false;
            self.focus = Focus::Sidebar;
            let here = self.current_location().clone();
            let sb = &mut self.nav.sidebar;
            if let Some(i) = sb.items.iter().position(|it| it.loc == here && it.section != Section::Tags) {
                sb.cursor = i;
            }
            self.reveal_side_cursor();
        }
        self.dirty = true;
    }

    pub(super) fn toggle_pin(&mut self, loc: Location) {
        let pinned = &mut self.nav.sidebar.state.pinned;
        match pinned.iter().position(|l| *l == loc) {
            Some(i) => {
                pinned.remove(i);
            }
            None => pinned.push(loc),
        }
        self.pins_changed();
    }

    fn pins_changed(&mut self) {
        let list = self.nav.sidebar.state.pinned.clone();
        self.nav.sidebar.rebuild();
        self.effects.push(Effect::Pinned(list));
        self.layout();
        self.dirty = true;
    }

    pub(super) fn pin_selection(&mut self) {
        let folders = self.selected_folders();
        if folders.is_empty() {
            return self.status("Select the folders to pin");
        }
        let pinned = &mut self.nav.sidebar.state.pinned;
        if folders.iter().all(|l| pinned.contains(l)) {
            pinned.retain(|l| !folders.contains(l));
        } else {
            for l in folders {
                if !pinned.contains(&l) {
                    pinned.push(l);
                }
            }
        }
        self.pins_changed();
    }

    pub(super) fn pin_current(&mut self) {
        let here = self.current_location().clone();
        self.toggle_pin(here);
    }

    pub(super) fn toggle_pin_item(&mut self, i: usize) {
        if let Some(loc) = self.nav.sidebar.items.get(i).map(|it| it.loc.clone()) {
            self.toggle_pin(loc);
        }
    }

    pub(super) fn start_pin_drag(&mut self, y: f32) -> bool {
        let Hit::Side(i) = self.pressed else { return false };
        let sb = &self.nav.sidebar;
        if sb.items.get(i).is_none_or(|it| it.section != Section::Pinned) || (y - sb.press_y).abs() < self.s(5.0) {
            return false;
        }
        self.drag_pin(i, y);
        true
    }

    pub(super) fn drag_pin(&mut self, from: usize, y: f32) {
        let sb = &self.nav.sidebar;
        let to = sb.rects.iter().take(sb.state.pinned.len()).filter(|r| r.y + r.h / 2.0 < y).count();
        self.drag = Drag::PinMove { from, to };
        self.dirty = true;
    }

    pub(super) fn drop_pin(&mut self, from: usize, to: usize) {
        let pinned = &mut self.nav.sidebar.state.pinned;
        let target = if to > from { to - 1 } else { to };
        if from >= pinned.len() || target == from {
            return;
        }
        let loc = pinned.remove(from);
        pinned.insert(target.min(pinned.len()), loc);
        self.pins_changed();
    }

    pub(super) fn open_side_item(&mut self, i: usize, new_tab: bool, now: Instant) {
        if let Some(k) = self.nav.sidebar.tag_of(i) {
            return self.show_tag(k);
        }
        let Some(folder) = self.nav.sidebar.items.get(i).map(|it| it.loc.clone()) else { return };
        let target = if new_tab { OpenTarget::NewTab } else { OpenTarget::CurrentTab };
        self.dispatch(vec![Route { folder, select: None }], target, now);
    }

    pub(super) fn known_folder(&self, k: KnownFolder) -> Option<Location> {
        self.nav.sidebar.places.folders.iter().find(|(f, _)| *f == k).map(|(_, l)| l.clone())
    }

    pub(super) fn layout_sidebar(&mut self, top: f32, bottom: f32) {
        let width = self.sidebar_width();
        let sc = self.scale;
        let s = |v: f32| (v * sc).round();
        let sb = &mut self.nav.sidebar;
        sb.rect = Rect::new(0.0, top, width, (bottom - top).max(0.0));
        sb.splitter = if width > 0.0 { Rect::new(width - s(3.0), top, s(6.0), sb.rect.h) } else { Rect::default() };
        sb.rects.clear();
        sb.headers.clear();
        sb.hint = None;
        if width <= 0.0 {
            return;
        }
        let x = s(6.0);
        let w = (width - s(12.0)).max(0.0);
        let mut y = s(4.0);
        for section in [Section::Pinned, Section::Folders, Section::Drives, Section::Wsl, Section::Tags] {
            let count = sb.items.iter().filter(|it| it.section == section).count();
            if count == 0 && section != Section::Pinned {
                continue;
            }
            sb.headers.push((Rect::new(x, y, w, s(HEADER_H)), section));
            y += s(HEADER_H);
            if count == 0 {
                sb.hint = Some(Rect::new(x, y, w, s(ROW_H)));
                y += s(ROW_H);
            }
            for it in sb.items.iter().filter(|it| it.section == section) {
                let h = if it.usage.is_some() { s(DRIVE_H) } else { s(ROW_H) };
                sb.rects.push(Rect::new(x, y, w, h));
                y += h;
            }
            y += s(6.0);
        }
        sb.content_h = y;
        sb.scroll = sb.scroll.clamp(0.0, (sb.content_h - sb.rect.h).max(0.0));
        let dy = top - sb.scroll;
        for r in sb.rects.iter_mut().chain(sb.headers.iter_mut().map(|(r, _)| r)).chain(sb.hint.iter_mut()) {
            r.y += dy;
        }
    }

    pub(super) fn sidebar_hit(&self, x: f32, y: f32) -> Option<Hit> {
        let sb = &self.nav.sidebar;
        if sb.rect.w <= 0.0 {
            return None;
        }
        if sb.splitter.contains(x, y) {
            return Some(Hit::Splitter);
        }
        if !sb.rect.contains(x, y) {
            return None;
        }
        for (i, r) in sb.rects.iter().enumerate() {
            if r.contains(x, y) {
                let pin = sb.pin_rect(*r, self.scale).contains(x, y) && sb.tag_of(i).is_none();
                return Some(if pin { Hit::SidePin(i) } else { Hit::Side(i) });
            }
        }
        Some(Hit::SideEmpty)
    }

    pub(super) fn sidebar_wheel(&mut self, dy: f32) {
        let sb = &mut self.nav.sidebar;
        let step = 3.0 * (ROW_H * self.scale).round();
        sb.scroll = (sb.scroll - dy * step).clamp(0.0, (sb.content_h - sb.rect.h).max(0.0));
        self.layout();
        self.dirty = true;
    }

    pub(super) fn begin_splitter_drag(&mut self, x: f32) {
        self.drag = Drag::Splitter { start_x: x, start_w: self.nav.sidebar.state.sidebar_w };
    }

    pub(super) fn drag_splitter(&mut self, x: f32, start_x: f32, start_w: f32) {
        let w = (start_w + (x - start_x) / self.scale).clamp(SIDEBAR_MIN_W, SIDEBAR_MAX_W);
        self.nav.sidebar.state.sidebar_w = w.round();
        self.layout();
        self.dirty = true;
    }

    fn reveal_side_cursor(&mut self) {
        self.layout();
        let sb = &mut self.nav.sidebar;
        let Some(r) = sb.rects.get(sb.cursor).copied() else { return };
        if r.y < sb.rect.y {
            sb.scroll -= sb.rect.y - r.y;
        } else if r.bottom() > sb.rect.bottom() {
            sb.scroll += r.bottom() - sb.rect.bottom();
        }
        self.layout();
    }

    pub(super) fn sidebar_key(&mut self, key: Key, m: Mods, now: Instant) -> bool {
        let n = self.nav.sidebar.items.len();
        let plain = !m.ctrl && !m.alt && !m.shift;
        let cur = self.nav.sidebar.cursor;
        let to = match key {
            Key::Escape => {
                self.focus = Focus::List;
                return true;
            }
            Key::Enter if !m.alt => {
                self.open_side_item(cur, m.ctrl, now);
                return true;
            }
            Key::Delete if plain => {
                if self.nav.sidebar.items.get(cur).is_some_and(|it| it.section == Section::Pinned) {
                    self.toggle_pin_item(cur);
                }
                return true;
            }
            Key::Up if plain => cur.saturating_sub(1),
            Key::Down if plain => (cur + 1).min(n.saturating_sub(1)),
            Key::Home if plain => 0,
            Key::End if plain => n.saturating_sub(1),
            _ => return false,
        };
        self.nav.sidebar.cursor = to;
        self.reveal_side_cursor();
        true
    }

    pub(super) fn side_menu(&mut self, h: Hit, x: f32, y: f32) {
        let sb = &self.nav.sidebar;
        let tag = match h {
            Hit::Side(i) => sb.tag_of(i),
            _ => None,
        };
        if tag.is_some() || sb.headers.iter().any(|(r, s)| *s == Section::Tags && r.contains(x, y)) {
            let menu = self.build_tag_menu(tag);
            self.flit_menu(x, y, menu);
        }
    }

    pub(super) fn draw_sidebar<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme) {
        let sb = &self.nav.sidebar;
        let area = sb.rect;
        if area.w <= 0.0 {
            return;
        }
        g.dl.rect(area, t.chrome);
        let line = if matches!(self.hover, Hit::Splitter) || matches!(self.drag, Drag::Splitter { .. }) { t.accent } else { t.separator };
        g.dl.rect(Rect::new(area.right() - 1.0, area.y, 1.0, area.h), line);
        g.dl.set_clip(Some(Rect::new(area.x, area.y, area.w - 1.0, area.h)));
        let font = self.ui_font(13.0);
        let small = self.ui_font(12.0);
        let icons = self.icon_font(14.0);
        let pin_font = self.icon_font(11.0);
        let m = g.metrics(font);
        let ms = g.metrics(small);
        let (s4, s8, s12, s36) = (self.s(4.0), self.s(8.0), self.s(12.0), self.s(36.0));
        let here = self.tabs[self.cur].location();
        let focused = self.active && self.focus == Focus::Sidebar;
        let drop = match self.drop_spot() {
            Some(super::Spot::Side(i)) => Some(i),
            _ => None,
        };
        for &(r, section) in &sb.headers {
            let baseline = (r.y + (r.h + ms.ascent - ms.descent) / 2.0 + self.s(3.0)).round();
            g.text(r.x + s8, baseline, section.title(), small, t.text_dim, r.w - s8);
        }
        if let Some(r) = sb.hint {
            let baseline = (r.y + (r.h + ms.ascent - ms.descent) / 2.0).round();
            g.text(r.x + s12, baseline, "Ctrl+D pins the current folder", small, t.text_dim.alpha(0xA0), r.w - s12);
        }
        for (i, (it, &r)) in sb.items.iter().zip(&sb.rects).enumerate() {
            if r.bottom() < area.y || r.y > area.bottom() {
                continue;
            }
            let hovered = matches!(self.hover, Hit::Side(k) | Hit::SidePin(k) if k == i);
            let row = Rect::new(r.x, r.y + 1.0, r.w, r.h - 2.0);
            let tag = sb.tag_of(i).and_then(|k| sb.tags.defs.get(k));
            if (it.loc == *here && tag.is_none()) || drop == Some(i) {
                g.dl.round_rect(row, t.selection, s4);
            } else if hovered {
                g.dl.round_rect(row, t.button_hover, s4);
            }
            if drop == Some(i) {
                g.dl.outline(row, t.accent, s4, self.s(1.0).max(1.0));
            } else if focused && sb.cursor == i {
                g.dl.outline(row, t.focus_ring, s4, 1.0);
            }
            let text_h = self.s(ROW_H);
            let icon_r = Rect::new(r.x + s4, r.y, self.s(28.0), text_h);
            let color = match tag {
                Some(tag) => Color::rgb(tag.color),
                None if it.section == Section::Drives || it.section == Section::Wsl => t.text_dim,
                None => t.accent,
            };
            centered(g, icon_r, it.glyph, icons, color);
            let baseline = (r.y + (text_h + m.ascent - m.descent) / 2.0).round();
            let pin_r = sb.pin_rect(Rect::new(r.x, r.y, r.w, text_h), self.scale);
            let text_right = if hovered && tag.is_none() { pin_r.x - s4 } else { r.right() - s8 };
            g.text(r.x + s36, baseline, &it.label, font, t.text, text_right - r.x - s36);
            if let Some(used) = it.usage {
                let bar = Rect::new(r.x + s36, r.y + text_h - self.s(2.0), (r.right() - s12 - r.x - s36).max(0.0), self.s(4.0));
                g.dl.round_rect(bar, t.separator, bar.h / 2.0);
                let fill = if used >= 0.9 { t.error } else { t.accent };
                g.dl.round_rect(Rect::new(bar.x, bar.y, (bar.w * used.clamp(0.0, 1.0)).max(bar.h), bar.h), fill, bar.h / 2.0);
            }
            if hovered && tag.is_none() {
                if self.hover == Hit::SidePin(i) {
                    g.dl.round_rect(pin_r, t.button_hover, s4);
                }
                let glyph = if sb.is_pinned(&it.loc) { G_UNPIN } else { G_PIN };
                centered(g, pin_r, glyph, pin_font, t.text_dim);
            }
        }
        if let Drag::PinMove { to, .. } = self.drag {
            let n = sb.state.pinned.len().min(sb.rects.len());
            let before = sb.rects.get(to).filter(|_| to < n).map(|r| r.y);
            let y = before.or_else(|| sb.rects.get(n.saturating_sub(1)).map(|r| r.bottom())).unwrap_or(area.y);
            g.dl.rect(Rect::new(area.x + s8, y - 1.0, area.w - 2.0 * s8, self.s(2.0)), t.accent);
        }
        g.dl.set_clip(None);
    }
}

#[cfg(test)]
#[path = "../../test/ui/sidebar.rs"]
mod tests;
