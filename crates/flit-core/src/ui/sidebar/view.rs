use super::*;

impl App {
    pub(in crate::ui) fn layout_sidebar(&mut self, top: f32, bottom: f32) {
        let width = self.sidebar_width();
        let now = self.last_frame.unwrap_or_else(Instant::now);
        let sc = self.scale;
        let s = |v: f32| (v * sc).round();
        let sb = &mut self.nav.sidebar;
        let compact = sb.prefs.compact;
        sb.rect = Rect::new(0.0, top, width, (bottom - top).max(0.0));
        sb.splitter = if width > 0.0 && !compact { Rect::new(width - s(3.0), top, s(6.0), sb.rect.h) } else { Rect::default() };
        sb.rects.clear();
        sb.headers.clear();
        (sb.bar, sb.field, sb.clear) = (Rect::default(), Rect::default(), Rect::default());
        if width <= 0.0 {
            sb.list = Rect::default();
            return;
        }
        let (x, w) = if compact { (s(4.0), (width - s(8.0)).max(0.0)) } else { (s(6.0), (width - s(12.0)).max(0.0)) };
        let (header_h, row_h) = if compact { (s(COMPACT_HEADER_H), s(COMPACT_ROW_H)) } else { (s(HEADER_H), s(ROW_H)) };
        if !compact && sb.typing {
            sb.bar = Rect::new(0.0, top, width - 1.0, s(TOOL_H));
            sb.field = Rect::new(x, top + s(5.0), w, s(TOOL_H - 10.0));
            let c = s(22.0);
            sb.clear = if sb.filtering() { Rect::new(sb.field.right() - c - s(4.0), sb.field.y + ((sb.field.h - c) / 2.0).round(), c, c) } else { Rect::default() };
        }
        let list_top = sb.bar.bottom().max(top);
        sb.list = Rect::new(0.0, list_top, width - 1.0, (sb.rect.bottom() - list_top).max(0.0));
        let fold = sb.fold.map(|f| (f.section, f.shown(now)));
        let gap_h = |l: &Leaving| (l.h * (1.0 - eased(l.since, now, FOLD))).round();
        sb.fold_clip = None;
        sb.gaps.clear();
        let mut y = s(4.0);
        let mut it = 0;
        for section in sb.shown_sections() {
            sb.headers.push((Rect::new(x, y, w, header_h), section));
            y += header_h;
            let start = y;
            loop {
                if section == Section::Bookmarks {
                    for (k, l) in sb.leaving.iter().enumerate().filter(|(_, l)| l.index == it) {
                        let h = gap_h(l);
                        sb.gaps.push((Rect::new(x, y, w, h), k));
                        y += h;
                    }
                }
                if !sb.items.get(it).is_some_and(|i| i.section == section) {
                    break;
                }
                let h = if sb.items[it].usage.is_some() && !compact { s(DRIVE_H) } else { row_h };
                sb.rects.push(Rect::new(x, y, w, h));
                y += h + if compact { 0.0 } else { s(ROW_GAP) };
                it += 1;
            }
            if let Some((_, shown)) = fold.filter(|f| f.0 == section) {
                let visible = ((y - start) * shown).round();
                sb.fold_clip = Some((section, Rect::new(x, start, w, visible)));
                y = start + visible;
            }
            y += if compact { s(2.0) } else { s(SECTION_GAP) };
        }
        sb.content_h = y;
        sb.scroll.clamp(sb.content_h - sb.list.h);
        let dy = list_top - sb.scroll.pos.round();
        for r in sb.rects.iter_mut().chain(sb.headers.iter_mut().map(|(r, _)| r)).chain(sb.fold_clip.iter_mut().map(|(_, r)| r)).chain(sb.gaps.iter_mut().map(|(r, _)| r)) {
            r.y += dy;
        }
    }

