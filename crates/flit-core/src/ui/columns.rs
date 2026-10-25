use super::{icon_glyph, icon_key, App, Effect, Hit, IconRequest, Route};
use crate::listing::attr;
use crate::location::Location;
use crate::open::OpenTarget;
use crate::provider::{GenToken, ProviderRequest, SnapshotSlot};
use crate::render::Rect;
use crate::text::{Gfx, TextBackend};
use crate::theme::Theme;
use crate::view::DirView;
use std::sync::Arc;
use std::time::Instant;

pub(super) const MAX_PANES: usize = 4;
pub(super) const COLUMN_W: [f32; 5] = [200.0, 240.0, 280.0, 320.0, 380.0];

pub(super) struct Pane {
    pub loc: Location,
    gen: u64,
    slot: Arc<SnapshotSlot>,
    cancel: GenToken,
    pub view: Option<DirView>,
    scroll: Option<f32>,
}

impl Pane {
    pub(super) fn shown(loc: Location, view: DirView) -> Self {
        Self { loc, gen: view.snap().gen, slot: Arc::default(), cancel: GenToken::default(), view: Some(view), scroll: None }
    }
}

pub(super) fn pane_depth(loc: &Location) -> usize {
    std::iter::successors(loc.parent(), Location::parent).take(MAX_PANES).count()
}

impl App {
    pub(super) fn sync_panes(&mut self) {
        let t = self.tab();
        let mut want: Vec<Location> = std::iter::successors(t.view_loc.parent(), Location::parent).take(self.layout.panes.len()).collect();
        want.reverse();
        if t.panes.iter().map(|p| &p.loc).eq(want.iter()) {
            return self.settle_panes();
        }
        let mut old = std::mem::take(&mut self.tab_mut().panes);
        let panes: Vec<Pane> = want
            .into_iter()
            .map(|loc| match old.iter().position(|p| p.loc == loc) {
                Some(i) => old.swap_remove(i),
                None => self.list_pane(loc),
            })
            .collect();
        old.iter().for_each(|p| p.cancel.set(u64::MAX));
        self.tab_mut().panes = panes;
        self.settle_panes();
    }

    fn list_pane(&mut self, loc: Location) -> Pane {
        let gen = self.gen();
        let pane = Pane { loc, gen, slot: Arc::default(), cancel: GenToken::default(), view: None, scroll: None };
        pane.cancel.set(gen);
        let req = ProviderRequest::List { tab: 0, gen, path: pane.loc.path().clone(), view: self.tab().view_spec, case: self.case, out: pane.slot.clone(), cancel: pane.cancel.clone() };
        self.effects.push(Effect::Provider { provider: pane.loc.provider(), req });
        pane
    }

    fn settle_panes(&mut self) {
        let App { tabs, cur, layout, .. } = self;
        let t = &mut tabs[*cur];
        let rh = layout.row_h.max(1.0);
        let children: Vec<Vec<u8>> = (0..t.panes.len()).map(|i| t.panes.get(i + 1).map_or(&t.view_loc, |p| &p.loc).file_name().to_vec()).collect();
        for ((p, r), child) in t.panes.iter_mut().zip(&layout.panes).zip(children) {
            let (Some(v), None) = (&mut p.view, p.scroll) else { continue };
            let Some(row) = v.find(&child).and_then(|i| v.row_of(i)) else { continue };
            let max = (v.rows().len() as f32 * rh - r.h).max(0.0);
            p.scroll = Some((row as f32 * rh - (r.h - rh) / 2.0).clamp(0.0, max));
        }
    }

    pub(super) fn poll_panes(&mut self) -> bool {
        let mut shown = false;
        for (k, t) in self.tabs.iter_mut().enumerate() {
            for p in &mut t.panes {
                let Some(snap) = p.slot.take().filter(|s| s.gen == p.gen) else { continue };
                match &mut p.view {
                    Some(v) if v.snap().gen == snap.gen => v.replace(snap),
                    _ => p.view = Some(DirView::new(snap)),
                }
                shown |= k == self.cur;
            }
        }
        shown
    }

    pub(super) fn keep_as_pane(&mut self, k: usize) {
        let t = &mut self.tabs[k];
        if let Some(v) = t.view.take() {
            t.panes.push(Pane::shown(t.view_loc.clone(), v));
        }
    }

