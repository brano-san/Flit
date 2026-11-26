mod content;
mod paint;

mod document_layout;
mod pdf_view;

use document_layout::{Grid, TextFlow};
use super::field::{Field, TextCmd};
use super::menu::{Cmd, Mark};
use super::options::{InspectorAt, INSPECTOR_SIZE};
use super::widgets::Paint;
use super::{centered, icon_key, App, Bitmap, Drag, Effect, Focus, Hit, IconRequest};
use crate::fmt as format;
use crate::hash::{self, FxHashMap};
use crate::input::{CursorIcon, Key, Mods, MouseButton};
use crate::listing::{attr, Entry};
use crate::name;
use crate::path::{FsPath, PathKind};
use crate::i18n::{P, S};
use crate::preview::{self, Block, Doc, FolderSummary, HostCmd, HostEvent, Hosted, Kind, MediaCmd, Note, PreviewData, PreviewRequest, PropGroup, Run, Tone, COPY_PX, IMAGE_PX};
use crate::provider::GenToken;
use crate::recycle;
use crate::render::{Color, Rect};
use crate::text::{FontFamily, FontStyle, Gfx, TextBackend};
use crate::theme::{self, tone_color, Theme};
use serde::{Deserialize, Serialize};
use std::fmt::Write as _;
use std::ops::Range;
use std::time::{Duration, Instant};

const MIN_RIGHT: f32 = 200.0;
const MIN_BOTTOM: f32 = 120.0;
pub const HALF: f32 = 0.0;
const OLD_RIGHT_W: f32 = 340.0;
const OLD_BOTTOM_H: f32 = 260.0;
const MAX_SIDE: f32 = 4000.0;
const TOOLS_H: f32 = 30.0;
const FOOT_H: f32 = 24.0;
const TOOL_W: f32 = 26.0;
const PAD: f32 = 12.0;
const ROW: f32 = 22.0;
const ICON_PX: f32 = 48.0;
const BUTTON_H: f32 = 28.0;
const CONTROLS_H: f32 = 34.0;
const CARD_W: f32 = 320.0;
const CHECKER: f32 = 10.0;
const ZOOM_STEP: f32 = 1.25;
const ZOOM_RANGE: (f32, f32) = (0.05, 16.0);
const KEEP_PREVIEW_FOR: Duration = Duration::from_secs(60);
const PREVIOUS_PREVIEW_BYTES: usize = 12 << 20;
const G_PLAY: &str = "\u{E768}";
const G_PAUSE: &str = "\u{E769}";
const G_VOLUME: &str = "\u{E767}";
const G_CLOSE: &str = "\u{E8BB}";
const G_AUTO: &str = "\u{E72C}";
const G_BELOW: &str = "\u{E70D}";
const G_WRAP: &str = "\u{E7A6}";
const G_LINES: &str = "\u{E8FD}";
const G_SPACES: &str = "\u{E8D2}";
const G_FIT: &str = "\u{E922}";
const G_CUSTOM: &str = "\u{E721}";
const G_GRID: &str = "\u{E8A9}";
const G_DETAILS_OFF: &str = "\u{E946}";
const G_DETAILS_ON: &str = "\u{E8A5}";
const SPACE_DOT: &str = "\u{B7}";
const AT_CHOICES: [InspectorAt; 3] = [InspectorAt::Auto, InspectorAt::Right, InspectorAt::Below];

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PaneState {
    pub visible: bool,
    pub details: bool,
    pub right_w: f32,
    pub bottom_h: f32,
    pub volume: f32,
}

impl Default for PaneState {
    fn default() -> Self {
        Self { visible: false, details: false, right_w: HALF, bottom_h: HALF, volume: 0.8 }
    }
}

