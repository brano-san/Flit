use super::archive::Kind;
use super::edit::EditAction;
use super::field::{Field, FieldKey};
use super::a11y::put;
use super::{centered, App, Effect, Hit};
use crate::a11y::{Role, Tree};
use crate::input::{Key, Mods, MouseButton};
use crate::render::Rect;
use std::time::Instant;
use crate::text::{FontFamily, FontStyle, Gfx, TextBackend};
use crate::theme::Theme;

const FORM_ID: u64 = 12;
const WIDTH: f32 = 600.0;
const HEAD: f32 = 56.0;
const ROW: f32 = 34.0;
const VALUE_ROW: f32 = 24.0;
const LABEL_W: f32 = 170.0;
const BUTTON_W: f32 = 112.0;
const BUTTON_H: f32 = 32.0;
const NOTE_H: f32 = 34.0;
const SECTION_GAP: f32 = 10.0;
const BUTTON_GAP: f32 = 16.0;
const FLAGS_PER_LINE: usize = 4;

const G_LEFT: &str = "\u{E76B}";
const G_RIGHT: &str = "\u{E76C}";

fn form_scale(w: f32, h: f32, scale: f32) -> f32 {
    scale.min((w / 360.0).max(0.1)).min((h / 320.0).max(0.1))
}

pub(super) enum Control {
    Text { label: String, field: Field, placeholder: &'static str },
    Choice { label: String, options: Vec<String>, sel: usize },
    Check { label: String, on: bool },
    Flags { label: String, items: Vec<(String, bool)>, cur: usize, spots: Vec<Rect> },
    Value { label: String, value: String },
    Line(String),
}

impl Control {
    pub fn text(label: &str, value: &str, placeholder: &'static str) -> Self {
        let mut field = Field::default();
        field.edit.set(value);
        Control::Text { label: label.to_owned(), field, placeholder }
    }

    pub fn secret(label: &str, placeholder: &'static str) -> Self {
        let mut c = Self::text(label, "", placeholder);
        if let Control::Text { field, .. } = &mut c {
            field.secret = true;
        }
        c
    }

    pub fn choice(label: &str, options: Vec<String>, sel: usize) -> Self {
        let sel = sel.min(options.len().saturating_sub(1));
        Control::Choice { label: label.to_owned(), options, sel }
    }

    pub fn check(label: &str, on: bool) -> Self {
        Control::Check { label: label.to_owned(), on }
    }

    pub fn flags(label: &str, items: Vec<(String, bool)>) -> Self {
        Control::Flags { label: label.to_owned(), items, cur: 0, spots: Vec::new() }
    }

    pub fn value(label: &str, value: &str) -> Self {
        Control::Value { label: label.to_owned(), value: value.to_owned() }
    }

    pub fn line(text: &str) -> Self {
        Control::Line(text.to_owned())
    }

    fn height(&self) -> f32 {
        match self {
            Control::Value { .. } | Control::Line(_) => VALUE_ROW,
            Control::Flags { items, .. } => ROW * flag_grid(items.len()).0 as f32,
            _ => ROW,
        }
    }
}

fn section_gaps(controls: &[Control]) -> Vec<bool> {
    let line = |c: &Control| matches!(c, Control::Line(_));
    controls.iter().enumerate().map(|(k, c)| k > 0 && line(c) != line(&controls[k - 1])).collect()
}

fn flag_grid(n: usize) -> (usize, usize) {
    let lines = n.div_ceil(FLAGS_PER_LINE).max(1);
    (lines, n.div_ceil(lines).max(1))
}

pub(super) struct Form {
    pub kind: Kind,
    pub title: String,
    pub controls: Vec<Control>,
    pub buttons: Vec<&'static str>,
    pub focus: usize,
    pub note: String,
    pub error: bool,
    pub progress: Option<f32>,
    pub rows: Vec<Rect>,
    pub button_rects: Vec<Rect>,
    pub(super) rect: Rect,
    pub(super) close_rect: Rect,
    pub document: super::update_dialog::Document,
}

impl Form {
    fn visible_row(&self, row: Rect) -> Rect {
        if matches!(self.kind, Kind::Update(_)) { return row; }
        let v = self.document.viewport;
        let y = row.y - self.document.scroll.pos;
        let top = y.max(v.y);
        Rect::new(row.x, top, row.w, ((y + row.h).min(v.bottom()) - top).max(0.0))
    }

