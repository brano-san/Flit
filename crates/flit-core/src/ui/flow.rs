use crate::render::Rect;
use crate::view::DirView;
use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct FlowParams {
    pub across: bool,
    pub per: usize,
    pub cell: (f32, f32),
    pub head: f32,
}

impl Default for FlowParams {
    fn default() -> Self {
        Self { across: false, per: 1, cell: (1.0, 1.0), head: 1.0 }
    }
}

#[derive(Clone, Copy, Debug)]
struct Span {
    line: usize,
    rows: usize,
    head: bool,
    at: f32,
}

pub(super) struct Flow {
    p: FlowParams,
    spans: Vec<Span>,
    end: f32,
}

impl Flow {
    pub fn new(p: FlowParams, v: Option<&DirView>) -> Self {
        let p = FlowParams { per: p.per.max(1), cell: (p.cell.0.max(1.0), p.cell.1.max(1.0)), head: p.head.max(1.0), ..p };
        let mut f = Self { p, spans: Vec::new(), end: 0.0 };
        let Some(v) = v else { return f };
        let (groups, n) = (v.groups(), v.rows().len());
        if groups.is_empty() {
            f.push(Span { line: 0, rows: n, head: false, at: 0.0 });
        }
        for (g, gr) in groups.iter().enumerate() {
            let next = groups.get(g + 1).map_or(n, |x| x.start as usize);
            f.push(Span { line: gr.line as usize, rows: next - gr.start as usize, head: true, at: 0.0 });
        }
        f
    }

    fn push(&mut self, mut s: Span) {
        s.at = self.end;
        self.end += self.span_extent(&s);
        self.spans.push(s);
    }

    pub fn extent(&self) -> f32 {
        self.end
    }

    fn ext(&self) -> f32 {
        if self.p.across { self.p.cell.0 } else { self.p.cell.1 }
    }

    fn slot_ext(&self) -> f32 {
        if self.p.across { self.p.cell.1 } else { self.p.cell.0 }
    }

    fn head_slots(&self, s: &Span) -> usize {
        match (s.head, self.p.across) {
            (false, _) => 0,
            (true, true) => 1,
            (true, false) => self.p.per,
        }
    }

    fn bands(&self, s: &Span) -> usize {
        (self.head_slots(s) + s.rows).div_ceil(self.p.per)
    }

    fn head_band(&self, s: &Span, b: usize) -> bool {
        s.head && b == 0 && !self.p.across
    }

    fn span_extent(&self, s: &Span) -> f32 {
        let n = self.bands(s);
        if s.head && !self.p.across { self.p.head + (n - 1) as f32 * self.ext() } else { n as f32 * self.ext() }
    }

    fn band_span(&self, s: &Span, b: usize) -> (f32, f32) {
        match (self.head_band(s, b), s.head && !self.p.across) {
            (true, _) => (s.at, self.p.head),
            (false, true) => (s.at + self.p.head + (b - 1) as f32 * self.ext(), self.ext()),
            (false, false) => (s.at + b as f32 * self.ext(), self.ext()),
        }
    }

    fn span_of(&self, line: usize) -> Option<usize> {
        self.spans.partition_point(|s| s.line <= line).checked_sub(1)
    }

    fn pos_of(&self, s: &Span, line: usize) -> usize {
        match (s.head, line - s.line) {
            (true, 0) => 0,
            (true, k) => self.head_slots(s) + k - 1,
            (false, k) => k,
        }
    }

    fn line_of(&self, s: &Span, pos: usize) -> usize {
        let h = self.head_slots(s);
        if !s.head {
            s.line + pos
        } else if pos < h {
            s.line
        } else {
            s.line + 1 + pos - h
        }
    }

    fn last_pos(&self, s: &Span) -> usize {
        (self.head_slots(s) + s.rows).saturating_sub(1)
    }

