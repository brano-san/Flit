use super::*;

impl App {
    pub(in crate::ui) fn ops_mouse_down(&mut self, x: f32, y: f32, button: MouseButton, clicks: u8, now: Instant) -> bool {
        self.ops.press = None;
        self.ops.slow = None;
        self.ops.right_drag = None;
        let Some(r) = &self.ops.rename else { return false };
        if !r.rect.contains(x, y) {
            return !self.commit_rename();
        }
        match button {
            MouseButton::Right => self.rename_menu(x, y),
            MouseButton::Left => {
                let how = Grab::of(self.text_clicks((x, y), clicks, now), self.mods.shift);
                if let Some(r) = &mut self.ops.rename {
                    match how {
                        Some(how) => r.click = Some((x, how)),
                        None => r.edit.select_all(),
                    }
                    r.selecting = how.map(Grab::dragged);
                }
            }
            _ => {}
        }
        true
    }

    pub(in crate::ui) fn forget_press(&mut self) {
        self.ops.press = None;
    }

    pub(in crate::ui) fn ops_mouse_move(&mut self, x: f32, y: f32) -> bool {
        if let Some(r) = &mut self.ops.rename {
            if let Some(how) = r.selecting {
                r.click = Some((x, how));
                self.dirty = true;
                return true;
            }
            if r.rect.contains(x, y) {
                self.set_cursor(CursorIcon::Text);
                return true;
            }
        }
        let Some(p) = &self.ops.press else { return false };
        if (x - p.x).abs().max((y - p.y).abs()) < self.s(DRAG_START) {
            return false;
        }
        let right = p.right;
        self.ops.press = None;
        let items = self.selected_items();
        let t = self.tab();
        if self.caps.unix_paths && t.view_loc.path().is_network() {
            if !right {
                self.ops.network_drag = items.into_iter().filter_map(|i| t.entry_location(i)).filter(|p| crate::ui::network::network_address(p.path()).is_some()).map(|folder| Route { folder, select: None }).collect();
                if !self.ops.network_drag.is_empty() {
                    self.drag = Drag::Network;
                    self.pressed = Hit::None;
                    self.hover = Hit::None;
                    return true;
                }
            }
            return false;
        }
        let Ok(paths) = self.item_paths(&items) else { return false };
        if paths.is_empty() {
            return false;
        }
        self.pressed = Hit::None;
        if self.archive_view().is_some() {
            self.drag = Drag::None;
            if let Some(dest) = unpacked_copies() {
                self.ops.unpack_drag = true;
                self.start_op(FileOp::Copy { items: paths, dest }, Origin::Fresh, Then::Drag);
            }
            return true;
        }
        self.drag = Drag::None;
        self.hover = Hit::None;
        self.ops.right_drag = right.then(|| paths.clone());
        self.note_drag_source(&paths);
        self.effects.push(Effect::StartDrag(paths));
        true
    }

    pub(in crate::ui) fn unpacked(&mut self, then: &Then, paths: Vec<FsPath>) {
        match then {
            _ if paths.is_empty() => {}
            Then::Clipboard => self.effects.push(Effect::SetClipboardFiles { paths, cut: false }),
            Then::Drag if std::mem::take(&mut self.ops.unpack_drag) => self.effects.push(Effect::StartDrag(paths)),
            _ => {}
        }
    }