    pub fn new(kind: Kind, title: &str, controls: Vec<Control>, buttons: Vec<&'static str>) -> Self {
        let focus = controls.iter().position(|c| !matches!(c, Control::Value { .. } | Control::Line(_))).unwrap_or(controls.len());
        Self { kind, title: title.to_owned(), controls, buttons, focus, note: String::new(), error: false, progress: None, rect: Rect::default(), close_rect: Rect::default(), rows: Vec::new(), button_rects: Vec::new(), document: Default::default() }
    }

    pub fn text(&self, k: usize) -> &str {
        match self.controls.get(k) {
            Some(Control::Text { field, .. }) => field.text(),
            _ => "",
        }
    }

    pub fn wipe_secrets(&mut self) {
        for c in &mut self.controls {
            if let Control::Text { field, .. } = c {
                if field.secret {
                    field.edit.wipe();
                }
            }
        }
    }

    pub fn set_text(&mut self, k: usize, value: &str) {
        if let Some(Control::Text { field, .. }) = self.controls.get_mut(k) {
            field.edit.set(value);
        }
    }

    pub fn choice(&self, k: usize) -> usize {
        match self.controls.get(k) {
            Some(Control::Choice { sel, .. }) => *sel,
            _ => 0,
        }
    }

    pub fn checked(&self, k: usize) -> bool {
        matches!(self.controls.get(k), Some(Control::Check { on: true, .. }))
    }

    pub fn flags(&self, k: usize) -> Vec<bool> {
        match self.controls.get(k) {
            Some(Control::Flags { items, .. }) => items.iter().map(|(_, on)| *on).collect(),
            _ => Vec::new(),
        }
    }

    pub fn say(&mut self, note: impl Into<String>, error: bool) {
        self.note = note.into();
        self.error = error;
    }

    fn slots(&self) -> usize {
        self.controls.len() + self.buttons.len() + usize::from(matches!(self.kind, Kind::Properties(_)) && self.buttons.len() > 1)
    }

    fn step(&mut self, back: bool) {
        let n = self.slots();
        for _ in 0..n {
            self.focus = if back { (self.focus + n - 1) % n } else { (self.focus + 1) % n };
            if !matches!(self.controls.get(self.focus), Some(Control::Line(_))) {
                break;
            }
        }
    }
}

impl App {
    fn reveal_form_focus(&mut self) {
        let Some(f) = &mut self.jobs.form else { return };
        if matches!(f.kind, Kind::Update(_)) { return; }
        let Some(row) = f.rows.get(f.focus) else { return };
        let v = f.document.viewport;
        let top = row.y - v.y;
        let next = if top < f.document.scroll.target { top } else { f.document.scroll.target.max(top + row.h - v.h) };
        f.document.scroll.snap(next.clamp(0.0, f.document.max()));
    }

    pub(super) fn open_form(&mut self, form: Form) {
        self.nav.palette = None;
        self.jobs.form = Some(form);
        self.layout();
        self.dirty = true;
    }

    fn form_button(&mut self, b: usize) {
        let b = self.jobs.form.as_ref().map_or(b, |f| b.min(f.buttons.len().saturating_sub(1)));
        self.form_submit(b);
        self.layout();
        self.dirty = true;
    }

