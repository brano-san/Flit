use super::*;

impl App {
    pub fn on_event(&mut self, ev: &Event, now: Instant) {
        if self.tabs.tab_cycle && match ev {
            Event::Key { key, mods, pressed } => !mods.ctrl || (*pressed && *key != Key::Tab && *key != Key::Other),
            Event::Focus(false) | Event::MouseDown { .. } | Event::Wheel { .. } => true,
            _ => false,
        } {
            self.finish_tab_cycle();
        }
        if matches!(ev, Event::Focus(false)) {
            self.quick_tags = false;
        }
        if self.picker_event(ev, now) {
            return;
        }
        if let Event::WindowButtons(buttons) = ev {
            if self.tabs.panels.buttons != *buttons {
                self.tabs.panels.buttons = buttons.clone();
                self.hover = Hit::None;
                self.pressed = Hit::None;
                self.layout();
                self.dirty = true;
            }
            return;
        }
        if matches!(ev, Event::MouseDown { button: MouseButton::Middle | MouseButton::Left, .. } | Event::Key { pressed: true, .. } | Event::Focus(false)) {
            self.paste_after_layout = None;
        }
        if self.caps.unix_paths && self.middle_paste(ev, now) {
            return;
        }
        self.handle_event(ev, now);
        if self.caps.unix_paths && self.paste_after_layout.is_none() {
            self.offer_primary();
        }
    }

    pub(super) fn handle_event(&mut self, ev: &Event, now: Instant) {
        if let Event::MouseDown { x, y, button: MouseButton::Left, .. } = *ev {
            if self.touch_input {
                self.touch_event(ev, now);
                let snap = self.touch_snapshot();
                self.route_event(ev, now);
                return self.touch_pressed(x, y, snap, now);
            }
        }
        if !self.touch_event(ev, now) {
            self.route_event(ev, now);
        }
    }