    pub(in crate::ui) fn sidebar_hit(&self, x: f32, y: f32) -> Option<Hit> {
        let sb = &self.nav.sidebar;
        if sb.rect.w <= 0.0 {
            return None;
        }
        if sb.splitter.contains(x, y) {
            return Some(Hit::Splitter);
        }
        if !sb.rect.contains(x, y) {
            return None;
        }
        if sb.clear.contains(x, y) {
            return Some(Hit::QuickClear);
        }
        if sb.field.contains(x, y) {
            return Some(Hit::QuickField);
        }
        if !sb.list.contains(x, y) {
            return Some(Hit::SideEmpty);
        }
        for (i, r) in sb.rects.iter().enumerate() {
            let folded = sb.fold_clip.is_some_and(|(s, c)| sb.items.get(i).is_some_and(|it| it.section == s) && !c.contains(x, y));
            if r.contains(x, y) && !folded {
                let button = !sb.prefs.compact && sb.pin_rect(*r, self.scale).contains(x, y) && sb.removable(i);
                return Some(if button { Hit::SidePin(i) } else { Hit::Side(i) });
            }
        }
        if let Some(&(_, s)) = sb.headers.iter().find(|(r, _)| r.contains(x, y)) {
            return Some(Hit::SideHeader(s));
        }
        Some(Hit::SideEmpty)
    }

    pub(in crate::ui) fn sidebar_wheel(&mut self, dy: f32) {
        let step = self.wheel_px(((ROW_H + ROW_GAP) * self.scale).round(), self.nav.sidebar.list.h);
        let sb = &mut self.nav.sidebar;
        sb.scroll.by(-dy * step, sb.content_h - sb.list.h);
        self.layout();
        self.dirty = true;
    }

    pub(in crate::ui) fn touch_pan_sidebar(&mut self, dy: f32) -> bool {
        let sb = &mut self.nav.sidebar;
        let was = sb.scroll.pos;
        sb.scroll.snap((was - dy).clamp(0.0, (sb.content_h - sb.list.h).max(0.0)));
        let moved = sb.scroll.pos != was;
        if moved {
            self.layout();
        }
        moved
    }

    pub(in crate::ui) fn begin_splitter_drag(&mut self, x: f32) {
        self.drag = Drag::Splitter { start_x: x, start_w: self.nav.sidebar.state.sidebar_w };
    }

    pub(in crate::ui) fn drag_splitter(&mut self, x: f32, start_x: f32, start_w: f32) {
        let w = (start_w + (x - start_x) / self.scale).clamp(self.nav.sidebar.min_w(), SIDEBAR_MAX_W);
        self.nav.sidebar.state.sidebar_w = w.round();
        self.layout();
        self.dirty = true;
    }

    pub(super) fn reveal_side_cursor(&mut self) {
        self.layout();
        let sb = &mut self.nav.sidebar;
        let Some(r) = sb.stop_rect(sb.stop()) else { return };
        if r.y < sb.list.y {
            sb.scroll.snap(sb.scroll.pos - (sb.list.y - r.y));
        } else if r.bottom() > sb.list.bottom() {
            sb.scroll.snap(sb.scroll.pos + r.bottom() - sb.list.bottom());
        }
        self.layout();
    }

    pub(in crate::ui) fn sidebar_key(&mut self, key: Key, m: Mods, now: Instant) -> bool {
        let n = self.nav.sidebar.items.len();
        let plain = !m.ctrl && !m.alt && !m.shift;
        let cur = self.nav.sidebar.cursor;
        let typing = self.nav.sidebar.typing;
        if typing && !matches!(key, Key::Escape | Key::Enter | Key::Up | Key::Down | Key::PageUp | Key::PageDown) && self.quick_filter_key(key, m) {
            return true;
        }
        let head = self.nav.sidebar.head;
        let stops = self.nav.sidebar.stops();
        let at = stops.iter().position(|&s| s == self.nav.sidebar.stop()).unwrap_or(0);
        let last = stops.len().saturating_sub(1);
        let to = match key {
            Key::Escape if typing && self.nav.sidebar.filtering() => {
                self.clear_quick_filter();
                return true;
            }
            Key::Escape => {
                self.nav.sidebar.typing = false;
                self.nav.sidebar.filter.edit.primary_id = 0;
                self.focus = Focus::List;
                return true;
            }
            Key::Enter | Key::Right if head.is_some() && !m.alt => {
                self.unfold_side_section();
                return true;
            }
            Key::Enter if !m.alt => {
                self.open_side_item(cur, m.ctrl, now);
                return true;
            }
            Key::F(2) if plain && head.is_none() && self.side_drive(cur).is_some() => {
                if let Some(drive) = self.side_drive(cur) {
                    self.ask_label(drive);
                }
                return true;
            }
            Key::Delete if plain => {
                if head.is_none() && self.nav.sidebar.items.get(cur).is_some_and(|it| it.section == Section::Bookmarks) {
                    self.toggle_pin_item(cur, now);
                }
                return true;
            }
            Key::Left if plain && !typing => {
                self.fold_side_section();
                return true;
            }
            Key::Right if plain && !typing => return true,
            Key::Up if plain => at.saturating_sub(1),
            Key::Down if plain => (at + 1).min(last),
            Key::Home if plain => 0,
            Key::End if plain => last,
            Key::PageUp if plain => self.nav.sidebar.page(&stops, at, false),
            Key::PageDown if plain => self.nav.sidebar.page(&stops, at, true),
            _ => return false,
        };
        match stops.get(to) {
            Some(&stop) => self.nav.sidebar.go_to(stop),
            None => self.nav.sidebar.point(n.saturating_sub(1).min(cur)),
        }
        self.reveal_side_cursor();
        true
    }

