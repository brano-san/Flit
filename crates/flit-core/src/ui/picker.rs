use super::confirm::Confirmed;
use super::edit::EditAction;
use super::field::{Field, FieldKey};
use super::menu::Cmd;
use super::widgets::Paint;
use super::{App, Effect, Focus, Hit, OpenTarget, Route};
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
        A11Y_BASE
            + match self {
                Self::Name => 0,
                Self::Filter => 1,
                Self::Accept => 2,
                Self::Cancel => 3,
                Self::Choice(i) => 4 + i as u64,
            }
    }
}

fn option_label((id, label): &(String, String)) -> &str {
    if label.is_empty() {
        id
    } else {
        label
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
                if let Some(on) = self.checked(i) {
                    format!("{}  {}", if on { "✓" } else { "□" }, choice.label).into()
                } else {
                    let value = option_label(&choice.values[choice.selected]);
                    format!("{} · {value}", choice.label).into()
                }
            }
            Control::Filter => self.filters[self.request.filter].label.as_str().into(),
            Control::Accept => self.request.accept.as_deref().unwrap_or_else(|| if self.request.saving() { S::PickSave.t() } else { S::PickOpen.t() }).into(),
            Control::Cancel => S::PickCancel.t().into(),
            Control::Name => S::PickName.t().into(),
        }
    }

    fn checked(&self, i: usize) -> Option<bool> {
        let choice = &self.request.choices[i];
        let [(off, off_label), (on, on_label)] = choice.values.as_slice() else { return None };
        (off == "false" && on == "true" && off_label.is_empty() && on_label.is_empty()).then_some(choice.selected == 1)
    }

    fn dropdown(&self, control: Control) -> Option<Hit> {
        match control {
            Control::Filter => Some(Hit::PickerFilter),
            Control::Choice(i) if self.checked(i).is_none() => Some(Hit::PickerChoice(i)),
            _ => None,
        }
    }

    fn picked(&self, paths: Vec<FsPath>) -> Effect {
        let choices = self.request.choices.iter().map(|c| (c.id.clone(), c.values[c.selected].0.clone())).collect();
        Effect::Picked { paths, filter: self.request.filter, choices }
    }

    fn controls(&self) -> impl Iterator<Item = Control> {
        let saving = matches!(self.request.mode, Mode::Save(_));
        let filters = !self.filters.is_empty();
        [saving.then_some(Control::Name), filters.then_some(Control::Filter)].into_iter().flatten().chain((0..self.request.choices.len()).map(Control::Choice)).chain([Control::Accept, Control::Cancel])
    }
}

impl App {
    pub fn start_picker(&mut self, mut request: Request) {
        self.close_dropdown();
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
        self.dialogs.picker = Some(Picker { request, name, original_name, filters, focused, pressed: None, preedit: None });
        self.apply_pick_filter(self.tabs.cur);
        self.layout();
    }

    pub fn picker_title(&self) -> Option<&str> {
        self.dialogs.picker.as_ref().map(|p| p.request.title.as_deref().unwrap_or_else(|| if p.request.saving() { S::PickSaveTitle.t() } else { S::PickOpenTitle.t() }))
    }

    pub(super) fn picker_uses_mime(&self) -> bool {
        self.dialogs.picker.as_ref().is_some_and(|picker| !picker.request.directories() && picker.filters.iter().any(|filter| !filter.mime_types.is_empty()))
    }

    pub(super) fn apply_pick_filter(&mut self, tab: usize) {
        if let Some((picker, view)) = self.dialogs.picker.as_ref().zip(self.tabs.items[tab].view.as_mut()) {
            view.set_pick_filter(picker.request.directories(), picker.filters.get(picker.request.filter).cloned());
        }
    }

    pub(super) fn picker_open_entry(&mut self, i: u32, now: Instant) -> bool {
        let Some(picker) = &mut self.dialogs.picker else { return false };
        let t = &self.tabs.items[self.tabs.cur];
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
        let Some(picker) = &self.dialogs.picker else { return Vec::new() };
        let t = self.tab();
        let view = t.view.as_ref().filter(|view| view.snap().gen == t.gen);
        let current = view.filter(|view| view.snap().done && view.snap().error.is_none()).and_then(|_| pick::local_path(self.current_location()));
        if matches!(picker.request.mode, Mode::Save(_)) {
            return current.filter(|_| pick::valid_name(picker.name(), self.caps.unix_paths)).map(|dir| dir.join(picker.name())).into_iter().collect();
        }
        let directories = picker.request.directories();
        let multiple = matches!(picker.request.mode, Mode::Open { multiple: true, .. });
        let paths: Vec<_> = view.into_iter().flat_map(|view| view.selected().filter(|&i| view.listing().entries[i as usize].is_dir() == directories)).take(if multiple { max } else { 1 }).filter_map(|i| pick::local_path(&t.entry_location(i)?).cloned()).collect();
        if directories && paths.is_empty() {
            current.into_iter().cloned().collect()
        } else {
            paths
        }
    }

