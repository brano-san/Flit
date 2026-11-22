use super::*;

impl App {
    pub(super) fn layout(&mut self) {
        if self.tabs.panels.here != self.tabs.panels.active {
            let r = self.panel_rect();
            return self.layout_panel(r);
        }
        self.layout_panels();
        self.layout_jobs();
        self.layout_settings();
    }

    pub(super) fn layout_panel(&mut self, area: Rect) {
        let active = self.tabs.panels.here == self.tabs.panels.active;
        let (caption, inset) = self.panel_edges(area);
        let ops_button = active && self.ops_button_visible();
        let status_bar = active && (self.dialogs.picker.is_some() || self.prefs.status_bar || self.tab().error.is_some() || self.tab().filter.is_some());
        let (n, actions) = (self.tabs.items.len(), self.tab_actions_button);
        let at = if active && self.focus == Focus::TabBar && self.kb < n { self.kb } else { self.tabs.cur };
        let target = (at, self.tabs.items[at].id);
        let sc = self.scale;
        let s = |v: f32| (v * sc).round();
        let status_h = if active && self.dialogs.picker.is_some() { s(self.picker_height(area.w)) } else { self.status_slide(if status_bar { s(STATUS_H) } else { 0.0 }) };
        let l = &mut self.layout;
        l.row_h = s(ROW_H);
        l.title = Rect::new(area.x, area.y, area.w, s(TAB_H));
        let limit = l.title.right().min(caption);
        let x0 = (area.x + inset + s(6.0)).min(limit);
        let bh = l.title.h - s(8.0);
        let button_y = area.y + s(4.0) + ((l.title.h - s(4.0) - bh) / 2.0).round();
        let button = |x: f32| Rect::new(x, button_y, s(30.0), bh);
        l.actions = if actions && limit - x0 >= s(34.0) { Rect::new(limit - s(34.0), area.y + ((l.title.h - bh) / 2.0).round(), s(30.0), bh) } else { Rect::default() };
        let end = if l.actions.w > 0.0 { l.actions.x } else { limit };
        let room = (end - s(40.0) - x0).max(0.0);
        let (min_w, arrow_w) = (s(TAB_MIN_W), s(TAB_ARROW_W));
        let squeezed = n > 1 && n as f32 * min_w > room;
        let over = squeezed && room >= 2.0 * arrow_w + min_w;
        let lane = if over { room - 2.0 * arrow_w } else { room };
        let fit = if squeezed { ((lane / min_w).floor() as usize).clamp(1, n) } else { n };
        let tw = if squeezed { (lane / fit as f32).floor() } else { s(TAB_W).min((room / n.max(1) as f32).floor()) };
        let first = l.strip.place(target, n, fit);
        let x_tabs = x0 + if over { arrow_w } else { 0.0 };
        l.tabs.clear();
        for i in 0..n {
            let shown = (first..first + fit).contains(&i);
            let x = x_tabs + (i.clamp(first, first + fit) - first) as f32 * tw;
            let r = Rect::new(x, area.y + s(4.0), if shown { tw } else { 0.0 }, l.title.h - s(4.0));
            let c = if shown && tw >= s(26.0) { Rect::new(r.right() - s(26.0), r.y + ((r.h - s(20.0)) / 2.0).round(), s(20.0), s(20.0)) } else { Rect::default() };
            l.tabs.push((r, c));
        }
        let lane_end = x_tabs + (n.min(first + fit) - first) as f32 * tw;
        l.tab_arrows = if over { [Rect::new(x0, button_y, arrow_w, bh), Rect::new(lane_end, button_y, arrow_w, bh)] } else { [Rect::default(); 2] };
        let next_x = lane_end + if over { arrow_w } else { 0.0 } + s(4.0);
        l.new_tab = if next_x + s(30.0) <= end { button(next_x) } else { Rect::default() };

        l.toolbar = Rect::new(area.x, l.title.bottom(), area.w, s(TOOL_H));
        for (i, r) in l.nav.iter_mut().enumerate() {
            *r = Rect::new(area.x + s(6.0) + i as f32 * s(NAV_W), l.toolbar.y + s(4.0), s(NAV_W), s(TOOL_H - 8.0));
        }
        l.nav[3] = Rect::new(l.nav[2].right(), l.nav[2].y, s(VISITED_W), l.nav[2].h);
        l.view = Rect::new(l.nav[3].right() + s(9.0), l.nav[3].y, s(NAV_W), l.nav[3].h);
        l.bookmark = Rect::new(l.view.right(), l.view.y, s(NAV_W), l.view.h);
        let ax = l.bookmark.right() + s(9.0);
        l.more = Rect::new(area.right() - s(6.0) - s(NAV_W), l.nav[0].y, s(NAV_W), l.nav[0].h);
        l.ops_button = if ops_button { Rect::new(l.more.x - s(NAV_W), l.nav[0].y, s(NAV_W), l.nav[0].h) } else { Rect::default() };
        let edge = if ops_button { l.ops_button.x } else { l.more.x };
        l.address = Rect::new(ax, l.toolbar.y + s(5.0), (edge - s(9.0) - ax).max(s(40.0)), s(TOOL_H - 10.0));
        l.header = Rect::new(area.x, l.toolbar.bottom(), area.w, s(HEADER_H));
        l.status = Rect::new(area.x, area.bottom() - status_h, area.w, status_h);
        l.bottom = l.status.y;
        if !status_bar {
            l.encoding = Rect::default();
            l.bin = Default::default();
            l.branch = Rect::default();
        }
        if active {
            self.layout_preview();
        }
        let l = &self.layout;
        let content = Rect::new(l.header.x, l.toolbar.bottom(), l.header.w, (l.bottom - l.toolbar.bottom()).max(0.0));
        self.layout_pane(content);
    }

