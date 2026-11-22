use super::*;

impl App {
    pub(in crate::ui) fn draw_preview<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme) {
        let r = self.preview.r;
        self.preview.buttons.clear();
        if r.pane.w <= 0.0 || r.pane.h <= 0.0 {
            return;
        }
        g.dl.rect(r.pane, t.bg);
        let hot = matches!(self.hover, Hit::Preview(PaneHit::Split)) || matches!(self.drag, Drag::Preview { hit: PaneHit::Split, .. });
        let line = if self.right_side() { Rect::new(r.pane.x, r.pane.y, 1.0, r.pane.h) } else { Rect::new(r.pane.x, r.pane.y, r.pane.w, 1.0) };
        g.dl.rect(line, if hot { t.accent } else { t.separator });
        g.dl.set_clip(Some(r.body));
        self.preview.hmax = 0.0;
        let extent = self.draw_preview_body(g, t);
        g.dl.set_clip(None);
        let p = &mut self.preview;
        p.extent = extent;
        p.scroll = p.scroll.clamp(0.0, (extent - r.body.h).max(0.0));
        p.hscroll = p.hscroll.clamp(0.0, p.hmax);
        if !matches!(p.content, Content::Loading | Content::Empty | Content::Handler) {
            if let Some(t) = p.requested.take() {
                tracing::debug!("preview drawn {:.1} ms after selection, {} KB held", t.elapsed().as_secs_f64() * 1e3, p.held() / 1024);
            }
        }
        self.draw_inspector_tools(g, t);
        self.draw_inspector_foot(g, t);
        self.draw_card(g, t);
        self.place_host();
    }

    fn tool_button<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme, r: Rect, glyph: &str, hit: PaneHit, on: bool) {
        let state = self.button(Hit::Preview(hit), on, true);
        let font = if glyph.is_ascii() { self.ui_font(11.5) } else { self.icon_font(11.0) };
        Paint::new(g, t, self.scale).icon_button(r, glyph, font, state, t.text_dim);
        self.preview.buttons.push((r, hit));
    }

    fn toggle_tool<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme, r: Rect, glyph: &str, hit: PaneHit, on: bool) {
        let font = if glyph.is_ascii() { self.ui_font(11.5) } else { self.icon_font(11.0) };
        Paint::new(g, t, self.scale).toggle_button(r, glyph, font, on, self.lit(Hit::Preview(hit)));
        self.preview.buttons.push((r, hit));
    }

    fn draw_inspector_tools<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme) {
        let r = self.preview.r.tools;
        g.dl.rect(r, t.chrome);
        g.dl.rect(Rect::new(r.x, r.bottom() - 1.0, r.w, 1.0), t.separator);
        let bw = self.s(TOOL_W);
        let y = r.y + ((r.h - bw) / 2.0).round();
        let mut x = r.x + self.s(4.0);
        self.tool_button(g, t, Rect::new(x, y, bw, bw), G_CLOSE, PaneHit::Close, false);
        x += bw + self.s(8.0);
        let group = Rect::new(x - self.s(2.0), y - self.s(2.0), 3.0 * bw + self.s(4.0), bw + self.s(4.0));
        Paint::new(g, t, self.scale).dashed(group, t.input_border);
        for at in AT_CHOICES {
            let r = Rect::new(x, y, bw, bw);
            let hit = PaneHit::At(at);
            match at {
                InspectorAt::Auto => self.tool_button(g, t, r, G_AUTO, hit, self.preview.look.at == at),
                InspectorAt::Right | InspectorAt::Below => {
                    let state = self.button(Hit::Preview(hit), self.preview.look.at == at, true);
                    Paint::new(g, t, self.scale).pane_button(r, at == InspectorAt::Below, state, t.text_dim);
                    self.preview.buttons.push((r, hit));
                }
            }
            x += bw;
        }
        x += self.s(8.0);
        let look = self.preview.look;
        let mut tools: Vec<(&str, PaneHit, bool)> = match &self.preview.content {
            Content::Doc(d) => {
                let wrap = self.preview.effective_wrap(d);
                let mut v = vec![(G_WRAP, PaneHit::Tool(PaneTool::Wrap), wrap)];
                if d.mono {
                    v.push((G_LINES, PaneHit::Tool(PaneTool::Lines), look.lines));
                    v.push((G_SPACES, PaneHit::Tool(PaneTool::Spaces), look.spaces));
                }
                v
            }
            Content::Image => vec![
                (G_FIT, PaneHit::Tool(PaneTool::Scale(Scale::Fit)), look.scale == Scale::Fit),
                ("1:1", PaneHit::Tool(PaneTool::Scale(Scale::Original)), look.scale == Scale::Original),
                (G_CUSTOM, PaneHit::Tool(PaneTool::Scale(Scale::Custom)), look.scale == Scale::Custom),
                (G_GRID, PaneHit::Tool(PaneTool::Grid), look.grid),
            ],
            _ => Vec::new(),
        };
        if self.preview_is_svg() {
            tools.retain(|(_, hit, _)| *hit != PaneHit::Tool(PaneTool::Grid));
        }
        if !tools.is_empty() {
            g.dl.rect(Rect::new(x - self.s(4.0), y + self.s(5.0), 1.0, bw - self.s(10.0)), t.separator);
        }
        let image = matches!(self.preview.content, Content::Image);
        for (k, (glyph, hit, on)) in tools.into_iter().enumerate() {
            if image && k == 0 {
                let grp = Rect::new(x - self.s(2.0), y - self.s(2.0), 3.0 * bw + self.s(4.0), bw + self.s(4.0));
                Paint::new(g, t, self.scale).dashed(grp, t.input_border);
            }
            if image && k == 3 {
                x += self.s(8.0);
            }
            match image && k < 3 {
                true => self.tool_button(g, t, Rect::new(x, y, bw, bw), glyph, hit, on),
                false => self.toggle_tool(g, t, Rect::new(x, y, bw, bw), glyph, hit, on),
            }
            x += bw;
        }
        let title = self.preview.subject.as_ref().map(|s| s.title.clone()).unwrap_or_default();
        let font = self.ui_font(12.5);
        let room = (r.right() - x - self.s(16.0)).max(0.0);
        let tw = g.measure(&title, font).min((room - self.s(20.0)).max(0.0));
        let w = tw + self.s(22.0);
        let tr = Rect::new(r.right() - self.s(4.0) - w, y, w, bw);
        let open = self.preview.look.card;
        let state = self.button(Hit::Preview(PaneHit::Title), open, true);
        let p = Paint::new(g, t, self.scale);
        if let Some(fill) = p.fill_of(state) {
            p.g.dl.round_rect(tr, fill, self.s(4.0));
        }
        let ink = p.ink_of(state, t.text);
        let m = g.metrics(font);
        g.text(tr.x + self.s(6.0), (tr.y + (tr.h + m.ascent - m.descent) / 2.0).round(), &title, font, ink, tw);
        let now = self.last_frame.unwrap_or_else(std::time::Instant::now);
        self.motion |= self.preview.turn.aim(if open { 90.0 } else { 0.0 }, self.animations, now, crate::ui::address::TURN);
        let cr = Rect::new(tr.right() - self.s(16.0), tr.y, self.s(14.0), tr.h);
        g.dl.chevron((cr.x + cr.w / 2.0, cr.y + cr.h / 2.0), self.s(8.0), self.preview.turn.value(now, crate::ui::address::TURN), self.s(1.25).max(1.0), ink);
        self.preview.r.title = tr;
        self.preview.buttons.push((tr, PaneHit::Title));
    }

    fn foot_blocks(&self) -> (Vec<FootBlock>, Option<(S, usize)>) {
        match &self.preview.content {
            Content::Doc(d) => {
                let mut v = vec![("", self.settings.opts.inspector_size.to_string(), Some(PaneHit::Size)), (G_LINES, crate::ui::thousands(d.paras.len() as u64), None)];
                if self.pretty_available() {
                    let (label, hit) = match self.preview.pretty {
                        Pretty::Off => (S::PreviewPrettyButton, Some(PaneHit::Pretty)),
                        Pretty::Loading => (S::PreviewPrettyWorking, None),
                        Pretty::On(_) => (S::PreviewPrettyOriginal, Some(PaneHit::Pretty)),
                    };
                    v.insert(0, ("", label.t().to_owned(), hit));
                }
                if !d.encoding.is_empty() {
                    v.push(("", d.encoding.to_owned(), None));
                }
                (v, Some((S::PreviewFilterLines, self.preview.doc.as_ref().map_or(d.paras.len(), |l| l.shown))))
            }
            Content::Image => {
                let Some((w, h)) = self.preview.image.as_ref().map(|i| (i.width as f32, i.height as f32)) else { return (Vec::new(), None) };
                let b = self.preview.r.body;
                let k = image_factor(self.preview.look.scale, self.preview.look.zoom, (w, h), (b.w, b.h));
                let mut blocks = vec![("", format!("{}%", (k * 100.0).round()), Some(PaneHit::Size)), ("", format!("{} \u{D7} {}", w as u32, h as u32), None)];
                if self.preview_is_svg() {
                    blocks.insert(0, ("", self.settings.opts.svg_background.label().t().to_owned(), Some(PaneHit::SvgBackground)));
                }
                (blocks, None)
            }
            Content::Folder(f) => (vec![(crate::ui::G_FILE, crate::ui::thousands(f.files), None), (crate::ui::G_FOLDER, crate::ui::thousands(f.dirs), None)], Some((S::PreviewFilterFiles, self.folder_rows(f).len()))),
            _ => (Vec::new(), None),
        }
    }

    fn draw_inspector_foot<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme) {
        let slot = self.preview.r.foot;
        self.preview.r.filter = Rect::default();
        if slot.h <= 0.0 {
            return;
        }
        let r = Rect { h: self.s(FOOT_H), ..slot };
        let radius = self.s(6.0);
        let right_from = g.dl.instances.len();
        let font = self.ui_font(12.0);
        let icons = self.icon_font(11.0);
        let m = g.metrics(font);
        let baseline = (r.y + (r.h + m.ascent - m.descent) / 2.0).round();
        let mut right = r.right() - self.s(6.0);
        let block_h = r.h - self.s(4.0);
        let (blocks, filterable) = self.foot_blocks();
        for (glyph, label, hit) in blocks {
            let icon_w = if glyph.is_empty() { 0.0 } else { self.s(18.0) };
            let w = g.measure(&label, font) + self.s(14.0) + icon_w;
            if right - w < r.x + self.s(6.0) {
                continue;
            }
            let br = Rect::new(right - w, r.y + self.s(2.0), w, block_h);
            if let Some(h) = hit {
                g.dl.round_rect(br, t.input_bg, self.s(3.0));
                g.dl.round_rect(br, crate::ui::widgets::faded(t.button_hover, self.lit(Hit::Preview(h))), self.s(3.0));
                g.dl.round_rect(br, t.accent.alpha(0x40), self.s(3.0));
                self.preview.buttons.push((br, h));
            }
            if icon_w > 0.0 {
                centered(g, Rect::new(br.x + self.s(4.0), br.y, icon_w, br.h), glyph, icons, t.text_dim);
            }
            g.text(br.x + self.s(7.0) + icon_w, baseline, &label, font, if hit.is_some() { t.text } else { t.text_dim }, f32::INFINITY);
            right = br.x - self.s(6.0);
        }
        if g.dl.instances.len() > right_from {
            let piece = Rect::new(right, r.y, r.right() - right, r.h);
            g.dl.beneath(right_from, |dl| crate::ui::bottom::bar_piece(dl, t, r, piece, false, radius));
        }
        let Some((label, count)) = filterable else { return };
        let fr = Rect::new(r.x + self.s(6.0), r.y + self.s(2.0), (right - r.x - self.s(12.0)).clamp(0.0, self.s(260.0)), block_h);
        self.preview.r.filter = fr;
        let left_from = g.dl.instances.len();
        let focused = self.preview.filtering && self.focus == Focus::Preview;
        let mut p = Paint::new(g, t, self.scale);
        p.field(fr, focused);
        centered(g, Rect::new(fr.x + self.s(2.0), fr.y, self.s(18.0), fr.h), "\u{E721}", self.icon_font(10.0), t.text_dim);
        let clear_w = if self.preview.filter.is_some() { self.s(18.0) } else { 0.0 };
        let inner = Rect::new(fr.x + self.s(22.0), fr.y, (fr.w - self.s(26.0) - clear_w).max(0.0), fr.h);
        let placeholder = crate::tr!(label, n = crate::ui::thousands(count as u64));
        let preedit = focused.then_some(&self.preedit);
        let sc = self.scale;
        let field = self.preview.filter.get_or_insert_with(Field::default);
        field.draw(g, t, inner, font, focused, &placeholder, sc, preedit);
        if self.preview.filter.as_ref().is_some_and(|f| f.text().is_empty()) && !focused {
            self.preview.filter = None;
        }
        self.preview.buttons.push((fr, PaneHit::Filter));
        if clear_w > 0.0 {
            let cr = Rect::new(fr.right() - clear_w - self.s(2.0), fr.y, clear_w, fr.h);
            self.tool_button(g, t, cr, G_CLOSE, PaneHit::FilterClear, false);
        }
        let piece = Rect::new(r.x, r.y, fr.right() + self.s(6.0) - r.x, r.h);
        g.dl.beneath(left_from, |dl| crate::ui::bottom::bar_piece(dl, t, r, piece, true, radius));
    }

    fn draw_card<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme) {
        let card = self.preview.r.card;
        self.preview.chips.clear();
        self.preview.r.button = Rect::default();
        if card.w <= 0.0 || card.h <= 0.0 {
            return;
        }
        let Some(s) = self.preview.subject.clone() else { return };
        let head_h = self.s(30.0);
        let saved = (self.preview.r.body, self.preview.scroll);
        let inner = Rect::new(card.x + self.s(10.0), card.y + head_h, card.w - self.s(20.0), 0.0);
        self.preview.r.body = Rect { h: (card.h - head_h - self.s(8.0)).max(0.0), ..inner };
        self.preview.scroll = self.preview.card_scroll;
        let from = g.dl.instances.len();
        g.dl.set_clip(Some(self.preview.r.body));
        let extent = self.draw_details(g, t, &s);
        g.dl.set_clip(None);
        let used = (head_h + extent + self.s(10.0)).min(card.h);
        let shown = Rect { h: used, ..card };
        let body = self.preview.r.body;
        self.preview.r.body = saved.0;
        self.preview.scroll = saved.1;
        self.preview.card_extent = extent;
        self.preview.r.card = shown;
        let mut p = Paint::new(g, t, self.scale);
        let before = p.g.dl.instances.len();
        p.frame(shown);
        let behind = p.g.dl.instances.len() - before;
        p.g.dl.instances[from..].rotate_right(behind);
        let bold = FontStyle::new(FontFamily::Ui, 13.0 * self.scale, true);
        let m = g.metrics(bold);
        let bw = self.s(TOOL_W);
        let close = Rect::new(shown.right() - self.s(6.0) - bw, shown.y + (head_h - bw) / 2.0, bw, bw);
        let details = Rect { x: close.x - bw, ..close };
        g.text(shown.x + self.s(10.0), (shown.y + (head_h + m.ascent - m.descent) / 2.0).round(), &s.title, bold, t.text, details.x - shown.x - self.s(14.0));
        let on = self.preview.state.details;
        self.toggle_tool(g, t, details, if on { G_DETAILS_ON } else { G_DETAILS_OFF }, PaneHit::CardDetails, on);
        self.tool_button(g, t, close, G_BELOW, PaneHit::CardClose, false);
        g.dl.rect(Rect::new(shown.x + 1.0, shown.y + head_h - 1.0, shown.w - 2.0, 1.0), t.separator);
        let _ = body;
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

    pub(super) fn field<B: TextBackend>(&self, g: &mut Gfx<B>, y: &mut f32, label: &str, value: &str, t: &Theme) {
        let b = self.preview.r.body;
        let font = self.ui_font(12.5);
        let m = g.metrics(font);
        let h = self.s(ROW);
        let top = b.y + *y - self.preview.scroll;
        if top + h >= b.y && top <= b.bottom() {
            let base = (top + (h + m.ascent - m.descent) / 2.0).round();
            let lw = (b.w * 0.34).round().min(self.s(110.0));
            g.text(b.x, base, label, font, t.text, lw - self.s(8.0));
            g.text(b.x + lw, base, value, font, t.text_dim, b.w - lw);
        }
        *y += h;
    }

    fn type_label(&self, s: &Subject) -> String {
        let Some(e) = s.entry else { return S::DetailsTypeFolder.t().into() };
        if e.attrs & attr::VOLUME != 0 {
            return S::DetailsTypeDrive.t().into();
        }
        self.ext_type_label(&s.ext.to_ascii_lowercase(), e.is_dir())
    }

    fn draw_icon<B: TextBackend>(&mut self, g: &mut Gfx<B>, s: &Subject, r: Rect) {
        let e = s.entry.unwrap_or(Entry { size: 0, modified: 0, created: 0, accessed: 0, name_off: 0, attrs: attr::DIR, name_len: 0, ext_len: 0, tags: 0 });
        let name = s.path.as_ref().map(|p| p.file_name().to_vec()).unwrap_or_default();
        let (key, kind) = icon_key(&name, &s.ext, &e, r.w as u32, &s.dir_key, self.icon_epoch);
        self.file_icons.see(key, kind);
        let mut requests = Vec::new();
        if g.image(key, r) {
            self.draw_file_emblems(g, e.attrs, r, 255, &mut requests);
            self.request_icons(requests);
            return;
        }
        if !self.folder_stand_in(g, kind, r.w as u32, r, 255, &mut requests) {
            let glyph = if e.attrs & attr::VOLUME != 0 { crate::ui::G_DRIVE } else if e.is_dir() { crate::ui::G_FOLDER } else { crate::ui::G_FILE };
            centered(g, r, glyph, FontStyle::new(FontFamily::Icons, r.h * 0.8, false), self.theme().text_dim);
        }
        if let (false, Some(path)) = (self.icons.contains_key(&key), s.path.clone()) {
            requests.push(IconRequest { key, kind, path, size: r.w as u32, gen: 0, row: 0 });
        }
        self.draw_file_emblems(g, e.attrs, r, 255, &mut requests);
        self.request_icons(requests);
    }

    pub(super) fn header<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme, s: &Subject, y: &mut f32) {
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

    fn draw_details<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme, s: &Subject) -> f32 {
        let mut y = 0.0;
        let mut text = String::new();
        let (off, style) = (self.utc_offset_min, self.style.clone());
        let mut date = |v: i64| {
            text.clear();
            format::datetime_in(&mut text, v, off, &style);
            text.clone()
        };
        let mut rows: Vec<(&str, String)> = Vec::new();
        if let Some(p) = &s.path {
            rows.push((S::PreviewFieldPath.t(), p.display().into_owned()));
        }
        rows.push((S::PreviewFieldType.t(), if s.many.is_some() { String::new() } else { self.type_label(s) }));
        if let Some((n, bytes)) = s.many {
            rows.push((S::PreviewFieldItems.t(), crate::ui::thousands(n as u64)));
            rows.push((S::PreviewFieldTotalSize.t(), self.size_text(bytes)));
        }
        if let Some((total, free)) = s.drive {
            rows.push((S::PreviewFieldUsedSpace.t(), self.size_text(total.saturating_sub(free))));
            rows.push((S::PreviewFieldFreeSpace.t(), self.size_text(free)));
            rows.push((S::PreviewFieldTotalSize.t(), self.size_text(total)));
        }
        let details = self.preview.state.details;
        if let Some(e) = s.entry.filter(|e| e.attrs & attr::VOLUME == 0) {
            if !e.is_dir() || e.attrs & attr::SIZE_KNOWN != 0 {
                rows.push((S::DetailsColumnSize.t(), crate::trn!(P::PreviewFieldSizeBytes, e.size, size = self.size_text(e.size))));
            }
            if !details {
                if e.created > 0 {
                    rows.push((S::PreviewFieldCreated.t(), date(e.created)));
                }
                if e.modified > 0 {
                    rows.push((S::PreviewFieldModified.t(), date(e.modified)));
                }
            }
            let flags: Vec<&str> = [
                (attr::READONLY, S::PreviewAttrReadOnly),
                (attr::HIDDEN, S::PreviewAttrHidden),
                (attr::SYSTEM, S::PreviewAttrSystem),
                (attr::LINK, S::PreviewAttrLink),
                (attr::COMPRESSED, S::PreviewAttrCompressed),
                (attr::ENCRYPTED, S::PreviewAttrEncrypted),
                (attr::ONLINE_ONLY, S::PreviewAttrOnlineOnly),
                (attr::PINNED, S::PreviewAttrPinned),
                (attr::OFFLINE, S::PreviewAttrOffline),
            ]
            .iter()
            .filter(|f| e.attrs & f.0 != 0)
            .map(|f| f.1.t())
            .collect();
            if !flags.is_empty() {
                rows.push((S::PreviewFieldAttributes.t(), flags.join(", ")));
            }
        }
        rows.retain(|r| !r.1.is_empty());
        for (label, value) in &rows {
            self.field(g, &mut y, label, value, t);
        }
        self.tag_row(g, s, &mut y, t);
        if let (Content::Folder(f), true) = (&self.preview.content, s.many.is_none()) {
            y += self.s(6.0);
            self.folder_lines(g, t, f, &mut y);
        }
        if details {
            let b = self.preview.r.body;
            let top = b.y + y + self.s(4.0) - self.preview.scroll;
            g.dl.rect(Rect::new(b.x, top, b.w, 1.0), t.separator);
            y += self.s(8.0);
            let groups = self.preview.props.clone();
            if self.preview.props_loading {
                self.text_line(g, &mut y, S::PreviewReading.t(), self.ui_font(12.5), t.text_dim);
            }
            let bold = FontStyle::new(FontFamily::Ui, 12.5 * self.scale, true);
            for grp in &groups {
                self.text_line(g, &mut y, &grp.title, bold, t.text);
                for (k, v) in &grp.rows {
                    self.field(g, &mut y, k, v, t);
                }
                y += self.s(4.0);
            }
        }
        if s.entry.is_some() || s.many.is_some() {
            let b = self.preview.r.body;
            y += self.s(6.0);
            let br = Rect::new(b.x, b.y + y - self.preview.scroll, self.s(110.0).min(b.w), self.s(BUTTON_H));
            g.dl.round_rect(br, t.input_bg, self.s(4.0));
            g.dl.round_rect(br, crate::ui::widgets::faded(t.button_hover, self.lit(Hit::Preview(PaneHit::Properties))), self.s(4.0));
            g.dl.outline(br, t.input_border, self.s(4.0), 1.0);
            centered(g, br, S::PreviewProperties.t(), self.ui_font(12.5), t.text);
            self.preview.r.button = br;
            y += self.s(BUTTON_H);
        }
        y
    }

    fn tag_row<B: TextBackend>(&mut self, g: &mut Gfx<B>, s: &Subject, y: &mut f32, t: &Theme) {
        if (s.entry.is_none() && s.many.is_none()) || !self.taggable_here() {
            return;
        }
        let b = self.preview.r.body;
        let (font, label) = (self.ui_font(12.0), self.ui_font(12.5));
        let (lm, fm) = (g.metrics(label), g.metrics(font));
        let (h, pad, dot, gap) = (self.s(22.0), self.s(8.0), self.s(8.0), self.s(6.0));
        let lw = (b.w * 0.34).round().min(self.s(110.0));
        let top = b.y + *y - self.preview.scroll;
        g.text(b.x, (top + (self.s(ROW) + lm.ascent - lm.descent) / 2.0).round(), S::DetailsColumnTag.t(), label, t.text, lw - self.s(8.0));
        let (any, all) = self.selection_tags();
        let chips: Vec<(PaneHit, String, Option<Color>)> = self.tag_defs().iter().enumerate().map(|(k, d)| (PaneHit::Tag(k), d.name.clone(), Some(Color::rgb(d.color)))).chain([(PaneHit::NewTag, S::PreviewNewTag.t().to_owned(), None)]).collect();
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
        if s.many.is_some() {
            return self.draw_types(g, t);
        }
        match &self.preview.content {
            Content::Doc(_) => return self.draw_doc(g, t),
            Content::Pdf(_) => return self.draw_pdf(g, t),
            Content::Image => {
                if let Some((w, h)) = dims {
                    return self.draw_image(g, t, w, h);
                }
            }
            Content::Handler => {
                self.text_line(g, &mut y, S::PreviewLoading.t(), small, t.text_dim);
                return y;
            }
            &Content::Media { video } => return self.draw_media(g, t, &s, video),
            Content::Folder(f) => {
                let f = f.clone();
                return self.draw_folder(g, t, &f);
            }
            Content::Loading if s.kind != Kind::Folder && s.entry.is_some() => {
                self.text_line(g, &mut y, S::UiStatusLoading.t(), small, t.text_dim);
                return y;
            }
            _ => {}
        }
        let note = match &self.preview.content {
            Content::Basic(n) => n.as_ref().map(Note::text),
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
        if let Some((total, free)) = s.drive {
            let bar = Rect::new(b.x, b.y + y - self.preview.scroll, b.w, self.s(8.0));
            g.dl.round_rect(bar, t.input_bg, self.s(4.0));
            let used = if total > 0 { (total - free.min(total)) as f32 / total as f32 } else { 0.0 };
            g.dl.round_rect(Rect { w: (bar.w * used).round(), ..bar }, if used > 0.9 { t.error } else { t.accent }, self.s(4.0));
            y += self.s(16.0);
            rows.push((S::PreviewFieldFree.t(), crate::tr!(S::PreviewFieldFreeOf, free = self.size_text(free), total = self.size_text(total))));
        }
        if let Some(e) = s.entry.filter(|e| !e.is_dir()) {
            rows.push((S::DetailsColumnSize.t(), self.size_text(e.size)));
            let mut d = String::new();
            format::datetime_in(&mut d, e.modified, self.utc_offset_min, &self.style);
            rows.push((S::PreviewFieldModified.t(), d));
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
        if matches!(self.preview.content, Content::Basic(Some(Note::OnlineOnly))) {
            y += self.s(8.0);
            let label = S::PreviewDownload.t();
            let w = (g.measure(label, small) + self.s(28.0)).min(b.w).round();
            let r = Rect::new(b.x, (b.y + y - self.preview.scroll).round(), w, self.s(BUTTON_H));
            let hot = matches!(self.hover, Hit::Preview(PaneHit::Download)).then_some(t.accent);
            Paint::new(g, t, self.scale).button(r, label, small, hot);
            self.preview.buttons.push((r, PaneHit::Download));
            y += r.h;
        }
        y
    }

    fn draw_types<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme) -> f32 {
        let b = self.preview.r.body;
        let font = self.ui_font(12.5);
        let m = g.metrics(font);
        let h = self.s(ROW + 4.0);
        let icons = self.icon_font(12.0);
        let mut y = 0.0;
        for (label, n) in self.preview.types.clone() {
            let top = b.y + y - self.preview.scroll;
            if top + h >= b.y && top <= b.bottom() {
                let base = (top + (h + m.ascent - m.descent) / 2.0).round();
                let dir = label == S::DetailsTypeFolder.t();
                centered(g, Rect::new(b.x, top, self.s(18.0), h), if dir { crate::ui::G_FOLDER } else { crate::ui::G_FILE }, icons, if dir { t.accent } else { t.text_dim });
                let count = crate::ui::thousands(n as u64);
                let cw = g.measure(&count, font);
                g.text(b.x + self.s(26.0), base, &label, font, t.text, b.w - self.s(34.0) - cw);
                g.text(b.right() - cw, base, &count, font, t.text_dim, f32::INFINITY);
            }
            y += h;
        }
        y
    }

    pub(super) fn preview_is_svg(&self) -> bool {
        self.preview.subject.as_ref().is_some_and(|s| preview::is_svg(&s.ext))
    }

    pub(super) fn draw_image<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme, w: f32, h: f32) -> f32 {
        use preview::SvgBackground as Bg;
        let b = self.preview.r.body;
        let look = self.preview.look;
        let k = image_factor(look.scale, look.zoom, (w, h), (b.w, b.h));
        let (dw, dh) = ((w * k).round(), (h * k).round());
        let y0 = if dh <= b.h { (b.y + (b.h - dh) / 2.0).round() } else { b.y - self.preview.scroll };
        self.preview.hmax = (dw - b.w).max(0.0);
        let x0 = if dw <= b.w { (b.x + (b.w - dw) / 2.0).round() } else { (b.x - self.preview.hscroll.min(self.preview.hmax)).round() };
        let r = Rect::new(x0, y0, dw, dh);
        let background = if self.preview_is_svg() { Some(self.settings.opts.svg_background) } else { look.grid.then_some(Bg::Checker) };
        if let Some(Bg::Black | Bg::White) = background {
            g.dl.rect(r, if background == Some(Bg::Black) { theme::BLACK } else { theme::WHITE });
        }
        if background == Some(Bg::Checker) {
            let cell = self.s(CHECKER).max(4.0);
            let visible = Rect::new(r.x.max(b.x), r.y.max(b.y), (r.right().min(b.right()) - r.x.max(b.x)).max(0.0), (r.bottom().min(b.bottom()) - r.y.max(b.y)).max(0.0));
            let (light, dark) = theme::preview_checker(self.dark);
            g.dl.rect(visible, light);
            let (cols, rows) = ((visible.w / cell).ceil() as usize, (visible.h / cell).ceil() as usize);
            for row in 0..rows {
                for col in (row % 2..cols).step_by(2) {
                    let x = visible.x + col as f32 * cell;
                    let yy = visible.y + row as f32 * cell;
                    g.dl.rect(Rect::new(x, yy, cell.min(visible.right() - x), cell.min(visible.bottom() - yy)), dark);
                }
            }
        }
        g.dl.preview(r);
        let _ = t;
        dh
    }

    fn folder_rows<'a>(&self, f: &'a FolderSummary) -> Vec<&'a preview::FolderItem> {
        let q = self.preview.query();
        f.entries.iter().filter(|e| q.is_empty() || e.name.to_lowercase().contains(&q)).collect()
    }

    fn draw_folder<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme, f: &FolderSummary) -> f32 {
        let b = self.preview.r.body;
        let font = self.ui_font(12.5);
        let m = g.metrics(font);
        let h = self.s(ROW + 2.0);
        let icons = self.icon_font(12.0);
        let (size_w, date_w) = (self.s(80.0), if b.w > self.s(360.0) { self.s(130.0) } else { 0.0 });
        let name_w = (b.w - size_w - date_w).max(0.0);
        let head = Rect::new(b.x, b.y, b.w, self.s(24.0));
        let rows = self.folder_rows(f);
        let mut y = head.h;
        let mut text = String::new();
        for e in &rows {
            let top = b.y + y - self.preview.scroll;
            if top + h >= head.bottom() && top <= b.bottom() {
                let base = (top + (h + m.ascent - m.descent) / 2.0).round();
                centered(g, Rect::new(b.x, top, self.s(18.0), h), if e.dir { crate::ui::G_FOLDER } else { crate::ui::G_FILE }, icons, if e.dir { t.accent } else { t.text_dim });
                g.text(b.x + self.s(24.0), base, &e.name, font, t.text, name_w - self.s(30.0));
                if date_w > 0.0 && e.modified > 0 {
                    text.clear();
                    format::datetime_in(&mut text, e.modified, self.utc_offset_min, &self.style);
                    g.text(b.x + name_w, base, &text, font, t.text_dim, date_w - self.s(8.0));
                }
                if !e.dir {
                    let s = self.size_text(e.size);
                    let sw = g.measure(&s, font);
                    g.text(b.right() - sw, base, &s, font, t.text_dim, f32::INFINITY);
                }
            }
            y += h;
        }
        g.dl.rect(head, t.bg);
        let hm = g.metrics(self.ui_font(12.0));
        let hb = (head.y + (head.h + hm.ascent - hm.descent) / 2.0).round();
        let hf = self.ui_font(12.0);
        g.text(b.x + self.s(24.0), hb, S::DetailsColumnName.t(), hf, t.text_dim, name_w);
        if date_w > 0.0 {
            g.text(b.x + name_w, hb, S::DetailsColumnModified.t(), hf, t.text_dim, date_w);
        }
        let sl = S::DetailsColumnSize.t();
        let sw = g.measure(sl, hf);
        g.text(b.right() - sw, hb, sl, hf, t.text_dim, f32::INFINITY);
        g.dl.rect(Rect::new(b.x, head.bottom() - 1.0, b.w, 1.0), t.separator);
        if rows.is_empty() {
            self.text_line(g, &mut y, S::PreviewFolderEmpty.t(), font, t.text_dim);
        }
        y
    }

    fn folder_lines<B: TextBackend>(&self, g: &mut Gfx<B>, t: &Theme, f: &FolderSummary, y: &mut f32) {
        let mut lines = vec![(format!("{}, {}", crate::trn!(P::DetailsCountFolders, f.dirs), crate::trn!(P::DetailsCountFiles, f.files)), t.text)];
        let mut size = self.size_text(f.bytes);
        if !f.complete {
            size = crate::trn!(P::PreviewFolderScanned, preview::FOLDER_SCAN, size = size);
        }
        lines.push((size, t.text));
        if f.hidden > 0 {
            lines.push((crate::trn!(P::PreviewFolderHidden, f.hidden), t.text_dim));
        }
        if f.newest > 0 {
            let mut d = String::new();
            format::datetime_in(&mut d, f.newest, self.utc_offset_min, &self.style);
            lines.push((crate::tr!(S::PreviewFolderLastChange, date = d), t.text_dim));
        }
        if !f.types.is_empty() {
            let types: Vec<String> = f.types.iter().map(|(e, n)| format!("{} {}", if e.is_empty() { S::PreviewFolderNoExtension.t() } else { e }, crate::ui::thousands(*n))).collect();
            lines.push((crate::tr!(S::PreviewFolderMostCommon, types = types.join(", ")), t.text_dim));
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
        let lit = self.lit(Hit::Preview(PaneHit::Play));
        if lit > 0.0 {
            g.dl.round_rect(play, crate::ui::widgets::faded(t.button_hover, lit), self.s(4.0));
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

    pub(super) fn draw_doc<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme) -> f32 {
        let (sc, dark, mono) = (self.scale, self.dark, crate::ui::options::mono_px(self.settings.opts.inspector_size));
        let b = self.preview.r.body;
        let note_font = self.ui_font(12.0);
        let num_font = FontStyle::new(FontFamily::Mono, mono * sc, false);
        let query = self.preview.query();
        let look = self.preview.look;
        let ink = if self.focus == Focus::Preview { t.text_selection } else { t.selection_inactive };
        let p = &mut self.preview;
        let (from, to) = p.pick.range();
        let Content::Doc(d) = &p.content else { return 0.0 };
        let wrap = p.effective_wrap(d);
        let numbers = d.mono && look.lines;
        let digits = (d.paras.len().max(1).ilog10() + 1) as usize;
        let gutter = if numbers { char_w(g, &mut p.widths, '0', num_font) * digits as f32 + (12.0 * sc).round() } else { 0.0 };
        let width = (b.w - gutter).max(0.0);
        let key = hash::hash(&(width.to_bits(), sc.to_bits(), mono.to_bits(), wrap, &query));
        if p.doc.as_ref().is_none_or(|l| l.key != key) {
            let mut lay = layout_doc(g, d, width, (sc, mono), &mut p.widths, wrap, &query);
            lay.key = key;
            p.doc = Some(lay);
        }
        p.r.gutter = gutter;
        let Some(lay) = &p.doc else { return 0.0 };
        let scroll = p.scroll;
        let first = lay.lines.partition_point(|l| l.y + l.h < scroll);
        let last = lay.lines.partition_point(|l| b.y + l.y - scroll <= b.bottom()).max(first);
        let x0 = b.x + gutter;
        let dot_color = t.text_dim.alpha(0x70);
        p.hmax = if wrap { 0.0 } else { (lay.wide - width).max(0.0) };
        let shift = p.hscroll.min(p.hmax).round();
        Grid::paint(&lay.grids, g, t, b, scroll);
        for line in &lay.lines[first..last] {
            let top = b.y + line.y - scroll;
            match line.block {
                Block::Code if !d.mono => g.dl.rect(Rect::new(x0, top, width, line.h), t.input_bg),
                Block::Rule => g.dl.rect(Rect::new(x0, top + (line.h / 2.0).round(), width, 1.0), t.separator),
                _ => {}
            }
            let depth = d.paras[line.para as usize].quote_depth;
            if depth > 0 {
                let step = (14.0 * sc).min(width * 0.5 / f32::from(depth));
                for level in 0..depth {
                    g.dl.rect(Rect::new(x0 + step * f32::from(level) + (2.0 * sc).min(step * 0.2), top, (3.0 * sc).min(step * 0.4), line.h), t.separator);
                }
            }
            if numbers && line.num > 0 {
                let n = line.num.to_string();
                let nw = g.measure(&n, num_font);
                g.text(x0 - (8.0 * sc).round() - nw, top + line.base, &n, num_font, t.text_dim.alpha(0xA0), f32::INFINITY);
            }
        }
        if shift > 0.0 {
            g.dl.set_clip(Some(Rect::new(x0, b.y, width, b.h)));
        }
        for (k, line) in lay.lines.iter().enumerate().take(last).skip(first) {
            let top = b.y + line.y - scroll;
            if from != to && (from.para..=to.para).contains(&line.para) {
                let lo = if line.para == from.para { from.at } else { 0 };
                let hi = if line.para == to.para { to.at } else { u32::MAX };
                let ends = line.para < to.para && lay.lines.get(k + 1).is_none_or(|n| n.para != line.para);
                let span = line_span_x(d, lay, line, &p.widths, (sc, mono), (lo, hi));
                let span = match (span, ends) {
                    (Some((a, z)), true) => Some((a, z + (line.h * 0.3).round())),
                    (None, true) => Some(line_end_x(d, lay, line, &p.widths, (sc, mono))).map(|e| (e, e + (line.h * 0.3).round())),
                    (span, false) => span,
                };
                if let Some((a, z)) = span {
                    g.dl.rect(Rect::new(x0 + a - shift, top, z - a, line.h), ink);
                }
            }
            let grid = if matches!(line.block, Block::Table(_)) { Grid::at(&lay.grids, line.y) } else { None };
            for piece in &lay.pieces[line.pieces.start as usize..line.pieces.end as usize] {
                let instance = g.dl.instances.len();
                let run = &d.runs[piece.run as usize];
                let font = run_font(run, line.block, sc, mono);
                let s = &d.text[piece.start as usize..piece.end as usize];
                let px = x0 + piece.x - shift;
                let color = tone_color(t, dark, run.tone);
                g.text(px, top + line.base, s, font, color, f32::INFINITY);
                if run.strike {
                    let w = g.measure(s, font);
                    let y = top + line.base - (g.metrics(font).ascent * 0.35).round();
                    g.dl.rect(Rect::new(px, y, w, sc.max(1.0)), color);
                }
                if look.spaces && s.contains(' ') {
                    let mut x = px;
                    for c in s.chars() {
                        let cw = char_w(g, &mut p.widths, c, font);
                        if c == ' ' {
                            let dw = char_w(g, &mut p.widths, '\u{B7}', font);
                            g.text(x + ((cw - dw) / 2.0).round(), top + line.base, SPACE_DOT, font, dot_color, f32::INFINITY);
                        }
                        x += cw;
                    }
                }
                if let Some(grid) = grid {
                    g.dl.confine(instance, grid.cell_clip(b, piece.x));
                }
            }
        }
        if shift > 0.0 {
            g.dl.set_clip(Some(b));
        }
        let mut h = lay.height;
        if d.truncated {
            let m = g.metrics(note_font);
            let top = b.y + h - scroll + (8.0 * sc).round();
            g.text(x0, top + m.ascent, &crate::trn!(P::PreviewTruncated, preview::TEXT_CHARS), note_font, t.text_dim, width);
            h += (8.0 * sc).round() + m.ascent + m.descent + (4.0 * sc).round();
        }
        h
    }

    fn size_text(&self, bytes: u64) -> String {
        let mut s = String::new();
        format::size_in(&mut s, bytes, self.style.decimal);
        s
    }
}
