use super::*;

impl App {
    pub(in crate::ui) fn layout_settings(&mut self) {
        let Some(mut ui) = self.settings.screen.take() else { return };
        let mut lines = Vec::new();
        let mut starts = Vec::new();
        match ui.query().to_owned().as_str() {
            "" => {
                for page in Page::ALL {
                    starts.push(Start { line: lines.len(), page, matches: 0 });
                    lines.push(Line::Group(page.title()));
                    self.page_lines(page, &mut lines);
                }
            }
            q => self.search_lines(q, &mut lines, &mut starts),
        }
        ui.lines = lines;
        ui.starts = starts;
        let (w, h, sc, describe) = (self.size.0, self.size.1, self.scale, self.settings.opts.describe);
        let s = |v: f32| (v * sc).round();
        let room_w = w - 2.0 * s(EDGE);
        let want = (w / 2.0).round().max(s(MIN_W)).min(s(MAX_W));
        let full = ui.fit.nav_w(s);
        let nav_w = match room_w {
            r if r - full >= s(MIN_W) => full,
            r if r - s(RAIL_W) >= s(RAIL_CONTENT_MIN) => s(RAIL_W),
            _ => 0.0,
        };
        ui.rail = nav_w == s(RAIL_W);
        let pw = want.min(room_w - nav_w).max(s(320.0));
        let total = pw + nav_w;
        let mut y = 0.0;
        let mut offsets = Vec::with_capacity(ui.lines.len());
        for l in &ui.lines {
            let lh = match l {
                Line::Buttons(..) => buttons_height(l, pw, &ui.fit, s),
                _ => l.height(describe, &ui.fit, s),
            };
            offsets.push((y, lh));
            y += lh;
        }
        ui.content_h = y;
        let x = ((w - total) / 2.0).round().max(0.0);
        let fixed = s(8.0) + s(FIELD_H) + s(6.0) + s(HINT_H);
        let top = crate::ui::palette::popup_top(sc, h, fixed + s(SETTING_H) * 4.0);
        let list_h = (h - 2.0 * top - fixed).max(s(SETTING_H) * 4.0).min((h - top - s(EDGE) - fixed).max(0.0));
        ui.search_rect = Rect::new(x + s(8.0), top + s(8.0), total - s(16.0) - 2.0 * (s(FIELD_H) + s(4.0)), s(FIELD_H));
        ui.describe = Rect::new(ui.search_rect.right() + s(4.0), ui.search_rect.y, s(FIELD_H), s(FIELD_H));
        ui.close = Rect::new(ui.describe.right() + s(4.0), ui.search_rect.y, s(FIELD_H), s(FIELD_H));
        let body = ui.search_rect.bottom() + s(6.0);
        ui.nav = Rect::new(x, body, nav_w, list_h);
        ui.list = Rect::new(x + nav_w, body, pw, list_h);
        ui.hints = Rect::new(x, ui.list.bottom(), total, s(HINT_H));
        ui.panel = Rect::new(x, top, total, ui.hints.bottom() - top);
        let max = (ui.content_h - ui.list.h).max(0.0);
        if let Some((p, smooth)) = ui.jump.take() {
            if let Some(&Start { line: k, .. }) = ui.starts.iter().find(|st| st.page == p) {
                let to = offsets[k].0.min(max);
                if smooth {
                    ui.scroll.target = to;
                } else {
                    ui.scroll.snap(to);
                }
                ui.held = Some((p, to));
                ui.focus = ui.focusable(k, 1).unwrap_or(k);
                ui.sub = 0;
            }
        }
        if ui.focus >= ui.lines.len() || !ui.lines[ui.focus].focusable() {
            ui.focus = ui.first_focusable();
            ui.sub = 0;
        }
        ui.sub = ui.sub.min(ui.lines.get(ui.focus).map_or(0, |l| l.count().saturating_sub(1)));
        let group_h = Line::Group("").height(describe, &ui.fit, s);
        if std::mem::take(&mut ui.reveal) {
            if let Some(&(fy, fh)) = offsets.get(ui.focus) {
                let lead = if matches!(ui.lines[ui.focus], Line::Group(_)) { 0.0 } else { group_h };
                if fy - lead < ui.scroll.target {
                    ui.scroll.target = (fy - lead).max(0.0);
                } else if fy + fh > ui.scroll.target + ui.list.h {
                    ui.scroll.target = fy + fh - ui.list.h;
                }
            }
        }
        ui.scroll.clamp(max);
        ui.page = match ui.held {
            Some((p, at)) if (ui.scroll.target - at).abs() < 0.5 => p,
            _ => {
                ui.held = None;
                let target = ui.scroll.target;
                ui.starts.iter().rev().find(|st| offsets[st.line].0 <= target + 0.5).or(ui.starts.first()).map_or(ui.page, |st| st.page)
            }
        };
        let n = ui.starts.len();
        ui.nav_rects = if nav_w > 0.0 && n > 0 {
            let item_h = ((list_h - 2.0 * s(NAV_PAD)) / n as f32).floor().clamp(s(NAV_ITEM_MIN), s(NAV_ITEM_H));
            (0..n).map(|k| Rect::new(ui.nav.x + s(NAV_PAD), ui.nav.y + s(NAV_PAD) + k as f32 * item_h, nav_w - 2.0 * s(NAV_PAD), item_h)).collect()
        } else {
            Vec::new()
        };
        ui.nav_at = ui.nav_at.min(n.saturating_sub(1));
        if ui.zone == Zone::Nav && ui.nav_rects.is_empty() {
            ui.zone = Zone::List;
        }
        ui.bar = if ui.content_h > ui.list.h { Rect::new(ui.list.right() - s(BAR_W), ui.list.y, s(BAR_W), ui.list.h) } else { Rect::default() };
        let row_w = ui.list.w - ui.bar.w;
        ui.rects = offsets.into_iter().map(|(ly, lh)| Rect::new(ui.list.x, ui.list.y + ly - ui.scroll.pos, row_w, lh)).collect();
        ui.sticky = ui.starts.iter().rposition(|st| ui.rects[st.line].y <= ui.list.y + 0.5).filter(|&at| ui.rects[ui.starts[at].line].y < ui.list.y - 0.5).map(|at| {
            let line = ui.starts[at].line;
            let next = ui.starts.get(at + 1).map_or(f32::INFINITY, |st| ui.rects[st.line].y);
            (line, Rect::new(ui.list.x, ui.list.y.min(next - group_h), row_w, group_h))
        });
        self.settings.screen = Some(ui);
    }

