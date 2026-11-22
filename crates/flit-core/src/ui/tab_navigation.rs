use super::*;

impl App {
    pub fn open(&mut self, cmd: &Open, shell_folder: &dyn Fn(&str) -> Option<Location>, now: Instant) {
        let split = network::split_remote(cmd).filter(|_| self.caps.unix_paths);
        let cmd = match &split {
            Some((servers, rest)) => {
                for uri in servers {
                    self.launch(crate::provider::Launch::ConnectServer(uri.clone()));
                }
                if rest.is_empty() {
                    self.addr.pending = None;
                    return;
                }
                rest
            }
            None => cmd,
        };
        let (routes, errors) = open::routes_for(cmd, shell_folder, self.caps.unix_paths);
        let target = if cmd.target == OpenTarget::Default { self.open_target } else { cmd.target };
        let typed = self.addr.pending.take();
        if let Some(text) = typed.as_ref().filter(|_| routes.is_empty() && matches!(errors.first(), Some(ResolveError::Invalid(_)))) {
            if self.caps.unix_paths && open::typed_as_path(text) {
                return self.show_open_errors(&errors);
            }
            return self.run_typed(text);
        }
        let watched = !routes.is_empty();
        self.dispatch_open(routes, target, cmd.properties, now);
        if let Some(text) = typed.filter(|_| watched) {
            self.watch_typed(text);
        }
        self.show_open_errors(&errors);
    }

    pub fn show_open_errors(&mut self, errors: &[ResolveError]) {
        if let Some(e) = errors.first() {
            self.tab_mut().error = Some(match e {
                ResolveError::UnknownShellLocation(s) => self.hand_to_explorer(s),
                ResolveError::Invalid(s) => crate::tr!(S::UiInvalidPath, path = crate::redact::passwords(s)),
            });
            self.dirty = true;
        }
    }

    pub fn dispatch_open(&mut self, routes: Vec<Route>, target: OpenTarget, properties: bool, now: Instant) {
        self.dispatch(routes, target, now);
        if let Some(l) = self.tab_mut().loading.as_mut().filter(|l| properties && l.select.is_some()) {
            l.properties = true;
        }
    }

    pub fn dispatch(&mut self, routes: Vec<Route>, target: OpenTarget, now: Instant) {
        self.dispatch_routes(routes, target, false, now);
    }

    pub(super) fn dispatch_entries(&mut self, routes: Vec<Route>, target: OpenTarget, now: Instant) {
        self.dispatch_routes(routes, target, true, now);
    }

    fn dispatch_routes(&mut self, mut routes: Vec<Route>, target: OpenTarget, connect: bool, now: Instant) {
        if routes.is_empty() {
            return;
        }
        let total = routes.len();
        routes.truncate(MAX_ROUTES);
        routes.iter_mut().for_each(|r| r.folder.canonicalize(&self.nav.sidebar.places.wsl));
        match target {
            OpenTarget::NewWindow => {
                let network = connect && self.caps.unix_paths && routes.iter().any(|r| network::network_address(r.folder.path()).is_some());
                self.effects.push(if network { Effect::NewNetworkWindow(routes) } else { Effect::NewWindow(routes) });
            }
            OpenTarget::CurrentTab => {
                let mut it = routes.into_iter();
                let Some(first) = it.next() else { return };
                self.route_here(first, now);
                if connect {
                    self.begin_network(self.tabs.cur);
                }
                for r in it {
                    self.add_tab_like_current(r, now);
                    if connect {
                        self.begin_network(self.tabs.cur);
                    }
                }
            }
            OpenTarget::NewTab | OpenTarget::Default => {
                for r in routes {
                    self.add_tab_like_current(r, now);
                    if connect {
                        self.begin_network(self.tabs.cur);
                    }
                }
            }
        }
        self.report_dropped(total);
        self.dirty = true;
    }

    pub(super) fn route_here(&mut self, mut r: Route, now: Instant) {
        let k = self.tabs.cur;
        r.folder = admin::stays_admin(self.tabs.items[k].location(), r.folder);
        let keep = self.tabs.items[k].capture();
        let t = &mut self.tabs.items[k];
        if !t.history.go(r.folder, keep) {
            let current = t.gen;
            if let (Some(name), Some(v)) = (&r.select, t.view.as_mut().filter(|v| v.snap().gen == current)) {
                if let Some(i) = v.find(name).filter(|&i| v.is_visible(i)) {
                    v.clear_selection();
                    v.set_selected(i, true);
                    v.focus = Some(i);
                    v.anchor = Some(i);
                    return self.reveal_focus(true);
                }
            }
        }
        self.load_tab(k, r.select, now);
        self.after_tab_change();
    }

