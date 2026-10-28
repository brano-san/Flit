use super::menu::{Mark, MenuItem};
use super::widgets::{baseline, Paint};
use crate::input::Key;
use crate::render::Rect;
use crate::text::{FontFamily, FontStyle, Gfx, TextBackend};
use crate::theme::{ink_on, Theme};

const ROW_H: f32 = 28.0;
const SEPARATOR_H: f32 = 9.0;
const PAD: f32 = 4.0;
const LABEL_X: f32 = 32.0;
const MARK: f32 = 14.0;
const KEYS_GAP: f32 = 40.0;
const ARROW_W: f32 = 28.0;
const MIN_W: f32 = 160.0;
const RADIUS: f32 = 6.0;
const ARM_DISTANCE: f32 = 4.0;
const FONT_PX: f32 = 13.0;
const ICON_PX: f32 = 11.0;
const G_SUBMENU: &str = "\u{E76C}";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuOutcome {
    Open,
    Command(usize),
    Toggled(usize),
    Closed,
}

enum Node {
    Command { label: String, keys: String, checked: bool, stay: bool, mark: Mark, id: usize },
    Submenu { label: String, list: usize },
    Separator,
}

impl Node {
    fn label(&self) -> &str {
        match self {
            Node::Command { label, .. } | Node::Submenu { label, .. } => label,
            Node::Separator => "",
        }
    }

    fn height(&self, scale: f32) -> f32 {
        (if matches!(self, Node::Separator) { SEPARATOR_H } else { ROW_H }) * scale
    }
}

struct List {
    nodes: Vec<Node>,
    width: f32,
    height: f32,
}

#[derive(Clone, Copy, PartialEq)]
struct Panel {
    list: usize,
    rect: Rect,
    hot: Option<usize>,
}

pub struct MenuView {
    lists: Vec<List>,
    panels: Vec<Panel>,
    bounds: Rect,
    scale: f32,
    start: (f32, f32),
    armed: bool,
}

struct Measure<'a, B: TextBackend> {
    g: &'a mut Gfx<B>,
    font: FontStyle,
    scale: f32,
    next_id: usize,
}

impl<B: TextBackend> Measure<'_, B> {
    fn flatten(&mut self, items: &[MenuItem], lists: &mut Vec<List>) -> usize {
        let at = lists.len();
        lists.push(List { nodes: Vec::new(), width: 0.0, height: 0.0 });
        let s = self.scale;
        let (mut width, mut height) = (MIN_W * s, 2.0 * PAD * s);
        let mut nodes = Vec::with_capacity(items.len());
        for item in items {
            let node = match item {
                MenuItem::Command { label, keys, checked, stay, mark } => {
                    self.next_id += 1;
                    Node::Command { label: label.clone(), keys: keys.clone(), checked: *checked, stay: *stay, mark: *mark, id: self.next_id - 1 }
                }
                MenuItem::Submenu { label, items } => Node::Submenu { label: label.clone(), list: self.flatten(items, lists) },
                MenuItem::Separator => Node::Separator,
            };
            let keys = match &node {
                Node::Command { keys, .. } if !keys.is_empty() => KEYS_GAP * s + self.g.measure(keys, self.font),
                _ => 0.0,
            };
            if !matches!(node, Node::Separator) {
                width = width.max((LABEL_X * s + self.g.measure(node.label(), self.font) + keys + (ARROW_W + PAD) * s).ceil());
            }
            height += node.height(s);
            nodes.push(node);
        }
        lists[at] = List { nodes, width, height };
        at
    }
}

impl MenuView {
    pub fn new<B: TextBackend>(items: &[MenuItem], x: f32, y: f32, bounds: Rect, scale: f32, g: &mut Gfx<B>) -> Self {
        let mut lists = Vec::new();
        Measure { g, font: font(scale), scale, next_id: 0 }.flatten(items, &mut lists);
        let (w, h) = (lists[0].width, lists[0].height);
        let below = y + h <= bounds.bottom();
        let top = if below { y } else if y - h >= bounds.y { y - h } else { bounds.bottom() - h };
        let mut view = Self { lists, panels: Vec::new(), bounds, scale, start: (x, y), armed: false };
        let rect = view.fit(w, h, x, x, top);
        view.panels.push(Panel { list: 0, rect, hot: None });
        view
    }

    pub fn align_right(&mut self, right: f32) {
        let b = self.bounds;
        if let Some(p) = self.panels.first_mut() {
            p.rect.x = (right - p.rect.w).min(b.right() - p.rect.w).max(b.x);
        }
    }

