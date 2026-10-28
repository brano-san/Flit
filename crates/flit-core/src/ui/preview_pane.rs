use super::field::Field;
use super::options::{InspectorAt, INSPECTOR_SIZE};
use super::widgets::{Button, Paint};
use super::{centered, icon_key, App, Bitmap, Drag, Effect, Focus, Hit, IconRequest, IconState};
use crate::fmt as format;
use crate::hash::{self, FxHashMap};
use crate::input::{CursorIcon, Key, Mods};
use crate::listing::{attr, Entry};
use crate::name;
use crate::path::{FsPath, PathKind};
use crate::i18n::{P, S};
use crate::preview::{self, Block, Doc, FolderSummary, HostCmd, HostEvent, Hosted, Kind, MediaCmd, Note, PreviewData, PreviewRequest, PropGroup, Run, Tone, IMAGE_PX};
use crate::provider::GenToken;
use crate::recycle;
use crate::render::{Color, Rect};
use crate::text::{FontFamily, FontStyle, Gfx, TextBackend};
use crate::theme::Theme;
use serde::{Deserialize, Serialize};
use std::fmt::Write as _;
use std::ops::Range;
use std::time::Instant;

const MIN_RIGHT: f32 = 200.0;
const MIN_BOTTOM: f32 = 120.0;
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
const G_PLAY: &str = "\u{E768}";
const G_PAUSE: &str = "\u{E769}";
const G_VOLUME: &str = "\u{E767}";
const G_CLOSE: &str = "\u{E8BB}";
const G_AUTO: &str = "\u{E72C}";
const G_RIGHT: &str = "\u{E8A0}";
const G_BELOW: &str = "\u{E70D}";
const G_WRAP: &str = "\u{E7A6}";
const G_LINES: &str = "\u{E8FD}";
const G_SPACES: &str = "\u{E8D2}";
const G_FIT: &str = "\u{E922}";
const G_CUSTOM: &str = "\u{E721}";
const G_GRID: &str = "\u{E8A9}";
const G_NEXT: &str = "\u{E76C}";
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
        Self { visible: false, details: false, right_w: 340.0, bottom_h: 260.0, volume: 0.8 }
    }
}

