use super::*;

impl App {
    pub(in crate::ui) fn search_event(&mut self, ev: &Event, now: Instant) -> bool {
        match ev {
            Event::Key { key, mods, pressed } => {
                self.mods = *mods;
                !*pressed || self.search_key(*key, *mods, now)
            }
            Event::Char(c) => {
                if !c.is_control() {
                    let mut b = [0; 4];
                    self.search.edit.insert(c.encode_utf8(&mut b));
                    self.search_query(now);
                }
                true
            }
            Event::MouseDown { x, y, button, clicks } => self.search_mouse_down(*x, *y, *button, *clicks, now),
            Event::MouseUp { x, y, button } => self.search_mouse_up(*x, *y, *button),
            Event::MouseMove { x, y } => self.search_mouse_move(*x, *y, now),
            Event::Wheel { dx, dy } => {
                if matches!(self.search.hover, SHit::ResultFolder(_) | SHit::FolderBackground) {
                    let sl = self.search_layout();
                    let shown = ((sl.folders.h - self.layout.header.h).max(0.0) / sl.row_h) as usize;
                    let max = self.search.folders_summary.len().saturating_sub(shown.max(1));
                    self.search.folders_scroll = self.search.folders_scroll.saturating_add_signed((-dy * 3.0).round() as isize).min(max);
                    self.dirty = true;
                    return true;
                }
                if *dx != 0.0 || self.mods.shift {
                    let delta = if *dx != 0.0 { *dx } else { -*dy };
                    self.search.scroll_x = (self.search.scroll_x + delta * self.s(48.0)).clamp(0.0, self.search_layout().max_x);
                    self.dirty = true;
                    return true;
                }
                let sl = self.search_layout();
                let step = f64::from(self.wheel_px(sl.row_h, sl.list.h));
                let max = self.search_max_scroll();
                self.search.scroll = (self.search.scroll - *dy as f64 * step).clamp(0.0, max);
                self.dirty = true;
                true
            }
            Event::DroppedFiles(_) => {
                self.close_search();
                false
            }
            _ => false,
        }
    }

    fn recent_key(&mut self, key: Key, m: Mods, now: Instant) -> bool {
        let n = self.search.recent().len();
        if n == 0 {
            return false;
        }
        let s = &mut self.search;
        match key {
            Key::Down => s.recent_sel = Some(s.recent_sel.map_or(0, |k| (k + 1) % n)),
            Key::Up => s.recent_sel = Some(s.recent_sel.map_or(n - 1, |k| (k + n - 1) % n)),
            Key::Escape => {
                s.recent_open = false;
                s.recent_sel = None;
            }
            Key::Enter if s.recent_sel.is_some() => {
                let k = s.recent_sel.unwrap_or(0);
                self.apply_recent(k, now);
            }
            Key::Delete if m.shift && s.recent_sel.is_some() => {
                let k = s.recent_sel.unwrap_or(0);
                s.history.remove(k);
                let left = s.recent().len();
                s.recent_sel = (left > 0).then(|| k.min(left - 1));
            }
            _ => return false,
        }
        true
    }

    fn apply_recent(&mut self, k: usize, now: Instant) {
        let Some(q) = self.search.history.get(k).cloned() else { return };
        let s = &mut self.search;
        s.edit.set(&q);
        s.edit.select_all();
        s.recent_open = false;
        s.recent_sel = None;
        self.search_query(now);
        self.search.recent_open = false;
    }