    fn side_drive(&self, i: usize) -> Option<FsPath> {
        let item = self.nav.sidebar.items.get(i).filter(|it| it.section == Section::Storage)?;
        self.side_label(item.loc.path()).map(|_| item.loc.path().clone())
    }

    pub(in crate::ui) fn side_label(&self, drive: &FsPath) -> Option<String> {
        if self.caps.unix_paths {
            return None;
        }
        self.nav.sidebar.places.drives.iter().find(|d| d.path == *drive)?.volume_label()
    }

    pub(in crate::ui) fn ask_label(&mut self, drive: FsPath) {
        if let Some(label) = self.side_label(&drive) {
            self.open_pick(crate::ui::palette::Palette::new(crate::ui::palette::Pick::Label(drive), Vec::new(), &label));
        }
    }

    pub(in crate::ui) fn relabel_drive(&mut self, drive: FsPath, text: &str) {
        let label = text.trim();
        if self.side_label(&drive).is_some_and(|old| old != label) {
            self.effects.push(Effect::SetVolumeLabel { drive, label: label.to_owned() });
        }
    }

    fn fold_side_section(&mut self) {
        let sb = &self.nav.sidebar;
        if sb.head.is_some() || sb.filtering() {
            return;
        }
        let Some(s) = sb.items.get(sb.cursor).map(|it| it.section) else { return };
        self.toggle_section(s);
        self.nav.sidebar.head = Some(s);
        self.reveal_side_cursor();
    }

    fn unfold_side_section(&mut self) {
        let Some(s) = self.nav.sidebar.head else { return };
        self.toggle_section(s);
        let sb = &mut self.nav.sidebar;
        if let Some(i) = sb.items.iter().position(|it| it.section == s) {
            sb.point(i);
        }
        self.reveal_side_cursor();
    }

