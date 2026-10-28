use super::columns::{pane_depth, COLUMN_W};
use super::details::{folder_key, CHECK, CHECK_GAP};
use super::flow::{Flow, FlowParams};
use super::{App, Effect, Hit, Tab, ROW_H, SCROLLBAR_W};
use crate::desktop_ini::FolderKind;
use crate::i18n::S;
use crate::input::{Key, Mods};
use crate::render::Rect;
use crate::view::DirView;
use serde::{Deserialize, Serialize};
use std::ops::Range;
use std::time::Instant;

const LEVELS: [u8; 5] = [5, 5, 4, 5, 12];
const ROWS: [f32; 5] = [20.0, 24.0, 28.0, 32.0, 40.0];
const ROW_ICONS: [f32; 5] = [16.0, 16.0, 20.0, 24.0, 32.0];
const LIST_W: [f32; 5] = [160.0, 200.0, 240.0, 280.0, 340.0];
const CARD_ICONS: [f32; 4] = [32.0, 48.0, 64.0, 96.0];
const CARD_W: [f32; 4] = [240.0, 280.0, 320.0, 380.0];
const GRID_ICONS: [f32; 12] = [32.0, 40.0, 48.0, 64.0, 80.0, 96.0, 128.0, 160.0, 192.0, 224.0, 256.0, 320.0];
const GROUP_H: f32 = 32.0;
const CARD_LINE: f32 = 18.0;
const MEDIA_SHARE: (usize, usize) = (3, 5);
const ADAPTIVE_CAP: usize = 4096;

const IMAGES: [&[u8]; 20] = [
    b"jpg", b"jpeg", b"jpe", b"jfif", b"png", b"gif", b"bmp", b"webp", b"heic", b"heif", b"avif", b"jxl", b"tif", b"tiff", b"ico", b"dng", b"cr2", b"cr3", b"nef", b"arw",
];
const VIDEOS: [&[u8]; 14] = [b"mp4", b"m4v", b"mkv", b"avi", b"mov", b"wmv", b"webm", b"mpg", b"mpeg", b"3gp", b"flv", b"ts", b"m2ts", b"mts"];
const DOCUMENTS: [&[u8]; 5] = [b"pdf", b"docx", b"xlsx", b"pptx", b"psd"];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LayoutMode {
    #[default]
    Details,
    List,
    Cards,
    Columns,
    Grid,
    Adaptive,
}

pub const LAYOUTS: [LayoutMode; 6] = [LayoutMode::Details, LayoutMode::List, LayoutMode::Cards, LayoutMode::Columns, LayoutMode::Grid, LayoutMode::Adaptive];