    pub(in crate::ui) fn ops_mouse_up(&mut self, x: f32, y: f32, button: MouseButton, now: Instant) {
        self.ops.unpack_drag = false;
        if let Some(r) = &mut self.ops.rename {
            r.selecting = None;
        }
        if button == MouseButton::Right && self.drag == Drag::None && self.ops.rename.is_none() && self.ops.right_drag.is_none() {
            let hit = self.hit(x, y);
            match hit {
                Hit::Row(_) | Hit::ListEmpty | Hit::Group(_) if !self.right_release_matches(hit) => {}
                Hit::Row(_) => self.shell_menu(x, y, true, self.mods.shift),
                Hit::ListEmpty | Hit::Group(_) => self.shell_menu(x, y, false, self.mods.shift),
                Hit::Header(c) | Hit::ColSep(c) => self.header_menu(x, y, self.layout.columns.get(c).copied()),
                Hit::HeaderEmpty => self.header_menu(x, y, None),
                h @ (Hit::Side(_) | Hit::SidePin(_) | Hit::SideHeader(_) | Hit::SideEmpty) => self.side_menu(x, y, h),
                h @ (Hit::HomeItem(_) | Hit::HomeEmpty) => self.home_menu(x, y, h),
                Hit::Address => self.address_menu(x, y),
                Hit::Crumb(i) => self.crumb_menu(x, y, i),
                _ => {}
            }
        }
        let Some(p) = self.ops.press.take().filter(|p| (p.deferred || p.open || p.toggle) && button == MouseButton::Left) else { return };
        if self.hit(x, y) != Hit::Row(p.row) {
            return;
        }
        if p.toggle {
            if let Some(v) = &mut self.tabs.items[self.tabs.cur].view {
                if let Some(&i) = v.rows().get(p.row) {
                    v.set_selected(i, false);
                }
            }
            self.dirty = true;
            return;
        }
        if !p.deferred {
            let Some(i) = self.tab().view.as_ref().and_then(|v| v.rows().get(p.row).copied()) else { return };
            self.clicked_open = true;
            return self.open_entry(i, now);
        }
        let (tab, wait) = (self.tab().id, self.double_click);
        let Some(v) = &mut self.tabs.items[self.tabs.cur].view else { return };
        let Some(&i) = v.rows().get(p.row) else { return };
        v.clear_selection();
        v.set_selected(i, true);
        v.anchor = Some(i);
        v.focus = Some(i);
        let gen = v.snap().gen;
        if self.single_click_opens(i) {
            self.clicked_open = true;
            return self.open_entry(i, now);
        }
        if p.slow {
            self.ops.slow = Some(Slow { tab, gen, idx: i, at: now + wait });
            self.arm_timer(now, now + wait);
        }
    }

    pub(in crate::ui) fn ops_idle(&self) -> bool {
        self.ops.rename.is_none() && self.ops.press.is_none() && self.ops.dropping.is_none()
    }

    pub(in crate::ui) fn ops_timer_cancel(&mut self) {
        self.ops.slow = None;
    }

    pub(in crate::ui) fn ops_timer(&mut self, now: Instant) {
        let Some(s) = self.ops.slow.take() else { return };
        if now + TIMER_SLACK < s.at {
            self.arm_timer(now, s.at);
            self.ops.slow = Some(s);
            return;
        }
        let t = self.tab();
        let still = t.id == s.tab && t.view.as_ref().is_some_and(|v| v.snap().gen == s.gen && v.selected_count() == 1 && v.is_selected(s.idx));
        if still && self.rename_target() == Some(s.idx) {
            self.begin_rename(s.idx, false);
        }
    }

    fn right_release_matches(&self, hit: Hit) -> bool {
        let v = self.tab().view.as_ref();
        match (self.pressed, hit) {
            (Hit::Row(_), Hit::Row(r)) => v.is_some_and(|v| v.focus.is_some() && v.rows().get(r).copied() == v.focus),
            (Hit::Row(_), _) | (Hit::ListEmpty | Hit::Group(_), Hit::Row(_)) => false,
            _ => true,
        }
    }

    pub(in crate::ui) fn open_on_release(&mut self, row: usize) -> bool {
        let Some(p) = self.ops.press.as_mut().filter(|p| p.row == row) else { return false };
        p.open = true;
        true
    }

