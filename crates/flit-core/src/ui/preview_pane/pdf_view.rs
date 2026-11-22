use super::{App, Bitmap, Content, Effect, GenToken, Gfx, Kind, PreviewRequest, Rect, TextBackend, Theme};
use crate::hash::{self, FxHashMap, FxHashSet};
use crate::i18n::S;
use crate::preview::pdf::{Size, PAGE_PX};
use std::ops::Range;

const PENDING_LIMIT: usize = 2;

pub(super) struct PdfView {
    pages: Vec<Option<Size>>,
    estimate: Size,
    tops: Vec<f32>,
    geometry: (f32, f32),
    pending: FxHashMap<usize, (u64, GenToken)>,
    failed: FxHashSet<usize>,
}

impl PdfView {
    pub(super) fn new(pages: Vec<Option<Size>>) -> Self {
        let estimate = pages.iter().flatten().copied().next().unwrap_or(Size { width: 612.0, height: 792.0 });
        Self { pages, estimate, tops: Vec::new(), geometry: (0.0, 0.0), pending: FxHashMap::default(), failed: FxHashSet::default() }
    }

    pub(super) fn retained_bytes(&self) -> usize {
        self.pages.capacity() * size_of::<Option<Size>>() + self.tops.capacity() * size_of::<f32>() + self.pending.capacity() * size_of::<(usize, u64, GenToken)>() + self.failed.capacity() * size_of::<usize>()
    }

    fn arrange(&mut self, width: f32, gap: f32, scroll: f32, height: f32) -> Range<usize> {
        if self.geometry != (width, gap) {
            self.tops.clear();
            self.geometry = (width, gap);
            let mut y = gap;
            for size in &self.pages {
                self.tops.push(y);
                y += Self::height(size.unwrap_or(self.estimate), width) + gap;
            }
            self.tops.push(y);
        }
        let (first, last) = (self.tops.partition_point(|&y| y <= scroll).saturating_sub(1), self.tops.partition_point(|&y| y < scroll + height).min(self.pages.len()));
        first..last
    }

    fn height(size: Size, width: f32) -> f32 {
        (size.height / size.width).clamp(0.05, 20.0) * width
    }

    fn measure(&mut self, i: usize, (w, h): (u32, u32), scroll: &mut f32) {
        let size = Size { width: w as f32, height: h as f32 };
        self.pages[i] = Some(size);
        if let Some(&top) = self.tops.get(i) {
            let width = self.geometry.0;
            let old_height = Self::height(self.estimate, width);
            let change = Self::height(size, width) - old_height;
            *scroll += ((*scroll - top) / old_height).clamp(0.0, 1.0) * change;
            for top in &mut self.tops[i + 1..] {
                *top += change;
            }
        }
    }

    pub(super) fn cancel(&mut self) {
        for (_, (_, token)) in self.pending.drain() {
            token.set(0);
        }
    }
}

impl Drop for PdfView {
    fn drop(&mut self) {
        self.cancel();
    }
}

fn page_key(file: u64, i: usize) -> u64 {
    hash::hash(&("pdf-preview", file, i))
}

impl App {
    pub fn on_pdf_page<B: TextBackend>(&mut self, g: &mut Gfx<B>, gen: u64, page: u32, image: Option<Bitmap>) {
        let Some(i) = page.checked_sub(1).map(|i| i as usize) else { return };
        let Content::Pdf(pdf) = &mut self.preview.content else { return };
        if pdf.pending.get(&i).is_none_or(|(ask, _)| *ask != gen) {
            return;
        }
        pdf.pending.remove(&i);
        if !image.is_some_and(|b| {
            if !g.thumbs.insert(page_key(self.preview.key, i), b.width, b.height, &b.rgba) {
                return false;
            }
            if pdf.pages[i].is_none() {
                pdf.measure(i, (b.width, b.height), &mut self.preview.scroll);
            }
            true
        }) {
            pdf.failed.insert(i);
        }
        self.dirty = true;
    }

    pub(super) fn draw_pdf<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme) -> f32 {
        let font = self.ui_font(12.5);
        let gap = self.s(super::PAD);
        let body = self.preview.r.body;
        let width = (body.w - 2.0 * gap).min(self.s(720.0)).max(1.0);
        let p = &mut self.preview;
        let Content::Pdf(pdf) = &mut p.content else { return 0.0 };
        let visible = pdf.arrange(width, gap, p.scroll, body.h);
        pdf.pending.retain(|i, (_, token)| {
            if visible.contains(i) {
                true
            } else {
                token.set(0);
                false
            }
        });
        let mut requests = Vec::new();
        for i in visible {
            let key = page_key(p.key, i);
            if pdf.pages[i].is_none() {
                if let Some(dimensions) = g.thumbs.dimensions(key) {
                    pdf.measure(i, dimensions, &mut p.scroll);
                    self.motion = true;
                }
            }
            let rect = Rect::new(body.x + (body.w - width) / 2.0, body.y + pdf.tops[i] - p.scroll, width, PdfView::height(pdf.pages[i].unwrap_or(pdf.estimate), width));
            if rect.bottom() <= body.y || rect.y >= body.bottom() || g.thumbs.draw_fit(&mut g.dl, key, rect, 255, f32::INFINITY) {
                continue;
            }
            let failed = pdf.failed.contains(&i);
            g.dl.rect(rect, t.input_bg);
            if rect.h >= gap * 4.0 {
                let label = if failed { S::PreviewPdfFailed } else { S::PreviewLoading };
                let label_rect = Rect { y: rect.y.max(body.y), h: gap * 4.0, ..rect };
                super::centered(g, label_rect, label.t(), font, t.text_dim);
            }
            if !failed && !pdf.pending.contains_key(&i) && pdf.pending.len() + requests.len() < PENDING_LIMIT {
                requests.push(i);
            }
        }
        let extent = pdf.tops.last().copied().unwrap_or(0.0);
        for i in requests {
            let gen = self.gen();
            let p = &mut self.preview;
            let (Content::Pdf(pdf), Some(path)) = (&mut p.content, p.subject.as_ref().and_then(|s| s.path.clone())) else { break };
            let cancel = GenToken::default();
            cancel.set(gen);
            pdf.pending.insert(i, (gen, cancel.clone()));
            let req = PreviewRequest { gen, cancel, path, kind: Kind::Other, size: 0, details: false, px: PAGE_PX };
            self.effects.push(Effect::PreviewPdfPage { req, page: i as u32 + 1 });
        }
        extent
    }
}