    fn picker_activate(&mut self, control: Control, now: Instant) {
        if let Some(owner) = self.dialogs.picker.as_ref().and_then(|picker| picker.dropdown(control)) {
            self.open_picker_dropdown(control, owner);
            return;
        }
        match control {
            Control::Name | Control::Filter => {}
            Control::Choice(i) => {
                let Some(choice) = self.dialogs.picker.as_mut().and_then(|p| p.request.choices.get_mut(i)) else { return };
                choice.selected = usize::from(choice.selected == 0);
            }
            Control::Cancel => self.close_window(now),
            Control::Accept => {
                let Some(picker) = &self.dialogs.picker else { return };
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

    fn open_picker_dropdown(&mut self, control: Control, owner: Hit) {
        let Some((_, anchor)) = self.picker_rects().into_iter().find(|(c, r)| *c == control && r.w > 0.0 && r.h > 0.0) else { return };
        let Some(picker) = &mut self.dialogs.picker else { return };
        picker.pressed = None;
        let entries = match control {
            Control::Filter => picker.filters.iter().enumerate().map(|(k, filter)| (filter.label.clone(), Cmd::PickerOption(None, k), k == picker.request.filter)).collect(),
            Control::Choice(i) => {
                let choice = &picker.request.choices[i];
                choice.values.iter().enumerate().map(|(k, option)| (option_label(option).to_owned(), Cmd::PickerOption(Some(i), k), k == choice.selected)).collect()
            }
            _ => return,
        };
        self.dropdown_menu(owner, anchor, entries);
    }

    pub(super) fn choose_picker_option(&mut self, choice: Option<usize>, value: usize) {
        let Some(picker) = &mut self.dialogs.picker else { return };
        if let Some(i) = choice {
            let Some(choice) = picker.request.choices.get_mut(i).filter(|choice| value < choice.values.len()) else { return };
            choice.selected = value;
        } else {
            if value >= picker.filters.len() || value == picker.request.filter {
                return;
            }
            picker.request.filter = value;
            for tab in 0..self.tabs.items.len() {
                self.apply_pick_filter(tab);
            }
            self.reveal_focus(false);
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
        if let Some(picker) = &self.dialogs.picker {
            self.effects.push(picker.picked(vec![path]));
        }
    }

    fn picker_stacked(&self, width: f32) -> bool {
        self.dialogs.picker.as_ref().is_some_and(|p| matches!(p.request.mode, Mode::Save(_)) && width < self.s(if p.filters.is_empty() { 380.0 } else { 520.0 }))
    }

    pub(super) fn picker_height(&self, width: f32) -> f32 {
        let choices = self.dialogs.picker.as_ref().is_some_and(|p| !p.request.choices.is_empty());
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
        if let Some(picker) = &self.dialogs.picker {
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
                let name = if stacked { Rect::new(area.x + pad, area.y - self.s(32.0) + self.s(2.0), (area.w - 2.0 * pad).max(0.0), h) } else { Rect::new(area.x + pad, area.y + (area.h - h) * 0.5, (right - area.x - pad).max(0.0), h) };
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
        for (control, rect) in rects {
            if control == Control::Name {
                let Some(picker) = &mut self.dialogs.picker else { return };
                g.dl.round_rect(rect, t.input_bg, 3.0 * scale);
                let inner = Rect::new(rect.x + 6.0 * scale, rect.y, (rect.w - 12.0 * scale).max(0.0), rect.h);
                picker.name.draw(g, t, inner, font, picker.focused == Some(control), S::PickName.t(), scale, picker.preedit.as_ref());
                continue;
            }
            let Some(picker) = &self.dialogs.picker else { return };
            let focused = picker.focused == Some(control);
            let label = picker.label(control);
            let from = g.dl.instances.len();
            let mut paint = Paint::new(g, t, scale);
            if let Some(owner) = picker.dropdown(control) {
                let open = self.menu_open_from(owner);
                paint.picker(rect, &label, font, if open { 270.0 } else { 90.0 }, open, focused);
            } else {
                let hot = ((control == Control::Accept && enabled) || focused).then_some(t.accent);
                paint.button(rect, &label, font, hot);
            }
            g.dl.confine(from, rect);
        }
    }

    pub(super) fn picker_accessibility(&self, tree: &mut crate::a11y::Tree, focus: &mut Option<usize>) {
        use crate::a11y::Role;
        let Some(picker) = &self.dialogs.picker else { return };
        for (control, rect) in self.picker_rects() {
            let (role, name, value, selected) = match control {
                Control::Name => (Role::Edit, S::PickName.t().into(), picker.name.text(), false),
                Control::Filter => (Role::ComboBox, S::ActionsViewFilter.t().into(), picker.filters[picker.request.filter].label.as_str(), false),
                Control::Choice(i) => {
                    let choice = &picker.request.choices[i];
                    match picker.checked(i) {
                        Some(on) => (Role::CheckBox, Cow::Borrowed(choice.label.as_str()), "", on),
                        None => (Role::ComboBox, Cow::Borrowed(choice.label.as_str()), option_label(&choice.values[choice.selected]), false),
                    }
                }
                _ => (Role::Button, picker.label(control), "", false),
            };
            let i = tree.add(0, control.id(), role, name.into_owned(), rect);
            let node = &mut tree.nodes[i];
            node.value = value.to_owned();
            node.selected = selected;
            node.enabled = control != Control::Accept || !self.picker_paths(1).is_empty();
            if picker.focused == Some(control) {
                *focus = Some(i);
            }
        }
    }

    pub(super) fn picker_event(&mut self, event: &Event, now: Instant) -> bool {
        if self.dialogs.confirm.is_some() {
            return false;
        }
        let Some(picker) = &mut self.dialogs.picker else { return false };
        match *event {
            Event::A11yFocus(id) | Event::Invoke(id) => {
                let Some(control) = picker.controls().find(|c| c.id() == id) else { return false };
                picker.focused = Some(control);
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
                let Some(picker) = &mut self.dialogs.picker else { return false };
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
        if self.dialogs.picker.is_none() {
            return true;
        }
        matches!(id, "item.open" | "app.quit" | "tab.close" | "folder.dropdown") || ["address.", "list.", "nav.", "select.", "text.", "view."].iter().any(|prefix| id.starts_with(prefix))
    }
}

#[cfg(test)]
#[path = "../../test/ui/picker.rs"]
mod tests;
