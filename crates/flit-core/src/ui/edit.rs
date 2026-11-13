use crate::input::{Key, Mods};
use std::ops::Range;

#[derive(Default, Clone, Debug)]
pub struct LineEdit {
    pub text: String,
    pub caret: usize,
    pub anchor: usize,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    run: Run,
}

type Snapshot = (String, usize, usize);

const HISTORY_CAP: usize = 200;

#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
enum Run {
    #[default]
    Break,
    Typing,
    Erasing,
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
        self.undo.clear();
        self.redo.clear();
        self.run = Run::Break;
    }

    fn begin(&mut self, run: Run) {
        if run != Run::Break && self.run == run {
            return;
        }
        if self.undo.len() >= HISTORY_CAP {
            self.undo.remove(0);
        }
        self.undo.push((self.text.clone(), self.caret, self.anchor));
        self.redo.clear();
    }

    fn travel(&mut self, forward: bool) -> bool {
        let (from, to) = if forward { (&mut self.redo, &mut self.undo) } else { (&mut self.undo, &mut self.redo) };
        let Some((text, caret, anchor)) = from.pop() else { return false };
        to.push((std::mem::replace(&mut self.text, text), self.caret, self.anchor));
        self.caret = caret;
        self.anchor = anchor;
        self.run = Run::Break;
        true
    }

    pub fn undo(&mut self) -> bool {
        self.travel(false)
    }

    pub fn redo(&mut self) -> bool {
        self.travel(true)
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

    pub fn change_case(&mut self, upper: bool) -> bool {
        let r = self.selection();
        if r.is_empty() {
            return false;
        }
        let part = &self.text[r.clone()];
        let changed = if upper { part.to_uppercase() } else { part.to_lowercase() };
        if changed == part {
            return false;
        }
        let forward = self.caret >= self.anchor;
        self.begin(Run::Break);
        self.run = Run::Break;
        self.text.replace_range(r.clone(), &changed);
        let end = r.start + changed.len();
        (self.anchor, self.caret) = if forward { (r.start, end) } else { (end, r.start) };
        true
    }

    pub fn insert(&mut self, s: &str) {
        let s: String = s.chars().filter(|c| !c.is_control()).collect();
        if s.is_empty() {
            return;
        }
        let run = if self.selection().is_empty() && s.chars().count() == 1 { Run::Typing } else { Run::Break };
        self.begin(run);
        self.take_selection();
        self.text.insert_str(self.caret, &s);
        self.caret += s.len();
        self.anchor = self.caret;
        self.run = run;
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
        self.run = Run::Break;
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
                let run = if m.ctrl || !self.selection().is_empty() { Run::Break } else { Run::Erasing };
                if !self.selection().is_empty() || c > 0 {
                    self.begin(run);
                }
                if !self.take_selection() && c > 0 {
                    let from = if m.ctrl { self.word_left(c) } else { self.prev(c) };
                    self.text.replace_range(from..c, "");
                    self.move_to(from, false);
                }
                self.run = run;
                return EditAction::Changed;
            }
            Key::Delete => {
                let run = if m.ctrl || !self.selection().is_empty() { Run::Break } else { Run::Erasing };
                if !self.selection().is_empty() || c < self.text.len() {
                    self.begin(run);
                }
                if !self.take_selection() && c < self.text.len() {
                    let to = if m.ctrl { self.word_right(c) } else { self.next(c) };
                    self.text.replace_range(c..to, "");
                }
                self.run = run;
                return EditAction::Changed;
            }
            Key::Char('A') if shortcut => self.select_all(),
            Key::Char('Z' | 'Y') if shortcut => {
                let moved = if m.shift || key == Key::Char('Y') { self.redo() } else { self.undo() };
                return if moved { EditAction::Changed } else { EditAction::None };
            }
            Key::Char('C') if shortcut => return if self.selection().is_empty() { EditAction::None } else { EditAction::Copy(self.text[self.selection()].to_owned()) },
            Key::Char('X') if shortcut => {
                if self.selection().is_empty() {
                    return EditAction::None;
                }
                let s = self.text[self.selection()].to_owned();
                self.begin(Run::Break);
                self.take_selection();
                self.run = Run::Break;
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