    fn copy_value(&mut self, k: usize) {
        let Some(f) = &mut self.jobs.form else { return };
        if let Some(Control::Value { label, value }) = f.controls.get(k) {
            let text = value.clone();
            let feedback = crate::tr!(crate::i18n::S::FormCopied, label = label);
            if matches!(f.kind, Kind::Update(_)) {
                f.document.copied = Some((feedback, std::time::Instant::now()));
            } else {
                f.say(feedback, false);
            }
            self.effects.push(Effect::SetClipboard(text));
        }
        self.layout();
    }

    fn form_activate(&mut self, k: usize) {
        let Some(f) = &mut self.jobs.form else { return };
        let n = f.controls.len();
        if k >= n {
            return self.form_button(k - n);
        }
        match &mut f.controls[k] {
            Control::Check { on, .. } => *on = !*on,
            Control::Flags { items, cur, .. } => {
                if let Some(item) = items.get_mut(*cur) {
                    item.1 = !item.1;
                }
            }
            Control::Value { .. } => return self.copy_value(k),
            _ => return,
        }
        self.form_changed(k);
    }

    fn turn(&mut self, k: usize, back: bool) {
        let Some(f) = &mut self.jobs.form else { return };
        match f.controls.get_mut(k) {
            Some(Control::Choice { options, sel, .. }) if !options.is_empty() => {
                let n = options.len();
                *sel = if back { (*sel + n - 1) % n } else { (*sel + 1) % n };
            }
            Some(Control::Flags { items, cur, .. }) if !items.is_empty() => {
                let n = items.len();
                *cur = if back { (*cur + n - 1) % n } else { (*cur + 1) % n };
                return;
            }
            _ => return,
        }
        self.form_changed(k);
    }

    pub(super) fn form_key(&mut self, key: Key, m: Mods) -> bool {
        if self.update_scroll_key(key, m) {
            return true;
        }
        let Some(f) = &mut self.jobs.form else { return false };
        let k = f.focus;
        let n = f.controls.len();
        let plain = !m.ctrl && !m.alt;
        match key {
            Key::Escape => {
                let last = f.buttons.len().saturating_sub(1);
                self.form_button(last);
            }
            Key::Enter => self.form_button(k.saturating_sub(n)),
            Key::Tab => f.step(m.shift),
            Key::Up if plain => f.step(true),
            Key::Down if plain => f.step(false),
            Key::Left | Key::Right if k >= n => {
                let b = f.buttons.len();
                let at = (k - n).min(b.saturating_sub(1));
                f.focus = n + if key == Key::Left { (at + b - 1) % b } else { (at + 1) % b };
            }
            Key::Left | Key::Right if matches!(f.controls.get(k), Some(Control::Choice { .. } | Control::Flags { .. })) => self.turn(k, key == Key::Left),
            Key::Space if !matches!(f.controls.get(k), Some(Control::Text { .. })) => self.form_activate(k),
            Key::Char('C') if m.ctrl && matches!(f.controls.get(k), Some(Control::Value { .. })) => self.copy_value(k),
            _ => {
                let Some(Control::Text { field, .. }) = f.controls.get_mut(k) else { return true };
                let secret = field.secret;
                if let FieldKey::Edited { changed, action } = field.key(key, m) {
                    match action {
                        EditAction::Copy(s) if !secret => self.effects.push(Effect::SetClipboard(s)),
                        EditAction::Paste => self.effects.push(Effect::Paste),
                        _ => {}
                    }
                    if changed {
                        self.form_changed(k);
                    }
                }
            }
        }
        self.reveal_form_focus();
        self.dirty = true;
        true
    }

    pub(super) fn form_insert(&mut self, s: &str) -> bool {
        let Some(f) = &mut self.jobs.form else { return false };
        if !self.jobs.asks.is_empty() {
            return true;
        }
        let k = f.focus;
        if let Some(Control::Text { field, .. }) = f.controls.get_mut(k) {
            field.insert(s.lines().next().unwrap_or(""));
            self.form_changed(k);
            self.dirty = true;
        }
        true
    }

