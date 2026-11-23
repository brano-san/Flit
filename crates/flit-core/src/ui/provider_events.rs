use super::*;

impl App {
    pub fn poll(&mut self) -> bool {
        let mut visible = self.poll_here();
        visible |= self.poll_parked();
        self.poll_bin();
        visible |= self.poll_address();
        visible |= self.poll_quick();
        self.dirty |= visible;
        visible
    }

    pub(super) fn poll_here(&mut self) -> bool {
        let mut visible = false;
        for k in 0..self.tabs.items.len() {
            visible |= self.poll_tab(k);
            let changed = self.tabs.items[k].apply_diffs();
            if changed && k == self.tabs.cur {
                self.compute_dir_sizes();
                visible = true;
            }
        }
        visible | self.poll_panes() | self.poll_tree()
    }

    fn poll_tab(&mut self, k: usize) -> bool {
        let Some(snap) = self.tabs.items[k].slot.take() else { return false };
        self.apply_snapshot(k, snap)
    }

    pub(super) fn apply_snapshot(&mut self, k: usize, snap: Arc<crate::listing::DirSnapshot>) -> bool {
        if self.redirect(k, &snap) {
            return false;
        }
        self.sync_spec(k);
        let current = snap.gen == self.tabs.items[k].gen;
        let (loaded, fresh) = (snap.done && current, current && self.tabs.items[k].view.as_ref().is_none_or(|v| v.snap().gen != snap.gen));
        if fresh && self.tabs.items[k].look == LayoutMode::Columns {
            self.keep_as_pane(k);
        }
        let snap = self.with_saved_servers(k, snap);
        if loaded {
            self.check_discovery(k, &snap);
        }
        let clock = crate::query_attrs::Clock::system(self.utc_offset_min);
        let tags = self.tags().clone();
        let resort = self.tabs.items[k].on_snapshot(snap, clock, &tags.defs);
        if current {
            self.apply_pick_filter(k);
        }
        if std::mem::take(&mut self.tabs.items[k].properties_due) && k == self.tabs.cur {
            self.properties_dialog();
        }
        if fresh || loaded {
            self.settle_look(k, fresh, loaded);
        }
        if loaded {
            self.reopen_tree(k);
        }
        if resort {
            self.resort(k);
        }
        if loaded {
            self.on_loaded(k);
        }
        k == self.tabs.cur
    }

