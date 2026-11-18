#![allow(dead_code)]

use crate::fonts::{self, FontFile, Hinting, Pair};
use flit_core::hash::FxHashMap;
use flit_core::text::{FontFamily, FontMetrics, FontStyle, GlyphBitmap, GlyphKey, GlyphMode, ShapedGlyph, ShapedText, Subpixel, TextBackend};
use harfrust::{Buffer, Direction, Script, ShapeOptions, ShapePlan, ShapePlanKey, ShaperFont, Tag};
use skrifa::instance::{Location, Size};
use skrifa::outline::{DrawSettings, Engine, GlyphStyles, HintingInstance, HintingOptions, OutlineGlyphCollection, OutlinePen, SmoothMode, Target};
use skrifa::MetadataProvider;
use swash::scale::image::{Content, Image};
use swash::scale::outline::Outline;
use swash::scale::{Render, ScaleContext, Source, StrikeWith};
use swash::zeno::{Command, Format, Mask, Origin, PathData, Placement, Vector};
use unicode_bidi::{bidi_class, BidiClass, BidiInfo, Level};

const WGHT: [u8; 4] = *b"wght";
const SEMIBOLD: f32 = 600.0;
const EMBOLDEN_PER_PX: f32 = 1.0 / 32.0;
const NONE: u16 = u16::MAX;
const COLOR_TABLES: [&[u8; 4]; 4] = [b"COLR", b"CBDT", b"sbix", b"SVG "];
const COLOR_SOURCES: [Source; 3] = [Source::ColorOutline(0), Source::ColorBitmap(StrikeWith::BestFit), Source::Outline];
const THIRD: f32 = 1.0 / 3.0;
const MAX_HINTERS: usize = 8;
const LIGHT: Target = Target::Smooth { mode: SmoothMode::Light, symmetric_rendering: true, preserve_linear_metrics: true };

fn premultiplied(p: &[u8]) -> [u8; 4] {
    let a = u32::from(p[3]);
    let m = |c: u8| ((u32::from(c) * a + 127) / 255) as u8;
    [m(p[0]), m(p[1]), m(p[2]), p[3]]
}

struct Light {
    outlines: OutlineGlyphCollection<'static>,
    location: Location,
    engine: Engine,
}

impl Light {
    fn open(data: &'static [u8], index: u32, vars: &[(u32, f32)]) -> Option<Self> {
        let font = skrifa::FontRef::from_index(data, index).ok()?;
        let outlines = font.outline_glyphs();
        let location = font.axes().location(vars.iter().map(|&(t, v)| (skrifa::Tag::from_u32(t), v)));
        let engine = if outlines.prefer_interpreter() { Engine::Interpreter } else { Engine::Auto(Some(GlyphStyles::new(&outlines))) };
        Some(Self { outlines, location, engine })
    }
}

struct PathPen<'a>(&'a mut Vec<Command>);

impl OutlinePen for PathPen<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        self.0.push(Command::MoveTo(Vector::new(x, y)));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.0.push(Command::LineTo(Vector::new(x, y)));
    }
    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        self.0.push(Command::QuadTo(Vector::new(cx0, cy0), Vector::new(x, y)));
    }
    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        self.0.push(Command::CurveTo(Vector::new(cx0, cy0), Vector::new(cx1, cy1), Vector::new(x, y)));
    }
    fn close(&mut self) {
        self.0.push(Command::Close);
    }
}

pub fn glyph_mode_for(r: fonts::Rendering, lcd_blend: bool) -> GlyphMode {
    GlyphMode { subpixel: if lcd_blend { r.subpixel } else { Subpixel::None }, hinting: r.hinting as u8 }
}

pub fn channel_offsets(subpixel: Subpixel) -> [Vector; 3] {
    let x = |r: f32| [Vector::new(r, 0.0), Vector::ZERO, Vector::new(-r, 0.0)];
    let y = |r: f32| [Vector::new(0.0, r), Vector::ZERO, Vector::new(0.0, -r)];
    match subpixel {
        Subpixel::None => [Vector::ZERO; 3],
        Subpixel::Rgb => x(THIRD),
        Subpixel::Bgr => x(-THIRD),
        Subpixel::Vrgb => y(-THIRD),
        Subpixel::Vbgr => y(THIRD),
    }
}