    pub(super) fn form_text(&self, k: usize) -> Option<&Field> {
        match self.jobs.form.as_ref()?.controls.get(k)? {
            Control::Text { field, .. } => Some(field),
            _ => None,
        }
    }

    pub(super) fn form_text_mut(&mut self, k: usize) -> Option<&mut Field> {
        match self.jobs.form.as_mut()?.controls.get_mut(k)? {
            Control::Text { field, .. } => Some(field),
            _ => None,
        }
    }

    pub(super) fn form_text_focus(&self) -> Option<usize> {
        let k = self.jobs.form.as_ref().filter(|_| self.jobs.asks.is_empty())?.focus;
        self.form_text(k).map(|_| k)
    }

    pub(super) fn form_text_rect(&self, k: usize) -> Option<Rect> {
        let f = self.jobs.form.as_ref()?;
        f.rows.get(k).map(|r| f.visible_row(*r))
    }

    pub(super) fn form_down(&mut self, x: f32, y: f32, button: MouseButton, clicks: u8, now: Instant) -> bool {
        if self.jobs.form.is_none() || !self.jobs.asks.is_empty() {
            return false;
        }
        if let Some(f) = &self.jobs.form {
            if matches!(f.kind, Kind::Properties(_)) && !f.rect.contains(x, y) && !self.in_title(y) {
                if button == MouseButton::Left && !App::properties_edited(f) {
                    self.form_button(f.buttons.len().saturating_sub(1));
                }
                return true;
            }
        }
        if self.update_scroll_down(x, y) {
            return true;
        }
        let Some(Hit::Form(k)) = self.form_hit(x, y) else { return false };
        self.dirty = true;
        if self.form_text(k).is_some() {
            if let Some(f) = &mut self.jobs.form {
                f.focus = k;
            }
            if button == MouseButton::Right {
                self.text_field_menu(x, y);
                return true;
            }
            let (clicks, shift) = (self.text_clicks((x, y), clicks, now), self.mods.shift);
            if let Some(field) = self.form_text_mut(k) {
                field.click(x, clicks, shift);
            }
            self.pressed = Hit::Form(k);
            return true;
        }
        let Some(f) = &mut self.jobs.form else { return false };
        f.focus = k;
        let n = f.controls.len();
        self.pressed = Hit::Form(k);
        if k >= n {
            return true;
        }
        let row = f.visible_row(f.rows[k]);
        match &mut f.controls[k] {
            Control::Choice { .. } => self.turn(k, x < row.x + row.w * 0.66),
            Control::Flags { spots, cur, .. } => {
                if let Some(i) = spots.iter().position(|s| s.contains(x, y)) {
                    *cur = i;
                    self.form_activate(k);
                }
            }
            _ => self.form_activate(k),
        }
        true
    }

    pub(super) fn form_up(&mut self, h: Hit) -> bool {
        let (Hit::Form(k), Some(f)) = (h, &self.jobs.form) else { return false };
        if k >= f.controls.len() {
            self.form_activate(k);
        }
        true
    }

