use super::{App, Bitmap, Effect, IconKind};
use crate::hash::{self, FxHashMap, FxHashSet};
use crate::listing::{attr, Entry};
use crate::path::FsPath;
use crate::render::Rect;
use crate::text::{Gfx, TextBackend};
use std::cell::RefCell;

pub const OVERLAY_BATCH: usize = 64;
pub const OVERLAY_SLOTS: std::ops::RangeInclusive<u8> = 1..=15;
const MAX_KNOWN: usize = 50_000;
const SKIPPED: u32 = attr::VOLUME | attr::STREAM | attr::DELETED | attr::ARCHIVED | attr::NOT_LOCAL;

#[derive(Clone, Debug, PartialEq)]
pub struct OverlayItem {
    pub key: u64,
    pub path: FsPath,
}

#[derive(Clone, Debug, PartialEq)]
pub struct OverlayBatch {
    pub id: u64,
    pub size: u32,
    pub items: Vec<OverlayItem>,
    pub fetch: Vec<u8>,
    pub have: Vec<u8>,
}

#[derive(Clone, Debug, Default)]
pub struct OverlayReply {
    pub id: u64,
    pub size: u32,
    pub found: Vec<(u64, Option<u8>)>,
    pub skipped: Vec<u64>,
    pub images: Vec<(u8, Bitmap)>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum State {
    Pending(u64),
    Found(Option<u8>),
}

#[derive(Clone, Copy, Debug)]
struct Known {
    dir: u64,
    modified: i64,
    state: State,
}

struct Wanted {
    size: u32,
    dir: u64,
    modified: i64,
    item: OverlayItem,
}

#[derive(Default)]
pub(super) struct ShellOverlays {
    enabled: bool,
    epoch: u64,
    next: u64,
    known: FxHashMap<u64, Known>,
    asked: FxHashSet<(u8, u32)>,
    visible: RefCell<Vec<u64>>,
    wanted: RefCell<Vec<Wanted>>,
    images: RefCell<Vec<(u8, u32)>>,
}

pub(super) fn item_key(dir: &[u8], name: &[u8]) -> u64 {
    hash::hash(&("shell-overlay", dir, name))
}

fn image_key(slot: u8, size: u32, epoch: u64) -> u64 {
    hash::hash(&("shell-overlay-image", slot, size, epoch))
}

fn local_drive(dir: &[u8]) -> bool {
    dir.len() >= 2 && dir[0].is_ascii_alphabetic() && dir[1] == b':'
}

impl ShellOverlays {
    pub(super) fn begin(&mut self, epoch: u64) {
        if epoch != self.epoch {
            self.epoch = epoch;
            self.known.clear();
            self.asked.clear();
        }
        self.visible.get_mut().clear();
        self.wanted.get_mut().clear();
        self.images.get_mut().clear();
    }

    fn see(&self, key: u64) {
        let mut visible = self.visible.borrow_mut();
        if !visible.contains(&key) {
            visible.push(key);
        }
    }

    fn want(&self, w: Wanted) {
        self.see(w.item.key);
        let mut wanted = self.wanted.borrow_mut();
        if !wanted.iter().any(|o| o.item.key == w.item.key) {
            wanted.push(w);
        }
    }

    fn want_image(&self, slot: u8, size: u32) {
        let mut images = self.images.borrow_mut();
        if !self.asked.contains(&(slot, size)) && !images.contains(&(slot, size)) {
            images.push((slot, size));
        }
    }

    pub(super) fn forget(&mut self, dir: &[u8], name: &[u8]) {
        self.known.remove(&item_key(dir, name));
    }

    pub(super) fn forget_dir(&mut self, dir: &[u8]) {
        let dir = hash::hash(&dir);
        self.known.retain(|_, k| k.dir != dir);
    }
}

impl App {
    pub fn set_shell_overlays(&mut self, on: bool) {
        self.shell_overlays.enabled = on;
    }