    pub(super) fn pane_hit(&self, i: usize, y: f32) -> Hit {
        let (r, rh) = (self.layout.panes[i], self.layout.row_h.max(1.0));
        let Some(p) = self.tab().panes.get(i) else { return Hit::Pane(i, None) };
        let row = ((y - r.y + p.scroll.unwrap_or(0.0)) / rh).floor() as usize;
        Hit::Pane(i, p.view.as_ref().filter(|v| row < v.rows().len()).map(|_| row))
    }

    pub(super) fn open_pane_row(&mut self, i: usize, row: usize, now: Instant) {
        let Some((loc, name, dir)) = self.tab().panes.get(i).and_then(|p| {
            let v = p.view.as_ref()?;
            let idx = *v.rows().get(row)?;
            Some((p.loc.clone(), v.listing().name(idx).to_vec(), v.listing().entries[idx as usize].is_dir()))
        }) else {
            return;
        };
        let route = if dir { Route { folder: loc.join(&name), select: None } } else { Route { folder: loc, select: Some(name) } };
        self.dispatch(vec![route], OpenTarget::CurrentTab, now);
    }

    pub(super) fn pane_wheel(&mut self, dy: f32) -> bool {
        let Hit::Pane(i, _) = self.hover else { return false };
        let (h, rh) = (self.layout.panes.get(i).map_or(0.0, |r| r.h), self.layout.row_h);
        if let Some(p) = self.tab_mut().panes.get_mut(i) {
            let max = (p.view.as_ref().map_or(0, |v| v.rows().len()) as f32 * rh - h).max(0.0);
            p.scroll = Some((p.scroll.unwrap_or(0.0) - dy * 3.0 * rh).clamp(0.0, max));
        }
        self.dirty = true;
        true
    }

    pub(super) fn draw_panes<B: TextBackend>(&mut self, g: &mut Gfx<B>, th: &Theme) {
        let l = &self.layout;
        if l.panes.is_empty() {
            return;
        }
        let (rh, icon_px) = (l.row_h.max(1.0), l.icon);
        let font = self.ui_font(13.0);
        let m = g.metrics(font);
        let (s4, s8, s10) = (self.s(4.0), self.s(8.0), self.s(10.0));
        let t = &self.tabs[self.cur];
        let mut requests: Vec<IconRequest> = Vec::new();
        for (i, (&r, p)) in l.panes.iter().zip(&t.panes).enumerate() {
            g.dl.rect(Rect::new(r.right() - 1.0, r.y, 1.0, r.h), th.separator);
            let Some(v) = &p.view else { continue };
            let child = t.panes.get(i + 1).map_or(&t.view_loc, |p| &p.loc).file_name();
            let (listing, rows, scroll) = (v.listing(), v.rows(), p.scroll.unwrap_or(0.0));
            let dir = p.loc.to_windows();
            g.dl.set_clip(Some(Rect::new(r.x, r.y, r.w - 1.0, r.h)));
            let first = (scroll / rh).floor() as usize;
            let last = (((scroll + r.h) / rh).ceil() as usize).min(rows.len());
            for (row, &idx) in rows.iter().enumerate().take(last).skip(first) {
                let cell = Rect::new(r.x, (r.y + row as f32 * rh - scroll).round(), r.w - 1.0, rh);
                let name = listing.name(idx);
                let e = listing.entries[idx as usize];
                if name == child {
                    g.dl.round_rect(cell.inset(s4, 1.0), th.selection_inactive, s4);
                } else if self.hover == Hit::Pane(i, Some(row)) {
                    g.dl.round_rect(cell.inset(s4, 1.0), th.hover, s4);
                }
                let ir = Rect::new(cell.x + s10, cell.y + ((rh - icon_px) / 2.0).round(), icon_px, icon_px);
                let (key, kind) = icon_key(name, listing.ext(idx), &e, icon_px as u32, dir.as_ref().map_or(&b""[..], |d| d.as_bytes()), self.icon_epoch);
                if !g.image(key, ir) {
                    icon_glyph(g, th, &e, ir);
                    if let Some(path) = (!self.icons.contains_key(&key) && !requests.iter().any(|q| q.key == key)).then(|| p.loc.join(name).to_windows()).flatten() {
                        requests.push(IconRequest { key, kind, path, size: icon_px as u32, gen: 0, row: 0 });
                    }
                }
                let x = ir.right() + s8;
                let baseline = (cell.y + (rh + m.ascent - m.descent) / 2.0).round();
                let color = if e.attrs & attr::HIDDEN != 0 { th.text_dim } else { th.text };
                g.text(x, baseline, &self.shown_name(listing, idx), font, color, cell.right() - s10 - x);
            }
            g.dl.set_clip(None);
        }
        self.request_icons(requests);
    }
}
