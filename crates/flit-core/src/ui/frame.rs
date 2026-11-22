use super::*;

impl App {
    pub fn frame<B: TextBackend>(&mut self, g: &mut Gfx<B>, now: Instant) -> bool {
        let _span = tracing::trace_span!("ui_frame").entered();
        let dt = frame_dt(self.last_frame, now, self.moving);
        self.last_frame = Some(now);
        self.pump_shell_props(now);
        self.poll();
        self.show_pending_archive_offer();
        self.dirty = false;
        self.expire_error(now);
        self.expire_undo_bar(now);
        self.motion = false;
        self.file_icons.begin();
        self.shell_overlays.begin(self.icon_epoch);
        g.dl.round = self.round_corners;
        self.apply_tag_fixes(now);
        self.ops_frame(now);
        let mut animating = self.marquee_autoscroll(dt);
        animating |= self.autoscroll_step(dt);
        animating |= self.touch_fling(dt);
        animating |= self.drag_autoscroll(dt);
        animating |= self.scroll_step(dt);
        animating |= self.glide_overlays(dt);
        animating |= self.step_parked(dt);
        animating |= self.step_sidebar(now);
        animating |= self.step_split(now);
        self.layout();
        self.sync_preview(now);

        let mut arena = std::mem::take(&mut self.arena);
        arena.reset();
        self.fit_columns(g, &arena);
        self.fit_parked(g, &arena);
        self.fit_sidebar(g);
        self.fit_settings(g);
        self.look.font_missing = !g.set_ui_family(&self.look.font);
        g.set_mono_family(&self.settings.opts.inspector_font);
        let hot = (self.drag == Drag::None && self.hover != Hit::None).then_some((self.tabs.panels.active, self.hover));
        self.motion |= self.hover_glow.aim(hot, self.animations, now);
        let palette = self.look.palette;
        let (th, files) = (&palette.base, palette.area(Area::Files));
        let ids: Vec<TabId> = self.tabs.items.iter().chain(self.tabs.panels.list.iter().filter(|p| p.id != self.tabs.panels.here).flat_map(|p| &p.tabs)).map(|t| t.id).collect();
        if self.tabs.strip.arrivals(&ids, self.animations, now) {
            self.motion = true;
        }
        self.tabs.strip.begin(self.tabs.panels.list.iter().map(|p| p.id), self.animations);
        for _ in 0..2 {
            arena.reset();
            g.begin(files.bg);
            self.draw_backdrop(&mut g.dl);
            if self.search.active {
                self.clear_parked_rows();
                self.draw_search(g, &arena, th);
                self.draw_title(g, th);
                self.draw_search_bar(g, &arena, th);
                self.draw_chrome(g, th);
                self.draw_sidebar(g, palette.area(Area::Sidebar));
            } else {
                self.draw_panels(g, &arena, &palette);
                self.draw_chrome(g, th);
                self.draw_sidebar(g, palette.area(Area::Sidebar));
                self.draw_kb_focus(g, th);
            }
            self.draw_settings(g, th);
            if !self.search.active {
                self.draw_palette(g, th);
            }
            self.draw_tab_dock(g, th);
            self.draw_jobs(g, th);
            self.draw_own_menu(g, th);
            self.draw_confirm(g, th);
            self.draw_tooltip(g, th);
            self.draw_quick_tip(g, th);
            animating |= self.fade_ghosts(g, now);
            if !g.atlas_reset {
                break;
            }
            self.tabs.strip.drop_ghosts();
        }
        self.arena = arena;
        self.request_primary_after_layout();
        self.prefetch_shell();
        self.flush_shell_overlays();

        let t = self.tab();
        if let Some(l) = &t.loading {
            if t.view.as_ref().is_none_or(|v| v.snap().gen != l.gen) && now.duration_since(l.since) < Duration::from_millis(300) {
                animating = true;
            }
        }
        self.moving = animating || self.motion || g.atlas_reset;
        self.moving
    }

    pub(super) fn lit(&self, h: Hit) -> f32 {
        self.hover_glow.alpha((self.tabs.panels.here, h), self.last_frame.unwrap_or_else(Instant::now))
    }

    pub(super) fn button(&self, h: Hit, open: bool, enabled: bool) -> widgets::Button {
        match widgets::Button::of(false, open, enabled) {
            widgets::Button::Idle => widgets::Button::lit(self.lit(h)),
            other => other,
        }
    }

    pub(crate) fn clock(&self) -> Instant {
        let now = Instant::now();
        self.last_frame.map_or(now, |t| t.max(now))
    }

    pub(crate) fn smooth(&self) -> bool {
        self.smooth_scroll && self.animations
    }

    pub(super) fn scroll_step(&mut self, dt: f32) -> bool {
        let smooth = self.smooth();
        let t = self.tab_mut();
        glide(&mut t.scroll, t.scroll_target, dt, smooth)
    }

    fn glide_overlays(&mut self, dt: f32) -> bool {
        let smooth = self.smooth();
        let settings = self.settings.screen.as_mut().is_some_and(|ui| ui.scroll.step(dt, smooth));
        let palette = self.nav.palette.as_mut().is_some_and(|p| p.scroll.step(dt, smooth));
        let side = self.nav.sidebar.scroll.step(dt, smooth);
        let home = self.nav.home.scroll.step(dt, smooth);
        let update = self.dialogs.form.as_mut().is_some_and(|f| f.document.scroll.step(dt, smooth));
        settings | palette | side | home | update
    }

