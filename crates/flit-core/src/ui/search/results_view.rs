use super::*;

impl App {
    pub(super) fn search_layout(&self) -> SLayout {
        let l = &self.layout;
        let w = (l.address.right() - l.nav[1].x).max(self.s(40.0));
        let button = self.s(30.0).min(w / 6.0);
        let field = Rect::new(l.nav[1].x, l.address.y, w - button * 3.0, l.address.h);
        let tools = std::array::from_fn(|i| Rect::new(field.right() + button * i as f32, field.y, button, field.h));
        let mut list = l.list;
        let fw = if self.search.folders_open { self.s(255.0).min(list.w * 0.38) } else { 0.0 };
        let folder_y = l.header.y;
        let chip_h = if self.search.similar && !self.search.edit.text.trim().is_empty() { self.s(29.0) } else { 0.0 };
        let chips = Rect::new(list.x, folder_y, list.w, chip_h);
        let facet_h = if self.search.facets_open { self.s(29.0) } else { 0.0 };
        let facets = Rect::new(list.x, chips.bottom(), list.w, facet_h);
        let timeline = Rect::new(list.x, facets.bottom(), list.w, facet_h);
        let top = chip_h + 2.0 * facet_h;
        let folders = Rect::new(list.x, folder_y + top, fw, (list.bottom() - folder_y - top).max(0.0));
        list.y += top;
        list.h = (list.h - top).max(0.0);
        let track = Rect::new(l.track.x, l.track.y + top, l.track.w, (l.track.h - top).max(0.0));
        list.x += fw;
        list.w = (list.w - fw).max(0.0);
        let header = Rect::new(list.x, l.header.y + top, (l.header.w - fw).max(0.0), l.header.h);
        let size_w = self.s(90.0).min(list.w / 4.0);
        let date_w = self.s(140.0).min(list.w / 4.0);
        let name_w = (list.w * 0.32).max(self.s(140.0)).min(list.w - size_w - date_w);
        let folder_w = (list.w - name_w - size_w - date_w).max(0.0);
        let grid = self.settings.opts.view_rules.search_grid && !self.search.group_folder;
        let mut cols = [Rect::default(); 4];
        let defaults = [name_w, folder_w, size_w, date_w];
        let widths = if grid { defaults } else { self.search.widths.map(|widths| widths.map(|w| w * self.scale)).unwrap_or(defaults) };
        let content_w = widths.iter().sum::<f32>();
        let max_x = (content_w - list.w).max(0.0);
        let mut x = list.x - self.search.scroll_x.clamp(0.0, max_x);
        let htrack = if max_x > 0.0 { Rect::new(list.x, list.bottom() - self.s(14.0), list.w, self.s(14.0)) } else { Rect::default() };
        let hthumb = if max_x > 0.0 {
            list.h = (list.h - htrack.h).max(0.0);
            let width = (htrack.w * htrack.w / content_w).max(self.s(24.0)).min(htrack.w);
            Rect::new(htrack.x + (htrack.w - width) * self.search.scroll_x.clamp(0.0, max_x) / max_x, htrack.y + self.s(4.0), width, self.s(6.0))
        } else { Rect::default() };
        for (c, w) in cols.iter_mut().zip(widths) {
            *c = Rect::new(x, header.y, w, header.h);
            x += w;
        }
        let tiles = if grid { ((list.w / self.s(135.0).max(1.0)) as u64).max(1) } else { 1 };
        let rh = if grid { self.s(94.0) } else { l.row_h }.max(1.0);
        let content = self.search.lines().div_ceil(tiles) as f64 * rh as f64;
        let lh = list.h as f64;
        let thumb = if content > lh {
            let th = (track.h as f64 * lh / content).max(self.s(24.0) as f64).min(track.h as f64) as f32;
            let frac = (self.search.scroll / (content - lh)).clamp(0.0, 1.0) as f32;
            Rect::new(track.x + self.s(3.0), track.y + (track.h - th) * frac, track.w - self.s(6.0), th)
        } else {
            Rect::default()
        };
        SLayout { field, header, cols, list, track, thumb, htrack, hthumb, max_x, row_h: rh, folders, tools, chips, facets, timeline, grid, tiles }
    }

    pub(in crate::ui) fn search_pans_at(&self, x: f32, y: f32) -> bool {
        matches!(self.search_hit(x, y), SHit::Row(_) | SHit::Group(_) | SHit::Empty)
    }

    pub(in crate::ui) fn touch_pan_search(&mut self, dx: f32, dy: f32) -> (bool, bool) {
        let (max_x, max) = (self.search_layout().max_x, self.search_max_scroll());
        let s = &mut self.search;
        let x = (s.scroll_x - dx).clamp(0.0, max_x);
        let y = (s.scroll - f64::from(dy)).clamp(0.0, max);
        let moved = (x != s.scroll_x, y != s.scroll);
        (s.scroll_x, s.scroll) = (x, y);
        moved
    }

    pub(in crate::ui) fn search_let_go(&mut self) {
        self.search.pressed = SHit::None;
        self.search.drag = SDrag::None;
    }

    pub(super) fn search_max_scroll(&self) -> f64 {
        let sl = self.search_layout();
        (self.search.lines().div_ceil(sl.tiles) as f64 * f64::from(sl.row_h) - f64::from(sl.list.h)).max(0.0)
    }

