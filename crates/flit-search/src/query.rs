use crate::index::lowercase;
use regex::bytes::{Regex, RegexBuilder};

const TICKS_PER_SEC: i64 = 10_000_000;
const UNIX_EPOCH_SECS: i64 = 11_644_473_600;
const DAY: i64 = 86_400;
const REGEX_SIZE_LIMIT: usize = 1 << 22;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Scope {
    Name,
    Path,
}

#[derive(Debug, Clone)]
pub enum Pattern {
    Text(Vec<u8>),
    Glob(Vec<u8>),
    Regex(Regex),
}

#[derive(Debug, Clone)]
pub enum Filter {
    Match(Scope, Pattern),
    Ext(Vec<Vec<u8>>),
    Size(u64, u64),
    Modified(i64, i64),
}

#[derive(Debug, Clone)]
pub struct Term {
    pub negate: bool,
    pub filter: Filter,
}

#[derive(Debug, Clone, Default)]
pub struct Query {
    pub terms: Vec<Term>,
}

#[derive(Clone, Copy, Debug)]
pub struct Clock {
    pub now: i64,
    pub utc_offset_min: i32,
}

impl Clock {
    pub fn system(utc_offset_min: i32) -> Self {
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
        Self { now: (secs + UNIX_EPOCH_SECS) * TICKS_PER_SEC, utc_offset_min }
    }

    fn today(&self) -> i64 {
        (self.now / TICKS_PER_SEC - UNIX_EPOCH_SECS + self.utc_offset_min as i64 * 60).div_euclid(DAY)
    }

    fn day_start(&self, unix_day: i64) -> i64 {
        (unix_day * DAY - self.utc_offset_min as i64 * 60 + UNIX_EPOCH_SECS) * TICKS_PER_SEC
    }
}

impl Term {
    pub fn literal(&self) -> Option<&[u8]> {
        if self.negate {
            return None;
        }
        match &self.filter {
            Filter::Match(Scope::Name, Pattern::Text(t)) => Some(t),
            Filter::Match(Scope::Name, Pattern::Glob(g)) => g.split(|&b| b == b'*' || b == b'?').max_by_key(|s| s.len()).filter(|s| !s.is_empty()),
            _ => None,
        }
    }
}

impl Query {
    pub fn driver(&self) -> Option<(usize, &[u8])> {
        self.terms.iter().enumerate().filter_map(|(k, t)| t.literal().map(|l| (k, l))).max_by_key(|(_, l)| l.len())
    }

    pub fn needs_path(&self) -> bool {
        self.terms.iter().any(|t| matches!(t.filter, Filter::Match(Scope::Path, _)))
    }
}

fn tokens(text: &str) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    let (mut cur, mut quoted, mut literal) = (String::new(), false, false);
    for c in text.chars() {
        match c {
            '"' => {
                literal |= cur.is_empty() && !quoted;
                quoted = !quoted;
            }
            c if c.is_whitespace() && !quoted => {
                if !cur.is_empty() {
                    out.push((std::mem::take(&mut cur), literal));
                }
                literal = false;
            }
            c if c.is_control() => {}
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push((cur, literal));
    }
    out
}

fn strip_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let head = s.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix).then(|| &s[prefix.len()..])
}

pub fn parse(text: &str, clock: Clock) -> Result<Query, String> {
    let mut terms = Vec::new();
    for (tok, literal) in tokens(text) {
        if let Some(t) = term(&tok, literal, clock)? {
            terms.push(t);
        }
    }
    Ok(Query { terms })
}

fn term(tok: &str, literal: bool, clock: Clock) -> Result<Option<Term>, String> {
    let (mut s, mut negate, mut scope, mut regex) = (tok, false, Scope::Name, false);
    if !literal {
        loop {
            if let Some(r) = s.strip_prefix('!') {
                negate = !negate;
                s = r;
            } else if let Some(r) = strip_ci(s, "path:") {
                scope = Scope::Path;
                s = r;
            } else if let Some(r) = strip_ci(s, "regex:") {
                regex = true;
                s = r;
            } else {
                break;
            }
        }
        if !regex {
            let filter = if let Some(v) = strip_ci(s, "ext:") {
                Some(Filter::Ext(v.split(';').map(|e| lowercase(e.trim_start_matches('.').as_bytes())).collect()))
            } else if let Some(v) = strip_ci(s, "size:") {
                let (lo, hi) = interval(v, &|x| size_value(x).map(|n| (n as i128, n as i128 + 1))).ok_or_else(|| format!("invalid size: {v}"))?;
                Some(Filter::Size(lo.clamp(0, u64::MAX as i128) as u64, hi.clamp(0, u64::MAX as i128) as u64))
            } else if let Some(v) = strip_ci(s, "dm:") {
                let (lo, hi) = interval(v, &|x| date_value(x, clock)).ok_or_else(|| format!("invalid date: {v}"))?;
                Some(Filter::Modified(lo.clamp(i64::MIN as i128, i64::MAX as i128) as i64, hi.clamp(i64::MIN as i128, i64::MAX as i128) as i64))
            } else {
                None
            };
            if let Some(filter) = filter {
                return Ok(Some(Term { negate, filter }));
            }
        }
    }
    if s.is_empty() {
        return Ok(None);
    }
    let pattern = if regex {
        let re = RegexBuilder::new(s).case_insensitive(true).size_limit(REGEX_SIZE_LIMIT).build().map_err(|e| format!("invalid regex: {e}"))?;
        Pattern::Regex(re)
    } else {
        if s.contains('\\') {
            scope = Scope::Path;
        }
        let l = lowercase(s.as_bytes());
        if s.contains(['*', '?']) && !literal {
            Pattern::Glob(l)
        } else {
            Pattern::Text(l)
        }
    };
    Ok(Some(Term { negate, filter: Filter::Match(scope, pattern) }))
}

