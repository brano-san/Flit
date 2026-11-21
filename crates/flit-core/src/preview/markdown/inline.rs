use crate::preview::{Doc, Tone};
use pulldown_cmark::{CowStr, Event, Tag, TagEnd};

#[derive(Default)]
pub(super) struct Inline<'a> {
    strong: usize,
    strike: usize,
    links: usize,
    images: Vec<(CowStr<'a>, usize)>,
    keyboard: usize,
    comment: bool,
    html_text: bool,
    html_break: bool,
    incomplete_tag: String,
}

impl<'a> Inline<'a> {
    pub(super) fn end_block(&mut self) {
        self.keyboard = 0;
        self.comment = false;
        self.html_text = false;
        self.html_break = false;
        self.incomplete_tag.clear();
    }

    pub(super) fn event(&mut self, doc: &mut Doc, event: &Event<'a>, bold: bool) {
        match event {
            Event::Start(Tag::Strong) => self.strong += 1,
            Event::End(TagEnd::Strong) => self.strong -= 1,
            Event::Start(Tag::Strikethrough) => self.strike += 1,
            Event::End(TagEnd::Strikethrough) => self.strike -= 1,
            Event::Start(Tag::Link { .. }) => self.links += 1,
            Event::End(TagEnd::Link) => self.links -= 1,
            Event::Start(Tag::Image { dest_url, .. }) => {
                self.run(doc, "🖼 ", Tone::Dim, bold, false);
                self.images.push((dest_url.clone(), doc.text.len()));
            }
            Event::End(TagEnd::Image) => {
                if let Some((url, start)) = self.images.pop() {
                    if start == doc.text.len() {
                        let path = url.split(['?', '#']).next().unwrap_or("").trim_end_matches(['/', '\\']);
                        self.run(doc, path.rsplit(['/', '\\']).next().unwrap_or(""), Tone::Dim, bold, false);
                    }
                }
            }
            Event::Text(text) => self.text(doc, text, bold),
            Event::Code(text) => self.run(doc, text, Tone::String, bold, true),
            Event::Html(html) => self.html(doc, html, bold, true),
            Event::InlineHtml(html) => self.html(doc, html, bold, false),
            Event::HardBreak => self.run(doc, "\n", Tone::Plain, bold, false),
            Event::SoftBreak => self.run(doc, " ", Tone::Plain, bold, false),
            _ => {}
        }
    }

    fn run(&self, doc: &mut Doc, text: &str, tone: Tone, bold: bool, mono: bool) {
        let tone = if !self.images.is_empty() || tone == Tone::Dim {
            Tone::Dim
        } else if self.links > 0 {
            Tone::Link
        } else {
            tone
        };
        doc.styled_run(text, tone, bold || self.strong > 0, mono || self.keyboard > 0, self.strike > 0);
    }

    fn text(&self, doc: &mut Doc, mut text: &str, bold: bool) {
        if self.links == 0 && self.images.is_empty() && self.keyboard == 0 && text.as_bytes().contains(&b':') {
            while let Some((start, end)) = bare_url(text) {
                self.run(doc, &text[..start], Tone::Plain, bold, false);
                self.run(doc, &text[start..end], Tone::Link, bold, false);
                text = &text[end..];
            }
        }
        self.run(doc, text, Tone::Plain, bold, false);
    }