    pub(in crate::ui) fn press_row(&mut self, row: usize, button: MouseButton, clicks: u8, x: f32, y: f32, now: Instant) -> bool {
        if button == MouseButton::Right {
            self.ops.press = Some(Press { row, x, y, deferred: false, slow: false, open: false, toggle: false, right: true });
            return false;
        }
        if button != MouseButton::Left || clicks >= 2 {
            return false;
        }
        let m = self.mods;
        let settled = self.focused_at.is_none_or(|t| now.duration_since(t) >= self.double_click);
        let on_name = self.on_name(x, y, row);
        let Some(v) = &mut self.tabs.items[self.tabs.cur].view else { return false };
        let Some(&i) = v.rows().get(row) else { return false };
        let deferred = !m.ctrl && !m.shift && v.is_selected(i);
        let toggle = m.ctrl && !m.shift && v.is_selected(i);
        if deferred || toggle {
            v.focus = Some(i);
            v.anchor = Some(i);
        }
        let slow = deferred && settled && on_name && v.selected_count() == 1;
        self.ops.press = Some(Press { row, x, y, deferred, slow, open: false, toggle, right: false });
        deferred || toggle
    }

    pub fn drag(&mut self, ev: &DragEvent, now: Instant) -> u32 {
        if let DragEvent::DropRepository { x, y, url } = ev {
            if let Some(Target::Into { dest, spot }) = self.drop_target(*x, *y) {
                let tab = match spot {
                    Spot::Tab(i) => &self.tabs.items[i],
                    Spot::Row(r) => {
                        let t = self.tab();
                        let is_dir = t.view.as_ref().and_then(|v| v.rows().get(r).map(|&i| v.listing().entries[i as usize].is_dir()));
                        if is_dir != Some(true) {
                            return drop_effect::NONE;
                        }
                        t
                    }
                    _ => self.tab(),
                };
                if self.ask_clone_in(tab.id, dest, url) {
                    self.drag(&DragEvent::Leave, now);
                    return drop_effect::COPY;
                }
            }
            return drop_effect::NONE;
        }
        let normalized;
        let (ev, data) = match ev {
            DragEvent::DropData { x, y, data } => {
                normalized = DragEvent::Drop { x: *x, y: *y, mods: Mods::default(), allowed: drop_effect::COPY };
                (&normalized, Some(data))
            }
            _ => (ev, None),
        };
        self.dirty = true;
        self.drag_panel(ev);
        if data.is_some() && (!self.caps.unix_paths || self.tab().location().provider() != ProviderId::Local) {
            return self.drag(&DragEvent::Leave, now);
        }
        self.strip_drag(ev, now);
        match ev {
            DragEvent::Enter { paths, x, y, mods, allowed, right } => {
                let ours = !paths.is_empty() && self.ops.right_drag.as_ref() == Some(paths);
                self.ops.admin_drag.take_if(|own| own != paths);
                self.ops.dropping = Some(Dropping { paths: paths.clone(), target: None, effect: drop_effect::NONE, spring: None, right: *right || ours, ours, onto: None });
                self.ops.drag_scroll = Some((*x, *y, *mods, *allowed));
                self.drag_over(*x, *y, *mods, *allowed)
            }
            DragEvent::Over { x, y, mods, allowed } => {
                self.spring_load(*x, *y, now);
                self.ops.drag_scroll = Some((*x, *y, *mods, *allowed));
                self.drag_over(*x, *y, *mods, *allowed)
            }
            DragEvent::DropData { .. } | DragEvent::DropRepository { .. } => drop_effect::NONE,
            DragEvent::Leave => {
                self.nav.sidebar.dragging_out = false;
                self.ops.dropping = None;
                self.ops.drag_scroll = None;
                self.ops.admin_drag = None;
                drop_effect::NONE
            }
            DragEvent::Drop { x, y, mods, allowed } => {
                self.ops.drag_scroll = None;
                let effect = self.drag_over(*x, *y, *mods, *allowed);
                let Some(d) = self.ops.dropping.take() else { return drop_effect::NONE };
                self.nav.sidebar.dragging_out = false;
                let admin = self.ops.admin_drag.take().is_some_and(|own| own == d.paths);
                if d.ours {
                    self.ops.right_drag = None;
                }
                match d.target {
                    Some(Target::Reveal) if effect != drop_effect::NONE => self.on_event(&Event::DroppedFiles(d.paths), now),
                    Some(Target::NewTab) if effect != drop_effect::NONE => {
                        let routes = d.paths.into_iter().map(|p| Route { folder: Location::from_local(p), select: None }).collect();
                        self.dispatch(routes, OpenTarget::NewTab, now);
                    }
                    Some(Target::Trash(_)) if effect != drop_effect::NONE && admin => self.delete_or_ask(ProviderId::Admin, FileOp::Delete(d.paths)),
                    Some(Target::Trash(_)) if effect != drop_effect::NONE => self.start_op(FileOp::Trash(d.paths), Origin::Fresh, Then::Nothing),
                    Some(Target::Pin) if effect != drop_effect::NONE => self.effects.push(Effect::PinFolders(d.paths)),
                    Some(Target::Run { program, .. }) if effect != drop_effect::NONE => {
                        let tab = self.tab().id;
                        let launch = Launch::With { program, args: d.paths };
                        self.effects.push(Effect::Provider { provider: ProviderId::Local, req: ProviderRequest::Launch { tab, launch } });
                    }
                    Some(Target::Into { dest, .. }) if d.paths.is_empty() => {
                        if data.is_some() && dest.kind() != PathKind::Unix {
                            return drop_effect::NONE;
                        }
                        if (!self.caps.unix_paths || data.is_some()) && matches!(effect, drop_effect::COPY | drop_effect::MOVE) {
                            self.start_op(FileOp::PasteDropped { dest, moving: effect == drop_effect::MOVE, data: data.cloned() }, Origin::Fresh, Then::Select);
                        }
                    }
                    Some(Target::Into { dest, .. }) if d.right && effect != drop_effect::NONE => {
                        let choices = self.drop_choices(&d.paths, d.ours, &dest, *allowed);
                        self.ask_drop(Asked { paths: d.paths, dest, admin }, choices, effect, (*x, *y));
                        return allowed & drop_effect::COPY;
                    }
                    Some(Target::Into { dest, .. }) => {
                        let (items, dest) = (d.paths, dest);
                        let op = match effect {
                            drop_effect::COPY => FileOp::Copy { items, dest },
                            drop_effect::MOVE => FileOp::Move { items, dest },
                            drop_effect::LINK => FileOp::Link { items, dest },
                            _ => return drop_effect::NONE,
                        };
                        self.start_dropped(op, admin);
                    }
                    _ => {}
                }
                effect
            }
        }
    }