    pub(super) fn search_range(&self) -> (u64, u64) {
        let sl = self.search_layout();
        let rh = f64::from(sl.row_h);
        let s = &self.search;
        let first = (s.scroll / rh).floor() as u64;
        let last = ((s.scroll + f64::from(sl.list.h)) / rh).ceil() as u64;
        (first.saturating_mul(sl.tiles).min(s.lines()), last.saturating_mul(sl.tiles).min(s.lines()))
    }

    pub(super) fn search_row_range(&self) -> (u64, u64) {
        let (first, last) = self.search_range();
        let s = &self.search;
        let row_of = |line: u64| match s.line_at(line) {
            SLine::Row(r) => r,
            SLine::Header(k) => s.groups[k],
        };
        if first >= last {
            return (row_of(first.min(s.lines().saturating_sub(1))).min(s.total), row_of(first.min(s.lines().saturating_sub(1))).min(s.total));
        }
        (row_of(first), (row_of(last - 1) + 1).min(s.total))
    }

    pub(in crate::ui) fn search_field_at(&self, x: f32, y: f32) -> bool {
        self.search.active && self.search_hit(x, y) == SHit::Field
    }

    pub(super) fn search_hit(&self, x: f32, y: f32) -> SHit {
        let sl = self.search_layout();
        if let Some(k) = self.search.recent_rects.iter().position(|r| r.contains(x, y)) {
            return SHit::Recent(k);
        }
        if sl.chips.h > 0.0 && sl.chips.contains(x, y) {
            if let Some((i, (_, r))) = self.search.similar_chips.iter().enumerate().find(|(_, (_, r))| r.contains(x, y)) {
                return SHit::SimilarChip(i, x >= r.right() - self.s(20.0));
            }
        }
        if self.search.facets_open {
            if sl.facets.contains(x, y) {
                return SHit::Facet((((x - sl.facets.x) / sl.facets.w.max(1.0)) * 4.0) as usize);
            }
            let n = self.search.facets.dates.len();
            if n > 0 && sl.timeline.contains(x, y) {
                let i = (((x - sl.timeline.x) / sl.timeline.w.max(1.0)) * n as f32) as usize;
                return SHit::Date(i.min(n - 1));
            }
        }
        if sl.tools[2].contains(x, y) {
            return SHit::Facets;
        }
        if sl.tools[0].contains(x, y) {
            return SHit::Folders;
        }
        if sl.tools[1].contains(x, y) {
            return SHit::WithinResults;
        }
        if sl.folders.w > 0.0 && sl.folders.contains(x, y) {
            let at = y - sl.folders.y - self.layout.header.h;
            if at >= 0.0 {
                let i = self.search.folders_scroll + (at / sl.row_h).floor() as usize;
                if i < self.search.folders_summary.len() {
                    return SHit::ResultFolder(i);
                }
            }
            return SHit::FolderBackground;
        }
        if self.layout.nav[0].contains(x, y) {
            return SHit::Back;
        }
        if self.search.chip.contains(x, y) {
            return SHit::Scope;
        }
        if sl.field.contains(x, y) {
            return SHit::Field;
        }
        if sl.header.contains(x, y) {
            if !sl.grid {
                if let Some(c) = (0..4).find(|&c| (x - sl.cols[c].right()).abs() <= self.s(4.0)) {
                    return SHit::ColSep(c);
                }
            }
            return sl.cols.iter().position(|c| c.contains(x, y)).map_or(SHit::None, SHit::Header);
        }
        if sl.track.contains(x, y) {
            let t = sl.thumb;
            return if t.h > 0.0 && y >= t.y && y < t.bottom() { SHit::Thumb } else { SHit::Track };
        }
        if sl.htrack.contains(x, y) {
            return if sl.hthumb.contains(x, y) { SHit::HThumb } else { SHit::HTrack };
        }
        if sl.list.contains(x, y) {
            let band = ((self.search.scroll + (y - sl.list.y) as f64) / sl.row_h as f64).floor() as u64;
            let col = (((x - sl.list.x) / sl.list.w * sl.tiles as f32) as u64).min(sl.tiles - 1);
            let line = band.saturating_mul(sl.tiles).saturating_add(col);
            if line >= self.search.lines() {
                return SHit::Empty;
            }
            return match self.search.line_at(line) {
                SLine::Row(r) => SHit::Row(r),
                SLine::Header(k) => SHit::Group(k),
            };
        }
        SHit::None
    }

    pub(in crate::ui) fn search_tip_hit(&self) -> Hit {
        match self.search.hover {
            SHit::Row(row) if self.search.active && self.search.drag == SDrag::None => Hit::SearchRow(row),
            _ => Hit::None,
        }
    }

    pub(in crate::ui) fn search_tip_anchor(&self, row: u64) -> Option<Rect> {
        if !self.search.active {
            return None;
        }
        let sl = self.search_layout();
        let cell = sl.cell(self.search.line_of_row(row), self.search.scroll);
        let y = cell.y;
        let name = if sl.grid { cell } else { sl.cols[0] };
        let (left, right) = (name.x.max(sl.list.x), name.right().min(sl.list.right()));
        let (top, bottom) = (y.max(sl.list.y), (y + sl.row_h).min(sl.list.bottom()));
        (bottom > top && right > left).then(|| Rect::new(left, top, right - left, bottom - top))
    }

