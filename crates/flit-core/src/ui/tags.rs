use super::layouts::Parts;
use super::palette::{Palette, Pick};
use super::{App, Effect, Route, MAX_OPEN};
use crate::listing::attr;
use crate::location::{Location, ProviderId};
use crate::open::OpenTarget;
use crate::path::{FsPath, PathKind};
use crate::provider::{FileOp, OpReport, ProviderRequest};
use crate::render::{Color, Rect};
use crate::sort::{self, GroupField};
use crate::tags::{self, Tag, TagEvent, TagRequest, Tags, COLORS, MAX_TAGS};
use crate::text::{FontStyle, Gfx, TextBackend};
use crate::view::{DirView, Group};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub(super) const G_TAG: &str = "\u{E8EC}";
const FIX_WINDOW: Duration = Duration::from_secs(10);
const MAX_MARKS: usize = 6;
const MARK: f32 = 8.0;
const MARK_GAP: f32 = 3.0;
const NOT_TAGGABLE: &str = "Tags work only for items on local NTFS drives";

#[derive(Clone, Debug, PartialEq)]
pub(super) enum TagPick {
    Manage,
    Actions(usize),
    Name { tag: Option<usize>, assign: Vec<FsPath> },
    Color { tag: Option<usize>, name: String, assign: Vec<FsPath> },
    Delete(usize),
    OpenAll,
}

impl TagPick {
    pub(super) fn placeholder(&self) -> &'static str {
        match self {
            TagPick::Manage => "Type to filter tags",
            TagPick::Actions(_) => "Choose what to do with the tag",
            TagPick::Name { tag: None, .. } => "Name of the new tag",
            TagPick::Name { .. } => "New name of the tag",
            TagPick::Color { .. } => "Type to filter colors",
            TagPick::Delete(_) => "Delete the tag?",
            TagPick::OpenAll => "Choose the tag whose items to open",
        }
    }

    pub(super) fn empty(&self) -> &'static str {
        match self {
            TagPick::Name { tag: None, .. } => "Enter creates the tag",
            TagPick::Name { .. } => "Enter renames the tag",
            TagPick::Color { .. } => "No matching colors",
            _ => "No matching tags",
        }
    }

    pub(super) fn is_input(&self) -> bool {
        matches!(self, TagPick::Name { .. })
    }
}

fn items_text(n: usize) -> String {
    format!("{n} {}", if n == 1 { "item" } else { "items" })
}

impl App {
    pub fn tags(&self) -> &Arc<Tags> {
        &self.nav.sidebar.tags
    }

    pub fn tag_defs(&self) -> &[Tag] {
        &self.nav.sidebar.tags.defs
    }

    pub fn on_tags(&mut self, ev: TagEvent, now: Instant) {
        match ev {
            TagEvent::Changed { tags, files, dropped } => {
                if let Some(k) = dropped {
                    self.drop_tag_bit(k);
                }
                self.set_tags(tags);
                self.tag_fixes.extend(files.into_iter().map(|(path, mask)| (path, mask, now + FIX_WINDOW)));
                self.apply_tag_fixes(now);
            }
            TagEvent::Status(message) => self.status(message),
        }
    }

    pub fn selection_tags(&self) -> (u32, u32) {
        let Some(v) = &self.tab().view else { return (0, 0) };
        let mut masks = v.selected().map(|i| v.listing().entries[i as usize].tags).peekable();
        if masks.peek().is_none() {
            return (0, 0);
        }
        masks.fold((0, u32::MAX), |(any, all), m| (any | m, all & m))
    }

    pub fn toggle_tag(&mut self, k: usize) {
        let Some(items) = self.tag_items() else { return };
        let on = self.selection_tags().1 & (1 << k) == 0;
        self.effects.push(Effect::Tags(TagRequest::Set { items, tag: k, on }));
    }

    pub fn remove_tags(&mut self) {
        let Some(items) = self.tag_items() else { return };
        self.effects.push(Effect::Tags(TagRequest::Clear(items)));
    }

    fn tag_items(&mut self) -> Option<Vec<FsPath>> {
        let items = self.selected_items();
        let result = match (items.is_empty(), self.tab().view_loc.path().kind()) {
            (true, _) => Err("Select the items to tag"),
            (false, PathKind::Drive) => self.item_paths(&items),
            _ => Err(NOT_TAGGABLE),
        };
        result.map_err(|e| self.status(e)).ok()
    }

