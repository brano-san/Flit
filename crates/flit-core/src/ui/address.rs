use crate::i18n::S;
use super::actions::{Group, ACTIONS};
use super::menu::Cmd;
use super::ops::TIMER_SLACK;
use super::{centered, App, Effect, Hit};
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

const MAX_TYPED: usize = 64;
const MAX_SUGGESTIONS: usize = 25;
const MAX_HISTORY_MENU: usize = 16;
const LONG_PRESS: Duration = Duration::from_millis(500);
const NOT_URLS: [&str; 3] = ["file", "shell", "wsl"];
const EXPLORER: &str = "explorer.exe";
pub(super) const SEPS: [char; 2] = ['\\', '/'];

pub(super) struct Probe {
    pub loc: Location,
    pub gen: u64,
    pub slot: Arc<SnapshotSlot>,
    pub cancel: GenToken,
    pub view: Option<DirView>,
}

#[derive(Default)]
pub(super) struct Address {
    pub pending: Option<String>,
    pub press: Option<(usize, Instant)>,
    pub typed: Typed,
    watch: Option<(TabId, u64, String)>,
    targets: Vec<Location>,
    pub probe: Option<Probe>,
    missing: Vec<Location>,
    pub shift: f32,
    pub shift_for: Option<Location>,
}

pub(super) enum Listed {
    Pending,
    Missing,
    Found(Vec<(Vec<u8>, bool)>),
}

pub(super) fn is_url(text: &str) -> bool {
    let Some((scheme, rest)) = text.split_once(':') else { return false };
    scheme.len() >= 2
        && !rest.is_empty()
        && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        && !NOT_URLS.iter().any(|s| scheme.eq_ignore_ascii_case(s))
}

pub(super) fn split_command(text: &str) -> (String, String) {
    let t = text.trim();
    if let Some((program, args)) = t.strip_prefix('"').and_then(|r| r.split_once('"')) {
        return (program.to_owned(), args.trim().to_owned());
    }
    match t.split_once(char::is_whitespace) {
        Some((program, args)) => (program.to_owned(), args.trim().to_owned()),
        None => (t.to_owned(), String::new()),
    }
}

fn entries(v: &DirView) -> Vec<(Vec<u8>, bool)> {
    let l = v.listing();
    v.rows().iter().map(|&i| (l.name(i).to_vec(), l.entries[i as usize].is_dir())).collect()
}

type Rank = (u8, usize, usize, usize);

fn rank((tier, dist): (u8, usize), recency: usize, text: &str) -> Rank {
    (tier, recency, dist, text.chars().count())
}

fn lower(s: &str) -> Vec<char> {
    s.chars().flat_map(char::to_lowercase).collect()
}

pub(super) fn place_key(s: &str) -> String {
    s.trim().trim_end_matches(SEPS).to_lowercase()
}

fn allowance(q: &[char]) -> usize {
    (q.len() / 3).min(2)
}

fn edit_row(a: &[char], b: &[char]) -> Vec<usize> {
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut diag = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let up = row[j + 1];
            row[j + 1] = (diag + usize::from(ca != cb)).min(up + 1).min(row[j] + 1);
            diag = up;
        }
    }
    row
}

fn subsequence(q: &[char], c: &[char]) -> bool {
    let mut rest = c.iter();
    q.iter().all(|x| rest.any(|y| y == x))
}

fn literal(q: &[char], c: &[char]) -> Option<(u8, usize)> {
    if c.starts_with(q) {
        Some((0, 0))
    } else if q.len() > 1 && c.windows(q.len()).any(|w| w == q) {
        Some((1, 0))
    } else {
        None
    }
}

fn closeness(q: &[char], c: &[char], loose: bool) -> Option<(u8, usize)> {
    literal(q, c).or_else(|| {
        let d = edit_row(q, &c[..c.len().min(q.len() + 2)]).into_iter().min().unwrap_or(0);
        (d <= allowance(q) || loose && q.len() > 1 && subsequence(q, c)).then_some((2, d))
    })
}