    fn layout_pane(&mut self, area: Rect) {
        let (kinds, widths) = (self.column_kinds(), self.tab().columns);
        let sc = self.content_px();
        let s = |v: f32| (v * sc).round();
        let l = &mut self.layout;
        l.header = Rect::new(area.x, area.y, area.w, s(HEADER_H));
        l.bottom = area.bottom();
        self.layout_git_bar();
        self.layout_look();
        let now = self.clock();
        let animate = self.animations && !matches!(self.drag, Drag::ColSep { .. });
        let shown = self.tab_mut().motion.column_widths(kinds, widths.ordered(kinds).into_iter().map(|c| (c, if widths.shown(c) { s(widths.width(c)) } else { 0.0 })), animate, now);
        self.motion |= shown.1;
        let l = &mut self.layout;
        l.columns.clear();
        l.columns.extend(shown.0.iter().map(|&(c, _)| c));
        let fixed: f32 = shown.0.iter().filter(|&&(c, _)| c != Column::Name).map(|&(_, w)| w.floor()).sum();
        let name_w = if widths.width(Column::Name) > 0.0 { s(widths.width(Column::Name)) } else { (l.list.w - fixed).max(s(NAME_MIN_W)).floor() };
        let total = if l.mode.table() { name_w + fixed } else { 0.0 };
        let sx = self.table_scroll(total, s(SCROLLBAR_W));
        let l = &mut self.layout;
        let mut x = l.list.x - sx;
        l.cols.clear();
        for &(c, w) in &shown.0 {
            let cw = if c == Column::Name { name_w } else { w.floor() };
            l.cols.push(Rect::new(x, l.header.y, cw, l.header.h));
            x += cw;
        }
        if std::mem::take(&mut self.tab_mut().reveal_pending) {
            self.reveal_focus(true);
        }
        let f = self.flow();
        let (max_scroll, len, content) = (self.max_scroll_of(&f), self.view_len(), f.extent());
        let t = self.tab_mut();
        t.scroll_target = t.scroll_target.clamp(0.0, max_scroll);
        t.scroll = t.scroll.clamp(0.0, max_scroll);
        let frac = if max_scroll > 0.0 { t.scroll / max_scroll } else { 0.0 };
        let l = &mut self.layout;
        let tr = l.track;
        l.thumb = match (content > len, l.flow.across) {
            (false, _) => Rect::default(),
            (true, false) => {
                let th = (tr.h * len / content).max(s(24.0)).min(tr.h);
                Rect::new(tr.x + s(3.0), tr.y + (tr.h - th) * frac, tr.w - s(6.0), th)
            }
            (true, true) => {
                let tw = (tr.w * len / content).max(s(24.0)).min(tr.w);
                Rect::new(tr.x + (tr.w - tw) * frac, tr.y + s(3.0), tw, tr.h - s(6.0))
            }
        };
        self.sync_panes();
        self.layout_filter();
        self.layout_home();
    }