    pub fn on_provider_event(&mut self, ev: ProviderEvent, now: Instant) {
        if let Some(id) = self.parked_of_event(&ev) {
            return self.visit(id, |a| a.on_provider_event(ev, now));
        }
        let tab_of = |tabs: &[Tab], id: TabId, gen: u64| tabs.iter().position(|t| t.id == id && t.gen == gen);
        match ev {
            ProviderEvent::Changes { tab, gen, changes } => {
                if let Some(k) = tab_of(&self.tabs.items, tab, gen) {
                    if self.picker_uses_mime() {
                        self.reload(k, now);
                        return;
                    }
                    if let Some(dir) = self.tabs.items[k].view_loc.to_windows() {
                        for c in &changes {
                            self.shell_overlays.forget(dir.as_bytes(), &c.name);
                        }
                    }
                    self.tabs.items[k].pending_diffs.extend(changes);
                    self.git_refresh(k, true);
                    if self.tabs.items[k].pending_diffs.len() > MAX_PENDING_DIFFS {
                        return self.on_provider_event(ProviderEvent::Rescan { tab, gen }, now);
                    }
                    self.dirty |= k == self.tabs.cur;
                }
            }
            ProviderEvent::Rescan { tab, gen } => {
                if let Some(k) = tab_of(&self.tabs.items, tab, gen) {
                    self.reload(k, now);
                }
            }
            ProviderEvent::SizesSkipped { tab, gen, items } => {
                if let Some(k) = tab_of(&self.tabs.items, tab, gen) {
                    for idx in items {
                        self.tabs.items[k].sizing.1.remove(&idx);
                    }
                    self.dirty |= k == self.tabs.cur;
                }
            }
            ProviderEvent::DirSize { tab, gen, idx, stats } => {
                if let Some(k) = tab_of(&self.tabs.items, tab, gen) {
                    if let Some(v) = self.tabs.items[k].view.as_mut().filter(|v| v.snap().gen == gen) {
                        v.set_dir_stats(idx, stats);
                    }
                    self.tabs.items[k].sizing.1.remove(&idx);
                    self.dirty |= k == self.tabs.cur;
                }
            }
            ProviderEvent::OpDone { tab, id, error, report } => self.op_done(tab, id, error, report, now),
            ProviderEvent::SavedServerConnected { tab, address, path } => {
                self.change_server(ServerChange::Add(address));
                self.on_provider_event(ProviderEvent::Navigate { tab, path }, now);
            }
            ProviderEvent::OpProgress { id, progress, .. } => self.job_progress(id, progress, now),
            ProviderEvent::OpConflict { id, conflict, .. } => self.job_conflict(id, conflict),
            ProviderEvent::OpConflicts { id, conflicts, .. } => self.job_conflicts(id, conflicts),
            ProviderEvent::OpFailure { id, failure, .. } => self.job_failure(id, failure),
            ProviderEvent::Untrashable { id, items, .. } => self.offer_delete(id, items),
            ProviderEvent::Navigate { tab, path } => {
                if self.nav.sidebar.places.offline_path(&path) {
                    self.effects.push(Effect::RefreshPlaces);
                }
                if self.tab().id == tab {
                    if network::network_address(&path).is_some() {
                        self.route_here(Route { folder: Location::from_local(path), select: None }, now);
                        let t = &mut self.tabs.items[self.tabs.cur];
                        t.slot.take();
                        t.list_network(self.case, &mut self.effects);
                    } else {
                        self.navigate(Location::from_local(path), now);
                    }
                }
            }
            ProviderEvent::NetworkConnected { tab, gen, result } => {
                if let Some(k) = tab_of(&self.tabs.items, tab, gen) {
                    self.network_connected(k, result, now);
                }
            }
            ProviderEvent::NetworkDiscovery { tab, gen, warning } => {
                if let Some(k) = self.tabs.items.iter().position(|t| t.id == tab && t.discovery == network::DiscoveryCheck::Pending(gen)) {
                    self.tabs.items[k].discovery = network::DiscoveryCheck::Checked(warning);
                    self.dirty |= k == self.tabs.cur;
                }
            }
            ProviderEvent::Reveal { tab, path, new_tab } => {
                if self.tab().id == tab {
                    self.reveal_path(Location::from_local(path), new_tab, now);
                }
            }
            ProviderEvent::Hashed { id, result, .. } => self.hashed(id, result.map_err(|e| crate::i18n::localize(&e).into_owned())),
            ProviderEvent::Measured { id, totals, file_system, done, .. } => {
                if !self.quick_measured(id, totals, done) {
                    self.measured(id, totals, file_system, done);
                }
            }
            ProviderEvent::OpNotice { tab, text } => {
                if let Some(k) = self.tabs.items.iter().position(|t| t.id == tab) {
                    self.tabs.items[k].error = Some(crate::i18n::localize(&text).into_owned());
                    self.dirty |= k == self.tabs.cur;
                }
            }
            ProviderEvent::Status { tab, gen, text } => {
                if let Some(k) = tab_of(&self.tabs.items, tab, gen) {
                    if let Some(l) = self.tabs.items[k].loading.as_mut() {
                        l.note = Some(crate::redact::passwords(&crate::i18n::localize(&text)).into_owned());
                        self.dirty |= k == self.tabs.cur;
                    }
                }
            }
            ProviderEvent::Unwatched { tab, gen, limit } => {
                if let Some(k) = tab_of(&self.tabs.items, tab, gen) {
                    self.tabs.items[k].live_off = Some(if limit { tab::LiveOff::Limit } else { tab::LiveOff::Failed });
                    self.dirty |= k == self.tabs.cur;
                }
            }
            ProviderEvent::Access { id, items, accounts, .. } => self.properties_access(id, items, accounts),
            ProviderEvent::NotExecutable { tab, path } => self.ask_allow_run(tab, path),
            ProviderEvent::WrongPassphrase { tab, device } => self.unlock_again(tab, device),
            ProviderEvent::OpenAsk { tab, path, ask } => self.open_ask(tab, path, ask),
            ProviderEvent::MountAsk { tab, gen, ask, answer } => {
                if gen.is_none_or(|gen| tab_of(&self.tabs.items, tab, gen).is_some()) {
                    self.mount_ask(ask, answer);
                } else {
                    answer.send(crate::provider::MountReply::Cancel);
                }
            }
            ProviderEvent::MountAskDone { id, .. } => self.mount_ask_done(id),
            ProviderEvent::Denied { tab, error } => self.admin_denied(tab, &error, now),
        }
    }

    pub fn on_resorted(&mut self, tab: TabId, gen: u64, view: ViewSpec, base_len: usize, order: Vec<u32>) {
        if let Some(id) = self.parked_of(tab) {
            return self.visit(id, |a| a.on_resorted(tab, gen, view, base_len, order));
        }
        if let Some(k) = self.tabs.items.iter().position(|t| t.id == tab && t.gen == gen && t.view_spec == view) {
            if let Some(v) = &mut self.tabs.items[k].view {
                if base_len > v.listing().len() {
                    return self.resort(k);
                }
                v.merge_order(order, view, base_len);
                self.dirty |= k == self.tabs.cur;
            }
        }
    }