    pub(in crate::ui) fn search_key(&mut self, key: Key, m: Mods, now: Instant) -> bool {
        self.dirty = true;
        if self.recent_key(key, m, now) {
            return true;
        }
        let sl = self.search_layout();
        let page = ((sl.list.h / sl.row_h).floor() as i64).max(1) * sl.tiles as i64;
        let mut moved = None;
        if self.settings.keymap.runs(key, m, "search.open") {
            if self.search.within.is_some() {
                self.open_scoped(None);
            }
            self.search.edit.select_all();
            return true;
        }
        if self.settings.keymap.runs(key, m, "search.folder") {
            if self.search.within.is_none() && self.search.home.is_some() {
                self.toggle_scope();
            }
            self.search.edit.select_all();
            return true;
        }
        match (key, m.ctrl, m.alt) {
            (Key::Tab, false, false) => {
                self.toggle_scope();
                return true;
            }
            (Key::Escape, ..) => {
                self.close_search();
                return true;
            }
            (Key::Enter, false, false) => {
                self.open_search_row(self.search.focus.unwrap_or(0), now);
                return true;
            }
            (Key::Enter, true, false) => {
                self.reveal_search_row(self.search.focus.unwrap_or(0), now);
                return true;
            }
            (Key::Char('C'), true, false) if m.shift => {
                if let Some(p) = self.search.focus.and_then(|f| self.search_row_location(f)) {
                    let text = self.as_path_text(&p);
                    self.effects.push(Effect::SetClipboard(text));
                }
                return true;
            }
            (Key::Char('T' | 'W' | '1'..='9') | Key::Tab | Key::F(4), true, false) => {
                self.close_search();
                return false;
            }
            (Key::Left | Key::Right, false, false) if sl.grid && self.search.focus.is_some() && !m.shift => {
                moved = Some(if key == Key::Left { -1 } else { 1 });
            }
            (Key::Up, _, false) => moved = Some(if self.search.focus.is_some() { -(sl.tiles as i64) } else { -1 }),
            (Key::Down, _, false) => moved = Some(if self.search.focus.is_some() { sl.tiles as i64 } else { 1 }),
            (Key::PageUp, _, false) => moved = Some(-page),
            (Key::PageDown, _, false) => moved = Some(page),
            (Key::Home, true, false) => moved = Some(i64::MIN / 2),
            (Key::End, true, false) => moved = Some(i64::MAX / 2),
            _ => {}
        }
        if let Some(d) = moved {
            self.search_move(d);
            return true;
        }
        match self.search.edit.key(key, m) {
            EditAction::Changed => self.search_query(now),
            EditAction::Copy(s) => self.effects.push(Effect::SetClipboard(s)),
            EditAction::Paste => self.effects.push(Effect::Paste),
            EditAction::Submit | EditAction::Cancel | EditAction::None => {}
        }
        true
    }

    fn search_move(&mut self, delta: i64) {
        let total = self.search.total;
        if total == 0 {
            return;
        }
        let base = self.search.focus.map_or(if delta < 0 { total as i64 } else { -1 }, |f| f as i64);
        let row = base.saturating_add(delta).clamp(0, total as i64 - 1) as u64;
        self.search.focus = Some(row);
        self.search_reveal(row);
    }

    fn search_reveal(&mut self, row: u64) {
        let sl = self.search_layout();
        let (rh, lh) = (f64::from(sl.row_h), f64::from(sl.list.h));
        let line = self.search.line_of_row(row) / sl.tiles;
        let (top, bottom) = (line as f64 * rh, (line + 1) as f64 * rh);
        let max = self.search_max_scroll();
        let s = &mut self.search;
        if top < s.scroll {
            s.scroll = top;
        } else if bottom > s.scroll + lh {
            s.scroll = bottom - lh;
        }
        s.scroll = s.scroll.clamp(0.0, max);
    }

    fn search_row_location(&self, row: u64) -> Option<Location> {
        let r = self.search.row(row)?;
        if r.dir.is_empty() {
            return None;
        }
        Some(Location::parse_exact(&r.dir)?.join(&r.name))
    }

    pub(in crate::ui) fn search_target(&self) -> Option<(Location, bool)> {
        let row = self.search.focus?;
        Some((self.search_row_location(row)?, self.search.row(row)?.meta.attrs & attr::DIR != 0))
    }

    pub(in crate::ui) fn search_shell_target(&self) -> Option<crate::ui::ShellMenu> {
        let row = self.search.rows.get(&self.search.focus?)?;
        let dir = crate::FsPath::parse_exact(&row.dir)?;
        let path = dir.join(&row.name);
        if crate::archive::split(&path).is_some() || crate::recycle::in_bin(&path) {
            return None;
        }
        let kind = if row.meta.attrs & attr::DIR != 0 { crate::ui::MenuKind::Folder } else { crate::ui::MenuKind::File };
        Some(crate::ui::ShellMenu { dir, items: vec![path], x: 0.0, y: 0.0, extended: false, menu: Vec::new(), inline: self.shell_items_inline, shell: true, kind })
    }

    pub(in crate::ui) fn search_command(&mut self, action: &str, location: Location, folder: bool, now: Instant) {
        match action {
            "open" if !folder => self.effects.push(Effect::Provider { provider: location.provider(), req: ProviderRequest::Open { tab: self.tab().id, path: location.path().clone() } }),
            "open" | "reveal" | "reveal_tab" => {
                let (folder, select) = if action == "open" {
                    (location, None)
                } else {
                    let Some(parent) = location.parent() else { return };
                    (parent, Some(location.file_name().to_vec()))
                };
                self.remember_query();
                self.close_search();
                self.dispatch(vec![Route { folder, select }], if action == "reveal_tab" { OpenTarget::NewTab } else { OpenTarget::CurrentTab }, now);
            }
            "exclude" => {
                let mut paths = self.search.exclusions.clone();
                paths.insert(0, location);
                self.set_search_exclusions(paths);
            }
            "path" => {
                let text = self.as_path_text(&location);
                self.effects.push(Effect::SetClipboard(text));
            }
            "quoted" => self.effects.push(Effect::SetClipboard(format!("\"{}\"", location.display()))),
            "copy" => {
                if let Some(path) = location.to_windows() {
                    self.effects.push(Effect::SetClipboardFiles { paths: vec![path], cut: false });
                }
            }
            _ => {}
        }
    }