    pub fn visible_overlays(&self) -> Vec<u64> {
        self.shell_overlays.visible.borrow().clone()
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw_shell_overlay<B: TextBackend>(&self, g: &mut Gfx<B>, kind: IconKind, e: &Entry, dir: &[u8], name: &[u8], r: Rect, alpha: u8) {
        let o = &self.shell_overlays;
        if !o.enabled || !matches!(kind, IconKind::Type | IconKind::Folder) || e.attrs & SKIPPED != 0 || !local_drive(dir) {
            return;
        }
        let key = item_key(dir, name);
        let size = r.w.round().max(1.0) as u32;
        match o.known.get(&key).filter(|k| k.modified == e.modified).map(|k| k.state) {
            Some(State::Found(Some(slot))) => {
                if !g.image_alpha(image_key(slot, size, o.epoch), r, alpha) {
                    o.want_image(slot, size);
                }
            }
            Some(State::Found(None)) => {}
            Some(State::Pending(_)) => o.see(key),
            None => {
                if let Some(path) = FsPath::parse_exact(dir).map(|d| d.join(name)) {
                    o.want(Wanted { size, dir: hash::hash(&dir), modified: e.modified, item: OverlayItem { key, path } });
                }
            }
        }
    }

    pub(super) fn flush_shell_overlays(&mut self) {
        let o = &mut self.shell_overlays;
        let mut wanted = std::mem::take(o.wanted.get_mut());
        let images = std::mem::take(o.images.get_mut());
        if wanted.is_empty() && images.is_empty() {
            return;
        }
        if o.known.len() + wanted.len() > MAX_KNOWN {
            o.known.retain(|_, k| matches!(k.state, State::Pending(_)));
        }
        wanted.sort_by_key(|w| w.size);
        let mut sizes: Vec<u32> = wanted.iter().map(|w| w.size).chain(images.iter().map(|i| i.1)).collect();
        sizes.sort_unstable();
        sizes.dedup();
        let mut wanted = wanted.into_iter().peekable();
        for size in sizes {
            let mut mine = Vec::new();
            while let Some(w) = wanted.next_if(|w| w.size == size) {
                mine.push(w);
            }
            let fetch: Vec<u8> = images.iter().filter(|i| i.1 == size).map(|i| i.0).collect();
            let have: Vec<u8> = OVERLAY_SLOTS.filter(|&s| o.asked.contains(&(s, size))).collect();
            o.asked.extend(fetch.iter().map(|&s| (s, size)));
            let mut fetch = Some(fetch);
            while fetch.is_some() || !mine.is_empty() {
                o.next += 1;
                let id = o.next;
                let items = mine
                    .drain(..mine.len().min(OVERLAY_BATCH))
                    .map(|w| {
                        o.known.insert(w.item.key, Known { dir: w.dir, modified: w.modified, state: State::Pending(id) });
                        w.item
                    })
                    .collect();
                self.effects.push(Effect::Overlays(OverlayBatch { id, size, items, fetch: fetch.take().unwrap_or_default(), have: have.clone() }));
            }
        }
    }

    pub fn on_overlays<B: TextBackend>(&mut self, gfx: &mut Gfx<B>, reply: OverlayReply) {
        let o = &mut self.shell_overlays;
        for (slot, bm) in reply.images {
            if OVERLAY_SLOTS.contains(&slot) && bm.rgba.len() == (bm.width * bm.height * 4) as usize {
                o.asked.insert((slot, reply.size));
                gfx.add_image(image_key(slot, reply.size, o.epoch), bm.width, bm.height, bm.rgba);
            }
        }
        for (key, slot) in reply.found {
            if let Some(k) = o.known.get_mut(&key).filter(|k| k.state == State::Pending(reply.id)) {
                k.state = State::Found(slot.filter(|s| OVERLAY_SLOTS.contains(s)));
            }
        }
        for key in reply.skipped {
            if o.known.get(&key).is_some_and(|k| k.state == State::Pending(reply.id)) {
                o.known.remove(&key);
            }
        }
        self.dirty = true;
    }
}