impl PaneState {
    pub fn sanitized(self) -> Self {
        let d = Self::default();
        let fix = |v: f32, min: f32, old: f32| if v.is_finite() && (min..=MAX_SIDE).contains(&v) && v != old { v } else { HALF };
        let volume = if (0.0..=1.0).contains(&self.volume) { self.volume } else { d.volume };
        Self { right_w: fix(self.right_w, MIN_RIGHT, OLD_RIGHT_W), bottom_h: fix(self.bottom_h, MIN_BOTTOM, OLD_BOTTOM_H), volume, ..self }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Scale {
    #[default]
    Fit,
    Original,
    Custom,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PaneTool {
    Wrap,
    Lines,
    Spaces,
    Scale(Scale),
    Grid,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PaneHit {
    Split,
    Close,
    At(InspectorAt),
    Tool(PaneTool),
    Title,
    CardDetails,
    CardClose,
    Card,
    Properties,
    Filter,
    FilterClear,
    Size,
    Play,
    Seek,
    Volume,
    Tag(usize),
    NewTag,
    Download,
    Pretty,
    SvgBackground,
    Body,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    pub wrap: Option<bool>,
    pub lines: bool,
    pub spaces: bool,
    pub grid: bool,
    pub scale: Scale,
    pub zoom: f32,
    pub at: InspectorAt,
    pub card: bool,
}

impl Default for Look {
    fn default() -> Self {
        Self { wrap: None, lines: true, spaces: false, grid: true, scale: Scale::Fit, zoom: 1.0, at: InspectorAt::Auto, card: false }
    }
}

#[derive(Clone)]
struct Subject {
    title: String,
    path: Option<FsPath>,
    kind: Kind,
    entry: Option<Entry>,
    ext: Vec<u8>,
    dir_key: Vec<u8>,
    many: Option<(usize, u64)>,
    drive: Option<(u64, u64)>,
    listing_gen: Option<u64>,
}

impl Subject {
    fn key(&self) -> u64 {
        let e = self.entry.map(|e| (e.size, e.modified, e.attrs));
        hash::hash(&(&self.title, self.path.as_ref().map(|p| p.as_bytes()), self.kind, e, self.many, self.drive, self.listing_gen))
    }
}

#[derive(Default)]
enum Content {
    #[default]
    Empty,
    Loading,
    Doc(Doc),
    Image,
    Folder(FolderSummary),
    Media {
        video: bool,
    },
    Handler,
    Basic(Option<Note>),
    Pdf(pdf_view::PdfView),
}

struct PreviousPreview {
    key: u64,
    until: Instant,
    content: Content,
    image: Option<Bitmap>,
    px: u32,
    pretty: Pretty,
}

#[derive(Default)]
enum Pretty {
    #[default]
    Off,
    Loading,
    On(Doc),
}

#[derive(Default)]
struct Host {
    gen: u64,
    placed: Option<Rect>,
    video: bool,
    handler: bool,
    started: bool,
}

#[derive(Default)]
struct Media {
    position: f64,
    duration: f64,
    playing: bool,
}

#[derive(Clone, Copy)]
struct Piece {
    x: f32,
    start: u32,
    end: u32,
    run: u32,
}

struct VLine {
    y: f32,
    h: f32,
    base: f32,
    block: Block,
    num: u32,
    para: u32,
    pieces: Range<u32>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct Spot {
    pub para: u32,
    pub at: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Grain {
    #[default]
    Char,
    Word,
    Line,
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Pick {
    anchor: Spot,
    caret: Spot,
    from: (Spot, Spot),
    grain: Grain,
}

impl Pick {
    fn range(&self) -> (Spot, Spot) {
        (self.anchor.min(self.caret), self.anchor.max(self.caret))
    }
}

#[derive(Default)]
struct DocLayout {
    key: u64,
    lines: Vec<VLine>,
    pieces: Vec<Piece>,
    grids: Vec<Grid>,
    height: f32,
    wide: f32,
    shown: usize,
}

type FootBlock = (&'static str, String, Option<PaneHit>);

#[derive(Clone, Copy, Default)]
struct Rects {
    pane: Rect,
    split: Rect,
    tools: Rect,
    title: Rect,
    body: Rect,
    foot: Rect,
    card: Rect,
    button: Rect,
    play: Rect,
    seek: Rect,
    volume: Rect,
    media: Rect,
    filter: Rect,
    gutter: f32,
}

#[derive(Default)]
pub(super) struct PreviewPane {
    pub state: PaneState,
    pub look: Look,
    pub turn: super::widgets::Tween,
    foot_slide: super::widgets::Tween,
    key: u64,
    gen: u64,
    cancel: GenToken,
    subject: Option<Subject>,
    content: Content,
    scroll: f32,
    extent: f32,
    hscroll: f32,
    hmax: f32,
    card_scroll: f32,
    card_extent: f32,
    doc: Option<DocLayout>,
    pick: Pick,
    widths: FxHashMap<(char, FontStyle), f32>,
    image: Option<Bitmap>,
    image_rev: u64,
    image_px: u32,
    copy_gen: u64,
    host: Host,
    released: bool,
    media: Media,
    peeked: bool,
    requested: Option<Instant>,
    props: Vec<PropGroup>,
    props_gen: u64,
    props_key: u64,
    props_loading: bool,
    types: Vec<(String, usize)>,
    filter: Option<Field>,
    filtering: bool,
    room: (f32, f32),
    r: Rects,
    chips: Vec<(Rect, PaneHit)>,
    buttons: Vec<(Rect, PaneHit)>,
    download: u64,
    previous: Option<PreviousPreview>,
    pretty: Pretty,
}

impl PreviewPane {
    fn replace_text(&mut self, doc: Doc) {
        self.content = Content::Doc(doc);
        self.doc = None;
        self.pick = Pick::default();
        self.scroll = 0.0;
        self.hscroll = 0.0;
    }

    fn previous_size(&self) -> Option<usize> {
        let content = match &self.content {
            Content::Doc(d) => doc_capacity(d),
            Content::Image => 0,
            Content::Folder(f) => f.entries.capacity() * size_of::<preview::FolderItem>() + f.entries.iter().map(|e| e.name.capacity()).sum::<usize>() + f.types.capacity() * size_of::<(String, u64)>() + f.types.iter().map(|(t, _)| t.capacity()).sum::<usize>(),
            Content::Basic(None) if self.image.is_some() => 0,
            Content::Basic(Some(Note::Shortcut(target))) => target.capacity(),
            Content::Pdf(pdf) => pdf.retained_bytes(),
            _ => return None,
        };
        let raw = if let Pretty::On(raw) = &self.pretty { doc_capacity(raw) } else { 0 };
        Some(content + raw + self.image.as_ref().map_or(0, |b| b.rgba.capacity()))
    }

    fn held(&self) -> usize {
        let image = self.image.as_ref().map_or(0, |b| b.rgba.len() * 2);
        let doc = match &self.content {
            Content::Doc(d) => d.text.len() + d.runs.len() * size_of::<Run>() + d.paras.len() * size_of::<preview::Para>(),
            _ => 0,
        };
        let lay = self.doc.as_ref().map_or(0, |l| l.lines.len() * size_of::<VLine>() + l.pieces.len() * size_of::<Piece>());
        image + doc + lay
    }

    fn query(&self) -> String {
        self.filter.as_ref().map(|f| f.text().to_lowercase()).unwrap_or_default()
    }

    fn effective_wrap(&self, d: &Doc) -> bool {
        if d.mono {
            self.look.wrap.unwrap_or(d.wrap)
        } else {
            true
        }
    }
}

fn doc_capacity(d: &Doc) -> usize {
    d.text.capacity()
        + d.runs.capacity() * size_of::<Run>()
        + d.paras.capacity() * size_of::<preview::Para>()
        + d.tables.capacity() * size_of::<preview::Table>()
        + d.tables.iter().map(|t| t.align.capacity() * size_of::<preview::Alignment>() + t.rows.capacity() * size_of::<Vec<Range<u32>>>() + t.rows.iter().map(|r| r.capacity() * size_of::<Range<u32>>()).sum::<usize>()).sum::<usize>()
}

fn ui(px: f32, sc: f32, bold: bool) -> FontStyle {
    FontStyle::new(FontFamily::Ui, px * sc, bold)
}

fn run_font(r: &Run, block: Block, sc: f32, mono: f32) -> FontStyle {
    match block {
        Block::Footnote if r.mono => FontStyle::new(FontFamily::Mono, mono * 0.85 * sc, r.bold),
        Block::Footnote => ui(11.0, sc, r.bold),
        _ if r.mono => FontStyle::new(FontFamily::Mono, mono * sc, r.bold),
        Block::Heading(1) => ui(20.0, sc, true),
        Block::Heading(2) => ui(17.0, sc, true),
        Block::Heading(_) => ui(14.5, sc, true),
        _ => ui(13.0, sc, r.bold),
    }
}

fn para_font(block: Block, mono: bool, sc: f32, mono_px: f32) -> FontStyle {
    let r = Run { start: 0, end: 0, tone: Tone::Plain, bold: false, mono: mono || block == Block::Code, strike: false };
    run_font(&r, block, sc, mono_px)
}

fn char_w<B: TextBackend>(g: &mut Gfx<B>, widths: &mut FxHashMap<(char, FontStyle), f32>, c: char, font: FontStyle) -> f32 {
    *widths.entry((c, font)).or_insert_with(|| g.measure(c.encode_utf8(&mut [0; 4]), font))
}

fn put(pieces: &mut Vec<Piece>, first: usize, run: u32, start: u32, end: u32, x: f32) {
    let open = pieces.len() > first;
    match pieces.last_mut() {
        Some(p) if open && p.run == run && p.end == start => p.end = end,
        _ => pieces.push(Piece { x, start, end, run }),
    }
}

pub(super) fn line_matches(d: &Doc, k: usize, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    if let Some(Block::Table(i)) = d.paras.get(k).map(|p| p.block) {
        return d.tables[i].rows.iter().flatten().any(|cell| {
            if cell.is_empty() {
                return false;
            }
            let first = &d.runs[cell.start as usize];
            let last = &d.runs[cell.end as usize - 1];
            d.text[first.start as usize..last.end as usize].to_lowercase().contains(query)
        });
    }
    let text: String = d.para_text(k).collect();
    text.to_lowercase().contains(query)
}

pub(super) fn para_span(d: &Doc, k: usize) -> (u32, u32) {
    let all = d.text.len() as u32;
    let Some(p) = d.paras.get(k) else { return (all, all) };
    let start = d.runs.get(p.runs.start as usize).map_or(all, |r| r.start);
    let end = if p.runs.is_empty() { start } else { d.runs.get(p.runs.end as usize - 1).map_or(start, |r| r.end) };
    (start, end)
}

fn word_class(c: char) -> u8 {
    if c.is_alphanumeric() || c == '_' {
        0
    } else if c.is_whitespace() {
        1
    } else {
        2
    }
}

pub(super) fn word_at(text: &str, pos: usize) -> Range<usize> {
    let pos = (0..=pos.min(text.len())).rev().find(|&p| text.is_char_boundary(p)).unwrap_or(0);
    let (after, before) = (text[pos..].chars().next().map(word_class), text[..pos].chars().next_back().map(word_class));
    let class = match (after, before) {
        (Some(0), _) | (_, Some(0)) => 0,
        (Some(c), _) | (None, Some(c)) => c,
        (None, None) => return pos..pos,
    };
    let start = text[..pos].char_indices().rev().take_while(|&(_, c)| word_class(c) == class).last().map_or(pos, |(i, _)| i);
    let end = text[pos..].char_indices().find(|&(_, c)| word_class(c) != class).map_or(text.len(), |(i, _)| pos + i);
    start..end
}

pub(super) fn picked_text(d: &Doc, (a, b): (Spot, Spot), query: &str, line_break: &str) -> String {
    let mut out = String::new();
    if a == b {
        return out;
    }
    let last = (b.para as usize).min(d.paras.len().saturating_sub(1));
    let mut first = true;
    for k in a.para as usize..=last {
        if k >= d.paras.len() || !line_matches(d, k, query) {
            continue;
        }
        let (s, e) = para_span(d, k);
        let lo = if k == a.para as usize { a.at.clamp(s, e) } else { s };
        let hi = if k == b.para as usize { b.at.clamp(lo, e) } else { e };
        if !first {
            out.push_str(line_break);
        }
        first = false;
        out.push_str(d.text.get(lo as usize..hi as usize).unwrap_or_default());
    }
    out
}

fn line_span_x(d: &Doc, lay: &DocLayout, line: &VLine, widths: &FxHashMap<(char, FontStyle), f32>, (sc, mono): (f32, f32), (lo, hi): (u32, u32)) -> Option<(f32, f32)> {
    let mut span: Option<(f32, f32)> = None;
    for piece in &lay.pieces[line.pieces.start as usize..line.pieces.end as usize] {
        let (s, e) = (piece.start.max(lo), piece.end.min(hi));
        if s >= e {
            continue;
        }
        let font = run_font(&d.runs[piece.run as usize], line.block, sc, mono);
        let w = |a: u32, b: u32| d.text[a as usize..b as usize].chars().map(|c| widths.get(&(c, font)).copied().unwrap_or(0.0)).sum::<f32>();
        let x = piece.x + w(piece.start, s);
        let x2 = x + w(s, e);
        span = Some(span.map_or((x, x2), |(a, b)| (a.min(x), b.max(x2))));
    }
    span
}

fn line_end_x(d: &Doc, lay: &DocLayout, line: &VLine, widths: &FxHashMap<(char, FontStyle), f32>, fonts: (f32, f32)) -> f32 {
    let Some(last) = lay.pieces[line.pieces.start as usize..line.pieces.end as usize].last() else { return 0.0 };
    line_span_x(d, lay, line, widths, fonts, (last.start, last.end)).map_or(last.x, |s| s.1)
}

fn line_text(d: &Doc, lay: &DocLayout, line: &VLine) -> (u32, u32) {
    let pieces = &lay.pieces[line.pieces.start as usize..line.pieces.end as usize];
    match (pieces.first(), pieces.last()) {
        (Some(a), Some(b)) => (a.start, b.end),
        _ => para_span(d, line.para as usize),
    }
}

fn caret_line(d: &Doc, lay: &DocLayout, s: Spot) -> Option<usize> {
    let of_para = |l: &VLine| l.para == s.para;
    lay.lines.iter().position(|l| of_para(l) && line_text(d, lay, l).1 >= s.at).or_else(|| lay.lines.iter().rposition(of_para))
}

fn step_char(d: &Doc, lay: &DocLayout, s: Spot, right: bool) -> Spot {
    let (start, end) = para_span(d, s.para as usize);
    let text = d.text.as_str();
    if right && s.at < end {
        return Spot { para: s.para, at: s.at + text[s.at as usize..].chars().next().map_or(1, char::len_utf8) as u32 };
    }
    if !right && s.at > start {
        return Spot { para: s.para, at: s.at - text[..s.at as usize].chars().next_back().map_or(1, char::len_utf8) as u32 };
    }
    let next = if right { lay.lines.iter().find(|l| l.para > s.para) } else { lay.lines.iter().rev().find(|l| l.para < s.para) };
    next.map_or(s, |l| {
        let (a, b) = para_span(d, l.para as usize);
        Spot { para: l.para, at: if right { a } else { b } }
    })
}

fn spot_on_line(d: &Doc, lay: &DocLayout, line: &VLine, widths: &FxHashMap<(char, FontStyle), f32>, (sc, mono): (f32, f32), x: f32) -> Spot {
    let para = line.para;
    let mut at = para_span(d, para as usize).0;
    for piece in &lay.pieces[line.pieces.start as usize..line.pieces.end as usize] {
        let font = run_font(&d.runs[piece.run as usize], line.block, sc, mono);
        let mut px = piece.x;
        for (i, c) in d.text[piece.start as usize..piece.end as usize].char_indices() {
            let w = widths.get(&(c, font)).copied().unwrap_or(0.0);
            if x < px + w / 2.0 {
                return Spot { para, at: piece.start + i as u32 };
            }
            px += w;
        }
        at = piece.end;
    }
    Spot { para, at }
}

fn layout_doc<B: TextBackend>(g: &mut Gfx<B>, d: &Doc, width: f32, (sc, mono): (f32, f32), widths: &mut FxHashMap<(char, FontStyle), f32>, wrap: bool, query: &str) -> DocLayout {
    let mut out = DocLayout::default();
    let mut y = 0.0;
    for (k, para) in d.paras.iter().enumerate() {
        if !line_matches(d, k, query) {
            continue;
        }
        out.shown += 1;
        let block = para.block;
        let num = k as u32 + 1;
        let base_font = para_font(block, d.mono, sc, mono);
        let m = g.metrics(base_font);
        let lh = ((m.ascent + m.descent) * if d.mono { 1.25 } else { 1.4 }).round();
        let base = ((lh + m.ascent - m.descent) / 2.0).round();
        if block == Block::Rule {
            out.lines.push(VLine { y, h: (12.0 * sc).round(), base, block, num, para: k as u32, pieces: 0..0 });
            y += (12.0 * sc).round();
            continue;
        }
        let mut flow = TextFlow { gfx: g, doc: d, widths, scale: sc, mono, block };
        let quote_indent = (14.0 * sc * f32::from(para.quote_depth)).round().min(width * 0.5);
        if let Block::Table(index) = block {
            flow.table(index, k as u32, quote_indent..width, (lh, base), &mut y, &mut out);
            continue;
        }
        let indent = quote_indent
            + match block {
                Block::Item { depth } => ((depth as f32 * 18.0 + 4.0) * sc).round(),
                Block::Code if !d.mono => (8.0 * sc).round(),
                _ => 0.0,
            };
        let mut number = num;
        flow.lines(para.runs.clone(), indent..width, wrap, &mut out.pieces, |pieces, end| {
            out.lines.push(VLine { y, h: lh, base, block, num: number, para: k as u32, pieces });
            out.wide = out.wide.max(end);
            y += lh;
            number = 0;
        });
        let next = d.paras.get(k + 1).map(|p| p.block);
        if !d.mono && !(block == Block::Code && next == Some(Block::Code)) {
            y += (6.0 * sc).round();
        }
    }
    out.height = y;
    out
}

fn clock(out: &mut String, secs: f64) {
    let s = secs.max(0.0) as u64;
    let _ = if s >= 3600 { write!(out, "{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { write!(out, "{}:{:02}", s / 60, s % 60) };
}

pub(super) fn image_factor(scale: Scale, zoom: f32, image: (f32, f32), body: (f32, f32)) -> f32 {
    match scale {
        Scale::Fit => (body.0 / image.0.max(1.0)).min(body.1 / image.1.max(1.0)).min(1.0),
        Scale::Original => 1.0,
        Scale::Custom => zoom,
    }
}

pub(super) fn side_is_right(at: InspectorAt, room: (f32, f32)) -> bool {
    match at {
        InspectorAt::Right => true,
        InspectorAt::Below => false,
        InspectorAt::Auto => room.0 >= room.1,
    }
}

impl App {
    pub fn pane_state(&self) -> PaneState {
        self.preview.state
    }

    pub fn set_pane_state(&mut self, s: PaneState) {
        self.preview.state = s.sanitized();
        self.preview.look.card = self.preview.state.details;
        self.layout();
        self.dirty = true;
    }

    pub fn inspector_look(&self) -> Look {
        self.preview.look
    }

    pub fn preview_image(&self) -> (u64, Option<&Bitmap>) {
        (self.preview.image_rev, self.preview.image.as_ref())
    }

    pub(super) fn preview_area(&self) -> Rect {
        if self.preview.state.visible {
            self.preview.r.pane
        } else {
            Rect::default()
        }
    }

    fn show_inspector(&mut self, on: bool) {
        let p = &mut self.preview;
        if on && !p.state.visible {
            p.look.at = self.settings.opts.inspector_at;
        }
        p.state.visible = on;
        if !on {
            p.filtering = false;
            if self.focus == Focus::Preview {
                self.focus = Focus::List;
            }
        }
        self.layout();
        self.dirty = true;
    }

    pub(super) fn toggle_preview_pane(&mut self) {
        self.preview.peeked = false;
        let on = !self.preview.state.visible;
        self.show_inspector(on);
    }

    pub(super) fn show_preview_tab(&mut self, details: bool) {
        let p = &mut self.preview;
        p.peeked = false;
        if p.state.visible && p.look.card == details {
            return self.show_inspector(false);
        }
        p.look.card = details;
        p.state.details = details;
        self.show_inspector(true);
    }

    pub(super) fn toggle_inspector_details(&mut self) {
        let p = &mut self.preview;
        p.state.details = !p.state.details || !p.state.visible || !p.look.card;
        p.look.card = true;
        self.show_inspector(true);
    }

    pub(super) fn inspector_right(&self) -> bool {
        side_is_right(self.preview.look.at, self.preview.room)
    }

    pub(super) fn toggle_inspector_layout(&mut self) {
        let right = self.inspector_right();
        self.preview.look.at = if right { InspectorAt::Below } else { InspectorAt::Right };
        self.show_inspector(true);
    }

    fn set_inspector_at(&mut self, at: InspectorAt) {
        self.preview.look.at = at;
        self.layout();
        self.dirty = true;
    }

    pub(super) fn shows_image(&self) -> bool {
        matches!(self.preview.content, Content::Image) && self.preview.image.is_some()
    }

    fn shows_doc(&self) -> bool {
        matches!(self.preview.content, Content::Doc(_))
    }

    fn shows_folder(&self) -> bool {
        matches!(self.preview.content, Content::Folder(_))
    }

    pub(super) fn inspector_available(&self, id: &str) -> bool {
        let shown = self.preview.state.visible;
        match id {
            "inspector.wrap" | "inspector.line_numbers" | "inspector.whitespace" => shown && self.shows_doc(),
            "inspector.grid" => shown && self.shows_image() && !self.preview_is_svg(),
            "inspector.scale" => shown && self.shows_image(),
            "inspector.filter" => shown && (self.shows_doc() || self.shows_folder()),
            "inspector.zoom_in" | "inspector.zoom_out" => shown && (self.shows_doc() || self.shows_image()),
            _ => true,
        }
    }

    pub(super) fn inspector_zoom(&mut self, up: bool) {
        if self.shows_image() {
            let (w, h) = self.preview.image.as_ref().map_or((1.0, 1.0), |i| (i.width as f32, i.height as f32));
            let b = self.preview.r.body;
            let now = image_factor(self.preview.look.scale, self.preview.look.zoom, (w, h), (b.w, b.h));
            let next = if up { now * ZOOM_STEP } else { now / ZOOM_STEP };
            let look = &mut self.preview.look;
            look.zoom = next.clamp(ZOOM_RANGE.0, ZOOM_RANGE.1);
            look.scale = Scale::Custom;
        } else {
            let size = i64::from(self.settings.opts.inspector_size) + if up { 1 } else { -1 };
            self.set_inspector_size(size.clamp(INSPECTOR_SIZE.0, INSPECTOR_SIZE.2) as u8);
            self.preview.doc = None;
        }
        self.dirty = true;
    }

    pub(super) fn inspector_toggle(&mut self, tool: PaneTool) {
        let wrap = match &self.preview.content {
            Content::Doc(d) => self.preview.effective_wrap(d),
            _ => true,
        };
        let look = &mut self.preview.look;
        match tool {
            PaneTool::Wrap => look.wrap = Some(!wrap),
            PaneTool::Lines => look.lines = !look.lines,
            PaneTool::Spaces => look.spaces = !look.spaces,
            PaneTool::Grid => look.grid = !look.grid,
            PaneTool::Scale(s) => look.scale = s,
        }
        self.preview.doc = None;
        self.dirty = true;
    }

    pub(super) fn cycle_inspector_scale(&mut self) {
        let next = if self.preview.look.scale == Scale::Fit { Scale::Original } else { Scale::Fit };
        self.inspector_toggle(PaneTool::Scale(next));
    }

    pub(super) fn open_inspector_filter(&mut self) {
        if !self.preview.state.visible {
            self.show_inspector(true);
        }
        let p = &mut self.preview;
        p.filter.get_or_insert_with(Field::default).edit.select_all();
        p.filtering = true;
        self.focus = Focus::Preview;
        self.dirty = true;
    }

    fn close_inspector_filter(&mut self) {
        let p = &mut self.preview;
        p.filter = None;
        p.filtering = false;
        p.doc = None;
        p.scroll = 0.0;
        if self.focus == Focus::Preview {
            self.focus = Focus::List;
        }
        self.dirty = true;
    }

    pub(super) fn inspector_key(&mut self, key: Key, m: Mods) -> bool {
        if !self.preview.state.visible {
            return false;
        }
        if !self.preview.filtering {
            return self.preview_key(key, m);
        }
        match key {
            Key::Escape => self.close_inspector_filter(),
            Key::Enter | Key::Tab => {
                self.preview.filtering = false;
                self.focus = Focus::List;
            }
            _ => {
                let Some(f) = self.preview.filter.as_mut() else { return false };
                match f.key(key, m) {
                    super::field::FieldKey::Ignored => return false,
                    super::field::FieldKey::Edited { changed, action } => {
                        match action {
                            super::edit::EditAction::Copy(s) => self.effects.push(Effect::SetClipboard(s)),
                            super::edit::EditAction::Paste => self.effects.push(Effect::Paste),
                            _ => {}
                        }
                        if changed {
                            self.inspector_filter_changed();
                        }
                    }
                }
            }
        }
        self.dirty = true;
        true
    }

    pub(super) fn inspector_insert(&mut self, s: &str) -> bool {
        if !self.preview.filtering || !self.preview.state.visible {
            return false;
        }
        if let Some(f) = self.preview.filter.as_mut() {
            f.insert(s);
        }
        self.inspector_filter_changed();
        true
    }

    fn inspector_filter_changed(&mut self) {
        let p = &mut self.preview;
        p.doc = None;
        p.scroll = 0.0;
        self.dirty = true;
    }

    pub(super) fn inspector_field_focused(&self) -> bool {
        self.preview.filtering && self.preview.filter.is_some() && self.focus == Focus::Preview
    }

    pub(super) fn inspector_field(&mut self) -> Option<&mut super::edit::LineEdit> {
        self.preview.filter.as_mut().filter(|_| self.preview.filtering).map(|f| &mut f.edit)
    }

    fn preview_copyable(&self) -> bool {
        self.shows_doc() || self.shows_image()
    }

    fn preview_key(&mut self, key: Key, m: Mods) -> bool {
        if self.focus != Focus::Preview || self.preview.r.pane.w <= 0.0 {
            return false;
        }
        let only_ctrl = m.ctrl && !m.shift && !m.alt;
        match key {
            Key::Char('C') | Key::Insert if only_ctrl => self.copy_from_preview(),
            Key::Char('A') if only_ctrl && self.shows_doc() => self.select_preview_text(),
            Key::Left | Key::Right | Key::Up | Key::Down | Key::Home | Key::End if m.shift && !m.alt && self.shows_doc() => self.extend_preview_pick(key, m.ctrl),
            _ if super::field::is_menu_key(key, m) && self.preview_copyable() => {
                let b = self.preview.r.body;
                self.preview_menu(b.x, b.y);
            }
            _ => return false,
        }
        self.dirty = true;
        true
    }

    fn copy_from_preview(&mut self) {
        if matches!(self.preview.content, Content::Image) && self.copy_full_image() {
            return;
        }
        let line_break = if self.caps.unix_paths { "\n" } else { "\r\n" };
        let query = self.preview.query();
        let p = &self.preview;
        let effect = match &p.content {
            Content::Doc(d) => Some(picked_text(d, p.pick.range(), &query, line_break)).filter(|t| !t.is_empty()).map(Effect::SetClipboard),
            Content::Image => p.image.clone().map(Effect::SetClipboardImage),
            _ => None,
        };
        self.effects.extend(effect);
    }

    fn copy_full_image(&mut self) -> bool {
        let p = &self.preview;
        let Some(b) = &p.image else { return false };
        let Some(s) = p.subject.as_ref().filter(|s| s.kind == Kind::Image && b.width.max(b.height) >= p.image_px) else { return false };
        let (path, size) = (s.path.clone(), s.entry.map_or(0, |e| e.size));
        let Some(path) = path else { return false };
        let gen = self.gen();
        let cancel = GenToken::default();
        cancel.set(gen);
        self.preview.copy_gen = gen;
        self.effects.push(Effect::Preview(PreviewRequest { gen, cancel, path, kind: Kind::Image, size, details: false, px: COPY_PX }));
        true
    }

    fn extend_preview_pick(&mut self, key: Key, ctrl: bool) {
        let fonts = (self.scale, super::options::mono_px(self.settings.opts.inspector_size));
        let p = &mut self.preview;
        let (Content::Doc(d), Some(lay)) = (&p.content, p.doc.as_ref()) else { return };
        let (Some(first), Some(last)) = (lay.lines.first(), lay.lines.last()) else { return };
        let s = p.pick.caret;
        let Some(k) = caret_line(d, lay, s) else { return };
        let line = &lay.lines[k];
        let (lo, hi) = line_text(d, lay, line);
        let caret_x = || line_span_x(d, lay, line, &p.widths, fonts, (lo, s.at)).map_or_else(|| lay.pieces.get(line.pieces.start as usize).map_or(0.0, |q| q.x), |r| r.1);
        let to = match key {
            Key::Left | Key::Right => step_char(d, lay, s, key == Key::Right),
            Key::Home if ctrl => Spot { para: first.para, at: line_text(d, lay, first).0 },
            Key::End if ctrl => Spot { para: last.para, at: line_text(d, lay, last).1 },
            Key::Home => Spot { para: line.para, at: lo },
            Key::End => Spot { para: line.para, at: hi },
            _ => {
                let target = if key == Key::Up { k.checked_sub(1) } else { Some(k + 1).filter(|&j| j < lay.lines.len()) };
                match target {
                    Some(j) => spot_on_line(d, lay, &lay.lines[j], &p.widths, fonts, caret_x()),
                    None if key == Key::Up => Spot { para: line.para, at: lo },
                    None => Spot { para: line.para, at: hi },
                }
            }
        };
        let shown = caret_line(d, lay, to).map(|j| (lay.lines[j].y, lay.lines[j].h));
        p.pick = Pick { anchor: p.pick.anchor, caret: to, from: (p.pick.anchor, p.pick.anchor), grain: Grain::Char };
        if let Some((y, h)) = shown {
            let room = p.r.body.h;
            if y < p.scroll {
                p.scroll = y;
            } else if y + h > p.scroll + room {
                p.scroll = (y + h - room).max(0.0);
            }
        }
    }

    fn select_preview_text(&mut self) {
        let p = &mut self.preview;
        let Content::Doc(d) = &p.content else { return };
        let Some(last) = d.paras.len().checked_sub(1) else { return };
        let end = Spot { para: last as u32, at: para_span(d, last).1 };
        p.pick = Pick { anchor: Spot::default(), caret: end, from: (Spot::default(), end), grain: Grain::Char };
    }

    fn preview_menu(&mut self, x: f32, y: f32) {
        let image = self.shows_image();
        let (a, b) = self.preview.pick.range();
        let menu = self.collect(|_, m| {
            let keys = m.keys.first_text("text.copy").unwrap_or_default();
            if image {
                return m.entry(Cmd::Text(TextCmd::Copy), S::PreviewCopyImage.t(), keys, Mark::None, false, false);
            }
            m.entry(Cmd::Text(TextCmd::Copy), S::MenuCopy.t(), keys, Mark::None, false, a == b);
            m.separator();
            let keys = m.keys.first_text("text.select_all").unwrap_or_default();
            m.entry(Cmd::Text(TextCmd::SelectAll), S::MenuFieldSelectAll.t(), keys, Mark::None, false, false);
        });
        self.flit_menu(x, y, menu);
    }

    fn preview_spot(&self, x: f32, y: f32) -> Option<Spot> {
        let p = &self.preview;
        let Content::Doc(d) = &p.content else { return None };
        let lay = p.doc.as_ref()?;
        let last = lay.lines.last()?;
        let fonts = (self.scale, super::options::mono_px(self.settings.opts.inspector_size));
        let b = p.r.body;
        let along = y - b.y + p.scroll;
        if along < 0.0 {
            return Some(spot_on_line(d, lay, &lay.lines[0], &p.widths, fonts, f32::NEG_INFINITY));
        }
        if along >= lay.height {
            return Some(spot_on_line(d, lay, last, &p.widths, fonts, f32::INFINITY));
        }
        let line = &lay.lines[lay.lines.partition_point(|l| l.y + l.h <= along).min(lay.lines.len() - 1)];
        let x0 = b.x + p.r.gutter;
        let across = if x < x0 { f32::NEG_INFINITY } else { x - x0 + p.hscroll.min(p.hmax).round() };
        Some(spot_on_line(d, lay, line, &p.widths, fonts, across))
    }

    fn grain_range(&self, s: Spot, grain: Grain) -> (Spot, Spot) {
        let Content::Doc(d) = &self.preview.content else { return (s, s) };
        let (start, end) = para_span(d, s.para as usize);
        let at = |k: usize| Spot { para: s.para, at: start + k as u32 };
        match grain {
            Grain::Char => (s, s),
            Grain::Word => {
                let r = word_at(&d.text[start as usize..end as usize], s.at.saturating_sub(start) as usize);
                (at(r.start), at(r.end))
            }
            Grain::Line => (at(0), at((end - start) as usize)),
        }
    }

    pub(super) fn preview_text_at(&self, x: f32, y: f32) -> bool {
        self.shows_doc() && self.preview.r.body.contains(x, y) && !self.preview.r.card.contains(x, y)
    }

    fn press_preview_body(&mut self, (x, y): (f32, f32), button: MouseButton, clicks: u8, now: Instant) {
        if !self.preview_copyable() || !self.preview.r.body.contains(x, y) {
            return;
        }
        self.focus = Focus::Preview;
        self.preview.filtering = false;
        if button == MouseButton::Right {
            return self.preview_menu(x, y);
        }
        let Some(spot) = self.preview_spot(x, y).filter(|_| button == MouseButton::Left) else { return };
        let clicks = self.text_clicks((x, y), clicks, now);
        let grain = match clicks {
            0 | 1 => Grain::Char,
            2 => Grain::Word,
            _ => Grain::Line,
        };
        let range = self.grain_range(spot, grain);
        let p = &mut self.preview.pick;
        *p = if self.mods.shift && grain == Grain::Char { Pick { anchor: p.anchor, caret: spot, from: (p.anchor, p.anchor), grain } } else { Pick { anchor: range.0, caret: range.1, from: range, grain } };
        self.drag = Drag::Preview { hit: PaneHit::Body, start: 0.0, size: 0.0 };
    }

    fn drag_preview_pick(&mut self, x: f32, y: f32) {
        let b = self.preview.r.body;
        let x0 = b.x + self.preview.r.gutter;
        let p = &mut self.preview;
        let below = (y - b.bottom()).max(0.0) - (b.y - y).max(0.0);
        p.scroll = (p.scroll + below).clamp(0.0, (p.extent - b.h).max(0.0));
        let aside = (x - b.right()).max(0.0) - (x0 - x).max(0.0);
        p.hscroll = (p.hscroll + aside).clamp(0.0, p.hmax);
        let Some(spot) = self.preview_spot(x, y) else { return };
        let grain = self.preview.pick.grain;
        let target = self.grain_range(spot, grain);
        let p = &mut self.preview.pick;
        let (a, z) = p.from;
        (p.anchor, p.caret) = if target.0 < a { (z, target.0) } else { (a, target.1.max(z)) };
        self.dirty = true;
    }

    pub(super) fn inspector_filter(&self) -> Option<&Field> {
        self.preview.filter.as_ref()
    }

    pub(super) fn inspector_filter_mut(&mut self) -> Option<&mut Field> {
        self.preview.filter.as_mut()
    }

    pub(super) fn inspector_filter_rect(&self) -> Rect {
        self.preview.r.filter
    }

    pub(super) fn quick_view(&mut self) {
        match self.subject().and_then(|s| s.path.filter(|_| s.many.is_none() && s.entry.is_some())) {
            Some(path) => self.effects.push(Effect::QuickView(path)),
            None => self.on_quick_view(false),
        }
    }

    pub fn on_quick_view(&mut self, shown: bool) {
        if shown {
            return;
        }
        let p = &mut self.preview;
        if p.state.visible && !p.look.card && p.peeked {
            p.peeked = false;
            self.show_inspector(false);
        } else {
            p.peeked = !p.state.visible;
            p.look.card = false;
            p.state.details = false;
            self.show_inspector(true);
        }
    }

    pub(super) fn play_preview(&mut self) {
        if self.preview.host.gen != 0 && !self.preview.host.handler {
            self.preview_click(PaneHit::Play);
        }
    }

    pub(super) fn layout_preview(&mut self) {
        fn sized(saved: f32, room: f32, min: f32, s: impl Fn(f32) -> f32) -> f32 {
            if saved == HALF {
                (room / 2.0).round().clamp(min.min(room), s(MAX_SIDE))
            } else {
                s(saved)
            }
        }
        let sc = self.scale;
        let s = |v: f32| (v * sc).round();
        let st = self.preview.state;
        let shown = st.visible && !self.search.active && !self.on_home();
        let foot_h = if matches!(self.preview.content, Content::Media { .. }) {
            self.preview.foot_slide = Default::default();
            0.0
        } else {
            let foot_on = shown && self.focus == Focus::Preview && (self.prefs.status_bar || self.preview.filter.is_some());
            let now = self.clock();
            self.motion |= self.preview.foot_slide.aim(if foot_on { s(FOOT_H) } else { 0.0 }, self.animations, now, super::panels::SPLIT);
            self.preview.foot_slide.value(now, super::panels::SPLIT).round()
        };
        let l = &mut self.layout;
        l.bottom = l.status.y;
        let (x0, w, top, end) = (l.header.x, l.header.right(), l.toolbar.bottom(), l.status.bottom());
        let (room_w, room_h) = ((w - x0).max(0.0), (end - top).max(0.0));
        self.preview.room = (room_w, room_h);
        let right = self.inspector_right();
        let l = &mut self.layout;
        let r = &mut self.preview.r;
        *r = Rects::default();
        if !shown {
            return;
        }
        if right {
            let pw = sized(st.right_w, room_w, s(MIN_RIGHT), s).min((room_w * 0.7).round());
            r.pane = Rect::new(w - pw, top, pw, room_h);
            r.split = Rect::new(r.pane.x - s(3.0), top, s(6.0), room_h);
            l.header.w = (l.header.w - pw).max(0.0);
            l.status.w = (r.pane.x - l.status.x).max(0.0);
        } else {
            let status_h = l.status.h;
            let ph = sized(st.bottom_h, room_h, s(MIN_BOTTOM), s).min((room_h * 0.7).round());
            r.pane = Rect::new(x0, end - ph, room_w, ph);
            r.split = Rect::new(x0, r.pane.y - s(3.0), room_w, s(6.0));
            l.status.y = r.pane.y - status_h;
            l.bottom = l.status.y;
        }
        let pane = r.pane;
        r.tools = Rect::new(pane.x, pane.y, pane.w, s(TOOLS_H));
        r.foot = Rect::new(pane.x, pane.bottom() - foot_h, pane.w, foot_h);
        let top = r.tools.bottom() + s(8.0);
        r.body = Rect::new(pane.x + s(PAD), top, (pane.w - 2.0 * s(PAD)).max(0.0), (r.foot.y - s(6.0) - top).max(0.0));
        let card_w = s(CARD_W).min(pane.w - s(16.0)).max(0.0);
        r.card = if self.preview.look.card { Rect::new(pane.right() - card_w - s(8.0), r.tools.bottom() + s(4.0), card_w, (r.foot.y - r.tools.bottom() - s(12.0)).max(0.0)) } else { Rect::default() };
    }

    fn right_side(&self) -> bool {
        self.inspector_right()
    }

    pub(super) fn preview_hit(&self, x: f32, y: f32) -> Option<Hit> {
        let r = &self.preview.r;
        if r.split.contains(x, y) {
            return Some(Hit::Preview(PaneHit::Split));
        }
        if !r.pane.contains(x, y) {
            return None;
        }
        if let Some(&(_, h)) = self.preview.buttons.iter().find(|b| b.0.contains(x, y)) {
            return Some(Hit::Preview(h));
        }
        let h = match () {
            _ if r.card.contains(x, y) && r.button.contains(x, y) => PaneHit::Properties,
            _ if r.card.contains(x, y) => self.preview.chips.iter().find(|c| c.0.contains(x, y)).map_or(PaneHit::Card, |c| c.1),
            _ if r.play.contains(x, y) => PaneHit::Play,
            _ if r.seek.contains(x, y) => PaneHit::Seek,
            _ if r.volume.contains(x, y) => PaneHit::Volume,
            _ => PaneHit::Body,
        };
        Some(Hit::Preview(h))
    }

    pub(super) fn preview_rect(&self, h: PaneHit) -> Option<Rect> {
        self.preview.buttons.iter().find(|b| b.1 == h).map(|b| b.0)
    }

    pub(super) fn preview_cursor(&self) -> CursorIcon {
        if self.right_side() {
            CursorIcon::ResizeHorizontal
        } else {
            CursorIcon::ResizeVertical
        }
    }

    pub(super) fn preview_mouse_down(&mut self, h: PaneHit, (x, y): (f32, f32), button: MouseButton, clicks: u8, now: Instant) {
        let st = self.preview.state;
        let pane = self.preview.r.pane;
        let shown = |v: f32, px: f32| if v == HALF { px / self.scale } else { v };
        let (at, size) = if self.right_side() { (x, shown(st.right_w, pane.w)) } else { (y, shown(st.bottom_h, pane.h)) };
        if !matches!(h, PaneHit::Split | PaneHit::Close) {
            self.focus = Focus::Preview;
        }
        match h {
            PaneHit::Split => self.drag = Drag::Preview { hit: h, start: at, size },
            PaneHit::Seek | PaneHit::Volume => {
                self.drag = Drag::Preview { hit: h, start: 0.0, size: 0.0 };
                self.preview_slide(h, x);
            }
            PaneHit::Filter => {
                self.open_inspector_filter();
                if let Some(f) = self.preview.filter.as_mut() {
                    f.click(x, 1, false);
                }
            }
            PaneHit::Body => self.press_preview_body((x, y), button, clicks, now),
            _ => {}
        }
    }

    pub(super) fn preview_drag(&mut self, hit: PaneHit, x: f32, y: f32, start: f32, size: f32) {
        if hit == PaneHit::Body {
            return self.drag_preview_pick(x, y);
        }
        if hit != PaneHit::Split {
            return self.preview_slide(hit, x);
        }
        let right = self.right_side();
        let delta = (if right { start - x } else { start - y }) / self.scale;
        let st = &mut self.preview.state;
        if right {
            st.right_w = (size + delta).clamp(MIN_RIGHT, MAX_SIDE);
        } else {
            st.bottom_h = (size + delta).clamp(MIN_BOTTOM, MAX_SIDE);
        }
        self.layout();
        self.dirty = true;
    }

    fn preview_slide(&mut self, hit: PaneHit, x: f32) {
        let r = if hit == PaneHit::Seek { self.preview.r.seek } else { self.preview.r.volume };
        let f = ((x - r.x) / r.w.max(1.0)).clamp(0.0, 1.0);
        let cmd = if hit == PaneHit::Seek {
            let t = f as f64 * self.preview.media.duration;
            self.preview.media.position = t;
            MediaCmd::Seek(t)
        } else {
            self.preview.state.volume = f;
            MediaCmd::Volume(f)
        };
        self.effects.push(Effect::Host(HostCmd::Media(cmd)));
        self.dirty = true;
    }

    pub(super) fn preview_click(&mut self, h: PaneHit) {
        match h {
            PaneHit::Close => self.show_inspector(false),
            PaneHit::At(at) => self.set_inspector_at(at),
            PaneHit::Tool(t) => self.inspector_toggle(t),
            PaneHit::Title => {
                self.preview.look.card = !self.preview.look.card;
                self.preview.card_scroll = 0.0;
                self.layout();
            }
            PaneHit::CardClose => {
                self.preview.look.card = false;
                self.layout();
            }
            PaneHit::CardDetails => self.preview.state.details = !self.preview.state.details,
            PaneHit::FilterClear => self.close_inspector_filter(),
            PaneHit::Properties => self.properties_dialog(),
            PaneHit::Tag(k) => self.toggle_tag(k),
            PaneHit::NewTag => self.new_tag(true),
            PaneHit::Download => {
                let p = &mut self.preview;
                p.download = std::mem::take(&mut p.key);
            }
            PaneHit::Pretty => self.toggle_pretty(),
            PaneHit::SvgBackground => {
                if let (Some(i), Some(anchor)) = (super::settings::index("windows.svg_background"), self.preview_rect(h)) {
                    self.show_setting_choices(i, Hit::Preview(h), anchor);
                }
            }
            PaneHit::Play => {
                let p = &mut self.preview;
                p.media.playing = !p.media.playing;
                p.host.started = true;
                let cmd = if p.media.playing { MediaCmd::Play } else { MediaCmd::Pause };
                self.effects.push(Effect::Host(HostCmd::Media(cmd)));
            }
            _ => {}
        }
        self.dirty = true;
    }

    pub(super) fn preview_wheel(&mut self, dx: f32, dy: f32, zoom: bool, now: Instant) -> bool {
        let Hit::Preview(h) = self.hover else { return false };
        if (zoom && !self.mods.alt) || h == PaneHit::Size {
            let n = if super::pointer::upright(dx, dy) { self.wheel_steps.add(super::pointer::Steps::Inspector, dy, now) } else { 0 };
            for _ in 0..n.unsigned_abs() {
                self.inspector_zoom(n > 0);
            }
            return true;
        }
        let line = ROW * self.scale;
        if matches!(h, PaneHit::Card | PaneHit::Tag(_) | PaneHit::NewTag | PaneHit::Properties) {
            let step = self.wheel_px(line, self.preview.r.card.h);
            let p = &mut self.preview;
            p.card_scroll = (p.card_scroll - dy * step).clamp(0.0, (p.card_extent - p.r.card.h).max(0.0));
            self.dirty = true;
            return true;
        }
        let across = super::pointer::sideways(dx, dy, self.mods.shift);
        let (step, p) = (self.wheel_px(line, if across != 0.0 { self.preview.r.body.w } else { self.preview.r.body.h }), &mut self.preview);
        if across != 0.0 {
            p.hscroll = (p.hscroll + across * step).clamp(0.0, p.hmax);
        } else {
            p.scroll = (p.scroll - dy * step).clamp(0.0, (p.extent - p.r.body.h).max(0.0));
        }
        self.dirty = true;
        true
    }

    pub(super) fn preview_pick(&self) -> Pick {
        self.preview.pick
    }

    pub(super) fn restore_preview_pick(&mut self, pick: Pick) {
        self.preview.pick = pick;
    }

    pub(super) fn touch_pan_preview(&mut self, dx: f32, dy: f32) -> (bool, bool) {
        let p = &mut self.preview;
        let (x, y) = ((p.hscroll - dx).clamp(0.0, p.hmax), (p.scroll - dy).clamp(0.0, (p.extent - p.r.body.h).max(0.0)));
        let moved = (x != p.hscroll, y != p.scroll);
        (p.hscroll, p.scroll) = (x, y);
        moved
    }

    pub(super) fn scroll_preview(&mut self, dy: f32) -> bool {
        let p = &mut self.preview;
        p.scroll = (p.scroll - dy * 3.0 * ROW * self.scale).clamp(0.0, (p.extent - p.r.body.h).max(0.0));
        self.dirty = true;
        true
    }
}

#[cfg(test)]
#[path = "../../test/ui/preview_pane.rs"]
mod tests;
