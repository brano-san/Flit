use crate::index::lowercase;
use regex::bytes::{Regex, RegexBuilder};

use flit_core::query_attrs;
#[cfg(test)]
use flit_core::query_attrs::{DAY, TICKS_PER_SEC, UNIX_EPOCH_SECS};
pub use flit_core::query_attrs::{days_from_civil, Clock, MAX_EXTS};
const REGEX_SIZE_LIMIT: usize = 1 << 22;
pub const MAX_TERMS: usize = 32;
pub const MAX_REGEXES: usize = 4;
pub const MAX_PATTERN: usize = 1024;

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
    Tag(String),
    Any(Vec<Query>),
    Directory(bool),
    Created(i64, i64),
    Accessed(i64, i64),
    Attributes(u32),
    Children(u64, u64),
    Depth(u64, u64),
    Rating(u8, u8),
    Comment(String),
    PortableTag(String),
}

#[derive(Debug, Clone)]
pub struct Term {
    pub negate: bool,
    pub filter: Filter,
}

#[derive(Debug, Clone, Default)]
pub struct Query {
    pub terms: Vec<Term>,
    pub paths: query_attrs::SearchPaths,
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
        !self.paths.is_empty() || self.uses(&|f| matches!(f, Filter::Match(Scope::Path, _)))
    }

    pub fn has_tags(&self) -> bool {
        self.uses(&|f| matches!(f, Filter::Tag(_)))
    }

    pub fn uses(&self, predicate: &dyn Fn(&Filter) -> bool) -> bool {
        self.terms.iter().any(|t| predicate(&t.filter) || matches!(&t.filter, Filter::Any(queries) if queries.iter().any(|q| q.uses(predicate))))
    }

    pub fn visit_filters(&self, visit: &mut dyn FnMut(&Filter)) {
        for t in &self.terms {
            visit(&t.filter);
            if let Filter::Any(queries) = &t.filter {
                for query in queries {
                    query.visit_filters(visit);
                }
            }
        }
    }

    pub fn has_metadata(&self) -> bool {
        self.terms.iter().any(|t| matches!(t.filter, Filter::Rating(..) | Filter::Comment(_) | Filter::PortableTag(_)))
    }

    pub fn matches(&self, e: &Plain<'_>, has_tag: &dyn Fn(&str) -> bool) -> bool {
        self.matches_with(e, Facts::default(), has_tag, &|_| false)
    }

    pub fn matches_with(&self, e: &Plain<'_>, facts: Facts, has_tag: &dyn Fn(&str) -> bool, has_metadata: &dyn Fn(&Filter) -> bool) -> bool {
        let text = |p: &Pattern, lower: &[u8], original: &[u8]| match p {
            Pattern::Text(t) => memchr::memmem::find(lower, t).is_some(),
            Pattern::Glob(g) => glob(g, lower),
            Pattern::Regex(re) => re.is_match(original),
        };
        !self.paths.rejects(e.path) && self.terms.iter().all(|t| {
            let ok = match &t.filter {
                Filter::Match(Scope::Name, p) => text(p, e.lower_name, e.name),
                Filter::Match(Scope::Path, p) => text(p, e.lower_path, e.path),
                Filter::Ext(list) => !e.dir && list.iter().any(|x| x.as_slice() == plain_ext(e.lower_name)),
                Filter::Size(lo, hi) => !e.dir && (*lo..*hi).contains(&e.size),
                Filter::Modified(lo, hi) => (*lo..*hi).contains(&e.modified),
                Filter::Tag(name) => has_tag(name),
                Filter::Any(queries) => queries.iter().any(|q| q.matches_with(e, facts, has_tag, has_metadata)),
                Filter::Directory(dir) => e.dir == *dir,
                Filter::Created(lo, hi) => facts.created > 0 && (*lo..*hi).contains(&facts.created),
                Filter::Accessed(lo, hi) => facts.accessed > 0 && (*lo..*hi).contains(&facts.accessed),
                Filter::Attributes(mask) => facts.attrs & mask == *mask,
                Filter::Children(lo, hi) => e.dir && facts.children.is_some_and(|n| (*lo..*hi).contains(&u64::from(n))),
                Filter::Depth(lo, hi) => facts.depth.is_some_and(|n| (*lo..*hi).contains(&u64::from(n))),
                filter @ (Filter::Rating(..) | Filter::Comment(_) | Filter::PortableTag(_)) => has_metadata(filter),
            };
            ok != t.negate
        })
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Facts {
    pub attrs: u32,
    pub created: i64,
    pub accessed: i64,
    pub children: Option<u32>,
    pub depth: Option<u32>,
}

impl Filter {
    pub fn matches_metadata(&self, meta: &flit_core::portable_meta::Metadata) -> bool {
        match self {
            Self::Rating(lo, hi) => (*lo..*hi).contains(&meta.rating),
            Self::Comment(needle) => meta.comment.to_lowercase().contains(needle),
            Self::PortableTag(tag) => meta.tags.iter().any(|name| name.to_lowercase() == *tag),
            _ => false,
        }
    }
}

pub struct Plain<'a> {
    pub name: &'a [u8],
    pub lower_name: &'a [u8],
    pub path: &'a [u8],
    pub lower_path: &'a [u8],
    pub size: u64,
    pub modified: i64,
    pub dir: bool,
}