    fn drop_target(&self, x: f32, y: f32) -> Option<Target> {
        let t = self.tab();
        let into = |loc: &Location, spot: Spot| match loc.path().kind() {
            PathKind::RecycleBin => Some(Target::Trash(spot)),
            PathKind::Computer => None,
            _ if library::is_library(loc.path()) => self.library_save(loc.path()).map(|dest| Target::Into { dest, spot }),
            _ => loc.to_windows().map(|dest| Target::Into { dest, spot }),
        };
        let here = || into(&t.view_loc, Spot::List);
        match self.hit(x, y) {
            Hit::Row(_) if self.in_bin() => here(),
            Hit::Row(row) => {
                let v = t.view.as_ref()?;
                let i = *v.rows().get(row)?;
                let runnable = self.takes_dropped_files(v.listing(), i);
                if v.listing().entries[i as usize].is_dir() || archive::browsable(v.listing().name(i)).is_some_and(Format::can_add) {
                    into(&t.entry_location(i)?, Spot::Row(row))
                } else if runnable {
                    t.entry_location(i)?.to_windows().map(|program| Target::Run { program, spot: Spot::Row(row) })
                } else {
                    here()
                }
            }
            Hit::ListEmpty | Hit::Group(_) | Hit::Track | Hit::Thumb | Hit::HTrack | Hit::HThumb => here(),
            Hit::Tab(i) | Hit::TabClose(i) => into(self.tabs.items.get(i)?.location(), Spot::Tab(i)),
            Hit::Crumb(i) => into(&t.crumb(i)?.1, Spot::Crumb(i)),
            Hit::Side(i) | Hit::SidePin(i) if self.nav.sidebar.dragging_out && self.nav.sidebar.items.get(i).is_some_and(|it| it.section == Section::Bookmarks) => Some(Target::Pin),
            Hit::Side(i) | Hit::SidePin(i) if self.nav.sidebar.items.get(i).is_some_and(|it| !it.offline && it.glyph != crate::ui::G_FILE) => into(&self.nav.sidebar.items.get(i)?.loc, Spot::Side(i)),
            Hit::Side(_) | Hit::SidePin(_) => None,
            Hit::NewTab => Some(Target::NewTab),
            Hit::Header(_) | Hit::HeaderEmpty | Hit::ColSep(_) | Hit::Encoding | Hit::Bin(_) => None,
            Hit::SideHeader(Section::Bookmarks) | Hit::SideEmpty => Some(Target::Pin),
            Hit::SideHeader(_) | Hit::HomeItem(_) | Hit::HomeEmpty => None,
            Hit::Splitter | Hit::FilterField | Hit::FilterClose | Hit::PanelSep(_) | Hit::Menu | Hit::QuickToggle => None,
            Hit::PaletteRow(_) | Hit::PaletteKeys(_) | Hit::PaletteField | Hit::PaletteBox | Hit::PaletteOutside | Hit::PaletteClose | Hit::PaletteDescribe | Hit::PaletteScroll(_) => None,
            Hit::OpsButton | Hit::OpsPanel | Hit::OpsRow(_) | Hit::OpsClear | Hit::OpsCancel(_) | Hit::OpsPause(_) | Hit::OpsMore(_) | Hit::OpsMenu(_) => None,
            Hit::Dialog | Hit::DialogButton(_) | Hit::DialogCheck | Hit::Form(_) | Hit::Modal | Hit::Preview(_) => None,
            _ if self.layout.status.contains(x, y) => None,
            _ => Some(Target::Reveal),
        }
    }

