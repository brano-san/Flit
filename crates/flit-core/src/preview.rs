pub mod markdown;
pub mod syntax;

use crate::hash::FxHashMap;
use crate::name;
use crate::path::FsPath;
use crate::provider::GenToken;
use crate::render::Rect;
use crate::ui::Bitmap;
use std::ops::Range;
use syntax::{Syntax, SYNTAXES};

pub const TEXT_CHARS: usize = 10_000;
pub const TEXT_BYTES: usize = 64 * 1024;
pub const RTF_BYTES: usize = 1 << 20;
pub const SNIFF_LIMIT: u64 = 500 * 1024;
pub const IMAGE_PX: u32 = 1600;
pub const IMAGE_FILE_LIMIT: u64 = 256 << 20;
pub const POSTER_PX: u32 = 256;
pub const FOLDER_SCAN: usize = 20_000;
pub const MAX_PROPS: usize = 120;
pub const FOLDER_ROWS: usize = 2_000;

const IMAGES: &[&str] = &["jpg", "jpeg", "jpe", "jfif", "png", "gif", "bmp", "dib", "tif", "tiff", "ico", "webp", "heic", "heif", "avif", "jxr", "wdp", "dds", "jxl"];
const AUDIO: &[&str] = &["mp3", "wav", "wma", "flac", "m4a", "aac", "ogg", "oga", "opus", "aif", "aiff", "amr", "ac3"];
const VIDEO: &[&str] = &["mp4", "m4v", "mov", "wmv", "avi", "mkv", "webm", "mpg", "mpeg", "3gp", "3g2", "m2ts", "mts", "asf"];
const TEXT: &[&str] = &["txt", "log", "csv", "tsv", "nfo", "diz", "srt", "vtt", "lst", "text", "readme", "gitignore", "gitattributes"];
const MARKDOWN: &[&str] = &["md", "markdown", "mdown", "mkd", "mdx"];

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Kind {
    Folder,
    Image,
    Text,
    Code(u8),
    Markdown,
    Rtf,
    Audio,
    Video,
    Other,
}

