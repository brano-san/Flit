use super::confirm::Confirmed;
use super::edit::EditAction;
use super::field::{Field, FieldKey};
use super::widgets::Paint;
use super::{App, Effect, Focus, OpenTarget, Route};
use crate::i18n::S;
use crate::input::{Event, Key, MouseButton, Preedit};
use crate::pick::{self, Filter, Mode, Request};
use crate::render::Rect;
use crate::text::{Gfx, TextBackend};
use crate::theme::Theme;
use crate::FsPath;
use std::borrow::Cow;
use std::sync::Arc;
use std::time::Instant;

const A11Y_BASE: u64 = 1 << 58;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Control {
    Name,
    Filter,
    Choice(usize),
    Accept,
    Cancel,
}

impl Control {
    fn id(self) -> u64 {
        A11Y_BASE + match self {
            Self::Name => 0,
            Self::Filter => 1,
            Self::Accept => 2,
            Self::Cancel => 3,
            Self::Choice(i) => 4 + i as u64,
        }
    }
}

pub(super) struct Picker {
    request: Request,
    name: Field,
    original_name: String,
    filters: Vec<Arc<Filter>>,
    focused: Option<Control>,
    pressed: Option<Control>,
    preedit: Option<Preedit>,
}

impl Picker {
    fn name(&self) -> &[u8] {
        match &self.request.mode {
            Mode::Save(original) if self.name.text() == self.original_name => original,
            _ => self.name.text().as_bytes(),
        }
    }

    fn label(&self, control: Control) -> Cow<'_, str> {
        match control {
            Control::Choice(i) => {
                let choice = &self.request.choices[i];
                let (id, value) = &choice.values[choice.selected];
                if value.is_empty() {
                    format!("{}  {}", if id == "true" { "✓" } else { "□" }, choice.label).into()
                } else {
                    format!("{} · {value}", choice.label).into()
                }
            }
            Control::Filter => self.filters[self.request.filter].label.as_str().into(),
            Control::Accept => self.request.accept.as_deref().unwrap_or_else(|| if self.request.saving() { S::PickSave.t() } else { S::PickOpen.t() }).into(),
            Control::Cancel => S::PickCancel.t().into(),
            Control::Name => S::PickName.t().into(),
        }
    }

    fn picked(&self, paths: Vec<FsPath>) -> Effect {
        let choices = self.request.choices.iter().map(|c| (c.id.clone(), c.values[c.selected].0.clone())).collect();
        Effect::Picked { paths, filter: self.request.filter, choices }
    }

    fn controls(&self) -> impl Iterator<Item = Control> {
        let saving = matches!(self.request.mode, Mode::Save(_));
        let filters = !self.filters.is_empty();
        [saving.then_some(Control::Name), filters.then_some(Control::Filter)].into_iter().flatten()
            .chain((0..self.request.choices.len()).map(Control::Choice))
            .chain([Control::Accept, Control::Cancel])
    }
}

impl App {
    pub fn start_picker(&mut self, mut request: Request) {
        request.filter = request.filter.min(request.filters.len().saturating_sub(1));
        request.choices.retain(|c| !c.values.is_empty());
        for choice in &mut request.choices {
            choice.selected = choice.selected.min(choice.values.len() - 1);
        }
        let mut name = Field::default();
        let original_name = match &request.mode {
            Mode::Save(name) => crate::name::display(name).into_owned(),
            _ => String::new(),
        };
        name.edit.set(&original_name);
        name.edit.select_all();
        let focused = matches!(request.mode, Mode::Save(_)).then_some(Control::Name);
        let filters = std::mem::take(&mut request.filters).into_iter().map(Arc::new).collect();
        self.picker = Some(Picker { request, name, original_name, filters, focused, pressed: None, preedit: None });
        self.apply_pick_filter(self.cur);
        self.layout();
    }

    pub fn picker_title(&self) -> Option<&str> {
        self.picker.as_ref().map(|p| p.request.title.as_deref().unwrap_or_else(|| if p.request.saving() { S::PickSaveTitle.t() } else { S::PickOpenTitle.t() }))
    }

    pub(super) fn picker_uses_mime(&self) -> bool {
        self.picker.as_ref().is_some_and(|picker| {
            !picker.request.directories() && picker.filters.iter().any(|filter| !filter.mime_types.is_empty())
        })
    }