    fn drag_over(&mut self, x: f32, y: f32, m: Mods, allowed: u32) -> u32 {
        use drop_effect::*;
        let target = self.drop_target(x, y);
        let onto = self.onto_file(x, y);
        let Some(d) = &self.ops.dropping else { return NONE };
        let effect = match &target {
            None => NONE,
            Some(Target::Into { dest, .. }) if d.paths.is_empty() && archive::split(dest).is_none() => choose_effect(allowed & (COPY | MOVE), m, false),
            Some(_) if d.paths.is_empty() => NONE,
            Some(Target::Reveal | Target::NewTab) => [COPY, LINK].into_iter().find(|e| allowed & e != 0).unwrap_or(NONE),
            Some(Target::Trash(_)) if d.paths.is_empty() || d.paths.iter().any(recycle::in_bin) => NONE,
            Some(Target::Trash(_)) => allowed & MOVE,
            Some(Target::Run { program, .. }) if d.paths.is_empty() || d.paths.contains(program) => NONE,
            Some(Target::Run { .. }) => [COPY, LINK, MOVE].into_iter().find(|e| allowed & e != 0).unwrap_or(NONE),
            Some(Target::Pin) if d.paths.is_empty() => NONE,
            Some(Target::Pin) => [LINK, COPY, MOVE].into_iter().find(|e| allowed & e != 0).unwrap_or(NONE),
            Some(Target::Into { dest, .. }) => {
                if d.paths.iter().any(|p| p.same(dest) || dest.is_inside(p)) {
                    NONE
                } else if archive::split(dest).is_some() {
                    allowed & COPY
                } else {
                    let e = choose_effect(allowed, m, d.paths.iter().all(|p| same_volume(p, dest, &self.nav.sidebar.places.drives)));
                    let all_here = d.paths.iter().all(|p| p.parent().is_some_and(|q| q.same(dest)));
                    if d.right {
                        let choices = self.drop_choices(&d.paths, d.ours, dest, allowed);
                        [e, COPY, MOVE, LINK].into_iter().find(|c| choices & c != 0).unwrap_or(NONE)
                    } else if e == MOVE && all_here {
                        NONE
                    } else {
                        e
                    }
                }
            }
        };
        if let Some(d) = &mut self.ops.dropping {
            d.target = target;
            d.effect = effect;
            d.onto = onto.filter(|f| !d.right && !d.paths.contains(f));
        }
        effect
    }

