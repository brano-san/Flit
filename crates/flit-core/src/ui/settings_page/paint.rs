use super::*;
use crate::ui::MenuOwner;

impl App {
    pub(super) fn setting_help(&self, line: &Line) -> std::borrow::Cow<'static, str> {
        match line {
            Line::Off(i) if self.about.system_integration_unavailable && matches!(SETTINGS[*i].id, settings::AUTOSTART | settings::PROTOCOL_LINKS) => S::SettingsPageSystemUnavailable.t().into(),
            Line::Off(i) if settings::NEEDS_BUILTIN_ARCHIVES.contains(&SETTINGS[*i].id) => format!("{} {}", S::SettingsPageLineNeedsBuiltinArchives.t(), SETTINGS[*i].help.t()).into(),
            Line::Off(i) if SETTINGS[*i].id == settings::BACKGROUND => S::BackgroundUnavailable.t().into(),
            Line::Off(i) => SETTINGS[*i].help.t().into(),
            Line::Setting(i) => SETTINGS[*i].help.t().into(),
            _ => "".into(),
        }
    }

    pub(super) fn setting_value_text(&self, i: usize) -> String {
        match (SETTINGS[i].kind, SETTINGS[i].get(self)) {
            (Kind::Choice(c, ..), Value::Choice(k)) => c.get(k).map_or("", |c| c.1.t()).to_owned(),
            (Kind::Number { zero: Some(off), .. }, Value::Number(0)) => off.t().to_owned(),
            (Kind::Number { unit, .. }, Value::Number(n)) => format!("{n} {unit}"),
            (Kind::Text(..), _) => self.text_value(i),
            (Kind::Paths(..), Value::Paths(p)) => crate::trn!(P::SettingsPagePages, p.len()),
            (Kind::Pick(..), _) => self.pick_text(i).unwrap_or_default(),
            _ => String::new(),
        }
    }

    pub(in crate::ui) fn fit_settings<B: TextBackend>(&mut self, g: &mut Gfx<B>) {
        if self.settings.screen.is_none() {
            return;
        }
        let font = self.ui_font(13.0);
        let fit = Fit {
            body: line_h(g.metrics(font)),
            small: line_h(g.metrics(self.ui_font(12.0))),
            bold: line_h(g.metrics(FontStyle::new(FontFamily::Ui, 14.0 * self.scale, true))),
            labels: Btn::ALL.map(|b| g.measure(b.label(), font)),
            titles: Page::ALL.iter().map(|p| g.measure(p.title(), font)).fold(0.0, f32::max),
        };
        if let Some(ui) = self.settings.screen.as_mut().filter(|ui| ui.fit != fit) {
            ui.fit = fit;
            self.layout_settings();
        }
    }

    pub(in crate::ui) fn draw_settings<B: TextBackend>(&mut self, g: &mut Gfx<B>, t: &Theme) {
        let Some(mut ui) = self.settings.screen.take() else { return };
        let open = match self.menu_owner {
            Some(MenuOwner::Button(Hit::Settings(SHit::Control(k)))) if self.own_menu_open() => Some(k),
            _ => None,
        };
        if ui.turn.aim(open, self.animations, self.last_frame.unwrap_or_else(Instant::now)) {
            self.motion = true;
        }
        ui.knob_motion = false;
        if ui.lines.iter().any(|l| matches!(l, Line::Entry(_))) {
            self.request_user_line_images(g);
        }
        self.paint_settings(g, t, &mut ui);
        self.motion |= ui.knob_motion;
        self.settings.screen = Some(ui);
    }

    fn paint_settings<B: TextBackend>(&self, g: &mut Gfx<B>, t: &Theme, ui: &mut SettingsUi) {
        let sc = self.scale;
        let s = |v: f32| (v * sc).round();
        let (font, small, bold) = (self.ui_font(13.0), self.ui_font(12.0), FontStyle::new(FontFamily::Ui, 14.0 * sc, true));
        let icons = self.icon_font(10.0);
        let (s4, s6, s8) = (s(4.0), s(6.0), s(8.0));
        let p = ui.panel;
        Paint::new(g, t, sc).frame(p);
        let (mf, ms) = (g.metrics(font), g.metrics(small));
        let mid = |r: Rect, m: crate::text::FontMetrics| (r.y + (r.h + m.ascent - m.descent) / 2.0).round();
        let hover = self.hover;
        let query = ui.query().to_owned();
        let describe = self.settings.opts.describe;
        let sr = ui.search_rect;
        let focused_search = ui.searching() && self.active && self.nav.palette.is_none();
        Paint::new(g, t, sc).field(sr, focused_search);
        centered(g, Rect::new(sr.x + s4, sr.y, s(24.0), sr.h), G_SEARCH, icons, t.text_dim);
        let inner = Rect::new(sr.x + s(32.0), sr.y, sr.w - s(40.0), sr.h);
        ui.search.draw(g, t, inner, font, focused_search, S::OptionsPlaceholder.t(), sc, Some(&self.preedit));
        let big_icons = self.icon_font(12.0);
        Paint::new(g, t, sc).toggle_button(ui.describe, G_DESCRIBE, big_icons, describe, self.lit(Hit::Settings(SHit::Describe)));
        Paint::new(g, t, sc).icon_button(ui.close, G_CLOSE, icons, self.button(Hit::Settings(SHit::Close), false, true), t.text_dim);
        let hints = ui.hints;
        {
            let mut w = Paint::new(g, t, sc);
            w.separator(hints.x + s(12.0), hints.y, hints.w - s(24.0));
            let keys = [S::WidgetsKeyUp.t(), S::WidgetsKeyDown.t()];
            let all: [(&[&str], &str); 4] = [(&[S::WidgetsKeyTab.t()], S::SettingsPageLineSwitchArea.t()), (&keys, S::WidgetsHintNavigate.t()), (&[S::WidgetsKeyEnter.t()], S::WidgetsHintUse.t()), (&[S::WidgetsKeyEscape.t()], S::WidgetsHintDismiss.t())];
            w.hints_within(hints, &all[usize::from(ui.nav_rects.is_empty())..], small);
        }
        self.paint_settings_nav(g, t, ui, (font, small), (mf, ms));
        let list = ui.list;
        g.dl.set_clip(Some(list));
        let mut cut = Vec::new();
        let list_focused = ui.zone == Zone::List && self.active && self.nav.palette.is_none();
        for (k, (line, &r)) in ui.lines.iter().zip(&ui.rects).enumerate() {
            if r.bottom() < list.y || r.y > list.bottom() {
                continue;
            }
            let focused = list_focused && k == ui.focus;
            let row = Rect::new(r.x + s4, r.y + 1.0, r.w - s8, r.h - 2.0);
            if line.focusable() && !matches!(line, Line::Buttons(..)) {
                let lit = self.lit(Hit::Settings(SHit::Line(k)));
                if lit > 0.0 {
                    g.dl.round_rect(row, crate::ui::widgets::faded(t.hover, lit), s6);
                }
                if focused {
                    g.dl.outline(row, t.accent, s6, sc.round().max(1.0));
                }
            }
            match line {
                Line::Group(title) => paint_group(g, t, r, title, bold, s),
                Line::Note(text) => {
                    g.text(r.x + s(PAD), mid(r, mf), text, font, t.text_dim, r.w - 2.0 * s(PAD));
                }
                Line::Info(label, value) => {
                    let lw = info_label_w(r, s);
                    g.text(r.x + s(PAD), mid(r, ms), label, small, t.text_dim, lw);
                    g.text(r.x + s(PAD) + lw + s(10.0), mid(r, ms), value, small, t.text, r.w - s(PAD) * 2.0 - lw - s(10.0));
                }
                Line::Setting(i) | Line::Off(i) => {
                    let set = &SETTINGS[*i];
                    let tx = r.x + s(PAD) + s(indent(*i));
                    let restart = RESTART.contains(&set.id);
                    let text_w = label_w(*i, r, s);
                    let (tm, hm) = (title_marks(&query, set.title.t()), help_marks(&query, set.help.t()));
                    let (ty, help_y) = if describe { text_baselines(r, mf, ms, s) } else { (mid(r, mf), 0.0) };
                    Paint::new(g, t, sc).marked((tx, ty), set.title.t(), &tm, font, (t.text, t.accent), text_w);
                    if describe {
                        let help = self.setting_help(line);
                        let marks = if matches!(line, Line::Setting(_)) { hm } else { Vec::new() };
                        Paint::new(g, t, sc).marked((tx, help_y), &help, &marks, small, (t.text_dim, t.accent), text_w);
                        if g.measure(&help, small) > text_w {
                            cut.push((k, Rect::new(tx, help_y - ms.ascent, text_w, ms.ascent + ms.descent)));
                        }
                    }
                    if restart {
                        let c = control_rect(r, s);
                        centered(g, Rect::new(c.x - s(24.0), r.y, s(20.0), r.h), G_RESTART, icons, t.text_dim);
                    }
                    let value = self.setting_value_text(*i);
                    match (set.kind, set.get(self)) {
                        (Kind::Number { min, max, .. }, Value::Number(n)) if slider(*i) => {
                            let c = control_rect(r, s);
                            g.text(c.x, mid(c, ms), &value, small, t.text_dim, s(52.0));
                            let frac = if max > min { (n - min) as f32 / (max - min) as f32 } else { 0.0 };
                            let hot = if self.drag == crate::ui::Drag::Slide(k) { 1.0 } else { self.lit(Hit::Settings(SHit::Slide(k))) };
                            Paint::new(g, t, sc).slider(slider_track(r, s), frac, hot);
                        }
                        (Kind::Toggle(..), Value::Bool(on)) => {
                            let hot = self.lit(Hit::Settings(SHit::Control(k)));
                            let to = if on { 1.0 } else { 0.0 };
                            let now = self.last_frame.unwrap_or_else(Instant::now);
                            let knob = match ui.knobs.iter().position(|(x, _)| x == i) {
                                Some(n) => n,
                                None => {
                                    ui.knobs.push((*i, crate::ui::widgets::Tween::at(to)));
                                    ui.knobs.len() - 1
                                }
                            };
                            ui.knob_motion |= ui.knobs[knob].1.aim(to, self.animations, now, KNOB);
                            Paint::new(g, t, sc).switch(switch_rect(r, s), ui.knobs[knob].1.value(now, KNOB), hot, self.icon_font(8.0));
                        }
                        (Kind::Choice(..), _) => {
                            let h = Hit::Settings(SHit::Control(k));
                            let angle = ui.turn.angle(k, self.last_frame.unwrap_or_else(Instant::now));
                            Paint::new(g, t, sc).picker(control_rect(r, s), &value, font, angle, self.menu_open_from(h), hover == h);
                        }
                        (Kind::Text(..) | Kind::Pick(..), _) => {
                            let c = control_rect(r, s);
                            g.dl.round_rect(c, crate::theme::mix(t.input_bg, t.button_hover, self.lit(Hit::Settings(SHit::Control(k)))), s4);
                            g.dl.outline(c, t.input_border, s4, 1.0);
                            let mut tx = c.x + s(10.0);
                            if let Some(color) = self.swatch(*i) {
                                let sw = Rect::new(tx, c.y + (c.h - s(16.0)) / 2.0, s(16.0), s(16.0));
                                g.dl.round_rect(sw, color, s4);
                                g.dl.outline(sw, t.input_border, s4, 1.0);
                                tx = sw.right() + s(8.0);
                            }
                            g.text(tx, mid(c, mf), &value, font, t.text, c.right() - s(30.0) - tx);
                            centered(g, Rect::new(c.right() - s(30.0), c.y, s(24.0), c.h), G_DOWN, icons, t.text_dim);
                        }
                        (Kind::Number { .. }, _) => {
                            let lit = [false, true].map(|up| self.lit(Hit::Settings(SHit::Step(k, up))));
                            Paint::new(g, t, sc).stepper(control_rect(r, s), &value, font, icons, lit);
                        }
                        _ => {
                            let c = control_rect(r, s);
                            let vw = g.measure(&value, small).min(c.w);
                            g.text(c.right() - vw, mid(r, ms), &value, small, t.text_dim, c.w);
                        }
                    }
                    if let Line::Off(_) = line {
                        g.dl.rect(row, t.bg.alpha(0xA0));
                    }
                }
                Line::Path(i, n) => {
                    let text = self.paths_of(*i).get(*n).map(settings::path_text).unwrap_or_default();
                    g.text(r.x + s(32.0), mid(r, mf), &text, font, t.text, r.w - s(PAD) * 2.0 - s(60.0));
                    let rr = remove_rect(r, s);
                    let lit = self.lit(Hit::Settings(SHit::Remove(k)));
                    if lit > 0.0 {
                        g.dl.round_rect(rr, crate::ui::widgets::faded(t.button_hover, lit), s4);
                    }
                    centered(g, rr, G_CLOSE, self.icon_font(8.0), t.text_dim);
                }
                Line::Tag(n) => {
                    if let Some(tag) = self.tag_defs().get(*n) {
                        let dot = s(12.0);
                        g.dl.round_rect(Rect::new(r.x + s(PAD), r.y + (r.h - dot) / 2.0, dot, dot), Color::rgb(tag.color), dot / 2.0);
                        let count = self.tags().count(*n);
                        let label = crate::trn!(P::SettingsPageTagItems, count, color = tags::color_name(tag.color));
                        let limit = button_rects(line, r, &ui.fit, s).first().map_or(r.right(), |b| b.x) - s(BUTTON_GAP);
                        let nx = r.x + s(PAD) + dot + s(10.0);
                        let nw = g.text(nx, mid(r, mf), &tag.name, font, t.text, s(220.0).min(limit - nx).max(0.0));
                        let lx = nx + nw + s(14.0);
                        g.text(lx, mid(r, ms), &label, small, t.text_dim, s(220.0).min(limit - lx).max(0.0));
                    }
                }
                Line::Entry(at) => self.draw_user_line(g, t, r, *at, (font, small, icons)),
                Line::MenuItem(row) => self.draw_menu_row(g, t, r, k, row, (font, icons)),
                Line::Buttons(..) => {}
            }
            for (b, (br, btn)) in button_rects(line, r, &ui.fit, s).into_iter().zip(line.buttons()).enumerate() {
                let hot = self.lit(Hit::Settings(SHit::Button(k, b)));
                let danger = matches!(btn, Btn::Reset | Btn::Delete);
                g.dl.round_rect(br, crate::theme::mix(t.button_hover, t.button_pressed, hot), s4);
                g.dl.outline(br, if focused && ui.sub == b { t.accent } else { t.input_border }, s4, if focused && ui.sub == b { sc.round().max(1.0) } else { 1.0 });
                let glyph_w = glyph_w(btn, s);
                if glyph_w > 0.0 {
                    centered(g, Rect::new(br.x + s(8.0), br.y, glyph_w, br.h), G_PLUS, self.icon_font(9.0), t.accent);
                }
                let lw = g.measure(btn.label(), font).min(br.w - s(16.0) - glyph_w);
                g.text((br.x + glyph_w + (br.w - glyph_w - lw) / 2.0).round(), mid(br, mf), btn.label(), font, if danger { t.error } else { t.text }, br.w - s(16.0) - glyph_w);
            }
        }
        if let crate::ui::Drag::MenuMove { to: Some(to), .. } = self.drag {
            let rows = || ui.lines.iter().zip(&ui.rects).filter_map(|(l, r)| if let Line::MenuItem(row) = l { Some((row.depth, *r)) } else { None });
            let at = rows().filter(|(depth, _)| *depth == 0).nth(to).map(|(_, r)| r.y);
            if let Some(y) = at.or_else(|| rows().next_back().map(|(_, r)| r.bottom())) {
                g.dl.rect(Rect::new(list.x + s(PAD), y - 1.0, list.w - 2.0 * s(PAD), s(2.0)), t.accent);
            }
        }
        if let Some((k, r)) = ui.sticky {
            g.dl.rect(r, t.popup);
            if let Some(Line::Group(title)) = ui.lines.get(k) {
                paint_group(g, t, r, title, bold, s);
            }
            Paint::new(g, t, sc).separator(r.x + s(12.0), r.bottom() - 1.0, r.w - s(24.0));
        }
        g.dl.set_clip(None);
        if let Some(th) = scroll_thumb(list, ui.content_h, ui.scroll.pos, s) {
            let hot = matches!(hover, Hit::Settings(SHit::Thumb | SHit::Track)) || matches!(self.drag, crate::ui::Drag::OptionsThumb { .. });
            let bar = if hot { s(6.0) } else { s(4.0) };
            g.dl.round_rect(Rect::new(th.right() - s(3.0) - bar, th.y, bar, th.h), if hot { t.scrollbar_hover } else { t.scrollbar }, bar / 2.0);
        }
        ui.cut = cut;
        if let Some((k, (x, y))) = ui.tip {
            if let Some(line @ (Line::Setting(_) | Line::Off(_))) = ui.lines.get(k) {
                self.paint_tip(g, t, &self.setting_help(line), (x, y));
            }
        }
    }

    fn paint_settings_nav<B: TextBackend>(&self, g: &mut Gfx<B>, t: &Theme, ui: &SettingsUi, (font, small): (FontStyle, FontStyle), (mf, ms): (crate::text::FontMetrics, crate::text::FontMetrics)) {
        if ui.nav.w <= 0.0 {
            return;
        }
        let s = |v: f32| (v * self.scale).round();
        let nav = ui.nav;
        g.dl.rect(Rect::new(nav.right() - 1.0, nav.y + s(NAV_PAD), 1.0, (nav.h - 2.0 * s(NAV_PAD)).max(0.0)), t.separator);
        let icons = self.icon_font(13.0);
        let mid = |r: Rect, m: crate::text::FontMetrics| (r.y + (r.h + m.ascent - m.descent) / 2.0).round();
        let focused = ui.zone == Zone::Nav && self.active && self.nav.palette.is_none();
        let searching = !ui.query().is_empty();
        g.dl.set_clip(Some(nav));
        for (k, (&r, st)) in ui.nav_rects.iter().zip(&ui.starts).enumerate() {
            let glow = self.lit(Hit::Settings(SHit::Nav(k)));
            if st.page == ui.page && glow < 1.0 {
                g.dl.round_rect(r, t.selection, s(4.0));
            }
            if glow > 0.0 {
                g.dl.round_rect(r, crate::ui::widgets::faded(t.accent_fill, glow), s(4.0));
            }
            if focused && ui.nav_at == k {
                g.dl.outline(r, t.focus_ring, s(4.0), 1.0);
            }
            let ink = if glow >= 0.5 { crate::theme::ink_on(t.accent_fill) } else { t.text };
            let mark = if glow >= 0.5 { ink } else { t.accent };
            if ui.rail {
                centered(g, r, page_glyph(st.page), icons, mark);
                continue;
            }
            let icon = Rect::new(r.x + s(6.0), r.y, s(22.0), r.h);
            centered(g, icon, page_glyph(st.page), icons, mark);
            let tx = icon.right() + s(8.0);
            let mut right = r.right() - s(8.0);
            if searching {
                let count = st.matches.to_string();
                let cw = g.measure(&count, small);
                g.text(right - cw, mid(r, ms), &count, small, if glow >= 0.5 { ink } else { t.text_dim }, cw + 1.0);
                right -= cw + s(8.0);
            }
            g.text(tx, mid(r, mf), st.page.title(), font, ink, (right - tx).max(0.0));
        }
        g.dl.set_clip(None);
    }

    fn paint_tip<B: TextBackend>(&self, g: &mut Gfx<B>, t: &Theme, text: &str, (x, y): (f32, f32)) {
        let sc = self.scale;
        let font = self.ui_font(12.0);
        let m = g.metrics(font);
        let (pad, line_h, edge) = (self.s(8.0), (self.s(12.0) * 1.45).round(), self.s(8.0));
        let max_w = self.s(TIP_W).min(self.size.0 - 2.0 * edge - 2.0 * pad);
        let lines = wrap(g, text, font, max_w);
        let w = lines.iter().map(|l| g.measure(l, font)).fold(0.0, f32::max).ceil() + 2.0 * pad;
        let h = lines.len() as f32 * line_h + 2.0 * pad;
        let below = y + self.s(20.0);
        let ty = if below + h > self.size.1 - edge { y - self.s(6.0) - h } else { below };
        let b = Rect::new((x + self.s(12.0)).min(self.size.0 - w - edge).max(edge), ty.max(edge), w, h);
        Paint::new(g, t, sc).frame(b);
        for (n, l) in lines.iter().enumerate() {
            let base = b.y + pad + n as f32 * line_h + (line_h + m.ascent - m.descent) / 2.0;
            g.text(b.x + pad, base.round(), l, font, t.text, w - 2.0 * pad);
        }
    }
}

fn paint_group<B: TextBackend>(g: &mut Gfx<B>, t: &Theme, r: Rect, title: &str, bold: FontStyle, s: impl Fn(f32) -> f32) {
    let mb = g.metrics(bold);
    g.text(r.x + s(PAD), (r.bottom() - s(10.0) - mb.descent).round(), title, bold, t.text, r.w - 2.0 * s(PAD));
}
