use super::actions::{index, ACTIONS};
use super::address::{ready, Probe};
use super::ops::TIMER_SLACK;
use super::properties::MEASURE_IDS;
use super::sidebar::Section;
use super::widgets::{Paint, RADIUS};
use super::{App, Drag, Effect, Hit};
use crate::fmt as format;
use crate::i18n::S;
use crate::location::{Location, ProviderId};
use crate::path::PathKind;
use crate::preview::{self, PreviewRequest, PropGroup};
use crate::provider::{GenToken, OpCtl, OpId, ProviderRequest, Totals};
use crate::render::Rect;
use crate::text::{Gfx, TextBackend};
use crate::theme::Theme;
use std::time::{Duration, Instant};

pub(super) const QUICK_DELAY: Duration = Duration::from_millis(1000);
const GAP: f32 = 8.0;
const PAD_X: f32 = 12.0;
const PAD_Y: f32 = 8.0;
const ROW_GAP: f32 = 4.0;
const COL_GAP: f32 = 16.0;
const MAX_W: f32 = 520.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Subject {
    Folder,
    File,
    Volume,
}

pub(super) struct QuickTip {
    pub item: usize,
    pub loc: Location,
    pub subject: Subject,
    pub at: Instant,
    pub shown: Option<Instant>,
    probe: Option<Probe>,
    measure: Option<(OpId, OpCtl)>,
    props: u64,
    pub created: Option<i64>,
    pub size: Option<u64>,
    pub kind: Option<String>,
    pub totals: Option<Totals>,
    pub system: Vec<(String, String)>,
}

impl Drop for QuickTip {
    fn drop(&mut self) {
        if let Some((_, ctl)) = &self.measure {
            ctl.cancel();
        }
        if let Some(p) = &self.probe {
            p.cancel.set(u64::MAX);
        }
    }
}

pub(super) fn compact_count(n: u64) -> String {
    match n {
        0..1000 => n.to_string(),
        1000..1_000_000 => format!("{:.1}K", n as f64 / 1e3),
        _ => format!("{:.1}M", n as f64 / 1e6),
    }
}

impl App {
    fn quick_subject(&self, i: usize) -> Option<(Location, Subject)> {
        let sb = &self.nav.sidebar;
        let it = sb.items.get(i).filter(|it| it.section != Section::Tags)?;
        let loc = it.loc.clone();
        if loc.is_home_page() || matches!(loc.path().kind(), PathKind::Computer | PathKind::RecycleBin | PathKind::Home | PathKind::Shell) {
            return None;
        }
        if sb.recent_file(i).is_some() {
            return Some((loc, Subject::File));
        }
        let volume = it.usage.is_some() || loc.is_root();
        Some((loc, if volume { Subject::Volume } else { Subject::Folder }))
    }

    pub(super) fn quick_hover(&mut self, h: Hit, now: Instant) {
        let quiet = self.tooltips && self.drag == Drag::None && self.pressed == Hit::None && self.own_menu.is_none() && self.confirm.is_none() && self.nav.palette.is_none();
        let target = match h {
            Hit::Side(i) if quiet => self.quick_subject(i).map(|(loc, subject)| (i, loc, subject)),
            _ => None,
        };
        match target {
            Some((i, loc, _)) if self.quick.as_ref().is_some_and(|q| q.item == i && q.loc == loc) => {}
            Some((item, loc, subject)) => {
                let at = now + QUICK_DELAY;
                self.quick = Some(QuickTip { item, loc, subject, at, shown: None, probe: None, measure: None, props: 0, created: None, size: None, kind: None, totals: None, system: Vec::new() });
                self.arm_timer(now, at);
            }
            None => self.quick_hide(),
        }
    }

    pub(super) fn quick_hide(&mut self) {
        if let Some(q) = self.quick.take() {
            self.dirty |= q.shown.is_some();
        }
    }