impl LayoutMode {
    pub fn title(self) -> &'static str {
        match self {
            LayoutMode::Details => S::LayoutsDetails.t(),
            LayoutMode::List => S::LayoutsList.t(),
            LayoutMode::Cards => S::LayoutsCards.t(),
            LayoutMode::Columns => S::LayoutsColumns.t(),
            LayoutMode::Grid => S::LayoutsGrid.t(),
            LayoutMode::Adaptive => S::LayoutsAdaptive.t(),
        }
    }

    pub(super) fn slot(self) -> usize {
        match self {
            LayoutMode::Details | LayoutMode::Adaptive => 0,
            LayoutMode::List => 1,
            LayoutMode::Cards => 2,
            LayoutMode::Columns => 3,
            LayoutMode::Grid => 4,
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            LayoutMode::Details | LayoutMode::List => "\u{E8FD}",
            LayoutMode::Cards | LayoutMode::Adaptive => "\u{E8A9}",
            LayoutMode::Columns => "\u{E8A0}",
            LayoutMode::Grid => "\u{E8B9}",
        }
    }

    pub fn levels(self) -> u8 {
        LEVELS[self.slot()]
    }

    fn rows(self) -> bool {
        matches!(self, LayoutMode::Details | LayoutMode::List | LayoutMode::Columns | LayoutMode::Adaptive)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sizes(pub [u8; 5]);

impl Default for Sizes {
    fn default() -> Self {
        Self([1, 1, 1, 1, 5])
    }
}

impl Sizes {
    pub fn level(self, m: LayoutMode) -> usize {
        self.0[m.slot()] as usize
    }

    pub fn sanitized(self) -> Self {
        let d = Self::default();
        Self(std::array::from_fn(|k| if self.0[k] < LEVELS[k] { self.0[k] } else { d.0[k] }))
    }
}

fn ext_in(ext: &[u8], set: &[&[u8]]) -> bool {
    set.iter().any(|e| e.eq_ignore_ascii_case(ext))
}

pub fn is_media(ext: &[u8]) -> bool {
    ext_in(ext, &IMAGES) || ext_in(ext, &VIDEOS)
}

pub fn has_thumbnail(ext: &[u8]) -> bool {
    is_media(ext) || ext_in(ext, &DOCUMENTS)
}

pub fn adaptive_layout(kind: FolderKind, v: &DirView) -> LayoutMode {
    match kind {
        FolderKind::Pictures | FolderKind::Videos => return LayoutMode::Grid,
        FolderKind::Documents | FolderKind::Music | FolderKind::Generic => return LayoutMode::Details,
        FolderKind::Unknown => {}
    }
    let (l, rows) = (v.listing(), v.base_rows());
    let media = rows.iter().filter(|&&i| !l.entries[i as usize].is_dir() && is_media(l.ext(i))).count();
    if !rows.is_empty() && media * MEDIA_SHARE.1 > rows.len() * MEDIA_SHARE.0 { LayoutMode::Grid } else { LayoutMode::Details }
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Parts {
    pub body: Rect,
    pub icon: Rect,
    pub check: Rect,
    pub name: Rect,
    pub center: bool,
}

enum Step {
    Rows(isize),
    Bands(isize),
}

impl App {
    fn shown_look(&self) -> (LayoutMode, usize) {
        let t = self.tab();
        if self.search.active {
            return (LayoutMode::Details, 1);
        }
        (t.look, t.sizes.level(t.look))
    }

    pub(super) fn layout_look(&mut self) {
        let (mode, level) = self.shown_look();
        let depth = if mode == LayoutMode::Columns { pane_depth(&self.tab().view_loc) } else { 0 };
        let sc = self.scale;
        let s = |v: f32| (v * sc).round();
        let l = &mut self.layout;
        l.mode = mode;
        let row = mode.rows();
        l.row_h = if row { s(ROWS[level]) } else { s(ROW_H) };
        l.icon = s(match mode {
            LayoutMode::Cards => CARD_ICONS[level],
            LayoutMode::Grid => GRID_ICONS[level],
            _ => ROW_ICONS[level],
        });
        if mode != LayoutMode::Details {
            l.header.h = 0.0;
        }
        let (top, bottom) = (l.header.bottom(), l.bottom);
        let (x, w) = (l.header.x, l.header.w);
        let bar = s(SCROLLBAR_W);
        let across = mode == LayoutMode::List;
        let avail = (bottom - top).max(0.0);
        let cw = s(COLUMN_W[level.min(COLUMN_W.len() - 1)]);
        let panes = depth.min((((w - bar) / cw).floor() as usize).saturating_sub(1));
        l.panes.clear();
        l.panes.extend((0..panes).map(|k| Rect::new(x + k as f32 * cw, top, cw, avail)));
        let (x, w) = (x + panes as f32 * cw, w - panes as f32 * cw);
        if across {
            l.list = Rect::new(x, top, w, (avail - bar).max(0.0));
            l.track = Rect::new(x, l.list.bottom(), w, bar.min(avail));
        } else {
            l.list = Rect::new(x, top, (w - bar).max(0.0), avail);
            l.track = Rect::new(l.list.right(), top, bar, avail);
        }
        let list = l.list;
        let per = |cell: f32, room: f32| ((room / cell.max(1.0)).floor() as usize).max(1);
        l.flow = match mode {
            LayoutMode::List => {
                let rh = l.row_h;
                FlowParams { across: true, per: per(rh, list.h), cell: (s(LIST_W[level]), rh), head: rh }
            }
            LayoutMode::Cards => {
                let cw = s(CARD_W[level]);
                let ch = (l.icon + s(16.0)).max(s(3.0 * CARD_LINE + 16.0));
                FlowParams { across: false, per: per(cw, list.w), cell: (cw, ch), head: s(GROUP_H) }
            }
            LayoutMode::Grid => {
                let tw = (l.icon + s(32.0)).max(s(88.0));
                FlowParams { across: false, per: per(tw, list.w), cell: (tw, l.icon + s(44.0)), head: s(GROUP_H) }
            }
            _ => FlowParams { across: false, per: 1, cell: (list.w, l.row_h), head: l.row_h },
        };
    }

    pub(super) fn flow(&self) -> Flow {
        Flow::new(self.layout.flow, self.tab().view.as_ref())
    }

    pub(super) fn view_len(&self) -> f32 {
        if self.layout.flow.across { self.layout.list.w } else { self.layout.list.h }
    }

    pub(super) fn along_of(&self, x: f32, y: f32) -> f32 {
        if self.layout.flow.across { x } else { y }
    }

    pub(super) fn max_scroll_of(&self, f: &Flow) -> f32 {
        (f.extent() - self.view_len()).max(0.0)
    }

    pub(super) fn max_scroll(&self) -> f32 {
        self.max_scroll_of(&self.flow())
    }

    pub(super) fn line_rect(&self, f: &Flow, line: usize) -> Rect {
        let (c, l, s) = (f.cell(line), self.layout.list, self.tab().scroll);
        if self.layout.flow.across {
            Rect::new((l.x + c.x - s).round(), l.y + c.y, c.w, c.h)
        } else {
            Rect::new(l.x + c.x, (l.y + c.y - s).round(), c.w, c.h)
        }
    }

    pub(super) fn visible_lines(&self, f: &Flow) -> Range<usize> {
        let s = self.tab().scroll;
        f.lines_in(s, s + self.view_len())
    }

    pub(super) fn line_at(&self, f: &Flow, x: f32, y: f32) -> Option<usize> {
        let (l, s) = (self.layout.list, self.tab().scroll);
        let (lx, ly) = (x - l.x, y - l.y);
        if self.layout.flow.across { f.line_at(lx + s, ly) } else { f.line_at(ly + s, lx) }
    }

    pub(super) fn nearest_line(&self, f: &Flow, cx: f32, cy: f32) -> Option<usize> {
        if self.layout.flow.across { f.nearest(cx, cy) } else { f.nearest(cy, cx) }
    }

    pub(super) fn to_content(&self, x: f32, y: f32) -> (f32, f32) {
        let (l, s) = (self.layout.list, self.tab().scroll);
        if self.layout.flow.across { (x - l.x + s, y - l.y) } else { (x - l.x, y - l.y + s) }
    }

    pub(super) fn to_screen(&self, cx: f32, cy: f32) -> (f32, f32) {
        let (l, s) = (self.layout.list, self.tab().scroll);
        if self.layout.flow.across { (cx + l.x - s, cy + l.y) } else { (cx + l.x, cy + l.y - s) }
    }

    pub(super) fn list_hit(&self, x: f32, y: f32) -> Hit {
        let f = self.flow();
        let Some((n, line)) = self.line_at(&f, x, y).and_then(|n| Some((n, self.tab().view.as_ref()?.line(n)?))) else { return Hit::ListEmpty };
        match line {
            crate::view::Line::Header(g) => Hit::Group(g),
            crate::view::Line::Row(r) if self.layout.mode.rows() || self.item_parts(self.line_rect(&f, n)).body.contains(x, y) => Hit::Row(r),
            crate::view::Line::Row(_) => Hit::ListEmpty,
        }
    }

    pub(super) fn item_parts(&self, c: Rect) -> Parts {
        let (s4, s8, s10) = (self.s(4.0), self.s(8.0), self.s(10.0));
        let icon = self.layout.icon;
        let box_px = self.s(CHECK);
        let check = if self.prefs.checkboxes { self.s(CHECK + CHECK_GAP) } else { 0.0 };
        let mid = |r: Rect, h: f32| r.y + ((r.h - h) / 2.0).round();
        match self.layout.mode {
            LayoutMode::Cards => {
                let b = c.inset(s4, s4);
                let ir = Rect::new(b.x + s8 + check, mid(b, icon), icon, icon);
                let nx = ir.right() + s10;
                let line = self.s(CARD_LINE);
                let name = Rect::new(nx, mid(b, 3.0 * line), (b.right() - s8 - nx).max(0.0), line);
                Parts { body: b, icon: ir, check: Rect::new(b.x + s8, mid(b, box_px), box_px, box_px), name, center: false }
            }
            LayoutMode::Grid => {
                let b = c.inset(s4, s4);
                let ir = Rect::new(b.x + ((b.w - icon) / 2.0).round(), b.y + s8, icon, icon);
                let name = Rect::new(b.x + s4, ir.bottom() + s4, (b.w - s8).max(0.0), self.s(20.0));
                Parts { body: b, icon: ir, check: Rect::new(b.x + s4, b.y + s4, box_px, box_px), name, center: true }
            }
            mode => {
                let ir = Rect::new(c.x + s10 + check, mid(c, icon), icon, icon);
                let right = match mode {
                    LayoutMode::Details => self.layout.cols.first().map_or(c.right(), |col| col.right()),
                    _ => c.right(),
                };
                let nx = ir.right() + s8;
                Parts { body: c.inset(s4, 1.0), icon: ir, check: Rect::new(c.x + s10, mid(c, box_px), box_px, box_px), name: Rect::new(nx, c.y, (right - s10 - nx).max(0.0), c.h), center: false }
            }
        }
    }

    pub(super) fn row_rect(&self, row: usize) -> Option<Rect> {
        let v = self.tab().view.as_ref()?;
        (row < v.rows().len()).then(|| self.line_rect(&self.flow(), v.line_of_row(row)))
    }

    pub(super) fn on_checkbox(&self, x: f32, y: f32) -> bool {
        if !self.prefs.checkboxes {
            return false;
        }
        let f = self.flow();
        let Some(line) = self.line_at(&f, x, y) else { return false };
        let r = self.item_parts(self.line_rect(&f, line)).check;
        if self.layout.mode.rows() {
            return x >= r.x - self.s(10.0) && x < r.right() + self.s(CHECK_GAP / 2.0);
        }
        r.inset(-self.s(4.0), -self.s(4.0)).contains(x, y)
    }

    pub(super) fn on_name(&self, x: f32, y: f32, row: usize) -> bool {
        match self.layout.mode {
            LayoutMode::Details => self.layout.cols.first().is_some_and(|c| x < c.right()),
            _ => self.row_rect(row).is_some_and(|c| self.item_parts(c).name.inset(0.0, -self.s(4.0)).contains(x, y)),
        }
    }

    pub(super) fn name_edit_rect(&self, cell: Rect) -> Rect {
        let p = self.item_parts(cell);
        let (s2, s4) = (self.s(2.0), self.s(4.0));
        match self.layout.mode {
            LayoutMode::Cards => Rect::new(p.name.x - s4, p.name.y - s2, p.name.w + s4, p.name.h + 2.0 * s2),
            LayoutMode::Grid => Rect::new(p.body.x, p.name.y - s2, p.body.w, p.name.h + 2.0 * s2),
            _ => {
                let x0 = p.icon.right() + s4;
                let right = p.name.right() + self.s(10.0) - s4;
                Rect::new(x0, cell.y + 1.0, (right - x0).max(self.s(60.0)), cell.h - 2.0)
            }
        }
    }

    pub(super) fn reveal_focus(&mut self, center: bool) {
        let f = self.flow();
        let t = self.tab();
        let Some(line) = t.view.as_ref().and_then(|v| v.focus.and_then(|i| v.row_of(i)).map(|r| v.line_of_row(r))) else { return };
        let c = f.cell(line);
        let (top, ext) = if self.layout.flow.across { (c.x, c.w) } else { (c.y, c.h) };
        let (len, max) = (self.view_len(), self.max_scroll_of(&f));
        let t = self.tab_mut();
        let bottom = top + ext;
        let mut s = t.scroll_target;
        if center && (top < s || bottom > s + len) {
            s = top - (len - ext) / 2.0;
        } else if top < s {
            s = top;
        } else if bottom > s + len {
            s = bottom - len;
        }
        t.scroll_target = s.clamp(0.0, max);
        t.scroll = t.scroll_target;
    }

    pub(super) fn page_rows(&self) -> isize {
        let p = self.layout.flow;
        let ext = if p.across { p.cell.0 } else { p.cell.1 };
        ((self.view_len() / ext.max(1.0)).floor() as isize).max(1) * p.per.max(1) as isize
    }

    pub(super) fn wheel_step(&self) -> f32 {
        let p = self.layout.flow;
        match self.layout.mode {
            LayoutMode::List => p.cell.0,
            LayoutMode::Cards | LayoutMode::Grid => (p.cell.1 / 2.0).max(3.0 * self.layout.row_h),
            _ => 3.0 * self.layout.row_h,
        }
    }

    fn arrow_step(&self, key: Key) -> Option<Step> {
        let d = if matches!(key, Key::Up | Key::Left) { -1 } else { 1 };
        let vertical = matches!(key, Key::Up | Key::Down);
        match (self.layout.mode, vertical) {
            (LayoutMode::List, true) | (LayoutMode::Cards | LayoutMode::Grid, false) => Some(Step::Rows(d)),
            (LayoutMode::List, false) | (LayoutMode::Cards | LayoutMode::Grid, true) => Some(Step::Bands(d)),
            (_, true) => Some(Step::Rows(d)),
            (_, false) => None,
        }
    }

    pub(super) fn arrow_key(&mut self, key: Key, m: Mods, now: Instant) -> bool {
        if self.layout.mode == LayoutMode::Columns && !m.ctrl && !m.shift {
            match key {
                Key::Left => {
                    self.go_up(now);
                    return true;
                }
                Key::Right => {
                    self.enter_focused(now);
                    return true;
                }
                _ => {}
            }
        }
        match self.arrow_step(key) {
            Some(Step::Rows(d)) => self.tab_mut().move_focus(d, m),
            Some(Step::Bands(d)) => self.move_bands(d, m),
            None => return false,
        }
        self.reveal_focus(false);
        true
    }

    fn move_bands(&mut self, d: isize, m: Mods) {
        let f = self.flow();
        let t = self.tab_mut();
        let Some(v) = &t.view else { return };
        let Some(line) = v.focus.and_then(|i| v.row_of(i)).map(|r| v.line_of_row(r)) else { return t.move_focus(d, m) };
        let to = (0..d.unsigned_abs()).try_fold(line, |at, _| f.step(at, d.signum()).ok_or(at)).unwrap_or_else(|at| at);
        if to != line {
            let row = v.row_at_line(to);
            t.focus_row(row, m);
        }
    }

    fn enter_focused(&mut self, now: Instant) {
        let t = self.tab();
        let Some(i) = t.view.as_ref().and_then(|v| v.focus.filter(|&i| v.is_visible(i) && v.listing().entries[i as usize].is_dir())) else { return };
        self.open_entry(i, now);
    }

    fn concrete(&self, k: usize, mode: LayoutMode) -> LayoutMode {
        let t = &self.tabs[k];
        match (mode, &t.view) {
            (LayoutMode::Adaptive, Some(v)) => self.adaptive.get(&folder_key(&t.view_loc)).copied().unwrap_or_else(|| adaptive_layout(t.slot.kind(v.snap().gen), v)),
            (LayoutMode::Adaptive, None) => LayoutMode::Details,
            (m, _) => m,
        }
    }

    pub(super) fn set_layout(&mut self, mode: LayoutMode) {
        if mode == LayoutMode::Adaptive && self.prefs.same_for_all {
            return self.status(S::LayoutsAdaptiveShared.t());
        }
        let look = self.concrete(self.cur, mode);
        let t = self.tab_mut();
        if t.look != look {
            (t.scroll, t.scroll_target) = (0.0, 0.0);
            t.reveal_pending = true;
        }
        self.change_view(|t| (t.mode, t.look) = (mode, look));
    }

    pub(super) fn resize_items(&mut self, d: i32) {
        if self.search.active {
            return;
        }
        let look = self.tab().look;
        let level = (self.tab().sizes.level(look) as i32 + d).clamp(0, look.levels() as i32 - 1) as u8;
        if level as usize != self.tab().sizes.level(look) {
            self.tab_mut().reveal_pending = true;
            self.change_view(|t| t.sizes.0[look.slot()] = level);
        }
    }

    pub(super) fn clear_thumbnails(&mut self) {
        self.thumbs.clear();
        self.effects.push(Effect::ClearThumbnails);
        self.status(S::LayoutsThumbnailsCleared.t());
    }

    pub(super) fn settle_look(&mut self, k: usize, fresh: bool, done: bool) {
        let t = &self.tabs[k];
        let (Some(v), LayoutMode::Adaptive) = (&t.view, t.mode) else {
            if fresh {
                self.tabs[k].look = self.tabs[k].mode;
            }
            return;
        };
        let key = folder_key(&t.view_loc);
        let decided = adaptive_layout(t.slot.kind(v.snap().gen), v);
        if fresh {
            self.tabs[k].look = self.adaptive.get(&key).copied().unwrap_or(decided);
        }
        if done {
            if self.adaptive.len() >= ADAPTIVE_CAP {
                self.adaptive.clear();
            }
            self.adaptive.insert(key, decided);
        }
    }
}

impl Tab {
    pub(super) fn focus_row(&mut self, row: usize, m: Mods) {
        let Some(v) = &mut self.view else { return };
        let Some(&i) = v.rows().get(row) else { return };
        if m.shift {
            let anchor_row = v.anchor.and_then(|a| v.row_of(a)).unwrap_or(row);
            if !m.ctrl {
                v.clear_selection();
            }
            v.select_rows(anchor_row, row);
        } else if !m.ctrl {
            v.clear_selection();
            v.set_selected(i, true);
            v.anchor = Some(i);
        }
        v.focus = Some(i);
    }
}

#[cfg(test)]
#[path = "../../test/ui/layouts.rs"]
mod tests;