    fn add_tab_like_current(&mut self, r: Route, now: Instant) {
        let t = self.tab();
        let (view, show_hidden) = (t.folder_view(), t.view_spec.show_hidden);
        self.add_tab(r, view, show_hidden, now);
    }

    pub(super) fn add_tab(&mut self, r: Route, view: FolderView, show_hidden: bool, now: Instant) {
        let id = self.tabs.next_tab;
        self.tabs.next_tab += 1;
        let shown = self.tabs.items.get(self.tabs.cur).and_then(|c| c.view.as_ref().filter(|v| v.snap().done && v.snap().error.is_none() && c.view_loc == r.folder && *c.location() == r.folder).map(|v| v.snap().clone()));
        let mut t = Tab::new(id, r.folder, ViewSpec { show_hidden, ..ViewSpec::default() });
        t.set_folder_view(view);
        if let Some(snap) = shown {
            t.view = Some(crate::view::DirView::new(snap));
        }
        let at = (self.tabs.cur + 1).min(self.tabs.items.len());
        self.tabs.items.insert(at, t);
        self.tabs.cur = at;
        self.load_tab(self.tabs.cur, r.select, now);
        self.after_tab_change();
    }

    pub(super) fn after_tab_change(&mut self) {
        self.tabs.tab_cycle = false;
        self.record_tab_use();
        self.show_selected_tab();
    }

    fn record_tab_use(&mut self) {
        self.tabs.tab_use += 1;
        self.tab_mut().last_used = self.tabs.tab_use;
    }

    pub(super) fn finish_tab_cycle(&mut self) {
        if std::mem::take(&mut self.tabs.tab_cycle) {
            self.record_tab_use();
        }
    }

    pub(super) fn cycle_recent_tabs(&mut self, backward: bool) {
        if self.tabs.items.len() < 2 {
            return;
        }
        if !self.tabs.tab_cycle {
            self.record_tab_use();
            self.tabs.tab_cycle = true;
        }
        let mut order: Vec<usize> = (0..self.tabs.items.len()).collect();
        order.sort_by_key(|&i| (std::cmp::Reverse(self.tabs.items[i].last_used), i));
        let at = order.iter().position(|&i| i == self.tabs.cur).unwrap_or(0);
        let step = if backward { order.len() - 1 } else { 1 };
        self.tabs.cur = order[(at + step) % order.len()];
        self.show_selected_tab();
    }

    fn show_selected_tab(&mut self) {
        self.quick_tags = false;
        self.focus = Focus::List;
        self.wake_tab();
        self.effects.push(Effect::Title(self.window_title()));
        self.dirty = true;
    }

    pub(super) fn select_tab(&mut self, i: usize) {
        if i < self.tabs.items.len() && i != self.tabs.cur {
            self.tabs.cur = i;
            self.after_tab_change();
        }
    }

    pub(super) fn close_tab(&mut self, i: usize) {
        if i >= self.tabs.items.len() {
            return;
        }
        if self.tabs.items.len() == 1 {
            return self.close_panel();
        }
        let mut t = self.tabs.items.remove(i);
        t.shutdown(&mut self.effects);
        self.remember_closed(t);
        if self.tabs.cur > i || self.tabs.cur >= self.tabs.items.len() {
            self.tabs.cur = self.tabs.cur.saturating_sub(1);
        }
        self.after_tab_change();
    }

    pub(super) fn navigate(&mut self, to: Location, now: Instant) {
        self.dispatch(vec![Route { folder: to, select: None }], OpenTarget::CurrentTab, now);
    }

    pub(super) fn go_up(&mut self, now: Instant) {
        let cur = self.current_location().clone();
        if let Some(parent) = cur.parent() {
            let select = self.select_previous.then(|| cur.file_name().to_vec());
            self.dispatch(vec![Route { folder: parent, select }], OpenTarget::CurrentTab, now);
        }
    }

    pub(super) fn go_history(&mut self, forward: bool, now: Instant) {
        self.jump_history(if forward { 1 } else { -1 }, now);
    }

    pub(super) fn jump_history(&mut self, n: isize, now: Instant) {
        let (k, previous) = (self.tabs.cur, self.select_previous);
        let keep = self.tabs.items[k].capture();
        let t = &mut self.tabs.items[k];
        let from = t.location().clone();
        if t.history.jump(n, keep) {
            let restore = t.history.take_here();
            let named = restore.as_ref().is_some_and(crate::nav::Restore::has_names);
            let select = (!named && previous && from.parent().as_ref() == Some(t.location())).then(|| from.file_name().to_vec());
            self.load_tab_restored(k, select, restore, now);
            self.after_tab_change();
        }
    }