fn path_closeness(query: &str, cand: &str) -> Option<(u8, usize)> {
    let (q, c) = (lower(query.trim()), lower(cand));
    if let Some(m) = literal(&q, &c) {
        return Some(m);
    }
    let is_sep = |ch: &char| SEPS.contains(ch);
    let (qs, cs): (Vec<&[char]>, Vec<&[char]>) = (q.split(is_sep).collect(), c.split(is_sep).collect());
    let (last, head) = qs.split_last()?;
    if cs.len() < head.len() {
        return None;
    }
    let mut dist = 0;
    for (a, b) in head.iter().zip(&cs) {
        let d = edit_row(a, b)[b.len()];
        if d > allowance(a) {
            return None;
        }
        dist += d;
    }
    let (_, d) = closeness(last, cs.get(head.len()).copied().unwrap_or_default(), false)?;
    Some((2, dist + d))
}

pub(super) fn label(loc: &Location) -> String {
    loc.crumbs().pop().map(|c| c.0).unwrap_or_default()
}

pub(super) fn ready(p: &mut Probe) -> bool {
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
            return crate::tr!(S::AddressCannotOpen, location = location);
        }
        tracing::info!("handing {location} to File Explorer");
        self.launch(Launch::Run { program: EXPLORER.into(), args: format!("\"{location}\""), dir: None });
        crate::tr!(S::AddressHandedToExplorer, location = location)
    }

    fn remember_typed(&mut self, text: &str) {
        let mut typed = self.addr.typed.lock().unwrap_or_else(|e| e.into_inner());
        typed.retain(|t| !t.eq_ignore_ascii_case(text));
        typed.insert(0, text.to_owned());
        typed.truncate(MAX_TYPED);
    }

    pub(super) fn typed_address(&mut self, text: &str) -> bool {
        let text = text.trim().to_owned();
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

    pub(super) fn submit_address(&mut self, text: &str, now: Instant) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        let cwd = self.current_location().to_windows().map(|p| p.as_bytes().to_vec());
        let cmd = open::Open { paths: vec![text.as_bytes().to_vec()], target: open::OpenTarget::CurrentTab, cwd, ..Default::default() };
        if self.typed_address(text) {
            return;
        }
        if Location::parse(text).as_ref() == Some(self.current_location()) {
            return self.refresh(now);
        }
        self.effects.push(Effect::Open(cmd));
        self.dirty = true;
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
        self.tabs[k].drop_trail();
        self.load_tab(k, select, Instant::now());
        if k == self.cur {
            self.after_tab_change();
        } else {
            self.strip.rev += 1;
        }
    }

    pub(super) fn probe(&mut self, loc: Location) -> Probe {
        let gen = self.gen();
        let p = Probe { loc, gen, slot: Arc::default(), cancel: GenToken::default(), view: None };
        p.cancel.set(gen);
        let req = ProviderRequest::List { tab: 0, gen, path: p.loc.path().clone(), view: self.tab().view_spec, case: self.case, out: p.slot.clone(), cancel: p.cancel.clone() };
        self.effects.push(Effect::Provider { provider: p.loc.provider(), req });
        p
    }

    pub(super) fn listing(&mut self, loc: &Location) -> Listed {
        if self.addr.missing.contains(loc) {
            return Listed::Missing;
        }
        let t = self.tab();
        if let Some(v) = t.view.as_ref().filter(|v| t.view_loc == *loc && v.snap().done) {
            return Listed::Found(entries(v));
        }
        if let Some(p) = self.addr.probe.as_ref().filter(|p| p.loc == *loc) {
            return match &p.view {
                None => Listed::Pending,
                Some(v) if v.snap().error.is_some() && v.total_rows() == 0 => {
                    self.addr.missing.push(loc.clone());
                    Listed::Missing
                }
                Some(v) => Listed::Found(entries(v)),
            };
        }
        if let Some(old) = self.addr.probe.take() {
            old.cancel.set(u64::MAX);
        }
        self.addr.probe = Some(self.probe(loc.clone()));
        Listed::Pending
    }

    pub(super) fn resolve_typed(&self, dir: &str) -> Option<Location> {
        let cwd = self.current_location().to_windows().map(|p| p.as_bytes().to_vec());
        open::resolve(dir.as_bytes(), cwd.as_deref(), &|_| None).ok()
    }

    pub(super) fn poll_address(&mut self) -> bool {
        if self.addr.probe.as_mut().is_some_and(|p| p.view.is_none() && ready(p)) {
            self.go_refresh();
            return true;
        }
        false
    }

    pub(super) fn go_target(&mut self, k: usize, now: Instant) {
        if let Some(loc) = self.addr.targets.get(k).cloned() {
            self.navigate(loc, now);
        }
    }

    pub(super) fn visited_menu(&mut self) {
        let list = self.nav.sidebar.prefs.most_visited();
        let r = self.layout.nav[3];
        let menu = self.collect(|_, m| {
            for (k, loc) in list.iter().enumerate() {
                m.go(&label(loc), k, super::G_FOLDER);
            }
        });
        self.addr.targets = list;
        if !menu.is_empty() {
            self.button_menu(Hit::Nav(3), r.x, r.bottom(), menu);
        }
    }

    pub(super) fn panel_quick_menu(&mut self) {
        let r = self.layout.more;
        let menu = self.collect(|_, m| {
            for a in ACTIONS.iter().filter(|a| a.group == Group::Quick) {
                m.action(a.id, a.title.t());
            }
        });
        self.button_menu(Hit::PanelMenu, r.x, r.bottom(), menu);
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

    fn places(&self) -> Vec<(String, bool)> {
        let mut out: Vec<(String, bool)> = self.addr.typed.lock().unwrap_or_else(|e| e.into_inner()).iter().map(|t| (t.clone(), true)).collect();
        let shown = |l: &Location| (l.display().into_owned(), false);
        let sb = &self.nav.sidebar;
        let mut recent: Vec<_> = sb.places.recent.iter().filter(|r| r.dir).collect();
        recent.sort_by_key(|r| std::cmp::Reverse(r.at));
        out.extend(self.tab().history.back_list().iter().rev().map(shown));
        out.extend(recent.into_iter().map(|r| (r.target.display().into_owned(), false)));
        out.extend(sb.state.pinned.iter().map(shown));
        out.extend(sb.places.folders.iter().map(|(_, l)| shown(l)));
        out
    }

    pub(super) fn suggest(&mut self, text: &str) -> Vec<(String, bool)> {
        if text.trim().is_empty() {
            return Vec::new();
        }
        let places = self.places();
        let keys: Vec<String> = places.iter().map(|(p, _)| place_key(p)).collect();
        let recency = |s: &str| {
            let k = place_key(s);
            keys.iter().position(|p| *p == k).unwrap_or(usize::MAX)
        };
        let mut found: Vec<(Rank, String, bool)> = Vec::new();
        let typed_key = place_key(text);
        for ((p, typed), k) in places.iter().zip(&keys) {
            if let Some(m) = path_closeness(text, p).filter(|_| *k != typed_key) {
                found.push((rank(m, recency(p), p), p.clone(), *typed));
            }
        }
        let (mut end, mut cut) = (text.len(), text.rfind(SEPS));
        while let Some(c) = cut {
            let (dir, seg, sep) = (&text[..=c], &text[c + 1..end], &text[c..=c]);
            let listed = match self.resolve_typed(dir) {
                Some(loc) => self.listing(&loc),
                None => Listed::Pending,
            };
            match listed {
                Listed::Found(names) => {
                    let q = lower(seg);
                    for (n, _) in names.iter().filter(|e| e.1) {
                        let shown = name::display(n);
                        if let Some(m) = closeness(&q, &lower(&shown), true) {
                            let s = format!("{dir}{shown}{sep}");
                            found.push((rank(m, recency(&s), &s), s, false));
                        }
                    }
                    break;
                }
                Listed::Missing => (end, cut) = (c, text[..c].rfind(SEPS)),
                Listed::Pending => break,
            }
        }
        found.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        let mut seen = Vec::new();
        found
            .into_iter()
            .filter(|f| {
                let k = place_key(&f.1);
                !seen.contains(&k) && {
                    seen.push(k);
                    true
                }
            })
            .take(MAX_SUGGESTIONS)
            .map(|f| (f.1, f.2))
            .collect()
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
        centered(g, b, S::MenuEject.t(), self.ui_font(13.0), th.text);
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
