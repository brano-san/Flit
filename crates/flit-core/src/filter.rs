use crate::listing::{attr, Listing};
use crate::name;
use crate::query_attrs::{self, Clock};

const PARALLEL_FROM: usize = 1 << 15;
const NAMES_PER_THREAD: usize = 1 << 14;
pub(crate) const ATTRIBUTE_KEYS: &[&str] = &["ext", "kind", "size", "modified", "created", "hidden", "tag"];
pub(crate) const KIND_VALUES: &[&str] = &["file", "folder", "image", "audio", "video", "text", "code", "other"];

#[derive(Clone, Debug, PartialEq)]
struct NamePattern {
    needle: String,
    glob: bool,
}

impl NamePattern {
    fn new(text: &str) -> Self {
        let needle: String = text.chars().flat_map(char::to_lowercase).collect();
        let glob = needle.contains(['*', '?']);
        Self { needle, glob }
    }

    fn matches(&self, name: &[u8], buf: &mut String) -> bool {
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
}

#[derive(Clone, Debug, PartialEq)]
enum Attribute {
    Ext(Vec<Vec<u8>>),
    Kind(&'static str),
    Size(u64, u64),
    Modified(i64, i64),
    Created(i64, i64),
    Hidden(bool),
    Tag(u32),
}

impl Attribute {
    fn parse(key: &str, value: &str, clock: Clock, tags: &[crate::tags::Tag]) -> Option<Self> {
        Some(match key {
            "ext" => Self::Ext(query_attrs::extensions(value)?),
            "kind" => Self::Kind(KIND_VALUES.iter().copied().find(|k| k.eq_ignore_ascii_case(value))?),
            "size" => { let (lo, hi) = query_attrs::size_range(value)?; Self::Size(lo, hi) }
            "modified" => { let (lo, hi) = query_attrs::date_range(value, clock)?; Self::Modified(lo, hi) }
            "created" => { let (lo, hi) = query_attrs::date_range(value, clock)?; Self::Created(lo, hi) }
            "hidden" => Self::Hidden(match value.to_ascii_lowercase().as_str() {
                "true" | "yes" | "1" => true,
                "false" | "no" | "0" => false,
                _ => return None,
            }),
            "tag" => Self::Tag(1u32 << crate::tags::find(tags, value)?),
            _ => return None,
        })
    }

    fn matches(&self, listing: &Listing, i: u32) -> bool {
        use crate::preview::Kind;
        let e = &listing.entries[i as usize];
        match self {
            Self::Ext(exts) => {
                let lower = std::str::from_utf8(listing.ext(i)).unwrap_or_default().to_lowercase();
                !e.is_dir() && exts.iter().any(|p| p.as_slice() == lower.as_bytes())
            }
            Self::Kind(k) => match *k {
                "file" => !e.is_dir(),
                "folder" => e.is_dir(),
                k if !e.is_dir() => {
                    let kind = match crate::preview::kind_of(listing.name(i)) {
                        Kind::Image => "image",
                        Kind::Audio => "audio",
                        Kind::Video => "video",
                        Kind::Text | Kind::Markdown | Kind::Rtf => "text",
                        Kind::Code(_) => "code",
                        _ => "other",
                    };
                    k == kind
                }
                _ => false,
            },
            Self::Size(lo, hi) => !e.is_dir() && (*lo..*hi).contains(&e.size),
            Self::Modified(lo, hi) => e.modified > 0 && (*lo..*hi).contains(&e.modified),
            Self::Created(lo, hi) => e.created > 0 && (*lo..*hi).contains(&e.created),
            Self::Hidden(hidden) => (e.attrs & attr::HIDDEN != 0) == *hidden,
            Self::Tag(mask) => e.tags & mask != 0,
        }
    }
}

#[derive(Clone, Debug)]
pub struct NameFilter {
    include: Vec<NamePattern>,
    exclude: Vec<NamePattern>,
    attributes: Vec<(Attribute, bool)>,
    invalid: bool,
    matched: Vec<bool>,
    rows: Vec<u32>,
}

pub(crate) fn token_ranges(text: &str) -> Vec<std::ops::Range<usize>> {
    let mut ranges = Vec::new();
    let mut start = None;
    let mut quoted = false;
    for (at, c) in text.char_indices() {
        if c.is_whitespace() && !quoted {
            if let Some(from) = start.take() {
                ranges.push(from..at);
            }
        } else {
            start.get_or_insert(at);
            if c == '"' {
                quoted = !quoted;
            }
        }
    }
    if let Some(from) = start {
        ranges.push(from..text.len());
    }
    ranges
}

impl NameFilter {
    pub fn new(text: &str) -> Option<Self> {
        Self::with_context(text, Clock::system(0), &[])
    }

    pub(crate) fn with_context(text: &str, clock: Clock, tags: &[crate::tags::Tag]) -> Option<Self> {
        if text.is_empty() {
            return None;
        }
        let mut patterns = String::new();
        let mut attributes = Vec::new();
        let (mut invalid, mut end) = (false, 0);
        for range in token_ranges(text) {
            let token = &text[range.clone()];
            let keyval = token.trim_start_matches('!').split_once(':');
            let Some((key, value)) = keyval.filter(|(k, _)| ATTRIBUTE_KEYS.iter().any(|a| a.eq_ignore_ascii_case(k))) else { continue };
            patterns.push_str(&text[end..range.start]);
            end = range.end;
            match Attribute::parse(&key.to_ascii_lowercase(), value.trim_matches('"'), clock, tags) {
                Some(a) => attributes.push((a, token.starts_with('!'))),
                None => invalid = true,
            }
        }
        patterns.push_str(&text[end..]);
        let mut include = Vec::new();
        let mut exclude = Vec::new();
        let masks = patterns.contains(';') || patterns.starts_with('!') || end != 0;
        for p in patterns.split(';') {
            let p = if masks { p.trim() } else { p };
            if let Some(excluded) = p.strip_prefix('!').filter(|s| !s.is_empty()) {
                exclude.push(NamePattern::new(excluded));
            } else if !p.is_empty() {
                include.push(NamePattern::new(p));
            }
        }
        Some(Self { include, exclude, attributes, invalid, matched: Vec::new(), rows: Vec::new() })
    }

