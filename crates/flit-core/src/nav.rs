use crate::location::Location;

pub const MAX_RESTORED_NAMES: usize = 1000;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Restore {
    pub select: Vec<Vec<u8>>,
    pub focus: Option<Vec<u8>>,
    pub scroll: f32,
    pub scroll_target: f32,
}

impl Restore {
    pub fn capped(mut select: Vec<Vec<u8>>, focus: Option<Vec<u8>>, scroll: f32, scroll_target: f32) -> Self {
        select.truncate(MAX_RESTORED_NAMES);
        Self { select, focus, scroll, scroll_target }
    }

    pub fn has_names(&self) -> bool {
        !self.select.is_empty() || self.focus.is_some()
    }
}

#[derive(Debug, Clone)]
pub struct History {
    back: Vec<Location>,
    back_views: Vec<Option<Restore>>,
    fwd: Vec<Location>,
    fwd_views: Vec<Option<Restore>>,
    cur: Location,
    here: Option<Restore>,
}

impl History {
    pub fn new(start: Location) -> Self {
        Self { back: Vec::new(), back_views: Vec::new(), fwd: Vec::new(), fwd_views: Vec::new(), cur: start, here: None }
    }

    pub fn current(&self) -> &Location {
        &self.cur
    }

    pub fn go(&mut self, to: Location, keep: Option<Restore>) -> bool {
        if to == self.cur {
            return false;
        }
        let from = std::mem::replace(&mut self.cur, to);
        self.back.push(from);
        self.back_views.push(keep);
        self.fwd.clear();
        self.fwd_views.clear();
        self.here = None;
        true
    }

    pub fn can_back(&self) -> bool {
        !self.back.is_empty()
    }

    pub fn can_forward(&self) -> bool {
        !self.fwd.is_empty()
    }

    pub fn back(&mut self, keep: Option<Restore>) -> Option<&Location> {
        let prev = self.back.pop()?;
        let saved = self.back_views.pop().unwrap();
        let from = std::mem::replace(&mut self.cur, prev);
        self.fwd.push(from);
        self.fwd_views.push(keep);
        self.here = saved;
        Some(&self.cur)
    }

    pub fn forward(&mut self, keep: Option<Restore>) -> Option<&Location> {
        let next = self.fwd.pop()?;
        let saved = self.fwd_views.pop().unwrap();
        let from = std::mem::replace(&mut self.cur, next);
        self.back.push(from);
        self.back_views.push(keep);
        self.here = saved;
        Some(&self.cur)
    }

    pub fn take_here(&mut self) -> Option<Restore> {
        self.here.take()
    }

    pub fn back_list(&self) -> &[Location] {
        &self.back
    }

    pub fn forward_list(&self) -> &[Location] {
        &self.fwd
    }

    pub fn jump(&mut self, n: isize, keep: Option<Restore>) -> bool {
        let room = if n < 0 { self.back.len() } else { self.fwd.len() };
        if n == 0 || n.unsigned_abs() > room {
            return false;
        }
        let mut first = keep;
        for step in 0..n.unsigned_abs() {
            let hop = if step == 0 { first.take() } else { None };
            if n < 0 { self.back(hop); } else { self.forward(hop); };
        }
        true
    }

    pub fn replace(&mut self, to: Location) {
        self.cur = to;
        self.here = None;
    }

    pub fn retract(&mut self) -> bool {
        let Some(prev) = self.back.pop() else { return false };
        self.back_views.pop();
        self.cur = prev;
        self.here = None;
        true
    }
}

#[cfg(test)]
#[path = "../test/nav.rs"]
mod tests;