    fn route_event(&mut self, ev: &Event, now: Instant) {
        if matches!(ev, Event::MouseDown { .. } | Event::Key { pressed: true, .. } | Event::Wheel { .. } | Event::Pinch(_) | Event::MouseLeave | Event::Focus(false) | Event::ShowMenu { .. }) {
            self.tip_hide();
            self.quick_hide();
        }
        if self.confirm_event(ev, now) || self.own_menu_event(ev, now) {
            return;
        }
        if self.search.active && !self.dialogs.modal() && self.search_event(ev, now) {
            return;
        }
        match ev {
            Event::Resized { width, height } => {
                self.size = (*width as f32, *height as f32);
                self.layout();
                self.dirty = true;
            }
            Event::ScaleChanged(s) => {
                self.system_scale = *s;
                self.rescale();
            }
            Event::ThemeChanged { dark } => self.set_system_dark(*dark),
            Event::Contrast(colors) => self.set_contrast(*colors),
            Event::Animations(on) => self.set_system_animations(*on),
            Event::Accent(accent) => self.set_system_accent(*accent),
            Event::TextScale(factor) => self.set_system_text_scale(*factor),
            Event::IconTheme => {
                self.icon_epoch += 1;
                self.icons.clear();
                self.dirty = true;
            }
            Event::MouseSettings { double_click, wheel_lines } => (self.double_click, self.wheel_lines) = (*double_click, *wheel_lines),
            Event::WindowButtons(_) => {}
            Event::ServerDecorations(on) => self.set_server_frames(*on),
            Event::Invoke(id) => self.a11y_invoke(*id, now),
            Event::A11yFocus(id) => self.a11y_focus(*id),
            Event::A11ySelect(id) => self.a11y_select(*id, now),
            Event::Focus(f) => {
                if *f && !self.active {
                    self.check_places();
                }
                self.active = *f;
                self.mods = Mods::default();
                if !*f {
                    self.cancel_tab_drag();
                }
                self.focused_at = f.then_some(now);
                self.ops_timer_cancel();
                self.end_autoscroll();
                self.release_header();
                self.dirty = true;
            }
            Event::MouseMove { x, y } => self.mouse_move(*x, *y, now),
            Event::MouseLeave => {
                self.hide_settings_tip();
                self.hover_pick(Hit::None, now);
                if self.hover != Hit::None {
                    self.hover = Hit::None;
                    self.dirty = true;
                }
            }
            Event::Pointer { touch } => self.touch_input = *touch,
            Event::MouseDown { x, y, button, clicks } => self.mouse_down(*x, *y, *button, *clicks, now),
            Event::MouseUp { x, y, button } => self.mouse_up(*x, *y, *button, now),
            Event::Wheel { .. } | Event::Pinch(_) => self.wheel(ev, now),
            Event::TouchCancel => {}
            Event::Key { key, mods, pressed } => {
                let copy_flipped = self.mods.ctrl != mods.ctrl;
                self.mods = *mods;
                if copy_flipped {
                    self.redock();
                }
                if *pressed {
                    self.key(*key, *mods, now);
                }
            }
            Event::Char(c) => self.char_input(*c, now),
            Event::Pasted(text) => self.on_paste(text),
            Event::PrimaryPasted(paste, text) => self.on_primary_paste(*paste, text),
            Event::Preedit(text) => {
                self.preedit.clone_from(text);
                self.dirty = true;
            }
            Event::DroppedFiles(paths) => {
                let routes: Vec<Route> = paths
                    .iter()
                    .map(|p| {
                        let item = Location::from_local(p.clone());
                        match item.parent() {
                            Some(folder) => Route { select: Some(item.file_name().to_vec()), folder },
                            None => Route { folder: item, select: None },
                        }
                    })
                    .take(1)
                    .collect();
                self.dispatch(routes, OpenTarget::CurrentTab, now);
            }
            Event::CloseRequested => self.close_window(now),
            Event::ShellVerb(verb) => self.shell_verb(verb, now),
            Event::ShellError(message) => self.status(message.as_str()),
            Event::MenuCommand(k) => self.menu_command(*k, now),
            Event::ShowMenu { .. } | Event::ShellItems { .. } => {}
            Event::PaletteShell { ticket, items, shell } => self.receive_palette_shell(*ticket, items, shell),
            Event::DevicesChanged => {
                self.effects.push(Effect::RefreshPlaces);
                self.each_pane(|a, k| {
                    if a.tabs.items[k].location().path().kind() == PathKind::Computer {
                        a.reload(k, now);
                    }
                });
            }
            Event::Timer => {
                self.timer_at = None;
                self.ops_timer(now);
                self.hover_timer(now);
                self.history_timer(now);
                self.strip_timer(now);
                self.tip_timer(now);
                self.quick_timer(now);
                self.expire_previous_preview(now);
                self.expire_error(now);
                self.expire_undo_bar(now);
                self.dirty |= self.shell_props_due(now);
                self.touch_timer(now);
            }
            Event::ClipboardChanged(seq) => self.clipboard_changed(*seq),
            Event::ClipboardText(text) => self.clipboard_text_changed(*text),
            Event::LinkChoices { generation, choices } => self.on_link_choices(*generation, *choices),
            Event::SessionEnding => {}
            Event::Hosted(ev) => self.on_hosted(ev.clone()),
        }
    }

    pub(super) fn set_cursor(&mut self, c: CursorIcon) {
        if self.cursor != c {
            self.cursor = c;
            self.effects.push(Effect::Cursor(c));
        }
    }