    pub fn on_icon<B: TextBackend>(&mut self, gfx: &mut Gfx<B>, key: u64, result: IconResult) {
        match result {
            IconResult::Ready(bm) => {
                self.icons.remove(&key);
                gfx.add_image(key, bm.width, bm.height, bm.rgba);
            }
            IconResult::Shared(share, bm) => {
                if self.icons.get(&key) == Some(&IconState::Pending) {
                    self.icons.remove(&key);
                    let shared = hash::hash(&("shared-icon", share, self.icon_epoch));
                    if !gfx.has_image(shared) {
                        gfx.add_image(shared, bm.width, bm.height, bm.rgba);
                    }
                    gfx.alias_image(key, shared);
                }
            }
            IconResult::Failed => {
                self.icons.insert(key, IconState::Failed);
            }
            IconResult::Skipped => {
                self.icons.remove(&key);
            }
        }
        self.dirty = true;
    }

    pub fn on_thumb<B: TextBackend>(&mut self, gfx: &mut Gfx<B>, key: u64, result: IconResult) {
        match result {
            IconResult::Ready(bm) | IconResult::Shared(_, bm) if gfx.thumbs.insert(key, bm.width, bm.height, &bm.rgba) => {
                self.thumbs.remove(&key);
            }
            IconResult::Ready(_) | IconResult::Shared(..) | IconResult::Failed => {
                if self.thumbs.len() >= MAX_THUMB_STATES {
                    self.thumbs.retain(|_, s| *s == IconState::Pending);
                }
                self.thumbs.insert(key, IconState::Failed);
            }
            IconResult::Skipped => {
                self.thumbs.remove(&key);
            }
        }
        self.dirty = true;
    }

    pub fn on_type_name(&mut self, ext: Vec<u8>, name: Option<String>) {
        self.type_names.insert(ext, Some(name.unwrap_or_default()));
        self.dirty = true;
    }

    pub(super) fn resort(&mut self, k: usize) {
        self.sync_spec(k);
        let t = &mut self.tabs.items[k];
        if let Some(v) = &mut t.view {
            v.set_git(if t.view_spec.sort.field == SortField::GitHash { t.git.clone() } else { None });
            self.effects.push(Effect::Resort { tab: t.id, gen: t.gen, snap: v.snap().clone(), view: t.view_spec });
        }
    }

    pub(super) fn compute_dir_sizes(&mut self) {
        if !self.folder_stats_visible() {
            self.tab_mut().stop_sizes();
            return;
        }
        let t = self.tab();
        if t.view.as_ref().is_none_or(|v| v.snap().gen != t.gen) || t.view_loc.is_recycle_bin() || crate::library::is_library(t.view_loc.path()) {
            return;
        }
        if !self.sizes_allowed(&t.view_loc) {
            return;
        }
        let items = t.size_items();
        if items.is_empty() {
            return;
        }
        t.sizes_cancel.set(t.gen);
        let (id, gen, cancel, loc) = (t.id, t.gen, t.sizes_cancel.clone(), t.location().clone());
        let pending = items.iter().map(|(i, _)| *i);
        let t = self.tab_mut();
        if t.sizing.0 != gen {
            t.sizing = (gen, Default::default());
        }
        t.sizing.1.extend(pending);
        let allowed = [self.settings.opts.sizes_ssd, self.settings.opts.sizes_hdd, self.settings.opts.sizes_network];
        let req = ProviderRequest::DirSizes { tab: id, gen, dir: loc.path().clone(), items, cancel, allowed };
        self.effects.push(Effect::Provider { provider: loc.provider(), req });
    }

    pub(super) fn refresh_dir_sizes(&mut self) {
        if !self.folder_stats_visible() {
            return;
        }
        if let Some(v) = &mut self.tab_mut().view {
            let rows = v.rows().to_vec();
            for i in rows {
                v.invalidate_dir_stats(i);
            }
        }
        self.compute_dir_sizes();
        self.dirty = true;
    }

    pub(crate) fn folder_stats_visible(&self) -> bool {
        self.folder_stats_visible_in(self.tabs.cur)
    }

    pub(super) fn folder_stats_visible_in(&self, k: usize) -> bool {
        let t = &self.tabs.items[k];
        if t.look.table() {
            [Column::Folders, Column::Files, Column::Size].into_iter().any(|c| self.stat_enabled(c) && t.columns.shown(c))
        } else {
            self.prefs.auto_sizes
        }
    }

    pub(super) fn stat_enabled(&self, column: Column) -> bool {
        match column {
            Column::Size => self.prefs.auto_sizes,
            Column::Folders => self.prefs.count_folders,
            Column::Files => self.prefs.count_files,
            _ => false,
        }
    }
}