pub fn kind(ext: &[u8]) -> Kind {
    let e = name::display(ext).to_ascii_lowercase();
    let is = |list: &[&str]| list.contains(&e.as_str());
    match () {
        _ if is(IMAGES) => Kind::Image,
        _ if is(AUDIO) => Kind::Audio,
        _ if is(VIDEO) => Kind::Video,
        _ if is(TEXT) => Kind::Text,
        _ if is(MARKDOWN) => Kind::Markdown,
        _ if e == "rtf" => Kind::Rtf,
        _ => syntax::by_ext(&e).map_or(Kind::Other, Kind::Code),
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tone {
    Plain,
    Dim,
    Keyword,
    String,
    Comment,
    Number,
    Link,
    Added,
    Removed,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Run {
    pub start: u32,
    pub end: u32,
    pub tone: Tone,
    pub bold: bool,
    pub mono: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Block {
    Body,
    Heading(u8),
    Code,
    Quote,
    Item { depth: u8 },
    Rule,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Para {
    pub block: Block,
    pub runs: Range<u32>,
}

#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct Doc {
    pub text: String,
    pub runs: Vec<Run>,
    pub paras: Vec<Para>,
    pub mono: bool,
    pub wrap: bool,
    pub truncated: bool,
    pub encoding: &'static str,
}

impl Doc {
    fn open_start(&self) -> u32 {
        self.paras.last().map_or(0, |p| p.runs.end)
    }

    pub fn run(&mut self, s: &str, tone: Tone, bold: bool, mono: bool) {
        if s.is_empty() {
            return;
        }
        let start = self.text.len() as u32;
        self.text.push_str(s);
        let end = self.text.len() as u32;
        let open = self.runs.len() > self.open_start() as usize;
        match self.runs.last_mut() {
            Some(r) if open && r.end == start && r.tone == tone && r.bold == bold && r.mono == mono => r.end = end,
            _ => self.runs.push(Run { start, end, tone, bold, mono }),
        }
    }

    pub fn para(&mut self, block: Block) {
        let start = self.open_start();
        self.paras.push(Para { block, runs: start..self.runs.len() as u32 });
    }

    pub fn run_text(&self, r: &Run) -> &str {
        &self.text[r.start as usize..r.end as usize]
    }

    fn highlighted(&mut self, line: &str, syn: &Syntax, state: &mut Option<u8>) {
        let mut spans = Vec::new();
        syntax::highlight(syn, line, state, &mut spans);
        for (a, b, tone) in spans {
            self.run(&line[a as usize..b as usize], tone, false, true);
        }
    }
}

pub fn expand_tabs(line: &str, out: &mut String) {
    let mut col = 0;
    for c in line.chars() {
        if c == '\t' {
            let n = 4 - col % 4;
            out.extend(std::iter::repeat_n(' ', n));
            col += n;
        } else if c != '\r' {
            out.push(c);
            col += 1;
        }
    }
}

pub fn limit_chars(s: &mut String, max: usize) -> bool {
    match s.char_indices().nth(max) {
        Some((at, _)) => {
            s.truncate(at);
            true
        }
        None => false,
    }
}

fn utf8(b: &[u8]) -> String {
    match std::str::from_utf8(b) {
        Ok(s) => s.to_owned(),
        Err(e) if e.error_len().is_none() => String::from_utf8_lossy(&b[..e.valid_up_to()]).into_owned(),
        Err(_) => String::from_utf8_lossy(b).into_owned(),
    }
}

fn utf16(b: &[u8], unit: fn([u8; 2]) -> u16) -> String {
    char::decode_utf16(b.chunks_exact(2).map(|c| unit([c[0], c[1]]))).map(|r| r.unwrap_or('\u{FFFD}')).collect()
}

pub fn decode(b: &[u8]) -> String {
    if let Some(r) = b.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return utf8(r);
    }
    if let Some(r) = b.strip_prefix(&[0xFF, 0xFE]) {
        return utf16(r, u16::from_le_bytes);
    }
    if let Some(r) = b.strip_prefix(&[0xFE, 0xFF]) {
        return utf16(r, u16::from_be_bytes);
    }
    utf8(b)
}

pub fn looks_binary(b: &[u8]) -> bool {
    if b.starts_with(&[0xFF, 0xFE]) || b.starts_with(&[0xFE, 0xFF]) {
        return false;
    }
    let head = &b[..b.len().min(8192)];
    let control = head.iter().filter(|&&c| c < 0x20 && !matches!(c, b'\t' | b'\n' | b'\r' | 0x0C | 0x1B)).count();
    head.contains(&0) || control * 20 > head.len()
}

pub fn text_doc(src: &str, code: Option<u8>) -> Doc {
    let mut text = src.to_owned();
    let truncated = limit_chars(&mut text, TEXT_CHARS);
    let mut d = Doc { mono: true, wrap: code.is_none(), truncated, ..Default::default() };
    let (mut line, mut state) = (String::new(), None);
    for raw in text.split('\n') {
        line.clear();
        expand_tabs(raw, &mut line);
        match code {
            Some(k) => d.highlighted(&line, &SYNTAXES[k as usize], &mut state),
            None => d.run(&line, Tone::Plain, false, true),
        }
        d.para(if code.is_some() { Block::Code } else { Block::Body });
    }
    d
}

pub fn markdown_doc(src: &str) -> Doc {
    let mut text = src.to_owned();
    let truncated = limit_chars(&mut text, TEXT_CHARS);
    Doc { truncated, ..markdown::markdown(&text) }
}

pub fn encoding_of(b: &[u8]) -> &'static str {
    match b {
        [0xFF, 0xFE, ..] => "UTF-16 LE",
        [0xFE, 0xFF, ..] => "UTF-16 BE",
        [0xEF, 0xBB, 0xBF, ..] => "UTF-8 BOM",
        _ => "UTF-8",
    }
}

pub fn build(kind: Kind, bytes: &[u8]) -> Doc {
    let doc = match kind {
        Kind::Markdown => markdown_doc(&decode(bytes)),
        Kind::Code(k) => text_doc(&decode(bytes), Some(k)),
        Kind::Rtf => return Doc { encoding: "RTF", ..text_doc(&rtf_text(bytes), None) },
        _ => text_doc(&decode(bytes), None),
    };
    Doc { encoding: encoding_of(bytes), ..doc }
}

impl Doc {
    pub fn para_text(&self, k: usize) -> impl Iterator<Item = &str> {
        self.paras.get(k).map_or(0..0, |p| p.runs.clone()).map(|r| self.run_text(&self.runs[r as usize]))
    }
}

fn cp1252(c: u8) -> char {
    const HIGH: [u16; 32] = [
        0x20AC, 0x81, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160, 0x2039, 0x0152, 0x8D, 0x017D, 0x8F, 0x90, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014,
        0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0x9D, 0x017E, 0x0178,
    ];
    match c {
        0x80..=0x9F => char::from_u32(HIGH[(c - 0x80) as usize] as u32).unwrap_or('\u{FFFD}'),
        _ => c as char,
    }
}

const RTF_SKIP: &[&str] = &[
    "fonttbl", "colortbl", "stylesheet", "info", "pict", "object", "header", "footer", "headerl", "headerr", "footerl", "footerr", "footnote", "fldinst", "themedata", "colorschememapping", "datastore",
    "xmlnstbl", "listtable", "listoverridetable", "rsidtbl", "generator", "latentstyles", "filetbl", "revtbl", "pgdsctbl", "mmathPr", "wgrffmtfilter", "xe", "tc",
];

struct Rtf {
    out: String,
    chars: usize,
    skip_fallback: u32,
    high: Option<u16>,
}

impl Rtf {
    fn put(&mut self, c: char, hidden: bool) {
        if self.skip_fallback > 0 {
            self.skip_fallback -= 1;
        } else if !hidden {
            self.out.push(c);
            self.chars += 1;
        }
    }

    fn unit(&mut self, u: u16, hidden: bool) {
        match (self.high.take(), u) {
            (None, 0xD800..=0xDBFF) => self.high = Some(u),
            (Some(h), 0xDC00..=0xDFFF) => self.put(char::decode_utf16([h, u]).next().and_then(Result::ok).unwrap_or('\u{FFFD}'), hidden),
            (_, u) => self.put(char::from_u32(u as u32).unwrap_or('\u{FFFD}'), hidden),
        }
    }
}

pub fn rtf_text(b: &[u8]) -> String {
    let mut r = Rtf { out: String::new(), chars: 0, skip_fallback: 0, high: None };
    let (mut hidden, mut uc, mut stack) = (false, 1u32, Vec::new());
    let mut i = 0;
    while i < b.len() && r.chars < TEXT_CHARS {
        match b[i] {
            b'{' => {
                stack.push((hidden, uc));
                i += 1;
            }
            b'}' => {
                (hidden, uc) = stack.pop().unwrap_or((false, 1));
                i += 1;
            }
            b'\r' | b'\n' => i += 1,
            b'\\' => {
                i += 1;
                let Some(&c) = b.get(i) else { break };
                if !c.is_ascii_alphabetic() {
                    i += 1;
                    match c {
                        b'\'' => {
                            let v = b.get(i..i + 2).and_then(|h| u8::from_str_radix(std::str::from_utf8(h).ok()?, 16).ok());
                            i += 2;
                            if let Some(v) = v {
                                r.put(cp1252(v), hidden);
                            }
                        }
                        b'*' => hidden = true,
                        b'~' => r.put('\u{A0}', hidden),
                        b'_' => r.put('-', hidden),
                        b'\r' | b'\n' => r.put('\n', hidden),
                        b'-' => {}
                        other => r.put(other as char, hidden),
                    }
                    continue;
                }
                let start = i;
                while i < b.len() && b[i].is_ascii_alphabetic() {
                    i += 1;
                }
                let word = std::str::from_utf8(&b[start..i]).unwrap_or("");
                let num_start = i;
                if b.get(i) == Some(&b'-') {
                    i += 1;
                }
                while i < b.len() && b[i].is_ascii_digit() {
                    i += 1;
                }
                let param: Option<i64> = std::str::from_utf8(&b[num_start..i]).ok().and_then(|s| s.parse().ok());
                if b.get(i) == Some(&b' ') {
                    i += 1;
                }
                match word {
                    "par" | "line" | "sect" | "row" | "page" => r.put('\n', hidden),
                    "tab" | "cell" => r.put('\t', hidden),
                    "emdash" => r.put('\u{2014}', hidden),
                    "endash" => r.put('\u{2013}', hidden),
                    "bullet" => r.put('\u{2022}', hidden),
                    "lquote" => r.put('\u{2018}', hidden),
                    "rquote" => r.put('\u{2019}', hidden),
                    "ldblquote" => r.put('\u{201C}', hidden),
                    "rdblquote" => r.put('\u{201D}', hidden),
                    "uc" => uc = param.unwrap_or(1).clamp(0, 8) as u32,
                    "u" => {
                        r.unit(param.unwrap_or(0) as i16 as u16, hidden);
                        r.skip_fallback = uc;
                    }
                    "bin" => i = i.saturating_add(param.unwrap_or(0).max(0) as usize),
                    w if RTF_SKIP.contains(&w) => hidden = true,
                    _ => {}
                }
            }
            c => {
                r.put(cp1252(c), hidden);
                i += 1;
            }
        }
    }
    r.out
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FolderSummary {
    pub dirs: u64,
    pub files: u64,
    pub bytes: u64,
    pub hidden: u64,
    pub newest: i64,
    pub types: Vec<(String, u64)>,
    pub complete: bool,
    pub entries: Vec<FolderItem>,
    counts: FxHashMap<String, u64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FolderItem {
    pub name: String,
    pub dir: bool,
    pub size: u64,
    pub modified: i64,
}

impl FolderSummary {
    pub fn add(&mut self, name: &[u8], dir: bool, hidden: bool, size: u64, modified: i64) {
        self.hidden += hidden as u64;
        self.newest = self.newest.max(modified);
        if self.entries.len() < FOLDER_ROWS {
            self.entries.push(FolderItem { name: name::display(name).into_owned(), dir, size, modified });
        }
        if dir {
            self.dirs += 1;
            return;
        }
        self.files += 1;
        self.bytes += size;
        let ext = &name[name.len() - name::ext_len(name)..];
        *self.counts.entry(name::display(ext).to_ascii_lowercase()).or_default() += 1;
    }

    pub fn finish(mut self, complete: bool) -> Self {
        let mut types: Vec<(String, u64)> = std::mem::take(&mut self.counts).into_iter().collect();
        types.sort_unstable_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        types.truncate(5);
        let mut entries = std::mem::take(&mut self.entries);
        entries.sort_by_cached_key(|e| (!e.dir, e.name.to_lowercase()));
        Self { types, complete, entries, ..self }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PropGroup {
    pub title: String,
    pub rows: Vec<(String, String)>,
}

#[derive(Clone, Debug)]
pub enum PreviewData {
    Doc(Doc),
    Image(Bitmap),
    Folder(FolderSummary),
    Media { video: bool, poster: Option<Bitmap> },
    Handler(String),
    Basic { poster: Option<Bitmap>, note: Option<Note> },
    Props(Vec<PropGroup>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Note {
    Message(String),
    Archived,
    CannotPlay(String),
    Shortcut(String),
    TooLarge,
}

impl Note {
    pub fn text(&self) -> String {
        use crate::i18n::S;
        match self {
            Note::Message(m) => m.clone(),
            Note::Archived => S::PreviewArchived.t().to_owned(),
            Note::CannotPlay(error) => crate::tr!(S::PreviewCannotPlay, error = error),
            Note::Shortcut(target) => crate::tr!(S::PreviewShortcut, target = target),
            Note::TooLarge => S::PreviewTooLarge.t().to_owned(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct PreviewRequest {
    pub gen: u64,
    pub cancel: GenToken,
    pub path: FsPath,
    pub kind: Kind,
    pub size: u64,
    pub details: bool,
    pub px: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Hosted {
    Handler(String),
    Media { video: bool },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MediaCmd {
    Play,
    Pause,
    Seek(f64),
    Volume(f32),
}

#[derive(Clone, Debug, PartialEq)]
pub enum HostCmd {
    Show { gen: u64, what: Hosted, path: FsPath, rect: Option<Rect>, volume: f32, bg: u32 },
    Place(Option<Rect>),
    Media(MediaCmd),
    Close,
}

#[derive(Clone, Debug, PartialEq)]
pub enum HostEvent {
    Ready { gen: u64, ms: u32 },
    Failed { gen: u64, message: String },
    Media { gen: u64, position: f64, duration: f64, playing: bool },
}

#[cfg(test)]
#[path = "../test/preview.rs"]
mod tests;