    pub(super) fn apply_pick_filter(&mut self, tab: usize) {
        if let Some((picker, view)) = self.picker.as_ref().zip(self.tabs[tab].view.as_mut()) {
            view.set_pick_filter(picker.request.directories(), picker.filters.get(picker.request.filter).cloned());
        }
    }

    pub(super) fn picker_open_entry(&mut self, i: u32, now: Instant) -> bool {
        let Some(picker) = &mut self.picker else { return false };
        let t = &self.tabs[self.cur];
        let Some(view) = t.view.as_ref() else { return true };
        if view.snap().gen != t.gen || !view.is_visible(i) {
            return true;
        }
        let Some(location) = t.entry_location(i) else { return true };
        let entry = view.listing().entries[i as usize];
        if entry.is_dir() {
            self.dispatch(vec![Route { folder: location, select: None }], OpenTarget::CurrentTab, now);
        } else if let Some(path) = pick::local_path(&location) {
            match picker.request.mode {
                Mode::Open { directories: false, .. } => self.effects.push(picker.picked(vec![path.clone()])),
                Mode::Save(_) => {
                    picker.name.edit.set(&crate::name::display(path.file_name()));
                    picker.focused = Some(Control::Name);
                    self.dirty = true;
                }
                _ => {}
            }
        }
        true
    }

    fn picker_paths(&self, max: usize) -> Vec<FsPath> {
        let Some(picker) = &self.picker else { return Vec::new() };
        let t = self.tab();
        let view = t.view.as_ref().filter(|view| view.snap().gen == t.gen);
        let current = view.filter(|view| view.snap().done && view.snap().error.is_none()).and_then(|_| pick::local_path(self.current_location()));
        if matches!(picker.request.mode, Mode::Save(_)) {
            return current.filter(|_| pick::valid_name(picker.name(), self.caps.unix_paths)).map(|dir| dir.join(picker.name())).into_iter().collect();
        }
        let directories = picker.request.directories();
        let multiple = matches!(picker.request.mode, Mode::Open { multiple: true, .. });
        let paths: Vec<_> = view.into_iter().flat_map(|view| {
            view.selected().filter(|&i| view.listing().entries[i as usize].is_dir() == directories)
        }).take(if multiple { max } else { 1 })
            .filter_map(|i| pick::local_path(&t.entry_location(i)?).cloned()).collect();
        if directories && paths.is_empty() { current.into_iter().cloned().collect() } else { paths }
    }

    fn picker_activate(&mut self, control: Control, now: Instant) {
        match control {
            Control::Name => {}
            Control::Filter => {
                let Some(picker) = &mut self.picker else { return };
                picker.request.filter = (picker.request.filter + 1) % picker.filters.len();
                for tab in 0..self.tabs.len() {
                    self.apply_pick_filter(tab);
                }
                self.reveal_focus(false);
            }
            Control::Choice(i) => {
                let Some(choice) = self.picker.as_mut().and_then(|p| p.request.choices.get_mut(i)) else { return };
                choice.selected = (choice.selected + 1) % choice.values.len();
            }
            Control::Cancel => self.close_window(now),
            Control::Accept => {
                let Some(picker) = &self.picker else { return };
                let paths = self.picker_paths(usize::MAX);
                if let Some(path) = paths.first() {
                    if matches!(picker.request.mode, Mode::Save(_)) {
                        if let Some(directory) = self.picker_existing(path.file_name()) {
                            if !directory {
                                let anchor = self.picker_rects().into_iter().find(|(c, _)| *c == Control::Accept).map_or_else(Rect::default, |(_, r)| r);
                                let question = crate::tr!(S::UiExportReplaceTitle, name = crate::name::display(path.file_name()));
                                self.ask(anchor, (&question, S::UiButtonReplace.t()), true, Confirmed::Pick(path.clone()), now);
                            }
                            return;
                        }
                    }
                    self.effects.push(picker.picked(paths));
                }
            }
        }
        self.dirty = true;
    }

    fn picker_existing(&self, name: &[u8]) -> Option<bool> {
        let listing = self.tab().view.as_ref()?.listing();
        let local_name = (!self.caps.unix_paths).then(|| crate::name::display(name).to_lowercase());
        listing.entries.iter().enumerate().find_map(|(i, entry)| {
            let other = listing.name(i as u32);
            let same = other == name || local_name.as_ref().is_some_and(|s| *s == crate::name::display(other).to_lowercase());
            (same && entry.attrs & crate::listing::attr::DELETED == 0).then(|| entry.is_dir())
        })
    }

