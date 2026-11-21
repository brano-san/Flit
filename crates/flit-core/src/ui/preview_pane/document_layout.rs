use super::*;
use crate::preview::Alignment;

pub(super) struct TextFlow<'a, B: TextBackend> {
    pub gfx: &'a mut Gfx<B>,
    pub doc: &'a Doc,
    pub widths: &'a mut FxHashMap<(char, FontStyle), f32>,
    pub scale: f32,
    pub mono: f32,
    pub block: Block,
}

#[derive(Default)]
struct Column {
    widest_glyph: f32,
    widest_word: f32,
    natural: f32,
}

pub(super) struct Grid {
    left: f32,
    top: f32,
    header_bottom: f32,
    bottom: f32,
    edges: Vec<f32>,
}

impl Grid {
    pub(super) fn at(grids: &[Self], y: f32) -> Option<&Self> {
        grids.get(grids.partition_point(|grid| grid.bottom <= y)).filter(|grid| grid.top <= y)
    }

    pub(super) fn cell_clip(&self, body: Rect, x: f32) -> Rect {
        let index = self.edges.partition_point(|&right| right <= x);
        let left = index.checked_sub(1).map_or(self.left, |i| self.edges[i]);
        let right = self.edges.get(index).copied().unwrap_or(left);
        Rect::new(body.x + left, body.y, right - left, body.h)
    }

    pub(super) fn paint<B: TextBackend>(grids: &[Self], g: &mut Gfx<B>, theme: &Theme, body: Rect, scroll: f32) {
        let first = grids.partition_point(|grid| grid.bottom < scroll);
        for grid in grids[first..].iter().take_while(|grid| grid.top <= scroll + body.h) {
            let top = body.y + grid.top - scroll;
            let header = body.y + grid.header_bottom - scroll;
            let width = grid.edges.last().copied().unwrap_or(grid.left) - grid.left;
            g.dl.rect(Rect::new(body.x + grid.left, top, width, header - top), theme.hover);
            g.dl.rect(Rect::new(body.x + grid.left, header - 1.0, width, 1.0), theme.separator);
            for &edge in &grid.edges[..grid.edges.len().saturating_sub(1)] {
                g.dl.rect(Rect::new(body.x + edge - 1.0, top, 1.0, grid.bottom - grid.top), theme.separator);
            }
        }
    }
}