fn alpha_mask(path: &[Command], offset: Vector) -> (Vec<u8>, Placement) {
    let mut mask = Mask::new(path);
    mask.origin(Origin::BottomLeft).offset(offset).render_offset(offset).inspect(|_, _, _| {});
    mask.render()
}

pub fn render_mask(path: &[Command], subpixel: Subpixel, offset: Vector, out: &mut GlyphBitmap) {
    out.data.clear();
    out.color = false;
    out.lcd = subpixel != Subpixel::None;
    if !out.lcd {
        let (data, p) = alpha_mask(path, offset);
        (out.left, out.top, out.width, out.height, out.data) = (p.left, p.top, p.width, p.height, data);
        return;
    }
    let masks = channel_offsets(subpixel).map(|ch| alpha_mask(path, offset + ch));
    let left = masks.iter().map(|m| m.1.left).min().unwrap_or(0);
    let top = masks.iter().map(|m| m.1.top).max().unwrap_or(0);
    let right = masks.iter().map(|m| m.1.left + m.1.width as i32).max().unwrap_or(0);
    let bottom = masks.iter().map(|m| m.1.top - m.1.height as i32).min().unwrap_or(0);
    let (w, h) = ((right - left).max(0) as u32, (top - bottom).max(0) as u32);
    (out.left, out.top, out.width, out.height) = (left, top, w, h);
    out.data.resize((w * h * 4) as usize, 0);
    for (c, (data, p)) in masks.iter().enumerate() {
        let (dx, dy) = ((p.left - left) as usize, (top - p.top) as usize);
        for row in 0..p.height as usize {
            let src = &data[row * p.width as usize..][..p.width as usize];
            let dst = ((dy + row) * w as usize + dx) * 4;
            for (k, &v) in src.iter().enumerate() {
                out.data[dst + k * 4 + c] = v;
                if c == 1 {
                    out.data[dst + k * 4 + 3] = v;
                }
            }
        }
    }
}

struct Face {
    data: &'static [u8],
    index: u32,
    light: Option<Option<Light>>,
    sw: swash::FontRef<'static>,
    charmap: swash::Charmap<'static>,
    hr: harfrust::Font,
    upem: f32,
    ascent: f32,
    descent: f32,
    vars: Vec<(u32, f32)>,
    embolden: bool,
    color: bool,
    ascii: u128,
    plans: Vec<ShapePlan>,
}

impl Face {
    fn open(data: &'static [u8], index: u32, bold: bool) -> Option<Self> {
        let (face_index, instance) = (index & 0xFFFF, index >> 16);
        let sw = swash::FontRef::from_index(data, face_index as usize)?;
        let base = harfrust::Font::new(data, face_index)?;
        let mut vars: Vec<(u32, f32)> = match instance.checked_sub(1).and_then(|i| base.named_instances().get(i)) {
            Some(named) => named.variations().map(|v| (u32::from_be_bytes(v.tag.to_be_bytes()), v.value)).collect(),
            None => Vec::new(),
        };
        let wght = u32::from_be_bytes(WGHT);
        let mut embolden = false;
        if bold {
            if base.axes().get_by_tag(Tag::new(&WGHT)).is_some() {
                vars.retain(|v| v.0 != wght);
                vars.push((wght, SEMIBOLD));
            } else {
                embolden = true;
            }
        }
        let hr = if vars.is_empty() { base } else { base.instance_builder().variations(vars.iter().map(|&(t, v)| (Tag::new(&t.to_be_bytes()), v))).build() };
        let m = sw.metrics(&[]);
        let charmap = sw.charmap();
        let ascii = (0x20u32..0x7F).filter(|&c| charmap.map(c) != 0).fold(0u128, |a, c| a | 1 << c);
        let color = COLOR_TABLES.iter().any(|t| swash::TableProvider::table_by_tag(&sw, swash::tag_from_bytes(t)).is_some());
        Some(Self { data, index: face_index, light: None, sw, charmap, hr, upem: m.units_per_em.max(1) as f32, ascent: m.ascent, descent: m.descent, vars, embolden, color, ascii, plans: Vec::new() })
    }