    pub fn refresh(&mut self, now: Instant) {
        self.reload(self.tabs.cur, now);
        self.begin_network(self.tabs.cur);
    }

    pub(super) fn reload(&mut self, k: usize, now: Instant) {
        if let Some(dir) = self.tabs.items[k].view_loc.to_windows() {
            self.shell_overlays.forget_dir(dir.as_bytes());
        }
        let keep = self.tabs.items[k].capture();
        self.load_tab_restored(k, None, keep, now);
        self.dirty |= k == self.tabs.cur;
    }

    pub(super) fn open_entry(&mut self, i: u32, now: Instant) {
        if self.picker_open_entry(i, now) {
            return;
        }
        let t = self.tab();
        let Some(view) = &t.view else { return };
        let listing = view.listing();
        let Some(&e) = listing.entries.get(i as usize) else { return };
        if t.view_loc.is_recycle_bin() {
            let name = name::display(listing.name(i)).into_owned();
            self.tab_mut().error = Some(crate::tr!(S::UiInRecycleBin, name = name));
            return;
        }
        if self.caps.unix_paths && t.view_loc.path().is_network() {
            return self.open_network(i, now);
        }
        let Some(loc) = t.entry_location(i) else { return };
        if self.caps.unix_paths && t.view_loc.path().kind() == PathKind::Computer && self.mount_drive(loc.path()) {
            return;
        }
        if !e.is_dir() && self.open_archived(&loc) {
            return;
        }
        let t = self.tab();
        if e.is_dir() || self.browses_as_folder(loc.file_name()) || crate::library::is_library(loc.path()) {
            let route = vec![Route { folder: loc, select: None }];
            if self.folders_in_new_tab {
                self.open_in_tabs(route, now);
            } else {
                self.dispatch(route, OpenTarget::CurrentTab, now);
            }
        } else {
            self.effects.push(Effect::Provider { provider: loc.provider(), req: ProviderRequest::Open { tab: t.id, path: loc.path().clone() } });
        }
    }

    pub(crate) fn empty_text(&self, loc: &Location, filtered: bool) -> S {
        if filtered {
            S::UiEmptyFiltered
        } else if loc.is_recycle_bin() {
            S::UiEmptyBin
        } else if self.caps.unix_paths && loc.path().is_network() {
            if loc.path().is_root() {
                S::UiEmptyNetwork
            } else {
                S::UiEmptyNetworkConnect
            }
        } else {
            S::UiEmptyFolder
        }
    }

    pub(super) fn open_selection(&mut self, now: Instant) {
        let t = self.tab();
        let Some(v) = &t.view else { return };
        let is_dir = |i: u32| v.listing().entries[i as usize].is_dir();
        if self.caps.unix_paths && t.view_loc.path().is_network() {
            if let Some(i) = v.focus.or_else(|| v.selected().next()) {
                self.open_network(i, now);
            }
            return;
        }
        if v.selected_count() > 1 && !t.view_loc.is_recycle_bin() {
            return self.open_many(now);
        }
        if let Some(f) = v.focus.filter(|&f| is_dir(f)) {
            return self.open_entry(f, now);
        }
        let items: Vec<u32> = t.selection().into_iter().take(MAX_OPEN).collect();
        if let Some(&d) = items.iter().find(|&&i| is_dir(i)) {
            return self.open_entry(d, now);
        }
        for i in items {
            self.open_entry(i, now);
        }
    }

    pub(super) fn new_tab_here(&mut self, now: Instant) {
        let folder = self.current_location().clone();
        self.dispatch(vec![Route { folder, select: None }], OpenTarget::NewTab, now);
    }

    pub(super) fn open_in_tabs(&mut self, routes: Vec<Route>, now: Instant) {
        self.open_routes_in_tabs(routes, false, now);
    }

    pub(super) fn open_entries_in_tabs(&mut self, routes: Vec<Route>, now: Instant) {
        self.open_routes_in_tabs(routes, true, now);
    }

    fn open_routes_in_tabs(&mut self, routes: Vec<Route>, connect: bool, now: Instant) {
        let back = self.tabs.cur;
        self.dispatch_routes(routes, OpenTarget::NewTab, connect, now);
        if !self.switch_to_new_tab && self.tabs.cur != back {
            self.select_tab(back);
        }
    }
}