    pub(super) fn new_tag(&mut self, assign_selection: bool) {
        let assign = match assign_selection && !self.selected_items().is_empty() {
            true => match self.tag_items() {
                Some(items) => items,
                None => return,
            },
            false => Vec::new(),
        };
        if self.tag_defs().len() >= MAX_TAGS {
            return self.status("Flit supports at most 32 tags");
        }
        self.open_tag_pick(TagPick::Name { tag: None, assign }, "");
    }

    pub(super) fn open_tag_pick(&mut self, pick: TagPick, text: &str) {
        let t = self.tags().clone();
        let name = |k: usize| t.defs.get(k).map_or("", |d| d.name.as_str());
        let rows = || t.defs.iter().enumerate().map(|(k, d)| (d.name.clone(), format!("{} · {}", tags::color_name(d.color), items_text(t.count(k))))).collect::<Vec<_>>();
        let plain = |titles: &[&str]| titles.iter().map(|s| (s.to_string(), String::new())).collect::<Vec<_>>();
        let items = match &pick {
            TagPick::Manage => [rows(), plain(&["New tag"])].concat(),
            TagPick::OpenAll if t.defs.is_empty() => return self.status("There are no tags yet"),
            TagPick::OpenAll => rows(),
            TagPick::Actions(_) => plain(&["Rename", "Change color", "Delete"]),
            TagPick::Color { tag, .. } => {
                let current = tag.and_then(|k| t.defs.get(k)).map(|d| d.color);
                COLORS.iter().map(|&(n, c)| (n.to_owned(), if current == Some(c) { "Current".to_owned() } else { String::new() })).collect()
            }
            TagPick::Delete(k) => vec![(format!("Delete the tag \"{}\"", name(*k)), format!("Removes it from {}", items_text(t.count(*k)))), ("Cancel".into(), String::new())],
            TagPick::Name { .. } => Vec::new(),
        };
        self.open_pick(Palette::new(Pick::Tags(pick), items, text));
    }

    pub(super) fn tag_choose(&mut self, pick: TagPick, id: usize, now: Instant) {
        let n = self.tag_defs().len();
        match pick {
            TagPick::Manage if id == n => self.new_tag(false),
            TagPick::Manage => self.open_tag_pick(TagPick::Actions(id), ""),
            TagPick::OpenAll => self.open_all_tagged(id, now),
            TagPick::Actions(k) if id == 0 => {
                let name = self.tag_defs().get(k).map(|t| t.name.clone()).unwrap_or_default();
                self.open_tag_pick(TagPick::Name { tag: Some(k), assign: Vec::new() }, &name);
            }
            TagPick::Actions(k) if id == 1 => self.open_tag_pick(TagPick::Color { tag: Some(k), name: String::new(), assign: Vec::new() }, ""),
            TagPick::Actions(k) => self.open_tag_pick(TagPick::Delete(k), ""),
            TagPick::Color { tag: Some(k), .. } => self.effects.push(Effect::Tags(TagRequest::Recolor { tag: k, color: COLORS[id].1 })),
            TagPick::Color { tag: None, name, assign } => self.effects.push(Effect::Tags(TagRequest::Create { name, color: COLORS[id].1, assign })),
            TagPick::Delete(k) if id == 0 => self.effects.push(Effect::Tags(TagRequest::Delete(k))),
            TagPick::Delete(_) | TagPick::Name { .. } => {}
        }
    }

    pub(super) fn tag_input(&mut self, pick: TagPick, text: &str) {
        let TagPick::Name { tag, assign } = pick else { return };
        match (tags::validate_name(self.tag_defs(), text, tag), tag) {
            (Err(e), _) => {
                self.status(e);
                self.open_tag_pick(TagPick::Name { tag, assign }, text);
            }
            (Ok(name), Some(k)) => self.effects.push(Effect::Tags(TagRequest::Rename { tag: k, name })),
            (Ok(name), None) => self.open_tag_pick(TagPick::Color { tag: None, name, assign }, ""),
        }
    }

    pub(super) fn show_tag(&mut self, k: usize) {
        if let Some(t) = self.tag_defs().get(k) {
            let q = tags::query_for(&t.name);
            self.search_for(&q);
        }
    }

    pub(super) fn open_all_tagged(&mut self, k: usize, now: Instant) {
        let items: Vec<(FsPath, bool)> = self.tags().files.iter().filter(|f| f.present && f.mask & (1 << k) != 0).map(|f| (f.path.clone(), f.meta.attrs & attr::DIR != 0)).collect();
        let total = items.len();
        if total == 0 {
            return self.status("No available items have this tag");
        }
        let (dirs, files): (Vec<_>, Vec<_>) = items.into_iter().partition(|i| i.1);
        let tab = self.tab().id;
        for (path, _) in files.iter().take(MAX_OPEN) {
            self.effects.push(Effect::Provider { provider: ProviderId::Local, req: ProviderRequest::Open { tab, path: path.clone() } });
        }
        let routes: Vec<Route> = dirs.into_iter().take(MAX_OPEN).map(|(p, _)| Route { folder: Location::from_local(p), select: None }).collect();
        let opened = routes.len() + files.len().min(MAX_OPEN);
        if !routes.is_empty() {
            self.dispatch(routes, OpenTarget::NewTab, now);
        }
        if opened < total {
            self.status(format!("Opened {opened} of {total} tagged items"));
        }
    }

