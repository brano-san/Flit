use super::syntax::{self, SYNTAXES};
use super::{expand_tabs, Block, Doc, Tone};
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

#[derive(Default)]
struct Markdown {
    doc: Doc,
    heading: Option<u8>,
    lists: Vec<Option<u64>>,
    markers: Vec<Option<String>>,
    quotes: usize,
    strong: usize,
    links: usize,
    images: usize,
    cell: usize,
}

impl Markdown {
    fn marker(&mut self) {
        if let Some(mark) = self.markers.last_mut().and_then(Option::take) {
            self.doc.run(&mark, Tone::Dim, false, false);
        }
    }

    fn finish(&mut self) {
        if self.doc.open_start() as usize == self.doc.runs.len() {
            return;
        }
        let block = if let Some(level) = self.heading {
            Block::Heading(level)
        } else if !self.lists.is_empty() {
            Block::Item { depth: self.lists.len().saturating_sub(1).min(6) as u8 }
        } else if self.quotes > 0 {
            Block::Quote
        } else {
            Block::Body
        };
        self.doc.para(block);
    }

    fn code(&mut self, text: &str, lang: Option<u8>) {
        let (mut line, mut state) = (String::new(), None);
        for raw in text.lines() {
            line.clear();
            expand_tabs(raw, &mut line);
            if let Some(k) = lang {
                self.doc.highlighted(&line, &SYNTAXES[k as usize], &mut state);
            } else {
                self.doc.run(&line, Tone::Plain, false, true);
            }
            self.doc.para(Block::Code);
        }
    }
}

fn options() -> Options {
    Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS | Options::ENABLE_FOOTNOTES | Options::ENABLE_GFM
}

pub fn markdown(src: &str) -> Doc {
    let mut md = Markdown::default();
    md.doc.wrap = true;
    let mut events = Parser::new_ext(src, options());
    while let Some(event) = events.next() {
        match event {
            Event::Start(Tag::CodeBlock(kind)) => {
                md.finish();
                let lang = match kind {
                    CodeBlockKind::Fenced(info) => info.split_whitespace().next().and_then(syntax::by_name),
                    CodeBlockKind::Indented => None,
                };
                let mut code = String::new();
                for part in events.by_ref() {
                    match part {
                        Event::Text(text) => code.push_str(&text),
                        Event::End(TagEnd::CodeBlock) => break,
                        _ => {}
                    }
                }
                md.code(&code, lang);
            }
            Event::Start(Tag::Heading { level, .. }) => md.heading = Some(level as u8),
            Event::End(TagEnd::Heading(_)) => {
                md.finish();
                md.heading = None;
            }
            Event::Start(Tag::BlockQuote(_)) => {
                md.finish();
                md.quotes += 1;
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                md.finish();
                md.quotes -= 1;
            }
            Event::Start(Tag::List(start)) => {
                md.finish();
                md.lists.push(start);
            }
            Event::End(TagEnd::List(_)) => {
                md.lists.pop();
            }
            Event::Start(Tag::Item) => {
                let marker = match md.lists.last_mut() {
                    Some(Some(n)) => {
                        let label = format!("{n}. ");
                        *n += 1;
                        label
                    }
                    _ => "• ".to_owned(),
                };
                md.markers.push(Some(marker));
            }
            Event::End(TagEnd::Item) => {
                md.marker();
                md.finish();
                md.markers.pop();
            }
            Event::TaskListMarker(done) => {
                if let Some(mark) = md.markers.last_mut() {
                    *mark = Some(if done { "☑ " } else { "☐ " }.into());
                }
            }
            Event::End(TagEnd::Paragraph) => md.finish(),
            Event::Rule => {
                md.finish();
                md.doc.para(Block::Rule);
            }
            Event::Start(Tag::TableRow | Tag::TableHead) => md.cell = 0,
            Event::Start(Tag::TableCell) => {
                if md.cell > 0 {
                    md.doc.run("  │  ", Tone::Plain, false, false);
                }
                md.cell += 1;
            }
            Event::End(TagEnd::TableRow | TagEnd::TableHead) => md.finish(),
            Event::Start(Tag::Strong) => md.strong += 1,
            Event::End(TagEnd::Strong) => md.strong -= 1,
            Event::Start(Tag::Link { .. }) => md.links += 1,
            Event::End(TagEnd::Link) => md.links -= 1,
            Event::Start(Tag::Image { .. }) => md.images += 1,
            Event::End(TagEnd::Image) => md.images -= 1,
            Event::Text(text) | Event::Html(text) | Event::InlineHtml(text) => {
                md.marker();
                let tone = if md.images > 0 { Tone::Dim } else if md.links > 0 { Tone::Link } else { Tone::Plain };
                md.doc.run(&text, tone, md.strong > 0 || md.heading.is_some(), false);
            }
            Event::Code(text) => {
                md.marker();
                md.doc.run(&text, Tone::String, md.strong > 0 || md.heading.is_some(), true);
            }
            Event::SoftBreak | Event::HardBreak => md.doc.run(" ", Tone::Plain, false, false),
            _ => {}
        }
    }
    md.finish();
    md.doc
}

#[cfg(test)]
#[path = "../../test/preview/markdown.rs"]
mod tests;