    pub(super) fn pick_confirmed(&mut self, path: FsPath) {
        if let Some(picker) = &self.picker {
            self.effects.push(picker.picked(vec![path]));
        }
    }

    fn picker_stacked(&self, width: f32) -> bool {
        self.picker.as_ref().is_some_and(|p| matches!(p.request.mode, Mode::Save(_)) && width < self.s(if p.filters.is_empty() { 380.0 } else { 520.0 }))
    }

    pub(super) fn picker_height(&self, width: f32) -> f32 {
        let choices = self.picker.as_ref().is_some_and(|p| !p.request.choices.is_empty());
        36.0 + if self.picker_stacked(width) { 32.0 } else { 0.0 } + if choices { 32.0 } else { 0.0 }
    }

    fn picker_rects(&self) -> Vec<(Control, Rect)> {
        let status = self.layout.status;
        let stacked = self.picker_stacked(status.w);
        let h = self.s(36.0).min(status.h);
        let area = Rect::new(status.x, status.bottom() - h, status.w, h);
        let h = (area.h - self.s(8.0)).max(0.0);
        let pad = self.s(6.0);
        let mut right = area.right() - pad;
        let part = |right: &mut f32, width: f32| {
            *right -= width;
            let r = Rect::new(*right, area.y + (area.h - h) * 0.5, width, h);
            *right -= pad;
            r
        };
        let width = self.s(106.0).min((area.w - pad * 4.0).max(0.0) / 3.0);
        let mut out = vec![(Control::Cancel, part(&mut right, width)), (Control::Accept, part(&mut right, width))];
        if let Some(picker) = &self.picker {
            let count = picker.request.choices.len();
            if count != 0 {
                let space = (status.w - pad * (count as f32 + 1.0)).max(0.0);
                let width = (space / count as f32).min(self.s(220.0));
                out.extend((0..count).map(|i| {
                    let x = status.x + pad + (pad + width) * i as f32;
                    (Control::Choice(i), Rect::new(x, status.y + self.s(4.0), width, self.s(24.0)))
                }));
            }
            let remaining = (right - area.x - pad).max(0.0);
            if !picker.filters.is_empty() {
                let width = if !stacked && matches!(picker.request.mode, Mode::Save(_)) { remaining * 0.42 } else { remaining };
                out.push((Control::Filter, part(&mut right, width.min(self.s(250.0)))));
            }
            if matches!(picker.request.mode, Mode::Save(_)) {
                let name = if stacked {
                    Rect::new(area.x + pad, area.y - self.s(32.0) + self.s(2.0), (area.w - 2.0 * pad).max(0.0), h)
                } else {
                    Rect::new(area.x + pad, area.y + (area.h - h) * 0.5, (right - area.x - pad).max(0.0), h)
                };
                out.push((Control::Name, name));
            }
        }
        out
    }

