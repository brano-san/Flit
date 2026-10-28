use std::ops::Range;

const HIT: i32 = 16;
const RUN: i32 = 24;
const WORD: i32 = 20;
const FIRST: i32 = 12;
const GAP: i32 = 1;
const LEAD: i32 = 1;
const MAX_LEN: usize = 1024;
const NONE: i32 = i32::MIN / 4;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Match {
    pub score: i32,
    pub ranges: Vec<Range<usize>>,
}

fn word_start(prev: Option<char>, c: char) -> bool {
    match prev {
        None => true,
        Some(p) => !p.is_alphanumeric() || (p.is_lowercase() && c.is_uppercase()) || (p.is_alphabetic() != c.is_alphabetic()),
    }
}

fn fold(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

pub fn fuzzy(query: &str, text: &str) -> Option<Match> {
    let q: Vec<char> = query.chars().filter(|c| !c.is_whitespace()).map(fold).collect();
    if q.is_empty() {
        return Some(Match { score: 0, ranges: Vec::new() });
    }
    let chars: Vec<(usize, char)> = text.char_indices().take(MAX_LEN).collect();
    let n = chars.len();
    if q.len() > n {
        return None;
    }
    let lower: Vec<char> = chars.iter().map(|&(_, c)| fold(c)).collect();
    let bonus: Vec<i32> = (0..n).map(|j| if word_start(j.checked_sub(1).map(|p| chars[p].1), chars[j].1) { WORD } else { 0 }).collect();
    let m = q.len();
    let mut score = vec![NONE; m * n];
    let mut from = vec![usize::MAX; m * n];
    for (i, &qc) in q.iter().enumerate() {
        let (mut best, mut best_at) = (NONE, usize::MAX);
        for j in 0..n {
            if i > 0 && j >= 2 {
                let prev = score[(i - 1) * n + j - 2];
                if prev > NONE && prev - GAP > best - GAP {
                    best_at = j - 2;
                }
                best = (best - GAP).max(prev - GAP);
            } else if i > 0 {
                best = (best - GAP).max(NONE);
            }
            if lower[j] != qc {
                continue;
            }
            let own = HIT + bonus[j];
            if i == 0 {
                score[j] = own + if j == 0 { FIRST } else { -(LEAD * j as i32).min(HIT) };
                continue;
            }
            let run = if j > 0 { score[(i - 1) * n + j - 1] } else { NONE };
            let (via_run, via_gap) = (if run > NONE { run + RUN } else { NONE }, best);
            if via_run == NONE && via_gap <= NONE {
                continue;
            }
            let k = i * n + j;
            if via_run >= via_gap {
                (score[k], from[k]) = (via_run + own, j - 1);
            } else {
                (score[k], from[k]) = (via_gap + own, best_at);
            }
        }
    }
    let last = (m - 1) * n;
    let (end, total) = (0..n).map(|j| (j, score[last + j])).filter(|&(_, s)| s > NONE).max_by_key(|&(j, s)| (s, std::cmp::Reverse(j)))?;
    let mut at = vec![0; m];
    let mut j = end;
    for i in (0..m).rev() {
        at[i] = j;
        j = from[i * n + j];
    }
    let mut ranges: Vec<Range<usize>> = Vec::new();
    for &j in &at {
        let (start, c) = chars[j];
        let r = start..start + c.len_utf8();
        match ranges.last_mut() {
            Some(prev) if prev.end == r.start => prev.end = r.end,
            _ => ranges.push(r),
        }
    }
    Some(Match { score: total, ranges })
}

#[cfg(test)]
#[path = "../../test/ui/fuzzy.rs"]
mod tests;