    pub(in crate::ui) fn search_tip(&self, row: u64) -> Option<TipText> {
        if !self.search.active {
            return None;
        }
        let r = self.search.row(row)?;
        let tip = |line: String, details: Vec<String>| Some(TipText { line, command: None, keys: Vec::new(), details });
        if self.search.elided.contains(&row) {
            return tip(name::display(&r.name).into_owned(), Vec::new());
        }
        let arena = Bump::new();
        let labelled = |title: S, value: &str| (!value.is_empty()).then(|| format!("{}: {value}", title.t()));
        let is_dir = r.meta.attrs & attr::DIR != 0;
        let ext = if is_dir { &[][..] } else { &r.name[r.name.len() - name::ext_len(&r.name)..] };
        let line = labelled(S::DetailsColumnType, self.type_of(&arena, r.meta.attrs, ext))?;
        let mut details: Vec<String> = labelled(S::SearchColumnFolder, &name::display(&r.dir)).into_iter().collect();
        if !is_dir {
            let (mut size, mut date) = (String::new(), String::new());
            format::size_in(&mut size, r.meta.size, self.style.decimal);
            format::datetime_in(&mut date, r.meta.modified, self.utc_offset_min, &self.style);
            details.extend(labelled(S::DetailsColumnSize, &size));
            details.extend(labelled(S::DetailsColumnModified, &date));
        }
        tip(line, details)
    }

    fn fit_search_column<B: TextBackend>(&mut self, g: &mut Gfx<B>, arena: &Bump, col: usize) {
        let (font, hfont) = (self.ui_font(13.0), self.ui_font(12.0));
        let pad = self.s(10.0);
        let indent = if self.search.grouped() { self.s(16.0) } else { 0.0 };
        let lead = if col == 0 { indent + self.s(ICON) + self.s(8.0) } else { 0.0 };
        let mut w = g.measure(COLUMNS[col].0.t(), hfont) + self.s(20.0);
        let (first, last) = self.search_range();
        for line in first..last {
            let SLine::Row(row) = self.search.line_at(line) else { continue };
            let Some(r) = self.search.row(row) else { continue };
            let mut text = bumpalo::collections::String::new_in(arena);
            match col {
                0 => text.push_str(&name::display(&r.name)),
                1 => text.push_str(&name::display(&r.dir)),
                2 if r.meta.attrs & attr::DIR == 0 => format::size_in(&mut text, r.meta.size, self.style.decimal),
                2 => {}
                _ => format::datetime_in(&mut text, r.meta.modified, self.utc_offset_min, &self.style),
            }
            w = w.max(lead + g.measure(&text, font));
        }
        let mut widths = self.search.widths.unwrap_or_else(|| self.search_layout().cols.map(|c| c.w / self.scale));
        widths[col] = ((w + 2.0 * pad) / self.scale).clamp(48.0, 2400.0).ceil();
        self.search.widths = Some(widths);
    }

    fn draw_result_folders<B: TextBackend>(&self, g: &mut Gfx<B>, t: &Theme, sl: &SLayout) {
        let r = sl.folders;
        if r.w <= 0.0 {
            return;
        }
        let font = self.ui_font(12.0);
        let m = g.metrics(font);
        let pad = self.s(8.0);
        let title_y = r.y + (self.layout.header.h + m.ascent - m.descent) / 2.0;
        g.dl.set_clip(Some(r));
        g.dl.rect(r, t.chrome);
        g.text(r.x + pad, title_y, S::SearchExploreFolders.t(), font, t.text, (r.w - 2.0 * pad).max(0.0));
        let scroll = self.search.folders_scroll;
        let shown = ((r.h - self.layout.header.h).max(0.0) / sl.row_h).ceil() as usize;
        for (i, (path, count)) in self.search.folders_summary.iter().enumerate().skip(scroll).take(shown) {
            let y = r.y + self.layout.header.h + (i - scroll) as f32 * sl.row_h;
            let row = Rect::new(r.x + self.s(2.0), y, (r.w - self.s(4.0)).max(0.0), sl.row_h);
            let selected = self.search.result_folder.as_ref().is_some_and(|p| p.as_bytes() == path);
            if selected || self.search.hover == SHit::ResultFolder(i) {
                g.dl.round_rect(row, if selected { t.selection_inactive } else { t.button_hover }, self.s(4.0));
            }
            let y = row.y + (row.h + m.ascent - m.descent) / 2.0;
            let count = thousands(*count);
            let cw = g.measure(&count, font);
            g.text(r.right() - pad - cw, y, &count, font, t.text_dim, cw);
            let width = (r.w - cw - 3.0 * pad).max(0.0);
            let mut label = name::display(path);
            if g.measure(&label, font) > width {
                let sep = if self.caps.unix_paths { '/' } else { '\\' };
                let mut suffixes = label.trim_end_matches(sep).rmatch_indices(sep).map(|(start, _)| &label[start..]);
                if let Some(mut tail) = suffixes.next() {
                    let available = width - g.measure("…", font);
                    for suffix in suffixes.take_while(|suffix| g.measure(suffix, font) <= available) {
                        tail = suffix;
                    }
                    label = format!("…{tail}").into();
                }
            }
            g.text(r.x + pad, y, &label, font, t.text, width);
        }
        g.dl.set_clip(None);
    }