fn plain_ext(lower_name: &[u8]) -> &[u8] {
    match memchr::memrchr(b'.', lower_name) {
        Some(p) if p + 1 < lower_name.len() => &lower_name[p + 1..],
        _ => &[],
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

const OWN_PROPERTIES: &[&str] = &["path", "regex", "ext", "size", "dm", "tag", "file", "folder", "dc", "da", "attrib", "empty", "child-count", "depth"];

pub fn shell_query(text: &str, windows: bool) -> Option<String> {
    let text = text.trim();
    if let Some(rest) = text.strip_prefix('$') {
        return Some(rest.trim().to_owned());
    }
    if !windows {
        return None;
    }
    let foreign = |tok: &str| {
        let tok = tok.trim_start_matches(['!', '-']);
        let Some((name, value)) = tok.split_once(':') else { return false };
        name.len() > 1 && !value.is_empty() && name.bytes().all(|b| b.is_ascii_alphabetic() || b == b'.') && !OWN_PROPERTIES.iter().any(|p| p.eq_ignore_ascii_case(name))
    };
    tokens(text).iter().any(|(tok, literal)| !literal && foreign(tok)).then(|| text.to_owned())
}

#[derive(Debug)]
enum Token {
    Word(String, bool),
    Or,
    And,
    Open,
    Close,
}

fn normalize_operators(lexed: Vec<(usize, usize, Token)>) -> Vec<(usize, Token)> {
    let mut groups = vec![false; lexed.len()];
    let mut stack = Vec::new();
    let mut last_word = None;
    for (i, (_, _, token)) in lexed.iter().enumerate() {
        match token {
            Token::Open => stack.push(i),
            Token::Close => {
                if let Some(open) = stack.pop().filter(|open| last_word.is_some_and(|word| word > *open)) {
                    groups[open] = true;
                    groups[i] = true;
                }
            }
            Token::Word(..) => last_word = Some(i),
            _ => {}
        }
    }
    let operand = |i: usize, before: bool| {
        lexed.get(i).is_some_and(|(_, _, token)| match token {
            Token::Word(word, false) => !matches!(word.as_str(), "OR" | "AND") && (!before || !matches!(word.as_str(), "NOT" | "!")),
            Token::Word(..) => true,
            Token::Close if before => groups[i],
            Token::Open if !before => groups[i],
            _ => false,
        })
    };
    let binary: Vec<_> = (0..lexed.len()).map(|i| i > 0 && operand(i - 1, true) && operand(i + 1, false)).collect();
    let mut out = Vec::new();
    let (mut previous_end, mut previous_literal_symbol) = (0, false);
    for (i, (at, end, token)) in lexed.into_iter().enumerate() {
        let symbol = |word: &str| Token::Word(word.to_owned(), false);
        let (token, literal_symbol) = match token {
            Token::Open if !groups[i] => (symbol("<"), true),
            Token::Close if !groups[i] => (symbol(">"), true),
            Token::Or if !binary[i] => (symbol("|"), true),
            Token::Word(word, false) if binary[i] && word == "OR" => (Token::Or, false),
            Token::Word(word, false) if binary[i] && word == "AND" => (Token::And, false),
            token => (token, false),
        };
        if at == previous_end && (literal_symbol || previous_literal_symbol) {
            if let (Some((_, Token::Word(previous, _))), Token::Word(word, _)) = (out.last_mut(), &token) {
                previous.push_str(word);
                previous_end = end;
                previous_literal_symbol = literal_symbol;
                continue;
            }
        }
        out.push((at, token));
        previous_end = end;
        previous_literal_symbol = literal_symbol;
    }
    out
}

fn lex(text: &str) -> Result<Vec<(usize, Token)>, usize> {
    let mut out = Vec::new();
    let (mut start, mut value, mut quoted, mut literal) = (None, String::new(), false, false);
    let flush = |out: &mut Vec<_>, start: &mut Option<usize>, value: &mut String, literal: &mut bool, end| {
        if let Some(at) = start.take().filter(|_| !value.is_empty()) {
            out.push((at, end, Token::Word(std::mem::take(value), *literal)));
        }
        *literal = false;
    };
    for (at, c) in text.char_indices() {
        if c == '"' {
            if start.is_none() {
                start = Some(at);
                literal = true;
            }
            quoted = !quoted;
            continue;
        }
        if !quoted && (c.is_whitespace() || matches!(c, '<' | '>' | '|')) {
            let prefixed = value.trim_start_matches('!').to_ascii_lowercase();
            let regex = prefixed.starts_with("regex:") || prefixed.starts_with("path:regex:") || prefixed.starts_with("regex:path:");
            let comparison = matches!(prefixed.as_str(), "size:" | "dm:" | "dc:" | "da:" | "depth:" | "child-count:");
            let operator = match c {
                '|' if !regex => Some(Token::Or),
                '<' if !regex && !comparison => Some(Token::Open),
                '>' if !regex && !comparison => Some(Token::Close),
                _ => None,
            };
            if let Some(t) = operator {
                flush(&mut out, &mut start, &mut value, &mut literal, at);
                out.push((at, at + 1, t));
                continue;
            }
            if c.is_whitespace() {
                flush(&mut out, &mut start, &mut value, &mut literal, at);
                continue;
            }
        }
        if !c.is_control() {
            start.get_or_insert(at);
            value.push(c);
        }
    }
    if quoted {
        return Err(start.unwrap_or(text.len()));
    }
    flush(&mut out, &mut start, &mut value, &mut literal, text.len());
    Ok(normalize_operators(out))
}

#[derive(Default)]
struct ParseBudget {
    terms: usize,
    regexes: usize,
}

struct Parser<'a> {
    tokens: std::iter::Peekable<std::vec::IntoIter<(usize, Token)>>,
    clock: Clock,
    end: usize,
    budget: &'a mut ParseBudget,
}

type ParseResult<T> = Result<T, (usize, String)>;

impl Parser<'_> {
    fn expression(&mut self, depth: usize) -> ParseResult<Query> {
        let mut branches = Vec::new();
        let mut terms = self.conjunction(depth)?;
        while matches!(self.tokens.peek(), Some((_, Token::Or))) {
            let (at, _) = self.tokens.next().unwrap();
            if terms.is_empty() {
                return Err((at, "missing expression before |".into()));
            }
            branches.push(Query { terms, ..Query::default() });
            terms = self.conjunction(depth)?;
            if terms.is_empty() {
                return Err((at, "missing expression after |".into()));
            }
        }
        if !branches.is_empty() {
            branches.push(Query { terms, ..Query::default() });
            terms = vec![Term { negate: false, filter: Filter::Any(branches) }];
        }
        Ok(Query { terms, ..Query::default() })
    }

    fn conjunction(&mut self, depth: usize) -> ParseResult<Vec<Term>> {
        let mut terms = Vec::new();
        let mut negate = false;
        let mut unary = None;
        let mut conjunction = None;
        while self.tokens.peek().is_some_and(|(_, t)| !matches!(t, Token::Or | Token::Close)) {
            let (at, token) = self.tokens.next().unwrap();
            let parsed = match token {
                Token::Word(w, false) if w == "NOT" || w == "!" => {
                    negate = !negate;
                    unary = Some(at);
                    continue;
                }
                Token::And => {
                    if terms.is_empty() || conjunction.is_some() || unary.is_some() {
                        return Err((at, "missing expression before AND".into()));
                    }
                    conjunction = Some(at);
                    continue;
                }
                Token::Word(w, literal) => {
                    self.budget.terms += 1;
                    if self.budget.terms > MAX_TERMS {
                        return Err((at, format!("the query has more than {MAX_TERMS} terms")));
                    }
                    if !literal && w.trim_start_matches('!').split(':').take(3).any(|s| s.eq_ignore_ascii_case("regex")) {
                        self.budget.regexes += 1;
                        if self.budget.regexes > MAX_REGEXES {
                            return Err((at, format!("the query has more than {MAX_REGEXES} regular expressions")));
                        }
                    }
                    term(&w, literal, self.clock).map_err(|e| (at, e))?
                }
                Token::Open => {
                    if depth >= MAX_TERMS {
                        return Err((at, "groups are nested too deeply".into()));
                    }
                    let group = self.expression(depth + 1)?;
                    if group.terms.is_empty() {
                        return Err((at, "empty group".into()));
                    }
                    if !matches!(self.tokens.next(), Some((_, Token::Close))) {
                        return Err((at, "group is missing >".into()));
                    }
                    Some(Term { negate: false, filter: Filter::Any(vec![group]) })
                }
                _ => unreachable!(),
            };
            if let Some(mut t) = parsed {
                t.negate ^= std::mem::take(&mut negate);
                terms.push(t);
                (unary, conjunction) = (None, None);
            }
        }
        if let Some(at) = unary.or(conjunction) {
            return Err((at, "missing expression after operator".into()));
        }
        Ok(terms)
    }

    fn finish(mut self) -> ParseResult<Query> {
        let query = self.expression(0)?;
        if let Some((at, _)) = self.tokens.next() {
            return Err((at.min(self.end), "unexpected >".into()));
        }
        Ok(query)
    }
}