    pub(in crate::ui) fn settings_hit(&self, x: f32, y: f32) -> Option<Hit> {
        let ui = self.settings.screen.as_ref()?;
        if self.in_title(y) {
            return None;
        }
        let s = |v: f32| (v * self.scale).round();
        let h = |sh| Some(Hit::Settings(sh));
        if !ui.panel.contains(x, y) {
            return h(SHit::Outside);
        }
        if ui.close.contains(x, y) {
            return h(SHit::Close);
        }
        if ui.describe.contains(x, y) {
            return h(SHit::Describe);
        }
        if ui.search_rect.contains(x, y) {
            return h(SHit::Search);
        }
        if let Some(k) = ui.nav_rects.iter().position(|r| r.contains(x, y)) {
            return h(SHit::Nav(k));
        }
        if !ui.list.contains(x, y) {
            return h(SHit::Panel);
        }
        if ui.bar.contains(x, y) {
            let on_thumb = scroll_thumb(ui.list, ui.content_h, ui.scroll.pos, s).is_some_and(|t| y >= t.y && y < t.bottom());
            return h(if on_thumb { SHit::Thumb } else { SHit::Track });
        }
        if ui.sticky.is_some_and(|(_, r)| r.contains(x, y)) {
            return h(SHit::Panel);
        }
        let Some(k) = ui.rects.iter().position(|r| r.contains(x, y)) else { return h(SHit::Panel) };
        let (r, line) = (ui.rects[k], &ui.lines[k]);
        if let Some(b) = button_rects(line, r, &ui.fit, s).iter().position(|b| b.contains(x, y)) {
            return h(SHit::Button(k, b));
        }
        match line {
            Line::Setting(i) => match SETTINGS[*i].kind {
                Kind::Toggle(..) if switch_rect(r, s).contains(x, y) => h(SHit::Control(k)),
                Kind::Number { .. } if slider(*i) => {
                    let grip = slider_track(r, s).inset(-s(8.0), -s(10.0));
                    if grip.contains(x, y) {
                        h(SHit::Slide(k))
                    } else {
                        h(SHit::Line(k))
                    }
                }
                Kind::Number { .. } => {
                    let (minus, plus) = step_rects(r, s);
                    match (minus.contains(x, y), plus.contains(x, y)) {
                        (true, _) => h(SHit::Step(k, false)),
                        (_, true) => h(SHit::Step(k, true)),
                        _ => h(SHit::Line(k)),
                    }
                }
                Kind::Choice(..) | Kind::Text(..) | Kind::Pick(..) if control_rect(r, s).contains(x, y) => h(SHit::Control(k)),
                _ => h(SHit::Line(k)),
            },
            Line::Path(..) if remove_rect(r, s).contains(x, y) => h(SHit::Remove(k)),
            Line::MenuItem(row) => h(menu_editor::row_hit(row, r, k, (x, y), s)),
            _ => h(SHit::Line(k)),
        }
    }

