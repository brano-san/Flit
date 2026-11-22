use super::*;

impl App {
    pub(in crate::ui) fn rename_target(&self) -> Option<u32> {
        let t = self.tab();
        let v = t.view.as_ref().filter(|_| !matches!(t.view_loc.path().kind(), PathKind::Computer | PathKind::RecycleBin) && self.archive_view().is_none())?;
        v.focus.filter(|&f| v.is_visible(f)).or_else(|| if v.selected_count() == 1 { v.selected().next() } else { None })
    }

    pub fn rename_focused(&mut self) {
        let Some(i) = self.rename_target().or_else(|| self.labelled_drive()) else { return };
        self.ops.ids = 0;
        let batch = self.tab().view.as_ref().is_some_and(|v| v.selected_count() > 1 && v.is_selected(i));
        self.begin_rename(i, batch);
    }

    pub(in crate::ui) fn labelled_drive(&self) -> Option<u32> {
        let v = self.tab().view.as_ref()?;
        let i = v.focus.filter(|&f| v.is_visible(f)).or_else(|| if v.selected_count() == 1 { v.selected().next() } else { None })?;
        self.drive_label(i).map(|_| i)
    }

    fn drive_label(&self, idx: u32) -> Option<String> {
        let t = self.tab();
        if self.caps.unix_paths || t.view_loc.path().kind() != PathKind::Computer {
            return None;
        }
        let root = FsPath::parse_bytes(&[t.view.as_ref()?.listing().name(idx), b"\\"].concat())?;
        self.nav.sidebar.places.drives.iter().find(|d| d.path == root)?.volume_label()
    }

    pub(super) fn begin_rename(&mut self, idx: u32, batch: bool) {
        let volume = self.drive_label(idx);
        if volume.is_none() && self.tab().view_loc.path().kind() == PathKind::Computer {
            return;
        }
        let t = &mut self.tabs.items[self.tabs.cur];
        let Some(v) = t.view.as_mut() else { return };
        if v.listing().entries[idx as usize].attrs & attr::STREAM != 0 {
            t.error = Some(STREAMS_VIEW_ONLY.t().into());
            return;
        }
        let Ok(name) = std::str::from_utf8(v.listing().name(idx)) else {
            t.error = Some(S::UiOpsNameNotEditable.t().into());
            return;
        };
        let mut edit = LineEdit::default();
        edit.set(volume.as_deref().unwrap_or(name));
        edit.anchor = 0;
        edit.caret = match &volume {
            Some(label) => label.len(),
            None => base_name_len(name, v.listing().entries[idx as usize].is_dir()),
        };
        if !batch {
            v.clear_selection();
            v.set_selected(idx, true);
        }
        v.focus = Some(idx);
        v.anchor = Some(idx);
        let gen = v.snap().gen;
        t.error = None;
        let tab = t.id;
        self.reveal_focus(false);
        self.tab_mut().scroll_x = 0.0;
        self.ops.rename = Some(Rename { tab, gen, idx, edit, scroll: 0.0, rect: Rect::default(), click: None, selecting: None, batch, confirmed: None, hint: None, volume });
        self.ops.slow = None;
        self.focus = Focus::List;
        self.dirty = true;
    }

    pub fn is_renaming(&self) -> bool {
        self.ops.rename.is_some()
    }

    pub(in crate::ui) fn rename_selection(&self) -> Option<&str> {
        let edit = &self.ops.rename.as_ref()?.edit;
        edit.text.get(edit.selection())
    }

    pub(in crate::ui) fn rename_edit(&mut self) -> Option<&mut LineEdit> {
        self.ops.rename.as_mut().map(|r| &mut r.edit)
    }

    pub(in crate::ui) fn rename_contains(&self, x: f32, y: f32) -> bool {
        self.ops.rename.as_ref().is_some_and(|r| r.rect.contains(x, y))
    }