    fn covers(&self, c: char) -> bool {
        let u = c as u32;
        if u < 128 {
            self.ascii & 1 << u != 0
        } else {
            self.charmap.map(u) != 0
        }
    }

    fn plan(&mut self, direction: Direction, script: Option<Script>) -> usize {
        if let Some(i) = self.plans.iter().position(|p| ShapePlanKey::new(&self.hr, script, direction).matches(p)) {
            return i;
        }
        self.plans.push(ShapePlan::new(&self.hr, direction, script, None, &[]));
        self.plans.len() - 1
    }
}

struct Run {
    start: usize,
    end: usize,
    face: u16,
    level: Level,
    script: Option<Script>,
    x: f32,
    width: f32,
    first: usize,
}

#[derive(Clone, Copy)]
struct Placed {
    id: u16,
    cluster: u32,
    advance: f32,
    dx: f32,
    dy: f32,
}

pub struct LinuxText {
    faces: Vec<Face>,
    loaded: FxHashMap<(usize, u32, bool), u16>,
    default_ui: [u16; 2],
    ui: [u16; 2],
    mono: [u16; 2],
    icons: u16,
    fallback: Vec<FontFile>,
    fallback_faces: FxHashMap<(usize, bool), u16>,
    adopted: bool,
    chars: FxHashMap<(char, bool), u16>,
    scale: ScaleContext,
    image: Image,
    outline: Outline,
    path: Vec<Command>,
    hinters: Vec<(u16, u16, HintingInstance)>,
    hinting: Hinting,
    buffer: Option<Buffer>,
    runs: Vec<Run>,
    placed: Vec<Placed>,
}

impl Default for LinuxText {
    fn default() -> Self {
        Self::new()
    }
}

impl LinuxText {
    pub fn new() -> Self {
        let shared = fonts::fonts();
        let mut t = Self {
            faces: Vec::new(),
            loaded: FxHashMap::default(),
            default_ui: [0, 0],
            ui: [0, 0],
            mono: [0, 0],
            icons: 0,
            fallback: Vec::new(),
            fallback_faces: FxHashMap::default(),
            adopted: false,
            chars: FxHashMap::default(),
            scale: ScaleContext::new(),
            image: Image::new(),
            outline: Outline::new(),
            path: Vec::new(),
            hinters: Vec::new(),
            hinting: fonts::rendering().hinting,
            buffer: Some(Buffer::new()),
            runs: Vec::new(),
            placed: Vec::new(),
        };
        let sans = [t.load(fonts::SANS, 0, false), t.load(fonts::SANS, 0, true)];
        let icons = t.load(fonts::ICONS, 0, false);
        let (Some(regular), Some(bold), Some(icons)) = (sans[0], sans[1], icons) else { unreachable!("bundled fonts parse") };
        t.icons = icons;
        t.default_ui = [regular, bold];
        t.mono = [regular, bold];
        let source = shared.live().map(|l| &l.resolved).or(shared.cached.as_ref());
        if let Some(r) = source {
            if let Some(p) = r.ui.as_ref().and_then(|p| t.load_pair(p)) {
                t.default_ui = p;
            }
            if let Some(p) = r.mono.as_ref().and_then(|p| t.load_pair(p)) {
                t.mono = p;
            }
            t.fallback.clone_from(&r.fallback);
            t.adopted = true;
        }
        t.ui = t.default_ui;
        t
    }

    pub fn fonts_ready(&self) -> bool {
        self.adopted
    }

