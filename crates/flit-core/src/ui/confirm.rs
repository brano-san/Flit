use super::widgets::{baseline, Paint};
use super::App;
use crate::i18n::S;
use crate::input::{Event, Key, MouseButton};
use crate::location::Location;
use crate::render::Rect;
use crate::text::{Gfx, TextBackend};
use crate::theme::Theme;
use std::time::Instant;

const WIDTH: f32 = 240.0;
const PAD: f32 = 12.0;
const LINE: f32 = 22.0;
const BUTTON_H: f32 = 28.0;
const GAP: f32 = 8.0;

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Confirmed {
    Unpin(Location),
    Pin(super::MenuKind, String, bool),
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Confirm {
    pub(super) question: String,
    pub(super) yes: String,
    destructive: bool,
    then: Confirmed,
    pub anchor: Rect,
    pub rect: Rect,
    pub buttons: [Rect; 2],
    hot: Option<usize>,
    pressed: Option<usize>,
    opened: Instant,
}

impl App {
    pub(super) fn ask(&mut self, anchor: Rect, (question, yes): (&str, &str), destructive: bool, then: Confirmed, now: Instant) {
        let sc = self.scale;
        let s = |v: f32| (v * sc).round();
        let (w, h) = (s(WIDTH).min(self.size.0 - 2.0 * s(GAP)), 2.0 * s(PAD) + s(LINE) + s(GAP) + s(BUTTON_H));
        let below = anchor.bottom() + s(4.0);
        let y = if below + h <= self.size.1 - s(GAP) { below } else { (anchor.y - s(4.0) - h).max(s(GAP)) };
        let x = (anchor.x + anchor.w / 2.0 - w / 2.0).round().min(self.size.0 - w - s(GAP)).max(s(GAP));
        let rect = Rect::new(x, y.round(), w, h);
        let bw = ((w - 2.0 * s(PAD) - s(GAP)) / 2.0).floor();
        let by = rect.bottom() - s(PAD) - s(BUTTON_H);
        let buttons = [Rect::new(x + s(PAD), by, bw, s(BUTTON_H)), Rect::new(rect.right() - s(PAD) - bw, by, bw, s(BUTTON_H))];
        self.confirm = Some(Confirm { question: question.to_owned(), yes: yes.to_owned(), destructive, then, anchor, rect, buttons, hot: None, pressed: None, opened: now });
        self.tip_hide();
        self.dirty = true;
    }

    pub(super) fn confirm_event(&mut self, ev: &Event, now: Instant) -> bool {
        let Some(c) = &mut self.confirm else { return false };
        match *ev {
            Event::MouseMove { x, y } => {
                let hot = c.buttons.iter().position(|b| b.contains(x, y));
                if hot != c.hot {
                    c.hot = hot;
                    self.dirty = true;
                }
            }
            Event::MouseDown { x, y, button, .. } => {
                if !c.rect.contains(x, y) {
                    self.finish_confirm(false, now);
                } else if button == MouseButton::Left {
                    c.pressed = c.buttons.iter().position(|b| b.contains(x, y));
                }
            }
            Event::MouseUp { x, y, button: MouseButton::Left } => {
                if let Some(k) = c.pressed.take().filter(|&k| c.buttons[k].contains(x, y)) {
                    self.finish_confirm(k == 0, now);
                }
            }
            Event::Key { key: Key::Escape, pressed: true, .. } => self.finish_confirm(false, now),
            Event::Key { key: Key::Enter, pressed: true, .. } => self.finish_confirm(true, now),
            Event::MouseUp { .. } | Event::Key { .. } | Event::Char(_) | Event::Wheel { .. } | Event::MouseLeave => {}
            Event::Focus(false) | Event::Resized { .. } | Event::ScaleChanged(_) => {
                self.finish_confirm(false, now);
                return false;
            }
            _ => return false,
        }
        true
    }

    fn finish_confirm(&mut self, yes: bool, now: Instant) {
        let Some(c) = self.confirm.take() else { return };
        self.dirty = true;
        if yes {
            match c.then {
                Confirmed::Unpin(loc) => self.unpin(loc, now),
                Confirmed::Pin(kind, key, pin) => self.set_pin(kind, key, pin),
            }
        }
    }

    pub(super) fn draw_confirm<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme) {
        let Some(c) = &self.confirm else { return };
        let (font, sc) = (self.ui_font(13.0), self.scale);
        let from = g.dl.instances.len();
        let mut p = Paint::new(g, t, sc);
        p.frame(c.rect);
        let line = Rect::new(c.rect.x + PAD * sc, c.rect.y + PAD * sc, c.rect.w - 2.0 * PAD * sc, LINE * sc);
        let w = p.g.measure(&c.question, font).min(line.w);
        let y = baseline(p.g, line, font);
        p.g.text((line.x + (line.w - w) / 2.0).round(), y, &c.question, font, t.text, line.w);
        let yes_fill = if c.destructive { t.destructive } else { t.teal };
        p.button(c.buttons[0], &c.yes, font, (c.hot == Some(0)).then_some(yes_fill));
        p.button(c.buttons[1], S::WidgetsCancel.t(), font, (c.hot == Some(1)).then_some(t.teal));
        let (opened, r) = (c.opened, c.rect);
        self.pop(g, from, opened, (r.x + r.w / 2.0, r.y));
    }
}

#[cfg(test)]
#[path = "../../test/ui/confirm.rs"]
mod tests;