    fn drop_choices(&self, paths: &[FsPath], ours: bool, dest: &FsPath, allowed: u32) -> u32 {
        use drop_effect::*;
        let mut choices = if ours { COPY | MOVE | LINK } else { allowed & (COPY | MOVE | LINK) };
        if archive::split(dest).is_some() {
            choices &= COPY;
        }
        if paths.iter().all(|p| p.parent().is_some_and(|q| q.same(dest))) {
            choices &= !MOVE;
        }
        if !self.caps.supports("file.shortcut") {
            choices &= !LINK;
        }
        choices
    }

    pub(super) fn ask_drop(&mut self, asked: Asked, choices: u32, default: u32, (x, y): (f32, f32)) {
        self.ops.link_probe = None;
        if !self.caps.unix_paths && choices & drop_effect::LINK != 0 && archive::split(&asked.dest).is_none() {
            self.probe_links(asked.dest.clone(), Some(asked.paths.clone()));
        }
        self.show_drop_menu(asked, choices, default, (x, y));
    }

    fn show_drop_menu(&mut self, asked: Asked, choices: u32, default: u32, (x, y): (f32, f32)) {
        use drop_effect::*;
        let link = self.caps.supports("file.shortcut").then_some(if self.caps.unix_paths { S::UiDropLinkHere } else { S::UiDropShortcutHere });
        let menu = self.collect(|a, m| {
            for (effect, label) in [(COPY, Some(S::UiDropCopyHere)), (MOVE, Some(S::UiDropMoveHere)), (LINK, link)] {
                if let Some(label) = label {
                    m.entry(Cmd::Drop(effect), label.t(), String::new(), Mark::None, effect == default, choices & effect == 0);
                }
            }
            if !a.caps.unix_paths && archive::split(&asked.dest).is_none() {
                for (kind, label) in LINK_KINDS.into_iter().skip(1) {
                    let enabled = choices & LINK != 0 && a.ops.link_probe.as_ref().is_some_and(|p| p.dest == asked.dest && p.items.as_ref() == Some(&asked.paths) && p.choices.allows(kind));
                    m.entry(Cmd::DropAs(kind), label.t(), String::new(), Mark::None, false, !enabled);
                }
            }
            m.separator();
            m.entry(Cmd::Drop(NONE), S::UiButtonCancel.t(), String::new(), Mark::None, false, false);
        });
        self.ops.asked = Some(asked);
        self.flit_menu(x, y, menu);
    }

    pub(in crate::ui) fn drop_chosen(&mut self, effect: u32) {
        let Some(Asked { paths: items, dest, admin }) = self.ops.asked.take() else { return };
        let op = match effect {
            drop_effect::COPY => FileOp::Copy { items, dest },
            drop_effect::MOVE => FileOp::Move { items, dest },
            drop_effect::LINK => FileOp::Link { items, dest },
            _ => return,
        };
        self.start_dropped(op, admin);
    }

    fn start_dropped(&mut self, op: FileOp, admin: bool) {
        let provider = if admin { ProviderId::Admin } else { ProviderId::Local };
        self.start_op_on(provider, self.tab().id, op, Origin::Fresh, Then::Select);
    }

