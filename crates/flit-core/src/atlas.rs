#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Slot {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

struct Shelf {
    y: u32,
    h: u32,
    x: u32,
}

pub struct Atlas {
    size: u32,
    shelves: Vec<Shelf>,
    next_y: u32,
}

const PAD: u32 = 1;

impl Atlas {
    pub fn new(size: u32) -> Self {
        Self { size, shelves: Vec::new(), next_y: 0 }
    }

    pub fn size(&self) -> u32 {
        self.size
    }

    pub fn clear(&mut self) {
        self.shelves.clear();
        self.next_y = 0;
    }

    pub fn alloc(&mut self, w: u32, h: u32) -> Option<Slot> {
        let (pw, ph) = (w + PAD, h + PAD);
        if pw > self.size {
            return None;
        }
        let fit = self
            .shelves
            .iter_mut()
            .filter(|s| s.h >= ph && s.h <= ph + ph / 4 + 1 && s.x + pw <= self.size)
            .min_by_key(|s| s.h);
        let shelf = match fit {
            Some(s) => s,
            None => {
                if self.next_y + ph > self.size {
                    return None;
                }
                self.shelves.push(Shelf { y: self.next_y, h: ph, x: 0 });
                self.next_y += ph;
                self.shelves.last_mut().unwrap()
            }
        };
        let slot = Slot { x: shelf.x, y: shelf.y, w, h };
        shelf.x += pw;
        Some(slot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packs_without_overlap_until_full() {
        let mut a = Atlas::new(64);
        let mut slots = Vec::new();
        while let Some(s) = a.alloc(7, 9) {
            slots.push(s);
        }
        assert_eq!(slots.len(), 8 * 6);
        for (i, p) in slots.iter().enumerate() {
            assert!(p.x + p.w <= 64 && p.y + p.h <= 64);
            for q in &slots[i + 1..] {
                let disjoint = p.x + p.w <= q.x || q.x + q.w <= p.x || p.y + p.h <= q.y || q.y + q.h <= p.y;
                assert!(disjoint, "{p:?} overlaps {q:?}");
            }
        }
        a.clear();
        assert!(a.alloc(63, 63).is_some());
    }
}