    pub(in crate::ui) fn settings_a11y(&self, t: &mut Tree, targets: &mut Vec<(u64, Hit)>) -> Option<usize> {
        let ui = self.settings.screen.as_ref()?;
        let s = |v: f32| (v * self.scale).round();
        let at = Hit::Settings;
        let d = t.add(0, SETTINGS_ID, Role::Dialog, S::SettingsPageLineTitle.t(), ui.panel);
        put(t, targets, d, at(SHit::Close), Role::Button, S::A11yButtonClose.t(), ui.close);
        let search = put(t, targets, d, at(SHit::Search), Role::Edit, S::SettingsPageLineSearch.t(), ui.search_rect);
        t.nodes[search].value = ui.search.text().to_owned();
        let searching = !ui.query().is_empty();
        let describe = put(t, targets, d, at(SHit::Describe), Role::Button, S::OptionsDescriptions.t(), ui.describe);
        t.nodes[describe].selected = self.settings.opts.describe;
        let mut focus = if ui.searching() { search } else { d };
        if !ui.nav_rects.is_empty() {
            let nav = t.add(d, CATEGORIES_ID, Role::List, S::SettingsPageLineCategories.t(), ui.nav);
            for (k, (&r, st)) in ui.nav_rects.iter().zip(&ui.starts).enumerate() {
                let n = put(t, targets, nav, at(SHit::Nav(k)), Role::ListItem, st.page.title(), r);
                t.nodes[n].selected = st.page == ui.page;
                if searching {
                    t.nodes[n].value = crate::trn!(P::SettingsPageMatches, st.matches);
                }
                if ui.zone == Zone::Nav && k == ui.nav_at {
                    focus = n;
                }
            }
        }
        let list = t.add(d, LIST_ID, Role::Group, if searching { S::SettingsPageLineSearch.t() } else { ui.page.title() }, ui.list);
        let mut section = list;
        for (k, (line, &r)) in ui.lines.iter().zip(&ui.rects).enumerate() {
            if r.bottom() < ui.list.y || r.y > ui.list.bottom() {
                continue;
            }
            let id = leaf(at(SHit::Line(k)));
            let row = match line {
                Line::Group(title) => {
                    section = t.add(list, id, Role::Group, *title, r);
                    None
                }
                Line::Note(text) => Some(t.add(section, id, Role::Text, *text, r)),
                Line::Info(label, value) => {
                    let n = t.add(section, id, Role::Text, *label, r);
                    t.nodes[n].value = value.clone();
                    Some(n)
                }
                Line::Setting(i) => {
                    let set = &SETTINGS[*i];
                    let (role, hit) = match set.kind {
                        Kind::Toggle(..) => (Role::CheckBox, SHit::Control(k)),
                        Kind::Choice(..) => (Role::ComboBox, SHit::Control(k)),
                        Kind::Text(..) | Kind::Pick(..) => (Role::Button, SHit::Control(k)),
                        _ => (Role::Group, SHit::Line(k)),
                    };
                    let n = put(t, targets, section, at(hit), role, set.title.t(), r);
                    t.nodes[n].help = if RESTART.contains(&set.id) { format!("{} {}", set.help.t(), S::OptionsRestart.t()) } else { set.help.t().to_owned() };
                    t.nodes[n].value = self.setting_value_text(*i);
                    t.nodes[n].selected = set.get(self) == Value::Bool(true);
                    if let (Kind::Number { .. }, false) = (set.kind, slider(*i)) {
                        let (minus, plus) = step_rects(r, s);
                        put(t, targets, n, at(SHit::Step(k, false)), Role::Button, S::A11yButtonDecrease.t(), minus);
                        put(t, targets, n, at(SHit::Step(k, true)), Role::Button, S::A11yButtonIncrease.t(), plus);
                    }
                    Some(n)
                }
                Line::Off(i) => {
                    let set = &SETTINGS[*i];
                    let role = match set.kind {
                        Kind::Toggle(..) => Role::CheckBox,
                        Kind::Choice(..) => Role::ComboBox,
                        _ => Role::Group,
                    };
                    let n = t.add(section, id, role, set.title.t(), r);
                    t.nodes[n].help = self.setting_help(line).into_owned();
                    t.nodes[n].value = self.setting_value_text(*i);
                    t.nodes[n].selected = set.get(self) == Value::Bool(true);
                    t.nodes[n].enabled = false;
                    Some(n)
                }
                Line::Path(i, n) => {
                    let p = t.add(section, id, Role::Text, self.paths_of(*i).get(*n).map(settings::path_text).unwrap_or_default(), r);
                    put(t, targets, p, at(SHit::Remove(k)), Role::Button, S::A11yButtonRemove.t(), remove_rect(r, s));
                    Some(p)
                }
                Line::Tag(n) => Some(t.add(section, id, Role::Group, self.tag_defs().get(*n).map_or("", |tag| tag.name.as_str()), r)),
                Line::Entry(e) => Some(t.add(section, id, Role::Group, self.user_entry_name(*e), r)),
                Line::MenuItem(row) => {
                    let n = put(t, targets, section, at(SHit::Check(k)), Role::CheckBox, row.label(), r);
                    t.nodes[n].selected = !row.hidden;
                    if row.submenu {
                        t.nodes[n].value = if row.open { S::MenuEditorOpen.t() } else { S::MenuEditorClosed.t() }.to_owned();
                    } else {
                        let star = put(t, targets, n, at(SHit::Star(k)), Role::Button, S::MenuEditorPin.t(), menu_editor::star_rect(r, s));
                        t.nodes[star].selected = row.pinned;
                    }
                    Some(n)
                }
                Line::Buttons(..) => None,
            };
            let mut subs = Vec::new();
            for (b, br) in button_rects(line, r, &ui.fit, s).into_iter().enumerate() {
                let name = line.buttons().get(b).map_or("", |btn| btn.label());
                subs.push(put(t, targets, row.unwrap_or(section), at(SHit::Button(k, b)), Role::Button, name, br));
            }
            if ui.zone == Zone::List && k == ui.focus {
                focus = subs.get(ui.sub).copied().or(row).unwrap_or(focus);
            }
        }
        Some(focus)
    }

