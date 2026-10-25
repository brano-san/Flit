use crate::atlas::{Atlas, Slot};
use crate::hash::{self, FxHashMap};
use crate::render::{Color, DrawList, Rect};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum FontFamily {
    Ui,
    Icons,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct FontStyle {
    pub family: FontFamily,
    pub size_q: u16,
    pub bold: bool,
}

impl FontStyle {
    pub fn new(family: FontFamily, px: f32, bold: bool) -> Self {
        Self { family, size_q: (px * 4.0).round() as u16, bold }
    }
    pub fn px(&self) -> f32 {
        self.size_q as f32 / 4.0
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ShapedGlyph {
    pub font: u16,
    pub id: u16,
    pub x: f32,
    pub y: f32,
    pub cluster: u32,
}

#[derive(Clone, Debug, Default)]
pub struct ShapedText {
    pub glyphs: Vec<ShapedGlyph>,
    pub width: f32,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct GlyphKey {
    pub font: u16,
    pub id: u16,
    pub size_q: u16,
    pub subpx: u8,
}

#[derive(Clone, Debug, Default)]
pub struct GlyphBitmap {
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct FontMetrics {
    pub ascent: f32,
    pub descent: f32,
}

pub trait TextBackend {
    fn shape(&mut self, text: &str, style: FontStyle, out: &mut ShapedText);
    fn rasterize(&mut self, key: GlyphKey, out: &mut GlyphBitmap) -> bool;
    fn metrics(&mut self, style: FontStyle) -> FontMetrics;
}

#[derive(Default)]
pub struct Uploads {
    pub rects: Vec<(Slot, usize)>,
    pub data: Vec<u8>,
}

impl Uploads {
    pub fn clear(&mut self) {
        self.rects.clear();
        self.data.clear();
    }
}

#[derive(Clone, Copy)]
struct GlyphSlot {
    slot: Slot,
    left: i32,
    top: i32,
}

struct Image {
    w: u32,
    h: u32,
    rgba: Vec<u8>,
    slot: Option<Slot>,
}

type Shapes = FxHashMap<u64, (Box<str>, ShapedText)>;

const ELLIPSIS: &str = "…";
const MAX_SHAPES: usize = 50_000;

pub struct Gfx<B: TextBackend> {
    pub text: B,
    pub dl: DrawList,
    pub uploads: Uploads,
    atlas: Atlas,
    shapes: Shapes,
    glyphs: FxHashMap<GlyphKey, Option<GlyphSlot>>,
    images: FxHashMap<u64, Image>,
    metrics: FxHashMap<FontStyle, FontMetrics>,
    scratch: GlyphBitmap,
    pub atlas_reset: bool,
}

impl<B: TextBackend> Gfx<B> {
    pub fn new(text: B, atlas_size: u32) -> Self {
        Self {
            text,
            dl: DrawList::default(),
            uploads: Uploads::default(),
            atlas: Atlas::new(atlas_size),
            shapes: Default::default(),
            glyphs: Default::default(),
            images: Default::default(),
            metrics: Default::default(),
            scratch: GlyphBitmap::default(),
            atlas_reset: false,
        }
    }

    pub fn begin(&mut self, clear: Color) {
        self.dl.reset(clear);
        self.atlas_reset = false;
        if self.shapes.len() > MAX_SHAPES {
            self.shapes.clear();
        }
    }

    pub fn reset_atlas(&mut self) {
        self.split().1.reset();
    }

    fn split(&mut self) -> (&mut Shapes, GlyphCtx<'_, B>) {
        let Self { shapes, text, glyphs, images, atlas, uploads, dl, scratch, atlas_reset, .. } = self;
        (shapes, GlyphCtx { text, glyphs, images, atlas, uploads, dl, scratch, atlas_reset })
    }

    pub fn metrics(&mut self, style: FontStyle) -> FontMetrics {
        let text = &mut self.text;
        *self.metrics.entry(style).or_insert_with(|| text.metrics(style))
    }

    pub fn shape(&mut self, s: &str, style: FontStyle) -> &ShapedText {
        shape(&mut self.shapes, &mut self.text, s, style)
    }

    pub fn measure(&mut self, s: &str, style: FontStyle) -> f32 {
        self.shape(s, style).width
    }

    pub fn caret_x(&mut self, s: &str, style: FontStyle, idx: usize) -> f32 {
        let t = self.shape(s, style);
        t.glyphs.iter().find(|g| g.cluster as usize >= idx).map_or(t.width, |g| g.x)
    }

    pub fn hit(&mut self, s: &str, style: FontStyle, x: f32) -> usize {
        let t = self.shape(s, style);
        let mut best = (s.len(), (t.width - x).abs());
        for g in &t.glyphs {
            let d = (g.x - x).abs();
            if d < best.1 {
                best = (g.cluster as usize, d);
            }
        }
        best.0
    }

    pub fn text(&mut self, x: f32, y: f32, s: &str, style: FontStyle, color: Color, max_w: f32) -> f32 {
        let (shapes, mut ctx) = self.split();
        let width = shape(shapes, ctx.text, s, style).width;
        if width <= max_w {
            let t = &shapes[&key(s, style)].1;
            for g in &t.glyphs {
                ctx.draw(x + g.x, y + g.y, g, style, color);
            }
            return width;
        }
        let ell_w = shape(shapes, ctx.text, ELLIPSIS, style).width;
        let avail = max_w - ell_w;
        let t = &shapes[&key(s, style)].1;
        let mut end = 0.0;
        for (i, g) in t.glyphs.iter().enumerate() {
            let next = t.glyphs.get(i + 1).map_or(t.width, |n| n.x);
            if next > avail {
                break;
            }
            ctx.draw(x + g.x, y + g.y, g, style, color);
            end = next;
        }
        if avail > 0.0 {
            for g in &shapes[&key(ELLIPSIS, style)].1.glyphs {
                ctx.draw(x + end + g.x, y + g.y, g, style, color);
            }
        }
        end + ell_w
    }

    pub fn has_image(&self, key: u64) -> bool {
        self.images.contains_key(&key)
    }

    pub fn add_image(&mut self, key: u64, w: u32, h: u32, rgba: Vec<u8>) {
        debug_assert_eq!(rgba.len(), (w * h * 4) as usize);
        self.images.insert(key, Image { w, h, rgba, slot: None });
    }

    pub fn image(&mut self, key: u64, r: Rect) -> bool {
        let Some(img) = self.images.get_mut(&key) else { return false };
        let slot = match img.slot {
            Some(s) => s,
            None => {
                let s = match self.atlas.alloc(img.w, img.h) {
                    Some(s) => s,
                    None => {
                        self.reset_atlas();
                        return false;
                    }
                };
                self.uploads.rects.push((s, self.uploads.data.len()));
                self.uploads.data.extend_from_slice(&img.rgba);
                img.slot = Some(s);
                s
            }
        };
        let uv = uv(slot, self.atlas.size());
        self.dl.image(r, uv, 255);
        true
    }
}

fn key(s: &str, style: FontStyle) -> u64 {
    hash::hash(&(s, style))
}

fn shape<'a, B: TextBackend>(
    shapes: &'a mut Shapes,
    text: &mut B,
    s: &str,
    style: FontStyle,
) -> &'a ShapedText {
    let k = key(s, style);
    let e = shapes.entry(k).or_insert_with(|| (Box::from(""), ShapedText { glyphs: Vec::new(), width: -1.0 }));
    if e.1.width < 0.0 || &*e.0 != s {
        e.0 = s.into();
        e.1.glyphs.clear();
        text.shape(s, style, &mut e.1);
    }
    &e.1
}

fn uv(s: Slot, size: u32) -> [f32; 4] {
    let n = size as f32;
    [s.x as f32 / n, s.y as f32 / n, (s.x + s.w) as f32 / n, (s.y + s.h) as f32 / n]
}

struct GlyphCtx<'a, B> {
    text: &'a mut B,
    glyphs: &'a mut FxHashMap<GlyphKey, Option<GlyphSlot>>,
    images: &'a mut FxHashMap<u64, Image>,
    atlas: &'a mut Atlas,
    uploads: &'a mut Uploads,
    dl: &'a mut DrawList,
    scratch: &'a mut GlyphBitmap,
    atlas_reset: &'a mut bool,
}

impl<B: TextBackend> GlyphCtx<'_, B> {
    fn draw(&mut self, px: f32, py: f32, g: &ShapedGlyph, style: FontStyle, color: Color) {
        let ix = px.floor();
        let subpx = ((px - ix) * 4.0) as u8 & 3;
        let key = GlyphKey { font: g.font, id: g.id, size_q: style.size_q, subpx };
        let gs = match self.glyphs.get(&key) {
            Some(s) => *s,
            None => {
                let s = self.rasterize(key);
                self.glyphs.insert(key, s);
                s
            }
        };
        if let Some(gs) = gs {
            let r = Rect::new(ix + gs.left as f32, py.round() - gs.top as f32, gs.slot.w as f32, gs.slot.h as f32);
            self.dl.glyph(r, uv(gs.slot, self.atlas.size()), color);
        }
    }

    fn reset(&mut self) {
        self.atlas.clear();
        self.glyphs.clear();
        for img in self.images.values_mut() {
            img.slot = None;
        }
        self.uploads.clear();
        *self.atlas_reset = true;
    }

    fn rasterize(&mut self, key: GlyphKey) -> Option<GlyphSlot> {
        self.scratch.data.clear();
        if !self.text.rasterize(key, self.scratch) || self.scratch.width == 0 || self.scratch.height == 0 {
            return None;
        }
        let (w, h) = (self.scratch.width, self.scratch.height);
        let slot = match self.atlas.alloc(w, h) {
            Some(s) => s,
            None => {
                self.reset();
                self.atlas.alloc(w, h)?
            }
        };
        let bm = &*self.scratch;
        self.uploads.rects.push((slot, self.uploads.data.len()));
        self.uploads.data.extend(bm.data.iter().flat_map(|&a| [a, a, a, a]));
        Some(GlyphSlot { slot, left: bm.left, top: bm.top })
    }
}

#[cfg(test)]
#[path = "../test/text.rs"]
pub(crate) mod tests;