    fn draw_facets<B: TextBackend>(&self, g: &mut Gfx<B>, t: &Theme, sl: &SLayout) {
        if sl.facets.h <= 0.0 {
            return;
        }
        let font = self.ui_font(12.0);
        let m = g.metrics(font);
        let pad = self.s(6.0);
        let row = sl.facets;
        let timeline = sl.timeline;
        let captions = [S::DetailsColumnType, S::SearchExploreYear, S::DetailsColumnSize, S::DetailsColumnTag];
        g.dl.rect(row, t.chrome);
        g.dl.rect(timeline, t.chrome);
        for (i, label) in captions.into_iter().enumerate() {
            let r = Rect::new(row.x + i as f32 * row.w / 4.0, row.y, row.w / 4.0, row.h);
            let active = self.search.facet_filter[i].as_deref();
            if active.is_some() || self.search.hover == SHit::Facet(i) {
                g.dl.round_rect(r.inset(self.s(1.0), self.s(2.0)), if active.is_some() { t.selection_inactive } else { t.button_hover }, self.s(4.0));
            }
            let y = r.y + (r.h + m.ascent - m.descent) / 2.0;
            let text = active.map_or_else(|| label.t().to_owned(), |a| format!("{} · {a}", label.t()));
            g.text(r.x + pad, y, &text, font, t.text, (r.w - 2.0 * pad).max(0.0));
        }
        let dates = &self.search.facets.dates;
        if dates.is_empty() {
            return;
        }
        let total = dates.iter().map(|d| d.count).max().unwrap_or(1).max(1) as f32;
        let w = timeline.w / dates.len() as f32;
        for (i, b) in dates.iter().enumerate() {
            let h = (timeline.h - self.s(4.0)) * (b.count as f32 / total);
            let x = timeline.x + i as f32 * w;
            let dragging = matches!((self.search.pressed, self.search.hover), (SHit::Date(a), SHit::Date(z)) if (a.min(z)..=a.max(z)).contains(&i));
            let chosen = self.search.date_filter.is_some_and(|(a, z)| b.from <= z && b.to >= a);
            if dragging || chosen || self.search.hover == SHit::Date(i) {
                g.dl.rect(Rect::new(x, timeline.y, w, timeline.h), t.selection_inactive);
            }
            if h > 0.0 {
                g.dl.rect(Rect::new(x, timeline.bottom() - h, (w - self.s(2.0)).max(1.0), h), t.accent.alpha(0xB0));
            }
        }
        let range = if let SHit::Date(i) = self.search.hover {
            dates.get(i).map(|b| (b.from, b.to))
        } else {
            self.search.date_filter
        };
        if let Some((a, z)) = range {
            let text = if a == z { day_query(a) } else { format!("{} – {}", day_query(a), day_query(z)) };
            let w = g.measure(&text, font) + 2.0 * pad;
            let r = Rect::new(timeline.right() - w, timeline.y, w, timeline.h);
            g.dl.round_rect(r, t.chrome, self.s(4.0));
            g.text(r.x + pad, r.y + (r.h + m.ascent - m.descent) / 2.0, &text, font, t.text, w);
        }
    }