    pub(super) fn quick_timer(&mut self, now: Instant) {
        let Some(q) = self.quick.as_ref().filter(|q| q.shown.is_none()) else { return };
        if now + TIMER_SLACK < q.at {
            let at = q.at;
            return self.arm_timer(now, at);
        }
        if self.hover != Hit::Side(q.item) {
            self.quick = None;
            return;
        }
        let (loc, subject) = (q.loc.clone(), q.subject);
        let probe = loc.parent().filter(|_| subject != Subject::Volume).map(|parent| self.probe(parent));
        let measure = (subject == Subject::Folder && loc.provider() == ProviderId::Local).then(|| {
            let id = MEASURE_IDS + self.archive.take_id();
            let ctl = OpCtl::new(None);
            let tab = self.tab().id;
            self.effects.push(Effect::Provider { provider: ProviderId::Local, req: ProviderRequest::Measure { tab, id, items: vec![(loc.path().clone(), None)], ctl: ctl.clone() } });
            (id, ctl)
        });
        let props = if subject == Subject::File { self.gen() } else { 0 };
        if props != 0 {
            let cancel = GenToken::default();
            cancel.set(props);
            self.effects.push(Effect::Preview(PreviewRequest { gen: props, cancel, path: loc.path().clone(), kind: preview::Kind::Other, size: 0, details: true, px: 0 }));
        }
        if let Some(q) = self.quick.as_mut() {
            (q.shown, q.probe, q.measure, q.props) = (Some(now), probe, measure, props);
        }
        if subject == Subject::Volume {
            self.quick_volume();
        }
        self.dirty = true;
    }

    fn quick_volume(&mut self) {
        let Some(q) = self.quick.as_mut() else { return };
        let drive = self.nav.sidebar.places.drives.iter().find(|d| d.path == *q.loc.path());
        q.size = drive.map(|d| d.total).filter(|&t| t > 0);
    }

    pub(super) fn poll_quick(&mut self) -> bool {
        let Some(q) = self.quick.as_mut() else { return false };
        let Some(mut p) = q.probe.take() else { return false };
        if p.view.is_some() || !ready(&mut p) {
            q.probe = Some(p);
            return false;
        }
        let name = q.loc.file_name().to_vec();
        let mut found = (None, None, None);
        if let Some(v) = p.view.as_ref() {
            let l = v.listing();
            if let Some(i) = (0..l.entries.len() as u32).find(|&i| l.name(i).eq_ignore_ascii_case(&name)) {
                let e = &l.entries[i as usize];
                let arena = bumpalo::Bump::new();
                let file = !e.is_dir();
                found = ((e.created > 0).then_some(e.created), file.then_some(e.size), file.then(|| self.type_text(&arena, l, i).to_owned()));
            }
        }
        if let Some(q) = self.quick.as_mut() {
            (q.created, q.size, q.kind) = (found.0, found.1.or(q.size), found.2);
            q.probe = Some(p);
        }
        self.dirty = true;
        true
    }

    pub(super) fn quick_measured(&mut self, id: OpId, totals: Totals, done: bool) -> bool {
        let Some(q) = self.quick.as_mut().filter(|q| q.measure.as_ref().is_some_and(|m| m.0 == id)) else { return false };
        q.totals = Some(totals);
        if done {
            q.measure = None;
        }
        self.dirty = true;
        true
    }

    pub(super) fn quick_props(&mut self, gen: u64, groups: &[PropGroup]) -> bool {
        let Some(q) = self.quick.as_mut().filter(|q| q.props != 0 && q.props == gen) else { return false };
        q.system = groups.iter().flat_map(|g| g.rows.iter().cloned()).collect();
        self.dirty = true;
        true
    }

    pub(super) fn quick_rows(&self) -> Vec<(String, String)> {
        let Some(q) = self.quick.as_ref() else { return Vec::new() };
        let counting = || S::PropertiesCounting.t().to_owned();
        let bytes = |b: u64| {
            let mut s = String::new();
            format::size_in(&mut s, b, self.style.decimal);
            s
        };
        let date = |t: i64| {
            let mut s = String::new();
            format::datetime_in(&mut s, t, self.utc_offset_min, &self.style);
            s
        };
        let mut rows = Vec::new();
        if q.subject == Subject::File {
            if let Some(k) = &q.kind {
                rows.push((S::SidebarQuickType.t().to_owned(), k.clone()));
            }
            rows.extend(q.system.iter().cloned());
        }
        if let Some(c) = q.created {
            rows.push((S::SidebarQuickCreated.t().to_owned(), date(c)));
        }
        if q.subject == Subject::Folder && q.measure.is_some() || q.totals.is_some() {
            let t = q.totals;
            rows.push((S::SidebarQuickSubfolders.t().to_owned(), t.map_or_else(counting, |t| compact_count(t.folders))));
            rows.push((S::SidebarQuickSubfiles.t().to_owned(), t.map_or_else(counting, |t| compact_count(t.files))));
            rows.push((S::SidebarQuickSize.t().to_owned(), t.map_or_else(counting, |t| bytes(t.size))));
        } else if let Some(s) = q.size {
            rows.push((S::SidebarQuickSize.t().to_owned(), bytes(s)));
        }
        rows
    }