    pub(in crate::ui) fn draw_sidebar<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme) {
        let now = self.last_frame.unwrap_or_else(Instant::now);
        let hovered = match self.hover {
            Hit::Side(k) | Hit::SidePin(k) if self.drag == Drag::None => Some(k),
            _ => None,
        };
        self.motion |= self.side_glow.aim(hovered, self.animations, now);
        let sb = &self.nav.sidebar;
        let area = sb.rect;
        if area.w <= 0.0 {
            return;
        }
        let compact = sb.prefs.compact;
        g.dl.rect(area, t.chrome);
        let line = if matches!(self.hover, Hit::Splitter) || matches!(self.drag, Drag::Splitter { .. }) { t.accent } else { t.separator };
        let radius = self.s(3.0);
        let border = self.s(1.0).max(1.0);
        if g.dl.round {
            g.dl.rect(Rect::new(area.right() - border, area.y + radius, border, (area.h - radius).max(0.0)), line);
            let corner = Rect::new(area.right() - border, area.y - border, radius + border, radius + border);
            g.dl.rect(corner, self.look.palette.base.chrome);
            let from = g.dl.instances.len();
            let circle = Rect::new(corner.x, corner.y, 2.0 * (radius + border), 2.0 * (radius + border));
            g.dl.circle(circle, self.look.palette.area(crate::theme::Area::Toolbar).bg);
            g.dl.ring(circle, line, border);
            g.dl.confine(from, corner);
        } else {
            g.dl.rect(Rect::new(area.right() - border, area.y, border, area.h), line);
        }
        let font = self.ui_font(13.0);
        let small = self.ui_font(12.0);
        let icons = self.icon_font(ROW_ICON * 0.85);
        let pin_font = self.icon_font(11.0);
        let chevron = self.icon_font(8.0);
        let header_font = self.header_font();
        let header_icons = self.icon_font(13.0);
        let m = g.metrics(font);
        let mh = g.metrics(header_font);
        let (s4, s8) = (self.s(4.0), self.s(8.0));
        let here = self.tabs.items[self.tabs.cur].location();
        let focused = self.active && self.focus == Focus::Sidebar;
        if sb.field.w > 0.0 {
            let typing = focused && sb.typing;
            let mut w = Paint::new(g, t, self.scale);
            w.field(sb.field, typing);
            let fr = sb.field;
            w.centered(Rect::new(fr.x, fr.y, fr.h, fr.h), G_SEARCH, self.icon_font(11.0), t.text_dim);
            let right = if sb.clear.w > 0.0 { sb.clear.x - s4 } else { fr.right() - s8 };
            let inner = Rect::new(fr.x + fr.h, fr.y, (right - fr.x - fr.h).max(0.0), fr.h);
            let preedit = typing.then_some(&self.preedit);
            self.nav.sidebar.filter.draw(g, t, inner, font, typing, S::SidebarFilter.t(), self.scale, preedit);
            let sb = &self.nav.sidebar;
            if sb.clear.w > 0.0 {
                Paint::new(g, t, self.scale).icon_button(sb.clear, G_CLOSE, self.icon_font(8.0), self.button(Hit::QuickClear, false, true), t.text_dim);
            }
        }
        let sb = &self.nav.sidebar;
        g.dl.set_clip(Some(sb.list));
        let (drop, drop_header) = match self.drop_spot() {
            Some(crate::ui::Spot::Side(i)) => (Some(i), None),
            Some(crate::ui::Spot::SideHeader(s)) => (None, Some(s)),
            _ => (None, None),
        };
        let target = t.teal.alpha(0x70);
        for &(r, section) in &sb.headers {
            let open = self.menu_open_from(Hit::SideHeader(section));
            let glow = if open { 1.0 } else { self.lit(Hit::SideHeader(section)) };
            let lit = glow >= 0.5;
            if drop_header == Some(section) {
                g.dl.round_rect(r, target, s4);
            } else if glow > 0.0 {
                g.dl.round_rect(r, crate::ui::widgets::faded(t.accent_fill, glow), s4);
            }
            if focused && !sb.typing && sb.head == Some(section) {
                g.dl.outline(r, t.focus_ring, s4, 1.0);
            }
            if compact {
                g.dl.rect(Rect::new(r.x + s8, (r.y + r.h / 2.0).round(), r.w - 2.0 * s8, 1.0), t.separator);
                continue;
            }
            let ink = if lit && drop_header != Some(section) { ink_on(t.accent_fill) } else { t.text };
            let (icon_r, title_x, baseline, chevron_r) = header_parts(r, mh.ascent, mh.descent, self.scale);
            centered(g, icon_r, section.glyph(), header_icons, ink);
            g.text(title_x, baseline, section.title(), header_font, ink, chevron_r.x - title_x - s4);
            let angle = match sb.fold.filter(|f| f.section == section) {
                Some(f) => 90.0 * f.shown(now),
                None if sb.is_collapsed(section) && !sb.filtering() => 0.0,
                None => 90.0,
            };
            let center = (chevron_r.x + chevron_r.w / 2.0, chevron_r.y + chevron_r.h / 2.0);
            g.dl.chevron(center, self.s(8.0), angle, self.s(1.25).max(1.0), if lit { ink } else { t.text_dim });
        }
        let mut requests: Vec<IconRequest> = Vec::new();
        let list = sb.list;
        for &(r, k) in &sb.gaps {
            let Some(l) = sb.leaving.get(k) else { continue };
            let fade = (1.0 - eased(l.since, now, FOLD)).clamp(0.0, 1.0);
            let alpha = |c: Color| c.alpha((fade * 255.0) as u8);
            g.dl.set_clip(Some(clip_to(list, r)));
            let text_h = self.s(ROW_H);
            let (icon_r, text_x) = (item_icon(Rect::new(r.x, r.y, r.w, text_h), self.scale, false), item_parts(r, self.scale).1);
            centered(g, icon_r, l.glyph, icons, alpha(t.accent));
            g.text(text_x, (r.y + (text_h + m.ascent - m.descent) / 2.0).round(), &l.label, font, alpha(t.text), r.right() - text_x);
        }
        g.dl.set_clip(Some(list));
        for (i, (it, &r)) in sb.items.iter().zip(&sb.rects).enumerate() {
            if r.bottom() < list.y || r.y > list.bottom() {
                continue;
            }
            match sb.fold_clip.filter(|(s, _)| *s == it.section) {
                Some((_, c)) if r.y >= c.bottom() => continue,
                Some((_, c)) => g.dl.set_clip(Some(clip_to(list, c))),
                None => g.dl.set_clip(Some(list)),
            }
            let hovered = hovered == Some(i);
            let glow = self.side_glow.alpha(i, now);
            let row = Rect::new(r.x, r.y + 1.0, r.w, r.h - 2.0);
            let tag = sb.tag_of(i).and_then(|k| sb.tags.defs.get(k));
            if drop == Some(i) {
                g.dl.round_rect(row, target, s4);
            } else {
                if it.loc == *here && tag.is_none() && glow < 1.0 {
                    g.dl.round_rect(row, t.selection, s4);
                }
                if glow > 0.0 {
                    g.dl.round_rect(row, crate::ui::widgets::faded(t.accent_fill, glow), s4);
                }
            }
            if focused && !sb.typing && sb.head.is_none() && sb.cursor == i && drop != Some(i) {
                g.dl.outline(row, t.focus_ring, s4, 1.0);
            }
            let lit = glow >= 0.5 && drop != Some(i);
            let ink = if lit {
                ink_on(t.accent_fill)
            } else if it.offline {
                t.text_dim
            } else {
                t.text
            };
            let glyph = if it.offline { G_NET_OFFLINE } else { it.glyph };
            let color = match tag {
                Some(tag) => Color::rgb(tag.color),
                None if lit || it.offline => ink,
                None if it.section == Section::Storage || it.section == Section::Wsl => t.text_dim,
                None => t.accent,
            };
            if compact {
                if !self.place_icon(g, it, r, &mut requests) {
                    centered(g, r, glyph, icons, color);
                }
                if let Some(used) = it.usage {
                    let bar = Rect::new(r.x + s8, r.bottom() - self.s(5.0), (r.w - 2.0 * s8).max(0.0), self.s(3.0));
                    self.draw_usage(g, t, bar, used);
                }
                continue;
            }
            let text_h = self.s(ROW_H);
            let (icon_r, text_x) = (item_icon(r, self.scale, it.usage.is_some()), item_parts(r, self.scale).1);
            if !self.place_icon(g, it, icon_r, &mut requests) {
                centered(g, icon_r, glyph, icons, color);
            }
            let baseline = (r.y + (text_h + m.ascent - m.descent) / 2.0).round();
            let pin_r = sb.pin_rect(Rect::new(r.x, r.y, r.w, text_h), self.scale);
            let trash = hovered && sb.removable(i);
            let connect = it.offline && it.section == Section::Storage;
            let text_right = if trash || connect { pin_r.x - s4 } else { r.right() - s8 };
            let mark = if lit { ink } else { t.accent };
            let w = Paint::new(g, t, self.scale).marked((text_x, baseline), &it.label, &it.marks, font, (ink, mark), text_right - text_x);
            if sb.prefs.quick.show_paths && !it.path.is_empty() {
                let gap = self.s(PATH_GAP);
                let x = text_x + w + gap;
                if x < text_right {
                    let cw = g.text(x, baseline - self.s(1.0), G_COLLAPSED, chevron, if lit { ink } else { t.text_dim }, text_right - x);
                    let px = x + cw + gap;
                    g.text(px, baseline, &it.path, small, if lit { ink.alpha(0xC0) } else { t.text_dim }, text_right - px);
                }
            }
            if let Some(used) = it.usage {
                let bar = self.drive_bar(r);
                self.draw_usage(g, t, bar, used);
            }
            if connect {
                Paint::new(g, t, self.scale).icon_button(pin_r, "\u{E72C}", pin_font, self.button(Hit::Side(i), false, true), ink);
            } else if trash {
                let open = self.dialogs.confirm.as_ref().is_some_and(|c| c.anchor == pin_r);
                Paint::new(g, t, self.scale).icon_button(pin_r, G_TRASH, pin_font, self.button(Hit::SidePin(i), open, true), ink);
            }
        }
        g.dl.set_clip(Some(list));
        let row_insert = |first: usize, n: usize, to: usize| {
            let n = n.min(sb.rects.len().saturating_sub(first));
            let before = sb.rects.get(first + to).filter(|_| to < n).map(|r| r.y);
            before.or_else(|| sb.rects.get((first + n).saturating_sub(1)).map(|r| r.bottom())).unwrap_or(list.y)
        };
        let insert_y = match self.drag {
            Drag::PinMove { to, .. } => Some(row_insert(sb.first_bookmark(), sb.state.pinned.len(), to)),
            Drag::PlaceMove { to, .. } => Some(row_insert(sb.first_place(), sb.items.iter().filter(|it| it.section == Section::Places).count(), to)),
            Drag::SectionMove { .. } => self.tabs.strip.pointer.and_then(|(x, y)| sb.section_drop_slot(x, y)).map(|to| {
                let gap = self.s(if compact { 2.0 } else { SECTION_GAP });
                sb.headers.get(to).map_or(list.y - sb.scroll.pos.round() + sb.content_h - gap, |(r, _)| r.y)
            }),
            _ => None,
        };
        if let Some(y) = insert_y {
            g.dl.rect(Rect::new(area.x + s8, y - 1.0, area.w - 2.0 * s8, self.s(2.0)), t.accent);
        }
        g.dl.set_clip(None);
        let item = |i: usize| sb.items.get(i).map(|it| (it.glyph, it.label.as_str()));
        let moving = match self.drag {
            Drag::PinMove { from, .. } => item(sb.first_bookmark() + from),
            Drag::PlaceMove { from, .. } => item(sb.first_place() + from),
            Drag::SectionMove { from } => Some((from.glyph(), from.title())),
            _ => None,
        };
        if let (Some((glyph, label)), Some((x, y))) = (moving, self.tabs.strip.pointer) {
            let r = Rect::new(x + s8, y + s8, area.w.min(self.s(240.0)), self.s(ROW_H));
            g.dl.round_rect(r, t.bg, s4);
            g.dl.outline(r, t.accent, s4, 1.0);
            centered(g, Rect::new(r.x, r.y, self.s(32.0), r.h), glyph, icons, t.accent);
            g.text(r.x + self.s(34.0), r.y + (r.h + m.ascent - m.descent) / 2.0, label, font, t.text, r.w - self.s(42.0));
        }
        self.request_icons(requests);
    }

    fn draw_usage<B: TextBackend>(&self, g: &mut Gfx<B>, t: &Theme, bar: Rect, used: f32) {
        g.dl.round_rect(bar, t.separator, bar.h / 2.0);
        let fill = if used >= 0.9 { t.error } else { t.accent };
        g.dl.round_rect(Rect::new(bar.x, bar.y, (bar.w * used.clamp(0.0, 1.0)).max(bar.h), bar.h), fill, bar.h / 2.0);
    }

    pub(super) fn header_font(&self) -> FontStyle {
        FontStyle::new(FontFamily::Ui, 13.0 * self.scale, true)
    }

    pub(super) fn drive_bar(&self, r: Rect) -> Rect {
        let (_, x) = item_parts(r, self.scale);
        let avail = (r.right() - self.s(12.0) - x).max(0.0);
        Rect::new(x, r.y + self.s(ROW_H) - self.s(2.0), self.s(self.nav.sidebar.bar_w).min(avail), self.s(4.0))
    }

    pub(in crate::ui) fn fit_sidebar<B: TextBackend>(&mut self, g: &mut Gfx<B>) {
        let font = self.ui_font(13.0);
        let sb = &mut self.nav.sidebar;
        let widest = sb.items.iter().filter(|it| it.usage.is_some()).map(|it| g.measure(&it.label, font)).fold(0.0, f32::max);
        let bar_w = if widest > 0.0 { (widest / self.scale).ceil() + BAR_MARGIN } else { 0.0 };
        sb.bar_w = bar_w;
        let min = sb.min_w();
        if sb.state.sidebar_w < min {
            sb.state.sidebar_w = min;
            self.layout();
        }
    }

    pub(super) fn place_icon<B: TextBackend>(&self, g: &mut Gfx<B>, it: &Item, r: Rect, requests: &mut Vec<IconRequest>) -> bool {
        let px = self.s(PLACE_ICON);
        self.system_icon(g, place_icon_request(it, &self.nav.sidebar.places, px as u32), r, px, 0xFF, requests)
    }
}