    pub(super) fn mouse_move(&mut self, x: f32, y: f32, now: Instant) {
        if self.update_scroll_move(x, y) {
            return;
        }
        self.strip_pointer(x, y);
        self.settings_tip(x, y);
        if self.autoscroll_move(self.along_of(x, y)) || self.ops_mouse_move(x, y) || self.nav_mouse_move(x) || self.marquee_move(x, y) {
            return;
        }
        match self.drag {
            Drag::Thumb { grab } => {
                let (l, max) = (&self.layout, self.max_scroll());
                let (track, thumb, at) = if l.flow.across { (l.track.x, l.track.w - l.thumb.w, x) } else { (l.track.y, l.track.h - l.thumb.h, y) };
                let frac = ((at - grab - track) / thumb.max(1.0)).clamp(0.0, 1.0);
                let t = self.tab_mut();
                t.scroll = frac * max;
                t.scroll_target = t.scroll;
                self.layout();
                self.dirty = true;
                return;
            }
            Drag::ColSep { .. } => return self.drag_column_edge(x),
            Drag::HThumb { grab } => return self.drag_hthumb(x, grab),
            Drag::ColMove { .. } => {
                self.move_header(x);
                return;
            }
            Drag::Splitter { start_x, start_w } => return self.drag_splitter(x, start_x, start_w),
            Drag::PinMove { from, .. } => return self.drag_pin(from, y),
            Drag::PlaceMove { from, .. } => {
                if x > self.nav.sidebar.rect.right() + self.s(8.0) && self.drag_place_out(from) {
                    self.drag = Drag::None;
                    return;
                }
                return self.drag_place(from, y);
            }
            Drag::Tab { .. } => return self.drag_tab(x, y),
            Drag::Network => {
                return self.set_cursor(if self.network_drop_target(x, y) { CursorIcon::Hand } else { CursorIcon::Default });
            }
            Drag::Preview { hit, start, size } => return self.preview_drag(hit, x, y, start, size),
            Drag::PanelSep { sep, start, first } => return self.drag_sep(sep, x, y, start, first),
            Drag::Slide(k) => return self.slide_setting(k, x),
            Drag::OptionsThumb { grab } => return self.drag_settings_thumb(y, grab),
            Drag::MenuMove { from, .. } => return self.drag_menu_row(from, y),
            Drag::Marquee | Drag::None => {}
        }
        if self.start_pin_drag(y) || self.move_header(x) {
            return;
        }
        let h = self.hit(x, y);
        let cursor = match h {
            Hit::ColSep(_) | Hit::Splitter => CursorIcon::ResizeHorizontal,
            Hit::QuickField | Hit::FilterField | Hit::PaletteField | Hit::Settings(settings_page::SHit::Search) => CursorIcon::Text,
            Hit::Preview(preview_pane::PaneHit::Split) => self.preview_cursor(),
            Hit::Preview(preview_pane::PaneHit::Body) if self.preview_text_at(x, y) => CursorIcon::Text,
            Hit::PanelSep(k) => self.sep_cursor(k),
            _ => CursorIcon::Default,
        };
        self.set_cursor(cursor);
        let check = match h {
            Hit::Row(r) if self.prefs.checkboxes && !self.layout.mode.rows() => Some(r),
            Hit::Row(_) => self.checkbox_row(x, y),
            _ => None,
        };
        if check != self.details.checks.hover {
            self.details.checks.hover = check;
            self.dirty = true;
        }
        let head = match h {
            Hit::Header(i) => Some((i, self.layout.columns.get(i) == Some(&Column::Type) && self.funnel_rect(self.layout.cols[i]).contains(x, y))),
            _ => None,
        };
        if head != self.head_hot {
            self.head_hot = head;
            self.dirty = true;
        }
        self.parked_hover(x, y);
        self.hover_pick(h, now);
        if h != self.hover {
            self.hover = h;
            self.tip_hover(h, now);
            self.quick_hover(h, now);
            self.dirty = true;
        }
    }

