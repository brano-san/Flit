use super::syntax::{self, SYNTAXES};
use super::{expand_tabs, Block, Doc, Tone};

#[derive(Clone, Copy, PartialEq)]
enum Open {
    Body,
    Quote,
    Item(u8),
}

struct Md {
    doc: Doc,
    open: Option<(Open, String)>,
    mark: String,
}

fn heading(t: &str) -> Option<(u8, &str)> {
    let n = t.bytes().take_while(|&c| c == b'#').count();
    let rest = &t[n..];
    ((1..=6).contains(&n) && (rest.is_empty() || rest.starts_with(' '))).then(|| (n as u8, rest.trim().trim_end_matches('#').trim_end()))
}

fn rule(t: &str) -> bool {
    let c = t.as_bytes()[0];
    matches!(c, b'-' | b'*' | b'_') && t.bytes().filter(|&b| b == c).count() >= 3 && t.bytes().all(|b| b == c || b == b' ')
}

fn item(t: &str) -> Option<(String, &str)> {
    let b = t.as_bytes();
    if matches!(b[0], b'-' | b'*' | b'+') && b.get(1) == Some(&b' ') {
        let rest = t[2..].trim_start();
        return Some(match rest.get(..3) {
            Some("[ ]") => ("\u{2610}".into(), rest[3..].trim_start()),
            Some("[x]" | "[X]") => ("\u{2611}".into(), rest[3..].trim_start()),
            _ => ("\u{2022}".into(), rest),
        });
    }
    let digits = b.iter().take_while(|c| c.is_ascii_digit()).count();
    if (1..=9).contains(&digits) && matches!(b.get(digits), Some(b'.' | b')')) && b.get(digits + 1) == Some(&b' ') {
        return Some((format!("{}.", &t[..digits]), t[digits + 2..].trim_start()));
    }
    None
}

fn table_separator(t: &str) -> bool {
    t.bytes().all(|c| matches!(c, b'|' | b'-' | b':' | b' '))
}

impl Md {
    fn flush(&mut self) {
        let Some((kind, text)) = self.open.take() else { return };
        let block = match kind {
            Open::Body => Block::Body,
            Open::Quote => Block::Quote,
            Open::Item(depth) => {
                self.doc.run(&format!("{} ", self.mark), Tone::Dim, false, false);
                Block::Item { depth }
            }
        };
        inline(&mut self.doc, &text, false);
        self.doc.para(block);
    }

    fn append(&mut self, kind: Open, text: &str) {
        match &mut self.open {
            Some((k, s)) if *k == kind || (kind == Open::Body && matches!(k, Open::Item(_) | Open::Quote)) => {
                s.push(' ');
                s.push_str(text);
            }
            _ => {
                self.flush();
                self.open = Some((kind, text.to_owned()));
            }
        }
    }
}

pub fn markdown(src: &str) -> Doc {
    let mut md = Md { doc: Doc { wrap: true, ..Default::default() }, open: None, mark: String::new() };
    let mut fence: Option<(&str, Option<u8>, Option<u8>)> = None;
    let mut tab = String::new();
    for raw in src.lines() {
        if let Some((marker, syn, state)) = &mut fence {
            if raw.trim_start().starts_with(*marker) {
                fence = None;
                continue;
            }
            tab.clear();
            expand_tabs(raw, &mut tab);
            match syn {
                Some(k) => md.doc.highlighted(&tab, &SYNTAXES[*k as usize], state),
                None => md.doc.run(&tab, Tone::Plain, false, true),
            }
            md.doc.para(Block::Code);
            continue;
        }
        let line = raw.trim_end();
        let t = line.trim_start();
        let indent = line.len() - t.len();
        if t.is_empty() {
            md.flush();
            continue;
        }
        if t.starts_with("```") || t.starts_with("~~~") {
            md.flush();
            fence = Some((&t[..3], syntax::by_name(t[3..].trim()), None));
            continue;
        }
        if let Some((level, text)) = heading(t) {
            md.flush();
            inline(&mut md.doc, text, true);
            md.doc.para(Block::Heading(level));
            continue;
        }
        if matches!(md.open, Some((Open::Body, _))) && (t.bytes().all(|c| c == b'=') || t.bytes().all(|c| c == b'-')) {
            let (_, text) = md.open.take().unwrap_or((Open::Body, String::new()));
            inline(&mut md.doc, &text, true);
            md.doc.para(Block::Heading(if t.starts_with('=') { 1 } else { 2 }));
            continue;
        }
        if rule(t) {
            md.flush();
            md.doc.para(Block::Rule);
            continue;
        }
        if let Some(q) = t.strip_prefix('>') {
            md.append(Open::Quote, q.trim_start());
            continue;
        }
        if let Some((mark, rest)) = item(t) {
            md.flush();
            md.mark = mark;
            md.open = Some((Open::Item((indent / 2).min(6) as u8), rest.to_owned()));
            continue;
        }
        if t.starts_with('|') {
            md.flush();
            if !table_separator(t) {
                let cells: Vec<&str> = t.trim_matches('|').split('|').map(str::trim).collect();
                inline(&mut md.doc, &cells.join("  \u{2502}  "), false);
                md.doc.para(Block::Body);
            }
            continue;
        }
        md.append(Open::Body, t);
    }
    md.flush();
    md.doc
}

