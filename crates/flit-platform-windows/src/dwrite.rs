use crate::err;
use flit_core::hash::FxHashMap;
use flit_core::text::{FontFamily, FontMetrics, FontStyle, GlyphBitmap, GlyphKey, ShapedGlyph, ShapedText, TextBackend};
use flit_platform::PlatformError;
use std::cell::RefCell;
use std::ffi::c_void;
use std::mem::ManuallyDrop;
use std::slice::from_raw_parts;
use windows::core::{w, Interface, PCWSTR};
use windows::Win32::Graphics::DirectWrite::*;
use windows_core::{implement, ComObject, IUnknown, Ref, BOOL};

struct Run {
    face: IDWriteFontFace,
    x: f32,
    y: f32,
    rtl: bool,
    ids: Vec<u16>,
    advances: Vec<f32>,
    offsets: Vec<(f32, f32)>,
    text_pos: u32,
    cluster_map: Vec<u16>,
}

#[implement(IDWriteTextRenderer)]
struct Collector {
    runs: RefCell<Vec<Run>>,
}

impl IDWritePixelSnapping_Impl for Collector_Impl {
    fn IsPixelSnappingDisabled(&self, _: *const c_void) -> windows_core::Result<BOOL> {
        Ok(true.into())
    }
    fn GetCurrentTransform(&self, _: *const c_void, transform: *mut DWRITE_MATRIX) -> windows_core::Result<()> {
        unsafe { *transform = DWRITE_MATRIX { m11: 1.0, m12: 0.0, m21: 0.0, m22: 1.0, dx: 0.0, dy: 0.0 } };
        Ok(())
    }
    fn GetPixelsPerDip(&self, _: *const c_void) -> windows_core::Result<f32> {
        Ok(1.0)
    }
}

impl IDWriteTextRenderer_Impl for Collector_Impl {
    fn DrawGlyphRun(
        &self,
        _: *const c_void,
        x: f32,
        y: f32,
        _: DWRITE_MEASURING_MODE,
        run: *const DWRITE_GLYPH_RUN,
        desc: *const DWRITE_GLYPH_RUN_DESCRIPTION,
        _: Ref<IUnknown>,
    ) -> windows_core::Result<()> {
        unsafe {
            let run = &*run;
            let desc = &*desc;
            let Some(face) = (*run.fontFace).clone() else { return Ok(()) };
            let n = run.glyphCount as usize;
            let ids = if n == 0 { Vec::new() } else { from_raw_parts(run.glyphIndices, n).to_vec() };
            let advances = if run.glyphAdvances.is_null() { vec![0.0; n] } else { from_raw_parts(run.glyphAdvances, n).to_vec() };
            let offsets = if run.glyphOffsets.is_null() {
                vec![(0.0, 0.0); n]
            } else {
                from_raw_parts(run.glyphOffsets, n).iter().map(|o| (o.advanceOffset, o.ascenderOffset)).collect()
            };
            let cluster_map = if desc.clusterMap.is_null() { Vec::new() } else { from_raw_parts(desc.clusterMap, desc.stringLength as usize).to_vec() };
            self.runs.borrow_mut().push(Run { face, x, y, rtl: run.bidiLevel & 1 == 1, ids, advances, offsets, text_pos: desc.textPosition, cluster_map });
        }
        Ok(())
    }
    fn DrawUnderline(&self, _: *const c_void, _: f32, _: f32, _: *const DWRITE_UNDERLINE, _: Ref<IUnknown>) -> windows_core::Result<()> {
        Ok(())
    }
    fn DrawStrikethrough(&self, _: *const c_void, _: f32, _: f32, _: *const DWRITE_STRIKETHROUGH, _: Ref<IUnknown>) -> windows_core::Result<()> {
        Ok(())
    }
    fn DrawInlineObject(&self, _: *const c_void, _: f32, _: f32, _: Ref<IDWriteInlineObject>, _: BOOL, _: BOOL, _: Ref<IUnknown>) -> windows_core::Result<()> {
        Ok(())
    }
}

pub struct DWriteText {
    factory: IDWriteFactory2,
    collection: IDWriteFontCollection,
    default_ui: PCWSTR,
    ui_family: Vec<u16>,
    icon_family: PCWSTR,
    mono_family: PCWSTR,
    formats: FxHashMap<FontStyle, IDWriteTextFormat>,
    primary: FxHashMap<(FontFamily, bool), Option<DWRITE_FONT_METRICS>>,
    faces: Vec<IDWriteFontFace>,
    collector: ComObject<Collector>,
    renderer: IDWriteTextRenderer,
    wide: Vec<u16>,
    byte_of: Vec<u32>,
}