    fn draw_title<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme) {
        let now = self.last_frame.unwrap_or_else(Instant::now);
        let lane = self.strip_lane(now);
        let l = &self.layout;
        g.dl.rect(l.title, t.chrome);
        let border = self.s(1.0).max(1.0);
        let base = l.title.bottom() - border;
        let foot = if g.dl.round { self.s(3.0) } else { 0.0 };
        let start = l.title.x + if l.title.x == self.nav.sidebar.rect.right() && l.title.x > 0.0 { foot } else { 0.0 };
        if let Some(r) = l.tabs.get(self.tabs.cur).map(|&(r, _)| self.dragged_tab().unwrap_or_else(|| self.tabs.strip.at(lane, tabbar::Part::Tab(self.tabs.items[self.tabs.cur].id), r, now))).filter(|r| r.w > 0.0) {
            g.dl.rect(Rect::new(start, base, (r.x - foot - start).max(0.0), border), t.separator);
            g.dl.rect(Rect::new(r.right() + foot, base, (l.title.right() - r.right() - foot).max(0.0), border), t.separator);
        } else {
            g.dl.rect(Rect::new(start, base, l.title.right() - start, border), t.separator);
        }
        let font = self.ui_font(12.5);
        let m = g.metrics(font);
        let close_font = self.icon_font(8.0);
        let drop = self.drop_spot();
        let l = &self.layout;
        let mut requests: Vec<IconRequest> = Vec::new();
        let ghosts_from = g.dl.instances.len();
        self.tabs.strip.ghosts(lane, &mut g.dl, now);
        let floating = self.dragged_tab();
        let order = (0..l.tabs.len()).filter(|&i| floating.is_none() || i != self.tabs.cur).chain(floating.map(|_| self.tabs.cur));
        for i in order {
            let (target, c) = l.tabs[i];
            let part = tabbar::Part::Tab(self.tabs.items[i].id);
            let r = floating.filter(|_| i == self.tabs.cur).unwrap_or_else(|| self.tabs.strip.at(lane, part, target, now));
            let from = g.dl.instances.len();
            if r.w <= 0.0 {
                self.tabs.strip.record(lane, part, r, &[]);
                continue;
            }
            let c = if c.w > 0.0 { Rect::new(c.x + r.right() - target.right(), c.y, c.w, c.h) } else { c };
            let active = i == self.tabs.cur;
            let hovered = self.lit(Hit::Tab(i)).max(self.lit(Hit::TabClose(i)));
            let accented = self.menu_open_from(Hit::Tab(i)) || (active && self.tabs.docking.is_some() && self.tabs.panels.here == self.tabs.panels.active);
            if active {
                g.dl.outline(Rect::new(r.x, r.y, r.w, r.h + self.s(8.0)), t.separator, self.s(6.0), border);
                g.dl.confine(from, Rect::new(r.x, r.y, r.w, base - foot - r.y + border));
            }
            if accented {
                g.dl.round_rect(r.inset(0.0, self.s(2.0)), t.accent, self.s(6.0));
            } else if active {
                g.dl.round_rect(Rect::new(r.x + border, r.y + border, r.w - 2.0 * border, r.h + self.s(8.0)), t.bg, self.s(6.0));
            } else if hovered > 0.0 {
                g.dl.round_rect(r.inset(0.0, self.s(2.0)), widgets::faded(t.button_hover, hovered), self.s(6.0));
            }
            if active && foot > 0.0 {
                for left in [true, false] {
                    let x = if left { r.x - foot } else { r.right() - border };
                    let clip = Rect::new(x, base - foot, foot + border, foot + border);
                    g.dl.rect(clip, t.bg);
                    let circle = Rect::new(if left { x - foot - border } else { x }, base - 2.0 * foot - border, 2.0 * (foot + border), 2.0 * (foot + border));
                    let from = g.dl.instances.len();
                    g.dl.circle(circle, t.chrome);
                    g.dl.ring(circle, t.separator, border);
                    g.dl.confine(from, clip);
                }
            }
            if drop == Some(Spot::Tab(i)) {
                let target = r.inset(0.0, self.s(2.0));
                g.dl.round_rect(target, t.selection, self.s(6.0));
                g.dl.outline(target, t.accent, self.s(6.0), self.s(1.0).max(1.0));
            }
            let baseline = (r.y + (r.h + m.ascent - m.descent) / 2.0).round();
            let label = self.tabs.items[i].title();
            let torn = active && matches!(self.drag, Drag::Tab { torn: true, .. });
            let color = if accented { theme::ink_on(t.accent) } else if torn { t.text_dim.alpha(0x60) } else if active { t.text } else { t.text_dim };
            let icon = Rect::new(r.x + self.s(10.0), r.y + (r.h - self.s(16.0)) / 2.0, self.s(16.0), self.s(16.0));
            let loc = self.tabs.items[i].location();
            if loc.is_admin() {
                admin::draw_tab_mark(g, t, r, icon, self.icon_font(12.0), self.s(2.0), if torn || accented { Some(color) } else { None });
            } else if !self.system_icon(g, sidebar::folder_icon(loc, &self.nav.sidebar, icon.w as u32), icon, icon.w, if torn { 0x60 } else { 0xFF }, &mut requests) {
                centered(g, icon, tabbar::tab_glyph(loc), self.icon_font(12.0), if torn || accented { color } else { t.accent.alpha(0xC0) });
            }
            g.text(icon.right() + self.s(6.0), baseline, &label, font, color, c.x - icon.right() - self.s(10.0));
            if c.w > 0.0 && (active || hovered > 0.0) {
                let state = self.button(Hit::TabClose(i), false, true);
                let ink = if accented { color } else { t.text_dim };
                widgets::Paint::new(g, t, self.scale).icon_button(c, G_CLOSE, close_font, state, if active { ink } else { widgets::faded(ink, hovered) });
            }
            let shown = self.tabs.strip.arrival(self.tabs.items[i].id, now);
            if shown < 1.0 {
                g.dl.grow(from, shown, (r.x, r.y + r.h / 2.0), 0.6);
            }
            if r.w != target.w {
                g.dl.confine(from, Rect::new(r.x - foot, l.title.y, r.w + 2.0 * foot, l.title.h));
            }
            self.tabs.strip.record(lane, part, r, &g.dl.instances[from..]);
        }
        if self.tabs.strip.busy(lane) {
            g.dl.confine(ghosts_from, self.strip_clip(true));
        }
        let new_tab_dx = self.tabs.strip.at(lane, tabbar::Part::Plus, l.new_tab, now).x - l.new_tab.x;
        let new_tab_from = g.dl.instances.len();
        let mut w = widgets::Paint::new(g, t, self.scale);
        if l.new_tab.w > 0.0 {
            if drop == Some(Spot::NewTab) {
                w.g.dl.round_rect(l.new_tab, t.teal, self.s(4.0));
                w.centered(l.new_tab, G_ADD, self.icon_font(10.0), theme::ink_on(t.teal));
            } else {
                w.icon_button(l.new_tab, G_ADD, self.icon_font(10.0), self.button(Hit::NewTab, false, true), t.text_dim);
            }
        }
        w.g.dl.shift(new_tab_from, new_tab_dx.round(), 0.0);
        self.tabs.strip.record(lane, tabbar::Part::Plus, Rect::new(l.new_tab.x + new_tab_dx.round(), l.new_tab.y, l.new_tab.w, l.new_tab.h), &[]);
        if l.tab_arrows[0].w > 0.0 {
            let n = self.tabs.items.len();
            for (k, glyph) in [G_CRUMB_LEFT, G_CRUMB_RIGHT].into_iter().enumerate() {
                let on = l.strip.hidden(k == 1, n) > 0;
                let r = self.tabs.strip.at(lane, tabbar::Part::Arrow(k == 1), l.tab_arrows[k], now);
                w.icon_button(r, glyph, self.icon_font(8.0), self.button(Hit::TabScroll(k == 1), false, on), t.text_dim);
                self.tabs.strip.record(lane, tabbar::Part::Arrow(k == 1), r, &[]);
            }
        }
        if self.tabs.strip.busy(lane) {
            let clip = self.strip_clip(false);
            w.g.dl.confine(ghosts_from, clip);
        }
        if l.actions.w > 0.0 {
            let open = self.tabs.panels.here == self.tabs.panels.active && self.nav.palette.as_ref().is_some_and(|p| p.pick == palette::Pick::Tabs);
            let state = self.button(Hit::TabActions, open, true);
            w.icon_button(l.actions, tabbar::G_TAB_ACTIONS, self.icon_font(12.0), state, t.text_dim);
        }
        if let Some(r) = self.insert_mark() {
            w.g.dl.rect(r, t.accent);
        }
        self.request_icons(requests);
    }

    pub(super) fn draw_body<B: TextBackend>(&mut self, g: &mut Gfx<B>, arena: &Bump, palette: &theme::Palette) {
        let (th, files) = (&palette.base, palette.area(Area::Files));
        let active = self.tabs.panels.here == self.tabs.panels.active;
        if self.on_home() {
            self.draw_home(g, files);
        } else {
            self.draw_list(g, arena, files);
            self.draw_panes(g, files);
            if active {
                self.draw_marquee(g, files);
                self.draw_ops(g, files);
            }
            self.draw_header(g, files);
            self.draw_git_bar(g, files);
            if active {
                self.draw_preview(g, palette.area(Area::Pane));
            }
        }
        self.draw_title(g, th);
        self.draw_toolbar(g, palette.area(Area::Toolbar));
        if self.layout.status.h > 0.0 {
            let from = g.dl.instances.len();
            if self.dialogs.picker.is_some() {
                self.draw_picker(g, th);
            } else {
                self.draw_status(g, arena, th);
            }
            g.dl.confine(from, self.layout.status);
        }
        if active {
            self.draw_drop_panel(g, files);
        }
    }

    fn draw_toolbar<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme) {
        let l = &self.layout;
        g.dl.rect(l.toolbar, t.bg);
        let icons = self.icon_font(12.0);
        let tab = self.tab();
        let enabled = [tab.history.can_back(), tab.history.can_forward(), tab.location().parent().is_some(), !self.nav.sidebar.prefs.quick.visits.is_empty()];
        let mut w = widgets::Paint::new(g, t, self.scale);
        for (i, glyph) in [G_BACK, G_FORWARD, G_UP, sidebar::G_EXPANDED].into_iter().enumerate() {
            let state = self.button(Hit::Nav(i), self.menu_open_from(Hit::Nav(i)), enabled[i]);
            let font = if i == 3 { self.icon_font(8.0) } else { icons };
            w.icon_button(l.nav[i].inset(self.s(2.0), 0.0), glyph, font, state, t.text);
        }
        let sep = |r: Rect| Rect::new(r.x - self.s(5.0), r.y + self.s(8.0), 1.0, r.h - self.s(16.0));
        w.g.dl.rect(sep(l.view), t.separator);
        w.g.dl.rect(sep(Rect::new(l.address.x, l.view.y, 0.0, l.view.h)), t.separator);
        let view = self.button(Hit::ViewMode, self.menu_open_from(Hit::ViewMode), true);
        w.icon_button(l.view.inset(self.s(2.0), 0.0), details::view_mode_glyph(self.view_mode()), icons, view, t.text);
        let marked = self.nav.sidebar.is_pinned(self.current_location());
        w.toggle_button(l.bookmark.inset(self.s(2.0), 0.0), sidebar::G_PIN, icons, marked, self.lit(Hit::Bookmark));
        let more = self.button(Hit::PanelMenu, self.menu_open_from(Hit::PanelMenu), true);
        w.icon_button(l.more.inset(self.s(2.0), 0.0), G_MORE, icons, more, t.text);
        let address = *self.look.palette.area(Area::Address);
        self.draw_address(g, &address);
        self.draw_ops_button(g, t);
    }

    fn draw_address<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme) {
        let r = self.layout.address;
        if t.bg != self.look.palette.area(Area::Toolbar).bg {
            g.dl.round_rect(r, t.bg, self.s(4.0));
        }
        let lit = self.lit(Hit::Address);
        if lit > 0.0 {
            g.dl.round_rect(r, widgets::faded(t.hover, lit), self.s(4.0));
        }
        let font = self.ui_font(13.0);
        let m = g.metrics(font);
        let baseline = (r.y + (r.h + m.ascent - m.descent) / 2.0).round();
        let r = if self.current_location().is_admin() { admin::draw_address_badge(g, t, r, font, baseline, self.s(3.0)) } else { r };
        self.draw_crumbs(g, t, r, baseline, font);
    }

    fn draw_crumbs<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme, area: Rect, baseline: f32, font: FontStyle) {
        let chevron = self.icon_font(8.0);
        let gap = self.s(6.0);
        let chev_w = self.s(16.0);
        let now = self.last_frame.unwrap_or_else(Instant::now);
        let here = self.current_location().clone();
        if self.addr.shift_for.as_ref() != Some(&here) {
            self.addr.shift = 0.0;
            self.addr.shift_for = Some(here);
        }
        let tab = &self.tabs.items[self.tabs.cur];
        let real = tab.crumbs.len();
        let ghosts = if self.settings.opts.breadcrumbs { &tab.ghosts[..] } else { &[] };
        let crumbs: Vec<&(String, Location)> = tab.crumbs.iter().chain(ghosts).collect();
        let own = self.tabs.panels.here == self.tabs.panels.active;
        let open = match self.nav.palette.as_ref().map(|p| &p.pick) {
            Some(palette::Pick::Crumb(i)) if own => Some(*i),
            _ => None,
        };
        if own && self.addr.turn.aim(open, self.animations, now) {
            self.motion = true;
        }
        let skip = self.compact_skip(&tab.crumbs);
        let marked = |i: usize| if tab.subfolders && i + 1 == real { tab::SUBFOLDERS_MARK } else { "" };
        let widths: Vec<f32> = crumbs.iter().enumerate().map(|(i, (label, _))| if i < skip { 0.0 } else { g.measure(label, font) + g.measure(marked(i), font) + 2.0 * gap }).collect();
        let n = widths.len();
        let span = |end: usize| widths[..end].iter().sum::<f32>() + chev_w * end.saturating_sub(1 + skip) as f32;
        let (all, upto) = (span(n), span(real));
        let arrow_w = self.s(18.0);
        let over = all > area.w;
        let view = if over { Rect::new(area.x + 2.0 * arrow_w, area.y, area.w - 2.0 * arrow_w, area.h) } else { area };
        let x0 = if upto > view.w { view.right() - upto } else { view.x };
        let (lo, hi) = ((view.right() - (x0 + all)).min(0.0), (view.x - x0).max(0.0));
        self.addr.shift = self.addr.shift.clamp(lo, hi);
        let mut x = x0 + self.addr.shift;
        let (s1, s3, s6) = (self.s(1.0), self.s(3.0), self.s(6.0));
        let drop = self.drop_spot();
        let (left, right) = (x < view.x - 0.5, x + all > view.right() + 0.5);
        self.layout.crumb_arrows = if over { [Rect::new(area.x, area.y + s3, arrow_w, area.h - s6), Rect::new(area.x + arrow_w, area.y + s3, arrow_w, area.h - s6)] } else { [Rect::default(); 2] };
        let mut rects = vec![Rect::default(); n];
        let mut chevrons = vec![Rect::default(); n];
        let visible = |r: Rect| if r.right() > view.x && r.x < view.right() { Rect::new(r.x.max(view.x), r.y, r.right().min(view.right()) - r.x.max(view.x), r.h) } else { Rect::default() };
        g.dl.set_clip(Some(view));
        for i in skip..n {
            let w = widths[i];
            let cr = Rect::new(x, area.y + s3, w, area.h - s6);
            rects[i] = visible(cr);
            if drop == Some(Spot::Crumb(i)) {
                g.dl.round_rect(cr, t.selection, s3);
                g.dl.outline(cr, t.accent, s3, s1.max(1.0));
            } else if self.lit(Hit::Crumb(i)) > 0.0 {
                g.dl.round_rect(cr, widgets::faded(t.button_hover, self.lit(Hit::Crumb(i))), s3);
            }
            let ink = match () {
                _ if i + 1 == real => t.text,
                _ if i >= real => t.text_dim.alpha(0x80),
                _ => t.text_dim,
            };
            let label_w = g.text(x + gap, baseline, &crumbs[i].0, font, ink, f32::INFINITY);
            if !marked(i).is_empty() {
                g.text(x + gap + label_w, baseline, marked(i), font, t.text_dim, f32::INFINITY);
            }
            x += w;
            if i + 1 < n {
                let chev = Rect::new(x, cr.y, chev_w, cr.h);
                chevrons[i] = visible(chev);
                let lit = if open == Some(i) { 1.0 } else { self.lit(Hit::CrumbChevron(i)) };
                if lit > 0.0 {
                    g.dl.round_rect(chev, widgets::faded(t.button_hover, lit), s3);
                }
                let ink = t.text_dim.alpha(if i + 1 >= real { 0x80 } else { 0xFF });
                let angle = if own { self.addr.turn.angle(i, now) } else { 0.0 };
                g.dl.chevron((chev.x + chev.w / 2.0, chev.y + chev.h / 2.0), self.s(8.0), angle, self.s(1.25).max(1.0), ink);
                x += chev_w;
            }
        }
        g.dl.set_clip(None);
        self.layout.crumbs = rects;
        self.layout.crumb_chevrons = chevrons;
        if over {
            let mut w = widgets::Paint::new(g, t, self.scale);
            for (k, (glyph, on)) in [(G_CRUMB_LEFT, left), (G_CRUMB_RIGHT, right)].into_iter().enumerate() {
                w.icon_button(self.layout.crumb_arrows[k], glyph, chevron, self.button(Hit::CrumbScroll(k == 1), false, on), t.text_dim);
            }
        }
    }

    fn compact_skip(&self, crumbs: &[(String, Location)]) -> usize {
        let home = self.known_folder(sidebar::KnownFolder::Home).filter(|_| self.settings.opts.compact_paths);
        home.and_then(|h| crumbs.iter().position(|(_, l)| *l == h)).unwrap_or(0)
    }

    pub(crate) fn crumb_rect(&self, i: usize) -> Option<Rect> {
        let shown = |rects: &[Rect]| rects.get(i).copied().filter(|r| r.w > 0.0);
        shown(&self.layout.crumb_chevrons).or_else(|| shown(&self.layout.crumbs)).or(Some(self.layout.address))
    }

    pub(super) fn scroll_crumbs(&mut self, right: bool) {
        let step = self.s(CRUMB_STEP);
        self.addr.shift += if right { -step } else { step };
        self.dirty = true;
    }

    fn draw_header<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme) {
        if self.layout.header.h <= 0.0 {
            return;
        }
        let now = self.last_frame.unwrap_or_else(Instant::now);
        let (field, angle) = (self.tab().view_spec.sort.field, if self.tab().view_spec.sort.descending { 90.0 } else { -90.0 });
        let animate = self.animations;
        let m = &mut self.tab_mut().motion;
        if m.sort != Some(field) {
            m.sort_born = (m.sort.is_some() && animate).then_some(now);
            m.sort = Some(field);
            m.sort_turn = widgets::Tween::at(angle);
        }
        let mut moving = m.sort_turn.aim(angle, animate, now, address::TURN);
        if m.sort_born.is_some_and(|b| now.saturating_duration_since(b) >= address::TURN) {
            m.sort_born = None;
        }
        moving |= m.sort_born.is_some();
        let (arrow_angle, arrow_alpha) = (m.sort_turn.value(now, address::TURN), m.sort_born.map_or(1.0, |b| widgets::eased(b, now, address::TURN)));
        self.motion |= moving;
        let l = &self.layout;
        g.dl.rect(l.header, t.header);
        g.dl.rect(Rect::new(l.header.x, l.header.bottom() - 1.0, l.header.w, 1.0), t.separator);
        let font = self.content_font(12.0);
        let m = g.metrics(font);
        let baseline = (l.header.y + (l.header.h + m.ascent - m.descent) / 2.0).round();
        let pad = self.cs(10.0);
        let sort = self.tab().view_spec.sort;
        let here = self.tabs.panels.here;
        let hot = self.head_hot.filter(|h| self.hover == Hit::Header(h.0) && self.drag == Drag::None).map(|(i, funnel)| (here, i, funnel));
        if hot.is_some() || self.head_glow.touches(|k| k.0 == here) {
            self.motion |= self.head_glow.aim(hot, self.animations, now);
        }
        let l = &self.layout;
        for (i, col) in l.columns.iter().enumerate() {
            let r = l.cols[i];
            let shown = self.header_clip(r);
            if shown.w <= 0.0 {
                continue;
            }
            let lit = self.head_glow.alpha((here, i, false), now);
            if lit > 0.0 {
                g.dl.rect(shown, widgets::faded(t.button_hover, lit));
            }
            if i > 0 && r.x >= l.list.x {
                g.dl.rect(Rect::new(r.x, r.y + self.cs(6.0), 1.0, r.h - self.cs(12.0)), t.separator);
            }
            if i + 1 == l.columns.len() && r.right() < l.list.right() - 1.0 {
                g.dl.rect(Rect::new(r.right(), r.y + self.cs(6.0), 1.0, r.h - self.cs(12.0)), t.separator);
            }
            g.dl.set_clip(Some(shown));
            let sorted = Some(sort.field) == col.field();
            let mut x = r.x + pad;
            if *col == Column::Name && self.prefs.checkboxes {
                let b = self.header_check(r);
                let all = self.tab().view.as_ref().is_some_and(|v| !v.rows().is_empty() && v.selected_count() == v.rows().len());
                self.draw_checkbox(g, t, b, all);
                x = b.right() + self.cs(6.0);
            }
            let mut room = r.right() - pad - x;
            if *col == Column::Type {
                let fr = self.funnel_rect(r);
                let lit = self.head_glow.alpha((here, i, true), now);
                if lit > 0.0 {
                    let d = fr.h.min(fr.w) - self.cs(4.0);
                    g.dl.round_rect(Rect::new(fr.x + (fr.w - d) / 2.0, fr.y + (fr.h - d) / 2.0, d, d).snap(), widgets::faded(t.button_hover, lit), self.cs(4.0));
                }
                let active = !self.tab().hidden_types.is_empty();
                centered(g, fr, G_FUNNEL, self.content_icons(11.0), if active { t.accent } else { t.text_dim });
                room = fr.x - x;
            }
            if sorted {
                let gw = self.cs(14.0);
                let right = if *col == Column::Type { self.funnel_rect(r).x } else { r.right() - pad };
                g.dl.chevron((right - gw / 2.0, r.y + r.h / 2.0), self.cs(8.0), arrow_angle, self.cs(1.25).max(1.0), widgets::faded(t.text, arrow_alpha));
                room = room.min(right - gw - x);
            }
            g.text(x, baseline, col.title(), font, if sorted { t.text } else { t.text_dim }, room.max(0.0));
            g.dl.set_clip(None);
        }
        self.draw_column_move(g, t);
    }

    fn draw_list<B: TextBackend>(&mut self, g: &mut Gfx<B>, arena: &Bump, t: &Theme) {
        let now = self.last_frame.unwrap_or_else(Instant::now);
        let tab_id = self.tab().id;
        let hovered = match self.hover {
            Hit::Row(r) if self.drag == Drag::None => self.tab().view.as_ref().and_then(|v| v.rows().get(r)).map(|&i| (tab_id, i)),
            _ => None,
        };
        if hovered.is_some() || self.row_glow.touches(|k| k.0 == tab_id) {
            self.motion |= self.row_glow.aim(hovered, self.animations, now);
        }
        let tm = &mut self.tab_mut().motion;
        if tm.fold.is_some_and(|f| now.saturating_duration_since(f.since) >= sidebar::FOLD) {
            tm.fold = None;
        }
        let fold = tm.fold.filter(|_| self.layout.mode == LayoutMode::Tree);
        self.motion |= fold.is_some();
        let l = &self.layout;
        let (list, mode) = (l.list, l.mode);
        let font = self.content_font(13.0);
        let m = g.metrics(font);
        let baseline = |r: Rect| (r.y + (r.h + m.ascent - m.descent) / 2.0).round();
        let pad = self.cs(10.0);
        let icon_px = l.icon;
        let list_focused = self.active && self.focus == Focus::List;
        let tab = &self.tabs.items[self.tabs.cur];
        let spin = widgets::spin_phase(now, self.animations);
        let mut spinning = false;
        let in_bin = tab.view_loc.is_recycle_bin();
        let f = self.flow();
        let lines = self.visible_lines(&f);
        let shown_lines = lines.len();
        let fold = fold.map(|fd| {
            let parent = self.line_rect(&f, fd.line.saturating_sub(1));
            let n = fd.lines.unsigned_abs().min(shown_lines + 1) as f32;
            let e = widgets::eased(fd.since, now, sidebar::FOLD);
            let dy = (if fd.lines > 0 { -(1.0 - e) } else { 1.0 - e } * n * parent.h).round();
            (fd, dy, parent.bottom(), e)
        });
        let lines = match fold {
            Some((fd, _, _, _)) if fd.lines > 0 => lines.start..lines.end + (fd.lines as usize).min(shown_lines + 1),
            _ => lines,
        };
        let mut moved: Option<(usize, f32)> = None;
        g.dl.set_clip(Some(list));
        let mut requests: Vec<IconRequest> = Vec::new();
        let mut thumb_requests: Vec<IconRequest> = Vec::new();
        let mut type_requests: Vec<Vec<u8>> = Vec::new();
        let mut checks: Vec<(u32, bool)> = Vec::new();
        let mut elided: Vec<usize> = Vec::new();
        let want_thumbs = self.prefs.thumbnails && icon_px >= self.cs(THUMB_MIN_ICON) && !in_bin && (self.settings.opts.thumbs != options::Thumbs::Local || !self.network_place(&tab.view_loc));
        let thumb_px = thumbs::class_px(icon_px as u32);
        if let Some(v) = &tab.view {
            if let Some(warning) = self.discovery_warning() {
                self.draw_discovery_warning(g, t, list, warning);
            } else if let Some(e) = v.snap().error.as_ref().filter(|_| v.total_rows() == 0) {
                self.draw_unavailable(g, t, list, e);
            } else if v.rows().is_empty() && v.groups().is_empty() && v.snap().done {
                let msg = self.empty_text(&tab.view_loc, v.is_filtered());
                let y = list.y + self.cs(40.0);
                if matches!(msg, S::UiEmptyNetwork | S::UiEmptyNetworkConnect) {
                    self.draw_network_notice(g, Rect { y, ..list }, msg.t(), font, t.text_dim);
                } else {
                    let w = g.measure(msg.t(), font);
                    g.text(list.x + (list.w - w) / 2.0, y, msg.t(), font, t.text_dim, f32::INFINITY);
                }
            }
            let loc = &tab.view_loc;
            let shell_dir = loc.to_windows();
            let in_library = crate::library::is_library(loc.path());
            let in_root = loc.path().kind() == PathKind::Computer;
            let listing = v.listing();
            let cut = self.cut_names();
            let want_types = !in_root && (mode == LayoutMode::Cards || l.columns.contains(&Column::Type) || v.snap().view.group.field == GroupField::Type);
            let mut want_type = |i: u32| {
                let folded = listing.ext(i).to_ascii_lowercase();
                if want_types && !listing.entries[i as usize].is_dir() && !self.type_names.contains_key(&folded) && !type_requests.contains(&folded) {
                    type_requests.push(folded);
                }
            };
            for line in lines {
                if let Some((from, dy)) = moved.take() {
                    g.dl.shift_range(from..g.dl.instances.len(), 0.0, dy);
                }
                if let Some((fd, dy, top, _)) = fold.filter(|(fd, dy, _, _)| line >= fd.line && *dy != 0.0) {
                    moved = Some((g.dl.instances.len(), dy));
                    let child = fd.lines > 0 && line < fd.line + fd.lines as usize;
                    let floor = if child { top.max(list.y) } else { list.y };
                    g.dl.set_clip(Some(Rect::new(list.x, floor - dy, list.w, (list.bottom() - floor).max(0.0))));
                }
                let cell = self.line_rect(&f, line);
                let row = match v.line(line) {
                    Some(Line::Row(r)) => r,
                    Some(Line::Header(k)) => {
                        want_type(v.groups()[k].first);
                        self.draw_group_line(g, arena, t, v, k, cell);
                        continue;
                    }
                    None => break,
                };
                let i = v.rows()[row];
                let e = listing.entries[i as usize];
                let p = self.item_parts(cell);
                let rr = p.body;
                let glow = self.row_glow.alpha((tab_id, i), now);
                if v.is_selected(i) {
                    g.dl.round_rect(rr, if list_focused { t.selection } else { t.selection_inactive }, self.cs(4.0));
                } else {
                    if self.settings.opts.alternating && mode.table() && row % 2 == 1 && glow < 1.0 {
                        g.dl.round_rect(rr, t.text.alpha(0x08), self.cs(4.0));
                    }
                    if glow > 0.0 {
                        g.dl.round_rect(rr, widgets::faded(t.hover, glow), self.cs(4.0));
                    }
                }
                if list_focused && v.focus == Some(i) {
                    g.dl.outline(rr, t.focus_ring, self.cs(4.0), 1.0);
                }
                let boxed = details::shows_check(self.prefs.checkboxes, v.is_selected(i), self.details.checks.hover == Some(row));
                checks.push((i, boxed));
                let corner = !mode.rows();
                let replaces = boxed && !corner;
                if boxed && !corner {
                    let from = g.dl.instances.len();
                    self.draw_checkbox(g, t, p.check, v.is_selected(i));
                    let c = p.check;
                    g.dl.grow(from, self.details.checks.progress(i, now), (c.x + c.w / 2.0, c.y + c.h / 2.0), 0.4);
                }

                let name = listing.name(i);
                let faded = e.attrs & attr::HIDDEN != 0 || name.starts_with(b".") || cut.is_some_and(|c| c.contains(name));
                let row_dir = if in_library { FsPath::parse_exact(crate::library::folder(listing, i)) } else if e.attrs & attr::NESTED != 0 { shell_dir.as_ref().map(|d| d.join(listing.folder(i))) } else { None };
                let dir = row_dir.as_ref().or(shell_dir.as_ref());
                let dir_key = if in_bin { recycle::origin(listing, i) } else { dir.map_or(&b""[..], |d| d.as_bytes()) };
                let (key, kind) = icon_key(name, listing.ext(i), &e, icon_px as u32, dir_key, self.icon_epoch);
                self.file_icons.see(key, kind);
                let ir = p.icon;
                let alpha = if faded { 0x70 } else { 0xFF };
                let thumb = (want_thumbs && !e.is_dir() && has_thumbnail(listing.ext(i))).then(|| hash::hash(&("thumb", dir_key, name, e.modified, e.size, thumb_px)));
                let thumbed = !replaces && thumb.is_some_and(|k| g.thumb(k, ir, alpha));
                if let (Some(k), false, Some(d)) = (thumb, thumbed, dir) {
                    if !self.thumbs.contains_key(&k) && !thumb_requests.iter().any(|r| r.key == k) {
                        thumb_requests.push(IconRequest { key: k, kind: IconKind::Thumbnail, path: d.join(name), size: thumb_px, gen: tab.gen, row: row as u32 });
                    }
                }
                let badge = tab.git_status(name).map(|st| {
                    let d = (ir.h * 0.42).max(self.cs(9.0)).round();
                    (Rect::new(ir.right() - d * 0.8, ir.bottom() - d * 0.8, d, d), st)
                });
                if !replaces && !thumbed && !network_listing::draw_server_icon(g, listing, i, ir, t.accent.alpha(alpha)) && !g.image_alpha(key, ir, alpha) {
                    if !self.folder_stand_in(g, kind, icon_px as u32, ir, alpha, &mut requests) {
                        icon_glyph(g, t, &e, ir);
                    }
                    let wanted = !self.icons.contains_key(&key) && !requests.iter().any(|r| r.key == key);
                    let path = match (wanted, in_bin) {
                        (false, _) => None,
                        (true, true) => recycle::recycled(listing, i),
                        (true, false) => row_dir.as_ref().map_or_else(|| loc.join(name).to_windows(), |d| Some(d.join(name))),
                    };
                    if let Some(path) = path {
                        requests.push(IconRequest { key, kind, path, size: icon_px as u32, gen: tab.gen, row: row as u32 });
                    }
                }

                if !replaces {
                    if !in_bin {
                        self.draw_shell_overlay(g, kind, &e, dir_key, name, ir, alpha);
                    }
                    self.draw_file_emblems(g, e.attrs, ir, alpha, &mut requests);
                }

                if boxed && corner {
                    let from = g.dl.instances.len();
                    if !v.is_selected(i) {
                        g.dl.round_rect(p.check, t.bg.alpha(0xC0), self.cs(3.0));
                    }
                    self.draw_checkbox(g, t, p.check, v.is_selected(i));
                    let c = p.check;
                    g.dl.grow(from, self.details.checks.progress(i, now), (c.x + c.w / 2.0, c.y + c.h / 2.0), 0.4);
                }

                let ty = baseline(p.name);
                let picked = v.is_selected(i) && list_focused;
                let name_color = match (picked, faded) {
                    (true, true) => t.selection_text.alpha(0x99),
                    (true, false) => t.selection_text,
                    (false, true) => t.text_dim,
                    (false, false) => t.name_ink(e.attrs),
                };
                if let Some((br, st)) = badge {
                    g.dl.round_rect(br, theme::git_status_color(st), br.w / 2.0);
                    centered(g, br, st.letter(), FontStyle::new(FontFamily::Ui, br.h * 0.72, true), crate::theme::WHITE);
                }
                let shown = self.shown_name(listing, i);
                if g.measure(&shown, font) > (p.name.w - self.mark_reserve(e.tags)).max(0.0) {
                    elided.push(row);
                }
                let end = self.draw_name(g, &p, &shown, &e, font, name_color);
                if mode == LayoutMode::Tree && e.is_dir() {
                    if let Some(cr) = self.chevron_rect(row) {
                        let angle = match fold.filter(|(fd, ..)| fd.parent == i) {
                            Some((fd, _, _, e)) => 90.0 * if fd.lines > 0 { e } else { 1.0 - e },
                            None if self.is_open_in_tree(i) => 90.0,
                            None => 0.0,
                        };
                        g.dl.chevron((cr.x + cr.w / 2.0, cr.y + cr.h / 2.0), self.cs(8.0), angle, self.cs(1.25).max(1.0), t.text_dim);
                    }
                }
                let folder = listing.folder(i);
                if !folder.is_empty() && mode != LayoutMode::Tree && !p.center {
                    let label = arena.alloc_str(&format!("{NESTED_MARK}{}", crate::name::display(folder)));
                    let x = end + self.cs(8.0);
                    g.text(x, ty, label, font, t.text_dim.alpha(0xA0), (p.name.right() - x).max(0.0));
                }
                want_type(i);
                match mode {
                    LayoutMode::Details | LayoutMode::Tree => {
                        for (c, &col) in l.columns.iter().enumerate().skip(1) {
                            let r = l.cols[c];
                            let s = self.cell_text(arena, listing, i, col);
                            if s.is_empty() && self.stat_enabled(col) && tab.sizing(i) {
                                let d = self.cs(12.0);
                                let at = if col == Column::Size { r.right() - pad - d } else { r.x + pad };
                                widgets::spinner(g, Rect::new(at, (p.name.y + (p.name.h - d) / 2.0).round(), d, d), spin, if picked { t.selection_text } else { t.text_dim });
                                spinning = true;
                                continue;
                            }
                            let x = if col == Column::Size { (r.right() - pad - g.measure(s, font)).max(r.x + pad) } else { r.x + pad };
                            g.text(x, ty, s, font, if picked { t.selection_text } else { t.text_dim }, r.w - 2.0 * pad);
                        }
                    }
                    LayoutMode::Cards => {
                        let size = self.cell_text(arena, listing, i, Column::Size);
                        let last = if size.is_empty() { self.cell_text(arena, listing, i, Column::Modified) } else { size };
                        for (k, s) in [self.type_text(arena, listing, i), last].into_iter().enumerate() {
                            let r = Rect { y: p.name.y + (k + 1) as f32 * p.name.h, ..p.name };
                            g.text(r.x, baseline(r), s, self.content_font(12.0), t.text_dim, r.w);
                        }
                    }
                    LayoutMode::Grid => {
                        let size = self.cell_text(arena, listing, i, Column::Size);
                        let small = self.content_font(11.0);
                        let r = Rect { y: p.name.bottom(), h: self.cs(14.0), ..p.name };
                        let w = g.measure(size, small);
                        g.text(r.x + ((r.w - w) / 2.0).max(0.0).round(), baseline(r), size, small, t.text_dim, r.w);
                    }
                    _ => {}
                }
            }
            if let Some((from, dy)) = moved.take() {
                g.dl.shift_range(from..g.dl.instances.len(), 0.0, dy);
            }
        }
        g.dl.set_clip(None);
        self.motion |= spinning && self.animations;
        self.details.elided = elided;
        if self.details.checks.settle(checks, self.animations && self.prefs.checkboxes, now) {
            self.motion = true;
        }
        self.request_icons(requests);
        for r in thumb_requests {
            self.thumbs.insert(r.key, IconState::Pending);
            self.effects.push(Effect::Thumbnail(r));
        }
        for ext in type_requests {
            if !self.type_names.contains_key(&ext) {
                self.type_names.insert(ext.clone(), None);
                self.effects.push(Effect::TypeName(ext));
            }
        }
        let th = self.layout.thumb;
        if th.h > 0.0 {
            let hot = matches!(self.hover, Hit::Thumb | Hit::Track) || matches!(self.drag, Drag::Thumb { .. });
            g.dl.round_rect(th, if hot { t.scrollbar_hover } else { t.scrollbar }, th.w / 2.0);
        }
        self.draw_hbar(g, t);
    }
}