    pub(super) fn tags_moved(&mut self, r: &OpReport) {
        let copy = match r.op {
            FileOp::Copy { .. } => true,
            FileOp::Move { .. } | FileOp::Rename { .. } | FileOp::MoveTo(_) | FileOp::Restore(_) | FileOp::Trash(_) | FileOp::Flatten(_) => false,
            _ => return,
        };
        if self.tags().any() && !r.items.is_empty() {
            self.effects.push(Effect::Tags(TagRequest::Moved { pairs: r.items.clone(), copy }));
        }
    }

    fn drop_tag_bit(&mut self, k: usize) {
        for v in self.tabs.iter_mut().filter_map(|t| t.view.as_mut()) {
            let changes: Vec<(u32, u32)> = v.listing().entries.iter().enumerate().filter(|(_, e)| e.tags != 0).map(|(i, e)| (i as u32, tags::remove_bit(e.tags, k))).collect();
            v.set_tags(&changes);
        }
    }

    pub(super) fn apply_tag_fixes(&mut self, now: Instant) {
        if self.tag_fixes.is_empty() {
            return;
        }
        let fixes = std::mem::take(&mut self.tag_fixes);
        let (mut open, mut found) = (vec![false; fixes.len()], vec![false; fixes.len()]);
        for t in &mut self.tabs {
            let (Location::Local(dir), Some(v)) = (&t.view_loc, t.view.as_mut()) else { continue };
            let mut changes = Vec::new();
            for (k, (path, mask, _)) in fixes.iter().enumerate() {
                if path.parent().is_some_and(|p| p.as_bytes().eq_ignore_ascii_case(dir.as_bytes())) {
                    open[k] = true;
                    if let Some(i) = v.find(path.file_name()) {
                        changes.push((i, *mask));
                        found[k] = true;
                    }
                }
            }
            self.dirty |= !changes.is_empty();
            v.set_tags(&changes);
        }
        self.tag_fixes = fixes.into_iter().enumerate().filter(|(k, f)| open[*k] && !found[*k] && f.2 > now).map(|(_, f)| f).collect();
    }

    pub(super) fn tag_text(&self, s: &mut impl std::fmt::Write, mask: u32) {
        for (n, t) in tags::bits(mask).filter_map(|k| self.tag_defs().get(k)).enumerate() {
            let _ = write!(s, "{}{}", if n > 0 { ", " } else { "" }, t.name);
        }
    }

    pub(super) fn group_color(&self, v: &DirView, gr: &Group) -> Option<Color> {
        if v.snap().view.group.field != GroupField::Tag {
            return None;
        }
        let mask = v.listing().entries[gr.first as usize].tags;
        self.tag_defs().get(sort::first_tag(mask) as usize).map(|t| Color::rgb(t.color))
    }

    pub(super) fn draw_name<B: TextBackend>(&self, g: &mut Gfx<B>, p: &Parts, text: &str, mask: u32, font: FontStyle, color: Color) {
        let (r, m) = (p.name, g.metrics(font));
        let baseline = (r.y + (r.h + m.ascent - m.descent) / 2.0).round();
        let marks = || tags::bits(mask).filter_map(|k| self.tag_defs().get(k)).take(MAX_MARKS);
        let count = if mask == 0 { 0 } else { marks().count() };
        let (d, gap) = (self.s(MARK), self.s(MARK_GAP));
        let reserve = if count == 0 { 0.0 } else { count as f32 * (d + gap) + gap };
        let x = if p.center { r.x + ((r.w - reserve - g.measure(text, font)) / 2.0).max(0.0).round() } else { r.x };
        let w = g.text(x, baseline, text, font, color, (r.right() - x - reserve).max(0.0));
        let y = r.y + ((r.h - d) / 2.0).round();
        for (k, t) in marks().enumerate() {
            g.dl.round_rect(Rect::new((x + w + 2.0 * gap + k as f32 * (d + gap)).round(), y, d, d), Color::rgb(t.color), d / 2.0);
        }
    }
}

#[cfg(test)]
#[path = "../../test/ui/tags.rs"]
mod tests;