    pub(in crate::ui) fn settings_a11y_focus(&mut self, sh: SHit) {
        let Some(ui) = &mut self.settings.screen else { return };
        match sh {
            SHit::Search => ui.zone = Zone::Search,
            SHit::Nav(k) => (ui.zone, ui.nav_at) = (Zone::Nav, k),
            SHit::Line(k) | SHit::Control(k) | SHit::Slide(k) | SHit::Step(k, _) | SHit::Remove(k) | SHit::Check(k) | SHit::Star(k) | SHit::Fold(k) => (ui.zone, ui.focus, ui.sub) = (Zone::List, k, 0),
            SHit::Button(k, b) => (ui.zone, ui.focus, ui.sub) = (Zone::List, k, b),
            _ => {}
        }
    }

    pub(in crate::ui) fn settings_ime_area(&self) -> Option<Rect> {
        self.settings.screen.as_ref().filter(|s| s.searching() && self.nav.palette.is_none()).map(|s| s.search_rect)
    }

    pub(in crate::ui) fn settings_nav_tip(&self, k: usize) -> Option<(Rect, String)> {
        let ui = self.settings.screen.as_ref().filter(|ui| ui.rail)?;
        let (r, st) = (*ui.nav_rects.get(k)?, ui.starts.get(k)?);
        let text = match ui.query() {
            "" => st.page.title().to_owned(),
            _ => format!("{} · {}", st.page.title(), crate::trn!(P::SettingsPageMatches, st.matches)),
        };
        Some((r, text))
    }
}