impl<B: TextBackend> TextFlow<'_, B> {
    fn advance(&mut self, c: char, font: FontStyle) -> f32 {
        char_w(self.gfx, self.widths, c, font)
    }

    pub(super) fn lines(
        &mut self,
        runs: Range<u32>,
        area: Range<f32>,
        wrap: bool,
        pieces: &mut Vec<Piece>,
        mut emit: impl FnMut(Range<u32>, f32),
    ) {
        let mut first = pieces.len();
        let left = area.start;
        let mut x = left;
        let cell = matches!(self.block, Block::Table(_));
        let mut finish = |end: usize, first: &mut usize, x: &mut f32| {
            emit(*first as u32..end as u32, *x);
            *first = end;
            *x = left;
        };
        for ri in runs {
            let run = self.doc.runs[ri as usize];
            let font = run_font(&run, self.block, self.scale, self.mono);
            let mut at = run.start;
            for part in self.doc.run_text(&run).split_inclusive([' ', '\n']) {
                let start = at;
                at += part.len() as u32;
                let token = part.strip_suffix('\n').unwrap_or(part);
                let word = token.trim_end_matches(' ');
                let core: f32 = word.chars().map(|c| self.advance(c, font)).sum();
                let trailing = token.len() - word.len();
                let space = if trailing > 0 { self.advance(' ', font) } else { 0.0 };
                if wrap && x > left && x + core > area.end {
                    finish(pieces.len(), &mut first, &mut x);
                    if word.is_empty() && token.len() == part.len() {
                        continue;
                    }
                }
                if wrap && core > area.end - left {
                    let mut pos = start;
                    for c in token.chars() {
                        let cw = self.advance(c, font);
                        if x > left && x + cw > area.end {
                            if cell && c == ' ' {
                                break;
                            }
                            finish(pieces.len(), &mut first, &mut x);
                        }
                        put(pieces, first, ri, pos, pos + c.len_utf8() as u32, x);
                        pos += c.len_utf8() as u32;
                        x += cw;
                    }
                } else {
                    let fitted = if cell && x + core + space * trailing as f32 > area.end { word } else { token };
                    if !fitted.is_empty() {
                        put(pieces, first, ri, start, start + fitted.len() as u32, x);
                        x += core + space * (fitted.len() - word.len()) as f32;
                    }
                }
                if token.len() < part.len() {
                    finish(pieces.len(), &mut first, &mut x);
                }
            }
        }
        finish(pieces.len(), &mut first, &mut x);
    }

    fn columns(&mut self, table: &preview::Table) -> Vec<Column> {
        let mut cols: Vec<_> = (0..table.align.len()).map(|_| Column::default()).collect();
        for row in &table.rows {
            for (col, cell) in cols.iter_mut().zip(row) {
                let mut x = 0.0;
                let mut word = 0.0;
                for i in cell.clone() {
                    let r = self.doc.runs[i as usize];
                    let font = run_font(&r, self.block, self.scale, self.mono);
                    for c in self.doc.run_text(&r).chars() {
                        if c == '\n' {
                            x = 0.0;
                            word = 0.0;
                        } else {
                            let cw = self.advance(c, font);
                            x += cw;
                            word = if c == ' ' { 0.0 } else { word + cw };
                            col.widest_word = col.widest_word.max(word);
                            col.widest_glyph = col.widest_glyph.max(cw);
                            col.natural = col.natural.max(x);
                        }
                    }
                }
            }
        }
        cols
    }

    pub(super) fn table(&mut self, index: usize, para: u32, area: Range<f32>, (lh, base): (f32, f32), y: &mut f32, out: &mut DocLayout) {
        let width = area.end - area.start;
        let table = &self.doc.tables[index];
        let columns = self.columns(table);
        let mut first_col = 0;
        while first_col < columns.len() {
            let mut min = 0.0;
            let mut end = first_col;
            for col in &columns[first_col..] {
                let needed = col.widest_glyph.max(1.0);
                if end > first_col && min + needed > width {
                    break;
                }
                min += needed;
                end += 1;
            }
            let cols = &columns[first_col..end];
            let words: f32 = cols.iter().map(|c| c.widest_word.max(c.widest_glyph).max(1.0)).sum();
            let minimum = |c: &Column| {
                if words <= width { c.widest_word.max(c.widest_glyph).max(1.0) } else { c.widest_glyph.max(1.0) }
            };
            let min = cols.iter().map(minimum).sum::<f32>();
            let pad = (4.0 * self.scale).round().min(((width - min) / (2 * cols.len()) as f32).max(0.0));
            let extra: f32 = cols.iter().map(|c| (c.natural - minimum(c)).max(0.0)).sum();
            let ratio = if extra > 0.0 { ((width - min - 2.0 * pad * cols.len() as f32) / extra).clamp(0.0, 1.0) } else { 0.0 };
            let mut x = 0.0;
            let edges: Vec<_> = cols.iter().map(|c| {
                let min = minimum(c);
                x += min + (c.natural - min).max(0.0) * ratio + 2.0 * pad;
                area.start + x.min(width)
            }).collect();
            let mut grid = Grid { left: area.start, top: *y, bottom: *y, header_bottom: *y, edges };
            for (row_no, row) in table.rows.iter().enumerate() {
                let mut cell_pieces = Vec::new();
                let mut visual: Vec<Vec<Piece>> = Vec::new();
                let mut left = area.start;
                for (offset, &right) in grid.edges.iter().enumerate() {
                    let col = first_col + offset;
                    let cell_width = (right - left - 2.0 * pad).max(0.0);
                    let mut lines = Vec::new();
                    cell_pieces.clear();
                    let runs = row.get(col).cloned().unwrap_or(0..0);
                    self.lines(runs, 0.0..cell_width, true, &mut cell_pieces, |r, w| lines.push((r, w)));
                    visual.resize_with(visual.len().max(lines.len()), Vec::new);
                    for (line, (range, used)) in visual.iter_mut().zip(lines) {
                        let slack = (cell_width - used).max(0.0);
                        let x = left + pad + match table.align[col] {
                            Alignment::Center => slack / 2.0,
                            Alignment::Right => slack,
                            _ => 0.0,
                        };
                        line.extend(cell_pieces[range.start as usize..range.end as usize].iter().map(|p| Piece { x: x + p.x, ..*p }));
                    }
                    left = right;
                }
                for pieces in visual {
                    let start = out.pieces.len() as u32;
                    out.pieces.extend(pieces);
                    out.lines.push(VLine { y: *y, h: lh, base, block: self.block, num: 0, para, pieces: start..out.pieces.len() as u32 });
                    *y += lh;
                }
                if row_no == 0 {
                    grid.header_bottom = *y;
                }
            }
            grid.bottom = *y;
            out.wide = out.wide.max(grid.edges.last().copied().unwrap_or(0.0));
            out.grids.push(grid);
            *y += (6.0 * self.scale).round();
            first_col = end;
        }
    }
}
