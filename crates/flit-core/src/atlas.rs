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
#[path = "../test/atlas.rs"]
mod tests;
