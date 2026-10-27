use super::ops::TIMER_SLACK;
use super::palette::{Palette, Pick};
use super::{centered, App, Effect, Hit, LayoutMode, LAYOUTS};
use crate::archive;
use crate::i18n::S;
use crate::input::{Key, MouseButton};
use crate::path::PathKind;
use crate::render::Rect;
use crate::text::{Gfx, TextBackend};
use crate::theme::Theme;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

pub const BAR_H: f32 = 40.0;
const BUTTON_H: f32 = 32.0;
const ICON_W: f32 = 34.0;
const SEP_W: f32 = 13.0;
const PAD: f32 = 10.0;
const MAX_ITEMS: usize = 48;
const TIP_DELAY: Duration = Duration::from_millis(500);
const FLY_W: f32 = 272.0;
const FLY_ROW: f32 = 30.0;
const TOGGLES: [S; 4] = [S::ToolbarFlyoutShowHidden, S::ToolbarFlyoutShowExtensions, S::ToolbarFlyoutAdaptive, S::ToolbarFlyoutThumbnails];
const LAYOUT_IDS: [&str; 6] = ["view.layout.details", "view.layout.list", "view.layout.cards", "view.layout.columns", "view.layout.grid", "view.layout.adaptive"];

const G_MORE: &str = "\u{E712}";
const G_DOWN: &str = "\u{E70D}";
const G_CHECK: &str = "\u{E73E}";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum ToolMenu {
    New,
    Select,
    Sort,
    Group,
    Layout,
    Extract,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Action,
    Menu(ToolMenu),
}

struct Tool {
    id: &'static str,
    label: S,
    glyph: &'static str,
    kind: Kind,
}

const fn act(id: &'static str, label: S, glyph: &'static str) -> Tool {
    Tool { id, label, glyph, kind: Kind::Action }
}

const fn menu(id: &'static str, label: S, glyph: &'static str, m: ToolMenu) -> Tool {
    Tool { id, label, glyph, kind: Kind::Menu(m) }
}

const TOOLS: &[Tool] = &[
    menu("menu.new", S::ToolbarButtonNew, "\u{E710}", ToolMenu::New),
    act("file.cut", S::ToolbarButtonCut, "\u{E8C6}"),
    act("file.copy", S::ToolbarButtonCopy, "\u{E8C8}"),
    act("file.paste", S::ToolbarButtonPaste, "\u{E77F}"),
    act("file.rename", S::ToolbarButtonRename, "\u{E8AC}"),
    act("file.share", S::ToolbarButtonShare, "\u{E72D}"),
    act("item.trash", S::ToolbarButtonDelete, "\u{E74D}"),
    act("item.properties", S::ToolbarButtonProperties, "\u{E946}"),
    menu("menu.select", S::ToolbarButtonSelect, "\u{E8B3}", ToolMenu::Select),
    menu("menu.sort", S::ToolbarButtonSort, "\u{E8CB}", ToolMenu::Sort),
    menu("menu.group", S::ToolbarButtonGroup, "\u{E8FD}", ToolMenu::Group),
    menu("menu.layout", S::ToolbarButtonLayout, "\u{E8A9}", ToolMenu::Layout),
    act("view.pane", S::ToolbarButtonInfoPane, "\u{E8A0}"),
    act("bin.restore", S::ToolbarButtonRestore, "\u{E777}"),
    act("bin.restore_all", S::ToolbarButtonRestoreAll, "\u{E777}"),
    act("bin.empty", S::ToolbarButtonEmptyBin, "\u{E74D}"),
    act("file.new_folder", S::ToolbarButtonNewFolder, "\u{E8F4}"),
    act("file.new_item", S::ToolbarButtonNewItem, "\u{E7C3}"),
    act("item.copy_path", S::ToolbarButtonCopyPath, "\u{E71B}"),
    act("item.delete", S::ToolbarButtonDeletePermanently, "\u{E74D}"),
    act("app.terminal", S::ToolbarButtonTerminal, "\u{E756}"),
    act("item.pin", S::ToolbarButtonPin, "\u{E718}"),
    act("edit.undo", S::ToolbarButtonUndo, "\u{E7A7}"),
    act("edit.redo", S::ToolbarButtonRedo, "\u{E7A6}"),
    act("view.hidden", S::ToolbarButtonHiddenItems, "\u{E890}"),
    act("view.sizes", S::ToolbarButtonFolderSizes, "\u{EDA2}"),
    act("nav.refresh", S::ToolbarButtonRefresh, "\u{E72C}"),
    act("search.folder", S::ToolbarButtonSearch, "\u{E721}"),
    act("sidebar.toggle", S::ToolbarButtonSidebar, "\u{E700}"),
    menu("menu.extract", S::ToolbarButtonExtract, "\u{E8B7}", ToolMenu::Extract),
];

fn tool(id: &str) -> Option<&'static Tool> {
    TOOLS.iter().find(|t| t.id == id)
}


