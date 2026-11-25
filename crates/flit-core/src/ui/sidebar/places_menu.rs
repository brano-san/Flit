use super::*;

impl App {
    pub(in crate::ui) fn place_command(&mut self, c: PlaceCmd, now: Instant) {
        let tab = self.tab().id;
        let launch = |launch| Effect::Provider { provider: ProviderId::Local, req: ProviderRequest::Launch { tab, launch } };
        match c {
            PlaceCmd::Open(Location::Local(path), _) if self.mount_drive(&path) => {}
            PlaceCmd::Open(folder, target) => self.dispatch(vec![Route { folder, select: None }], target, now),
            PlaceCmd::Mount(device) => {
                self.mount_drive(&device);
            }
            PlaceCmd::OpenFile(path) => self.effects.push(Effect::Provider { provider: ProviderId::Local, req: ProviderRequest::Open { tab, path } }),
            PlaceCmd::Reveal(path) => {
                let item = Location::from_local(path);
                if let Some(folder) = item.parent() {
                    self.dispatch(vec![Route { select: Some(item.file_name().to_vec()), folder }], OpenTarget::CurrentTab, now);
                }
            }
            PlaceCmd::CopyPath(loc) => self.effects.push(Effect::SetClipboard(loc.display().into_owned())),
            PlaceCmd::Terminal(dir) => self.effects.push(launch(Launch::Terminal { dirs: vec![dir], admin: false })),
            PlaceCmd::Pin(loc) => self.toggle_pin(loc),
            PlaceCmd::MovePin(loc, by) => self.move_pin(loc, by),
            PlaceCmd::MovePlace(loc, by) => {
                let items = self.nav.sidebar.section_items(Section::Places);
                if let Some(from) = items.iter().position(|it| it.loc == loc) {
                    let to = (from as isize + if by > 0 { 2 } else { -1 }).clamp(0, items.len() as isize) as usize;
                    self.drop_place(from, to);
                }
            }
            PlaceCmd::HidePlace(loc) => {
                self.nav.sidebar.prefs.quick.places_hidden.push(loc);
                self.nav_prefs_changed();
            }
            PlaceCmd::RestorePlaces => {
                self.nav.sidebar.prefs.quick.places.clear();
                self.nav.sidebar.prefs.quick.places_hidden.clear();
                self.nav_prefs_changed();
            }
            PlaceCmd::Eject(drive) => self.effects.push(launch(Launch::Eject(drive))),
            PlaceCmd::Label(drive) => self.ask_label(drive),
            PlaceCmd::Launch(l) => self.effects.push(launch(l)),
            PlaceCmd::Properties(path) if self.caps.unix_paths => self.properties_for(Some(path)),
            PlaceCmd::Properties(path) => self.effects.push(Effect::Properties(vec![path])),
            PlaceCmd::FileProperties(path) if self.caps.unix_paths => self.properties_of(Some((path, false))),
            PlaceCmd::FileProperties(path) => self.effects.push(Effect::Properties(vec![path])),
            PlaceCmd::Hide(s) => self.toggle_section_shown(s),
            PlaceCmd::Compact => self.toggle_compact(),
            PlaceCmd::NewLibrary => self.ask_new_library(),
            PlaceCmd::RestoreLibraries => self.effects.push(Effect::Library(LibraryOp::RestoreDefaults)),
            PlaceCmd::Library(path) => self.library_dialog(path),
            PlaceCmd::EmptyBin => self.empty_bin(),
            PlaceCmd::RestoreAll => self.restore_all(),
            PlaceCmd::Widget(w) => self.toggle_widget(w),
            PlaceCmd::Forget(path) => self.forget_recent(path),
            PlaceCmd::ClearRecent => self.clear_recent(),
            PlaceCmd::Pane(loc, down) => self.open_in_panel(loc, down, now),
            PlaceCmd::Fold(collapsed) => self.fold_sections(collapsed),
            PlaceCmd::ShowPaths => {
                let q = &mut self.nav.sidebar.prefs.quick;
                q.show_paths = !q.show_paths;
                self.nav_prefs_changed();
            }
            PlaceCmd::Shift(s, by) => self.shift_section(s, by),
            PlaceCmd::ViewMode(mode) => self.set_view_mode(mode),
            PlaceCmd::DefaultView => self.set_default_view(),
            PlaceCmd::RestoreOrder => {
                self.nav.sidebar.prefs.quick.order.clear();
                self.nav_prefs_changed();
            }
        }
    }