    pub(in crate::ui) fn drop_as(&mut self, kind: LinkKind) {
        let Some(asked) = self.ops.asked.take() else { return };
        if self.ops.link_probe.as_ref().is_some_and(|p| p.tab == self.tab().id && p.location == *self.current_location() && p.dest == asked.dest && p.items.as_ref() == Some(&asked.paths) && p.choices.allows(kind)) {
            let admin = asked.admin;
            self.start_dropped(FileOp::LinkAs { items: asked.paths, dest: asked.dest, kind }, admin);
        }
    }

    fn spring_spot(&self, x: f32, y: f32) -> Option<Spot> {
        let t = self.tab();
        match self.hit(x, y) {
            Hit::Tab(i) | Hit::TabClose(i) if i != self.tabs.cur => Some(Spot::Tab(i)),
            Hit::Crumb(i) if i + 1 != t.crumbs.len() => Some(Spot::Crumb(i)),
            Hit::Row(row) if !self.in_bin() => {
                let v = t.view.as_ref()?;
                v.listing().entries[*v.rows().get(row)? as usize].is_dir().then_some(Spot::Row(row))
            }
            Hit::Side(i) | Hit::SidePin(i) if self.nav.sidebar.items.get(i).is_some_and(|it| !it.offline && it.glyph != crate::ui::G_FILE && it.loc != *self.current_location()) => Some(Spot::Side(i)),
            Hit::SideHeader(s) if self.nav.sidebar.is_collapsed(s) => Some(Spot::SideHeader(s)),
            _ => None,
        }
    }

    fn spring_load(&mut self, x: f32, y: f32, now: Instant) {
        if self.spring_delay_ms == 0 {
            return;
        }
        let delay = Duration::from_millis(self.spring_delay_ms.into());
        let spot = self.spring_spot(x, y);
        let Some(d) = &mut self.ops.dropping else { return };
        let due = match (spot, d.spring) {
            (Some(s), Some((held, since))) if s == held => now.duration_since(since) >= if matches!(s, Spot::SideHeader(_)) { SECTION_SPRING_DELAY.min(delay) } else { delay },
            _ => {
                d.spring = spot.map(|s| (s, now));
                false
            }
        };
        if !due {
            return;
        }
        d.spring = None;
        let t = self.tab();
        let folder = match spot {
            Some(Spot::Tab(i)) => return self.select_tab(i),
            Some(Spot::SideHeader(s)) => return self.toggle_section(s),
            Some(Spot::Side(i)) => self.nav.sidebar.items.get(i).map(|it| it.loc.clone()),
            Some(Spot::Crumb(i)) => t.crumb(i).map(|c| c.1.clone()),
            Some(Spot::Row(row)) => t.view.as_ref().and_then(|v| t.entry_location(*v.rows().get(row)?)),
            _ => None,
        };
        if let Some(folder) = folder {
            self.navigate(folder, now);
        }
    }

    pub fn drop_spot(&self) -> Option<Spot> {
        let d = self.ops.dropping.as_ref().filter(|d| d.effect != drop_effect::NONE)?;
        match d.target.as_ref()? {
            Target::Into { spot, .. } | Target::Run { spot, .. } | Target::Trash(spot) => Some(*spot),
            Target::NewTab => Some(Spot::NewTab),
            Target::Pin => Some(Spot::SideHeader(Section::Bookmarks)),
            Target::Reveal => None,
        }
    }

    fn onto_file(&self, x: f32, y: f32) -> Option<FsPath> {
        if self.caps.unix_paths || self.in_bin() || self.archive_view().is_some() {
            return None;
        }
        let Hit::Row(row) = self.hit(x, y) else { return None };
        let t = self.tab();
        let v = t.view.as_ref()?;
        let i = *v.rows().get(row)?;
        let l = v.listing();
        let runnable = RUNNABLE.iter().any(|e| l.ext(i).eq_ignore_ascii_case(e));
        if l.entries[i as usize].is_dir() || runnable || archive::browsable(l.name(i)).is_some_and(Format::can_add) {
            return None;
        }
        t.entry_location(i)?.to_windows().filter(|p| matches!(p.kind(), PathKind::Drive | PathKind::Unc))
    }