impl PaneState {
    pub fn sanitized(self) -> Self {
        let d = Self::default();
        let fix = |v: f32, min: f32, def: f32| if v.is_finite() && (min..=MAX_SIDE).contains(&v) { v } else { def };
        let volume = if (0.0..=1.0).contains(&self.volume) { self.volume } else { d.volume };
        Self { right_w: fix(self.right_w, MIN_RIGHT, d.right_w), bottom_h: fix(self.bottom_h, MIN_BOTTOM, d.bottom_h), volume, ..self }
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
}

impl Subject {
    fn key(&self) -> u64 {
        let e = self.entry.map(|e| (e.size, e.modified, e.attrs));
        hash::hash(&(&self.title, self.path.as_ref().map(|p| p.as_bytes()), self.kind, e, self.many, self.drive))
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
    Media { video: bool },
    Handler,
    Basic(Option<Note>),
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
    pieces: Range<u32>,
}

#[derive(Default)]
struct DocLayout {
    key: u64,
    lines: Vec<VLine>,
    pieces: Vec<Piece>,
    height: f32,
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
    key: u64,
    gen: u64,
    cancel: GenToken,
    subject: Option<Subject>,
    content: Content,
    scroll: f32,
    extent: f32,
    card_scroll: f32,
    card_extent: f32,
    doc: Option<DocLayout>,
    widths: FxHashMap<(char, FontStyle), f32>,
    image: Option<Bitmap>,
    image_rev: u64,
    host: Host,
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
}

impl PreviewPane {
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
        if d.mono { self.look.wrap.unwrap_or(d.wrap) } else { true }
    }
}

fn ui(px: f32, sc: f32, bold: bool) -> FontStyle {
    FontStyle::new(FontFamily::Ui, px * sc, bold)
}

fn run_font(r: &Run, block: Block, sc: f32, mono: f32) -> FontStyle {
    match block {
        _ if r.mono => FontStyle::new(FontFamily::Mono, mono * sc, false),
        Block::Heading(1) => ui(20.0, sc, true),
        Block::Heading(2) => ui(17.0, sc, true),
        Block::Heading(_) => ui(14.5, sc, true),
        _ => ui(13.0, sc, r.bold),
    }
}

fn para_font(block: Block, mono: bool, sc: f32, mono_px: f32) -> FontStyle {
    let r = Run { start: 0, end: 0, tone: Tone::Plain, bold: false, mono: mono || block == Block::Code };
    run_font(&r, block, sc, mono_px)
}

fn tone_color(t: &Theme, dark: bool, tone: Tone) -> Color {
    let pick = |d: u32, l: u32| Color::rgb(if dark { d } else { l });
    match tone {
        Tone::Plain => t.text,
        Tone::Dim | Tone::Comment => t.text_dim,
        Tone::Keyword => pick(0x569CD6, 0x0033B3),
        Tone::String => pick(0xCE9178, 0xA31515),
        Tone::Number => pick(0xB5CEA8, 0x098658),
        Tone::Link => t.accent,
        Tone::Added => pick(0x6A9955, 0x22863A),
        Tone::Removed => t.error,
    }
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
    let text: String = d.para_text(k).collect();
    text.to_lowercase().contains(query)
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
            out.lines.push(VLine { y, h: (12.0 * sc).round(), base, block, num, pieces: 0..0 });
            y += (12.0 * sc).round();
            continue;
        }
        let indent = match block {
            Block::Item { depth } => ((depth as f32 * 18.0 + 4.0) * sc).round(),
            Block::Quote => (14.0 * sc).round(),
            Block::Code if !d.mono => (8.0 * sc).round(),
            _ => 0.0,
        };
        let (mut x, mut first, mut number) = (indent, out.pieces.len(), num);
        let line = |out: &mut DocLayout, first: &mut usize, y: &mut f32, number: &mut u32| {
            out.lines.push(VLine { y: *y, h: lh, base, block, num: *number, pieces: *first as u32..out.pieces.len() as u32 });
            *y += lh;
            *first = out.pieces.len();
            *number = 0;
        };
        for ri in para.runs.clone() {
            let run = d.runs[ri as usize];
            let font = run_font(&run, block, sc, mono);
            let text = d.run_text(&run);
            let mut at = run.start;
            for token in text.split_inclusive(' ') {
                let start = at;
                at += token.len() as u32;
                let trimmed = token.trim_end_matches(' ');
                let tw: f32 = token.chars().map(|c| char_w(g, widths, c, font)).sum();
                let core: f32 = trimmed.chars().map(|c| char_w(g, widths, c, font)).sum();
                if wrap && x > indent && x + core > width {
                    line(&mut out, &mut first, &mut y, &mut number);
                    x = indent;
                    if trimmed.is_empty() {
                        continue;
                    }
                }
                if wrap && core > width - indent {
                    let mut pos = start;
                    for c in token.chars() {
                        let cw = char_w(g, widths, c, font);
                        if x > indent && x + cw > width {
                            line(&mut out, &mut first, &mut y, &mut number);
                            x = indent;
                        }
                        put(&mut out.pieces, first, ri, pos, pos + c.len_utf8() as u32, x);
                        pos += c.len_utf8() as u32;
                        x += cw;
                    }
                    continue;
                }
                put(&mut out.pieces, first, ri, start, at, x);
                x += tw;
            }
        }
        line(&mut out, &mut first, &mut y, &mut number);
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
        if self.preview.state.visible { self.preview.r.pane } else { Rect::default() }
    }

    fn show_inspector(&mut self, on: bool) {
        let p = &mut self.preview;
        if on && !p.state.visible {
            p.look.at = self.opts.inspector_at;
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
            "inspector.grid" | "inspector.scale" => shown && self.shows_image(),
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
            let size = i64::from(self.opts.inspector_size) + if up { 1 } else { -1 };
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
        if !self.preview.filtering || !self.preview.state.visible {
            return false;
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
        let sc = self.scale;
        let s = |v: f32| (v * sc).round();
        let st = self.preview.state;
        let shown = st.visible && !self.search.active && !self.on_home();
        let l = &mut self.layout;
        l.bottom = l.status.y;
        let (x0, w, top, end) = (l.header.x, l.header.right(), l.commands.bottom(), l.status.bottom());
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
            let pw = s(st.right_w).min((room_w * 0.7).round());
            r.pane = Rect::new(w - pw, top, pw, room_h);
            r.split = Rect::new(r.pane.x - s(3.0), top, s(6.0), room_h);
            l.header.w = (l.header.w - pw).max(0.0);
            l.status.w = (r.pane.x - l.status.x).max(0.0);
        } else {
            let status_h = l.status.h;
            let ph = s(st.bottom_h).min((room_h * 0.7).round());
            r.pane = Rect::new(x0, end - ph, room_w, ph);
            r.split = Rect::new(x0, r.pane.y - status_h - s(3.0), room_w, s(6.0));
            l.status.y = r.pane.y - status_h;
            l.bottom = l.status.y;
        }
        let pane = r.pane;
        r.tools = Rect::new(pane.x, pane.y, pane.w, s(TOOLS_H));
        r.foot = Rect::new(pane.x, pane.bottom() - s(FOOT_H), pane.w, s(FOOT_H));
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

    pub(super) fn preview_mouse_down(&mut self, h: PaneHit, x: f32, y: f32) {
        let st = self.preview.state;
        let (at, size) = if self.right_side() { (x, st.right_w) } else { (y, st.bottom_h) };
        match h {
            PaneHit::Split => self.drag = Drag::Preview { hit: h, start: at, size },
            PaneHit::Seek | PaneHit::Volume => {
                self.drag = Drag::Preview { hit: h, start: 0.0, size: 0.0 };
                self.preview_slide(h, x);
            }
            PaneHit::Filter => {
                self.open_inspector_filter();
                if let Some(f) = self.preview.filter.as_mut() {
                    f.click(x, 1);
                }
            }
            _ => {}
        }
    }

    pub(super) fn preview_drag(&mut self, hit: PaneHit, x: f32, y: f32, start: f32, size: f32) {
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

    pub(super) fn preview_wheel(&mut self, dy: f32) -> bool {
        let Hit::Preview(h) = self.hover else { return false };
        if self.mods.ctrl && !self.mods.alt {
            self.inspector_zoom(dy > 0.0);
            return true;
        }
        if h == PaneHit::Size {
            self.inspector_zoom(dy > 0.0);
            return true;
        }
        if matches!(h, PaneHit::Card | PaneHit::Tag(_) | PaneHit::NewTag | PaneHit::Properties) {
            let p = &mut self.preview;
            p.card_scroll = (p.card_scroll - dy * 3.0 * ROW * self.scale).clamp(0.0, (p.card_extent - p.r.card.h).max(0.0));
            self.dirty = true;
            return true;
        }
        self.scroll_preview(dy)
    }

    pub(super) fn scroll_preview(&mut self, dy: f32) -> bool {
        let p = &mut self.preview;
        p.scroll = (p.scroll - dy * 3.0 * ROW * self.scale).clamp(0.0, (p.extent - p.r.body.h).max(0.0));
        self.dirty = true;
        true
    }

    fn subject(&self) -> Option<Subject> {
        let t = self.tab();
        let v = t.view.as_ref().filter(|v| v.snap().gen == t.gen)?;
        let l = v.listing();
        let in_bin = t.view_loc.is_recycle_bin();
        let dir_key = t.view_loc.to_windows().map(|p| p.as_bytes().to_vec()).unwrap_or_default();
        let base = Subject { title: t.title(), path: None, kind: Kind::Folder, entry: None, ext: Vec::new(), dir_key, many: None, drive: None };
        match v.selected_count() {
            0 => Some(Subject { path: t.view_loc.to_windows().filter(|p| p.kind() != PathKind::Computer), ..base }),
            1 => {
                let i = v.focus.filter(|&f| v.is_selected(f)).or_else(|| v.selected().next())?;
                let e = l.entries[i as usize];
                let name = l.name(i);
                let path = if in_bin { recycle::recycled(l, i) } else { t.entry_location(i).to_windows() };
                let kind = if e.is_dir() { Kind::Folder } else { preview::kind(l.ext(i)) };
                let drive = (e.attrs & attr::VOLUME != 0)
                    .then(|| self.nav.sidebar.places.drives.iter().find(|d| d.path.as_bytes().get(..2).is_some_and(|p| p.eq_ignore_ascii_case(name)) && d.total > 0).map(|d| (d.total, d.free)))
                    .flatten();
                let dir_key = if in_bin { recycle::origin(l, i).to_vec() } else { base.dir_key };
                Some(Subject { title: name::display(name).into_owned(), path, kind, entry: Some(e), ext: l.ext(i).to_vec(), dir_key, drive, ..base })
            }
            n => Some(Subject { title: crate::trn!(P::PreviewSelected, n), many: Some((n, v.selected_size())), path: None, ..base }),
        }
    }

    fn selection_types(&self) -> (Vec<(String, usize)>, usize, usize) {
        let t = self.tab();
        let Some(v) = t.view.as_ref() else { return (Vec::new(), 0, 0) };
        let l = v.listing();
        let mut counts: FxHashMap<Vec<u8>, usize> = FxHashMap::default();
        let (mut dirs, mut files) = (0, 0);
        for i in v.selected() {
            let dir = l.entries[i as usize].is_dir();
            if dir {
                dirs += 1;
            } else {
                files += 1;
            }
            let key = if dir { Vec::new() } else { l.ext(i).to_ascii_lowercase() };
            *counts.entry(key).or_default() += 1;
        }
        let mut out: Vec<(String, usize)> = counts.into_iter().map(|(ext, n)| (self.ext_type_label(&ext, ext.is_empty() && dirs > 0 && n <= dirs), n)).collect();
        out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        (out, dirs, files)
    }

    fn ext_type_label(&self, ext: &[u8], dir: bool) -> String {
        if dir {
            return S::DetailsTypeFolder.t().into();
        }
        match self.type_names.get(ext) {
            Some(Some(n)) if !n.is_empty() => n.clone(),
            _ if ext.is_empty() => S::DetailsTypeFile.t().into(),
            _ => crate::tr!(S::DetailsTypeExtFile, ext = name::display(ext).to_uppercase()),
        }
    }

    pub(super) fn sync_preview(&mut self, now: Instant) {
        let shown = self.preview.state.visible && !self.search.active && !self.on_home();
        let subject = if shown { self.subject() } else { None };
        let key = subject.as_ref().map_or(0, Subject::key);
        let wants_props = shown && self.preview.look.card && self.preview.state.details;
        let props_key = if wants_props { key } else { 0 };
        if props_key != self.preview.props_key {
            self.preview.props_key = props_key;
            self.preview.props.clear();
            self.preview.props_loading = false;
            let path = subject.as_ref().filter(|s| s.many.is_none() && s.entry.is_some()).and_then(|s| s.path.clone());
            if let (true, Some(path), Some(s)) = (wants_props, path, subject.as_ref()) {
                let gen = self.gen();
                let p = &mut self.preview;
                p.props_gen = gen;
                p.props_loading = true;
                let cancel = GenToken::default();
                cancel.set(gen);
                let req = PreviewRequest { gen, cancel, path, kind: s.kind, size: 0, details: true, px: 0 };
                self.effects.push(Effect::Preview(req));
            }
        }
        if key == self.preview.key {
            return;
        }
        let gen = self.gen();
        let archived = self.archive_view().is_some();
        let (types, dirs, files) = if subject.as_ref().is_some_and(|s| s.many.is_some()) { self.selection_types() } else { (Vec::new(), 0, 0) };
        let p = &mut self.preview;
        p.key = key;
        p.gen = gen;
        p.cancel.set(gen);
        p.scroll = 0.0;
        p.doc = None;
        p.media = Media::default();
        p.types = types;
        if dirs + files > 0 {
            if let Some(s) = p.subject.as_mut() {
                s.title = String::new();
            }
        }
        if p.image.take().is_some() {
            p.image_rev += 1;
        }
        if std::mem::take(&mut p.host).gen != 0 {
            self.effects.push(Effect::Host(HostCmd::Close));
        }
        let p = &mut self.preview;
        p.content = Content::Empty;
        p.requested = None;
        p.subject = subject.clone().map(|mut s| {
            if s.many.is_some() {
                s.title = format!("{}, {}", crate::trn!(P::PreviewFolders, dirs), crate::trn!(P::PreviewFiles, files));
            }
            s
        });
        let Some(s) = subject.filter(|s| s.many.is_none() && s.drive.is_none()) else { return };
        let Some(path) = s.path else { return };
        if archived {
            p.content = Content::Basic(Some(Note::Archived));
            return;
        }
        let side = p.r.body.w.max(p.r.body.h) * 2.0;
        let px = (side as u32).clamp(256, IMAGE_PX);
        p.content = Content::Loading;
        p.requested = Some(now);
        let req = PreviewRequest { gen, cancel: p.cancel.clone(), path, kind: s.kind, size: s.entry.map_or(0, |e| e.size), details: false, px };
        self.effects.push(Effect::Preview(req));
    }

    pub fn on_preview(&mut self, gen: u64, data: PreviewData) {
        if let PreviewData::Props(groups) = &data {
            if self.quick_props(gen, groups) || self.properties_props(gen, groups) {
                return;
            }
            if gen == self.preview.props_gen && gen != 0 {
                self.preview.props.clone_from(groups);
                self.preview.props_loading = false;
                self.dirty = true;
                return;
            }
        }
        let p = &mut self.preview;
        if gen != p.gen {
            return;
        }
        self.dirty = true;
        let (content, bitmap, hosted) = match data {
            PreviewData::Doc(d) => (Content::Doc(d), None, None),
            PreviewData::Image(b) => (Content::Image, Some(b), None),
            PreviewData::Folder(f) => (Content::Folder(f), None, None),
            PreviewData::Media { video, poster } => (Content::Media { video }, poster, Some(Hosted::Media { video })),
            PreviewData::Handler(clsid) => (Content::Handler, None, Some(Hosted::Handler(clsid))),
            PreviewData::Basic { poster, note } => (Content::Basic(note), poster, None),
            PreviewData::Props(_) => (Content::Basic(None), None, None),
        };
        p.content = content;
        if let Some(b) = bitmap {
            p.image = Some(b);
            p.image_rev += 1;
        }
        let (Some(what), Some(path)) = (hosted, p.subject.as_ref().and_then(|s| s.path.clone())) else { return };
        p.host = Host { gen, handler: matches!(what, Hosted::Handler(_)), video: what == Hosted::Media { video: true }, ..Host::default() };
        let (volume, bg) = (p.state.volume, self.theme().bg.0);
        self.effects.push(Effect::Host(HostCmd::Show { gen, what, path, rect: None, volume, bg }));
    }

    pub(super) fn on_hosted(&mut self, ev: HostEvent) {
        let p = &mut self.preview;
        match ev {
            HostEvent::Ready { gen, ms } if gen == p.host.gen => {
                let total = p.requested.take().map_or(0.0, |t| t.elapsed().as_secs_f64() * 1e3);
                tracing::debug!("hosted preview shown {total:.1} ms after selection, {ms} ms in the handler");
            }
            HostEvent::Failed { gen, message } if gen == p.host.gen => {
                let media = !p.host.handler;
                p.host = Host::default();
                p.content = if media { Content::Basic(Some(Note::CannotPlay(message))) } else { Content::Basic(Some(Note::Message(message))) };
            }
            HostEvent::Media { gen, position, duration, playing } if gen == p.host.gen => p.media = Media { position, duration, playing },
            _ => return,
        }
        self.dirty = true;
    }

    fn place_host(&mut self) {
        let p = &self.preview;
        if p.host.gen == 0 {
            return;
        }
        let covered = self.nav.palette.is_some() || self.jobs.open || self.jobs.modal() || self.search.active || p.look.card;
        let want = match () {
            _ if covered || p.r.pane.w <= 0.0 => None,
            _ if p.host.handler => Some(p.r.body),
            _ if p.host.video && p.host.started && p.r.media.h > 0.0 => Some(p.r.media),
            _ => None,
        };
        if want != p.host.placed {
            self.preview.host.placed = want;
            self.effects.push(Effect::Host(HostCmd::Place(want)));
        }
    }

    pub(super) fn draw_preview<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme) {
        let r = self.preview.r;
        self.preview.buttons.clear();
        if r.pane.w <= 0.0 || r.pane.h <= 0.0 {
            return;
        }
        g.dl.rect(r.pane, t.bg);
        let hot = matches!(self.hover, Hit::Preview(PaneHit::Split)) || matches!(self.drag, Drag::Preview { hit: PaneHit::Split, .. });
        let line = if self.right_side() { Rect::new(r.pane.x, r.pane.y, 1.0, r.pane.h) } else { Rect::new(r.pane.x, r.pane.y, r.pane.w, 1.0) };
        g.dl.rect(line, if hot { t.accent } else { t.separator });
        g.dl.set_clip(Some(r.body));
        let extent = self.draw_preview_body(g, t);
        g.dl.set_clip(None);
        let p = &mut self.preview;
        p.extent = extent;
        p.scroll = p.scroll.clamp(0.0, (extent - r.body.h).max(0.0));
        if !matches!(p.content, Content::Loading | Content::Empty | Content::Handler) {
            if let Some(t) = p.requested.take() {
                tracing::debug!("preview drawn {:.1} ms after selection, {} KB held", t.elapsed().as_secs_f64() * 1e3, p.held() / 1024);
            }
        }
        self.draw_inspector_tools(g, t);
        self.draw_inspector_foot(g, t);
        self.draw_card(g, t);
        self.place_host();
    }

    fn tool_button<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme, r: Rect, glyph: &str, hit: PaneHit, on: bool) {
        let state = Button::of(self.hover == Hit::Preview(hit), on, true);
        let font = if glyph.is_ascii() { self.ui_font(11.5) } else { self.icon_font(11.0) };
        Paint::new(g, t, self.scale).icon_button(r, glyph, font, state, t.text_dim);
        self.preview.buttons.push((r, hit));
    }

    fn draw_inspector_tools<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme) {
        let r = self.preview.r.tools;
        g.dl.rect(r, t.chrome);
        g.dl.rect(Rect::new(r.x, r.bottom() - 1.0, r.w, 1.0), t.separator);
        let bw = self.s(TOOL_W);
        let y = r.y + ((r.h - bw) / 2.0).round();
        let mut x = r.x + self.s(4.0);
        self.tool_button(g, t, Rect::new(x, y, bw, bw), G_CLOSE, PaneHit::Close, false);
        x += bw + self.s(8.0);
        let group = Rect::new(x - self.s(2.0), y - self.s(2.0), 3.0 * bw + self.s(4.0), bw + self.s(4.0));
        Paint::new(g, t, self.scale).dashed(group, t.input_border);
        for (k, at) in AT_CHOICES.into_iter().enumerate() {
            let glyph = [G_AUTO, G_RIGHT, G_BELOW][k];
            self.tool_button(g, t, Rect::new(x, y, bw, bw), glyph, PaneHit::At(at), self.preview.look.at == at);
            x += bw;
        }
        x += self.s(8.0);
        let look = self.preview.look;
        let tools: Vec<(&str, PaneHit, bool)> = match &self.preview.content {
            Content::Doc(d) => {
                let wrap = self.preview.effective_wrap(d);
                let mut v = vec![(G_WRAP, PaneHit::Tool(PaneTool::Wrap), wrap)];
                if d.mono {
                    v.push((G_LINES, PaneHit::Tool(PaneTool::Lines), look.lines));
                    v.push((G_SPACES, PaneHit::Tool(PaneTool::Spaces), look.spaces));
                }
                v
            }
            Content::Image => vec![
                (G_FIT, PaneHit::Tool(PaneTool::Scale(Scale::Fit)), look.scale == Scale::Fit),
                ("1:1", PaneHit::Tool(PaneTool::Scale(Scale::Original)), look.scale == Scale::Original),
                (G_CUSTOM, PaneHit::Tool(PaneTool::Scale(Scale::Custom)), look.scale == Scale::Custom),
                (G_GRID, PaneHit::Tool(PaneTool::Grid), look.grid),
            ],
            _ => Vec::new(),
        };
        if !tools.is_empty() {
            g.dl.rect(Rect::new(x - self.s(4.0), y + self.s(5.0), 1.0, bw - self.s(10.0)), t.separator);
        }
        let image = matches!(self.preview.content, Content::Image);
        for (k, (glyph, hit, on)) in tools.into_iter().enumerate() {
            if image && k == 0 {
                let grp = Rect::new(x - self.s(2.0), y - self.s(2.0), 3.0 * bw + self.s(4.0), bw + self.s(4.0));
                Paint::new(g, t, self.scale).dashed(grp, t.input_border);
            }
            if image && k == 3 {
                x += self.s(8.0);
            }
            self.tool_button(g, t, Rect::new(x, y, bw, bw), glyph, hit, on);
            x += bw;
        }
        let title = self.preview.subject.as_ref().map(|s| s.title.clone()).unwrap_or_default();
        let font = self.ui_font(12.5);
        let chev = self.icon_font(8.0);
        let room = (r.right() - x - self.s(16.0)).max(0.0);
        let tw = g.measure(&title, font).min((room - self.s(20.0)).max(0.0));
        let w = tw + self.s(22.0);
        let tr = Rect::new(r.right() - self.s(4.0) - w, y, w, bw);
        let open = self.preview.look.card;
        let state = Button::of(self.hover == Hit::Preview(PaneHit::Title), open, true);
        let p = Paint::new(g, t, self.scale);
        if let Some(fill) = p.fill_of(state) {
            p.g.dl.round_rect(tr, fill, self.s(4.0));
        }
        let ink = p.ink_of(state, t.text);
        let m = g.metrics(font);
        g.text(tr.x + self.s(6.0), (tr.y + (tr.h + m.ascent - m.descent) / 2.0).round(), &title, font, ink, tw);
        centered(g, Rect::new(tr.right() - self.s(16.0), tr.y, self.s(14.0), tr.h), if open { G_BELOW } else { G_NEXT }, chev, ink);
        self.preview.r.title = tr;
        self.preview.buttons.push((tr, PaneHit::Title));
    }

    fn foot_blocks(&self) -> (Vec<FootBlock>, Option<(S, usize)>) {
        match &self.preview.content {
            Content::Doc(d) => {
                let mut v = vec![("", self.opts.inspector_size.to_string(), Some(PaneHit::Size)), (G_LINES, super::thousands(d.paras.len() as u64), None)];
                if !d.encoding.is_empty() {
                    v.push(("", d.encoding.to_owned(), None));
                }
                (v, Some((S::PreviewFilterLines, self.preview.doc.as_ref().map_or(d.paras.len(), |l| l.shown))))
            }
            Content::Image => {
                let Some((w, h)) = self.preview.image.as_ref().map(|i| (i.width as f32, i.height as f32)) else { return (Vec::new(), None) };
                let b = self.preview.r.body;
                let k = image_factor(self.preview.look.scale, self.preview.look.zoom, (w, h), (b.w, b.h));
                (vec![("", format!("{}%", (k * 100.0).round()), Some(PaneHit::Size)), ("", format!("{} \u{D7} {}", w as u32, h as u32), None)], None)
            }
            Content::Folder(f) => (vec![(super::G_FILE, super::thousands(f.files), None), (super::G_FOLDER, super::thousands(f.dirs), None)], Some((S::PreviewFilterFiles, self.folder_rows(f).len()))),
            _ => (Vec::new(), None),
        }
    }

    fn draw_inspector_foot<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme) {
        let r = self.preview.r.foot;
        g.dl.rect(r, t.chrome);
        g.dl.rect(Rect::new(r.x, r.y, r.w, 1.0), t.separator);
        let font = self.ui_font(12.0);
        let icons = self.icon_font(11.0);
        let m = g.metrics(font);
        let baseline = (r.y + (r.h + m.ascent - m.descent) / 2.0).round();
        let mut right = r.right() - self.s(6.0);
        let block_h = r.h - self.s(4.0);
        let (blocks, filterable) = self.foot_blocks();
        for (glyph, label, hit) in blocks {
            let icon_w = if glyph.is_empty() { 0.0 } else { self.s(18.0) };
            let w = g.measure(&label, font) + self.s(14.0) + icon_w;
            let br = Rect::new(right - w, r.y + self.s(2.0), w, block_h);
            if let Some(h) = hit {
                let fill = if self.hover == Hit::Preview(h) { t.button_hover } else { t.input_bg };
                g.dl.round_rect(br, fill, self.s(3.0));
                g.dl.round_rect(br, t.accent.alpha(0x40), self.s(3.0));
                self.preview.buttons.push((br, h));
            }
            if icon_w > 0.0 {
                centered(g, Rect::new(br.x + self.s(4.0), br.y, icon_w, br.h), glyph, icons, t.text_dim);
            }
            g.text(br.x + self.s(7.0) + icon_w, baseline, &label, font, if hit.is_some() { t.text } else { t.text_dim }, f32::INFINITY);
            right = br.x - self.s(6.0);
        }
        let Some((label, count)) = filterable else {
            self.preview.r.filter = Rect::default();
            return;
        };
        let fr = Rect::new(r.x + self.s(6.0), r.y + self.s(2.0), (right - r.x - self.s(12.0)).clamp(0.0, self.s(260.0)), block_h);
        self.preview.r.filter = fr;
        let focused = self.preview.filtering && self.focus == Focus::Preview;
        let mut p = Paint::new(g, t, self.scale);
        p.field(fr, focused);
        centered(g, Rect::new(fr.x + self.s(2.0), fr.y, self.s(18.0), fr.h), "\u{E721}", self.icon_font(10.0), t.text_dim);
        let clear_w = if self.preview.filter.is_some() { self.s(18.0) } else { 0.0 };
        let inner = Rect::new(fr.x + self.s(22.0), fr.y, (fr.w - self.s(26.0) - clear_w).max(0.0), fr.h);
        let placeholder = crate::tr!(label, n = super::thousands(count as u64));
        let preedit = focused.then_some(&self.preedit);
        let sc = self.scale;
        let field = self.preview.filter.get_or_insert_with(Field::default);
        field.draw(g, t, inner, font, focused, &placeholder, sc, preedit);
        if self.preview.filter.as_ref().is_some_and(|f| f.text().is_empty()) && !focused {
            self.preview.filter = None;
        }
        self.preview.buttons.push((fr, PaneHit::Filter));
        if clear_w > 0.0 {
            let cr = Rect::new(fr.right() - clear_w - self.s(2.0), fr.y, clear_w, fr.h);
            self.tool_button(g, t, cr, G_CLOSE, PaneHit::FilterClear, false);
        }
    }

    fn draw_card<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme) {
        let card = self.preview.r.card;
        self.preview.chips.clear();
        self.preview.r.button = Rect::default();
        if card.w <= 0.0 || card.h <= 0.0 {
            return;
        }
        let Some(s) = self.preview.subject.clone() else { return };
        let head_h = self.s(30.0);
        let saved = (self.preview.r.body, self.preview.scroll);
        let inner = Rect::new(card.x + self.s(10.0), card.y + head_h, card.w - self.s(20.0), 0.0);
        self.preview.r.body = Rect { h: (card.h - head_h - self.s(8.0)).max(0.0), ..inner };
        self.preview.scroll = self.preview.card_scroll;
        let from = g.dl.instances.len();
        g.dl.set_clip(Some(self.preview.r.body));
        let extent = self.draw_details(g, t, &s);
        g.dl.set_clip(None);
        let used = (head_h + extent + self.s(10.0)).min(card.h);
        let shown = Rect { h: used, ..card };
        let body = self.preview.r.body;
        self.preview.r.body = saved.0;
        self.preview.scroll = saved.1;
        self.preview.card_extent = extent;
        self.preview.r.card = shown;
        let mut p = Paint::new(g, t, self.scale);
        let before = p.g.dl.instances.len();
        p.frame(shown);
        let behind = p.g.dl.instances.len() - before;
        p.g.dl.instances[from..].rotate_right(behind);
        let bold = FontStyle::new(FontFamily::Ui, 13.0 * self.scale, true);
        let m = g.metrics(bold);
        let bw = self.s(TOOL_W);
        let close = Rect::new(shown.right() - self.s(6.0) - bw, shown.y + (head_h - bw) / 2.0, bw, bw);
        let details = Rect { x: close.x - bw, ..close };
        g.text(shown.x + self.s(10.0), (shown.y + (head_h + m.ascent - m.descent) / 2.0).round(), &s.title, bold, t.text, details.x - shown.x - self.s(14.0));
        let on = self.preview.state.details;
        self.tool_button(g, t, details, if on { G_DETAILS_ON } else { G_DETAILS_OFF }, PaneHit::CardDetails, on);
        self.tool_button(g, t, close, G_BELOW, PaneHit::CardClose, false);
        g.dl.rect(Rect::new(shown.x + 1.0, shown.y + head_h - 1.0, shown.w - 2.0, 1.0), t.separator);
        let _ = body;
    }

    fn text_line<B: TextBackend>(&self, g: &mut Gfx<B>, y: &mut f32, s: &str, font: FontStyle, color: Color) {
        let b = self.preview.r.body;
        let m = g.metrics(font);
        let h = self.s(ROW).max((m.ascent + m.descent + self.s(6.0)).round());
        let top = b.y + *y - self.preview.scroll;
        if top + h >= b.y && top <= b.bottom() {
            g.text(b.x, (top + (h + m.ascent - m.descent) / 2.0).round(), s, font, color, b.w);
        }
        *y += h;
    }

    fn field<B: TextBackend>(&self, g: &mut Gfx<B>, y: &mut f32, label: &str, value: &str, t: &Theme) {
        let b = self.preview.r.body;
        let font = self.ui_font(12.5);
        let m = g.metrics(font);
        let h = self.s(ROW);
        let top = b.y + *y - self.preview.scroll;
        if top + h >= b.y && top <= b.bottom() {
            let base = (top + (h + m.ascent - m.descent) / 2.0).round();
            let lw = (b.w * 0.34).round().min(self.s(110.0));
            g.text(b.x, base, label, font, t.text, lw - self.s(8.0));
            g.text(b.x + lw, base, value, font, t.text_dim, b.w - lw);
        }
        *y += h;
    }

    fn type_label(&self, s: &Subject) -> String {
        let Some(e) = s.entry else { return S::DetailsTypeFolder.t().into() };
        if e.attrs & attr::VOLUME != 0 {
            return S::DetailsTypeDrive.t().into();
        }
        self.ext_type_label(&s.ext.to_ascii_lowercase(), e.is_dir())
    }

    fn draw_icon<B: TextBackend>(&mut self, g: &mut Gfx<B>, s: &Subject, r: Rect) {
        let e = s.entry.unwrap_or(Entry { size: 0, modified: 0, created: 0, accessed: 0, name_off: 0, attrs: attr::DIR, name_len: 0, ext_len: 0, tags: 0 });
        let name = s.path.as_ref().map(|p| p.file_name().to_vec()).unwrap_or_default();
        let (key, kind) = icon_key(&name, &s.ext, &e, r.w as u32, &s.dir_key, self.icon_epoch);
        if g.image(key, r) {
            return;
        }
        let glyph = if e.attrs & attr::VOLUME != 0 { super::G_DRIVE } else if e.is_dir() { super::G_FOLDER } else { super::G_FILE };
        centered(g, r, glyph, FontStyle::new(FontFamily::Icons, r.h * 0.8, false), self.theme().text_dim);
        if let (false, Some(path)) = (self.icons.contains_key(&key), s.path.clone()) {
            self.icons.insert(key, IconState::Pending);
            self.effects.push(Effect::Icon(IconRequest { key, kind, path, size: r.w as u32, gen: 0, row: 0 }));
        }
    }

    fn header<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme, s: &Subject, y: &mut f32) {
        let b = self.preview.r.body;
        let px = self.s(ICON_PX);
        let ir = Rect::new(b.x, b.y + *y - self.preview.scroll, px, px);
        self.draw_icon(g, s, ir);
        let x = ir.right() + self.s(12.0);
        let (bold, small) = (FontStyle::new(FontFamily::Ui, 14.0 * self.scale, true), self.ui_font(12.5));
        let m = g.metrics(bold);
        g.text(x, ir.y + self.s(4.0) + m.ascent, &s.title, bold, t.text, b.right() - x);
        let sub = if s.many.is_some() { String::new() } else { self.type_label(s) };
        g.text(x, ir.y + self.s(26.0) + m.ascent, &sub, small, t.text_dim, b.right() - x);
        *y += px + self.s(12.0);
    }

    fn draw_details<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme, s: &Subject) -> f32 {
        let mut y = 0.0;
        let mut text = String::new();
        let (off, style) = (self.utc_offset_min, self.style.clone());
        let mut date = |v: i64| {
            text.clear();
            format::datetime_in(&mut text, v, off, &style);
            text.clone()
        };
        let mut rows: Vec<(&str, String)> = Vec::new();
        if let Some(p) = &s.path {
            rows.push((S::PreviewFieldPath.t(), p.display().into_owned()));
        }
        rows.push((S::PreviewFieldType.t(), if s.many.is_some() { String::new() } else { self.type_label(s) }));
        if let Some((n, bytes)) = s.many {
            rows.push((S::PreviewFieldItems.t(), super::thousands(n as u64)));
            rows.push((S::PreviewFieldTotalSize.t(), self.size_text(bytes)));
        }
        if let Some((total, free)) = s.drive {
            rows.push((S::PreviewFieldUsedSpace.t(), self.size_text(total.saturating_sub(free))));
            rows.push((S::PreviewFieldFreeSpace.t(), self.size_text(free)));
            rows.push((S::PreviewFieldTotalSize.t(), self.size_text(total)));
        }
        let details = self.preview.state.details;
        if let Some(e) = s.entry.filter(|e| e.attrs & attr::VOLUME == 0) {
            if !e.is_dir() || e.attrs & attr::SIZE_KNOWN != 0 {
                rows.push((S::DetailsColumnSize.t(), crate::trn!(P::PreviewFieldSizeBytes, e.size, size = self.size_text(e.size))));
            }
            if !details {
                if e.created > 0 {
                    rows.push((S::PreviewFieldCreated.t(), date(e.created)));
                }
                if e.modified > 0 {
                    rows.push((S::PreviewFieldModified.t(), date(e.modified)));
                }
            }
            let flags: Vec<&str> = [(attr::READONLY, S::PreviewAttrReadOnly), (attr::HIDDEN, S::PreviewAttrHidden), (attr::SYSTEM, S::PreviewAttrSystem), (attr::LINK, S::PreviewAttrLink)].iter().filter(|f| e.attrs & f.0 != 0).map(|f| f.1.t()).collect();
            if !flags.is_empty() {
                rows.push((S::PreviewFieldAttributes.t(), flags.join(", ")));
            }
        }
        rows.retain(|r| !r.1.is_empty());
        for (label, value) in &rows {
            self.field(g, &mut y, label, value, t);
        }
        self.tag_row(g, s, &mut y, t);
        if let (Content::Folder(f), true) = (&self.preview.content, s.many.is_none()) {
            y += self.s(6.0);
            self.folder_lines(g, t, f, &mut y);
        }
        if details {
            let b = self.preview.r.body;
            let top = b.y + y + self.s(4.0) - self.preview.scroll;
            g.dl.rect(Rect::new(b.x, top, b.w, 1.0), t.separator);
            y += self.s(8.0);
            let groups = self.preview.props.clone();
            if self.preview.props_loading {
                self.text_line(g, &mut y, S::PreviewReading.t(), self.ui_font(12.5), t.text_dim);
            }
            let bold = FontStyle::new(FontFamily::Ui, 12.5 * self.scale, true);
            for grp in &groups {
                self.text_line(g, &mut y, &grp.title, bold, t.text);
                for (k, v) in &grp.rows {
                    self.field(g, &mut y, k, v, t);
                }
                y += self.s(4.0);
            }
        }
        if s.entry.is_some() || s.many.is_some() {
            let b = self.preview.r.body;
            y += self.s(6.0);
            let br = Rect::new(b.x, b.y + y - self.preview.scroll, self.s(110.0).min(b.w), self.s(BUTTON_H));
            let c = if self.hover == Hit::Preview(PaneHit::Properties) { t.button_hover } else { t.input_bg };
            g.dl.round_rect(br, c, self.s(4.0));
            g.dl.outline(br, t.input_border, self.s(4.0), 1.0);
            centered(g, br, S::PreviewProperties.t(), self.ui_font(12.5), t.text);
            self.preview.r.button = br;
            y += self.s(BUTTON_H);
        }
        y
    }

    fn tag_row<B: TextBackend>(&mut self, g: &mut Gfx<B>, s: &Subject, y: &mut f32, t: &Theme) {
        if (s.entry.is_none() && s.many.is_none()) || self.tab().view_loc.path().kind() != PathKind::Drive {
            return;
        }
        let b = self.preview.r.body;
        let (font, label) = (self.ui_font(12.0), self.ui_font(12.5));
        let (lm, fm) = (g.metrics(label), g.metrics(font));
        let (h, pad, dot, gap) = (self.s(22.0), self.s(8.0), self.s(8.0), self.s(6.0));
        let lw = (b.w * 0.34).round().min(self.s(110.0));
        let top = b.y + *y - self.preview.scroll;
        g.text(b.x, (top + (self.s(ROW) + lm.ascent - lm.descent) / 2.0).round(), S::DetailsColumnTag.t(), label, t.text, lw - self.s(8.0));
        let (any, all) = self.selection_tags();
        let chips: Vec<(PaneHit, String, Option<Color>)> = self.tag_defs().iter().enumerate().map(|(k, d)| (PaneHit::Tag(k), d.name.clone(), Some(Color::rgb(d.color)))).chain([(PaneHit::NewTag, S::PreviewNewTag.t().to_owned(), None)]).collect();
        let (left, mut x, mut row) = (b.x + lw, b.x + lw, top + ((self.s(ROW) - h) / 2.0).round());
        for (hit, name, color) in chips {
            let mark = if color.is_some() { dot + gap } else { 0.0 };
            let w = (2.0 * pad + mark + g.measure(&name, font)).round().min(b.right() - left);
            if x + w > b.right() && x > left {
                (x, row) = (left, row + h + self.s(4.0));
            }
            let r = Rect::new(x, row, w, h);
            let bit = |mask: u32| matches!(hit, PaneHit::Tag(k) if mask & (1 << k) != 0);
            let fill = if bit(all) { Some(t.selection) } else if self.hover == Hit::Preview(hit) { Some(t.button_hover) } else { None };
            if let Some(c) = fill {
                g.dl.round_rect(r, c, h / 2.0);
            }
            g.dl.outline(r, t.input_border, h / 2.0, 1.0);
            if let Some(c) = color {
                let d = Rect::new(r.x + pad, r.y + ((h - dot) / 2.0).round(), dot, dot);
                if bit(any) {
                    g.dl.round_rect(d, c, dot / 2.0);
                } else {
                    g.dl.outline(d, c, dot / 2.0, 1.0);
                }
            }
            g.text(r.x + pad + mark, (r.y + (h + fm.ascent - fm.descent) / 2.0).round(), &name, font, if bit(any) { t.text } else { t.text_dim }, w - 2.0 * pad - mark);
            self.preview.chips.push((r, hit));
            x += w + self.s(4.0);
        }
        *y += row + h - top + self.s(6.0);
    }

    fn draw_preview_body<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme) -> f32 {
        let Some(s) = self.preview.subject.clone() else { return 0.0 };
        let b = self.preview.r.body;
        let small = self.ui_font(12.5);
        let mut y = 0.0;
        let dims = self.preview.image.as_ref().map(|i| (i.width as f32, i.height as f32));
        if s.many.is_some() {
            return self.draw_types(g, t);
        }
        match &self.preview.content {
            Content::Doc(_) => return self.draw_doc(g, t),
            Content::Image => {
                if let Some((w, h)) = dims {
                    return self.draw_image(g, t, w, h);
                }
            }
            Content::Handler => {
                self.text_line(g, &mut y, S::PreviewLoading.t(), small, t.text_dim);
                return y;
            }
            &Content::Media { video } => return self.draw_media(g, t, &s, video),
            Content::Folder(f) => {
                let f = f.clone();
                return self.draw_folder(g, t, &f);
            }
            Content::Loading if s.kind != Kind::Folder && s.entry.is_some() => {
                self.text_line(g, &mut y, S::UiStatusLoading.t(), small, t.text_dim);
                return y;
            }
            _ => {}
        }
        let note = match &self.preview.content {
            Content::Basic(n) => n.as_ref().map(Note::text),
            _ => None,
        };
        if let Some((w, h)) = dims {
            let k = (b.w / w).min(self.s(220.0) / h).min(1.0);
            let (dw, dh) = ((w * k).round(), (h * k).round());
            g.dl.preview(Rect::new((b.x + (b.w - dw) / 2.0).round(), b.y - self.preview.scroll, dw, dh));
            y += dh + self.s(12.0);
        }
        self.header(g, t, &s, &mut y);
        let mut rows: Vec<(&str, String)> = Vec::new();
        if let Some((total, free)) = s.drive {
            let bar = Rect::new(b.x, b.y + y - self.preview.scroll, b.w, self.s(8.0));
            g.dl.round_rect(bar, t.input_bg, self.s(4.0));
            let used = if total > 0 { (total - free.min(total)) as f32 / total as f32 } else { 0.0 };
            g.dl.round_rect(Rect { w: (bar.w * used).round(), ..bar }, if used > 0.9 { t.error } else { t.accent }, self.s(4.0));
            y += self.s(16.0);
            rows.push((S::PreviewFieldFree.t(), crate::tr!(S::PreviewFieldFreeOf, free = self.size_text(free), total = self.size_text(total))));
        }
        if let Some(e) = s.entry.filter(|e| !e.is_dir()) {
            rows.push((S::DetailsColumnSize.t(), self.size_text(e.size)));
            let mut d = String::new();
            format::datetime_in(&mut d, e.modified, self.utc_offset_min, &self.style);
            rows.push((S::PreviewFieldModified.t(), d));
        }
        for (label, value) in &rows {
            self.field(g, &mut y, label, value, t);
        }
        if let Some(n) = note {
            y += self.s(6.0);
            for l in n.lines() {
                self.text_line(g, &mut y, l, small, t.text_dim);
            }
        }
        y
    }

    fn draw_types<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme) -> f32 {
        let b = self.preview.r.body;
        let font = self.ui_font(12.5);
        let m = g.metrics(font);
        let h = self.s(ROW + 4.0);
        let icons = self.icon_font(12.0);
        let mut y = 0.0;
        for (label, n) in self.preview.types.clone() {
            let top = b.y + y - self.preview.scroll;
            if top + h >= b.y && top <= b.bottom() {
                let base = (top + (h + m.ascent - m.descent) / 2.0).round();
                let dir = label == S::DetailsTypeFolder.t();
                centered(g, Rect::new(b.x, top, self.s(18.0), h), if dir { super::G_FOLDER } else { super::G_FILE }, icons, if dir { t.accent } else { t.text_dim });
                let count = super::thousands(n as u64);
                let cw = g.measure(&count, font);
                g.text(b.x + self.s(26.0), base, &label, font, t.text, b.w - self.s(34.0) - cw);
                g.text(b.right() - cw, base, &count, font, t.text_dim, f32::INFINITY);
            }
            y += h;
        }
        y
    }

    fn draw_image<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme, w: f32, h: f32) -> f32 {
        let b = self.preview.r.body;
        let look = self.preview.look;
        let k = image_factor(look.scale, look.zoom, (w, h), (b.w, b.h));
        let (dw, dh) = ((w * k).round(), (h * k).round());
        let y0 = if dh <= b.h { (b.y + (b.h - dh) / 2.0).round() } else { b.y - self.preview.scroll };
        let r = Rect::new((b.x + (b.w - dw) / 2.0).round(), y0, dw, dh);
        if look.grid {
            let cell = self.s(CHECKER).max(4.0);
            let visible = Rect::new(r.x.max(b.x), r.y.max(b.y), (r.right().min(b.right()) - r.x.max(b.x)).max(0.0), (r.bottom().min(b.bottom()) - r.y.max(b.y)).max(0.0));
            g.dl.rect(visible, Color::rgb(if self.dark { 0x3A3A3A } else { 0xFFFFFF }));
            let dark = Color::rgb(if self.dark { 0x2A2A2A } else { 0xE4E4E4 });
            let (cols, rows) = ((visible.w / cell).ceil() as usize, (visible.h / cell).ceil() as usize);
            for row in 0..rows {
                for col in (row % 2..cols).step_by(2) {
                    let x = visible.x + col as f32 * cell;
                    let yy = visible.y + row as f32 * cell;
                    g.dl.rect(Rect::new(x, yy, cell.min(visible.right() - x), cell.min(visible.bottom() - yy)), dark);
                }
            }
        }
        g.dl.preview(r);
        let _ = t;
        dh
    }

    fn folder_rows<'a>(&self, f: &'a FolderSummary) -> Vec<&'a preview::FolderItem> {
        let q = self.preview.query();
        f.entries.iter().filter(|e| q.is_empty() || e.name.to_lowercase().contains(&q)).collect()
    }

    fn draw_folder<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme, f: &FolderSummary) -> f32 {
        let b = self.preview.r.body;
        let font = self.ui_font(12.5);
        let m = g.metrics(font);
        let h = self.s(ROW + 2.0);
        let icons = self.icon_font(12.0);
        let (size_w, date_w) = (self.s(80.0), if b.w > self.s(360.0) { self.s(130.0) } else { 0.0 });
        let name_w = (b.w - size_w - date_w).max(0.0);
        let head = Rect::new(b.x, b.y, b.w, self.s(24.0));
        let rows = self.folder_rows(f);
        let mut y = head.h;
        let mut text = String::new();
        for e in &rows {
            let top = b.y + y - self.preview.scroll;
            if top + h >= head.bottom() && top <= b.bottom() {
                let base = (top + (h + m.ascent - m.descent) / 2.0).round();
                centered(g, Rect::new(b.x, top, self.s(18.0), h), if e.dir { super::G_FOLDER } else { super::G_FILE }, icons, if e.dir { t.accent } else { t.text_dim });
                g.text(b.x + self.s(24.0), base, &e.name, font, t.text, name_w - self.s(30.0));
                if date_w > 0.0 && e.modified > 0 {
                    text.clear();
                    format::datetime_in(&mut text, e.modified, self.utc_offset_min, &self.style);
                    g.text(b.x + name_w, base, &text, font, t.text_dim, date_w - self.s(8.0));
                }
                if !e.dir {
                    let s = self.size_text(e.size);
                    let sw = g.measure(&s, font);
                    g.text(b.right() - sw, base, &s, font, t.text_dim, f32::INFINITY);
                }
            }
            y += h;
        }
        g.dl.rect(head, t.bg);
        let hm = g.metrics(self.ui_font(12.0));
        let hb = (head.y + (head.h + hm.ascent - hm.descent) / 2.0).round();
        let hf = self.ui_font(12.0);
        g.text(b.x + self.s(24.0), hb, S::DetailsColumnName.t(), hf, t.text_dim, name_w);
        if date_w > 0.0 {
            g.text(b.x + name_w, hb, S::DetailsColumnModified.t(), hf, t.text_dim, date_w);
        }
        let sl = S::DetailsColumnSize.t();
        let sw = g.measure(sl, hf);
        g.text(b.right() - sw, hb, sl, hf, t.text_dim, f32::INFINITY);
        g.dl.rect(Rect::new(b.x, head.bottom() - 1.0, b.w, 1.0), t.separator);
        if rows.is_empty() {
            self.text_line(g, &mut y, S::PreviewFolderEmpty.t(), font, t.text_dim);
        }
        y
    }

    fn folder_lines<B: TextBackend>(&self, g: &mut Gfx<B>, t: &Theme, f: &FolderSummary, y: &mut f32) {
        let mut lines = vec![(format!("{}, {}", crate::trn!(P::DetailsCountFolders, f.dirs), crate::trn!(P::DetailsCountFiles, f.files)), t.text)];
        let mut size = self.size_text(f.bytes);
        if !f.complete {
            size = crate::trn!(P::PreviewFolderScanned, preview::FOLDER_SCAN, size = size);
        }
        lines.push((size, t.text));
        if f.hidden > 0 {
            lines.push((crate::trn!(P::PreviewFolderHidden, f.hidden), t.text_dim));
        }
        if f.newest > 0 {
            let mut d = String::new();
            format::datetime_in(&mut d, f.newest, self.utc_offset_min, &self.style);
            lines.push((crate::tr!(S::PreviewFolderLastChange, date = d), t.text_dim));
        }
        if !f.types.is_empty() {
            let types: Vec<String> = f.types.iter().map(|(e, n)| format!("{} {}", if e.is_empty() { S::PreviewFolderNoExtension.t() } else { e }, super::thousands(*n))).collect();
            lines.push((crate::tr!(S::PreviewFolderMostCommon, types = types.join(", ")), t.text_dim));
        }
        for (l, c) in lines {
            self.text_line(g, y, &l, self.ui_font(12.5), c);
        }
    }

    fn draw_media<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme, s: &Subject, video: bool) -> f32 {
        let b = self.preview.r.body;
        let ch = self.s(CONTROLS_H);
        let area = Rect::new(b.x, b.y, b.w, (b.h - ch - self.s(8.0)).max(0.0));
        self.preview.r.media = if video { area } else { Rect::default() };
        if let Some((w, h)) = self.preview.image.as_ref().map(|i| (i.width as f32, i.height as f32)) {
            let k = (area.w / w).min(area.h / h).min(1.0);
            let (dw, dh) = ((w * k).round(), (h * k).round());
            g.dl.preview(Rect::new((area.x + (area.w - dw) / 2.0).round(), (area.y + (area.h - dh) / 2.0).round(), dw, dh));
        } else {
            self.draw_icon(g, s, Rect::new((area.x + (area.w - self.s(ICON_PX)) / 2.0).round(), (area.y + (area.h - self.s(ICON_PX)) / 2.0).round(), self.s(ICON_PX), self.s(ICON_PX)));
        }
        let row = Rect::new(b.x, area.bottom() + self.s(8.0), b.w, ch);
        let m = &self.preview.media;
        let play = Rect::new(row.x, row.y, ch, ch);
        let icons = self.icon_font(12.0);
        if self.hover == Hit::Preview(PaneHit::Play) {
            g.dl.round_rect(play, t.button_hover, self.s(4.0));
        }
        centered(g, play, if m.playing { G_PAUSE } else { G_PLAY }, icons, t.text);
        let font = self.ui_font(12.0);
        let mut time = String::new();
        clock(&mut time, m.position);
        time.push_str(" / ");
        clock(&mut time, m.duration);
        let tw = g.measure(&time, font);
        let vol_w = self.s(64.0).min(row.w / 4.0);
        let vol = Rect::new(row.right() - vol_w, row.y + (ch - self.s(6.0)) / 2.0, vol_w, self.s(6.0));
        let glyph = Rect::new(vol.x - self.s(24.0), row.y, self.s(20.0), ch);
        let tx = glyph.x - self.s(10.0) - tw;
        let seek = Rect::new(play.right() + self.s(8.0), vol.y, (tx - self.s(10.0) - play.right() - self.s(8.0)).max(0.0), self.s(6.0));
        let frac = if m.duration > 0.0 { (m.position / m.duration).clamp(0.0, 1.0) as f32 } else { 0.0 };
        for (r, f) in [(seek, frac), (vol, self.preview.state.volume)] {
            g.dl.round_rect(r, t.input_bg, r.h / 2.0);
            g.dl.round_rect(Rect { w: (r.w * f).round(), ..r }, t.accent, r.h / 2.0);
        }
        let fm = g.metrics(font);
        g.text(tx, (row.y + (ch + fm.ascent - fm.descent) / 2.0).round(), &time, font, t.text_dim, f32::INFINITY);
        centered(g, glyph, G_VOLUME, self.icon_font(11.0), t.text_dim);
        let grow = |r: Rect| Rect::new(r.x, row.y, r.w, ch);
        let pr = &mut self.preview.r;
        (pr.play, pr.seek, pr.volume) = (play, grow(seek), grow(vol));
        b.h
    }

    fn draw_doc<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme) -> f32 {
        let (sc, dark, mono) = (self.scale, self.dark, super::options::mono_px(self.opts.inspector_size));
        let b = self.preview.r.body;
        let note_font = self.ui_font(12.0);
        let num_font = FontStyle::new(FontFamily::Mono, mono * sc, false);
        let query = self.preview.query();
        let look = self.preview.look;
        let p = &mut self.preview;
        let Content::Doc(d) = &p.content else { return 0.0 };
        let wrap = p.effective_wrap(d);
        let numbers = d.mono && look.lines;
        let digits = (d.paras.len().max(1).ilog10() + 1) as usize;
        let gutter = if numbers { char_w(g, &mut p.widths, '0', num_font) * digits as f32 + (12.0 * sc).round() } else { 0.0 };
        let width = (b.w - gutter).max(0.0);
        let key = hash::hash(&(width.to_bits(), sc.to_bits(), mono.to_bits(), wrap, &query));
        if p.doc.as_ref().is_none_or(|l| l.key != key) {
            let mut lay = layout_doc(g, d, width, (sc, mono), &mut p.widths, wrap, &query);
            lay.key = key;
            p.doc = Some(lay);
        }
        p.r.gutter = gutter;
        let lay = p.doc.as_ref().unwrap();
        let scroll = p.scroll;
        let first = lay.lines.partition_point(|l| l.y + l.h < scroll);
        let x0 = b.x + gutter;
        let dot_color = t.text_dim.alpha(0x70);
        for line in &lay.lines[first..] {
            let top = b.y + line.y - scroll;
            if top > b.bottom() {
                break;
            }
            match line.block {
                Block::Code if !d.mono => g.dl.rect(Rect::new(x0, top, width, line.h), t.input_bg),
                Block::Quote => g.dl.rect(Rect::new(x0 + (2.0 * sc).round(), top, (3.0 * sc).round(), line.h), t.separator),
                Block::Rule => g.dl.rect(Rect::new(x0, top + (line.h / 2.0).round(), width, 1.0), t.separator),
                _ => {}
            }
            if numbers && line.num > 0 {
                let n = line.num.to_string();
                let nw = g.measure(&n, num_font);
                g.text(x0 - (8.0 * sc).round() - nw, top + line.base, &n, num_font, t.text_dim.alpha(0xA0), f32::INFINITY);
            }
            for piece in &lay.pieces[line.pieces.start as usize..line.pieces.end as usize] {
                let run = &d.runs[piece.run as usize];
                let font = run_font(run, line.block, sc, mono);
                let s = &d.text[piece.start as usize..piece.end as usize];
                g.text(x0 + piece.x, top + line.base, s, font, tone_color(t, dark, run.tone), f32::INFINITY);
                if look.spaces && s.contains(' ') {
                    let mut x = x0 + piece.x;
                    for c in s.chars() {
                        let cw = char_w(g, &mut p.widths, c, font);
                        if c == ' ' {
                            let dw = char_w(g, &mut p.widths, '\u{B7}', font);
                            g.text(x + ((cw - dw) / 2.0).round(), top + line.base, SPACE_DOT, font, dot_color, f32::INFINITY);
                        }
                        x += cw;
                    }
                }
            }
        }
        let mut h = lay.height;
        if d.truncated {
            let m = g.metrics(note_font);
            let top = b.y + h - scroll + (8.0 * sc).round();
            g.text(x0, top + m.ascent, &crate::trn!(P::PreviewTruncated, preview::TEXT_CHARS), note_font, t.text_dim, width);
            h += (8.0 * sc).round() + m.ascent + m.descent + (4.0 * sc).round();
        }
        h
    }
}


impl App {
    fn size_text(&self, bytes: u64) -> String {
        let mut s = String::new();
        format::size_in(&mut s, bytes, self.style.decimal);
        s
    }
}

#[cfg(test)]
#[path = "../../test/ui/preview_pane.rs"]
mod tests;
