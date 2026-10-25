use crate::location::Location;

#[derive(Debug, Clone)]
pub struct History {
    back: Vec<Location>,
    fwd: Vec<Location>,
    cur: Location,
}

impl History {
    pub fn new(start: Location) -> Self {
        Self { back: Vec::new(), fwd: Vec::new(), cur: start }
    }

    pub fn current(&self) -> &Location {
        &self.cur
    }

    pub fn go(&mut self, to: Location) -> bool {
        if to == self.cur {
            return false;
        }
        self.back.push(std::mem::replace(&mut self.cur, to));
        self.fwd.clear();
        true
    }

    pub fn can_back(&self) -> bool {
        !self.back.is_empty()
    }

    pub fn can_forward(&self) -> bool {
        !self.fwd.is_empty()
    }

    pub fn back(&mut self) -> Option<&Location> {
        let prev = self.back.pop()?;
        self.fwd.push(std::mem::replace(&mut self.cur, prev));
        Some(&self.cur)
    }

    pub fn forward(&mut self) -> Option<&Location> {
        let next = self.fwd.pop()?;
        self.back.push(std::mem::replace(&mut self.cur, next));
        Some(&self.cur)
    }

    pub fn back_list(&self) -> &[Location] {
        &self.back
    }

    pub fn forward_list(&self) -> &[Location] {
        &self.fwd
    }

    pub fn jump(&mut self, n: isize) -> bool {
        let room = if n < 0 { self.back.len() } else { self.fwd.len() };
        if n == 0 || n.unsigned_abs() > room {
            return false;
        }
        for _ in 0..n.unsigned_abs() {
            if n < 0 { self.back() } else { self.forward() };
        }
        true
    }

    pub fn replace(&mut self, to: Location) {
        self.cur = to;
    }

    pub fn retract(&mut self) -> bool {
        let Some(prev) = self.back.pop() else { return false };
        self.cur = prev;
        true
    }
}

#[cfg(test)]
#[path = "../test/nav.rs"]
mod tests;
