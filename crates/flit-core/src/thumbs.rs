use crate::atlas::Slot;
use crate::hash::FxHashMap;
use crate::render::{DrawList, Rect};
use crate::text::Uploads;

pub const THUMB_ATLAS: u32 = 4096;
const PAGE: u32 = 512;
const PAGES_PER_ROW: u32 = THUMB_ATLAS / PAGE;
const CLASSES: [u32; 4] = [64, 128, 256, 512];

pub fn class_px(px: u32) -> u32 {
    CLASSES.iter().copied().find(|&c| c >= px).unwrap_or(PAGE)
}

#[derive(Clone, Copy, Default)]
struct Cell {
    key: u64,
    used: u64,
    w: u32,
    h: u32,
}

#[derive(Default)]
struct Page {
    class: Option<usize>,
    cells: Vec<Cell>,
}

pub struct ThumbAtlas {
    pages: Vec<Page>,
    map: FxHashMap<u64, (usize, usize)>,
    frame: u64,
    pub uploads: Uploads,
}

impl Default for ThumbAtlas {
    fn default() -> Self {
        Self { pages: (0..PAGES_PER_ROW * PAGES_PER_ROW).map(|_| Page::default()).collect(), map: FxHashMap::default(), frame: 1, uploads: Uploads::default() }
    }
}

fn cells(class: usize) -> usize {
    ((PAGE / CLASSES[class]) * (PAGE / CLASSES[class])) as usize
}

impl ThumbAtlas {
    pub fn begin_frame(&mut self) {
        self.frame += 1;
    }

    pub fn clear(&mut self) {
        self.pages.iter_mut().for_each(|p| *p = Page::default());
        self.map.clear();
        self.uploads.clear();
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn contains(&self, key: u64) -> bool {
        self.map.contains_key(&key)
    }

    fn slot(&self, p: usize, c: usize) -> Slot {
        let side = CLASSES[self.pages[p].class.unwrap_or(0)];
        let per_row = PAGE / side;
        let (px, py) = (p as u32 % PAGES_PER_ROW * PAGE, p as u32 / PAGES_PER_ROW * PAGE);
        let cell = self.pages[p].cells[c];
        Slot { x: px + c as u32 % per_row * side, y: py + c as u32 / per_row * side, w: cell.w, h: cell.h }
    }

    fn alloc(&mut self, class: usize) -> Option<(usize, usize)> {
        let frame = self.frame;
        let ours: Vec<usize> = (0..self.pages.len()).filter(|&p| self.pages[p].class == Some(class)).collect();
        if let Some(free) = ours.iter().find_map(|&p| self.pages[p].cells.iter().position(|c| c.key == 0).map(|c| (p, c))) {
            return Some(free);
        }
        if let Some(p) = self.pages.iter().position(|pg| pg.class.is_none()) {
            return Some(self.reset_page(p, class));
        }
        let lru = ours.iter().flat_map(|&p| self.pages[p].cells.iter().enumerate().map(move |(c, cell)| (cell.used, p, c))).filter(|e| e.0 < frame).min();
        if let Some((_, p, c)) = lru {
            self.map.remove(&self.pages[p].cells[c].key);
            return Some((p, c));
        }
        let stale = self.pages.iter().enumerate().filter(|(_, pg)| pg.class != Some(class)).map(|(p, pg)| (pg.cells.iter().map(|c| c.used).max().unwrap_or(0), p)).filter(|e| e.0 < frame).min()?;
        Some(self.reset_page(stale.1, class))
    }

    fn reset_page(&mut self, p: usize, class: usize) -> (usize, usize) {
        for c in &self.pages[p].cells {
            self.map.remove(&c.key);
        }
        self.pages[p] = Page { class: Some(class), cells: vec![Cell::default(); cells(class)] };
        (p, 0)
    }

    pub fn insert(&mut self, key: u64, w: u32, h: u32, rgba: &[u8]) -> bool {
        if w == 0 || h == 0 || w.max(h) > PAGE || rgba.len() != (w * h * 4) as usize {
            return false;
        }
        if let Some((p, c)) = self.map.remove(&key) {
            self.pages[p].cells[c] = Cell::default();
        }
        let class = CLASSES.iter().position(|&s| s >= w.max(h)).unwrap_or(CLASSES.len() - 1);
        let Some((p, c)) = self.alloc(class) else { return false };
        self.pages[p].cells[c] = Cell { key, used: self.frame, w, h };
        self.map.insert(key, (p, c));
        let slot = self.slot(p, c);
        self.uploads.rects.push((slot, self.uploads.data.len()));
        self.uploads.data.extend_from_slice(rgba);
        true
    }

    pub fn draw(&mut self, dl: &mut DrawList, key: u64, r: Rect, alpha: u8) -> bool {
        let Some(&(p, c)) = self.map.get(&key) else { return false };
        self.pages[p].cells[c].used = self.frame;
        let s = self.slot(p, c);
        let k = (r.w / s.w as f32).min(r.h / s.h as f32).min(1.0);
        let (w, h) = ((s.w as f32 * k).round(), (s.h as f32 * k).round());
        let n = THUMB_ATLAS as f32;
        let uv = [s.x as f32 / n, s.y as f32 / n, (s.x + s.w) as f32 / n, (s.y + s.h) as f32 / n];
        dl.thumb(Rect::new((r.x + (r.w - w) / 2.0).round(), (r.y + (r.h - h) / 2.0).round(), w, h), uv, alpha);
        true
    }
}

#[cfg(test)]
#[path = "../test/thumbs.rs"]
mod tests;