    pub(in crate::ui) fn place_menu(&mut self, x: f32, y: f32, loc: Location, extra: impl FnOnce(&Self, &mut Menu)) {
        let path = loc.to_windows();
        let lib = library::is_library(loc.path());
        let offline = self.nav.sidebar.places.offline_location(&loc);
        let menu = self.collect(|a, m| {
            if offline {
                let open = if a.nav.sidebar.state.files.contains(&loc) { PlaceCmd::OpenFile(loc.path().clone()) } else { PlaceCmd::Open(loc.clone(), OpenTarget::CurrentTab) };
                m.place(S::SidebarDriveLoginConnect.t(), open, false);
            } else {
                m.submenu(S::MenuOpenIn.t(), |m| {
                    m.place(S::MenuOpen.t(), PlaceCmd::Open(loc.clone(), OpenTarget::CurrentTab), false);
                    m.place(S::MenuOpenInNewTab.t(), PlaceCmd::Open(loc.clone(), OpenTarget::NewTab), false);
                    m.place(S::MenuOpenInNewWindow.t(), PlaceCmd::Open(loc.clone(), OpenTarget::NewWindow), false);
                    a.place_pane_items(m, &loc);
                });
            }
            m.separator();
            if let Some(p) = &path {
                m.place(S::MenuCopyPath.t(), PlaceCmd::CopyPath(loc.clone()), false);
                if !lib && !offline {
                    m.place(S::MenuOpenInTerminal.t(), PlaceCmd::Terminal(p.clone()), false);
                }
            }
            m.separator();
            let pinned = &a.nav.sidebar.state.pinned;
            if !loc.is_recycle_bin() {
                m.place(if pinned.contains(&loc) { S::MenuUnpinFromSidebar.t() } else { S::MenuPinToSidebar.t() }, PlaceCmd::Pin(loc.clone()), false);
            }
            if let Some(i) = pinned.iter().position(|l| *l == loc) {
                if i > 0 {
                    m.place(S::SidebarMoveUp.t(), PlaceCmd::MovePin(loc.clone(), -1), false);
                }
                if i + 1 < pinned.len() {
                    m.place(S::SidebarMoveDown.t(), PlaceCmd::MovePin(loc.clone(), 1), false);
                }
            }
            let places = a.nav.sidebar.section_items(Section::Places);
            if let Some(i) = places.iter().position(|it| it.loc == loc) {
                m.separator();
                if i > 0 {
                    m.place(S::SidebarMoveUp.t(), PlaceCmd::MovePlace(loc.clone(), -1), false);
                }
                if i + 1 < places.len() {
                    m.place(S::SidebarMoveDown.t(), PlaceCmd::MovePlace(loc.clone(), 1), false);
                }
                m.place(S::WidgetsRemoveItem.t(), PlaceCmd::HidePlace(loc.clone()), false);
            }
            if a.nav.sidebar.places.drives.iter().any(|d| d.removable && !d.unmounted && d.path == *loc.path()) {
                m.place(S::MenuEject.t(), PlaceCmd::Eject(loc.path().clone()), false);
            }
            if !offline {
                a.drive_items(m, loc.path());
            }
            if a.side_label(loc.path()).is_some() {
                m.place(S::MenuRename.t(), PlaceCmd::Label(loc.path().clone()), false);
            }
            if a.supports("drive.disconnect") && a.nav.sidebar.places.drives.iter().any(|d| d.remote.is_some() && d.path == *loc.path() && d.path.kind() == PathKind::Drive) {
                m.place(S::SidebarDriveDisconnect.t(), PlaceCmd::Launch(Launch::Disconnect(Some(loc.path().clone()))), false);
            }
            if lib {
                m.place(S::SidebarLibraryLocations.t(), PlaceCmd::Library(loc.path().clone()), false);
            }
            if loc.is_recycle_bin() {
                m.place(S::SidebarRestoreAll.t(), PlaceCmd::RestoreAll, false);
                m.place(S::UiBinEmpty.t(), PlaceCmd::EmptyBin, false);
            }
            extra(a, m);
            m.separator();
            if let Some(p) = path.as_ref().filter(|_| !offline) {
                m.place(S::MenuProperties.t(), PlaceCmd::Properties(p.clone()), false);
            }
        });
        self.show_place_menu(x, y, path, menu, crate::ui::MenuKind::Folder);
    }

    pub(in crate::ui) fn show_place_menu(&mut self, x: f32, y: f32, path: Option<FsPath>, menu: Vec<crate::ui::MenuItem>, kind: crate::ui::MenuKind) {
        let path = path.filter(|p| !self.nav.sidebar.places.offline_path(p));
        let Some(p) = path else { return self.context_flit_menu(x, y, menu, kind) };
        let shift = self.mods.shift;
        let dir = p.parent().unwrap_or_else(FsPath::computer);
        self.push_shell_menu(ShellMenu { dir, items: vec![p], x, y, extended: shift, menu, inline: self.shell_items_inline || shift, shell: true, kind });
    }