    fn open_search_row(&mut self, row: u64, now: Instant) {
        let Some(loc) = self.search_row_location(row) else { return };
        self.remember_query();
        if self.search.row(row).is_some_and(|r| r.meta.attrs & attr::DIR != 0) {
            self.close_search();
            self.dispatch(vec![Route { folder: loc, select: None }], OpenTarget::CurrentTab, now);
        } else {
            let tab = self.tab().id;
            self.effects.push(Effect::Provider { provider: loc.provider(), req: ProviderRequest::Open { tab, path: loc.path().clone() } });
        }
    }

    pub(in crate::ui) fn reveal_search_focus(&mut self, now: Instant) {
        if let Some(row) = self.search.focus {
            self.reveal_search_row(row, now);
        }
    }

    fn reveal_search_row(&mut self, row: u64, now: Instant) {
        let Some(loc) = self.search_row_location(row) else { return };
        let Some(folder) = loc.parent() else { return };
        let select = Some(loc.file_name().to_vec());
        self.close_search();
        self.dispatch(vec![Route { folder, select }], OpenTarget::CurrentTab, now);
    }

    fn open_group(&mut self, k: usize, now: Instant) {
        let Some(dir) = self.search.groups.get(k).and_then(|&r| self.search.row(r)).map(|r| r.dir.clone()) else { return };
        let Some(folder) = Location::parse_exact(&dir) else { return };
        self.close_search();
        self.dispatch(vec![Route { folder, select: None }], OpenTarget::CurrentTab, now);
    }

    fn search_mouse_down(&mut self, x: f32, y: f32, button: MouseButton, clicks: u8, now: Instant) -> bool {
        if self.sidebar_hit(x, y).is_some() {
            self.close_search();
            return false;
        }
        if self.in_title(y) {
            if matches!(self.hit(x, y), Hit::Tab(_) | Hit::TabClose(_) | Hit::NewTab) {
                self.close_search();
            }
            return false;
        }
        if matches!(button, MouseButton::Back) {
            self.close_search();
            return true;
        }
        self.dirty = true;
        let h = self.search_hit(x, y);
        self.search.pressed = h;
        if !matches!(h, SHit::Recent(_) | SHit::Field) {
            self.search.recent_open = false;
        }
        match h {
            SHit::ColSep(col) if button == MouseButton::Left && clicks >= 2 => {
                self.search.drag = SDrag::None;
                self.search.fit = Some(col);
            }
            SHit::ColSep(col) if button == MouseButton::Left => {
                let widths = self.search_layout().cols.map(|c| c.w / self.scale);
                self.search.widths = Some(widths);
                self.search.drag = SDrag::ColSep { col, x, width: widths[col] };
                self.set_cursor(CursorIcon::ResizeHorizontal);
            }
            SHit::Field if button == MouseButton::Right => {
                let menu = self.collect(|a, m| crate::ui::field::text_entries(m, &a.search.edit, false, a.text_on_clipboard()));
                self.flit_menu(x, y, menu);
            }
            SHit::Field => {
                if self.search_layout().grid {
                    self.search.focus = None;
                }
                match Grab::of(self.text_clicks((x, y), clicks, now), self.mods.shift) {
                    Some(how) => {
                        self.search.pending_click = Some((x, how));
                        self.search.drag = SDrag::Select(how.dragged());
                    }
                    None => self.search.edit.select_all(),
                }
                self.search.recent_open = true;
            }
            SHit::Row(row) => {
                self.search.focus = Some(row);
                if clicks >= 2 && button == MouseButton::Left {
                    if self.mods.ctrl {
                        self.reveal_search_row(row, now);
                    } else {
                        self.open_search_row(row, now);
                    }
                }
            }
            SHit::Group(k) if clicks >= 2 && button == MouseButton::Left => self.open_group(k, now),
            SHit::Thumb => self.search.drag = SDrag::Thumb(y - self.search_layout().thumb.y),
            SHit::HThumb => self.search.drag = SDrag::HThumb(x - self.search_layout().hthumb.x),
            SHit::HTrack => {
                let sl = self.search_layout();
                self.search.scroll_x = (self.search.scroll_x + if x < sl.hthumb.x { -sl.list.w } else { sl.list.w }).clamp(0.0, sl.max_x);
            }
            SHit::Track => {
                let dir = if y < self.search_layout().thumb.y { -1.0 } else { 1.0 };
                let max = self.search_max_scroll();
                self.search.scroll = (self.search.scroll + dir * self.layout.list.h as f64).clamp(0.0, max);
            }
            _ => {}
        }
        true
    }

