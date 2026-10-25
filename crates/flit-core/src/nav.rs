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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn back_forward() {
        let p = |s| Location::parse(s).unwrap();
        let mut h = History::new(p(r"C:\"));
        assert!(h.go(p(r"C:\a")));
        assert!(h.go(p("wsl://Ubuntu/home")));
        assert!(!h.go(p(r"\\wsl$\Ubuntu\home")), "same location in another spelling");
        assert_eq!(h.back().unwrap(), &p(r"C:\a"));
        assert_eq!(h.back().unwrap(), &p(r"C:\"));
        assert!(h.back().is_none());
        assert_eq!(h.forward().unwrap(), &p(r"C:\a"));
        h.go(p(r"D:\"));
        assert!(!h.can_forward());
        assert_eq!(h.back().unwrap(), &p(r"C:\a"));
    }
}