impl DWriteText {
    pub fn new() -> Result<Self, PlatformError> {
        unsafe {
            let factory: IDWriteFactory2 = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).map_err(err)?;
            let mut collection = None;
            factory.GetSystemFontCollection(&mut collection, false).map_err(err)?;
            let collection = collection.ok_or_else(|| PlatformError::new(-1, "no system font collection"))?;
            let has = |name: PCWSTR| {
                let (mut i, mut exists) = (0u32, BOOL(0));
                collection.FindFamilyName(name, &mut i, &mut exists).is_ok() && exists.as_bool()
            };
            let default_ui = if has(w!("Segoe UI Variable Text")) { w!("Segoe UI Variable Text") } else { w!("Segoe UI") };
            let icon_family = if has(w!("Segoe Fluent Icons")) { w!("Segoe Fluent Icons") } else { w!("Segoe MDL2 Assets") };
            let mono_family = if has(w!("Cascadia Mono")) { w!("Cascadia Mono") } else { w!("Consolas") };
            let collector = ComObject::new(Collector { runs: RefCell::default() });
            let renderer: IDWriteTextRenderer = collector.to_interface();
            Ok(Self {
                factory,
                collection,
                default_ui,
                ui_family: default_ui.as_wide().iter().copied().chain([0]).collect(),
                icon_family,
                mono_family,
                formats: FxHashMap::default(),
                primary: FxHashMap::default(),
                faces: Vec::new(),
                collector,
                renderer,
                wide: Vec::new(),
                byte_of: Vec::new(),
            })
        }
    }

    fn family(&self, f: FontFamily) -> PCWSTR {
        match f {
            FontFamily::Ui => PCWSTR(self.ui_family.as_ptr()),
            FontFamily::Icons => self.icon_family,
            FontFamily::Mono => self.mono_family,
        }
    }

    fn has_family(&self, name: PCWSTR) -> bool {
        let (mut i, mut exists) = (0u32, BOOL(0));
        unsafe { self.collection.FindFamilyName(name, &mut i, &mut exists).is_ok() && exists.as_bool() }
    }

    fn family_name(family: &IDWriteFontFamily) -> Option<String> {
        unsafe {
            let names = family.GetFamilyNames().ok()?;
            let (mut i, mut exists) = (0u32, BOOL(0));
            let _ = names.FindLocaleName(w!("en-us"), &mut i, &mut exists);
            let i = if exists.as_bool() { i } else { 0 };
            let len = names.GetStringLength(i).ok()? as usize;
            let mut buf = vec![0u16; len + 1];
            names.GetString(i, &mut buf).ok()?;
            Some(String::from_utf16_lossy(&buf[..len]))
        }
    }

    fn weight(bold: bool) -> DWRITE_FONT_WEIGHT {
        if bold {
            DWRITE_FONT_WEIGHT_SEMI_BOLD
        } else {
            DWRITE_FONT_WEIGHT_NORMAL
        }
    }

    fn format(&mut self, style: FontStyle) -> Option<IDWriteTextFormat> {
        if let Some(f) = self.formats.get(&style) {
            return Some(f.clone());
        }
        let f = unsafe {
            let f = self
                .factory
                .CreateTextFormat(self.family(style.family), &self.collection, Self::weight(style.bold), DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, style.px().max(1.0), w!("en-us"))
                .ok()?;
            let _ = f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
            f
        };
        self.formats.insert(style, f.clone());
        Some(f)
    }

    fn face_index(&mut self, face: &IDWriteFontFace) -> u16 {
        match self.faces.iter().position(|f| f.as_raw() == face.as_raw()) {
            Some(i) => i as u16,
            None => {
                self.faces.push(face.clone());
                (self.faces.len() - 1) as u16
            }
        }
    }

    fn design_metrics(&mut self, family: FontFamily, bold: bool) -> Option<DWRITE_FONT_METRICS> {
        if let Some(m) = self.primary.get(&(family, bold)) {
            return *m;
        }
        let m = unsafe {
            (|| {
                let (mut i, mut exists) = (0u32, BOOL(0));
                self.collection.FindFamilyName(self.family(family), &mut i, &mut exists).ok()?;
                let font = self.collection.GetFontFamily(i).ok()?.GetFirstMatchingFont(Self::weight(bold), DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL).ok()?;
                let mut m = DWRITE_FONT_METRICS::default();
                font.GetMetrics(&mut m);
                Some(m)
            })()
        };
        self.primary.insert((family, bold), m);
        m
    }
}

impl TextBackend for DWriteText {
    fn set_ui_family(&mut self, name: &str) -> bool {
        let wide: Vec<u16> = name.encode_utf16().chain([0]).collect();
        let found = !name.is_empty() && !name.contains(' ') && self.has_family(PCWSTR(wide.as_ptr()));
        self.ui_family = if found { wide } else { unsafe { self.default_ui.as_wide() }.iter().copied().chain([0]).collect() };
        self.formats.retain(|style, _| style.family != FontFamily::Ui);
        self.primary.retain(|(family, _), _| *family != FontFamily::Ui);
        found || name.is_empty()
    }

