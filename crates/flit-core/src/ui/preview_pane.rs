use super::{centered, icon_key, App, Bitmap, Drag, Effect, Hit, IconRequest, IconState};
use crate::fmt as format;
use crate::hash::{self, FxHashMap};
use crate::input::CursorIcon;
use crate::listing::{attr, Entry};
use crate::name;
use crate::path::{FsPath, PathKind};
use crate::preview::{self, Block, Doc, FolderSummary, HostCmd, HostEvent, Hosted, Kind, MediaCmd, PreviewData, PreviewRequest, PropGroup, Run, Tone, IMAGE_PX};
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
const TABS_H: f32 = 34.0;
const TAB_W: f32 = 74.0;
const PAD: f32 = 14.0;
const ROW: f32 = 22.0;
const ICON_PX: f32 = 48.0;
const BUTTON_H: f32 = 28.0;
const CONTROLS_H: f32 = 34.0;
const TAB_TITLES: [&str; 2] = ["Preview", "Details"];
const G_PLAY: &str = "\u{E768}";
const G_PAUSE: &str = "\u{E769}";
const G_VOLUME: &str = "\u{E767}";

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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PaneHit {
    Split,
    Tab(usize),
    Properties,
    Play,
    Seek,
    Volume,
    Tag(usize),
    NewTag,
    Body,
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
    fn key(&self, details: bool) -> u64 {
        let e = self.entry.map(|e| (e.size, e.modified, e.attrs));
        hash::hash(&(&self.title, self.path.as_ref().map(|p| p.as_bytes()), self.kind, e, self.many, self.drive, details))
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
    Basic(Option<String>),
    Props(Vec<PropGroup>),
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
    pieces: Range<u32>,
}

#[derive(Default)]
struct DocLayout {
    width: f32,
    scale: f32,
    lines: Vec<VLine>,
    pieces: Vec<Piece>,
    height: f32,
}

#[derive(Clone, Copy, Default)]
struct Rects {
    pane: Rect,
    split: Rect,
    tabs: [Rect; 2],
    body: Rect,
    button: Rect,
    play: Rect,
    seek: Rect,
    volume: Rect,
    media: Rect,
}

#[derive(Default)]
pub(super) struct PreviewPane {
    pub state: PaneState,
    key: u64,
    gen: u64,
    cancel: GenToken,
    subject: Option<Subject>,
    content: Content,
    scroll: f32,
    extent: f32,
    doc: Option<DocLayout>,
    widths: FxHashMap<(char, FontStyle), f32>,
    image: Option<Bitmap>,
    image_rev: u64,
    host: Host,
    media: Media,
    peeked: bool,
    requested: Option<Instant>,
    r: Rects,
    chips: Vec<(Rect, PaneHit)>,
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
}

fn ui(px: f32, sc: f32, bold: bool) -> FontStyle {
    FontStyle::new(FontFamily::Ui, px * sc, bold)
}

fn run_font(r: &Run, block: Block, sc: f32) -> FontStyle {
    match block {
        _ if r.mono => FontStyle::new(FontFamily::Mono, 12.5 * sc, false),
        Block::Heading(1) => ui(20.0, sc, true),
        Block::Heading(2) => ui(17.0, sc, true),
        Block::Heading(_) => ui(14.5, sc, true),
        _ => ui(13.0, sc, r.bold),
    }
}

fn para_font(block: Block, mono: bool, sc: f32) -> FontStyle {
    let r = Run { start: 0, end: 0, tone: Tone::Plain, bold: false, mono: mono || block == Block::Code };
    run_font(&r, block, sc)
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

fn layout_doc<B: TextBackend>(g: &mut Gfx<B>, d: &Doc, width: f32, sc: f32, widths: &mut FxHashMap<(char, FontStyle), f32>) -> DocLayout {
    let mut out = DocLayout { width, scale: sc, ..Default::default() };
    let mut y = 0.0;
    for (k, para) in d.paras.iter().enumerate() {
        let block = para.block;
        let base_font = para_font(block, d.mono, sc);
        let m = g.metrics(base_font);
        let lh = ((m.ascent + m.descent) * if d.mono { 1.25 } else { 1.4 }).round();
        let base = ((lh + m.ascent - m.descent) / 2.0).round();
        if block == Block::Rule {
            out.lines.push(VLine { y, h: (12.0 * sc).round(), base, block, pieces: 0..0 });
            y += (12.0 * sc).round();
            continue;
        }
        let indent = match block {
            Block::Item { depth } => ((depth as f32 * 18.0 + 4.0) * sc).round(),
            Block::Quote => (14.0 * sc).round(),
            Block::Code if !d.mono => (8.0 * sc).round(),
            _ => 0.0,
        };
        let (mut x, mut first) = (indent, out.pieces.len());
        let line = |out: &mut DocLayout, first: &mut usize, y: &mut f32| {
            out.lines.push(VLine { y: *y, h: lh, base, block, pieces: *first as u32..out.pieces.len() as u32 });
            *y += lh;
            *first = out.pieces.len();
        };
        for ri in para.runs.clone() {
            let run = d.runs[ri as usize];
            let font = run_font(&run, block, sc);
            let text = d.run_text(&run);
            let mut at = run.start;
            for token in text.split_inclusive(' ') {
                let start = at;
                at += token.len() as u32;
                let trimmed = token.trim_end_matches(' ');
                let tw: f32 = token.chars().map(|c| char_w(g, widths, c, font)).sum();
                let core: f32 = trimmed.chars().map(|c| char_w(g, widths, c, font)).sum();
                if d.wrap && x > indent && x + core > width {
                    line(&mut out, &mut first, &mut y);
                    x = indent;
                    if trimmed.is_empty() {
                        continue;
                    }
                }
                if d.wrap && core > width - indent {
                    let mut pos = start;
                    for c in token.chars() {
                        let cw = char_w(g, widths, c, font);
                        if x > indent && x + cw > width {
                            line(&mut out, &mut first, &mut y);
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
        line(&mut out, &mut first, &mut y);
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

fn plural(n: u64, one: &str, many: &str) -> String {
    format!("{} {}", super::thousands(n), if n == 1 { one } else { many })
}

impl App {
    pub fn pane_state(&self) -> PaneState {
        self.preview.state
    }

    pub fn set_pane_state(&mut self, s: PaneState) {
        self.preview.state = s.sanitized();
        self.layout();
        self.dirty = true;
    }

    pub fn preview_image(&self) -> (u64, Option<&Bitmap>) {
        (self.preview.image_rev, self.preview.image.as_ref())
    }

    pub(super) fn toggle_preview_pane(&mut self) {
        let p = &mut self.preview;
        p.state.visible = !p.state.visible;
        p.peeked = false;
        self.layout();
        self.dirty = true;
    }

    pub(super) fn show_preview_tab(&mut self, details: bool) {
        let p = &mut self.preview;
        if p.state.visible && p.state.details == details {
            p.state.visible = false;
        } else {
            (p.state.visible, p.state.details) = (true, details);
        }
        p.peeked = false;
        self.layout();
        self.dirty = true;
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
        if p.state.visible && !p.state.details && p.peeked {
            p.state.visible = false;
            p.peeked = false;
        } else {
            p.peeked = !p.state.visible;
            (p.state.visible, p.state.details) = (true, false);
        }
        self.layout();
        self.dirty = true;
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
        let (w, h) = self.size;
        let shown = st.visible && !self.search.active && !self.on_home();
        let l = &mut self.layout;
        l.bottom = l.status.y;
        let r = &mut self.preview.r;
        *r = Rects::default();
        if !shown {
            return;
        }
        let (x0, top, bottom) = (l.header.x, l.commands.bottom(), l.status.y);
        let (room_w, room_h) = ((w - x0).max(0.0), (bottom - top).max(0.0));
        if w >= h {
            let pw = s(st.right_w).min((room_w * 0.7).round());
            r.pane = Rect::new(w - pw, top, pw, room_h);
            r.split = Rect::new(r.pane.x - s(3.0), top, s(6.0), room_h);
            l.header.w = (l.header.w - pw).max(0.0);
        } else {
            let ph = s(st.bottom_h).min((room_h * 0.7).round());
            r.pane = Rect::new(x0, bottom - ph, room_w, ph);
            r.split = Rect::new(x0, r.pane.y - s(3.0), room_w, s(6.0));
            l.bottom = r.pane.y;
        }
        for (i, t) in r.tabs.iter_mut().enumerate() {
            *t = Rect::new(r.pane.x + s(8.0) + i as f32 * s(TAB_W + 2.0), r.pane.y + s(4.0), s(TAB_W), s(TABS_H - 8.0));
        }
        let top = r.pane.y + s(TABS_H) + s(8.0);
        r.body = Rect::new(r.pane.x + s(PAD), top, (r.pane.w - 2.0 * s(PAD)).max(0.0), (r.pane.bottom() - s(PAD) - top).max(0.0));
        if st.details {
            r.button = Rect::new(r.body.x, r.body.bottom() - s(BUTTON_H), s(110.0).min(r.body.w), s(BUTTON_H));
            r.body.h = (r.body.h - s(BUTTON_H + 10.0)).max(0.0);
        }
    }

    fn right_side(&self) -> bool {
        self.size.0 >= self.size.1
    }

    pub(super) fn preview_hit(&self, x: f32, y: f32) -> Option<Hit> {
        let r = &self.preview.r;
        if r.split.contains(x, y) {
            return Some(Hit::Preview(PaneHit::Split));
        }
        if !r.pane.contains(x, y) {
            return None;
        }
        let h = match () {
            _ if r.tabs[0].contains(x, y) => PaneHit::Tab(0),
            _ if r.tabs[1].contains(x, y) => PaneHit::Tab(1),
            _ if r.button.contains(x, y) => PaneHit::Properties,
            _ if r.play.contains(x, y) => PaneHit::Play,
            _ if r.seek.contains(x, y) => PaneHit::Seek,
            _ if r.volume.contains(x, y) => PaneHit::Volume,
            _ => self.preview.chips.iter().find(|c| c.0.contains(x, y) && r.body.contains(x, y)).map_or(PaneHit::Body, |c| c.1),
        };
        Some(Hit::Preview(h))
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
            PaneHit::Tab(i) => {
                self.preview.state.details = i == 1;
                self.layout();
            }
            PaneHit::Properties => self.properties(),
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
        if !matches!(self.hover, Hit::Preview(_)) {
            return false;
        }
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
                    .then(|| self.nav.sidebar.places.drives.iter().find(|d| d.path.as_bytes().get(..2).is_some_and(|p| p.eq_ignore_ascii_case(name))).map(|d| (d.total, d.free)))
                    .flatten();
                let dir_key = if in_bin { recycle::origin(l, i).to_vec() } else { base.dir_key };
                Some(Subject { title: name::display(name).into_owned(), path, kind, entry: Some(e), ext: l.ext(i).to_vec(), dir_key, drive, ..base })
            }
            n => Some(Subject { title: format!("{} items selected", super::thousands(n as u64)), many: Some((n, v.selected_size())), path: None, ..base }),
        }
    }

    pub(super) fn sync_preview(&mut self, now: Instant) {
        let shown = self.preview.state.visible && !self.search.active && !self.on_home();
        let subject = if shown { self.subject() } else { None };
        let details = self.preview.state.details;
        let key = subject.as_ref().map_or(0, |s| s.key(details));
        if key == self.preview.key {
            return;
        }
        let gen = self.gen();
        let archived = self.archive_view().is_some();
        let p = &mut self.preview;
        p.key = key;
        p.gen = gen;
        p.cancel.set(gen);
        p.scroll = 0.0;
        p.doc = None;
        p.media = Media::default();
        if p.image.take().is_some() {
            p.image_rev += 1;
        }
        if std::mem::take(&mut p.host).gen != 0 {
            self.effects.push(Effect::Host(HostCmd::Close));
        }
        let p = &mut self.preview;
        p.content = Content::Empty;
        p.requested = None;
        p.subject = subject.clone();
        let Some(s) = subject.filter(|s| s.many.is_none() && s.drive.is_none()) else { return };
        let Some(path) = s.path else { return };
        if archived {
            p.content = Content::Basic(Some("Inside an archive: extract it to preview".into()));
            return;
        }
        let side = p.r.body.w.max(p.r.body.h) * 2.0;
        let px = (side as u32).clamp(256, IMAGE_PX);
        p.content = Content::Loading;
        p.requested = Some(now);
        let req = PreviewRequest { gen, cancel: p.cancel.clone(), path, kind: s.kind, size: s.entry.map_or(0, |e| e.size), details, px };
        self.effects.push(Effect::Preview(req));
    }

    pub fn on_preview(&mut self, gen: u64, data: PreviewData) {
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
            PreviewData::Props(groups) => (Content::Props(groups), None, None),
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
                tracing::info!("hosted preview shown {total:.1} ms after selection, {ms} ms in the handler");
            }
            HostEvent::Failed { gen, message } if gen == p.host.gen => {
                let media = !p.host.handler;
                p.host = Host::default();
                p.content = if media { Content::Basic(Some(format!("Cannot play this file: {message}"))) } else { Content::Basic(Some(message)) };
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
        let covered = self.nav.palette.is_some() || self.jobs.open || self.jobs.modal() || self.search.active || self.suggesting();
        let want = match () {
            _ if covered || p.r.pane.w <= 0.0 || p.state.details => None,
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
        if r.pane.w <= 0.0 || r.pane.h <= 0.0 {
            return;
        }
        g.dl.rect(r.pane, t.bg);
        let hot = matches!(self.hover, Hit::Preview(PaneHit::Split)) || matches!(self.drag, Drag::Preview { hit: PaneHit::Split, .. });
        let line = if self.right_side() { Rect::new(r.pane.x, r.pane.y, 1.0, r.pane.h) } else { Rect::new(r.pane.x, r.pane.y, r.pane.w, 1.0) };
        g.dl.rect(line, if hot { t.accent } else { t.separator });
        let font = self.ui_font(12.5);
        let details = self.preview.state.details;
        for (i, title) in TAB_TITLES.iter().enumerate() {
            let tr = r.tabs[i];
            let active = (i == 1) == details;
            if self.hover == Hit::Preview(PaneHit::Tab(i)) {
                g.dl.round_rect(tr, t.button_hover, self.s(4.0));
            }
            centered(g, tr, title, font, if active { t.text } else { t.text_dim });
            if active {
                g.dl.rect(Rect::new(tr.x + self.s(16.0), tr.bottom() - self.s(2.0), tr.w - self.s(32.0), self.s(2.0)), t.accent);
            }
        }
        g.dl.set_clip(Some(r.body));
        let extent = if details { self.draw_details(g, t) } else { self.draw_preview_body(g, t) };
        g.dl.set_clip(None);
        let p = &mut self.preview;
        p.extent = extent;
        p.scroll = p.scroll.clamp(0.0, (extent - r.body.h).max(0.0));
        if !matches!(p.content, Content::Loading | Content::Empty | Content::Handler) {
            if let Some(t) = p.requested.take() {
                tracing::info!("preview drawn {:.1} ms after selection, {} KB held", t.elapsed().as_secs_f64() * 1e3, p.held() / 1024);
            }
        }
        if details {
            let b = r.button;
            let c = if self.hover == Hit::Preview(PaneHit::Properties) { t.button_hover } else { t.input_bg };
            g.dl.round_rect(b, c, self.s(4.0));
            g.dl.outline(b, t.input_border, self.s(4.0), 1.0);
            centered(g, b, "Properties", self.ui_font(12.5), t.text);
        }
        self.place_host();
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
            let lw = (b.w * 0.38).round().min(self.s(140.0));
            g.text(b.x, base, label, font, t.text_dim, lw - self.s(8.0));
            g.text(b.x + lw, base, value, font, t.text, b.w - lw);
        }
        *y += h;
    }

    fn type_label(&self, s: &Subject) -> String {
        let Some(e) = s.entry else { return "File folder".into() };
        if e.attrs & attr::VOLUME != 0 {
            return "Drive".into();
        }
        if e.is_dir() {
            return "File folder".into();
        }
        match self.type_names.get(&s.ext.to_ascii_lowercase()) {
            Some(Some(n)) if !n.is_empty() => n.clone(),
            _ if s.ext.is_empty() => "File".into(),
            _ => format!("{} file", name::display(&s.ext).to_uppercase()),
        }
    }

    fn draw_icon<B: TextBackend>(&mut self, g: &mut Gfx<B>, s: &Subject, r: Rect) {
        let e = s.entry.unwrap_or(Entry { size: 0, modified: 0, created: 0, name_off: 0, attrs: attr::DIR, name_len: 0, ext_len: 0, tags: 0 });
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

    fn draw_details<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme) -> f32 {
        let Some(s) = self.preview.subject.clone() else { return 0.0 };
        let mut y = 0.0;
        self.header(g, t, &s, &mut y);
        let mut text = String::new();
        let (off, style) = (self.utc_offset_min, self.style.clone());
        let mut date = |v: i64| {
            text.clear();
            format::datetime_in(&mut text, v, off, &style);
            text.clone()
        };
        let mut rows: Vec<(&str, String)> = Vec::new();
        if let Some((n, bytes)) = s.many {
            rows.push(("Items", super::thousands(n as u64)));
            rows.push(("Total size", self.size_text(bytes)));
        }
        if let Some((total, free)) = s.drive {
            rows.push(("Used space", self.size_text(total.saturating_sub(free))));
            rows.push(("Free space", self.size_text(free)));
            rows.push(("Total size", self.size_text(total)));
        }
        if let Some(e) = s.entry.filter(|e| e.attrs & attr::VOLUME == 0) {
            if !e.is_dir() || e.attrs & attr::SIZE_KNOWN != 0 {
                rows.push(("Size", format!("{} ({} bytes)", self.size_text(e.size), super::thousands(e.size))));
            }
            if e.modified > 0 {
                rows.push(("Modified", date(e.modified)));
            }
            if e.created > 0 {
                rows.push(("Created", date(e.created)));
            }
            let flags: Vec<&str> = [(attr::READONLY, "Read-only"), (attr::HIDDEN, "Hidden"), (attr::SYSTEM, "System"), (attr::LINK, "Link")].iter().filter(|f| e.attrs & f.0 != 0).map(|f| f.1).collect();
            if !flags.is_empty() {
                rows.push(("Attributes", flags.join(", ")));
            }
        }
        if let Some(dir) = s.path.as_ref().and_then(|p| p.parent()).filter(|_| s.entry.is_some()) {
            rows.push(("Folder", dir.display().into_owned()));
        }
        for (label, value) in &rows {
            self.field(g, &mut y, label, value, t);
        }
        self.tag_row(g, &s, &mut y, t);
        let groups = match &self.preview.content {
            Content::Props(groups) => groups.clone(),
            Content::Loading => {
                y += self.s(8.0);
                self.text_line(g, &mut y, "Reading properties…", self.ui_font(12.5), t.text_dim);
                Vec::new()
            }
            _ => Vec::new(),
        };
        let bold = FontStyle::new(FontFamily::Ui, 12.5 * self.scale, true);
        for grp in &groups {
            y += self.s(8.0);
            self.text_line(g, &mut y, &grp.title, bold, t.text);
            for (k, v) in &grp.rows {
                self.field(g, &mut y, k, v, t);
            }
        }
        y
    }

    fn tag_row<B: TextBackend>(&mut self, g: &mut Gfx<B>, s: &Subject, y: &mut f32, t: &Theme) {
        self.preview.chips.clear();
        if (s.entry.is_none() && s.many.is_none()) || self.tab().view_loc.path().kind() != PathKind::Drive {
            return;
        }
        let b = self.preview.r.body;
        let (font, label) = (self.ui_font(12.0), self.ui_font(12.5));
        let (lm, fm) = (g.metrics(label), g.metrics(font));
        let (h, pad, dot, gap) = (self.s(22.0), self.s(8.0), self.s(8.0), self.s(6.0));
        let lw = (b.w * 0.38).round().min(self.s(140.0));
        let top = b.y + *y - self.preview.scroll;
        g.text(b.x, (top + (self.s(ROW) + lm.ascent - lm.descent) / 2.0).round(), "Tags", label, t.text_dim, lw - self.s(8.0));
        let (any, all) = self.selection_tags();
        let chips: Vec<(PaneHit, String, Option<Color>)> = self.tag_defs().iter().enumerate().map(|(k, d)| (PaneHit::Tag(k), d.name.clone(), Some(Color::rgb(d.color)))).chain([(PaneHit::NewTag, "+ New tag".to_owned(), None)]).collect();
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
        match &self.preview.content {
            Content::Doc(_) => return self.draw_doc(g, t),
            Content::Image => {
                if let Some((w, h)) = dims {
                    let k = (b.w / w).min(b.h / h).min(1.0);
                    let (dw, dh) = ((w * k).round(), (h * k).round());
                    g.dl.preview(Rect::new((b.x + (b.w - dw) / 2.0).round(), b.y, dw, dh));
                    return dh;
                }
            }
            Content::Handler => {
                self.text_line(g, &mut y, "Loading preview…", small, t.text_dim);
                return y;
            }
            &Content::Media { video } => return self.draw_media(g, t, &s, video),
            Content::Folder(f) => {
                let f = f.clone();
                self.header(g, t, &s, &mut y);
                self.folder_lines(g, t, &f, &mut y);
                return y;
            }
            Content::Loading if s.kind != Kind::Folder && s.entry.is_some() => {
                self.text_line(g, &mut y, "Loading…", small, t.text_dim);
                return y;
            }
            _ => {}
        }
        let note = match &self.preview.content {
            Content::Basic(n) => n.clone(),
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
        if let Some((_, bytes)) = s.many {
            rows.push(("Total size", self.size_text(bytes)));
        }
        if let Some((total, free)) = s.drive {
            let bar = Rect::new(b.x, b.y + y - self.preview.scroll, b.w, self.s(8.0));
            g.dl.round_rect(bar, t.input_bg, self.s(4.0));
            let used = if total > 0 { (total - free.min(total)) as f32 / total as f32 } else { 0.0 };
            g.dl.round_rect(Rect { w: (bar.w * used).round(), ..bar }, if used > 0.9 { t.error } else { t.accent }, self.s(4.0));
            y += self.s(16.0);
            rows.push(("Free", format!("{} of {}", self.size_text(free), self.size_text(total))));
        }
        if let Some(e) = s.entry.filter(|e| !e.is_dir()) {
            rows.push(("Size", self.size_text(e.size)));
            let mut d = String::new();
            format::datetime_in(&mut d, e.modified, self.utc_offset_min, &self.style);
            rows.push(("Modified", d));
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

    fn folder_lines<B: TextBackend>(&self, g: &mut Gfx<B>, t: &Theme, f: &FolderSummary, y: &mut f32) {
        let mut lines = vec![(format!("{}, {}", plural(f.dirs, "folder", "folders"), plural(f.files, "file", "files")), t.text)];
        let mut size = self.size_text(f.bytes);
        if !f.complete {
            let _ = write!(size, " in the first {} items", super::thousands(preview::FOLDER_SCAN as u64));
        }
        lines.push((size, t.text));
        if f.hidden > 0 {
            lines.push((format!("{} hidden", plural(f.hidden, "item", "items")), t.text_dim));
        }
        if f.newest > 0 {
            let mut d = "Last change ".to_string();
            format::datetime_in(&mut d, f.newest, self.utc_offset_min, &self.style);
            lines.push((d, t.text_dim));
        }
        if !f.types.is_empty() {
            let types: Vec<String> = f.types.iter().map(|(e, n)| format!("{} {}", if e.is_empty() { "no extension" } else { e }, super::thousands(*n))).collect();
            lines.push((format!("Most common: {}", types.join(", ")), t.text_dim));
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
        let (sc, dark) = (self.scale, self.dark);
        let b = self.preview.r.body;
        let note_font = self.ui_font(12.0);
        let p = &mut self.preview;
        let Content::Doc(d) = &p.content else { return 0.0 };
        if p.doc.as_ref().is_none_or(|l| l.width != b.w || l.scale != sc) {
            p.doc = Some(layout_doc(g, d, b.w, sc, &mut p.widths));
        }
        let lay = p.doc.as_ref().unwrap();
        let scroll = p.scroll;
        let first = lay.lines.partition_point(|l| l.y + l.h < scroll);
        for line in &lay.lines[first..] {
            let top = b.y + line.y - scroll;
            if top > b.bottom() {
                break;
            }
            match line.block {
                Block::Code if !d.mono => g.dl.rect(Rect::new(b.x, top, b.w, line.h), t.input_bg),
                Block::Quote => g.dl.rect(Rect::new(b.x + (2.0 * sc).round(), top, (3.0 * sc).round(), line.h), t.separator),
                Block::Rule => g.dl.rect(Rect::new(b.x, top + (line.h / 2.0).round(), b.w, 1.0), t.separator),
                _ => {}
            }
            for piece in &lay.pieces[line.pieces.start as usize..line.pieces.end as usize] {
                let run = &d.runs[piece.run as usize];
                let font = run_font(run, line.block, sc);
                let s = &d.text[piece.start as usize..piece.end as usize];
                g.text(b.x + piece.x, top + line.base, s, font, tone_color(t, dark, run.tone), f32::INFINITY);
            }
        }
        let mut h = lay.height;
        if d.truncated {
            let m = g.metrics(note_font);
            let top = b.y + h - scroll + (8.0 * sc).round();
            g.text(b.x, top + m.ascent, &format!("Showing the first {} characters", super::thousands(preview::TEXT_CHARS as u64)), note_font, t.text_dim, b.w);
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