    pub(super) fn mouse_down(&mut self, x: f32, y: f32, button: MouseButton, clicks: u8, now: Instant) {
        if matches!(self.drag, Drag::Tab { .. } | Drag::Network) {
            return;
        }
        self.dirty = true;
        self.nav.sidebar.dragging_out = false;
        if self.form_down(x, y, button, clicks, now) {
            return;
        }
        if !self.settings_open() {
            self.panel_press(x, y);
        }
        if self.jobs_mouse_down(self.hit(x, y)) || self.settings_mouse_down(self.hit(x, y), (x, y), button, clicks, now) || self.strip_mouse_down(self.hit(x, y), button, clicks, now) || self.autoscroll_down(self.hit(x, y), self.along_of(x, y), button) || self.ops_mouse_down(x, y, button, clicks, now) {
            return;
        }
        let h = self.hit(x, y);
        if self.nav_mouse_down(h, (x, y), button, clicks, now) {
            return;
        }
        match (button, h) {
            (MouseButton::Back, _) => return self.go_history(false, now),
            (MouseButton::Forward, _) => return self.go_history(true, now),
            (MouseButton::Middle, Hit::Tab(i) | Hit::TabClose(i)) => return self.close_tab(i),
            (MouseButton::Middle, Hit::Header(c)) => {
                if let Some(&col) = self.layout.columns.get(c) {
                    self.toggle_column(col);
                    self.layout();
                }
                return;
            }
            (MouseButton::Middle, _) => return self.middle_down(h, x, y, now),
            (MouseButton::Left, Hit::None) if clicks == 2 && self.strip_double_click(x, y, now) => return,
            _ => {}
        }
        self.pressed = h;
        match h {
            Hit::Tab(i) => self.press_tab(i, x, y, button),
            Hit::TabScroll(right) if button == MouseButton::Left => self.press_arrow(right, (x, y), tabbar::Held::Press, now),
            Hit::Nav(i @ 0..=1) if button == MouseButton::Left => self.arm_history_press(i, now),
            Hit::Address if button == MouseButton::Left => self.open_address(),
            Hit::Row(row) => {
                self.focus = Focus::List;
                if button == MouseButton::Left && self.on_chevron(x, y, row) {
                    self.tree_toggle_row(row, self.mods.shift);
                } else if button == MouseButton::Left && self.on_checkbox(x, y) {
                    self.toggle_row(row);
                } else if button == MouseButton::Left && clicks == 1 && (self.marquee_column(x) || (self.layout.mode == LayoutMode::Tree && !self.on_checkbox(x, y))) {
                    let base = self.marquee_base();
                    self.click_row(row, button, clicks, now);
                    self.arm_marquee(x, y, self.mods.ctrl, base);
                } else if !self.press_row(row, button, clicks, x, y, now) {
                    self.click_row(row, button, clicks, now);
                }
            }
            Hit::ListEmpty => {
                self.focus = Focus::List;
                let base = self.marquee_base();
                let (ctrl, shift) = (self.mods.ctrl, self.mods.shift);
                if let Some(v) = &mut self.tab_mut().view {
                    if !ctrl && !shift {
                        v.clear_selection();
                    }
                }
                if button == MouseButton::Left {
                    self.arm_marquee(x, y, ctrl, base);
                }
            }
            Hit::Thumb => {
                let th = self.layout.thumb;
                self.drag = Drag::Thumb { grab: self.along_of(x - th.x, y - th.y) };
            }
            Hit::HThumb | Hit::HTrack => self.hbar_down(h, x),
            Hit::Track => {
                let th = self.layout.thumb;
                let dir = if self.along_of(x - th.x, y - th.y) < 0.0 { -1.0 } else { 1.0 };
                let (len, max) = (self.view_len(), self.max_scroll());
                let t = self.tab_mut();
                t.scroll_target = (t.scroll_target + dir * len).clamp(0.0, max);
            }
            Hit::Group(g) => {
                self.focus = Focus::List;
                if button == MouseButton::Left {
                    self.group_press = self.group_chevron_click(x, y).map(|_| g);
                    if self.group_press.is_none() {
                        self.select_group(g);
                    }
                }
            }
            Hit::Preview(p) => self.preview_mouse_down(p, (x, y), button, clicks, now),
            Hit::ColSep(i) if button == MouseButton::Left && clicks >= 2 => {
                self.drag = Drag::None;
                self.column_command(menu::ColumnCmd::Fit(self.layout.columns[i]));
            }
            Hit::ColSep(i) if button == MouseButton::Left => self.press_column_edge(i, x),
            Hit::Header(i) if button == MouseButton::Left => self.press_header(i, x),
            Hit::PanelSep(k) if button == MouseButton::Left => self.press_sep(k, x, y, clicks),
            _ => {}
        }
    }

    pub(super) fn mouse_up(&mut self, x: f32, y: f32, button: MouseButton, now: Instant) {
        if button != MouseButton::Left && matches!(self.drag, Drag::Tab { .. } | Drag::Network) {
            return;
        }
        if let Some(f) = &mut self.dialogs.form {
            f.document.grab = None;
        }
        if self.autoscroll_up(x, y, button) {
            return;
        }
        self.ops_mouse_up(x, y, button, now);
        self.strip_mouse_up(x, y, button);
        self.end_marquee();
        let was = std::mem::replace(&mut self.drag, Drag::None);
        let pressed = std::mem::replace(&mut self.pressed, Hit::None);
        self.dirty = true;
        self.release_header();
        if was == Drag::Network {
            let routes = std::mem::take(&mut self.ops.network_drag);
            self.set_cursor(CursorIcon::Default);
            if self.network_drop_target(x, y) {
                self.open_entries_in_tabs(routes, now);
            }
            return;
        }
        if let Drag::ColMove { col, x, .. } = was {
            return self.drop_column(col, x);
        }
        if let Drag::PinMove { from, to } = was {
            return self.drop_pin(from, to);
        }
        if let Drag::PlaceMove { from, to } = was {
            return self.drop_place(from, to);
        }
        if let Drag::MenuMove { from, to } = was {
            return self.drop_menu_row(from, to);
        }
        if let Drag::PanelSep { .. } = was {
            self.tabs.strip.rev += 1;
        }
        if let Drag::Tab { torn: true, .. } = was {
            return self.tear_off(x, y);
        }
        if let (Drag::Tab { .. }, Some(d)) = (was, self.tabs.docking.take()) {
            if let Some(z) = d.hot.and_then(|k| d.zones.get(k)) {
                return self.dock_tab(z.dock, self.mods.ctrl, now);
            }
        }
        if button != MouseButton::Left || was != Drag::None {
            return;
        }
        let h = self.hit(x, y);
        if h != pressed || matches!(h, Hit::TabScroll(_)) || self.jobs_click(h, now) {
            return;
        }
        self.click(h, x, y, now);
    }