    pub(super) fn hit(&self, x: f32, y: f32) -> Hit {
        if let Some(h) = self.jobs_hit(x, y).or_else(|| self.palette_hit(x, y)).or_else(|| self.settings_hit(x, y)) {
            return h;
        }
        if let Some(h) = self.chrome_hit(x, y).or_else(|| self.nav_hit(x, y)).or_else(|| self.preview_hit(x, y)).or_else(|| self.sep_hit(x, y)) {
            return h;
        }
        let l = &self.layout;
        if l.title.contains(x, y) {
            return row_hit(l, x, y);
        }
        if l.toolbar.contains(x, y) {
            if let Some(i) = l.nav.iter().position(|r| r.contains(x, y)) {
                return Hit::Nav(i);
            }
            if l.view.contains(x, y) {
                return Hit::ViewMode;
            }
            if l.bookmark.contains(x, y) {
                return Hit::Bookmark;
            }
            if l.more.contains(x, y) {
                return Hit::PanelMenu;
            }
            if l.address.contains(x, y) {
                if let Some(k) = l.crumb_arrows.iter().position(|r| r.contains(x, y)) {
                    return Hit::CrumbScroll(k == 1);
                }
                if let Some(i) = l.crumb_chevrons.iter().position(|r| r.contains(x, y)) {
                    return Hit::CrumbChevron(i);
                }
                if let Some(i) = l.crumbs.iter().position(|r| r.contains(x, y)) {
                    return Hit::Crumb(i);
                }
                return Hit::Address;
            }
            return Hit::None;
        }
        if l.header.contains(x, y) {
            let (grip, lo, hi) = (self.s(4.0), l.list.x, l.list.right());
            let near = l.cols.iter().map(|r| r.right()).enumerate().filter(|&(_, e)| (lo..=hi + 0.5).contains(&e) && (x - e).abs() <= grip);
            if let Some((i, _)) = near.min_by(|a, b| (x - a.1).abs().total_cmp(&(x - b.1).abs())) {
                return Hit::ColSep(i);
            }
            if x < lo || x >= hi {
                return Hit::HeaderEmpty;
            }
            return l.cols.iter().position(|r| r.contains(x, y)).map_or(Hit::HeaderEmpty, Hit::Header);
        }
        if l.htrack.contains(x, y) {
            return if l.hthumb.contains(x, y) { Hit::HThumb } else { Hit::HTrack };
        }
        if l.track.contains(x, y) {
            let th = l.thumb;
            return if y >= th.y && y < th.bottom() && th.h > 0.0 { Hit::Thumb } else { Hit::Track };
        }
        if let Some(i) = l.panes.iter().position(|r| r.contains(x, y)).filter(|_| l.pane_area.contains(x, y)) {
            return self.pane_hit(i, y);
        }
        if self.grant_rect().is_some_and(|r| r.contains(x, y)) {
            return Hit::GrantAccess;
        }
        if self.eject_rect().is_some_and(|r| r.contains(x, y)) {
            return Hit::Eject;
        }
        if self.network_settings_rect().is_some_and(|r| r.contains(x, y)) {
            return Hit::NetworkSettings;
        }
        if l.list.inset(-l.gutter, 0.0).contains(x, y) {
            return self.list_hit(x, y);
        }
        if let Some(h) = self.bottom_hit(x, y) {
            return h;
        }
        if l.encoding.contains(x, y) {
            return Hit::Encoding;
        }
        if l.git_bar[2].contains(x, y) {
            return Hit::GitAbort;
        }
        if l.git_bar[3].contains(x, y) {
            return Hit::GitConflicts;
        }
        if l.branch.contains(x, y) {
            return Hit::Branch;
        }
        if let Some(k) = l.bin.iter().position(|r| r.contains(x, y)) {
            return Hit::Bin(k);
        }
        Hit::None
    }

    pub fn hit_test(&self, x: f32, y: f32) -> HitArea {
        if self.over_own_menu(x, y) || self.dialogs.confirm.as_ref().is_some_and(|c| c.rect.contains(x, y)) {
            return HitArea::Client;
        }
        match self.hit(x, y) {
            Hit::Caption(0) => HitArea::Minimize,
            Hit::Caption(1) => HitArea::Maximize,
            Hit::Caption(_) => HitArea::Close,
            Hit::None if !self.system_title && self.in_title(y) && self.parked_row_hit(x, y).is_none() => HitArea::Caption,
            _ => HitArea::Client,
        }
    }
}
