use super::menu::Cmd;
use super::ops::TIMER_SLACK;
use super::{centered, App, Effect, Hit, G_FOLDER};
use crate::input::{Key, Mods};
use crate::listing::{DirSnapshot, FsErrorKind};
use crate::location::{Location, ProviderId};
use crate::name;
use crate::open;
use crate::path::{FsPath, PathKind};
use crate::provider::{GenToken, Launch, ProviderRequest, SnapshotSlot, TabId};
use crate::render::Rect;
use crate::text::{Gfx, TextBackend};
use crate::theme::Theme;
use crate::view::DirView;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub type Typed = Arc<Mutex<Vec<String>>>;

pub(super) const OVERFLOW: &str = "\u{2026}";
const G_HISTORY: &str = "\u{E81C}";
const MAX_TYPED: usize = 64;
const MAX_SUGGESTIONS: usize = 10;
const MAX_MENU: usize = 256;
const MAX_HISTORY_MENU: usize = 16;
const LONG_PRESS: Duration = Duration::from_millis(500);
const SUGGESTION_H: f32 = 28.0;
const NOT_URLS: [&str; 3] = ["file", "shell", "wsl"];
const EXPLORER: &str = "explorer.exe";

type Anchor = ((f32, f32), Option<Vec<u8>>);

struct Probe {
    loc: Location,
    gen: u64,
    slot: Arc<SnapshotSlot>,
    cancel: GenToken,
    view: Option<DirView>,
}

#[derive(Default)]
pub(super) struct Address {
    pub pending: Option<String>,
    pub chevrons: Vec<Rect>,
    pub overflow: Rect,
    pub press: Option<(usize, Instant)>,
    pub typed: Typed,
    watch: Option<(TabId, u64, String)>,
    targets: Vec<Location>,
    menu: Option<(Probe, Anchor)>,
    probe: Option<Probe>,
    pub seen: Option<String>,
    items: Vec<(String, bool)>,
    rects: Vec<Rect>,
    sel: Option<usize>,
}

fn is_url(text: &str) -> bool {
    let Some((scheme, rest)) = text.split_once(':') else { return false };
    scheme.len() >= 2
        && !rest.is_empty()
        && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        && !NOT_URLS.iter().any(|s| scheme.eq_ignore_ascii_case(s))
}

fn split_command(text: &str) -> (String, String) {
    let t = text.trim();
    if let Some((program, args)) = t.strip_prefix('"').and_then(|r| r.split_once('"')) {
        return (program.to_owned(), args.trim().to_owned());
    }
    match t.split_once(char::is_whitespace) {
        Some((program, args)) => (program.to_owned(), args.trim().to_owned()),
        None => (t.to_owned(), String::new()),
    }
}

fn folders(v: &DirView) -> Vec<Vec<u8>> {
    let l = v.listing();
    v.rows().iter().filter(|&&i| l.entries[i as usize].is_dir()).map(|&i| l.name(i).to_vec()).collect()
}

fn label(loc: &Location) -> String {
    loc.crumbs().pop().map(|c| c.0).unwrap_or_default()
}

fn ready(p: &mut Probe) -> bool {
    if let Some(s) = p.slot.take().filter(|s| s.gen == p.gen && (s.done || s.error.is_some())) {
        p.view = Some(DirView::new(s));
    }
    p.view.is_some()
}

impl App {
    pub fn share_typed(&mut self, typed: Typed) {
        self.addr.typed = typed;
    }

    pub(super) fn launch(&mut self, launch: Launch) {
        let tab = self.tab().id;
        self.effects.push(Effect::Provider { provider: ProviderId::Local, req: ProviderRequest::Launch { tab, launch } });
    }

    pub(super) fn hand_to_explorer(&mut self, location: &str) -> String {
        if location.contains('"') {
            return format!("Cannot open {location} in Flit");
        }
        tracing::info!("handing {location} to File Explorer");
        self.launch(Launch::Run { program: EXPLORER.into(), args: format!("\"{location}\""), dir: None });
        format!("Flit cannot show {location}, so it opens in File Explorer")
    }

    fn remember_typed(&mut self, text: &str) {
        let mut typed = self.addr.typed.lock().unwrap_or_else(|e| e.into_inner());
        typed.retain(|t| !t.eq_ignore_ascii_case(text));
        typed.insert(0, text.to_owned());
        typed.truncate(MAX_TYPED);
    }

    pub(super) fn typed_address(&mut self) -> bool {
        let text = self.edit.text.trim().to_owned();
        if text.is_empty() {
            return false;
        }
        self.remember_typed(&text);
        if is_url(&text) {
            self.launch(Launch::Run { program: text, args: String::new(), dir: None });
            return true;
        }
        if Location::parse(&text).as_ref() != Some(self.current_location()) {
            self.addr.pending = Some(text);
        }
        false
    }

