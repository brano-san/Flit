#![allow(dead_code)]

use crate::fonts::{self, FontFile, Pair};
use flit_core::hash::FxHashMap;
use flit_core::text::{FontFamily, FontMetrics, FontStyle, GlyphBitmap, GlyphKey, ShapedGlyph, ShapedText, TextBackend};
use harfrust::{Buffer, Direction, Script, ShapeOptions, ShapePlan, ShapePlanKey, ShaperFont, Tag};
use swash::scale::image::Image;
use swash::scale::{Render, ScaleContext, Source};
use swash::zeno::{Format, Vector};
use unicode_bidi::{bidi_class, BidiClass, BidiInfo, Level};

const WGHT: [u8; 4] = *b"wght";
const SEMIBOLD: f32 = 600.0;
const EMBOLDEN_PER_PX: f32 = 1.0 / 32.0;
const NONE: u16 = u16::MAX;

struct Face {
    sw: swash::FontRef<'static>,
    charmap: swash::Charmap<'static>,
    hr: harfrust::Font,
    upem: f32,
    ascent: f32,
    descent: f32,
    vars: Vec<(u32, f32)>,
    embolden: bool,
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
        Some(Self { sw, charmap, hr, upem: m.units_per_em.max(1) as f32, ascent: m.ascent, descent: m.descent, vars, embolden, ascii, plans: Vec::new() })
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
        let mut scaler = self.scale.builder(face.sw).size(px).hint(false).variations(face.vars.iter().copied()).build();
        let mut render = Render::new(&[Source::Outline]);
        render.format(Format::Alpha).offset(Vector::new(key.subpx as f32 / 4.0, 0.0));
        if face.embolden {
            render.embolden(px * EMBOLDEN_PER_PX);
        }
        if !render.render_into(&mut scaler, key.id, &mut self.image) {
            return false;
        }
        let p = self.image.placement;
        out.left = p.left;
        out.top = p.top;
        out.width = p.width;
        out.height = p.height;
        out.data.clear();
        out.data.extend_from_slice(&self.image.data[..(p.width * p.height) as usize]);
        true
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
