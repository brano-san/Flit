mod inline;

use super::syntax::{self, SYNTAXES};
use super::{expand_tabs, Alignment, Block, Doc, Table, Tone};
use crate::hash::FxHashMap;
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

#[derive(Default)]
struct Markdown {
    doc: Doc,
    heading: Option<u8>,
    lists: Vec<Option<u64>>,
    markers: Vec<Option<String>>,
    quotes: usize,
    table: Option<Table>,
    cell_start: u32,
    table_head: bool,
    footnote: bool,
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
        } else if self.footnote {
            Block::Footnote
        } else if self.quotes > 0 {
            Block::Quote
        } else {
            Block::Body
        };
        self.para(block);
    }

    fn para(&mut self, block: Block) {
        self.doc.para(block);
        self.doc.paras.last_mut().unwrap().quote_depth = self.quotes.min(6) as u8;
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
            self.para(Block::Code);
        }
    }
}

fn options() -> Options {
    Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS | Options::ENABLE_FOOTNOTES | Options::ENABLE_GFM
}

pub fn markdown(src: &str) -> Doc {
    let mut md = Markdown::default();
    md.doc.wrap = true;
    let mut notes = Notes::default();
    render(&mut md, Parser::new_ext(src, options()), &mut notes);
    if !notes.definitions.is_empty() {
        md.para(Block::Rule);
        md.footnote = true;
        while !notes.definitions.is_empty() {
            let mut definitions = std::mem::take(&mut notes.definitions);
            definitions.sort_by_cached_key(|(label, _)| notes.number(label));
            for (label, body) in definitions {
                md.doc.run(&format!("[{}] ", notes.number(&label)), Tone::Dim, false, false);
                render(&mut md, body.into_iter(), &mut notes);
            }
        }
    }
    md.doc
}

#[derive(Default)]
struct Notes<'a> {
    numbers: FxHashMap<String, usize>,
    definitions: Vec<(String, Vec<Event<'a>>)>,
}

impl Notes<'_> {
    fn number(&mut self, label: &str) -> usize {
        let next = self.numbers.len() + 1;
        *self.numbers.entry(label.to_lowercase()).or_insert(next)
    }
}

fn render<'a>(md: &mut Markdown, mut events: impl Iterator<Item = Event<'a>>, notes: &mut Notes<'a>) {
    let mut inline = inline::Inline::default();
    while let Some(event) = events.next() {
        match event {
            Event::Start(Tag::FootnoteDefinition(label)) => {
                md.finish();
                let mut depth = 1;
                let body = events.by_ref().take_while(|event| {
                    match event {
                        Event::Start(Tag::FootnoteDefinition(_)) => depth += 1,
                        Event::End(TagEnd::FootnoteDefinition) => depth -= 1,
                        _ => {}
                    }
                    depth > 0
                }).collect();
                notes.definitions.push((label.into_string(), body));
            }
            Event::FootnoteReference(label) => {
                md.marker();
                md.doc.run(&format!("[{}]", notes.number(&label)), Tone::Dim, md.table_head, false);
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                md.marker();
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
                inline.end_block();
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
                md.marker();
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
                inline.end_block();
            }
            Event::TaskListMarker(done) => {
                if let Some(mark) = md.markers.last_mut() {
                    *mark = Some(if done { "☑ " } else { "☐ " }.into());
                }
            }
            Event::End(TagEnd::Paragraph | TagEnd::HtmlBlock) => {
                md.finish();
                inline.end_block();
            }
            Event::Rule => {
                md.marker();
                md.finish();
                md.para(Block::Rule);
            }
            Event::Start(Tag::Table(align)) => {
                md.marker();
                md.finish();
                let align = align.into_iter().map(|a| match a {
                    pulldown_cmark::Alignment::Center => Alignment::Center,
                    pulldown_cmark::Alignment::Right => Alignment::Right,
                    _ => Alignment::Left,
                }).collect();
                md.table = Some(Table { align, rows: Vec::new() });
            }
            Event::Start(Tag::TableRow | Tag::TableHead) => {
                let table = md.table.as_mut().unwrap();
                md.table_head = table.rows.is_empty();
                if !md.table_head {
                    md.doc.text.push('\n');
                }
                table.rows.push(Vec::with_capacity(table.align.len()));
            }
            Event::Start(Tag::TableCell) => {
                let row = md.table.as_ref().unwrap().rows.last().unwrap();
                if !row.is_empty() {
                    md.doc.text.push('\t');
                }
                md.cell_start = md.doc.runs.len() as u32;
            }
            Event::End(TagEnd::TableCell) => {
                let row = md.table.as_mut().unwrap().rows.last_mut().unwrap();
                row.push(md.cell_start..md.doc.runs.len() as u32);
                inline.end_block();
            }
            Event::End(TagEnd::Table) => {
                let index = md.doc.tables.len();
                md.doc.tables.push(md.table.take().unwrap());
                md.para(Block::Table(index));
                md.table_head = false;
            }
            _ => {
                if matches!(event, Event::Text(_) | Event::Code(_) | Event::Html(_) | Event::InlineHtml(_) | Event::Start(Tag::Image { .. })) {
                    md.marker();
                }
                inline.event(&mut md.doc, &event, md.heading.is_some() || md.table_head);
            }
        }
    }
    md.finish();
}

#[cfg(test)]
#[path = "../../test/preview/markdown.rs"]
mod tests;