    pub(super) fn watch_typed(&mut self, text: String) {
        let t = self.tab();
        self.addr.watch = Some((t.id, t.gen, text));
    }

    pub(super) fn run_typed(&mut self, text: &str) {
        let (program, args) = split_command(text);
        let dir = self.current_location().to_windows().filter(|p| self.caps.is_native(p));
        self.launch(Launch::Run { program, args, dir });
    }

    pub(super) fn redirect(&mut self, k: usize, snap: &DirSnapshot) -> bool {
        let t = &self.tabs[k];
        if snap.gen != t.gen {
            return false;
        }
        let typed = self.addr.watch.take_if(|w| w.0 == t.id && w.1 == snap.gen).map(|w| w.2);
        let Some(kind) = snap.error.as_ref().map(|e| e.kind) else { return false };
        let here = t.location().clone();
        match (kind, typed) {
            (FsErrorKind::NotADirectory, Some(_)) => {
                let tab = t.id;
                self.effects.push(Effect::Provider { provider: here.provider(), req: ProviderRequest::Open { tab, path: here.path().clone() } });
                self.step_back(k);
            }
            (FsErrorKind::NotFound, Some(text)) => {
                self.step_back(k);
                self.run_typed(&text);
            }
            (FsErrorKind::NotADirectory, None) => {
                let Some(parent) = here.parent() else { return false };
                self.tabs[k].history.replace(parent);
                self.reload_moved(k, Some(here.file_name().to_vec()));
            }
            _ => return false,
        }
        true
    }

    fn step_back(&mut self, k: usize) {
        let select = self.tabs[k].focused_name();
        if self.tabs[k].history.retract() {
            self.reload_moved(k, select);
        }
    }

    fn reload_moved(&mut self, k: usize, select: Option<Vec<u8>>) {
        self.load_tab(k, select, Instant::now());
        if k == self.cur {
            self.after_tab_change();
        } else {
            self.strip.rev += 1;
        }
    }

    fn probe(&mut self, loc: Location) -> Probe {
        let gen = self.gen();
        let p = Probe { loc, gen, slot: Arc::default(), cancel: GenToken::default(), view: None };
        p.cancel.set(gen);
        let req = ProviderRequest::List { tab: 0, gen, path: p.loc.path().clone(), view: self.tab().view_spec, case: self.case, out: p.slot.clone(), cancel: p.cancel.clone() };
        self.effects.push(Effect::Provider { provider: p.loc.provider(), req });
        p
    }

    fn shown_folders(&self, loc: &Location) -> Option<Vec<Vec<u8>>> {
        let t = self.tab();
        t.view.as_ref().filter(|v| t.view_loc == *loc && v.snap().done).map(folders)
    }

    pub(super) fn poll_address(&mut self) -> bool {
        let mut shown = false;
        if self.addr.probe.as_mut().is_some_and(|p| p.view.is_none() && ready(p)) {
            if self.editing {
                self.suggest();
            }
            shown = true;
        }
        if self.addr.menu.as_mut().is_some_and(|m| ready(&mut m.0)) {
            let Some((p, (at, mark))) = self.addr.menu.take() else { return shown };
            let names = p.view.as_ref().map(folders).unwrap_or_default();
            self.subfolder_menu(p.loc, names, mark, at);
        }
        shown
    }

    pub(super) fn crumb_menu(&mut self, i: usize) {
        let crumbs = &self.tab().crumbs;
        let Some(loc) = crumbs.get(i).map(|c| c.1.clone()) else { return };
        let mark = crumbs.get(i + 1).map(|c| c.1.file_name().to_vec());
        let at = (self.addr.chevrons.get(i).map_or(self.layout.address.x, |r| r.x), self.layout.address.bottom());
        if let Some(names) = self.shown_folders(&loc) {
            return self.subfolder_menu(loc, names, mark, at);
        }
        if let Some((old, ..)) = self.addr.menu.take() {
            old.cancel.set(u64::MAX);
        }
        let p = self.probe(loc);
        self.addr.menu = Some((p, (at, mark)));
    }

    fn subfolder_menu(&mut self, loc: Location, mut names: Vec<Vec<u8>>, mark: Option<Vec<u8>>, (x, y): (f32, f32)) {
        if names.is_empty() {
            return self.status(format!("{} has no folders", label(&loc)));
        }
        names.truncate(MAX_MENU);
        let entries = names.iter().enumerate().map(|(k, n)| (name::display(n).into_owned(), Cmd::Go(k), mark.as_deref() == Some(n.as_slice()))).collect();
        self.addr.targets = names.iter().map(|n| loc.join(n)).collect();
        let menu = self.list_menu(entries);
        self.flit_menu(x, y, menu);
    }