    fn load(&mut self, data: &'static [u8], index: u32, bold: bool) -> Option<u16> {
        let key = (data.as_ptr() as usize, index, bold);
        if let Some(&f) = self.loaded.get(&key) {
            return (f != NONE).then_some(f);
        }
        let face = Face::open(data, index, bold).filter(|_| self.faces.len() < NONE as usize);
        let id = match face {
            Some(face) => {
                self.faces.push(face);
                (self.faces.len() - 1) as u16
            }
            None => NONE,
        };
        self.loaded.insert(key, id);
        (id != NONE).then_some(id)
    }

    fn load_file(&mut self, f: &FontFile, bold: bool) -> Option<u16> {
        self.load(fonts::map_file(&f.path)?, f.index, bold)
    }

    fn load_pair(&mut self, p: &Pair) -> Option<[u16; 2]> {
        let regular = self.load_file(&p[0], false)?;
        let bold = if p[1] == p[0] { self.load_file(&p[0], true)? } else { self.load_file(&p[1], false)? };
        Some([regular, bold])
    }

    fn adopt(&mut self) {
        if self.adopted {
            return;
        }
        if let Some(live) = fonts::fonts().live() {
            self.fallback.clone_from(&live.resolved.fallback);
            self.fallback_faces.clear();
            self.chars.clear();
            self.adopted = true;
        }
    }

    fn primary(&self, style: FontStyle) -> u16 {
        let b = style.bold as usize;
        match style.family {
            FontFamily::Ui => self.ui[b],
            FontFamily::Mono => self.mono[b],
            FontFamily::Icons => self.icons,
        }
    }

    fn fallback_face(&mut self, i: usize, bold: bool) -> u16 {
        if let Some(&f) = self.fallback_faces.get(&(i, bold)) {
            return f;
        }
        let file = self.fallback[i].clone();
        let f = self.load_file(&file, bold).unwrap_or(NONE);
        self.fallback_faces.insert((i, bold), f);
        f
    }

    fn face_for(&mut self, primary: u16, c: char, bold: bool) -> u16 {
        if self.faces[primary as usize].covers(c) {
            return primary;
        }
        if let Some(&f) = self.chars.get(&(c, bold)) {
            return if f == NONE { primary } else { f };
        }
        let mut found = NONE;
        for i in 0..self.fallback.len() {
            let f = self.fallback_face(i, false);
            if f != NONE && self.faces[f as usize].covers(c) {
                found = if bold { self.fallback_face(i, true) } else { f };
                break;
            }
        }
        self.chars.insert((c, bold), found);
        if found == NONE {
            primary
        } else {
            found
        }
    }

    fn itemize(&mut self, text: &str, primary: u16, bold: bool) {
        self.runs.clear();
        let run = |start, end, face, level, script| Run { start, end, face, level, script, x: 0.0, width: 0.0, first: 0 };
        let face = &self.faces[primary as usize];
        if text.bytes().all(|b| b < 128 && face.ascii & 1 << b != 0) {
            self.runs.push(run(0, text.len(), primary, Level::ltr(), None));
            return;
        }
        let rtl = text.chars().any(|c| c >= '\u{590}' && matches!(bidi_class(c), BidiClass::R | BidiClass::AL | BidiClass::RLE | BidiClass::RLO | BidiClass::RLI));
        let levels = rtl.then(|| BidiInfo::new(text, Some(Level::ltr())).levels);
        let mut prev: Option<u16> = None;
        for (i, c) in text.char_indices() {
            let level = levels.as_ref().map_or(Level::ltr(), |l| l[i]);
            let joins = |p: u16, faces: &[Face]| {
                matches!(bidi_class(c), BidiClass::BN) || ('\u{FE00}'..='\u{FE0F}').contains(&c) || (matches!(bidi_class(c), BidiClass::NSM) && faces[p as usize].covers(c))
            };
            let face = match prev {
                Some(p) if joins(p, &self.faces) => p,
                _ => self.face_for(primary, c, bold),
            };
            let script = match harfrust::unicode::script(c as u32) {
                Script::COMMON | Script::INHERITED | Script::UNKNOWN => None,
                s => Some(s),
            };
            let end = i + c.len_utf8();
            match self.runs.last_mut() {
                Some(r) if r.face == face && r.level == level && (script.is_none() || r.script.is_none() || r.script == script) => {
                    r.end = end;
                    r.script = r.script.or(script);
                }
                _ => self.runs.push(run(i, end, face, level, script)),
            }
            prev = Some(face);
        }
    }