    pub fn cell(&self, line: usize) -> Rect {
        let Some(k) = self.span_of(line) else { return Rect::default() };
        let s = &self.spans[k];
        let pos = self.pos_of(s, line);
        let (b, slot) = (pos / self.p.per, pos % self.p.per);
        let (at, ext) = self.band_span(s, b);
        if self.head_band(s, b) {
            return Rect::new(0.0, at, self.p.per as f32 * self.p.cell.0, ext);
        }
        let cross = slot as f32 * self.slot_ext();
        if self.p.across { Rect::new(at, cross, ext, self.p.cell.1) } else { Rect::new(cross, at, self.p.cell.0, ext) }
    }

    fn locate(&self, along: f32) -> Option<(usize, usize)> {
        if along < 0.0 || along >= self.end {
            return None;
        }
        let k = self.spans.partition_point(|s| s.at <= along).checked_sub(1)?;
        let s = &self.spans[k];
        let rel = along - s.at;
        let b = match s.head && !self.p.across {
            true if rel < self.p.head => 0,
            true => 1 + ((rel - self.p.head) / self.ext()) as usize,
            false => (rel / self.ext()) as usize,
        };
        Some((k, b.min(self.bands(s) - 1)))
    }

    pub fn line_at(&self, along: f32, cross: f32) -> Option<usize> {
        let (k, b) = self.locate(along)?;
        let s = &self.spans[k];
        if self.head_band(s, b) {
            return Some(s.line);
        }
        let slot = (cross / self.slot_ext()).floor();
        if !(0.0..self.p.per as f32).contains(&slot) {
            return None;
        }
        let pos = b * self.p.per + slot as usize;
        (pos <= self.last_pos(s) && (s.head || s.rows > 0)).then(|| self.line_of(s, pos))
    }

    pub fn nearest(&self, along: f32, cross: f32) -> Option<usize> {
        let (k, b) = self.locate(along.clamp(0.0, (self.end - 0.5).max(0.0)))?;
        let s = &self.spans[k];
        let slot = ((cross / self.slot_ext()).floor().max(0.0) as usize).min(self.p.per - 1);
        Some(self.line_of(s, (b * self.p.per + slot).min(self.last_pos(s))))
    }

    fn band_lines(&self, k: usize, b: usize) -> Range<usize> {
        let s = &self.spans[k];
        let last = self.last_pos(s);
        let start = self.line_of(s, (b * self.p.per).min(last));
        let end = self.line_of(s, ((b + 1) * self.p.per - 1).min(last)) + 1;
        start..end.max(start + (s.head || s.rows > 0) as usize)
    }

    pub fn lines_in(&self, a: f32, b: f32) -> Range<usize> {
        if self.spans.is_empty() || b <= 0.0 || a >= self.end {
            return 0..0;
        }
        let first = self.locate(a.max(0.0)).map_or(0..0, |(k, band)| self.band_lines(k, band));
        let last = self.locate(b.min(self.end) - 0.001).map_or(0..0, |(k, band)| self.band_lines(k, band));
        first.start..last.end.max(first.start)
    }

    pub fn step(&self, line: usize, dir: isize) -> Option<usize> {
        let k = self.span_of(line)?;
        let s = &self.spans[k];
        let pos = self.pos_of(s, line);
        let slot = pos % self.p.per;
        let (mut k, mut b) = (k, pos / self.p.per);
        loop {
            if dir > 0 {
                b += 1;
                if b >= self.bands(&self.spans[k]) {
                    k += 1;
                    b = 0;
                }
            } else if b > 0 {
                b -= 1;
            } else {
                k = k.checked_sub(1)?;
                b = self.bands(&self.spans[k]) - 1;
            }
            let s = self.spans.get(k)?;
            if self.head_band(s, b) || s.rows == 0 {
                continue;
            }
            let pos = (b * self.p.per + slot).max(self.head_slots(s)).min(self.last_pos(s));
            return Some(self.line_of(s, pos));
        }
    }
}

#[cfg(test)]
#[path = "../../test/ui/flow.rs"]
mod tests;
