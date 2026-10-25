#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }
    pub fn inset(&self, dx: f32, dy: f32) -> Self {
        Self::new(self.x + dx, self.y + dy, (self.w - 2.0 * dx).max(0.0), (self.h - 2.0 * dy).max(0.0))
    }
    pub fn snap(&self) -> Self {
        let (x, y) = (self.x.round(), self.y.round());
        Self::new(x, y, (self.right().round() - x).max(0.0), (self.bottom().round() - y).max(0.0))
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Color(pub u32);

impl Color {
    pub const fn rgb(hex: u32) -> Self {
        Self::rgba((hex << 8) | 0xFF)
    }
    pub const fn rgba(hex: u32) -> Self {
        let (r, g, b, a) = (hex >> 24, (hex >> 16) & 0xFF, (hex >> 8) & 0xFF, hex & 0xFF);
        Self(r | g << 8 | b << 16 | a << 24)
    }
    pub const fn alpha(self, a: u8) -> Self {
        Self((self.0 & 0x00FF_FFFF) | (a as u32) << 24)
    }
    pub fn to_f32(self) -> [f32; 4] {
        let c = self.0;
        [c & 0xFF, (c >> 8) & 0xFF, (c >> 16) & 0xFF, c >> 24].map(|v| v as f32 / 255.0)
    }
}

pub mod kind {
    pub const SOLID: u32 = 0;
    pub const GLYPH: u32 = 1;
    pub const IMAGE: u32 = 2;
    pub const THUMB: u32 = 3;
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Instance {
    pub rect: [f32; 4],
    pub uv: [f32; 4],
    pub clip: [f32; 4],
    pub color: u32,
    pub radius: f32,
    pub border: f32,
    pub kind: u32,
}

const NO_CLIP: [f32; 4] = [-1e9, -1e9, 1e9, 1e9];

pub struct DrawList {
    pub clear: Color,
    pub instances: Vec<Instance>,
    clip: [f32; 4],
}

impl Default for DrawList {
    fn default() -> Self {
        Self { clear: Color::default(), instances: Vec::with_capacity(4096), clip: NO_CLIP }
    }
}

impl DrawList {
    pub fn reset(&mut self, clear: Color) {
        self.clear = clear;
        self.instances.clear();
        self.clip = NO_CLIP;
    }

    pub fn set_clip(&mut self, r: Option<Rect>) {
        self.clip = r.map_or(NO_CLIP, |r| [r.x, r.y, r.right(), r.bottom()]);
    }

    fn push(&mut self, r: Rect, uv: [f32; 4], color: Color, radius: f32, border: f32, kind: u32) {
        self.instances.push(Instance {
            rect: [r.x, r.y, r.w, r.h],
            uv,
            clip: self.clip,
            color: color.0,
            radius,
            border,
            kind,
        });
    }

    pub fn rect(&mut self, r: Rect, color: Color) {
        self.push(r.snap(), [0.0; 4], color, 0.0, 0.0, kind::SOLID);
    }

    pub fn round_rect(&mut self, r: Rect, color: Color, radius: f32) {
        self.push(r.snap(), [0.0; 4], color, radius, 0.0, kind::SOLID);
    }

    pub fn outline(&mut self, r: Rect, color: Color, radius: f32, width: f32) {
        self.push(r.snap(), [0.0; 4], color, radius, width, kind::SOLID);
    }

    pub fn glyph(&mut self, r: Rect, uv: [f32; 4], color: Color) {
        self.push(r, uv, color, 0.0, 0.0, kind::GLYPH);
    }

    pub fn image(&mut self, r: Rect, uv: [f32; 4], alpha: u8) {
        self.push(r, uv, Color(0xFFFF_FFFF).alpha(alpha), 0.0, 0.0, kind::IMAGE);
    }

    pub fn thumb(&mut self, r: Rect, uv: [f32; 4], alpha: u8) {
        self.push(r, uv, Color(0xFFFF_FFFF).alpha(alpha), 0.0, 0.0, kind::THUMB);
    }
}

#[cfg(test)]
#[path = "../test/render.rs"]
mod tests;
