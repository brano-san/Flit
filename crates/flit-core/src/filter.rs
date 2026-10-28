use crate::listing::Listing;
use crate::name;

const PARALLEL_FROM: usize = 1 << 15;

#[derive(Clone, Debug)]
pub struct NameFilter {
    needle: String,
    glob: bool,
    matched: Vec<bool>,
    rows: Vec<u32>,
}

impl NameFilter {
    pub fn new(text: &str) -> Option<Self> {
        let needle: String = text.chars().flat_map(char::to_lowercase).collect();
        if needle.is_empty() {
            return None;
        }
        let glob = needle.contains(['*', '?']);
        Some(Self { needle, glob, matched: Vec::new(), rows: Vec::new() })
    }

    pub fn matches_name(&self, name: &[u8], buf: &mut String) -> bool {
        if !self.glob && (self.needle.is_ascii() || name.is_ascii()) {
            return ascii_subsequence(name, self.needle.as_bytes());
        }
        if self.glob && name.is_ascii() {
            return glob(self.needle.as_bytes(), name);
        }
        buf.clear();
        for c in name::display(name).chars() {
            if c.is_ascii() {
                buf.push(c.to_ascii_lowercase());
            } else {
                buf.extend(c.to_lowercase());
            }
        }
        if self.glob {
            glob(self.needle.as_bytes(), buf.as_bytes())
        } else {
            let mut hay = buf.chars();
            self.needle.chars().all(|n| hay.any(|h| h == n))
        }
    }

    pub fn narrows(&self, prev: &NameFilter) -> bool {
        let mut hay = self.needle.chars();
        !self.glob && !prev.glob && prev.needle.chars().all(|n| hay.any(|h| h == n))
    }

    pub fn needle(&self) -> Option<&str> {
        (!self.glob).then_some(self.needle.as_str())
    }

    pub fn seed_from(&mut self, prev: &NameFilter, listing: &Listing) {
        self.matched = self.match_range(listing, 0, prev.matched.len(), &|i| prev.matched[i]);
    }

    fn match_range(&self, listing: &Listing, from: usize, to: usize, keep: &(dyn Fn(usize) -> bool + Sync)) -> Vec<bool> {
        let one = |start: usize, end: usize| {
            let mut buf = String::new();
            (start..end).map(|i| keep(i) && self.matches_name(listing.name(i as u32), &mut buf)).collect::<Vec<bool>>()
        };
        let threads = if to - from < PARALLEL_FROM { 1 } else { std::thread::available_parallelism().map_or(1, |n| n.get()) };
        if threads == 1 {
            return one(from, to);
        }
        let chunk = (to - from).div_ceil(threads);
        std::thread::scope(|s| {
            let parts: Vec<_> = (from..to).step_by(chunk).map(|start| s.spawn(move || one(start, (start + chunk).min(to)))).collect();
            parts.into_iter().flat_map(|p| p.join().unwrap_or_default()).collect()
        })
    }

    pub fn sync(&mut self, listing: &Listing, order: &[u32]) {
        if self.matched.len() > listing.len() {
            self.matched.clear();
        }
        let fresh = self.match_range(listing, self.matched.len(), listing.len(), &|_| true);
        self.matched.extend(fresh);
        let matched = &self.matched;
        self.rows.clear();
        self.rows.extend(order.iter().copied().filter(|&i| matched[i as usize]));
    }

    #[inline]
    pub fn matched(&self, i: u32) -> bool {
        self.matched.get(i as usize).copied().unwrap_or(false)
    }

    #[inline]
    pub fn rows(&self) -> &[u32] {
        &self.rows
    }
}

fn ascii_subsequence(hay: &[u8], needle: &[u8]) -> bool {
    let mut hay = hay.iter();
    needle.iter().all(|&n| hay.any(|h| h.to_ascii_lowercase() == n))
}

fn glob(p: &[u8], s: &[u8]) -> bool {
    let step = |i: usize| i + 1 + s[i + 1..].iter().take_while(|&&b| b & 0xC0 == 0x80).count();
    let (mut pi, mut si, mut star) = (0, 0, None);
    while si < s.len() {
        match p.get(pi) {
            Some(b'*') => {
                star = Some((pi, si));
                pi += 1;
            }
            Some(b'?') => {
                pi += 1;
                si = step(si);
            }
            Some(&c) if c == s[si].to_ascii_lowercase() => {
                pi += 1;
                si += 1;
            }
            _ => match star {
                Some((sp, ss)) => {
                    let next = step(ss);
                    star = Some((sp, next));
                    pi = sp + 1;
                    si = next;
                }
                None => return false,
            },
        }
    }
    p[pi..].iter().all(|&c| c == b'*')
}

#[cfg(test)]
#[path = "../test/filter.rs"]
mod tests;