    pub(super) fn form_a11y(&self, t: &mut Tree, targets: &mut Vec<(u64, Hit)>) -> Option<usize> {
        let f = self.jobs.form.as_ref().filter(|_| self.jobs.asks.is_empty())?;
        let d = t.add(0, FORM_ID, Role::Dialog, &f.title, f.rect);
        t.nodes[d].help = f.note.clone();
        let mut focus = d;
        for (k, (c, &r)) in f.controls.iter().zip(&f.rows).enumerate() {
            let r = f.visible_row(r);
            let (role, label, value) = match c {
                Control::Text { label, field, .. } => (Role::Edit, label, if field.secret { String::new() } else { field.text().to_owned() }),
                Control::Choice { label, options, sel } => (Role::ComboBox, label, options.get(*sel).cloned().unwrap_or_default()),
                Control::Check { label, .. } => (Role::CheckBox, label, String::new()),
                Control::Flags { label, items, .. } => (Role::Group, label, items.iter().filter(|i| i.1).map(|i| i.0.as_str()).collect::<Vec<_>>().join(", ")),
                Control::Value { label, value } => (Role::Text, label, value.clone()),
                Control::Line(text) => (Role::Text, text, String::new()),
            };
            let node = put(t, targets, d, Hit::Form(k), role, label, r);
            t.nodes[node].value = value;
            t.nodes[node].selected = matches!(c, Control::Check { on: true, .. });
            if f.focus == k {
                focus = node;
            }
            if let Control::Flags { items, cur, spots, .. } = c {
                for (j, (name, on)) in items.iter().enumerate() {
                    let option = put(t, targets, node, Hit::FormFlag(k, j), Role::CheckBox, name, spots.get(j).copied().unwrap_or_default());
                    t.nodes[option].selected = *on;
                    if f.focus == k && *cur == j {
                        focus = option;
                    }
                }
            }
        }
        let n = f.controls.len();
        for (b, (label, &r)) in f.buttons.iter().zip(&f.button_rects).enumerate() {
            let node = put(t, targets, d, Hit::Form(n + b), Role::Button, label, r);
            if f.focus == n + b {
                focus = node;
            }
        }
        if matches!(f.kind, Kind::Properties(_)) && f.buttons.len() > 1 {
            let k = n + f.buttons.len();
            let node = put(t, targets, d, Hit::Form(k), Role::Button, crate::i18n::S::ArchiveClose.t(), f.close_rect);
            if f.focus == k { focus = node; }
        }
        Some(focus)
    }

    pub(super) fn form_a11y_focus(&mut self, k: usize) {
        if let Some(f) = &mut self.jobs.form {
            f.focus = k.min(f.slots().saturating_sub(1));
        }
        self.reveal_form_focus();
    }

    pub(super) fn form_a11y_flag(&mut self, k: usize, j: usize, toggle: bool) {
        self.form_a11y_focus(k);
        if let Some(Control::Flags { items, cur, .. }) = self.jobs.form.as_mut().and_then(|f| f.controls.get_mut(k)) {
            if j >= items.len() {
                return;
            }
            *cur = j;
            if toggle {
                self.form_activate(k);
            }
        }
        self.dirty = true;
    }

    pub(super) fn form_a11y_invoke(&mut self, k: usize) {
        self.form_a11y_focus(k);
        match self.jobs.form.as_ref().and_then(|f| f.controls.get(k)) {
            Some(Control::Choice { .. }) => self.turn(k, false),
            Some(Control::Text { .. }) => {}
            _ => self.form_activate(k),
        }
        self.dirty = true;
    }

    pub(super) fn form_hit(&self, x: f32, y: f32) -> Option<Hit> {
        let f = self.jobs.form.as_ref()?;
        if matches!(f.kind, Kind::Properties(_)) && f.close_rect.contains(x, y) {
            return Some(Hit::Form(f.controls.len() + if f.buttons.len() > 1 { f.buttons.len() } else { 0 }));
        }
        if let Some(b) = f.button_rects.iter().position(|r| r.contains(x, y)) {
            return Some(Hit::Form(f.controls.len() + b));
        }
        if let Some(k) = f.rows.iter().take(if matches!(f.kind, Kind::Update(_)) { 2 } else { f.rows.len() }).position(|r| { let r = f.visible_row(*r); r.h > 0.0 && r.contains(x, y) }) {
            return Some(Hit::Form(k));
        }
        if self.in_title(y) {
            return Some(self.caption_hit(x, y));
        }
        Some(if f.rect.contains(x, y) { Hit::Dialog } else { Hit::Modal })
    }