    fn draw_similar_chips<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme, r: Rect) {
        self.search.similar_chips.clear();
        if r.h <= 0.0 {
            return;
        }
        let font = self.ui_font(12.0);
        let metrics = g.metrics(font);
        let (pad, close) = (self.s(7.0), self.s(20.0));
        let mut x = r.x + pad;
        g.dl.set_clip(Some(r));
        g.dl.rect(r, t.chrome);
        for range in crate::filter::token_ranges(&self.search.edit.text) {
            let label = &self.search.edit.text[range.clone()];
            let remaining = r.right() - x - pad;
            if remaining < self.s(50.0) {
                break;
            }
            let w = (g.measure(label, font) + 2.0 * pad + close).min(remaining);
            let chip = Rect::new(x, r.y + self.s(3.0), w, (r.h - self.s(6.0)).max(0.0));
            let i = self.search.similar_chips.len();
            let hot = matches!(self.search.hover, SHit::SimilarChip(k, _) if k == i);
            g.dl.round_rect(chip, if hot { t.button_hover } else { t.selection_inactive }, self.s(4.0));
            let y = chip.y + (chip.h + metrics.ascent - metrics.descent) / 2.0;
            g.text(chip.x + pad, y, label, font, t.text, (w - 2.0 * pad - close).max(0.0));
            centered(g, Rect::new(chip.right() - close, chip.y, close, chip.h), crate::ui::G_CLOSE, self.icon_font(8.0), t.text_dim);
            self.search.similar_chips.push((range, chip));
            x = chip.right() + pad;
        }
        g.dl.set_clip(None);
    }

    pub(in crate::ui) fn draw_search<B: TextBackend>(&mut self, g: &mut Gfx<B>, arena: &Bump, t: &Theme) {
        if let Some(col) = self.search.fit.take() {
            self.fit_search_column(g, arena, col);
        }
        self.search.scroll_x = self.search.scroll_x.clamp(0.0, self.search_layout().max_x);
        self.search.scroll = self.search.scroll.clamp(0.0, self.search_max_scroll());
        self.request_visible_pages();
        let sl = self.search_layout();
        self.draw_similar_chips(g, t, sl.chips);
        self.draw_facets(g, t, &sl);
        let font = self.ui_font(13.0);
        let m = g.metrics(font);
        let rh = sl.row_h;
        let text_dy = ((rh + m.ascent - m.descent) / 2.0).round();
        let pad = self.s(10.0);
        let icon_px = self.s(if sl.grid { 42.0 } else { ICON });
        let focused = self.active;
        let (first, last) = self.search_range();
        g.dl.set_clip(Some(sl.list));
        let mut requests: Vec<IconRequest> = Vec::new();
        let mut elided: Vec<u64> = Vec::new();
        let mut type_requests: Vec<Vec<u8>> = Vec::new();
        let s = &self.search;
        self.draw_result_folders(g, t, &sl);
        g.dl.set_clip(Some(sl.list));
        let headers = s.grouped();
        if s.done && s.total == 0 && !s.walking {
            let msg = if s.error.is_some() { S::SearchInvalid.t() } else { S::SearchNoMatch.t() };
            let w = g.measure(msg, font);
            g.text(sl.list.x + (sl.list.w - w) / 2.0, sl.list.y + self.s(40.0), msg, font, t.text_dim, f32::INFINITY);
        }
        for line in first..last {
            let cell = sl.cell(line, s.scroll);
            let y = cell.y;
            let row = match s.line_at(line) {
                SLine::Row(r) => r,
                SLine::Header(k) => {
                    self.draw_search_group(g, arena, t, k, Rect::new(sl.list.x, y, sl.list.w, rh), &mut requests);
                    continue;
                }
            };
            let rr = Rect::new(cell.x + self.s(4.0), y + 1.0, cell.w - self.s(8.0), rh - 2.0);
            if s.focus == Some(row) {
                g.dl.round_rect(rr, if focused { t.selection } else { t.selection_inactive }, self.s(4.0));
            } else if s.hover == SHit::Row(row) && s.drag == SDrag::None {
                g.dl.round_rect(rr, t.hover, self.s(4.0));
            }
            let Some(r) = s.row(row) else { continue };
            let is_dir = r.meta.attrs & attr::DIR != 0;
            let ext_at = if is_dir { r.name.len() } else { r.name.len() - name::ext_len(&r.name) };
            let ext = &r.name[ext_at..];
            let folded = ext.to_ascii_lowercase();
            if !sl.grid && !is_dir && !self.type_names.contains_key(&folded) && !type_requests.contains(&folded) {
                type_requests.push(folded);
            }
            let entry = Entry { size: r.meta.size, modified: r.meta.modified, created: r.meta.created, accessed: 0, name_off: 0, attrs: r.meta.attrs, name_len: 0, ext_len: 0, tags: 0 };
            let (key, kind) = icon_key(&r.name, ext, &entry, icon_px as u32, &r.dir, self.icon_epoch);
            self.file_icons.see(key, kind);
            let indent = if headers { self.s(16.0) } else { 0.0 };
            let ir = if sl.grid {
                Rect::new((cell.x + (cell.w - icon_px) / 2.0).round(), y + self.s(14.0), icon_px, icon_px)
            } else {
                Rect::new(sl.cols[0].x + pad + indent, y + ((rh - icon_px) / 2.0).round(), icon_px, icon_px)
            };
            if !g.image(key, ir) {
                if !self.folder_stand_in(g, kind, icon_px as u32, ir, 255, &mut requests) {
                    let (glyph, color) = if is_dir { (G_FOLDER, t.accent.alpha(0xB0)) } else { (G_FILE, t.text_dim.alpha(0x90)) };
                    centered(g, ir, glyph, FontStyle::new(FontFamily::Icons, icon_px * 0.85, false), color);
                }
                let wanted = !self.icons.contains_key(&key) && !requests.iter().any(|q| q.key == key);
                if let Some(path) = wanted.then(|| Location::parse_exact(&r.dir)).flatten().and_then(|d| d.join(&r.name).to_windows()) {
                    requests.push(IconRequest { key, kind, path, size: icon_px as u32, gen: 0, row: 0 });
                }
            }
            self.draw_shell_overlay(g, kind, &entry, &r.dir, &r.name, ir, 255);
            self.draw_file_emblems(g, entry.attrs, ir, 255, &mut requests);
            let color = if r.meta.attrs & attr::HIDDEN != 0 { t.text_dim } else { t.text };
            if sl.grid {
                let shown = name::display(&r.name);
                let room = (cell.w - pad * 2.0).max(0.0);
                let width = g.measure(&shown, font);
                if width > room {
                    elided.push(row);
                }
                g.text(cell.x + (cell.w - width.min(room)) / 2.0, ir.bottom() + self.s(6.0) + m.ascent, &shown, font, color, room);
                continue;
            }
            let ty = y + text_dy;
            let name_x = ir.right() + self.s(8.0);
            let (shown, room) = (name::display(&r.name), sl.cols[0].right() - name_x - pad);
            if g.measure(&shown, font) > room {
                elided.push(row);
            }
            g.text(name_x, ty, &shown, font, color, room);
            let run_start = row == 0 || s.row(row - 1).is_none_or(|p| p.dir != r.dir);
            if !headers && (!s.group_folder || run_start) {
                let folder_color = if s.group_folder { t.text } else { t.text_dim };
                g.text(sl.cols[1].x + pad, ty, &name::display(&r.dir), font, folder_color, sl.cols[1].w - 2.0 * pad);
            }
            if !headers && s.group_folder && run_start && line > first {
                g.dl.rect(Rect::new(sl.list.x + self.s(4.0), y, sl.list.w - self.s(8.0), 1.0), t.separator);
            }
            if !is_dir {
                let mut sz = bumpalo::collections::String::new_in(arena);
                format::size_in(&mut sz, r.meta.size, self.style.decimal);
                let w = g.measure(&sz, font);
                g.text((sl.cols[2].right() - pad - w).max(sl.cols[2].x + pad), ty, &sz, font, t.text_dim, sl.cols[2].w - 2.0 * pad);
            }
            let mut d = bumpalo::collections::String::new_in(arena);
            format::datetime_in(&mut d, r.meta.modified, self.utc_offset_min, &self.style);
            g.text(sl.cols[3].x + pad, ty, &d, font, t.text_dim, sl.cols[3].w - 2.0 * pad);
        }
        g.dl.set_clip(None);
        self.search.elided = elided;
        self.request_icons(requests);
        for ext in type_requests {
            self.type_names.insert(ext.clone(), None);
            self.effects.push(Effect::TypeName(ext));
        }
        let s = &self.search;
        if sl.thumb.h > 0.0 {
            let hot = matches!(s.hover, SHit::Thumb | SHit::Track) || matches!(s.drag, SDrag::Thumb(_));
            g.dl.round_rect(sl.thumb, if hot { t.scrollbar_hover } else { t.scrollbar }, sl.thumb.w / 2.0);
        }
        if sl.max_x > 0.0 {
            g.dl.rect(sl.htrack, t.bg);
            g.dl.round_rect(sl.hthumb, if matches!(s.hover, SHit::HThumb) || matches!(s.drag, SDrag::HThumb(_)) { t.scrollbar_hover } else { t.scrollbar }, sl.hthumb.h / 2.0);
        }
        let head_from = g.dl.instances.len();
        g.dl.rect(sl.header, t.header);
        g.dl.rect(Rect::new(0.0, sl.header.bottom() - 1.0, sl.header.w, 1.0), t.separator);
        let hfont = self.ui_font(12.0);
        let hm = g.metrics(hfont);
        let baseline = (sl.header.y + (sl.header.h + hm.ascent - hm.descent) / 2.0).round();
        for (i, (title, key)) in COLUMNS.iter().enumerate() {
            let r = sl.cols[i];
            if s.hover == SHit::Header(i) {
                g.dl.rect(r, t.button_hover);
            }
            if i > 0 {
                g.dl.rect(Rect::new(r.x, r.y + self.s(6.0), 1.0, r.h - self.s(12.0)), t.separator);
            }
            g.dl.set_clip(Some(r));
            let w = g.text(r.x + pad, baseline, title.t(), hfont, t.text_dim, r.w - 2.0 * pad);
            if s.sort.key == *key {
                let glyph = if s.sort.descending { G_SORT_DOWN } else { G_SORT_UP };
                g.text(r.x + pad + w + self.s(6.0), baseline, glyph, self.icon_font(8.0), t.text_dim, f32::INFINITY);
            }
            g.dl.set_clip(None);
        }
        g.dl.confine(head_from, sl.header);
    }

    fn draw_search_group<B: TextBackend>(&self, g: &mut Gfx<B>, arena: &Bump, t: &Theme, k: usize, r: Rect, requests: &mut Vec<IconRequest>) {
        let s = &self.search;
        let font = FontStyle::new(FontFamily::Ui, 12.5 * self.scale, true);
        let m = g.metrics(font);
        let baseline = (r.y + (r.h + m.ascent - m.descent) / 2.0).round();
        if s.hover == SHit::Group(k) {
            g.dl.round_rect(r.inset(self.s(4.0), 1.0), t.hover, self.s(4.0));
        }
        let icon = Rect::new(r.x + self.s(10.0), r.y, self.s(16.0), r.h);
        let folder = s.row(s.groups[k]).and_then(|row| Location::parse_exact(&row.dir));
        let system = folder.and_then(|loc| folder_icon(&loc, &self.nav.sidebar, icon.w as u32));
        if !self.system_icon(g, system, icon, icon.w, 0xFF, requests) {
            centered(g, icon, G_FOLDER, self.icon_font(11.0), t.accent.alpha(0xC0));
        }
        let dir = s.row(s.groups[k]).map(|row| name::display(&row.dir).into_owned()).unwrap_or_else(|| "…".into());
        let x = icon.right() + self.s(8.0);
        let mut count = bumpalo::collections::String::new_in(arena);
        let n = s.group_len(k);
        trn_into!(&mut count, P::DetailsCountItems, n);
        let small = self.ui_font(12.0);
        let cw = g.measure(&count, small);
        let w = g.text(x, baseline, &dir, font, t.text, (r.right() - x - cw - self.s(32.0)).max(0.0));
        g.text(x + w + self.s(12.0), baseline, &count, small, t.text_dim, f32::INFINITY);
        g.dl.rect(Rect::new(x + w + cw + self.s(24.0), (r.y + r.h / 2.0).round(), (r.right() - x - w - cw - self.s(32.0)).max(0.0), 1.0), t.separator);
    }

    pub(in crate::ui) fn draw_search_bar<B: TextBackend>(&mut self, g: &mut Gfx<B>, arena: &Bump, t: &Theme) {
        let l = &self.layout;
        let (toolbar, back, status) = (l.toolbar, l.nav[0], l.status);
        g.dl.rect(toolbar, t.bg);
        if self.search.hover == SHit::Back {
            g.dl.round_rect(back.inset(self.s(2.0), 0.0), t.button_hover, self.s(4.0));
        }
        centered(g, back, G_BACK, self.icon_font(12.0), t.text);
        self.draw_search_field(g, t);
        let tools = self.search_layout().tools;
        for (i, (hit, icon, selected)) in [(SHit::Folders, "\u{E8B7}", self.search.folders_open), (SHit::WithinResults, G_SEARCH, self.search.within_results.is_some()), (SHit::Facets, G_FACETS, self.search.facets_open)].into_iter().enumerate() {
            let r = tools[i];
            if selected || self.search.hover == hit {
                g.dl.round_rect(r.inset(self.s(1.0), self.s(1.0)), if selected { t.selection_inactive } else { t.button_hover }, self.s(4.0));
            }
            let color = if hit == SHit::Facets && self.search.facets_open && self.search.has_facet_filters() { t.accent } else { t.text };
            centered(g, r, icon, self.icon_font(12.0), color);
        }

        g.dl.rect(status, t.chrome);
        let font = self.ui_font(12.0);
        let m = g.metrics(font);
        let baseline = (status.y + (status.h + m.ascent - m.descent) / 2.0).round();
        let pad = self.s(12.0);
        let s = &self.search;
        let mut left = bumpalo::collections::String::new_in(arena);
        let mut color = t.text_dim;
        if let Some(e) = &s.error {
            left.push_str(&crate::i18n::localize(e));
            color = t.error;
        } else if s.walking {
            if s.folders > 0 {
                trn_into!(&mut left, P::SearchWalkingIn, s.folders, results = trn!(P::SearchResults, s.total));
            } else {
                trn_into!(&mut left, P::SearchWalking, s.total);
            }
        } else if s.done {
            let results = trn!(P::SearchResults, s.total);
            match s.elapsed {
                Some(e) => {
                    let mut ms = String::new();
                    i18n::decimal(&mut ms, e.as_secs_f64() * 1e3, 1);
                    tr_into!(&mut left, S::SearchDoneIn, results = results, ms = ms);
                }
                None => left.push_str(&results),
            }
            if s.folders > 0 {
                left.push_str(" · ");
                trn_into!(&mut left, P::SearchFolders, s.folders);
            }
        } else {
            left.push_str(S::SearchSearching.t());
        }
        if let Some(label) = match s.hover {
            SHit::Folders => Some(S::SearchExploreFolders),
            SHit::WithinResults => Some(S::SearchExploreWithin),
            SHit::Facets => Some(S::SearchExploreFacets),
            _ => None,
        } {
            left.clear();
            left.push_str(label.t());
            color = t.text_dim;
        }
        g.text(status.x + pad, baseline, &left, font, color, status.w / 2.0 - pad);
        let mut right = bumpalo::collections::String::new_in(arena);
        let tagged = crate::tags::has_tag_term(&s.edit.text);
        right.push_str(match (s.source, s.backend) {
            (Some(SearchSource::Walk), _) => S::SearchSourceWalk,
            (Some(SearchSource::Shell), _) if self.caps.unix_paths => S::SearchSourceContent,
            (Some(SearchSource::Shell), _) => S::SearchSourceShell,
            _ if tagged => S::SearchSourceTagged,
            (None, SearchBackend::Connecting) => S::SearchSourceConnecting,
            (None, SearchBackend::Service) => S::SearchSourceService,
            (None, SearchBackend::Fallback) if self.caps.unix_paths => S::SearchSourceLocal,
            (None, SearchBackend::Fallback) => S::SearchSourceFallback,
            (None, SearchBackend::Off) => S::SearchSourceOff,
        }.t());
        for v in s.volumes.iter().filter(|_| !tagged && s.source.is_none()) {
            let label = if v.id.is_empty() { S::SearchIndexDefault.t() } else { &v.id };
            right.push_str(" · ");
            match &v.state {
                IndexState::Ready => trn_into!(&mut right, P::SearchIndexReady, v.files, label = label),
                IndexState::Scanning(p) => trn_into!(&mut right, P::SearchIndexScanning, v.files, label = label, percent = format!("{:.0}", p * 100.0)),
                IndexState::Error(e) => tr_into!(&mut right, S::SearchIndexError, label = label, error = e),
            }
        }
        let rcolor = if (matches!(s.backend, SearchBackend::Fallback) && !self.caps.unix_paths || matches!(s.backend, SearchBackend::Off)) && !tagged && s.source.is_none() { t.error } else { t.text_dim };
        let rw = g.measure(&right, font).min(status.w / 2.0 - pad);
        g.text(status.right() - pad - rw, baseline, &right, font, rcolor, status.w / 2.0 - pad);
        self.draw_recent(g, t);
    }

    fn draw_search_field<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme) {
        let r = self.search_layout().field;
        let radius = self.s(4.0);
        g.dl.round_rect(r, t.input_bg, radius);
        g.dl.outline(r, t.accent, radius, self.s(1.0));
        let icon = Rect::new(r.x + self.s(4.0), r.y, self.s(24.0), r.h);
        centered(g, icon, G_SEARCH, self.icon_font(11.0), t.text_dim);
        let font = self.ui_font(13.0);
        let small = self.ui_font(12.0);
        let m = g.metrics(font);
        let baseline = (r.y + (r.h + m.ascent - m.descent) / 2.0).round();
        let label = match &self.search.within {
            Some(w) => tr!(S::SearchIn, name = w.name()),
            None if self.search.home.is_some() => S::SearchAllDrives.t().to_owned(),
            None => String::new(),
        };
        let mut x = icon.right() + self.s(4.0);
        self.search.chip = Rect::default();
        if !label.is_empty() {
            let cw = (g.measure(&label, small) + self.s(30.0)).min(r.w / 3.0);
            let chip = Rect::new(x, r.y + self.s(4.0), cw, r.h - self.s(8.0));
            let hot = self.search.hover == SHit::Scope;
            g.dl.round_rect(chip, if hot { t.button_hover } else { t.selection_inactive }, self.s(3.0));
            let w = g.text(chip.x + self.s(8.0), baseline, &label, small, t.text, chip.w - self.s(24.0));
            g.text(chip.x + self.s(12.0) + w, baseline - self.s(1.0), G_CHEVRON, self.icon_font(7.0), t.text_dim, f32::INFINITY);
            self.search.chip = chip;
            x = chip.right() + self.s(8.0);
        }
        let inner = Rect::new(x, r.y, r.right() - x - self.s(12.0), r.h);
        g.dl.set_clip(Some(inner));
        let s = &mut self.search;
        if let Some((x, how)) = s.pending_click.take() {
            let pos = g.hit(&s.edit.text, font, x - inner.x + s.edit_scroll);
            s.edit.grab(pos, how);
        }
        let caret_h = (r.h * 0.6).round();
        let look = crate::ui::field::EditLook { font, baseline, line_y: r.y + (r.h - caret_h) / 2.0, line_h: caret_h, stroke: 1.0f32.max(self.scale.round()), focused: true };
        let preedit = Some(&self.preedit).filter(|p| !p.is_empty());
        if s.edit.text.is_empty() && preedit.is_none() {
            s.edit_scroll = 0.0;
            g.text(inner.x, baseline, if s.within_results.is_some() { S::SearchExploreWithin } else if self.caps.unix_paths { S::SearchPlaceholderUnix } else { S::SearchPlaceholder }.t(), font, t.text_dim.alpha(0xA0), inner.w);
            g.dl.rect(Rect::new(inner.x, look.line_y, look.stroke, caret_h), t.text);
        } else {
            let line = crate::ui::field::EditLine { text: &s.edit.text, caret: s.edit.caret, selection: s.edit.selection() };
            crate::ui::field::draw_line(g, t, inner, &mut s.edit_scroll, line, preedit, look);
        }
        g.dl.set_clip(None);
    }

    fn draw_recent<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme) {
        let recent = self.search.recent();
        self.search.recent_rects.clear();
        if recent.is_empty() {
            return;
        }
        let field = self.search_layout().field;
        let font = self.ui_font(13.0);
        let m = g.metrics(font);
        let (s4, s6, row_h) = (self.s(4.0), self.s(6.0), self.s(30.0));
        let w = field.w.min(self.s(560.0));
        let x = field.x;
        let y = field.bottom() + s4;
        let h = s6 * 2.0 + row_h * (recent.len() + 1) as f32;
        let panel = Rect::new(x, y, w, h);
        crate::ui::widgets::Paint::new(g, t, self.scale).frame(panel);
        let small = self.ui_font(12.0);
        let head = Rect::new(x + s6, y + s6, w - 2.0 * s6, row_h);
        g.text(head.x + self.s(10.0), (head.y + (head.h + m.ascent - m.descent) / 2.0).round(), S::SearchRecent.t(), small, t.text_dim, head.w);
        for (n, &k) in recent.iter().enumerate() {
            let r = Rect::new(x + s6, head.bottom() + n as f32 * row_h, w - 2.0 * s6, row_h);
            if self.search.recent_sel == Some(n) {
                g.dl.round_rect(r, t.selection, s4);
            } else if self.search.hover == SHit::Recent(n) {
                g.dl.round_rect(r, t.hover, s4);
            }
            centered(g, Rect::new(r.x + s4, r.y, self.s(24.0), r.h), G_HISTORY, self.icon_font(11.0), t.text_dim);
            let q = &self.search.history[k];
            g.text(r.x + self.s(34.0), (r.y + (r.h + m.ascent - m.descent) / 2.0).round(), q, font, t.text, r.w - self.s(44.0));
            self.search.recent_rects.push(r);
        }
    }

    pub(in crate::ui) fn search_ime_area(&self) -> Option<Rect> {
        self.search.active.then(|| self.search_layout().field)
    }
}