    pub(super) fn draw_picker<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme) {
        self.layout.bar.clear();
        let rects = self.picker_rects();
        let font = self.ui_font(12.0);
        let scale = self.scale;
        let enabled = !self.picker_paths(1).is_empty();
        let Some(picker) = &mut self.picker else { return };
        for (control, rect) in rects {
            let focused = picker.focused == Some(control);
            if control == Control::Name {
                g.dl.round_rect(rect, t.input_bg, 3.0 * scale);
                picker.name.draw(g, t, Rect::new(rect.x + 6.0 * scale, rect.y, (rect.w - 12.0 * scale).max(0.0), rect.h), font, focused, S::PickName.t(), scale, picker.preedit.as_ref());
                continue;
            }
            let label = picker.label(control);
            let hot = ((control == Control::Accept && enabled) || focused).then_some(t.accent);
            let from = g.dl.instances.len();
            Paint::new(g, t, scale).button(rect, &label, font, hot);
            g.dl.confine(from, rect);
        }
    }

    pub(super) fn picker_accessibility(&self, tree: &mut crate::a11y::Tree, focus: &mut Option<usize>) {
        use crate::a11y::Role;
        let Some(picker) = &self.picker else { return };
        for (control, rect) in self.picker_rects() {
            let role = if control == Control::Name { Role::Edit } else { Role::Button };
            let i = tree.add(0, control.id(), role, picker.label(control).into_owned(), rect);
            if control == Control::Name {
                tree.nodes[i].value = picker.name.text().to_owned();
            }
            if control == Control::Accept {
                tree.nodes[i].enabled = !self.picker_paths(1).is_empty();
            }
            if picker.focused == Some(control) {
                *focus = Some(i);
            }
        }
    }

    pub(super) fn picker_event(&mut self, event: &Event, now: Instant) -> bool {
        if self.confirm.is_some() {
            return false;
        }
        let Some(picker) = &mut self.picker else { return false };
        match *event {
            Event::A11yFocus(id) | Event::Invoke(id) => {
                let control = picker.controls().find(|c| c.id() == id);
                picker.focused = control;
                let Some(control) = control else { return false };
                self.focus = Focus::List;
                if matches!(event, Event::Invoke(_)) {
                    self.picker_activate(control, now);
                }
                self.dirty = true;
            }
            Event::Key { key: Key::Escape, pressed: true, .. } => self.picker_activate(Control::Cancel, now),
            Event::Key { key: Key::Tab, mods, pressed: true } if !mods.ctrl && !mods.alt && self.focus == Focus::List => {
                let controls: Vec<_> = picker.controls().map(Some).chain(std::iter::once(None)).collect();
                let at = controls.iter().position(|c| *c == picker.focused).unwrap_or(controls.len() - 1);
                picker.focused = controls[(at + if mods.shift { controls.len() - 1 } else { 1 }) % controls.len()];
                self.dirty = true;
            }
            Event::Key { key: Key::Enter, pressed: true, .. } if self.focus == Focus::List => {
                let focused = picker.focused;
                if focused.is_none() && self.picker_paths(1).is_empty() {
                    return false;
                }
                self.picker_activate(focused.filter(|c| *c != Control::Name).unwrap_or(Control::Accept), now);
            }
            Event::Key { key: Key::Space, pressed: true, .. } if picker.focused.is_some_and(|c| c != Control::Name) => {
                if let Some(control) = picker.focused {
                    self.picker_activate(control, now);
                }
            }
            Event::MouseDown { x, y, button: MouseButton::Left, clicks } => {
                let target = self.picker_rects().into_iter().find(|(_, r)| r.contains(x, y));
                let Some(picker) = &mut self.picker else { return false };
                picker.focused = target.map(|(control, _)| control);
                picker.pressed = picker.focused;
                picker.preedit = None;
                let Some((control, rect)) = target else { return false };
                self.focus = Focus::List;
                if control == Control::Name {
                    picker.name.click(x - rect.x - 6.0 * self.scale, clicks, self.mods.shift);
                }
                self.dirty = true;
            }
            Event::MouseUp { x, y, button: MouseButton::Left } => {
                let Some(pressed) = picker.pressed.take() else { return false };
                if self.picker_rects().iter().any(|(c, r)| *c == pressed && r.contains(x, y)) {
                    self.picker_activate(pressed, now);
                }
            }
            Event::MouseDown { button: MouseButton::Right, .. } | Event::ShowMenu { .. } | Event::DroppedFiles(_) => {}
            _ => {
                if picker.focused != Some(Control::Name) {
                    return false;
                }
                match event {
                    Event::Char(c) if !c.is_control() => picker.name.insert(c.encode_utf8(&mut [0; 4])),
                    Event::Pasted(text) => picker.name.insert(text),
                    Event::Preedit(preedit) => picker.preedit = Some(preedit.clone()),
                    Event::Key { key, mods, pressed: true } => match picker.name.key(*key, *mods) {
                        FieldKey::Edited { action: EditAction::Copy(text), .. } => self.effects.push(Effect::SetClipboard(text)),
                        FieldKey::Edited { action: EditAction::Paste, .. } => self.effects.push(Effect::Paste),
                        FieldKey::Edited { .. } => {}
                        FieldKey::Ignored => return false,
                    },
                    _ => return false,
                }
                self.dirty = true;
            }
        }
        true
    }

    pub(super) fn picker_allows(&self, id: &str) -> bool {
        if self.picker.is_none() {
            return true;
        }
        matches!(id, "item.open" | "app.quit" | "tab.close" | "folder.dropdown")
            || ["address.", "list.", "nav.", "select.", "text.", "view."].iter().any(|prefix| id.starts_with(prefix))
    }
}

#[cfg(test)]
#[path = "../../test/ui/picker.rs"]
mod tests;