    pub(super) fn layout_form(&mut self) {
        let (w, h) = self.size;
        let sc = form_scale(w, h, self.scale);
        let s = |v: f32| (v * sc).round();
        let Some(f) = &mut self.jobs.form else { return };
        if matches!(f.kind, Kind::Update(_)) {
            f.layout_update(w, h, sc);
            return;
        }
        let fw = s(WIDTH).min((w - s(24.0)).max(1.0));
        let gaps = section_gaps(&f.controls);
        let body: f32 = f.controls.iter().map(|c| s(c.height())).sum::<f32>() + gaps.iter().filter(|&&g| g).count() as f32 * s(SECTION_GAP);
        let note = if f.note.is_empty() && f.progress.is_none() && !matches!(f.kind, Kind::Properties(_)) { s(8.0) } else { s(NOTE_H) };
        let footer = if matches!(f.kind, Kind::Properties(_)) && f.buttons.len() == 1 { s(12.0) } else { s(BUTTON_GAP) + s(BUTTON_H) + s(20.0) };
        let fh = (s(HEAD) + body + note + footer).min((h - s(24.0)).max(1.0));
        let x = ((w - fw) / 2.0).round();
        let y = ((h - fh) / 2.0).round();
        f.rect = Rect::new(x, y, fw, fh);
        f.document.scale = sc;
        f.document.height = body;
        f.document.viewport = Rect::new(x + s(20.0), y + s(HEAD), (fw - s(40.0)).max(0.0), (fh - s(HEAD) - note - footer).max(0.0));
        f.document.scroll.clamp(f.document.max());
        f.close_rect = if matches!(f.kind, Kind::Properties(_)) { Rect::new(f.rect.right() - s(44.0), y + s(12.0), s(32.0), s(32.0)) } else { Rect::default() };
        let mut ry = y + s(HEAD);
        f.rows = f
            .controls
            .iter()
            .zip(&gaps)
            .map(|(c, &gap)| {
                if gap {
                    ry += s(SECTION_GAP);
                }
                let r = Rect::new(x + s(20.0), ry, (fw - s(40.0) - if f.document.max() > 0.0 { s(14.0) } else { 0.0 }).max(0.0), s(c.height()));
                ry += r.h;
                r
            })
            .collect();
        let n = f.buttons.len();
        let gap = s(8.0);
        let room = (fw - s(40.0) - gap * (n as f32 - 1.0)) / n.max(1) as f32;
        let top = f.rect.bottom() - s(20.0) - s(BUTTON_H);
        let mut right = f.rect.right() - s(20.0);
        let mut rects: Vec<Rect> = f
            .buttons
            .iter()
            .rev()
            .map(|b| {
                let bw = s(b.chars().map(|c| if c >= '\u{2E80}' { 13.0 } else { 7.5 }).sum::<f32>() + 32.0).max(s(BUTTON_W)).min(room);
                right -= bw;
                let r = Rect::new(right, top, bw, s(BUTTON_H));
                right -= gap;
                r
            })
            .collect();
        rects.reverse();
        f.button_rects = rects;
        if matches!(f.kind, Kind::Properties(_)) && f.buttons.len() == 1 {
            f.button_rects[0] = f.close_rect;
        }
    }