    pub fn matches_name(&self, name: &[u8], buf: &mut String) -> bool {
        (self.include.is_empty() || self.include.iter().any(|p| p.matches(name, buf))) && !self.exclude.iter().any(|p| p.matches(name, buf))
    }

    fn matches_entry(&self, listing: &Listing, i: u32, buf: &mut String) -> bool {
        if self.invalid || !self.attributes.iter().all(|(a, negated)| a.matches(listing, i) != *negated) {
            return false;
        }
        let name = listing.name(i);
        let label = listing.labeled(i).map(str::as_bytes);
        let matches = |p: &NamePattern, buf: &mut String| p.matches(name, buf) || label.is_some_and(|l| p.matches(l, buf));
        (self.include.is_empty() || self.include.iter().any(|p| matches(p, buf))) && !self.exclude.iter().any(|p| matches(p, buf))
    }

    pub fn narrows(&self, prev: &NameFilter) -> bool {
        if self.invalid {
            return true;
        }
        if prev.invalid || !prev.attributes.iter().all(|a| self.attributes.contains(a)) || !prev.exclude.iter().all(|p| self.exclude.contains(p)) {
            return false;
        }
        if prev.include.is_empty() || self.include == prev.include {
            return true;
        }
        match (self.include.as_slice(), prev.include.as_slice()) {
            ([p], [previous]) if !p.glob && !previous.glob => {
                let mut hay = p.needle.chars();
                previous.needle.chars().all(|n| hay.any(|h| h == n))
            }
            _ => false,
        }
    }

    pub fn needle(&self) -> Option<&str> {
        match self.include.as_slice() {
            [p] if !p.glob && self.exclude.is_empty() && self.attributes.is_empty() && !self.invalid => Some(&p.needle),
            _ => None,
        }
    }

    pub(crate) fn refresh_entries(&mut self, listing: &Listing, touched: &[u32], relabeled: bool) {
        if !relabeled && self.attributes.is_empty() {
            return;
        }
        let mut buf = String::new();
        for &i in touched {
            if (i as usize) < self.matched.len() {
                self.matched[i as usize] = self.matches_entry(listing, i, &mut buf);
            }
        }
    }

    pub fn seed_from(&mut self, prev: &NameFilter, listing: &Listing) {
        self.matched = self.match_range(listing, 0, prev.matched.len(), &|i| prev.matched[i]);
    }

    fn match_range(&self, listing: &Listing, from: usize, to: usize, keep: &(dyn Fn(usize) -> bool + Sync)) -> Vec<bool> {
        let one = |start: usize, end: usize| {
            let mut buf = String::new();
            (start..end).map(|i| keep(i) && self.matches_entry(listing, i as u32, &mut buf)).collect::<Vec<bool>>()
        };
        let threads = if to - from < PARALLEL_FROM { 1 } else { std::thread::available_parallelism().map_or(1, |n| n.get()).min((to - from) / NAMES_PER_THREAD) };
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

pub(crate) fn glob(p: &[u8], s: &[u8]) -> bool {
    glob_match(p, s, false)
}

pub(crate) fn picker_glob(p: &[u8], s: &[u8]) -> bool {
    glob_match(p, s, true)
}

fn class_match(pattern: &[u8], value: u32) -> Option<(usize, bool)> {
    let negated = pattern.first() == Some(&b'!');
    let first = usize::from(negated);
    let close = first + pattern.get(first..)?.iter().enumerate().skip(1).find_map(|(i, &b)| (b == b']').then_some(i))?;
    let mut class = &pattern[first..close];
    let mut matched = false;
    while !class.is_empty() {
        let (start, width) = name::decode(class);
        class = &class[width..];
        if class.first() == Some(&b'-') && class.len() > 1 {
            let (end, width) = name::decode(&class[1..]);
            matched |= (start..=end).contains(&value);
            class = &class[width + 1..];
        } else {
            matched |= start == value;
        }
    }
    Some((close + 2, matched != negated))
}

fn glob_match(p: &[u8], s: &[u8], classes: bool) -> bool {
    let step = |i: usize| i + 1 + s[i + 1..].iter().take_while(|&&b| b & 0xC0 == 0x80).count();
    let (mut pi, mut si, mut star) = (0, 0, None);
    while si < s.len() {
        let advance = match p.get(pi) {
            Some(b'*') => {
                star = Some((pi, si));
                Some((1, si))
            }
            Some(b'?') => Some((1, step(si))),
            Some(b'[') if classes => {
                let value = if s[si].is_ascii() { u32::from(s[si].to_ascii_lowercase()) } else { name::decode(&s[si..]).0 };
                match class_match(&p[pi + 1..], value) {
                    Some((width, true)) => Some((width, step(si))),
                    None if s[si] == b'[' => Some((1, si + 1)),
                    _ => None,
                }
            }
            Some(&c) if c == s[si].to_ascii_lowercase() => Some((1, si + 1)),
            _ => None,
        };
        if let Some((width, next)) = advance {
            pi += width;
            si = next;
        } else if let Some((sp, ss)) = star {
            let next = step(ss);
            star = Some((sp, next));
            pi = sp + 1;
            si = next;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&c| c == b'*')
}

#[cfg(test)]
#[path = "../test/filter.rs"]
mod tests;