#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Show {
    Icon,
    Label,
    Both,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolItem {
    Button(String, Show),
    Separator,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ToolContext {
    Folder,
    RecycleBin,
    ThisPc,
}

pub const CONTEXTS: [ToolContext; 3] = [ToolContext::Folder, ToolContext::RecycleBin, ToolContext::ThisPc];

impl ToolContext {
    fn index(self) -> usize {
        self as usize
    }

    fn reset_message(self) -> S {
        match self {
            ToolContext::Folder => S::ToolbarResetFolders,
            ToolContext::RecycleBin => S::ToolbarResetRecycleBin,
            ToolContext::ThisPc => S::ToolbarResetThisPc,
        }
    }

    fn edit_label(self) -> S {
        match self {
            ToolContext::Folder => S::ToolbarPickEditFolders,
            ToolContext::RecycleBin => S::ToolbarPickEditRecycleBin,
            ToolContext::ThisPc => S::ToolbarPickEditThisPc,
        }
    }

    pub fn defaults(self) -> Vec<ToolItem> {
        let b = |id: &str, show| ToolItem::Button(id.to_owned(), show);
        let view = [ToolItem::Separator, b("menu.select", Show::Both), b("menu.sort", Show::Both), b("menu.group", Show::Both), b("menu.layout", Show::Both), ToolItem::Separator, b("view.pane", Show::Icon)];
        let head = match self {
            ToolContext::Folder => vec![
                b("menu.new", Show::Both),
                ToolItem::Separator,
                b("file.cut", Show::Icon),
                b("file.copy", Show::Icon),
                b("file.paste", Show::Icon),
                b("file.rename", Show::Icon),
                b("file.share", Show::Icon),
                b("item.trash", Show::Icon),
                ToolItem::Separator,
                b("item.properties", Show::Icon),
            ],
            ToolContext::RecycleBin => vec![b("bin.restore", Show::Both), b("bin.restore_all", Show::Both), b("bin.empty", Show::Both)],
            ToolContext::ThisPc => vec![b("item.properties", Show::Both)],
        };
        let mut items = head;
        items.extend(view.into_iter().filter(|i| self != ToolContext::ThisPc || *i != b("menu.select", Show::Both)));
        items
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolbarState {
    pub visible: bool,
    pub sets: [Vec<ToolItem>; 3],
}

impl Default for ToolbarState {
    fn default() -> Self {
        Self { visible: true, sets: CONTEXTS.map(ToolContext::defaults) }
    }
}

impl ToolbarState {
    pub fn sanitized(mut self) -> Self {
        for set in &mut self.sets {
            let mut seen: Vec<String> = Vec::new();
            set.retain(|item| match item {
                ToolItem::Separator => true,
                ToolItem::Button(id, _) if tool(id).is_some() && !seen.contains(id) => {
                    seen.push(id.clone());
                    true
                }
                ToolItem::Button(..) => false,
            });
            set.truncate(MAX_ITEMS);
        }
        self
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum FlyHit {
    Layout(usize),
    Slider,
    Toggle(usize),
    Panel,
    Outside,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum ToolHit {
    Button(usize),
    More,
    Bar,
    Fly(FlyHit),
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum Overflow {
    Action(&'static str, &'static str),
    Menu(ToolMenu, &'static str),
    Separator,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum ToolPick {
    Items(ToolContext),
    Item(ToolContext, usize),
    Add(ToolContext),
}

impl ToolPick {
    pub(super) fn placeholder(&self) -> &'static str {
        match self {
            ToolPick::Items(_) => S::ToolbarPickItems.t(),
            ToolPick::Item(..) => S::ToolbarPickItem.t(),
            ToolPick::Add(_) => S::ToolbarPickAdd.t(),
        }
    }
}

#[derive(Default)]
struct Fly {
    rect: Rect,
    rows: [Rect; 6],
    slider: Rect,
    toggles: [Rect; 4],
}

struct Tip {
    item: usize,
    at: Instant,
    shown: bool,
}

#[derive(Default)]
pub(super) struct Toolbar {
    pub state: ToolbarState,
    placed: Vec<(usize, Rect)>,
    overflow: Vec<usize>,
    more: Rect,
    fly: Option<Fly>,
    tip: Option<Tip>,
    sliding: bool,
    archive: bool,
}

impl App {
    pub fn toolbar_state(&self) -> ToolbarState {
        self.toolbar.state.clone()
    }

    pub fn set_toolbar_state(&mut self, s: ToolbarState) {
        self.toolbar.state = s.sanitized();
        self.layout();
        self.dirty = true;
    }

    pub fn toolbar_items(&self) -> Vec<ToolItem> {
        if self.tab().view_loc.is_home_page() {
            return Vec::new();
        }
        let mut items = self.toolbar.state.sets[self.tool_context().index()].clone();
        items.retain(|item| !matches!(item, ToolItem::Button(id, _) if !self.supports(id)));
        let extract = ToolItem::Button("menu.extract".into(), Show::Both);
        if self.toolbar.archive && self.supports("menu.extract") && !items.contains(&extract) {
            items.extend([ToolItem::Separator, extract]);
        }
        items
    }

    fn archive_context(&self) -> bool {
        let selected = self.tab().view.as_ref().is_some_and(|v| {
            let l = v.listing();
            self.selected_items().into_iter().any(|i| !l.entries[i as usize].is_dir() && archive::format_of(l.name(i)).is_some())
        });
        selected || self.archive_view().is_some()
    }

    pub(super) fn toolbar_h(&self) -> f32 {
        if self.toolbar.state.visible && !self.search.active && (self.split_shown() || !self.tab().view_loc.is_home_page()) {
            self.s(BAR_H)
        } else {
            0.0
        }
    }

    fn tool_context(&self) -> ToolContext {
        let loc = &self.tab().view_loc;
        match loc.path().kind() {
            _ if loc.is_recycle_bin() => ToolContext::RecycleBin,
            PathKind::Computer => ToolContext::ThisPc,
            _ => ToolContext::Folder,
        }
    }

    pub(super) fn toggle_toolbar(&mut self) {
        let st = &mut self.toolbar.state;
        st.visible = !st.visible;
        self.toolbar.fly = None;
        self.toolbar.tip = None;
        self.layout();
        self.dirty = true;
    }

    pub(super) fn reset_toolbar(&mut self) {
        let ctx = self.tool_context();
        self.toolbar.state.sets[ctx.index()] = ctx.defaults();
        self.status(ctx.reset_message().t());
    }

    pub(super) fn customize_toolbar(&mut self) {
        let ctx = self.tool_context();
        self.open_tool_pick(ToolPick::Items(ctx), 0);
    }

    pub(super) fn share_selection(&mut self) {
        let items = self.selected_items();
        if items.is_empty() {
            return self.status(S::ToolbarSelectToShare.t());
        }
        match self.item_paths(&items) {
            Ok(paths) => self.effects.push(Effect::Share(paths)),
            Err(e) => self.status(e),
        }
    }

    fn tool_enabled(&self, id: &str) -> bool {
        let t = self.tab();
        let selected = t.view.as_ref().is_some_and(|v| v.selected_count() > 0);
        let files = !t.view_loc.is_recycle_bin() && t.view_loc.path().kind() != PathKind::Computer;
        match id {
            "file.cut" | "file.copy" | "file.rename" | "file.share" | "item.trash" | "item.delete" => selected && files,
            "file.paste" | "menu.new" | "file.new_folder" | "file.new_item" | "app.terminal" => files,
            "bin.restore" => selected,
            _ => true,
        }
    }

    fn tool_checked(&self, id: &str) -> bool {
        match id {
            "view.pane" => self.preview.state.visible,
            "view.hidden" => self.tab().view_spec.show_hidden,
            "sidebar.toggle" => self.nav.sidebar.state.sidebar,
            "menu.layout" => self.toolbar.fly.is_some(),
            _ => false,
        }
    }

    pub(super) fn flyout_hit(&self, x: f32, y: f32) -> Option<Hit> {
        self.toolbar.fly.as_ref()?;
        self.toolbar_hit(x, y)
    }

    pub(super) fn toolbar_hit(&self, x: f32, y: f32) -> Option<Hit> {
        let tb = &self.toolbar;
        if let Some(f) = &tb.fly {
            let h = if !f.rect.contains(x, y) {
                return match tb.placed.iter().find(|(_, r)| r.contains(x, y)) {
                    Some(&(i, _)) if self.toolbar_items().get(i).is_some_and(|it| matches!(it, ToolItem::Button(id, _) if id == "menu.layout")) => Some(Hit::Tool(ToolHit::Button(i))),
                    _ => Some(Hit::Tool(ToolHit::Fly(FlyHit::Outside))),
                };
            } else if let Some(k) = f.rows.iter().position(|r| r.contains(x, y)) {
                FlyHit::Layout(k)
            } else if f.slider.inset(0.0, -self.s(6.0)).contains(x, y) {
                FlyHit::Slider
            } else if let Some(k) = f.toggles.iter().position(|r| r.contains(x, y)) {
                FlyHit::Toggle(k)
            } else {
                FlyHit::Panel
            };
            return Some(Hit::Tool(ToolHit::Fly(h)));
        }
        let row = self.layout.commands;
        if !row.contains(x, y) {
            return None;
        }
        if tb.more.contains(x, y) {
            return Some(Hit::Tool(ToolHit::More));
        }
        let button = tb.placed.iter().find(|(i, r)| r.contains(x, y) && matches!(self.toolbar_items().get(*i), Some(ToolItem::Button(..))));
        Some(Hit::Tool(button.map_or(ToolHit::Bar, |&(i, _)| ToolHit::Button(i))))
    }

    pub(super) fn toolbar_mouse_down(&mut self, h: Hit, x: f32, button: MouseButton) -> bool {
        let fly_click = matches!(h, Hit::Tool(ToolHit::Fly(_)) | Hit::Tool(ToolHit::Button(_)));
        if self.toolbar.fly.is_some() && !fly_click {
            self.toolbar.fly = None;
        }
        let Hit::Tool(t) = h else { return false };
        if self.editing {
            self.cancel_edit();
        }
        self.toolbar.tip = None;
        self.dirty = true;
        match t {
            ToolHit::Fly(FlyHit::Outside) => self.toolbar.fly = None,
            ToolHit::Fly(FlyHit::Slider) if button == MouseButton::Left => {
                self.toolbar.sliding = true;
                self.slide_to(x);
            }
            _ if button == MouseButton::Left => self.pressed = h,
            _ => {}
        }
        true
    }

    pub(super) fn toolbar_mouse_move(&mut self, x: f32) -> bool {
        if !self.toolbar.sliding {
            return false;
        }
        self.slide_to(x);
        true
    }

    pub(super) fn toolbar_mouse_up(&mut self, x: f32, y: f32, button: MouseButton) -> bool {
        if std::mem::take(&mut self.toolbar.sliding) {
            self.pressed = Hit::None;
            return true;
        }
        if button != MouseButton::Right {
            return false;
        }
        match self.hit(x, y) {
            Hit::Tool(ToolHit::Button(_) | ToolHit::More | ToolHit::Bar) => {
                let menu = self.build_toolbar_menu();
                self.flit_menu(x, y, menu);
                true
            }
            Hit::Tool(_) => true,
            _ => false,
        }
    }

    pub(super) fn toolbar_click(&mut self, h: ToolHit, now: Instant) {
        match h {
            ToolHit::Button(i) => self.press_tool(i, now),
            ToolHit::More => {
                let items = self.overflow_items();
                let r = self.toolbar.more;
                let menu = self.build_overflow_menu(&items);
                self.flit_menu(r.x, r.bottom(), menu);
            }
            ToolHit::Fly(FlyHit::Layout(k)) => {
                self.run_action(LAYOUT_IDS[k], now);
            }
            ToolHit::Fly(FlyHit::Toggle(k)) => self.fly_toggle(k, now),
            ToolHit::Fly(_) | ToolHit::Bar => {}
        }
    }

    fn press_tool(&mut self, i: usize, now: Instant) {
        let Some(ToolItem::Button(id, _)) = self.toolbar_items().get(i).cloned() else { return };
        let Some(tool) = tool(&id) else { return };
        if !self.tool_enabled(tool.id) {
            return;
        }
        let anchor = self.toolbar.placed.iter().find(|(k, _)| *k == i).map_or(self.layout.commands, |&(_, r)| r);
        match tool.kind {
            Kind::Action => {
                self.run_action(tool.id, now);
            }
            Kind::Menu(ToolMenu::Layout) => {
                self.toolbar.fly = match self.toolbar.fly {
                    Some(_) => None,
                    None => Some(Fly { rect: Rect::new(anchor.x, anchor.bottom(), 0.0, 0.0), ..Default::default() }),
                };
            }
            Kind::Menu(m) => {
                let menu = self.build_tool_menu(m);
                self.flit_menu(anchor.x, anchor.bottom(), menu);
            }
        }
    }

    fn overflow_items(&self) -> Vec<Overflow> {
        let items = self.toolbar_items();
        let mut out: Vec<Overflow> = Vec::new();
        for &k in &self.toolbar.overflow {
            match items.get(k) {
                Some(ToolItem::Separator) => {
                    if out.last().is_some_and(|o| *o != Overflow::Separator) {
                        out.push(Overflow::Separator);
                    }
                }
                Some(ToolItem::Button(id, _)) => match tool(id) {
                    Some(t) if t.kind == Kind::Action && self.tool_enabled(t.id) => out.push(Overflow::Action(t.id, t.label.t())),
                    Some(Tool { kind: Kind::Menu(m), label, .. }) => out.push(Overflow::Menu(*m, label.t())),
                    _ => {}
                },
                None => {}
            }
        }
        out
    }

    fn fly_toggle(&mut self, k: usize, now: Instant) {
        match k {
            0 => {
                self.run_action("view.hidden", now);
            }
            1 => {
                self.run_action("view.extensions", now);
            }
            2 => {
                let t = self.tab();
                let to = if t.mode == LayoutMode::Adaptive { t.look } else { LayoutMode::Adaptive };
                self.set_layout(to);
            }
            _ => {
                self.run_action("view.thumbnails", now);
            }
        }
    }

    fn fly_checked(&self, k: usize) -> bool {
        let t = self.tab();
        match k {
            0 => t.view_spec.show_hidden,
            1 => self.prefs.show_extensions,
            2 => t.mode == LayoutMode::Adaptive,
            _ => self.prefs.thumbnails,
        }
    }

    fn slide_to(&mut self, x: f32) {
        let Some(f) = &self.toolbar.fly else { return };
        let track = f.slider;
        let look = self.tab().look;
        let n = look.levels() as f32;
        let frac = ((x - track.x) / track.w.max(1.0)).clamp(0.0, 1.0);
        let level = (frac * (n - 1.0)).round() as i32;
        let current = self.tab().sizes.level(look) as i32;
        if level != current {
            self.resize_items(level - current);
        }
        self.dirty = true;
    }

    pub(super) fn toolbar_hover(&mut self, h: Hit, now: Instant) {
        let item = match h {
            Hit::Tool(ToolHit::Button(i)) => Some(i),
            _ => None,
        };
        match (item, &self.toolbar.tip) {
            (Some(i), Some(tip)) if tip.item == i => {}
            (Some(i), _) => {
                let shown = self.toolbar.tip.as_ref().is_some_and(|t| t.shown);
                let at = if shown { now } else { now + TIP_DELAY };
                self.toolbar.tip = Some(Tip { item: i, at, shown });
                if !shown {
                    self.arm_timer(now, at);
                }
                self.dirty |= shown;
            }
            (None, Some(_)) => {
                self.dirty |= self.toolbar.tip.take().is_some_and(|t| t.shown);
            }
            (None, None) => {}
        }
    }

    pub(super) fn toolbar_timer(&mut self, now: Instant) {
        let Some(tip) = &mut self.toolbar.tip else { return };
        if tip.shown {
            return;
        }
        if now + TIMER_SLACK < tip.at {
            let at = tip.at;
            return self.arm_timer(now, at);
        }
        tip.shown = true;
        self.dirty = true;
    }

    pub(super) fn toolbar_key(&mut self, key: Key) -> bool {
        self.toolbar.tip = None;
        if self.toolbar.fly.is_none() {
            return false;
        }
        self.toolbar.fly = None;
        self.dirty = true;
        key == Key::Escape
    }

    pub(super) fn tool_buttons(&self) -> Vec<(ToolHit, Rect, &'static str, &'static str, bool)> {
        let items = self.toolbar_items();
        let unplaced: Vec<(usize, Rect)>;
        let placed = if self.toolbar.placed.is_empty() && self.toolbar.more.w <= 0.0 && self.layout.commands.h > 0.0 {
            unplaced = (0..items.len()).map(|i| (i, Rect::default())).collect();
            &unplaced
        } else {
            &self.toolbar.placed
        };
        let mut out: Vec<_> = placed
            .iter()
            .filter_map(|&(i, r)| match items.get(i) {
                Some(ToolItem::Button(id, _)) => tool(id).map(|t| (ToolHit::Button(i), r, t.label.t(), t.id, self.tool_enabled(t.id))),
                _ => None,
            })
            .collect();
        if self.toolbar.more.w > 0.0 {
            out.push((ToolHit::More, self.toolbar.more, S::A11yButtonMore.t(), "", true));
        }
        out
    }

    pub fn toolbar_tip(&self) -> Option<String> {
        let tip = self.toolbar.tip.as_ref().filter(|t| t.shown)?;
        let items = self.toolbar_items();
        let Some(ToolItem::Button(id, _)) = items.get(tip.item) else { return None };
        let t = tool(id)?;
        Some(match self.keymap.first_text(t.id) {
            Some(k) => format!("{} ({k})", t.label.t()),
            None => t.label.t().to_owned(),
        })
    }

    pub(super) fn draw_toolbar_row<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme) {
        let row = self.layout.commands;
        self.toolbar.archive = row.h > 0.0 && self.archive_context();
        self.toolbar.placed.clear();
        self.toolbar.overflow.clear();
        self.toolbar.more = Rect::default();
        if row.h <= 0.0 {
            return;
        }
        g.dl.rect(row, t.bg);
        g.dl.rect(Rect::new(row.x, row.bottom() - 1.0, row.w, 1.0), t.separator);
        let (font, icons, chevron) = (self.ui_font(12.5), self.icon_font(13.0), self.icon_font(7.0));
        let m = g.metrics(font);
        let (pad, icon_w, sep_w) = (self.s(PAD), self.s(ICON_W), self.s(SEP_W));
        let bh = self.s(BUTTON_H);
        let items = self.toolbar_items();
        let width = |g: &mut Gfx<B>, item: &ToolItem| match item {
            ToolItem::Separator => sep_w,
            ToolItem::Button(id, show) => match (tool(id), show) {
                (None, _) => 0.0,
                (Some(_), Show::Icon) => icon_w,
                (Some(tl), show) => {
                    let mut w = g.measure(tl.label.t(), font) + 2.0 * pad;
                    if *show == Show::Both {
                        w += icon_w - pad;
                    }
                    if matches!(tl.kind, Kind::Menu(_)) {
                        w += self.s(12.0);
                    }
                    w
                }
            },
        };
        let widths: Vec<f32> = items.iter().map(|it| width(g, it)).collect();
        let left = row.x + self.s(6.0);
        let total: f32 = widths.iter().sum();
        let room = row.right() - self.s(6.0) - left;
        let limit = if total <= room { room } else { room - icon_w };
        let mut x = left;
        for (k, w) in widths.iter().enumerate() {
            if x + w > left + limit {
                self.toolbar.overflow.extend(k..items.len());
                break;
            }
            let r = Rect::new(x, row.y + ((row.h - bh) / 2.0).round(), *w, bh);
            self.toolbar.placed.push((k, r));
            x += w;
        }
        if !self.toolbar.overflow.is_empty() {
            self.toolbar.more = Rect::new(row.right() - self.s(6.0) - icon_w, row.y + ((row.h - bh) / 2.0).round(), icon_w, bh);
        }
        let radius = self.s(4.0);
        for &(k, r) in &self.toolbar.placed {
            let (id, show) = match &items[k] {
                ToolItem::Separator => {
                    g.dl.rect(Rect::new((r.x + r.w / 2.0).round(), r.y + self.s(6.0), 1.0, r.h - self.s(12.0)), t.separator);
                    continue;
                }
                ToolItem::Button(id, show) => (id.as_str(), *show),
            };
            let Some(tl) = tool(id) else { continue };
            let enabled = self.tool_enabled(tl.id);
            let hot = self.hover == Hit::Tool(ToolHit::Button(k)) && enabled;
            if self.tool_checked(tl.id) {
                g.dl.round_rect(r, t.selection, radius);
            } else if hot {
                g.dl.round_rect(r, if self.pressed == Hit::Tool(ToolHit::Button(k)) { t.button_pressed } else { t.button_hover }, radius);
            }
            let color = if enabled { t.text } else { t.text_dim.alpha(0x70) };
            let mut cx = r.x;
            if show != Show::Label {
                let icon_color = if enabled && matches!(tl.id, "item.trash" | "item.delete" | "bin.empty") { t.text } else { color };
                centered(g, Rect::new(r.x, r.y, icon_w, r.h), tl.glyph, icons, icon_color);
                cx += icon_w - pad;
            }
            if show != Show::Icon {
                let baseline = (r.y + (r.h + m.ascent - m.descent) / 2.0).round();
                let w = g.text(cx + pad, baseline, tl.label.t(), font, color, f32::INFINITY);
                if matches!(tl.kind, Kind::Menu(_)) {
                    g.text(cx + pad + w + self.s(5.0), baseline - self.s(1.0), G_DOWN, chevron, t.text_dim, f32::INFINITY);
                }
            }
        }
        let more = self.toolbar.more;
        if more.w > 0.0 {
            if self.hover == Hit::Tool(ToolHit::More) {
                g.dl.round_rect(more, t.button_hover, radius);
            }
            centered(g, more, G_MORE, icons, t.text);
        }
    }

    pub(super) fn draw_toolbar_popups<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme) {
        self.draw_fly(g, t);
        self.draw_tip(g, t);
    }

    fn draw_fly<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme) {
        let Some(anchor) = self.toolbar.fly.as_ref().map(|f| f.rect) else { return };
        let anchor = self.toolbar.placed.iter().find(|(k, _)| matches!(self.toolbar_items().get(*k), Some(ToolItem::Button(id, _)) if id == "menu.layout")).map_or(anchor, |&(_, r)| r);
        let (font, small, icons) = (self.ui_font(13.0), self.ui_font(12.0), self.icon_font(11.0));
        let m = g.metrics(font);
        let (s4, s6, s12, row_h) = (self.s(4.0), self.s(6.0), self.s(12.0), self.s(FLY_ROW));
        let w = self.s(FLY_W).min(self.size.0 - self.s(16.0));
        let x = anchor.x.min(self.size.0 - w - self.s(8.0)).max(self.s(8.0));
        let y = anchor.bottom() + s4;
        let mut f = Fly::default();
        let mut ry = y + s6;
        for r in f.rows.iter_mut() {
            *r = Rect::new(x + s6, ry, w - 2.0 * s6, row_h);
            ry += row_h;
        }
        ry += s6;
        let size_row = Rect::new(x + s6, ry, w - 2.0 * s6, self.s(36.0));
        let label_w = self.s(64.0);
        f.slider = Rect::new(size_row.x + label_w, size_row.y + size_row.h / 2.0 - self.s(2.0), size_row.w - label_w - s12, self.s(4.0));
        ry = size_row.bottom() + s6;
        let sep_y = ry;
        ry += s6;
        for r in f.toggles.iter_mut() {
            *r = Rect::new(x + s6, ry, w - 2.0 * s6, row_h);
            ry += row_h;
        }
        f.rect = Rect::new(x, y, w, ry + s6 - y);
        let r = f.rect;
        g.dl.shadow(Rect::new(r.x, r.y + s4, r.w, r.h), self.shadow(), s6, s6 * 2.0);
        g.dl.round_rect(r, t.bg, s6);
        g.dl.outline(r, t.input_border, s6, 1.0);
        let baseline = |r: Rect| (r.y + (r.h + m.ascent - m.descent) / 2.0).round();
        let tab = self.tab();
        let (mode, look) = (tab.mode, tab.look);
        let level = tab.sizes.level(look);
        for (k, &row) in f.rows.iter().enumerate() {
            let lm = LAYOUTS[k];
            if self.hover == Hit::Tool(ToolHit::Fly(FlyHit::Layout(k))) {
                g.dl.round_rect(row, t.hover, s4);
            }
            if lm == mode {
                g.dl.round_rect(Rect::new(row.x, row.y + s6, self.s(3.0), row.h - 2.0 * s6), t.accent, self.s(1.5));
                centered(g, Rect::new(row.x + s4, row.y, self.s(24.0), row.h), G_CHECK, icons, t.accent);
            }
            g.text(row.x + self.s(32.0), baseline(row), lm.title(), font, t.text, row.w);
            let keys = self.keymap.first_text(LAYOUT_IDS[k]).unwrap_or_default();
            let kw = g.measure(&keys, small);
            g.text(row.right() - s12 - kw, baseline(row), &keys, small, t.text_dim, f32::INFINITY);
        }
        g.text(size_row.x + self.s(10.0), baseline(size_row), S::ToolbarFlyoutSize.t(), font, t.text, label_w);
        let tr = f.slider;
        g.dl.round_rect(tr, t.separator, tr.h / 2.0);
        let n = look.levels() as usize;
        let frac = if n > 1 { level as f32 / (n - 1) as f32 } else { 0.0 };
        g.dl.round_rect(Rect::new(tr.x, tr.y, tr.w * frac, tr.h), t.accent, tr.h / 2.0);
        for k in 0..n {
            let sx = tr.x + tr.w * k as f32 / (n - 1).max(1) as f32;
            g.dl.round_rect(Rect::new(sx - 1.0, tr.y - self.s(3.0), 2.0, tr.h + self.s(6.0)), t.text_dim.alpha(0x60), 1.0);
        }
        let knob = self.s(14.0);
        let kx = tr.x + tr.w * frac - knob / 2.0;
        let hot = self.toolbar.sliding || self.hover == Hit::Tool(ToolHit::Fly(FlyHit::Slider));
        g.dl.round_rect(Rect::new(kx, tr.y + tr.h / 2.0 - knob / 2.0, knob, knob), if hot { t.accent } else { t.input_bg }, knob / 2.0);
        g.dl.outline(Rect::new(kx, tr.y + tr.h / 2.0 - knob / 2.0, knob, knob), t.accent, knob / 2.0, self.s(1.5));
        g.dl.rect(Rect::new(r.x + s6, sep_y, r.w - 2.0 * s6, 1.0), t.separator);
        for (k, &row) in f.toggles.iter().enumerate() {
            if self.hover == Hit::Tool(ToolHit::Fly(FlyHit::Toggle(k))) {
                g.dl.round_rect(row, t.hover, s4);
            }
            let bx = Rect::new(row.x + self.s(9.0), row.y + (row.h - self.s(14.0)) / 2.0, self.s(14.0), self.s(14.0));
            if self.fly_checked(k) {
                g.dl.round_rect(bx, t.accent, self.s(3.0));
                centered(g, bx, G_CHECK, self.icon_font(10.0), crate::theme::ink_on(t.accent));
            } else {
                g.dl.outline(bx, t.text_dim, self.s(3.0), 1.0);
            }
            g.text(row.x + self.s(32.0), baseline(row), TOGGLES[k].t(), font, t.text, row.w);
        }
        self.toolbar.fly = Some(f);
    }

    fn draw_tip<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme) {
        let Some(text) = self.toolbar_tip() else { return };
        let Some(item) = self.toolbar.tip.as_ref().map(|t| t.item) else { return };
        let Some(&(_, anchor)) = self.toolbar.placed.iter().find(|(k, _)| *k == item) else { return };
        let font = self.ui_font(12.0);
        let m = g.metrics(font);
        let (pad, h) = (self.s(8.0), self.s(26.0));
        let w = g.measure(&text, font) + 2.0 * pad;
        let x = anchor.x.min(self.size.0 - w - self.s(4.0)).max(self.s(4.0));
        let r = Rect::new(x, anchor.bottom() + self.s(6.0), w, h);
        g.dl.shadow(Rect::new(r.x, r.y + self.s(1.0), r.w, r.h), self.shadow(), self.s(4.0), self.s(4.0));
        g.dl.round_rect(r, t.chrome, self.s(4.0));
        g.dl.outline(r, t.input_border, self.s(4.0), 1.0);
        g.text(r.x + pad, (r.y + (r.h + m.ascent - m.descent) / 2.0).round(), &text, font, t.text, f32::INFINITY);
    }

    fn open_tool_pick(&mut self, pick: ToolPick, select: usize) {
        let label = |item: &ToolItem| match item {
            ToolItem::Separator => (S::ToolbarPickSeparator.t().to_owned(), String::new()),
            ToolItem::Button(id, show) => {
                let name = tool(id).map_or(id.as_str(), |t| t.label.t()).to_owned();
                let how = match show {
                    Show::Icon => S::ToolbarPickIconOnly.t(),
                    Show::Label => S::ToolbarPickLabelOnly.t(),
                    Show::Both => S::ToolbarPickIconAndLabel.t(),
                };
                (name, how.to_owned())
            }
        };
        let rows: Vec<(String, String)> = match &pick {
            ToolPick::Items(ctx) => {
                let set = &self.toolbar.state.sets[ctx.index()];
                let mut rows: Vec<(String, String)> = set.iter().map(label).collect();
                rows.push((S::ToolbarPickAddButton.t().into(), String::new()));
                rows.push((S::ToolbarPickAddSeparator.t().into(), String::new()));
                rows.push((S::ToolbarPickReset.t().into(), String::new()));
                for other in CONTEXTS.into_iter().filter(|c| c != ctx) {
                    rows.push((other.edit_label().t().to_owned(), String::new()));
                }
                rows
            }
            ToolPick::Item(ctx, k) => {
                let mut rows = [S::ToolbarPickMoveLeft, S::ToolbarPickMoveRight, S::ToolbarPickRemove].map(|s| (s.t().to_owned(), String::new())).to_vec();
                if matches!(self.toolbar.state.sets[ctx.index()].get(*k), Some(ToolItem::Button(..))) {
                    rows.extend([S::ToolbarPickShowIcon, S::ToolbarPickShowLabel, S::ToolbarPickShowBoth].map(|s| (s.t().to_owned(), String::new())));
                }
                rows
            }
            ToolPick::Add(ctx) => self.addable(*ctx).into_iter().map(|t| (t.label.t().to_owned(), self.keymap.first_text(t.id).unwrap_or_default())).collect(),
        };
        let mut p = Palette::new(Pick::Toolbar(pick), rows, "");
        p.select_row(select);
        self.open_pick(p);
    }

    fn addable(&self, ctx: ToolContext) -> Vec<&'static Tool> {
        let set = &self.toolbar.state.sets[ctx.index()];
        TOOLS.iter().filter(|t| self.supports(t.id) && !set.iter().any(|i| matches!(i, ToolItem::Button(id, _) if id == t.id))).collect()
    }

    pub(super) fn tool_choose(&mut self, pick: ToolPick, id: usize) {
        match pick {
            ToolPick::Items(ctx) => {
                let n = self.toolbar.state.sets[ctx.index()].len();
                match id.checked_sub(n) {
                    None => self.open_tool_pick(ToolPick::Item(ctx, id), 0),
                    Some(0) => self.open_tool_pick(ToolPick::Add(ctx), 0),
                    Some(1) => {
                        let set = &mut self.toolbar.state.sets[ctx.index()];
                        if set.len() < MAX_ITEMS {
                            set.push(ToolItem::Separator);
                        }
                        self.open_tool_pick(ToolPick::Items(ctx), n);
                    }
                    Some(2) => {
                        self.toolbar.state.sets[ctx.index()] = ctx.defaults();
                        self.open_tool_pick(ToolPick::Items(ctx), 0);
                    }
                    Some(k) => {
                        let others: Vec<ToolContext> = CONTEXTS.into_iter().filter(|c| *c != ctx).collect();
                        if let Some(&other) = others.get(k - 3) {
                            self.open_tool_pick(ToolPick::Items(other), 0);
                        }
                    }
                }
            }
            ToolPick::Item(ctx, k) => {
                let set = &mut self.toolbar.state.sets[ctx.index()];
                if k >= set.len() {
                    return;
                }
                let at = match id {
                    0 if k > 0 => {
                        set.swap(k, k - 1);
                        k - 1
                    }
                    1 if k + 1 < set.len() => {
                        set.swap(k, k + 1);
                        k + 1
                    }
                    2 => {
                        set.remove(k);
                        k.min(set.len().saturating_sub(1))
                    }
                    3..=5 => {
                        if let ToolItem::Button(_, show) = &mut set[k] {
                            *show = [Show::Icon, Show::Label, Show::Both][id - 3];
                        }
                        k
                    }
                    _ => k,
                };
                self.open_tool_pick(ToolPick::Items(ctx), at);
            }
            ToolPick::Add(ctx) => {
                let Some(t) = self.addable(ctx).get(id).copied() else { return };
                let set = &mut self.toolbar.state.sets[ctx.index()];
                if set.len() < MAX_ITEMS {
                    set.push(ToolItem::Button(t.id.to_owned(), if t.kind == Kind::Action { Show::Icon } else { Show::Both }));
                }
                let at = set.len() - 1;
                self.open_tool_pick(ToolPick::Items(ctx), at);
            }
        }
        self.dirty = true;
    }
}

#[cfg(test)]
#[path = "../../test/ui/toolbar.rs"]
mod tests;