    fn network_drop_target(&self, x: f32, y: f32) -> bool {
        match self.hit(x, y) {
            Hit::Tab(_) | Hit::TabClose(_) | Hit::NewTab => true,
            Hit::None => self.layout.title.contains(x, y),
            _ => false,
        }
    }

    pub(super) fn click(&mut self, h: Hit, x: f32, y: f32, now: Instant) {
        match h {
            Hit::TabClose(i) => self.close_tab(i),
            Hit::NewTab => self.open_new_tab(now),
            Hit::Nav(0) => self.go_history(false, now),
            Hit::Nav(1) => self.go_history(true, now),
            Hit::Nav(2) => self.go_up(now),
            Hit::Nav(_) => self.visited_menu(),
            Hit::ViewMode => self.view_mode_menu(),
            Hit::Bookmark => self.pin_current(),
            Hit::PanelMenu => self.panel_quick_menu(),
            Hit::CrumbScroll(right) => self.scroll_crumbs(right),
            Hit::TabScroll(right) => self.scroll_tabs(right),
            Hit::GoButton(k) => self.go_button(k, now),
            Hit::QuickClear => self.clear_quick_filter(),
            Hit::Caption(0) => self.effects.push(Effect::Window(WindowCmd::Minimize)),
            Hit::Caption(1) => self.effects.push(Effect::Window(WindowCmd::ToggleMaximize)),
            Hit::Caption(_) => self.close_window(now),
            Hit::Menu => self.open_main_menu(),
            Hit::QuickToggle => self.toggle_sidebar(),
            Hit::Crumb(i) if self.mods.ctrl => self.open_crumb_in_tab(i, now),
            Hit::Crumb(i) => {
                if let Some((_, p)) = self.tab().crumb(i).cloned().filter(|(_, p)| p != self.current_location()) {
                    self.navigate(p, now);
                }
            }
            Hit::CrumbChevron(i) => self.toggle_crumb(i),
            Hit::Header(c) => {
                let (col, r) = (self.layout.columns[c], self.layout.cols[c]);
                if col == Column::Type && self.funnel_rect(r).contains(x, y) {
                    self.open_types();
                } else if col == Column::Name && self.prefs.checkboxes && self.header_check(r).inset(-self.s(4.0), -self.s(4.0)).contains(x, y) {
                    self.toggle_select_all();
                } else if let Some(f) = col.field() {
                    self.sort_column(f);
                }
            }
            Hit::Group(g) => {
                if self.group_press.take() == Some(g) {
                    self.toggle_group(g);
                }
            }
            Hit::Pane(i, Some(row)) => self.open_pane_row(i, row, now),
            Hit::Encoding => self.pick_encoding(),
            Hit::Bottom(b) => self.bottom_click(b),
            Hit::Branch => self.git_branches_menu(),
            Hit::GitAbort => self.ask_git_abort(),
            Hit::GitConflicts => self.show_git_conflicts(),
            Hit::Preview(p) => self.preview_click(p),
            Hit::Bin(k) => self.bin_button(k),
            Hit::Side(i) => self.open_side_item(i, self.mods.ctrl, now),
            Hit::SidePin(i) => self.toggle_pin_item(i, now),
            Hit::SideHeader(s) => self.toggle_section(s),
            Hit::HomeItem(i) => self.open_home_item(i, self.mods.ctrl, now),
            Hit::FilterClose => self.close_filter(),
            Hit::PaletteRow(i) => self.run_palette_item(i, now),
            Hit::PaletteKeys(i) => self.palette_shortcuts(i),
            Hit::PaletteClose => self.close_palette(),
            Hit::PaletteDescribe => self.toggle_descriptions(),
            Hit::TabActions => self.open_tab_search(),
            Hit::Eject => self.eject(),
            Hit::GrantAccess => self.grant_access(),
            Hit::NetworkSettings => self.open_network_settings(),
            Hit::Settings(s) => self.settings_click(s, x, y),
            _ => {}
        }
    }