    pub(super) fn overflow_menu(&mut self) {
        let hidden: Vec<(String, Location)> = self.tab().crumbs.iter().zip(&self.layout.crumbs).filter(|(_, r)| r.w == 0.0).map(|(c, _)| c.clone()).collect();
        let entries = hidden.iter().enumerate().map(|(k, c)| (c.0.clone(), Cmd::Go(k), false)).collect();
        self.addr.targets = hidden.into_iter().map(|c| c.1).collect();
        let menu = self.list_menu(entries);
        self.flit_menu(self.addr.overflow.x, self.layout.address.bottom(), menu);
    }

    pub(super) fn go_target(&mut self, k: usize, now: Instant) {
        if let Some(loc) = self.addr.targets.get(k).cloned() {
            self.navigate(loc, now);
        }
    }

    pub(super) fn history_menu(&mut self, i: usize) {
        let h = &self.tab().history;
        let (list, sign) = if i == 1 { (h.forward_list(), 1) } else { (h.back_list(), -1) };
        let entries: Vec<_> = list.iter().rev().take(MAX_HISTORY_MENU).enumerate().map(|(k, loc)| (label(loc), Cmd::Jump(sign * (k as isize + 1)), false)).collect();
        if entries.is_empty() {
            return;
        }
        let r = self.layout.nav[i];
        let menu = self.list_menu(entries);
        self.flit_menu(r.x, r.bottom(), menu);
    }

    pub(super) fn arm_history_press(&mut self, i: usize, now: Instant) {
        self.addr.press = Some((i, now + LONG_PRESS));
        self.arm_timer(now, now + LONG_PRESS);
    }

    pub(super) fn history_timer(&mut self, now: Instant) {
        let Some((i, at)) = self.addr.press else { return };
        if now + TIMER_SLACK < at {
            return self.arm_timer(now, at);
        }
        self.addr.press = None;
        if self.pressed == Hit::Nav(i) {
            self.pressed = Hit::None;
            self.history_menu(i);
        }
    }

    pub(super) fn layout_address(&mut self) {
        if !self.editing {
            if self.addr.seen.take().is_some() {
                self.addr.items.clear();
                self.addr.rects.clear();
                self.addr.sel = None;
            }
            return;
        }
        match self.addr.seen.replace(self.edit.text.clone()) {
            Some(was) if was != self.edit.text => self.suggest(),
            _ => {}
        }
        let (a, h, gap) = (self.layout.address, self.s(SUGGESTION_H), self.s(4.0));
        self.addr.rects = (0..self.addr.items.len()).map(|k| Rect::new(a.x, a.bottom() + gap + k as f32 * h, a.w, h)).collect();
    }

    fn suggest(&mut self) {
        let text = self.edit.text.clone();
        let mut items: Vec<(String, bool)> = Vec::new();
        if !text.trim().is_empty() {
            let typed = self.addr.typed.lock().unwrap_or_else(|e| e.into_inner());
            items.extend(typed.iter().filter(|t| t.len() > text.len() && name::starts_with_ci(t.as_bytes(), &text)).take(MAX_SUGGESTIONS / 2).map(|t| (t.clone(), true)));
        }
        if let Some(cut) = text.rfind(['\\', '/']) {
            let (dir, prefix, sep) = (&text[..=cut], &text[cut + 1..], &text[cut..=cut]);
            for n in self.subfolders(dir).unwrap_or_default().iter().filter(|n| name::starts_with_ci(n, prefix)) {
                let s = format!("{dir}{}{sep}", name::display(n));
                if items.len() == MAX_SUGGESTIONS {
                    break;
                }
                if !items.iter().any(|(t, _)| *t == s) {
                    items.push((s, false));
                }
            }
        }
        self.addr.items = items;
        self.addr.sel = None;
    }

    fn subfolders(&mut self, dir: &str) -> Option<Vec<Vec<u8>>> {
        let cwd = self.current_location().to_windows().map(|p| p.as_bytes().to_vec());
        let loc = open::resolve(dir.as_bytes(), cwd.as_deref(), &|_| None).ok()?;
        if let Some(names) = self.shown_folders(&loc) {
            return Some(names);
        }
        if let Some(p) = self.addr.probe.as_ref().filter(|p| p.loc == loc) {
            return p.view.as_ref().map(folders);
        }
        if let Some(old) = self.addr.probe.take() {
            old.cancel.set(u64::MAX);
        }
        self.addr.probe = Some(self.probe(loc));
        None
    }

