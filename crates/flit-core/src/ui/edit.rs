use crate::input::{Key, Mods};
use std::ops::Range;

#[derive(Default, Clone, Debug)]
pub struct LineEdit {
    pub text: String,
    pub caret: usize,
    pub anchor: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub enum EditAction {
    None,
    Changed,
    Copy(String),
    Paste,
    Submit,
    Cancel,
}

fn is_sep(c: char) -> bool {
    c.is_whitespace() || matches!(c, '\\' | '/' | '.' | '-' | '_')
}

impl LineEdit {
    pub fn set(&mut self, s: &str) {
        self.text.clear();
        self.text.push_str(s);
        self.caret = s.len();
        self.anchor = s.len();
    }

    pub fn select_all(&mut self) {
        self.anchor = 0;
        self.caret = self.text.len();
    }

    pub fn selection(&self) -> Range<usize> {
        self.caret.min(self.anchor)..self.caret.max(self.anchor)
    }

    fn take_selection(&mut self) -> bool {
        let r = self.selection();
        if r.is_empty() {
            return false;
        }
        self.text.replace_range(r.clone(), "");
        self.caret = r.start;
        self.anchor = r.start;
        true
    }

    pub fn insert(&mut self, s: &str) {
        self.take_selection();
        let s: String = s.chars().filter(|c| !c.is_control()).collect();
        self.text.insert_str(self.caret, &s);
        self.caret += s.len();
        self.anchor = self.caret;
    }

    fn prev(&self, pos: usize) -> usize {
        self.text[..pos].char_indices().next_back().map_or(0, |(i, _)| i)
    }

    fn next(&self, pos: usize) -> usize {
        self.text[pos..].chars().next().map_or(pos, |c| pos + c.len_utf8())
    }

    fn word_left(&self, pos: usize) -> usize {
        let mut it = self.text[..pos].char_indices().rev().peekable();
        while it.next_if(|&(_, c)| is_sep(c)).is_some() {}
        let mut start = it.peek().map_or(0, |&(i, _)| i);
        for (i, c) in it {
            if is_sep(c) {
                break;
            }
            start = i;
        }
        start
    }

    fn word_right(&self, pos: usize) -> usize {
        let mut it = self.text[pos..].char_indices().peekable();
        while it.next_if(|&(_, c)| !is_sep(c)).is_some() {}
        while it.next_if(|&(_, c)| is_sep(c)).is_some() {}
        it.peek().map_or(self.text.len(), |&(i, _)| pos + i)
    }

    pub fn move_to(&mut self, pos: usize, extend: bool) {
        self.caret = (0..=pos.min(self.text.len())).rev().find(|&p| self.text.is_char_boundary(p)).unwrap_or(0);
        if !extend {
            self.anchor = self.caret;
        }
    }

    pub fn key(&mut self, key: Key, m: Mods) -> EditAction {
        let c = self.caret;
        let shortcut = m.ctrl && !m.alt;
        match key {
            Key::Left => {
                let to = if !m.shift && !self.selection().is_empty() {
                    self.selection().start
                } else if m.ctrl {
                    self.word_left(c)
                } else {
                    self.prev(c)
                };
                self.move_to(to, m.shift);
            }
            Key::Right => {
                let to = if !m.shift && !self.selection().is_empty() {
                    self.selection().end
                } else if m.ctrl {
                    self.word_right(c)
                } else {
                    self.next(c)
                };
                self.move_to(to, m.shift);
            }
            Key::Home => self.move_to(0, m.shift),
            Key::End => self.move_to(self.text.len(), m.shift),
            Key::Backspace => {
                if !self.take_selection() && c > 0 {
                    let from = if m.ctrl { self.word_left(c) } else { self.prev(c) };
                    self.text.replace_range(from..c, "");
                    self.move_to(from, false);
                }
                return EditAction::Changed;
            }
            Key::Delete => {
                if !self.take_selection() && c < self.text.len() {
                    let to = if m.ctrl { self.word_right(c) } else { self.next(c) };
                    self.text.replace_range(c..to, "");
                }
                return EditAction::Changed;
            }
            Key::Char('A') if shortcut => self.select_all(),
            Key::Char('C') if shortcut => return EditAction::Copy(self.text[self.selection()].to_owned()),
            Key::Char('X') if shortcut => {
                let s = self.text[self.selection()].to_owned();
                self.take_selection();
                return EditAction::Copy(s);
            }
            Key::Char('V') if shortcut => return EditAction::Paste,
            Key::Enter => return EditAction::Submit,
            Key::Escape => return EditAction::Cancel,
            _ => {}
        }
        EditAction::None
    }
}

#[cfg(test)]
#[path = "../../test/ui/edit.rs"]
mod tests;