    pub(super) fn draw_form<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme) {
        if self.draw_update_dialog(g, t) {
            return;
        }
        let sc = form_scale(self.size.0, self.size.1, self.scale);
        let (font, small, bold) = (FontStyle::new(FontFamily::Ui, 13.0 * sc, false), FontStyle::new(FontFamily::Ui, 12.0 * sc, false), FontStyle::new(FontFamily::Ui, 16.0 * sc, true));
        let icons = FontStyle::new(FontFamily::Icons, 10.0 * sc, false);
        let s = |v: f32| (v * sc).round();
        let (s4, s8, s20) = (s(4.0), s(8.0), s(20.0));
        let (hover, active, pressed) = (self.hover, self.active, self.pressed);
        let Some(f) = &mut self.jobs.form else { return };
        let d = f.rect;
        g.dl.round_rect(d, t.bg, s8);
        g.dl.outline(d, t.popup_border, s8, 1.0);
        let mb = g.metrics(bold);
        let properties = matches!(f.kind, Kind::Properties(_));
        g.text(d.x + s20, (d.y + s20 + mb.ascent).round(), &f.title, bold, t.text, d.w - if properties { s(72.0) } else { 2.0 * s20 });
        let mf = g.metrics(font);
        let mid = |r: Rect| (r.y + (r.h + mf.ascent - mf.descent) / 2.0).round();
        let n = f.controls.len();
        g.dl.set_clip(Some(f.document.viewport));
        for k in 0..n {
            let mut row = f.rows[k];
            row.y -= f.document.scroll.pos;
            if row.bottom() <= f.document.viewport.y || row.y >= f.document.viewport.bottom() { continue; }
            let focused = f.focus == k;
            let label_w = s(LABEL_W).min(row.w * 0.42);
            let lx = row.x + label_w;
            let area = Rect::new(lx, row.y + s(3.0), row.right() - lx, row.h - s(6.0));
            match &mut f.controls[k] {
                Control::Text { label, field, placeholder } => {
                    g.text(row.x, mid(row), label, font, t.text, label_w - s8);
                    g.dl.round_rect(area, t.input_bg, s4);
                    g.dl.outline(area, if focused { t.accent } else { t.input_border }, s4, if focused { sc.round().max(1.0) } else { 1.0 });
                    let inner = Rect::new(area.x + s8, area.y, area.w - 2.0 * s8, area.h);
                    field.draw(g, t, inner, font, focused && active, placeholder, sc, Some(&self.preedit));
                }
                Control::Choice { label, options, sel } => {
                    g.text(row.x, mid(row), label, font, t.text, label_w - s8);
                    g.dl.round_rect(area, if hover == Hit::Form(k) { t.button_hover } else { t.input_bg }, s4);
                    g.dl.outline(area, if focused { t.accent } else { t.input_border }, s4, if focused { sc.round().max(1.0) } else { 1.0 });
                    let text = options.get(*sel).map_or("", String::as_str);
                    g.text(area.x + s8, mid(area), text, font, t.text, area.w - s(60.0));
                    let arrow = |dx: f32| Rect::new(area.right() - dx, area.y, s(24.0), area.h);
                    centered(g, arrow(s(52.0)), G_LEFT, icons, t.text_dim);
                    centered(g, arrow(s(28.0)), G_RIGHT, icons, t.text_dim);
                }
                Control::Check { label, on } => {
                    let bx = Rect::new(lx, row.y + (row.h - s(18.0)) / 2.0, s(18.0), s(18.0));
                    draw_box(g, t, bx, *on, focused || hover == Hit::Form(k), s4, sc);
                    g.text(bx.right() + s8, mid(row), label, font, t.text, row.right() - bx.right() - s8);
                }
                Control::Flags { label, items, cur, spots } => {
                    let line_h = s(ROW);
                    g.text(row.x, mid(Rect::new(row.x, row.y, row.w, line_h)), label, font, t.text, label_w - s8);
                    let cols = flag_grid(items.len()).1;
                    let cw = ((row.right() - lx) / cols as f32).floor();
                    spots.clear();
                    for (i, (name, on)) in items.iter().enumerate() {
                        let cell = Rect::new(lx + (i % cols) as f32 * cw, row.y + (i / cols) as f32 * line_h, cw, line_h);
                        let room = (cw - s(27.0)).max(0.0);
                        let tw = g.measure(name, small).min(room);
                        let spot = Rect::new(cell.x, cell.y + s(4.0), s(22.0) + tw + s(6.0), cell.h - s(8.0));
                        let bx = Rect::new(cell.x, cell.y + (cell.h - s(16.0)) / 2.0, s(16.0), s(16.0));
                        draw_box(g, t, bx, *on, focused && i == *cur, s4, sc);
                        g.text(bx.right() + s(5.0), mid(cell), name, small, t.text, room);
                        if focused && i == *cur {
                            g.dl.rect(Rect::new(bx.right() + s(5.0), spot.bottom() - s(2.0), tw, s(1.0).max(1.0)), t.accent);
                        }
                        spots.push(spot);
                    }
                }
                Control::Line(text) => {
                    let ms = g.metrics(small);
                    let base = (row.y + (row.h + ms.ascent - ms.descent) / 2.0).round();
                    g.text(row.x + s8, base, text, small, t.text, row.w - s8);
                }
                Control::Value { label, value } => {
                    if hover == Hit::Form(k) || focused {
                        g.dl.round_rect(row, if focused { t.selection } else { t.hover }, s4);
                    }
                    let ms = g.metrics(small);
                    let base = (row.y + (row.h + ms.ascent - ms.descent) / 2.0).round();
                    g.text(row.x + s8, base, label, small, t.text_dim, label_w - s(16.0));
                    g.text(lx, base, value, small, t.text, row.right() - lx - s8);
                }
            }
        }
        if f.document.max() > 0.0 {
            g.dl.round_rect(f.document.thumb(), if f.document.grab.is_some() { t.scrollbar_hover } else { t.scrollbar }, s(3.0));
        }
        g.dl.set_clip(None);
        let note_y = f.document.viewport.bottom() + s(6.0);
        if let Some(p) = f.progress {
            let bar = Rect::new(d.x + s20, note_y, d.w - 2.0 * s20, s4);
            g.dl.round_rect(bar, t.separator, s(2.0));
            g.dl.round_rect(Rect::new(bar.x, bar.y, (bar.w * p.clamp(0.0, 1.0)).max(s4), bar.h), t.accent, s(2.0));
        }
        if !f.note.is_empty() {
            let base = (note_y + s(6.0) + mf.ascent).round();
            g.text(d.x + s20, base, &f.note, small, if f.error { t.error } else { t.text_dim }, d.w - 2.0 * s20);
        }
        for (b, (r, label)) in f.button_rects.iter().zip(&f.buttons).enumerate() {
            if properties && f.buttons.len() == 1 { continue; }
            let primary = f.focus == n + b || (f.focus < n && b == 0);
            let hot = hover == Hit::Form(n + b) || pressed == Hit::Form(n + b);
            if primary {
                g.dl.round_rect(*r, if hot { t.accent.alpha(0xD0) } else { t.accent }, s4);
            } else {
                g.dl.round_rect(*r, if hot { t.button_pressed } else { t.button_hover }, s4);
                g.dl.outline(*r, t.input_border, s4, 1.0);
            }
            let lw = g.measure(label, font);
            g.text((r.x + (r.w - lw) / 2.0).round(), mid(*r), label, font, if primary { t.bg } else { t.text }, r.w);
        }
        if properties {
            let k = n + if f.buttons.len() > 1 { f.buttons.len() } else { 0 };
            if hover == Hit::Form(k) || pressed == Hit::Form(k) || f.focus == k {
                g.dl.round_rect(f.close_rect, t.button_hover, s4);
            }
            if f.focus == k { g.dl.outline(f.close_rect, t.accent, s4, sc.max(1.0)); }
            centered(g, f.close_rect, super::G_CLOSE, icons, t.text);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_box<B: TextBackend>(g: &mut Gfx<B>, t: &Theme, bx: Rect, on: bool, hot: bool, r: f32, sc: f32) {
    if on {
        g.dl.round_rect(bx, t.accent, r);
        g.dl.check(bx, crate::theme::ink_on(t.accent));
    } else {
        g.dl.outline(bx, if hot { t.accent } else { t.text_dim }, r, sc.round().max(1.0));
    }
}
