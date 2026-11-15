use super::{App, Effect, IconKind, IconRequest};
use crate::hash;
use crate::listing::attr;
use crate::render::Rect;
use crate::text::{Gfx, TextBackend};
use std::cell::RefCell;

pub(super) const CONTENT_PER_FRAME: usize = 8;

#[derive(Default)]
pub(super) struct FileIcons {
    visible: RefCell<Vec<u64>>,
    requested: usize,
}

impl FileIcons {
    pub(super) fn begin(&mut self) {
        self.visible.get_mut().clear();
        self.requested = 0;
    }

    pub(super) fn see(&self, key: u64, kind: IconKind) {
        if kind == IconKind::Content {
            let mut visible = self.visible.borrow_mut();
            if !visible.contains(&key) {
                visible.push(key);
            }
        }
    }
}

pub(super) fn folder_key(size: u32, epoch: u64) -> (u64, IconKind) {
    (hash::hash(&("dir", size, epoch)), IconKind::Folder)
}

pub(super) fn emblems(attrs: u32, icon: Rect, epoch: u64) -> Vec<(u64, IconKind, Rect)> {
    if !cfg!(target_os = "linux") || attrs & (attr::LINK | attr::UNREADABLE) == 0 {
        return Vec::new();
    }
    let side = (icon.w.min(icon.h) * 0.4).round().max(1.0);
    let mut out = Vec::with_capacity(2);
    for (flag, kind) in [(attr::UNREADABLE, IconKind::UnreadableEmblem), (attr::LINK, IconKind::LinkEmblem)] {
        if attrs & flag != 0 {
            let r = Rect::new(icon.right() - side * (out.len() + 1) as f32, icon.bottom() - side, side, side);
            out.push((hash::hash(&("emblem", kind as u8, side as u32, epoch)), kind, r));
        }
    }
    out
}

impl App {
    pub fn visible_content_icons(&self) -> Vec<u64> {
        self.file_icons.visible.borrow().clone()
    }

    pub(super) fn draw_file_emblems<B: TextBackend>(&self, g: &mut Gfx<B>, attrs: u32, icon: Rect, alpha: u8, requests: &mut Vec<IconRequest>) {
        for (key, kind, r) in emblems(attrs, icon, self.icon_epoch) {
            if !g.image_alpha(key, r, alpha) && !self.icons.contains_key(&key) && !requests.iter().any(|q| q.key == key) {
                requests.push(IconRequest { key, kind, path: crate::path::FsPath::home_page(), size: r.w as u32, gen: 0, row: 0 });
            }
        }
    }

    pub(super) fn folder_stand_in<B: TextBackend>(&self, g: &mut Gfx<B>, kind: IconKind, size: u32, r: Rect, alpha: u8, requests: &mut Vec<IconRequest>) -> bool {
        if !cfg!(target_os = "linux") || kind != IconKind::CustomFolder {
            return false;
        }
        let (key, kind) = folder_key(size, self.icon_epoch);
        if g.image_alpha(key, r, alpha) {
            return true;
        }
        if !self.icons.contains_key(&key) && !requests.iter().any(|q| q.key == key) {
            requests.push(IconRequest { key, kind, path: crate::path::FsPath::home_page(), size, gen: 0, row: 0 });
        }
        false
    }

    pub(super) fn request_file_icons(&mut self, requests: Vec<IconRequest>) {
        for r in requests {
            if r.kind == IconKind::Content {
                if self.file_icons.requested >= CONTENT_PER_FRAME {
                    self.motion = true;
                    continue;
                }
                self.file_icons.requested += 1;
            }
            self.icons.insert(r.key, super::IconState::Pending);
            self.effects.push(Effect::Icon(r));
        }
    }
}