    fn html(&mut self, doc: &mut Doc, html: &str, bold: bool, block: bool) {
        let joined;
        let mut rest = if self.incomplete_tag.is_empty() {
            html
        } else {
            self.incomplete_tag.push_str(html);
            joined = std::mem::take(&mut self.incomplete_tag);
            &joined
        };
        while !rest.is_empty() {
            if self.comment {
                let Some((_, after)) = rest.split_once("-->") else { return };
                self.comment = false;
                rest = after;
                continue;
            }
            let (visible, markup) = rest.split_at(rest.find('<').unwrap_or(rest.len()));
            if block {
                for (i, line) in visible.split('\n').enumerate() {
                    if i > 0 && self.html_text {
                        self.html_break = true;
                    }
                    let line = line.trim_end_matches('\r');
                    if !line.is_empty() {
                        if self.html_break {
                            self.run(doc, "\n", Tone::Plain, bold, false);
                            self.html_break = false;
                        }
                        self.text(doc, line, bold);
                        self.html_text = true;
                    }
                }
            } else {
                self.text(doc, visible, bold);
            }
            if markup.is_empty() {
                return;
            }
            if let Some(after) = markup.strip_prefix("<!--") {
                self.comment = true;
                rest = after;
                continue;
            }
            let after = &markup[1..];
            let tag = after.strip_prefix('/').unwrap_or(after);
            if !tag.as_bytes().first().is_some_and(|c| c.is_ascii_alphabetic() || matches!(c, b'!' | b'?')) {
                self.run(doc, "<", Tone::Plain, bold, false);
                rest = after;
                continue;
            }
            let Some(end) = tag_end(after) else {
                self.incomplete_tag.push_str(markup);
                return;
            };
            let name = tag.split([' ', '\t', '\r', '\n', '/', '>']).next().unwrap_or("");
            let closing = after.starts_with('/');
            if name.eq_ignore_ascii_case("br") && !closing {
                self.run(doc, "\n", Tone::Plain, bold, false);
                self.html_break = false;
            } else if name.eq_ignore_ascii_case("kbd") {
                if closing {
                    self.keyboard = self.keyboard.saturating_sub(1);
                } else if !after[..end].trim_end().ends_with('/') {
                    self.keyboard += 1;
                }
            }
            rest = &after[end + 1..];
        }
    }
}

fn tag_end(tag: &str) -> Option<usize> {
    let mut quote = None;
    for (i, b) in tag.bytes().enumerate() {
        if quote == Some(b) {
            quote = None;
        } else if quote.is_none() {
            match b {
                b'>' => return Some(i),
                b'\'' | b'"' => quote = Some(b),
                _ => {}
            }
        }
    }
    None
}

fn bare_url(text: &str) -> Option<(usize, usize)> {
    for (colon, _) in text.match_indices("://") {
        let Some(start) = ["https", "http"].into_iter().find_map(|scheme| {
            let start = colon.checked_sub(scheme.len())?;
            text.get(start..colon)?.eq_ignore_ascii_case(scheme).then_some(start)
        }) else {
            continue;
        };
        if text[..start].chars().next_back().is_some_and(|c| !c.is_whitespace() && !matches!(c, '(' | '*' | '_' | '~')) {
            continue;
        }
        let after = &text[colon + 3..];
        let host = after.split(|c: char| !(c.is_alphanumeric() || matches!(c, '.' | '-' | '_'))).next().unwrap_or("").trim_end_matches('.');
        if !host.contains('.') || host.split('.').any(str::is_empty) || host.rsplit('.').take(2).any(|segment| segment.contains('_')) {
            continue;
        }
        let stop = after.find(|c: char| c.is_whitespace() || c == '<').unwrap_or(after.len());
        let mut url = &after[..stop];
        let opens = url.bytes().filter(|&c| c == b'(').count();
        let mut closes = url.bytes().filter(|&c| c == b')').count();
        loop {
            url = url.trim_end_matches(['.', ',', ':', '!', '?', '*', '_', '~']);
            if let Some((prefix, entity)) = url.strip_suffix(';').and_then(|s| s.rsplit_once('&')) {
                if !entity.is_empty() && entity.bytes().all(|b| b.is_ascii_alphanumeric()) {
                    url = prefix;
                    continue;
                }
            }
            if closes <= opens || !url.ends_with(')') {
                break;
            }
            closes -= 1;
            url = &url[..url.len() - 1];
        }
        return Some((start, colon + 3 + url.len()));
    }
    None
}