    pub(super) fn suggesting(&self) -> bool {
        self.editing && !self.addr.rects.is_empty()
    }

    pub(super) fn pick_suggestion(&mut self, k: usize) {
        if let Some((text, _)) = self.addr.items.get(k).cloned() {
            self.edit.set(&text);
            self.dirty = true;
        }
    }

    pub(super) fn address_hit(&self, x: f32, y: f32) -> Option<Hit> {
        if self.editing {
            return self.addr.rects.iter().position(|r| r.contains(x, y)).map(Hit::Suggestion);
        }
        if !self.layout.address.contains(x, y) {
            return None;
        }
        if self.addr.overflow.contains(x, y) {
            return Some(Hit::CrumbOverflow);
        }
        self.addr.chevrons.iter().position(|r| r.contains(x, y)).map(Hit::CrumbMenu)
    }

    pub(super) fn address_key(&mut self, key: Key, m: Mods) -> bool {
        let n = self.addr.items.len();
        if n == 0 || m.ctrl || m.alt {
            return false;
        }
        let sel = self.addr.sel;
        match key {
            Key::Down => self.addr.sel = Some(sel.map_or(0, |k| (k + 1) % n)),
            Key::Up => self.addr.sel = Some(sel.map_or(n - 1, |k| (k + n - 1) % n)),
            Key::Tab => self.pick_suggestion(sel.unwrap_or(0)),
            Key::Escape => {
                self.addr.items.clear();
                self.addr.rects.clear();
                self.addr.sel = None;
            }
            Key::Enter => {
                if let Some(k) = sel {
                    self.pick_suggestion(k);
                }
                return false;
            }
            _ => return false,
        }
        self.dirty = true;
        true
    }

    pub(super) fn draw_suggestions<B: TextBackend>(&self, g: &mut Gfx<B>, th: &Theme) {
        let (Some(first), Some(last)) = (self.addr.rects.first().copied(), self.addr.rects.last().copied()) else { return };
        if !self.editing {
            return;
        }
        let (s4, s6) = (self.s(4.0), self.s(6.0));
        let b = Rect::new(first.x, first.y - s4, first.w, last.bottom() - first.y + 2.0 * s4);
        g.dl.round_rect(b, th.bg, s6);
        g.dl.outline(b, th.input_border, s6, 1.0);
        let (font, icons) = (self.ui_font(13.0), self.icon_font(12.0));
        let m = g.metrics(font);
        for (k, (r, (text, typed))) in self.addr.rects.iter().zip(&self.addr.items).enumerate() {
            if self.addr.sel == Some(k) {
                g.dl.round_rect(r.inset(s4, 1.0), th.selection, s4);
            } else if self.hover == Hit::Suggestion(k) {
                g.dl.round_rect(r.inset(s4, 1.0), th.hover, s4);
            }
            let icon = Rect::new(r.x + self.s(10.0), r.y, self.s(16.0), r.h);
            centered(g, icon, if *typed { G_HISTORY } else { G_FOLDER }, icons, th.text_dim);
            let x = icon.right() + self.s(8.0);
            g.text(x, (r.y + (r.h + m.ascent - m.descent) / 2.0).round(), text, font, th.text, r.right() - x - s6);
        }
    }

    pub(super) fn eject_rect(&self) -> Option<Rect> {
        let t = self.tab();
        let v = t.view.as_ref()?;
        let no_media = v.total_rows() == 0 && v.snap().error.as_ref().is_some_and(|e| e.kind == FsErrorKind::NoMedia);
        if !no_media || t.view_loc.path().kind() != PathKind::Drive {
            return None;
        }
        let (l, w) = (self.layout.list, self.s(120.0));
        Some(Rect::new((l.x + (l.w - w) / 2.0).round(), l.y + self.s(152.0), w, self.s(30.0)))
    }

    pub(super) fn draw_eject<B: TextBackend>(&self, g: &mut Gfx<B>, th: &Theme) {
        let Some(b) = self.eject_rect() else { return };
        let s4 = self.s(4.0);
        g.dl.round_rect(b, if self.hover == Hit::Eject { th.button_pressed } else { th.button_hover }, s4);
        g.dl.outline(b, th.input_border, s4, 1.0);
        centered(g, b, "Eject", self.ui_font(13.0), th.text);
    }

    pub(super) fn eject(&mut self) {
        if let Some(root) = self.eject_rect().and_then(|_| FsPath::parse_bytes(self.tab().view_loc.path().root_bytes())) {
            self.launch(Launch::Eject(root));
        }
    }
}

#[cfg(test)]
#[path = "../../test/ui/address.rs"]
mod tests;