    fn shape_runs(&mut self, text: &str, px: f32) {
        self.placed.clear();
        let mut buffer = self.buffer.take().unwrap_or_default();
        for run in &mut self.runs {
            let face = &mut self.faces[run.face as usize];
            let direction = if run.level.is_rtl() { Direction::RightToLeft } else { Direction::LeftToRight };
            buffer.clear();
            buffer.push_str(&text[run.start..run.end]);
            buffer.set_direction(direction);
            match run.script {
                Some(s) => buffer.set_script(Some(s)),
                None => buffer.guess_segment_properties(),
            }
            let plan = face.plan(direction, buffer.script());
            let k = px / face.upem;
            run.first = self.placed.len();
            if harfrust::shape(&ShaperFont::new(&face.hr), &mut buffer, ShapeOptions::new().plan(Some(&face.plans[plan]))).is_err() {
                continue;
            }
            for (info, pos) in buffer.glyph_infos().iter().zip(buffer.glyph_positions()) {
                let advance = pos.x_advance as f32 * k;
                run.width += advance;
                self.placed.push(Placed {
                    id: u16::try_from(info.glyph_id).unwrap_or(0),
                    cluster: run.start as u32 + info.cluster,
                    advance,
                    dx: pos.x_offset as f32 * k,
                    dy: -pos.y_offset as f32 * k,
                });
            }
        }
        buffer.clear();
        self.buffer = Some(buffer);
    }

    pub fn set_hinting(&mut self, hinting: Hinting) {
        self.hinting = hinting;
    }

    fn light_path(&mut self, key: GlyphKey, px: f32) -> bool {
        let face = &mut self.faces[key.font as usize];
        let Some(light) = face.light.get_or_insert_with(|| Light::open(face.data, face.index, &face.vars)) else { return false };
        let size_q = key.size_q;
        let at = match self.hinters.iter().position(|h| h.0 == key.font && h.1 == size_q) {
            Some(i) => i,
            None => {
                let options = HintingOptions { engine: light.engine.clone(), target: LIGHT };
                let Ok(hinter) = HintingInstance::new(&light.outlines, Size::new(px), &light.location, options) else { return false };
                if self.hinters.len() >= MAX_HINTERS {
                    self.hinters.remove(0);
                }
                self.hinters.push((key.font, size_q, hinter));
                self.hinters.len() - 1
            }
        };
        let Some(glyph) = light.outlines.get(skrifa::GlyphId::new(u32::from(key.id))) else { return false };
        self.path.clear();
        glyph.draw(DrawSettings::hinted(&self.hinters[at].2, false), &mut PathPen(&mut self.path)).is_ok()
    }

    fn swash_path(&mut self, key: GlyphKey, px: f32) -> bool {
        let face = &self.faces[key.font as usize];
        let mut scaler = self.scale.builder(face.sw).size(px).hint(self.hinting == Hinting::Full).variations(face.vars.iter().copied()).build();
        self.outline.clear();
        if !scaler.scale_outline_into(key.id, &mut self.outline) {
            return false;
        }
        if face.embolden {
            self.outline.embolden(px * EMBOLDEN_PER_PX, px * EMBOLDEN_PER_PX);
        }
        self.path.clear();
        self.path.extend(self.outline.path().commands());
        true
    }