    pub(in crate::ui) fn build_section_menu(&mut self, s: Section) -> Vec<crate::ui::MenuItem> {
        self.collect(|a, m| {
            let sb = &a.nav.sidebar;
            let shown = sb.shown_sections();
            let at = shown.iter().position(|&x| x == s).unwrap_or(0);
            let none_collapsed = shown.iter().all(|&x| !sb.is_collapsed(x));
            m.place_icon(S::SidebarCollapseAll.t(), PlaceCmd::Fold(true), G_COLLAPSED, false);
            m.place_icon(S::SidebarExpandAll.t(), PlaceCmd::Fold(false), G_EXPANDED, none_collapsed);
            m.toggle(S::SidebarShowPaths.t(), PlaceCmd::ShowPaths, sb.prefs.quick.show_paths);
            m.iconed(G_PATHS);
            sections_submenu(m, sb);
            m.separator();
            m.place_icon(S::SidebarShiftUp.t(), PlaceCmd::Shift(s, -1), G_SHIFT_UP, at == 0);
            m.place_icon(S::SidebarShiftDown.t(), PlaceCmd::Shift(s, 1), G_EXPANDED, at + 1 >= shown.len());
            m.place_icon(S::SidebarRestoreOrder.t(), PlaceCmd::RestoreOrder, G_RESTORE_ORDER, sb.prefs.order() == Section::ALL);
            m.separator();
            match s {
                Section::Places => m.place(S::SidebarRestoreOrder.t(), PlaceCmd::RestorePlaces, false),
                Section::Recents => {
                    m.action("home.clear_recent", S::SidebarClearRecents.t());
                    m.iconed(G_TRASH);
                }
                Section::Bookmarks => {
                    m.action("bookmarks.clear", S::SidebarClearBookmarks.t());
                    m.iconed(G_TRASH);
                }
                Section::Tags => {
                    m.action("tags.new", S::MenuNewTag.t());
                    m.action("tags.manage", S::MenuManageTags.t());
                }
                Section::Libraries => library_places(a, m),
                Section::Storage | Section::Wsl => {}
            }
        })
    }

    fn section_menu(&mut self, x: f32, y: f32) {
        let menu = self.collect(|a, m| {
            let sb = &a.nav.sidebar;
            m.action("drive.map", S::SidebarDriveMap.t());
            m.action("drive.disconnect", S::SidebarDriveDisconnectDialog.t());
            m.action("drive.connect_server", S::SidebarDriveConnectServer.t());
            m.separator();
            library_places(a, m);
            m.separator();
            sections_submenu(m, sb);
            m.toggle(S::SidebarCompact.t(), PlaceCmd::Compact, sb.prefs.compact);
        });
        self.flit_menu(x, y, menu);
    }

    pub(in crate::ui) fn side_menu(&mut self, x: f32, y: f32, h: Hit) {
        match h {
            Hit::Side(i) | Hit::SidePin(i) => {
                if let Some(k) = self.nav.sidebar.tag_of(i) {
                    let menu = self.build_tag_menu(Some(k));
                    return self.flit_menu(x, y, menu);
                }
                if let Some(r) = self.nav.sidebar.recent_entry(i) {
                    return self.recent_menu(x, y, r);
                }
                if let Some(loc) = self.nav.sidebar.items.get(i).map(|it| it.loc.clone()) {
                    self.place_menu(x, y, loc, |_, _| {});
                }
            }
            Hit::SideHeader(s) => {
                let r = self.nav.sidebar.headers.iter().find(|h| h.1 == s).map_or(Rect::new(x, y, 0.0, 0.0), |h| h.0);
                let menu = self.build_section_menu(s);
                self.button_menu(Hit::SideHeader(s), r.x, r.bottom(), menu);
            }
            _ => self.section_menu(x, y),
        }
    }
}

fn sections_submenu(m: &mut Menu, sb: &Sidebar) {
    m.submenu(S::SidebarSections.t(), |m| {
        for s in Section::ALL.into_iter().filter(|&s| sb.available(s)) {
            m.toggle(s.title(), PlaceCmd::Hide(s), !sb.is_hidden(s));
        }
    });
}

fn library_places(a: &App, m: &mut Menu) {
    if a.caps.supports("libraries.new") {
        m.place(S::SidebarNewLibrary.t(), PlaceCmd::NewLibrary, false);
    }
    if a.caps.supports("libraries.restore") {
        m.place(S::SidebarRestoreLibraries.t(), PlaceCmd::RestoreLibraries, false);
    }
}