    pub(in crate::ui) fn rename_insert(&mut self, what: Insert) {
        let Some(r) = &self.ops.rename else { return };
        let t = self.tab();
        let entry = t.view.as_ref().filter(|v| t.id == r.tab && v.snap().gen == r.gen).map(|v| v.listing().entries[r.idx as usize]);
        let off = self.utc_offset_min;
        let text = match what {
            Insert::Id => {
                self.ops.ids += 1;
                self.ops.ids.to_string()
            }
            Insert::Today => iso_date(crate::fmt::now_ticks(), off),
            Insert::Created => match entry.filter(|e| e.created > 0) {
                Some(e) => iso_date(e.created, off),
                None => return,
            },
            Insert::Modified => match entry.filter(|e| e.modified > 0) {
                Some(e) => iso_date(e.modified, off),
                None => return,
            },
        };
        if let Some(r) = &mut self.ops.rename {
            r.edit.insert(&text);
        }
        self.dirty = true;
    }

    pub(in crate::ui) fn change_case(&mut self, upper: bool) {
        let edit = match self.ops.rename.as_mut() {
            Some(r) => Some(&mut r.edit),
            None => None,
        };
        let edit = match edit {
            Some(e) => Some(e),
            None if self.focus == Focus::Filter => self.tabs.items[self.tabs.cur].filter.as_mut().map(|f| &mut f.edit),
            None => self.inspector_field(),
        };
        let Some(edit) = edit else { return };
        if edit.change_case(upper) {
            if self.focus == Focus::Filter && self.ops.rename.is_none() {
                self.refilter_tab(self.tabs.cur);
            }
            self.dirty = true;
        }
    }

    pub(super) fn rename_menu(&mut self, x: f32, y: f32) {
        let menu = self.collect(|a, m| {
            if let Some(r) = &a.ops.rename {
                crate::ui::field::text_entries(m, &r.edit, false, a.text_on_clipboard());
                m.separator();
            }
            for (id, label) in [
                ("rename.insert_id", S::ActionsRenameInsertId),
                ("rename.insert_date", S::ActionsRenameInsertDate),
                ("rename.insert_created", S::ActionsRenameInsertCreated),
                ("rename.insert_modified", S::ActionsRenameInsertModified),
            ] {
                m.action(id, label.t());
            }
            m.separator();
            m.action("text.uppercase", S::ActionsTextUppercase.t());
            m.action("text.lowercase", S::ActionsTextLowercase.t());
        });
        self.flit_menu(x, y, menu);
    }

    pub fn rename_text(&self) -> Option<&str> {
        self.ops.rename.as_ref().map(|r| r.edit.text.as_str())
    }

    pub fn rename_hint(&self) -> Option<&str> {
        self.ops.rename.as_ref().and_then(|r| r.hint.as_deref())
    }

    fn names_like_windows(&self) -> bool {
        windows_names(self.tab().view_loc.path(), &self.nav.sidebar.places.drives)
    }

    fn keep_renaming(&mut self, mut r: Rename, hint: impl Into<String>) -> bool {
        r.hint = Some(hint.into());
        self.ops.rename = Some(r);
        self.dirty = true;
        false
    }

    fn name_taken(&self, idx: u32, name: &[u8], nocase: bool) -> bool {
        let t = self.tab();
        let Some(v) = &t.view else { return false };
        let l = v.listing();
        let folded = nocase.then(|| crate::name::display(name).to_lowercase());
        let dir = t.entry_dir(idx);
        (0..l.len() as u32)
            .filter(|&i| i != idx && l.entries[i as usize].attrs & attr::DELETED == 0 && l.folder(i) == l.folder(idx))
            .any(|i| same_name(l.name(i), name, folded.as_deref()) && t.entry_dir(i) == dir)
    }

