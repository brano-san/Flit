const MIN_SPREAD: f64 = 1.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Touch {
    Pass,
    Skip,
    Cancel,
    Pinch(f32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum Mode {
    #[default]
    Idle,
    One(usize),
    Two,
    Blocked,
}

#[derive(Default)]
pub(crate) struct Fingers {
    down: Vec<(usize, (f64, f64))>,
    mode: Mode,
    base: f64,
    scale: f64,
}

impl Fingers {
    pub(crate) fn press(&mut self, id: usize, at: (f64, f64)) -> Touch {
        let known = self.down.iter().any(|f| f.0 == id);
        self.down.retain(|f| f.0 != id);
        self.down.push((id, at));
        match (self.mode, self.down.len()) {
            (Mode::Idle, 1) => {
                self.mode = Mode::One(id);
                Touch::Pass
            }
            (Mode::One(_), 2) if !known => {
                self.mode = Mode::Two;
                (self.base, self.scale) = (self.spread(), 1.0);
                Touch::Cancel
            }
            (Mode::One(_), _) if known => Touch::Skip,
            _ => {
                self.mode = Mode::Blocked;
                Touch::Skip
            }
        }
    }

    pub(crate) fn moved(&mut self, id: usize, at: (f64, f64)) -> Touch {
        let Some(f) = self.down.iter_mut().find(|f| f.0 == id) else { return Touch::Skip };
        f.1 = at;
        match self.mode {
            Mode::One(first) if first == id => Touch::Pass,
            Mode::Two => self.pinch(),
            _ => Touch::Skip,
        }
    }

    pub(crate) fn release(&mut self, id: usize) -> Touch {
        if !self.forget(id) {
            return Touch::Skip;
        }
        match self.mode {
            Mode::One(first) if first == id => {
                self.mode = Mode::Idle;
                Touch::Pass
            }
            _ => {
                self.settle();
                Touch::Skip
            }
        }
    }

    pub(crate) fn lost(&mut self, id: usize) -> Touch {
        if !self.forget(id) {
            return Touch::Skip;
        }
        match self.mode {
            Mode::One(first) if first == id => {
                self.mode = Mode::Idle;
                Touch::Cancel
            }
            _ => {
                self.settle();
                Touch::Skip
            }
        }
    }

    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }

    fn forget(&mut self, id: usize) -> bool {
        let before = self.down.len();
        self.down.retain(|f| f.0 != id);
        self.down.len() != before
    }

    fn settle(&mut self) {
        self.mode = if self.down.is_empty() { Mode::Idle } else { Mode::Blocked };
    }

    fn spread(&self) -> f64 {
        match self.down.as_slice() {
            [(_, a), (_, b), ..] => (a.0 - b.0).hypot(a.1 - b.1),
            _ => 0.0,
        }
    }

    fn pinch(&mut self) -> Touch {
        let spread = self.spread();
        if !spread.is_finite() {
            return Touch::Skip;
        }
        if !(self.base.is_finite() && self.base >= MIN_SPREAD) {
            (self.base, self.scale) = (spread, 1.0);
            return Touch::Skip;
        }
        let scale = spread / self.base;
        let delta = scale - self.scale;
        self.scale = scale;
        if delta.is_finite() && delta != 0.0 {
            Touch::Pinch(delta as f32)
        } else {
            Touch::Skip
        }
    }
}

#[cfg(test)]
#[path = "../test/touch.rs"]
mod tests;
