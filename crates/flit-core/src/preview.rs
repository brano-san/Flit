pub mod markdown;
pub mod pdf;
pub mod syntax;

use crate::hash::FxHashMap;
use crate::listing::attr;
use crate::name;
use crate::path::FsPath;
use crate::provider::GenToken;
use crate::render::Rect;
use crate::ui::Bitmap;
use std::ops::Range;
use std::sync::{Arc, Mutex, MutexGuard};
use syntax::{Syntax, SYNTAXES};

pub const TEXT_CHARS: usize = 10_000;
pub const TEXT_BYTES: usize = 64 * 1024;
pub const RTF_BYTES: usize = 1 << 20;
pub const SNIFF_LIMIT: u64 = 500 * 1024;
pub const IMAGE_PX: u32 = 1600;
pub const COPY_PX: u32 = 16384;
pub const IMAGE_FILE_LIMIT: u64 = 256 << 20;
pub const POSTER_PX: u32 = 256;
pub const FOLDER_SCAN: usize = 20_000;
pub const MAX_PROPS: usize = 120;
pub const FOLDER_ROWS: usize = 2_000;
pub const PRETTY_FILE_LIMIT: u64 = 8 << 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrettyFormat {
    Json,
    Yaml,
    Toml,
    Xml,
}

impl PrettyFormat {
    pub fn for_ext(ext: &[u8]) -> Option<Self> {
        [(b"json".as_slice(), Self::Json), (b"yaml", Self::Yaml), (b"yml", Self::Yaml), (b"toml", Self::Toml), (b"xml", Self::Xml)].into_iter().find_map(|(suffix, format)| ext.eq_ignore_ascii_case(suffix).then_some(format))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrettyError {
    Invalid,
    TooLarge,
    Unavailable,
}

pub const IMAGES: &[&str] = &["jpg", "jpeg", "jpe", "jfif", "png", "gif", "bmp", "dib", "tif", "tiff", "ico", "webp", "heic", "heif", "avif", "jxr", "wdp", "dds", "jxl"];
const RAW: &[&[u8]] = &[b"dng", b"cr2", b"nef", b"nrw", b"arw", b"sr2", b"pef", b"rw2", b"orf"];

pub fn is_raw(ext: &[u8]) -> bool {
    RAW.iter().any(|s| ext.eq_ignore_ascii_case(s))
}

pub fn is_svg(ext: &[u8]) -> bool {
    ext.eq_ignore_ascii_case(b"svg") || ext.eq_ignore_ascii_case(b"svgz")
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SvgBackground {
    #[default]
    Checker,
    White,
    Black,
}

impl SvgBackground {
    pub(super) const ALL: [Self; 3] = [Self::Checker, Self::White, Self::Black];

    pub(super) fn label(self) -> crate::i18n::S {
        use crate::i18n::S;
        match self {
            Self::Checker => S::PreviewSvgChecker,
            Self::White => S::PreviewSvgWhite,
            Self::Black => S::PreviewSvgBlack,
        }
    }
}
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

pub fn waits_for_download(attrs: u32) -> bool {
    attrs & (attr::DIR | attr::VOLUME) == 0 && attrs & attr::NOT_LOCAL != 0
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

pub fn kind_of(file_name: &[u8]) -> Kind {
    let ext = &file_name[file_name.len() - name::ext_len(file_name)..];
    match kind(ext) {
        Kind::Text | Kind::Other => syntax::by_file_name(&name::display(file_name)).map_or_else(|| kind(ext), Kind::Code),
        k => k,
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
    pub strike: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Block {
    Body,
    Heading(u8),
    Code,
    Quote,
    Item { depth: u8 },
    Rule,
    Table(usize),
    Footnote,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Alignment {
    Left,
    Center,
    Right,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Table {
    pub align: Vec<Alignment>,
    pub rows: Vec<Vec<Range<u32>>>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Para {
    pub block: Block,
    pub quote_depth: u8,
    pub runs: Range<u32>,
}

#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct Doc {
    pub text: String,
    pub runs: Vec<Run>,
    pub paras: Vec<Para>,
    pub tables: Vec<Table>,
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
        self.styled_run(s, tone, bold, mono, false);
    }

    fn styled_run(&mut self, s: &str, tone: Tone, bold: bool, mono: bool, strike: bool) {
        if s.is_empty() {
            return;
        }
        let start = self.text.len() as u32;
        self.text.push_str(s);
        let end = self.text.len() as u32;
        let open = self.runs.len() > self.open_start() as usize;
        match self.runs.last_mut() {
            Some(r) if open && r.end == start && r.tone == tone && r.bold == bold && r.mono == mono && r.strike == strike => r.end = end,
            _ => self.runs.push(Run { start, end, tone, bold, mono, strike }),
        }
    }

    pub fn para(&mut self, block: Block) {
        self.para_with_quote_depth(block, u8::from(block == Block::Quote));
    }

    fn para_with_quote_depth(&mut self, block: Block, quote_depth: u8) {
        let start = self.open_start();
        self.paras.push(Para { block, quote_depth, runs: start..self.runs.len() as u32 });
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

pub fn straight_rgba(rgba: &[u8]) -> Vec<u8> {
    let unmix = |c: u8, a: u8| ((u32::from(c) * 255 + u32::from(a) / 2) / u32::from(a)).min(255) as u8;
    rgba.chunks_exact(4)
        .flat_map(|p| match p[3] {
            0 => [0, 0, 0, 0],
            255 => [p[0], p[1], p[2], 255],
            a => [unmix(p[0], a), unmix(p[1], a), unmix(p[2], a), a],
        })
        .collect()
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
    let kind = match kind {
        Kind::Text => syntax::by_shebang(bytes).map_or(kind, Kind::Code),
        k => k,
    };
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
    const HIGH: [u16; 32] =
        [0x20AC, 0x81, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160, 0x2039, 0x0152, 0x8D, 0x017D, 0x8F, 0x90, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014, 0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0x9D, 0x017E, 0x0178];
    match c {
        0x80..=0x9F => char::from_u32(HIGH[(c - 0x80) as usize] as u32).unwrap_or('\u{FFFD}'),
        _ => c as char,
    }
}

const RTF_SKIP: &[&str] = &[
    "fonttbl",
    "colortbl",
    "stylesheet",
    "info",
    "pict",
    "object",
    "header",
    "footer",
    "headerl",
    "headerr",
    "footerl",
    "footerr",
    "footnote",
    "fldinst",
    "themedata",
    "colorschememapping",
    "datastore",
    "xmlnstbl",
    "listtable",
    "listoverridetable",
    "rsidtbl",
    "generator",
    "latentstyles",
    "filetbl",
    "revtbl",
    "pgdsctbl",
    "mmathPr",
    "wgrffmtfilter",
    "xe",
    "tc",
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
    Pdf(Vec<Option<pdf::Size>>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Note {
    Message(String),
    Archived,
    CannotPlay(String),
    Shortcut(String),
    TooLarge,
    OnlineOnly,
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
            Note::OnlineOnly => S::PreviewOnlineOnly.t().to_owned(),
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

#[derive(Clone, Default)]
pub struct Frames(Arc<Mutex<Option<(u64, Bitmap)>>>);

impl Frames {
    pub fn put(&self, gen: u64, frame: Bitmap) -> bool {
        self.slot().replace((gen, frame)).is_none()
    }

    pub fn take(&self) -> Option<(u64, Bitmap)> {
        self.slot().take()
    }

    fn slot(&self) -> MutexGuard<'_, Option<(u64, Bitmap)>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl PartialEq for Frames {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl std::fmt::Debug for Frames {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Frames")
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum HostEvent {
    Ready { gen: u64, ms: u32 },
    Failed { gen: u64, message: String },
    Media { gen: u64, position: f64, duration: f64, playing: bool },
    Frame(Frames),
    Released { gen: u64 },
}

pub fn elf_program(head: &[u8]) -> bool {
    read_elf_program(&mut std::io::Cursor::new(head))
}

pub fn read_elf_program(reader: &mut (impl std::io::Read + std::io::Seek)) -> bool {
    use std::io::SeekFrom;

    let mut header = [0; 64];
    if reader.read_exact(&mut header[..52]).is_err() || &header[..4] != b"\x7fELF" || !matches!(header[4], 1 | 2) || !matches!(header[5], 1 | 2) {
        return false;
    }
    let wide = header[4] == 2;
    if wide && reader.read_exact(&mut header[52..]).is_err() {
        return false;
    }
    let little = header[5] == 1;
    let number = |bytes: &[u8]| -> u64 {
        if little {
            bytes.iter().rev().fold(0, |n, &b| n << 8 | u64::from(b))
        } else {
            bytes.iter().fold(0, |n, &b| n << 8 | u64::from(b))
        }
    };
    match number(&header[16..18]) {
        2 => return true,
        3 => {}
        _ => return false,
    }
    let (offset, stride, count, word) = if wide { (number(&header[32..40]), number(&header[54..56]), number(&header[56..58]), 8) } else { (number(&header[28..32]), number(&header[42..44]), number(&header[44..46]), 4) };
    if stride != if wide { 56 } else { 32 } || count > 128 || reader.seek(SeekFrom::Start(offset)).is_err() {
        return false;
    }
    let mut entry = [0; 56];
    let mut dynamic = None;
    for _ in 0..count {
        if reader.read_exact(&mut entry[..stride as usize]).is_err() {
            return false;
        }
        match number(&entry[..4]) {
            3 => return true,
            2 => dynamic = Some((number(&entry[word..word * 2]), number(&entry[word * 4..word * 5]))),
            _ => {}
        }
    }
    let Some((offset, size)) = dynamic else { return false };
    if reader.seek(SeekFrom::Start(offset)).is_err() {
        return false;
    }
    for _ in 0..(size / (2 * word as u64)).min(256) {
        if reader.read_exact(&mut entry[..word * 2]).is_err() {
            break;
        }
        match number(&entry[..word]) {
            0 => break,
            0x6fff_fffb => return number(&entry[word..word * 2]) & 0x0800_0000 != 0,
            _ => {}
        }
    }
    false
}

#[cfg(test)]
#[path = "../test/preview.rs"]
mod tests;