    pub(super) fn commit_rename(&mut self) -> bool {
        let Some(mut r) = self.ops.rename.take() else { return true };
        let windows = self.names_like_windows();
        let t = self.tab();
        let Some(v) = t.view.as_ref().filter(|v| t.id == r.tab && v.snap().gen == r.gen) else { return true };
        let old = v.listing().name(r.idx).to_vec();
        if let Some(volume) = r.volume.take() {
            let drive = FsPath::parse_bytes(&[old.as_slice(), b"\\"].concat());
            if let Some(drive) = drive.filter(|_| r.edit.text != volume) {
                self.effects.push(Effect::SetVolumeLabel { drive, label: r.edit.text });
            }
            return true;
        }
        let is_dir = v.listing().entries[r.idx as usize].is_dir();
        let name = match validate_name(&r.edit.text, windows) {
            Ok(n) => n,
            Err(e) => return self.keep_renaming(r, e),
        };
        if name.as_bytes() == old {
            return true;
        }
        if r.batch && v.selected_count() > 1 {
            let ext = extension(&old, is_dir);
            let base = match name.len().checked_sub(ext.len()) {
                Some(n) if n > 0 && name.as_bytes()[n..].eq_ignore_ascii_case(ext) => &name[..n],
                _ => &name,
            };
            self.batch_rename(base);
            return true;
        }
        if self.name_taken(r.idx, name.as_bytes(), windows) {
            return self.keep_renaming(r, S::UiOpsNameTaken.t());
        }
        let old_str = String::from_utf8_lossy(&old);
        if !is_dir && self.warn_extension_change && r.confirmed.as_deref() != Some(&name) && changes_extension(&old_str, &name) {
            let old_str = old_str.into_owned();
            r.confirmed = Some(name);
            self.ops.rename = Some(r);
            self.ask_extension_change(old_str);
            return false;
        }
        let dir = t.entry_dir(r.idx);
        let (item, target) = (op_path(&dir.join(&old)), op_path(&dir.join(name.as_bytes())));
        let (Some(item), Some(target)) = (item, target) else {
            self.status(S::UiOpsNameNotShell.t());
            return true;
        };
        self.tab_mut().error = None;
        let (tab, before) = (self.tab().id, self.ops.next_id);
        self.start_op_on(dir.provider(), tab, FileOp::Rename { item, name: target.file_name().to_vec() }, Origin::Fresh, Then::Select);
        if self.ops.next_id != before {
            let dir = self.tab().view_loc.clone();
            self.ops.retry = Some(Retry { id: self.ops.next_id, tab, dir, old, text: r.edit.text });
        }
        true
    }

    pub(in crate::ui) fn extension_change_answered(&mut self, yes: bool) {
        if yes {
            self.commit_rename();
        } else if let Some(r) = &mut self.ops.rename {
            r.confirmed = None;
        }
        self.dirty = true;
    }

    pub(super) fn rename_again(&mut self, retry: Retry, error: &str) -> bool {
        if self.ops.rename.is_some() || self.tab().id != retry.tab || !same_location(&retry.dir, &self.tab().view_loc) {
            return false;
        }
        let Some(v) = self.tab_mut().view.as_mut() else { return false };
        let Some(i) = v.find(&retry.old).filter(|&i| v.is_visible(i)) else { return false };
        let dir = v.listing().entries[i as usize].is_dir();
        self.begin_rename(i, false);
        let Some(r) = self.ops.rename.as_mut() else { return false };
        r.edit.set(&retry.text);
        r.edit.anchor = 0;
        r.edit.caret = base_name_len(&retry.text, dir);
        r.hint = Some(error.to_owned());
        true
    }

    pub(super) fn batch_rename(&mut self, base: &str) {
        let t = self.tab();
        let Some(v) = &t.view else { return };
        let l = v.listing();
        let live = |i: &u32| l.entries[*i as usize].attrs & attr::DELETED == 0;
        let taken: FxHashSet<Vec<u8>> = (0..l.len() as u32).filter(live).filter(|&i| !v.is_selected(i)).map(|i| l.name(i).to_ascii_lowercase()).collect();
        let items: Vec<(&[u8], bool)> = v.rows().iter().filter(|&&i| v.is_selected(i)).map(|&i| (l.name(i), l.entries[i as usize].is_dir())).collect();
        let names = batch_names(base, &items, |n| taken.contains(n));
        let dirs: Vec<Location> = v.rows().iter().filter(|&&i| v.is_selected(i)).map(|&i| t.entry_dir(i)).collect();
        let renamed = items.iter().zip(&names).zip(&dirs).filter(|(((old, _), new), _)| *old != new.as_slice());
        let pairs: Option<Vec<(FsPath, FsPath)>> = renamed.map(|(((old, _), new), dir)| Some((op_path(&dir.join(old))?, op_path(&dir.join(new))?))).collect();
        let (provider, tab) = (dirs.first().map_or(ProviderId::Local, Location::provider), t.id);
        match pairs {
            Some(p) if p.is_empty() => {}
            Some(p) => {
                self.tab_mut().error = None;
                self.start_op_on(provider, tab, FileOp::MoveTo(p), Origin::Fresh, Then::Select);
            }
            None => self.status(S::UiOpsNamesNotShell.t()),
        }
    }