    fn families(&mut self) -> Vec<String> {
        let n = unsafe { self.collection.GetFontFamilyCount() };
        let mut out: Vec<String> = (0..n).filter_map(|i| unsafe { self.collection.GetFontFamily(i) }.ok()).filter_map(|f| Self::family_name(&f)).filter(|s| !s.is_empty()).collect();
        out.sort_by_key(|s| s.to_lowercase());
        out.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
        out
    }

    fn shape(&mut self, text: &str, style: FontStyle, out: &mut ShapedText) {
        out.glyphs.clear();
        out.width = 0.0;
        if text.is_empty() {
            return;
        }
        let Some(format) = self.format(style) else { return };
        self.wide.clear();
        self.byte_of.clear();
        for (b, c) in text.char_indices() {
            let mut buf = [0u16; 2];
            for &u in c.encode_utf16(&mut buf).iter() {
                self.wide.push(u);
                self.byte_of.push(b as u32);
            }
        }
        self.byte_of.push(text.len() as u32);
        let runs = unsafe {
            let Ok(layout) = self.factory.CreateTextLayout(&self.wide, &format, 1.0e6, 1.0e6) else { return };
            self.collector.runs.borrow_mut().clear();
            if layout.Draw(None, &self.renderer, 0.0, 0.0).is_err() {
                return;
            }
            let mut m = DWRITE_TEXT_METRICS::default();
            if layout.GetMetrics(&mut m).is_ok() {
                out.width = m.widthIncludingTrailingWhitespace;
            }
            std::mem::take(&mut *self.collector.runs.borrow_mut())
        };
        let base_y = runs.first().map_or(0.0, |r| r.y);
        for r in runs {
            let font = self.face_index(&r.face);
            let mut first_char = vec![u32::MAX; r.ids.len()];
            for (j, &g) in r.cluster_map.iter().enumerate().rev() {
                if let Some(slot) = first_char.get_mut(g as usize) {
                    *slot = j as u32;
                }
            }
            let mut cluster = 0u32;
            let mut pen = 0.0;
            for (i, &first) in first_char.iter().enumerate() {
                if first != u32::MAX {
                    cluster = first;
                }
                let adv = r.advances[i];
                let (ox, oy) = r.offsets[i];
                let x = if r.rtl { r.x - pen - adv - ox } else { r.x + pen + ox };
                let utf16 = (r.text_pos + cluster) as usize;
                let byte = self.byte_of.get(utf16).copied().unwrap_or(text.len() as u32);
                out.glyphs.push(ShapedGlyph { font, id: r.ids[i], x, y: r.y - base_y - oy, cluster: byte });
                pen += adv;
            }
        }
    }

    fn rasterize(&mut self, key: GlyphKey, out: &mut GlyphBitmap) -> bool {
        let Some(face) = self.faces.get(key.font as usize) else { return false };
        let id = key.id;
        let advance = 0.0f32;
        let offset = DWRITE_GLYPH_OFFSET::default();
        let run = DWRITE_GLYPH_RUN {
            fontFace: ManuallyDrop::new(Some(face.clone())),
            fontEmSize: key.size_q as f32 / 4.0,
            glyphCount: 1,
            glyphIndices: &id,
            glyphAdvances: &advance,
            glyphOffsets: &offset,
            isSideways: false.into(),
            bidiLevel: 0,
        };
        let result = unsafe {
            (|| {
                let analysis = self
                    .factory
                    .CreateGlyphRunAnalysis(
                        &run,
                        None,
                        DWRITE_RENDERING_MODE_NATURAL_SYMMETRIC,
                        DWRITE_MEASURING_MODE_NATURAL,
                        DWRITE_GRID_FIT_MODE_DEFAULT,
                        DWRITE_TEXT_ANTIALIAS_MODE_GRAYSCALE,
                        key.subpx as f32 / 4.0,
                        0.0,
                    )
                    .ok()?;
                let r = analysis.GetAlphaTextureBounds(DWRITE_TEXTURE_ALIASED_1x1).ok()?;
                let (w, h) = ((r.right - r.left).max(0) as u32, (r.bottom - r.top).max(0) as u32);
                out.left = r.left;
                out.top = -r.top;
                out.width = w;
                out.height = h;
                out.data.resize((w * h) as usize, 0);
                if w > 0 && h > 0 {
                    analysis.CreateAlphaTexture(DWRITE_TEXTURE_ALIASED_1x1, &r, &mut out.data).ok()?;
                }
                Some(())
            })()
        };
        drop(ManuallyDrop::into_inner(run.fontFace));
        result.is_some()
    }

    fn metrics(&mut self, style: FontStyle) -> FontMetrics {
        let px = style.px();
        match self.design_metrics(style.family, style.bold) {
            Some(m) if m.designUnitsPerEm > 0 => {
                let k = px / m.designUnitsPerEm as f32;
                FontMetrics { ascent: m.ascent as f32 * k, descent: m.descent as f32 * k }
            }
            _ => FontMetrics { ascent: px * 0.8, descent: px * 0.2 },
        }
    }
}

#[cfg(test)]
#[path = "../test/dwrite.rs"]
mod tests;