    fn search_mouse_up(&mut self, x: f32, y: f32, button: MouseButton) -> bool {
        if self.in_title(y) && self.search.drag == SDrag::None {
            return false;
        }
        let was = std::mem::take(&mut self.search.drag);
        let pressed = std::mem::take(&mut self.search.pressed);
        self.dirty = true;
        let h = self.search_hit(x, y);
        if button == MouseButton::Right && matches!(h, SHit::Header(_) | SHit::Row(_) | SHit::Group(_) | SHit::Empty) {
            let menu = self.build_search_menu(matches!(h, SHit::Row(_)));
            self.flit_menu(x, y, menu);
            return true;
        }
        if button != MouseButton::Left || matches!(was, SDrag::Thumb(_) | SDrag::HThumb(_) | SDrag::ColSep { .. }) {
            return true;
        }
        if let (SHit::Date(a), SHit::Date(b)) = (pressed, h) {
            self.select_dates(a, b);
            return true;
        }
        if h != pressed {
            return true;
        }
        match h {
            SHit::Back => self.close_search(),
            SHit::Scope => self.toggle_scope(),
            SHit::Folders => self.toggle_search_folders(),
            SHit::Facets => self.toggle_facets(),
            SHit::Facet(k) => self.facet_menu(k),
            SHit::WithinResults => self.toggle_within_results(Instant::now()),
            SHit::ResultFolder(k) => self.choose_result_folder(k, Instant::now()),
            SHit::SimilarChip(i, remove) => {
                if let Some((range, _)) = self.search.similar_chips.get(i).cloned() {
                    self.search.edit.move_to(range.start, false);
                    self.search.edit.move_to(range.end, true);
                    if remove {
                        self.search.edit.key(Key::Backspace, Mods::default());
                        self.search_query(Instant::now());
                    }
                }
            }
            SHit::Recent(k) => self.apply_recent(k, Instant::now()),
            SHit::Header(c) => {
                let key = COLUMNS[c].1;
                self.search.group_folder &= key == SearchKey::Folder;
                let s = &mut self.search.sort;
                *s = SearchSort { key, descending: s.key == key && !s.descending };
                self.search_query(Instant::now());
            }
            _ => {}
        }
        true
    }

    fn search_mouse_move(&mut self, x: f32, y: f32, now: Instant) -> bool {
        match self.search.drag {
            SDrag::HThumb(grab) => {
                let sl = self.search_layout();
                self.search.scroll_x = ((x - grab - sl.htrack.x) / (sl.htrack.w - sl.hthumb.w).max(1.0)).clamp(0.0, 1.0) * sl.max_x;
                self.dirty = true;
                return true;
            }
            SDrag::ColSep { col, x: start, width } => {
                if let Some(widths) = &mut self.search.widths {
                    widths[col] = (width + (x - start) / self.scale).clamp(48.0, 2400.0);
                }
                self.dirty = true;
                return true;
            }
            SDrag::Thumb(grab) => {
                let sl = self.search_layout();
                let range = (sl.track.h - sl.thumb.h).max(1.0);
                let frac = ((y - grab - sl.track.y) / range).clamp(0.0, 1.0) as f64;
                self.search.scroll = frac * self.search_max_scroll();
                self.dirty = true;
                return true;
            }
            SDrag::Select(how) => {
                self.search.pending_click = Some((x, how));
                self.dirty = true;
                return true;
            }
            SDrag::None => {}
        }
        if self.in_title(y) {
            if self.search.hover != SHit::None {
                self.search.hover = SHit::None;
                self.dirty = true;
            }
            return false;
        }
        let h = self.search_hit(x, y);
        self.set_cursor(match h {
            SHit::Field => CursorIcon::Text,
            SHit::ColSep(_) => CursorIcon::ResizeHorizontal,
            _ => CursorIcon::Default,
        });
        if h != self.search.hover {
            self.search.hover = h;
            self.tip_hover(self.search_tip_hit(), now);
            self.dirty = true;
        }
        true
    }
}