    pub(in crate::ui) fn ops_key(&mut self, key: Key, m: Mods) -> bool {
        self.ops.slow = None;
        if self.ops.rename.is_some() && key == Key::Tab && !m.ctrl && !m.alt {
            self.rename_step(m.shift);
            return true;
        }
        if let Some(r) = self.ops.rename.as_ref().filter(|_| key == Key::Apps || (key == Key::F(10) && m.shift && !m.ctrl && !m.alt)) {
            let row = self.tab().view.as_ref().and_then(|v| v.row_of(r.idx));
            let rect = row.and_then(|row| self.row_rect(row)).map_or(r.rect, |c| self.name_edit_rect(c));
            self.rename_menu(rect.x, rect.bottom());
            return true;
        }
        let Some(action) = self.ops.rename.as_mut().map(|r| r.edit.key(key, m)) else { return false };
        match action {
            EditAction::Submit => {
                self.commit_rename();
            }
            EditAction::Cancel => self.ops.rename = None,
            EditAction::Copy(s) => self.effects.push(Effect::SetClipboard(s)),
            EditAction::Paste => self.effects.push(Effect::Paste),
            EditAction::Changed => {
                if let Some(r) = &mut self.ops.rename {
                    r.hint = None;
                }
            }
            EditAction::None => {}
        }
        true
    }

    fn rename_step(&mut self, back: bool) {
        let Some(idx) = self.ops.rename.as_ref().filter(|r| !r.batch).map(|r| r.idx) else {
            self.commit_rename();
            return;
        };
        let next = self.tab().view.as_ref().and_then(|v| {
            let row = v.row_of(idx)?;
            let to = if back { row.checked_sub(1)? } else { row + 1 };
            v.rows().get(to).copied()
        });
        if self.commit_rename() {
            if let Some(i) = next {
                self.begin_rename(i, false);
            }
        }
    }

    pub(in crate::ui) fn ops_char(&mut self, c: char) -> bool {
        if self.ops.rename.is_none() {
            return false;
        }
        let windows = self.names_like_windows();
        let Some(r) = &mut self.ops.rename else { return false };
        if !bad_name_char(c, windows) {
            let mut b = [0; 4];
            r.edit.insert(c.encode_utf8(&mut b));
            r.hint = None;
        } else if !c.is_control() {
            r.hint = Some(bad_chars_hint(windows).into());
            self.dirty = true;
        }
        true
    }

    pub(in crate::ui) fn ops_paste(&mut self, text: &str) -> bool {
        if self.ops.rename.is_none() {
            return false;
        }
        let windows = self.names_like_windows();
        let Some(r) = &mut self.ops.rename else { return false };
        let line = text.lines().next().unwrap_or("");
        let kept: String = line.chars().filter(|&c| !bad_name_char(c, windows)).collect();
        r.edit.insert(&kept);
        r.hint = line.chars().any(|c| bad_name_char(c, windows) && !c.is_control()).then(|| bad_chars_hint(windows).into());
        self.dirty = true;
        true
    }

    pub(in crate::ui) fn rename_ime_area(&self) -> Option<Rect> {
        self.ops.rename.as_ref().map(|r| r.rect)
    }