    fn place(&mut self, out: &mut ShapedText) {
        let mut x = 0.0;
        if self.runs.iter().any(|r| r.level.is_rtl()) {
            let levels: Vec<Level> = self.runs.iter().map(|r| r.level).collect();
            for i in BidiInfo::reorder_visual(&levels) {
                self.runs[i].x = x;
                x += self.runs[i].width;
            }
        } else {
            for r in &mut self.runs {
                r.x = x;
                x += r.width;
            }
        }
        out.width = x;
        for (k, r) in self.runs.iter().enumerate() {
            let end = self.runs.get(k + 1).map_or(self.placed.len(), |n| n.first);
            let glyphs = &self.placed[r.first..end];
            let start = out.glyphs.len();
            let mut pen = r.x;
            for g in glyphs {
                out.glyphs.push(ShapedGlyph { font: r.face, id: g.id, x: pen + g.dx, y: g.dy, cluster: g.cluster });
                pen += g.advance;
            }
            if r.level.is_rtl() {
                out.glyphs[start..].reverse();
            }
        }
    }
}

impl TextBackend for LinuxText {
    fn shape(&mut self, text: &str, style: FontStyle, out: &mut ShapedText) {
        out.glyphs.clear();
        out.width = 0.0;
        if text.is_empty() {
            return;
        }
        self.adopt();
        let primary = self.primary(style);
        self.itemize(text, primary, style.bold && style.family != FontFamily::Icons);
        self.shape_runs(text, style.px().max(1.0));
        self.place(out);
    }

    fn rasterize(&mut self, key: GlyphKey, out: &mut GlyphBitmap) -> bool {
        let Some(face) = self.faces.get(key.font as usize) else { return false };
        let px = key.size_q as f32 / 4.0;
        let light = self.hinting == Hinting::Slight && !face.embolden;
        if !face.color && (key.subpixel != Subpixel::None || light) {
            if !(light && self.light_path(key, px)) && !self.swash_path(key, px) {
                return false;
            }
            render_mask(&self.path, key.subpixel, Vector::new(key.subpx as f32 / 4.0, 0.0), out);
            return true;
        }
        let mut scaler = self.scale.builder(face.sw).size(px).hint(self.hinting == Hinting::Full).variations(face.vars.iter().copied()).build();
        let mut render = Render::new(if face.color { &COLOR_SOURCES } else { &COLOR_SOURCES[2..] });
        render.format(Format::Alpha).offset(Vector::new(key.subpx as f32 / 4.0, 0.0));
        if face.embolden {
            render.embolden(px * EMBOLDEN_PER_PX);
        }
        self.image.data.clear();
        if !render.render_into(&mut scaler, key.id, &mut self.image) {
            return false;
        }
        let p = self.image.placement;
        out.left = p.left;
        out.top = p.top;
        out.width = p.width;
        out.height = p.height;
        out.color = self.image.content == Content::Color;
        out.lcd = false;
        out.data.clear();
        let bytes = (p.width * p.height) as usize * if out.color { 4 } else { 1 };
        let Some(pixels) = self.image.data.get(..bytes) else { return false };
        if out.color {
            out.data.extend(pixels.chunks_exact(4).flat_map(premultiplied));
        } else {
            out.data.extend_from_slice(pixels);
        }
        true
    }

    fn glyph_mode(&mut self) -> GlyphMode {
        let r = fonts::rendering();
        self.hinting = r.hinting;
        glyph_mode_for(r, fonts::lcd_blend())
    }

    fn metrics(&mut self, style: FontStyle) -> FontMetrics {
        let face = &self.faces[self.primary(style) as usize];
        let k = style.px() / face.upem;
        FontMetrics { ascent: face.ascent * k, descent: face.descent * k }
    }

    fn set_ui_family(&mut self, name: &str) -> bool {
        self.ui = self.default_ui;
        if name.is_empty() {
            return true;
        }
        let shared = fonts::fonts();
        if let Some(p) = shared.remembered(name).and_then(|p| self.load_pair(&p)) {
            self.ui = p;
            return true;
        }
        let Some(pair) = shared.wait().fc.as_ref().and_then(|fc| fc.family(name)) else { return false };
        let Some(p) = self.load_pair(&pair) else { return false };
        shared.remember(name, &pair);
        self.ui = p;
        true
    }

    fn families(&mut self) -> Vec<String> {
        fonts::fonts().wait().fc.as_ref().map(|fc| fc.families()).unwrap_or_default()
    }
}

#[cfg(test)]
#[path = "../test/text.rs"]
mod tests;