#[derive(Clone, Debug)]
pub struct ParseError {
    pub position: usize,
    detail: String,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (position {})", self.detail, self.position)
    }
}

pub fn parse_with_position(text: &str, clock: Clock) -> Result<Query, ParseError> {
    parse_part(text, clock, &mut ParseBudget::default())
}

fn parse_part(text: &str, clock: Clock, budget: &mut ParseBudget) -> Result<Query, ParseError> {
    let err = |at, detail| ParseError { position: text[..at].chars().count() + 1, detail };
    let tokens = lex(text).map_err(|at| err(at, "unterminated quote".into()))?;
    Parser { tokens: tokens.into_iter().peekable(), clock, end: text.len(), budget }
        .finish().map_err(|(at, message)| err(at, message))
}

pub fn parse(text: &str, clock: Clock) -> Result<Query, String> {
    parse_with_position(text, clock).map_err(|e| e.to_string())
}

pub fn parse_with_conditions(text: &str, conditions: &[String], clock: Clock) -> Result<Query, ParseError> {
    let mut budget = ParseBudget::default();
    let mut query = parse_part(text, clock, &mut budget)?;
    for condition in conditions {
        query.terms.extend(parse_part(condition, clock, &mut budget)?.terms);
    }
    Ok(query)
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
                Some(Filter::Ext(query_attrs::extensions(v).ok_or_else(|| format!("ext: lists at most {MAX_EXTS} extensions"))?))
            } else if let Some(v) = strip_ci(s, "size:") {
                let (lo, hi) = query_attrs::size_range(v).ok_or_else(|| format!("invalid size: {v}"))?;
                Some(Filter::Size(lo, hi))
            } else if let Some(v) = strip_ci(s, "dm:") {
                let (lo, hi) = query_attrs::date_range(v, clock).ok_or_else(|| format!("invalid date: {v}"))?;
                Some(Filter::Modified(lo, hi))
            } else if let Some(v) = strip_ci(s, "dc:").or_else(|| strip_ci(s, "da:")) {
                let (lo, hi) = query_attrs::date_range(v, clock).ok_or_else(|| format!("invalid date: {v}"))?;
                Some(if strip_ci(s, "dc:").is_some() { Filter::Created(lo, hi) } else { Filter::Accessed(lo, hi) })
            } else if let Some(v) = strip_ci(s, "child-count:").or_else(|| strip_ci(s, "depth:")) {
                let (lo, hi) = query_attrs::number_range(v).ok_or_else(|| format!("invalid count: {v}"))?;
                Some(if strip_ci(s, "depth:").is_some() { Filter::Depth(lo, hi) } else { Filter::Children(lo, hi) })
            } else if let Some(v) = strip_ci(s, "empty:") {
                match v.to_ascii_lowercase().as_str() {
                    "true" | "yes" | "1" => Some(Filter::Children(0, 1)),
                    "false" | "no" | "0" => Some(Filter::Children(1, u64::MAX)),
                    _ => return Err(format!("invalid boolean: {v}")),
                }
            } else if let Some(v) = strip_ci(s, "attrib:") {
                let mut mask = 0;
                for name in v.split([';', ',']) {
                    mask |= match name.to_ascii_lowercase().as_str() {
                        "hidden" => flit_core::attr::HIDDEN,
                        "system" => flit_core::attr::SYSTEM,
                        "readonly" | "read-only" => flit_core::attr::READONLY,
                        "link" => flit_core::attr::LINK,
                        "folder" | "directory" => flit_core::attr::DIR,
                        _ => return Err(format!("unknown attribute: {name}")),
                    };
                }
                Some(Filter::Attributes(mask))
            } else if let Some((dir, rest)) = strip_ci(s, "file:").map(|s| (false, s)).or_else(|| strip_ci(s, "folder:").map(|s| (true, s))) {
                let filter = Filter::Directory(dir);
                if rest.len() > MAX_PATTERN {
                    return Err(format!("a search term is longer than {MAX_PATTERN} bytes"));
                }
                if rest.is_empty() {
                    Some(filter)
                } else {
                    let pattern = if rest.contains(['*', '?']) { Pattern::Glob(lowercase(rest.as_bytes())) } else { Pattern::Text(lowercase(rest.as_bytes())) };
                    Some(Filter::Any(vec![Query { terms: vec![
                        Term { negate: false, filter },
                        Term { negate: false, filter: Filter::Match(Scope::Name, pattern) },
                    ], ..Query::default() }]))
                }
            } else {
                strip_ci(s, "tag:").map(|v| Filter::Tag(v.trim().to_owned()))
            };
            let filter = if let Some(v) = strip_ci(s, "rating:") {
                let (lo, hi) = query_attrs::interval(v, &rating_value).ok_or_else(|| flit_core::i18n::S::PortableMetaInvalidRating.en().to_owned())?;
                Some(Filter::Rating(lo.clamp(0, 11) as u8, hi.clamp(0, 11) as u8))
            } else if let Some(v) = strip_ci(s, "comment:") {
                Some(Filter::Comment(v.to_lowercase()))
            } else if let Some(v) = strip_ci(s, "portable-tag:") {
                Some(Filter::PortableTag(v.to_lowercase()))
            } else {
                filter
            };
            if let Some(filter) = filter {
                return Ok(Some(Term { negate, filter }));
            }
        }
    }
    if s.is_empty() {
        return Ok(None);
    }
    if s.len() > MAX_PATTERN {
        return Err(format!("a search term is longer than {MAX_PATTERN} bytes"));
    }
    let pattern = if regex {
        let re = RegexBuilder::new(s).case_insensitive(true).size_limit(REGEX_SIZE_LIMIT).build().map_err(|e| format!("invalid regex: {e}"))?;
        Pattern::Regex(re)
    } else {
        if s.contains(['\\', '/']) {
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

fn rating_value(s: &str) -> Option<(i128, i128)> {
    let n = s.trim().parse::<f64>().ok()? * 2.0;
    if !(0.0..=10.0).contains(&n) || n.fract() != 0.0 { return None; }
    Some((n as i128, n as i128 + 1))
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
    let wild = |c: &u8| *c == b'*' || *c == b'?';
    if let Some(suffix) = p.strip_prefix(b"*").filter(|r| !r.iter().any(wild)) {
        return s.ends_with(suffix);
    }
    if let Some(prefix) = p.strip_suffix(b"*").filter(|r| !r.iter().any(wild)) {
        return s.starts_with(prefix);
    }
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