    pub fn drop_onto(&self) -> Option<FsPath> {
        self.ops.dropping.as_ref()?.onto.clone()
    }

    pub fn drop_folder(&self) -> Option<FsPath> {
        let d = self.ops.dropping.as_ref().filter(|d| matches!(d.effect, drop_effect::COPY | drop_effect::MOVE))?;
        match d.target.as_ref()? {
            Target::Into { dest, .. } if archive::split(dest).is_none() => Some(dest.clone()),
            _ => None,
        }
    }

    pub fn drop_caption(&self) -> Option<(String, String)> {
        let d = self.ops.dropping.as_ref().filter(|d| d.effect != drop_effect::NONE)?;
        let name = |p: &FsPath| crate::name::display(p.file_name()).into_owned();
        let (verb, target) = match d.target.as_ref()? {
            Target::Into { dest, .. } if d.effect == drop_effect::COPY => (S::UiDropCopy, name(dest)),
            Target::Into { dest, .. } if d.effect == drop_effect::LINK => (S::UiDropLink, name(dest)),
            Target::Into { dest, .. } => (S::UiDropMove, name(dest)),
            Target::Trash(_) => (S::UiDropMove, S::PathRecycleBin.t().to_owned()),
            Target::Run { program, .. } => (S::UiDropOpenWith, name(program)),
            Target::NewTab if d.paths.len() == 1 => (S::UiDropNewTab, name(&d.paths[0])),
            Target::NewTab => (S::UiDropNewTabs, String::new()),
            Target::Pin => (S::UiDropPin, S::UiDropSidebar.t().to_owned()),
            Target::Reveal => return None,
        };
        Some((verb.t().to_owned(), target))
    }

    pub(in crate::ui) fn draw_drop_panel<B: TextBackend>(&self, g: &mut Gfx<B>, th: &Theme) {
        if self.drop_spot() == Some(Spot::List) {
            let panel = self.panel_rect();
            g.dl.rect(panel, th.teal.alpha(DROP_FILL));
            g.dl.outline(panel.inset(1.0, 1.0), th.teal, self.s(4.0), self.s(2.0).max(1.0));
        }
    }

    pub(in crate::ui) fn drag_autoscroll(&mut self, dt: f32) -> bool {
        let Some((x, y, mods, allowed)) = self.ops.drag_scroll else { return false };
        let over = |at: f32, lo: f32, hi: f32| {
            const EDGE: f32 = 40.0;
            if at < lo + EDGE {
                at - lo - EDGE
            } else if at > hi - EDGE {
                at - (hi - EDGE)
            } else {
                0.0
            }
        };
        let mut moved = false;
        let list = self.layout.list;
        if list.w > 0.0 && list.inset(-4.0, 0.0).contains(x, y) {
            let out = over(y, list.y, list.bottom());
            if out != 0.0 {
                let max = self.max_scroll();
                let t = self.tab_mut();
                let next = (t.scroll_target + out * DRAG_SCROLL_RATE * dt).clamp(0.0, max);
                moved |= next != t.scroll_target;
                t.scroll_target = next;
            }
        }
        let sb = &mut self.nav.sidebar;
        if sb.rect.w > 0.0 && sb.list.h > 0.0 && sb.list.contains(x, y) {
            let out = over(y, sb.list.y, sb.list.bottom());
            if out != 0.0 {
                let before = sb.scroll.target;
                sb.scroll.by(-out * DRAG_SCROLL_RATE * dt, sb.content_h - sb.list.h);
                moved |= sb.scroll.target != before;
            }
        }
        if moved {
            self.dirty = true;
            self.drag_over(x, y, mods, allowed);
        }
        moved
    }
}