    pub(super) fn click_row(&mut self, row: usize, button: MouseButton, clicks: u8, now: Instant) {
        let m = self.mods;
        let plain = !m.ctrl && !m.shift;
        let opened_by_first_click = std::mem::take(&mut self.clicked_open);
        let entry = self.tab().view.as_ref().and_then(|v| v.rows().get(row).copied());
        if let Some(i) = entry.filter(|_| self.dialogs.picker.is_none() && button == MouseButton::Left && clicks >= 2 && !opened_by_first_click) {
            if self.modified_double_click(i, now) {
                return;
            }
        }
        let Some(v) = &mut self.tabs.items[self.tabs.cur].view else { return };
        let i = v.rows()[row];
        if button == MouseButton::Right {
            if !v.is_selected(i) {
                v.clear_selection();
                v.set_selected(i, true);
            }
            v.focus = Some(i);
            return;
        }
        if clicks >= 2 && plain && !opened_by_first_click {
            return self.open_entry(i, now);
        }
        if m.shift {
            let anchor_row = v.anchor.and_then(|a| v.row_of(a)).unwrap_or(0);
            if !m.ctrl {
                v.clear_selection();
            }
            v.select_rows(anchor_row, row);
        } else if m.ctrl {
            let on = !v.is_selected(i);
            v.set_selected(i, on);
            v.anchor = Some(i);
        } else {
            v.clear_selection();
            v.set_selected(i, true);
            v.anchor = Some(i);
        }
        v.focus = Some(i);
        if clicks == 1 && plain && self.single_click_opens(i) && !self.open_on_release(row) {
            self.clicked_open = true;
            self.open_entry(i, now);
        }
    }

    pub(super) fn single_click_opens(&self, i: u32) -> bool {
        if self.dialogs.picker.is_some() {
            return false;
        }
        let t = self.tab();
        let Some(v) = t.view.as_ref().filter(|_| !t.view_loc.is_recycle_bin()) else { return false };
        match v.listing().entries.get(i as usize).is_some_and(|e| e.is_dir()) {
            true if t.look == LayoutMode::Columns => self.single_click_columns.opens(self.touch_input),
            true => self.single_click_folders.opens(self.touch_input),
            false => self.single_click_files.opens(self.touch_input),
        }
    }

    pub(super) fn key(&mut self, key: Key, m: Mods, now: Instant) {
        self.dirty = true;
        if key == Key::Escape && self.drag == Drag::Network {
            self.drag = Drag::None;
            self.ops.network_drag.clear();
            return self.set_cursor(CursorIcon::Default);
        }
        if self.field_menu_key(key, m) || self.jobs_key(key, m) || self.settings_key(key, m) || self.ops_key(key, m) || self.strip_key(key) || self.inspector_key(key, m) {
            return;
        }
        if self.nav.palette.is_none() && self.focus == Focus::List && self.quick_tag_key(key, m) {
            return;
        }
        if self.focus_key(key, m, now) {
            return;
        }
        if self.nav_key(key, m, now) {
            return;
        }
        match key {
            Key::BrowserBack => self.go_history(false, now),
            Key::BrowserForward => self.go_history(true, now),
            Key::NumPlus | Key::NumMinus | Key::NumStar => {
                self.tree_numpad(key, m);
            }
            _ => {}
        }
    }

    fn char_input(&mut self, c: char, now: Instant) {
        if c.is_control() || self.tree_swallows(c) {
            return;
        }
        self.dirty = true;
        if self.form_insert(c.encode_utf8(&mut [0; 4])) || self.jobs_char(c) || self.settings_insert(c.encode_utf8(&mut [0; 4])) || self.ops_char(c) || self.inspector_insert(c.encode_utf8(&mut [0; 4])) {
            return;
        }
        if self.nav_insert(c.encode_utf8(&mut [0; 4])) {
            return;
        }
        if self.quick_tags && self.focus == Focus::List && ('1'..='9').contains(&c) && self.mods == Mods::default() {
            return;
        }
        if self.mods.ctrl != self.mods.alt {
            return;
        }
        if c == ' ' && !self.tab().typing(now) {
            return;
        }
        if self.focus == Focus::List && self.caps.unix_paths && !self.tab().typing(now) && (c == '/' || c == '~') {
            self.open_address();
            self.nav_insert(c.encode_utf8(&mut [0; 4]));
            self.dirty = true;
            return;
        }
        if self.settings.opts.char_key == options::CharKey::Filter && self.focus == Focus::List {
            self.open_filter();
            self.filter_insert(c.encode_utf8(&mut [0; 4]));
            return;
        }
        if self.tab_mut().type_ahead(c, now) {
            self.reveal_focus(false);
        }
    }
}