    pub fn panels(&self) -> impl Iterator<Item = Rect> + '_ {
        self.panels.iter().map(|p| p.rect)
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        self.panels.iter().any(|p| p.rect.contains(x, y))
    }

    pub fn mouse_move(&mut self, x: f32, y: f32) -> bool {
        if !self.armed && (x - self.start.0).hypot(y - self.start.1) > ARM_DISTANCE * self.scale {
            self.armed = true;
        }
        let before = self.panels.clone();
        match self.hit(x, y) {
            Some((k, row)) => self.focus(k, row),
            None => {
                if let Some(top) = self.panels.last_mut() {
                    top.hot = None;
                }
            }
        }
        before != self.panels
    }

    pub fn mouse_down(&mut self, x: f32, y: f32) -> MenuOutcome {
        self.armed = true;
        if self.contains(x, y) { MenuOutcome::Open } else { MenuOutcome::Closed }
    }

    pub fn mouse_up(&mut self, x: f32, y: f32) -> MenuOutcome {
        if !self.armed {
            return MenuOutcome::Open;
        }
        let Some((k, Some(row))) = self.hit(x, y) else { return MenuOutcome::Open };
        if let Some(outcome) = self.pick(k, row) {
            return outcome;
        }
        self.focus(k, Some(row));
        MenuOutcome::Open
    }

    pub fn key(&mut self, key: Key) -> MenuOutcome {
        let k = self.panels.len() - 1;
        match key {
            Key::Escape | Key::Left if k > 0 => {
                self.panels.pop();
            }
            Key::Escape => return MenuOutcome::Closed,
            Key::Down | Key::Up | Key::Home | Key::End => self.step(key),
            Key::Char(c) => self.jump(c),
            Key::Right | Key::Enter | Key::Space => {
                let Some(row) = self.panels[k].hot else { return MenuOutcome::Open };
                match self.lists[self.panels[k].list].nodes[row] {
                    Node::Command { .. } if key != Key::Right => return self.pick(k, row).unwrap_or(MenuOutcome::Open),
                    Node::Submenu { .. } => {
                        self.focus(k, Some(row));
                        self.step(Key::Home);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        MenuOutcome::Open
    }

    fn pick(&mut self, k: usize, row: usize) -> Option<MenuOutcome> {
        let list = self.panels[k].list;
        match &mut self.lists[list].nodes[row] {
            Node::Command { stay: true, checked, id, .. } => {
                *checked = !*checked;
                Some(MenuOutcome::Toggled(*id))
            }
            Node::Command { id, .. } => Some(MenuOutcome::Command(*id)),
            _ => None,
        }
    }

    pub fn draw<B: TextBackend>(&self, g: &mut Gfx<B>, t: &Theme) {
        let s = self.scale;
        let (font, icons) = (font(s), FontStyle::new(FontFamily::Icons, ICON_PX * s, false));
        let mut p = Paint::new(g, t, s);
        for panel in &self.panels {
            let r = panel.rect;
            p.frame(r);
            for (i, row) in self.rows(panel) {
                let node = &self.lists[panel.list].nodes[i];
                if let Node::Separator = node {
                    p.separator(row.x + PAD * s, row.y + row.h / 2.0, row.w - 2.0 * PAD * s);
                    continue;
                }
                let hot = panel.hot == Some(i);
                if hot {
                    p.g.dl.round_rect(row, t.accent_fill, RADIUS * s * 0.66);
                }
                let (ink, dim) = if hot { (ink_on(t.accent_fill), ink_on(t.accent_fill)) } else { (t.text, t.text_dim) };
                let text_y = baseline(p.g, row, font);
                let side = Rect::new(row.right() - ARROW_W * s, row.y, ARROW_W * s, row.h);
                p.g.text(r.x + LABEL_X * s, text_y, node.label(), font, ink, side.x - r.x - LABEL_X * s);
                match node {
                    Node::Command { keys, checked, mark, .. } => {
                        if !keys.is_empty() {
                            let w = p.g.measure(keys, font);
                            p.g.text(side.x - w, text_y, keys, font, dim, f32::INFINITY);
                        }
                        let bx = Rect::new(side.x + (side.w - MARK * s) / 2.0, row.y + (row.h - MARK * s) / 2.0, MARK * s, MARK * s).snap();
                        match (mark, checked) {
                            (Mark::Radio, on) => p.radio(bx, *on, None),
                            (Mark::Check, on) | (Mark::None, on @ true) => p.checkbox(bx, *on),
                            (Mark::None, false) => {}
                        }
                    }
                    Node::Submenu { .. } => p.centered(side, G_SUBMENU, icons, ink),
                    Node::Separator => {}
                }
            }
        }
    }

    fn rows(&self, p: &Panel) -> impl Iterator<Item = (usize, Rect)> + '_ {
        let s = self.scale;
        let (x, w) = (p.rect.x + PAD * s, p.rect.w - 2.0 * PAD * s);
        let mut y = p.rect.y + PAD * s;
        self.lists[p.list].nodes.iter().enumerate().map(move |(i, n)| {
            let h = n.height(s);
            y += h;
            (i, Rect::new(x, y - h, w, h))
        })
    }

    fn hit(&self, x: f32, y: f32) -> Option<(usize, Option<usize>)> {
        let k = self.panels.iter().rposition(|p| p.rect.contains(x, y))?;
        let row = self.rows(&self.panels[k]).find(|(_, r)| r.contains(x, y)).map(|(i, _)| i);
        Some((k, row.filter(|&i| !matches!(self.lists[self.panels[k].list].nodes[i], Node::Separator))))
    }

    fn focus(&mut self, k: usize, row: Option<usize>) {
        let p = self.panels[k];
        let opened = self.panels.get(k + 1).map(|c| c.list);
        let sub = row.and_then(|i| match self.lists[p.list].nodes[i] {
            Node::Submenu { list, .. } => Some(list),
            _ => None,
        });
        self.panels[k].hot = row;
        if sub.is_some() && sub == opened {
            self.panels.truncate(k + 2);
            self.panels[k + 1].hot = None;
            return;
        }
        self.panels.truncate(k + 1);
        if let (Some(list), Some(i)) = (sub, row) {
            let anchor = self.rows(&p).nth(i).map_or(p.rect, |(_, r)| r);
            let (w, h) = (self.lists[list].width, self.lists[list].height);
            let s = self.scale;
            let top = if anchor.y - PAD * s + h <= self.bounds.bottom() { anchor.y - PAD * s } else { self.bounds.bottom() - h };
            let rect = self.fit(w, h, p.rect.right() - PAD * s, p.rect.x + PAD * s, top);
            self.panels.push(Panel { list, rect, hot: None });
        }
    }

    fn fit(&self, w: f32, h: f32, x: f32, flip_x: f32, y: f32) -> Rect {
        let b = self.bounds;
        let x = if x + w <= b.right() { x } else { flip_x - w };
        Rect::new(x.min(b.right() - w).max(b.x), y.min(b.bottom() - h).max(b.y), w, h)
    }

    fn selectable(&self, list: usize) -> Vec<usize> {
        self.lists[list].nodes.iter().enumerate().filter(|(_, n)| !matches!(n, Node::Separator)).map(|(i, _)| i).collect()
    }

    fn step(&mut self, key: Key) {
        let top = self.panels.len() - 1;
        let rows = self.selectable(self.panels[top].list);
        if rows.is_empty() {
            return;
        }
        let at = self.panels[top].hot.and_then(|h| rows.iter().position(|&r| r == h));
        let n = rows.len();
        let next = match (key, at) {
            (Key::Home, _) | (Key::Down, None) => 0,
            (Key::Down, Some(i)) => (i + 1) % n,
            (Key::Up, Some(i)) => (i + n - 1) % n,
            (Key::End | Key::Up, _) => n - 1,
            _ => return,
        };
        self.panels[top].hot = Some(rows[next]);
    }

    fn jump(&mut self, c: char) {
        let top = self.panels.len() - 1;
        let p = self.panels[top];
        let rows = self.selectable(p.list);
        let from = p.hot.and_then(|h| rows.iter().position(|&r| r == h)).map_or(0, |i| i + 1);
        let starts = |i: &usize| self.lists[p.list].nodes[*i].label().chars().next().is_some_and(|f| f.to_lowercase().eq(c.to_lowercase()));
        if let Some(&row) = rows.iter().cycle().skip(from).take(rows.len()).find(|i| starts(i)) {
            self.panels[top].hot = Some(row);
        }
    }
}

fn font(scale: f32) -> FontStyle {
    FontStyle::new(FontFamily::Ui, FONT_PX * scale, false)
}

#[cfg(test)]
#[path = "../../test/ui/popup.rs"]
mod tests;