fn interval(v: &str, value: &dyn Fn(&str) -> Option<(i128, i128)>) -> Option<(i128, i128)> {
    let (open_lo, open_hi) = (i128::MIN, i128::MAX);
    if let Some(x) = v.strip_prefix(">=") {
        return Some((value(x)?.0, open_hi));
    }
    if let Some(x) = v.strip_prefix("<=") {
        return Some((open_lo, value(x)?.1));
    }
    if let Some(x) = v.strip_prefix('>') {
        return Some((value(x)?.1, open_hi));
    }
    if let Some(x) = v.strip_prefix('<') {
        return Some((open_lo, value(x)?.0));
    }
    let v = v.strip_prefix('=').unwrap_or(v);
    if let Some((a, b)) = v.split_once("..") {
        let lo = if a.is_empty() { open_lo } else { value(a)?.0 };
        let hi = if b.is_empty() { open_hi } else { value(b)?.1 };
        return Some((lo, hi));
    }
    value(v)
}

fn size_value(x: &str) -> Option<u64> {
    let x = x.trim().to_ascii_lowercase();
    let split = x.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(x.len());
    let (num, unit) = x.split_at(split);
    let mult: u64 = match unit {
        "" | "b" => 1,
        "k" | "kb" => 1 << 10,
        "m" | "mb" => 1 << 20,
        "g" | "gb" => 1 << 30,
        "t" | "tb" => 1 << 40,
        _ => return None,
    };
    let n: f64 = num.parse().ok().filter(|n: &f64| n.is_finite() && *n >= 0.0)?;
    Some((n * mult as f64).round().min(u64::MAX as f64) as u64)
}

pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn date_value(x: &str, clock: Clock) -> Option<(i128, i128)> {
    let x = x.trim().to_ascii_lowercase();
    let today = clock.today();
    let (first, last) = match x.as_str() {
        "today" => (today, today + 1),
        "yesterday" => (today - 1, today),
        _ => {
            let parts: Vec<&str> = x.split(['-', '/']).collect();
            let num = |s: &str| s.parse::<i64>().ok().filter(|_| !s.is_empty() && s.len() <= 4);
            match parts.as_slice() {
                [y] if y.len() == 4 => {
                    let y = num(y)?;
                    (days_from_civil(y, 1, 1), days_from_civil(y + 1, 1, 1))
                }
                [y, m] if y.len() == 4 => {
                    let (y, m) = (num(y)?, num(m)?);
                    if !(1..=12).contains(&m) {
                        return None;
                    }
                    let (ny, nm) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
                    (days_from_civil(y, m as u32, 1), days_from_civil(ny, nm as u32, 1))
                }
                [y, m, d] if y.len() == 4 => {
                    let (y, m, d) = (num(y)?, num(m)?, num(d)?);
                    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
                        return None;
                    }
                    let start = days_from_civil(y, m as u32, d as u32);
                    (start, start + 1)
                }
                _ => return None,
            }
        }
    };
    Some((clock.day_start(first) as i128, clock.day_start(last) as i128))
}

#[inline]
fn char_len(s: &[u8], at: usize) -> usize {
    let n = match s[at] {
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xFF => 4,
        _ => 1,
    };
    n.min(s.len() - at)
}

pub fn glob(p: &[u8], s: &[u8]) -> bool {
    let (mut pi, mut si) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while si < s.len() {
        match p.get(pi) {
            Some(b'*') => {
                star = Some((pi, si));
                pi += 1;
                continue;
            }
            Some(b'?') => {
                si += char_len(s, si);
                pi += 1;
                continue;
            }
            Some(&c) if c == s[si] => {
                pi += 1;
                si += 1;
                continue;
            }
            _ => {}
        }
        match star {
            Some((sp, ss)) => {
                let next = ss + char_len(s, ss);
                star = Some((sp, next));
                pi = sp + 1;
                si = next;
            }
            None => return false,
        }
    }
    p[pi..].iter().all(|&c| c == b'*')
}

#[cfg(test)]
#[path = "../test/query.rs"]
mod tests;