    pub(super) fn draw_rename<B: TextBackend>(&mut self, g: &mut Gfx<B>, th: &Theme) {
        let Some(row) = self.ops.rename.as_ref().and_then(|r| {
            let t = self.tab();
            t.view.as_ref().filter(|_| t.id == r.tab).and_then(|v| v.row_of(r.idx))
        }) else {
            return;
        };
        let list = self.layout.list;
        let Some(rect) = self.row_rect(row).map(|c| self.name_edit_rect(c)) else { return };
        if let Some(cell) = self.row_rect(row) {
            let veil = self.look.palette.area(crate::theme::Area::Files).bg.alpha(RENAME_VEIL);
            g.dl.set_clip(Some(list));
            g.dl.rect(Rect::new(list.x, list.y, list.w, (cell.y - list.y).max(0.0)), veil);
            g.dl.rect(Rect::new(list.x, cell.bottom(), list.w, (list.bottom() - cell.bottom()).max(0.0)), veil);
            g.dl.set_clip(None);
        }
        let mut th = *th;
        th.text_selection = th.accent;
        let th = &th;
        let font = self.content_font(13.0);
        let m = g.metrics(font);
        let baseline = (rect.y + (rect.h + m.ascent - m.descent) / 2.0).round();
        let (pad, radius, caret_w, inset) = (self.cs(4.0), self.cs(3.0), self.cs(1.0).max(1.0), self.cs(3.0));
        let Some(r) = self.ops.rename.as_mut() else { return };
        r.rect = rect;
        let inner = Rect::new(rect.x + pad, rect.y, rect.w - 2.0 * pad, rect.h);
        if let Some((x, how)) = r.click.take() {
            let pos = g.hit(&r.edit.text, font, x - inner.x + r.scroll);
            r.edit.grab(pos, how);
        }
        g.dl.set_clip(Some(list));
        g.dl.round_rect(rect, th.input_bg, radius);
        g.dl.outline(rect, th.accent, radius, caret_w);
        g.dl.set_clip(Some(Rect::new(inner.x, inner.y.max(list.y), inner.w, inner.h.min(list.bottom() - inner.y.max(list.y)).max(0.0))));
        let line = crate::ui::field::EditLine { text: &r.edit.text, caret: r.edit.caret, selection: r.edit.selection() };
        let look = crate::ui::field::EditLook { font, baseline, line_y: rect.y + inset, line_h: rect.h - 2.0 * inset, stroke: caret_w, focused: true };
        let preedit = Some(&self.preedit).filter(|p| !p.is_empty());
        crate::ui::field::draw_line(g, th, inner, &mut r.scroll, line, preedit, look);
        g.dl.set_clip(None);
        self.draw_rename_hint(g, th, rect);
    }

    fn draw_rename_hint<B: TextBackend>(&self, g: &mut Gfx<B>, th: &Theme, edit: Rect) {
        let Some(hint) = self.rename_hint() else { return };
        let (font, icons, list) = (self.ui_font(12.5), self.icon_font(14.0), self.layout.list);
        let m = g.metrics(font);
        let (gap, pad_x, pad_y, icon) = (self.s(HINT_GAP), self.s(HINT_PAD_X), self.s(HINT_PAD_Y), self.s(HINT_ICON));
        let room = (list.w - 2.0 * gap).max(0.0);
        let w = (pad_x / 2.0 + icon + g.measure(hint, font) + pad_x).ceil().min(room);
        let h = (m.ascent + m.descent).ceil() + 2.0 * pad_y;
        let below = edit.bottom() + gap;
        let y = if below + h <= list.bottom() || edit.y - gap - h < list.y { below } else { edit.y - gap - h };
        let x = edit.x.min(list.right() - gap - w).max(list.x + gap);
        let r = Rect::new(x.round(), y.round(), w, h);
        g.dl.round_rect(r, th.popup, self.s(crate::ui::widgets::RADIUS));
        g.dl.outline(r, th.error, self.s(crate::ui::widgets::RADIUS), 1.0);
        crate::ui::centered(g, Rect::new(r.x + pad_x / 2.0, r.y, icon, r.h), G_WARNING, icons, th.error);
        let left = r.x + pad_x / 2.0 + icon;
        g.text(left, (r.y + (r.h + m.ascent - m.descent) / 2.0).round(), hint, font, th.text, r.right() - pad_x - left);
    }
}
