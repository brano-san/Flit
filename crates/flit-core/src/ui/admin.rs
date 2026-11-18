use super::{App, OpenTarget, Route};
use crate::i18n::S;
use crate::location::{Location, ProviderId};
use crate::path::{FsPath, PathKind};
use crate::provider::{FileOp, TabId};
use crate::render::{Color, Rect};
use crate::text::{FontStyle, Gfx, TextBackend};
use crate::theme::{self, Theme};
use std::time::Instant;

pub(super) const G_ADMIN: &str = "\u{E7BA}";
pub(super) const MAX_ADMIN_TABS: usize = 8;

pub(super) fn admin_location(loc: &Location) -> Option<Location> {
    match loc {
        Location::Local(p) | Location::Admin(p) if p.kind() == PathKind::Unix && crate::archive::split(p).is_none() => Some(Location::Admin(p.clone())),
        _ => None,
    }
}

pub(super) fn stays_admin(from: &Location, to: Location) -> Location {
    match (from, to) {
        (Location::Admin(_), Location::Local(p)) => Location::as_admin(p),
        (_, to) => to,
    }
}

fn dragged_items(op: &FileOp) -> Option<&Vec<FsPath>> {
    match op {
        FileOp::Copy { items, .. } | FileOp::Move { items, .. } | FileOp::Link { items, .. } | FileOp::Trash(items) => Some(items),
        _ => None,
    }
}

impl App {
    pub(super) fn open_admin(&mut self, now: Instant) {
        let t = self.tab();
        let folders: Vec<Location> = match &t.view {
            Some(v) => self.selected_items().into_iter().filter(|&i| v.listing().entries[i as usize].is_dir()).map(|i| t.entry_location(i)).collect(),
            None => Vec::new(),
        };
        let picked = if folders.is_empty() { vec![t.location().clone()] } else { folders };
        let routes: Vec<Route> = picked.iter().filter_map(admin_location).take(MAX_ADMIN_TABS).map(|folder| Route { folder, select: None }).collect();
        if routes.is_empty() {
            return self.status(S::ErrorsEngineAdminUnsupported.t());
        }
        self.dispatch(routes, OpenTarget::NewTab, now);
    }

    pub(super) fn admin_route(&self, provider: ProviderId, tab: TabId, op: &FileOp) -> ProviderId {
        if provider != ProviderId::Local {
            return provider;
        }
        let in_admin_tab = self.tabs.iter().any(|t| t.id == tab && t.location().is_admin());
        let from_admin_drag = dragged_items(op).is_some_and(|items| self.ops.admin_drag.as_ref() == Some(items));
        if in_admin_tab || from_admin_drag {
            ProviderId::Admin
        } else {
            provider
        }
    }

    pub(super) fn note_drag_source(&mut self, paths: &[FsPath]) {
        self.ops.admin_drag = self.tab().location().is_admin().then(|| paths.to_vec());
    }

    pub(super) fn admin_denied(&mut self, tab: TabId, error: &str, now: Instant) {
        let message = crate::i18n::localize(error).into_owned();
        if let Some(k) = self.tabs.iter().position(|t| t.id == tab && t.location().is_admin()) {
            if self.tabs.len() > 1 {
                let mut t = self.tabs.remove(k);
                t.shutdown(&mut self.effects);
                if self.cur > k || self.cur >= self.tabs.len() {
                    self.cur = self.cur.saturating_sub(1);
                }
                self.after_tab_change();
            } else {
                let here = Location::from_local(self.tabs[k].location().path().clone());
                self.tabs[k].shutdown(&mut self.effects);
                self.tabs[k].history = crate::nav::History::new(here);
                self.load_tab(k, None, now);
                self.after_tab_change();
            }
        }
        self.status(message);
    }
}

pub(super) fn draw_tab_mark<B: TextBackend>(g: &mut Gfx<B>, t: &Theme, tab: Rect, icon: Rect, font: FontStyle, s2: f32, ink: Option<Color>) {
    super::centered(g, icon, G_ADMIN, font, ink.unwrap_or(t.destructive));
    g.dl.round_rect(Rect::new(tab.x + 3.0 * s2, tab.y + s2, (tab.w - 6.0 * s2).max(0.0), s2.max(1.0)), t.destructive, s2);
}

pub(super) fn draw_address_badge<B: TextBackend>(g: &mut Gfx<B>, t: &Theme, r: Rect, font: FontStyle, baseline: f32, s3: f32) -> Rect {
    let label = S::AddressAdminBadge.t();
    let pad = 2.0 * s3;
    g.dl.round_rect(r, t.destructive.alpha(0x20), s3);
    let badge = Rect::new(r.x + s3, r.y + s3, (g.measure(label, font) + 2.0 * pad).min(r.w / 2.0), (r.h - 2.0 * s3).max(0.0));
    g.dl.round_rect(badge, t.destructive, s3);
    g.text(badge.x + pad, baseline, label, font, theme::ink_on(t.destructive), badge.w - 2.0 * pad);
    let x = badge.right() + s3;
    Rect::new(x, r.y, (r.right() - x).max(0.0), r.h)
}

#[cfg(test)]
#[path = "../../test/ui/admin.rs"]
mod tests;