    pub(super) fn draw_quick_tip<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme) {
        let Some(q) = self.quick.as_ref().filter(|q| q.shown.is_some()) else { return };
        let Some(&anchor) = self.nav.sidebar.rects.get(q.item) else { return };
        let (font, small, sc) = (self.ui_font(12.5), self.ui_font(12.0), self.scale);
        let s = |v: f32| (v * sc).round();
        let (w, h) = self.size;
        let name = self.nav.sidebar.items.get(q.item).map(|it| it.label.clone()).unwrap_or_default();
        let path = q.loc.display().into_owned();
        let rows = self.quick_rows();
        let id = index("item.open_tab");
        let command = id.map(|i| (ACTIONS[i].title.t().to_owned(), vec!["Ctrl+MouseLeft".to_owned(), "MouseMiddle".to_owned()]));
        let since = q.shown.unwrap_or(q.at);
        let from = g.dl.instances.len();
        let mut p = Paint::new(g, t, sc);
        let m = p.g.metrics(font);
        let line_h = (m.ascent + m.descent).ceil() + s(ROW_GAP);
        let head_w = p.g.measure(&name, font).max(rows.iter().map(|(k, _)| p.g.measure(k, small)).fold(0.0, f32::max)).max(p.g.measure(S::WidgetsCmd.t(), small));
        let path_w = p.g.measure(&path, small);
        let value_w = rows.iter().map(|(_, v)| p.g.measure(v, small)).fold(path_w, f32::max);
        let cmd_w = command.as_ref().map_or(0.0, |(c, keys)| p.g.measure(c, small) + s(8.0) + p.keycaps_w(keys, small));
        let max_w = s(MAX_W).min(w - 2.0 * s(GAP));
        let bw = (head_w + s(COL_GAP) + value_w.max(cmd_w) + 2.0 * s(PAD_X)).ceil().min(max_w);
        let lines = 1 + rows.len() + usize::from(command.is_some());
        let bh = 2.0 * s(PAD_Y) + lines as f32 * line_h;
        let x = (anchor.right() + s(GAP)).min(w - bw - s(GAP)).max(s(GAP));
        let y = anchor.y.min(h - bh - s(GAP)).max(s(GAP)).round();
        let r = Rect::new(x, y, bw, bh);
        p.g.dl.round_rect(r, t.popup, s(RADIUS));
        p.g.dl.outline(r, t.popup_border, s(RADIUS), 1.0);
        let left = r.x + s(PAD_X);
        let col = left + head_w + s(COL_GAP);
        let room = r.right() - s(PAD_X) - col;
        let mut base = r.y + s(PAD_Y) + m.ascent;
        p.g.text(left, base.round(), &name, font, t.text, head_w + s(COL_GAP));
        p.g.text(col, base.round(), &path, small, t.text_dim, room);
        for (k, v) in &rows {
            base += line_h;
            p.g.text(left, base.round(), k, small, t.text, head_w);
            p.g.text(col, base.round(), v, small, t.text_dim, room);
        }
        if let Some((c, keys)) = &command {
            base += line_h;
            p.g.text(left, base.round(), S::WidgetsCmd.t(), small, t.text_dim.alpha(0xA0), head_w);
            let cw = p.g.text(col, base.round(), c, small, t.text_dim, room);
            p.keycaps(col + cw + s(8.0), base - m.ascent / 2.0 + s(1.0), keys, small);
        }
        self.pop(g, from, since, (r.x, r.y + r.h / 2.0));
    }
}

#[cfg(test)]
#[path = "../../test/ui/quick.rs"]
mod tests;