fn closing(s: &str, from: usize, pat: &str) -> Option<usize> {
    s.get(from..)?.find(pat).map(|p| p + from).filter(|&p| p > from)
}

pub(super) fn inline(d: &mut Doc, s: &str, heading: bool) {
    let (mut bold, mut em) = (heading, None);
    let mut buf = String::new();
    let mut i = 0;
    let b = s.as_bytes();
    let flush = |d: &mut Doc, buf: &mut String, bold: bool| {
        d.run(buf, Tone::Plain, bold, false);
        buf.clear();
    };
    while i < b.len() {
        let c = b[i];
        if c == b'\\' && b.get(i + 1).is_some_and(u8::is_ascii_punctuation) {
            buf.push(b[i + 1] as char);
            i += 2;
            continue;
        }
        if c == b'`' {
            let ticks = b[i..].iter().take_while(|&&x| x == b'`').count();
            if let Some(end) = closing(s, i + ticks, &s[i..i + ticks]) {
                flush(d, &mut buf, bold);
                d.run(s[i + ticks..end].trim(), Tone::String, bold, true);
                i = end + ticks;
                continue;
            }
        }
        if s[i..].starts_with("**") || s[i..].starts_with("__") {
            flush(d, &mut buf, bold);
            bold = heading || !bold;
            i += 2;
            continue;
        }
        let snake = c == b'_' && i > 0 && b[i - 1].is_ascii_alphanumeric() && b.get(i + 1).is_some_and(u8::is_ascii_alphanumeric);
        if matches!(c, b'*' | b'_') && !snake {
            if em == Some(c) && i > 0 && !b[i - 1].is_ascii_whitespace() {
                em = None;
                i += 1;
                continue;
            }
            if em.is_none() && b.get(i + 1).is_some_and(|n| !n.is_ascii_whitespace()) && closing(s, i + 1, &s[i..i + 1]).is_some() {
                em = Some(c);
                i += 1;
                continue;
            }
        }
        let image = c == b'!' && b.get(i + 1) == Some(&b'[');
        if c == b'[' || image {
            let open = i + image as usize;
            if let Some(end) = closing(s, open + 1, "](").and_then(|close| closing(s, close + 2, ")").map(|end| (close, end))) {
                flush(d, &mut buf, bold);
                d.run(&s[open + 1..end.0], if image { Tone::Dim } else { Tone::Link }, bold, false);
                i = end.1 + 1;
                continue;
            }
        }
        if c == b'<' && (s[i..].starts_with("<http://") || s[i..].starts_with("<https://")) {
            if let Some(end) = closing(s, i + 1, ">") {
                flush(d, &mut buf, bold);
                d.run(&s[i + 1..end], Tone::Link, bold, false);
                i = end + 1;
                continue;
            }
        }
        let n = s[i..].chars().next().map_or(1, char::len_utf8);
        buf.push_str(&s[i..i + n]);
        i += n;
    }
    flush(d, &mut buf, bold);
}

#[cfg(test)]
#[path = "../../test/preview/markdown.rs"]
mod tests;
