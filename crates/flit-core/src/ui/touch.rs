use super::layouts::LayoutMode;
use super::ops::TIMER_SLACK;
use super::preview_pane::{PaneHit, Pick};
use super::{App, Drag, Hit};
use crate::input::{Event, MouseButton};
use crate::provider::TabId;
use std::time::{Duration, Instant};

const PAN_START: f32 = 8.0;
const FLING_DECAY: f32 = 3.0;
const FLING_MIN: f32 = 30.0;
const FLING_STALE: Duration = Duration::from_millis(120);
pub(super) const HOLD: Duration = Duration::from_millis(500);

#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum Target {
    List(TabId),
    Pane(TabId, usize),
    Sidebar,
    Preview,
    Settings,
    Menu,
    Search,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Phase {
    Pending,
    Panning,
    Done,
}

#[derive(Default)]
enum Before {
    #[default]
    Nothing,
    List(Vec<u32>, Option<u32>, Option<u32>),
    Pick(Pick),
}

pub(super) struct Snapshot {
    tab: TabId,
    list: (Vec<u32>, Option<u32>, Option<u32>),
    pick: Pick,
}

struct Press {
    target: Target,
    start: (f32, f32),
    last: (f32, f32),
    at: Instant,
    since: Instant,
    velocity: (f32, f32),
    phase: Phase,
    before: Before,
}

#[derive(Default)]
pub(super) struct Touch {
    press: Option<Press>,
    fling: Option<(Target, (f32, f32))>,
}

impl App {
    pub(super) fn touch_snapshot(&self) -> Snapshot {
        let list = self.tab().view.as_ref().map_or_else(Default::default, |v| (v.selected().collect(), v.focus, v.anchor));
        Snapshot { tab: self.tab().id, list, pick: self.preview_pick() }
    }

    pub(super) fn touch_pressed(&mut self, x: f32, y: f32, snap: Snapshot, now: Instant) {
        let Some(target) = self.touch_target(x, y) else { return };
        let before = match target {
            Target::List(tab) if tab == snap.tab => Before::List(snap.list.0, snap.list.1, snap.list.2),
            Target::Preview => Before::Pick(snap.pick),
            _ => Before::Nothing,
        };
        self.touch.press = Some(Press { target, start: (x, y), last: (x, y), at: now, since: now, velocity: (0.0, 0.0), phase: Phase::Pending, before });
        if self.caps.unix_paths {
            self.arm_timer(now, now + HOLD);
        }
    }

    pub(super) fn touch_event(&mut self, ev: &Event, now: Instant) -> bool {
        match *ev {
            Event::MouseDown { .. } | Event::Focus(false) => {
                self.touch.press = None;
                self.touch.fling = None;
                false
            }
            Event::Wheel { .. } | Event::Pinch(_) => {
                self.touch.fling = None;
                false
            }
            Event::MouseMove { x, y } => self.touch_move(x, y, now),
            Event::MouseUp { button: MouseButton::Left, .. } => self.touch_up(now),
            Event::TouchCancel => {
                self.touch_cancel();
                true
            }
            _ => false,
        }
    }

    fn touch_target(&self, x: f32, y: f32) -> Option<Target> {
        if self.confirm.is_some() {
            return None;
        }
        if self.own_menu_open() {
            return self.context_menu_open().is_some_and(|c| c.pans_at(x, y)).then_some(Target::Menu);
        }
        if self.search.active {
            return (!self.jobs.modal() && self.search_pans_at(x, y)).then_some(Target::Search);
        }
        match self.hit(x, y) {
            Hit::Settings(_) => self.settings_pans_at(x, y).then_some(Target::Settings),
            Hit::Row(_) | Hit::ListEmpty | Hit::Group(_) => Some(Target::List(self.tab().id)),
            Hit::Pane(i, _) => Some(Target::Pane(self.tab().id, i)),
            Hit::Side(_) | Hit::SidePin(_) | Hit::SideEmpty | Hit::SideHeader(_) => Some(Target::Sidebar),
            Hit::Preview(PaneHit::Body) => Some(Target::Preview),
            _ => None,
        }
    }

    fn touch_axes(&self, target: Target) -> (bool, bool) {
        match target {
            Target::List(_) => (self.layout.flow.across, !self.layout.flow.across),
            Target::Pane(..) | Target::Preview | Target::Search => (true, true),
            Target::Sidebar | Target::Settings | Target::Menu => (false, true),
        }
    }

    fn touch_valid(&self, target: Target) -> bool {
        match target {
            Target::List(tab) => self.tab().id == tab,
            Target::Pane(tab, i) => self.tab().id == tab && self.layout.mode == LayoutMode::Columns && i < self.layout.panes.len(),
            Target::Sidebar | Target::Preview => true,
            Target::Settings => self.settings_open(),
            Target::Menu => self.context_menu_open().is_some(),
            Target::Search => self.search.active,
        }
    }

    fn touch_scroll(&mut self, target: Target, dx: f32, dy: f32) -> (bool, bool) {
        match target {
            Target::List(_) => self.touch_pan_list(dx, dy),
            Target::Pane(_, i) => self.touch_pan_pane(i, dx, dy),
            Target::Sidebar => (false, self.touch_pan_sidebar(dy)),
            Target::Preview => self.touch_pan_preview(dx, dy),
            Target::Settings => (false, self.touch_pan_settings(dy)),
            Target::Menu => (false, self.context_menu_mut().is_some_and(|c| c.pan(dy))),
            Target::Search => self.touch_pan_search(dx, dy),
        }
    }

    fn touch_move(&mut self, x: f32, y: f32, now: Instant) -> bool {
        let threshold = self.s(PAN_START);
        let Some(p) = self.touch.press.as_ref() else { return false };
        let target = p.target;
        if p.phase == Phase::Done {
            return true;
        }
        if !self.touch_valid(target) {
            self.touch.press = None;
            return false;
        }
        self.strip_pointer(x, y);
        let (ax, ay) = self.touch_axes(target);
        let along = |dx: f32, dy: f32| (if ax { dx } else { 0.0 }, if ay { dy } else { 0.0 });
        let Some(p) = self.touch.press.as_mut() else { return false };
        if p.phase == Phase::Pending {
            let (dx, dy) = along(x - p.start.0, y - p.start.1);
            if dx.hypot(dy) < threshold {
                return true;
            }
            (p.phase, p.last, p.at) = (Phase::Panning, p.start, now);
            let before = std::mem::take(&mut p.before);
            self.touch_release(before);
        }
        let Some(p) = self.touch.press.as_mut() else { return true };
        let (dx, dy) = along(x - p.last.0, y - p.last.1);
        let dt = now.saturating_duration_since(p.at).as_secs_f32();
        if dt > 0.0 {
            p.velocity = (0.5 * p.velocity.0 + 0.5 * dx / dt, 0.5 * p.velocity.1 + 0.5 * dy / dt);
        }
        (p.last, p.at) = ((x, y), now);
        self.touch_scroll(target, dx, dy);
        self.dirty = true;
        true
    }

    fn touch_up(&mut self, now: Instant) -> bool {
        let Some(p) = self.touch.press.take() else { return false };
        match p.phase {
            Phase::Pending => false,
            Phase::Done => true,
            Phase::Panning => {
                let fresh = now.saturating_duration_since(p.at) < FLING_STALE;
                self.touch.fling = (fresh && p.velocity.0.hypot(p.velocity.1) >= FLING_MIN).then_some((p.target, p.velocity));
                self.forget_press();
                self.end_marquee();
                self.pressed = Hit::None;
                self.release_header();
                self.dirty = true;
                true
            }
        }
    }

    fn touch_release(&mut self, before: Before) {
        match before {
            Before::List(picked, focus, anchor) => {
                if let Some(v) = self.tab_mut().view.as_mut() {
                    v.clear_selection();
                    for i in picked {
                        v.set_selected(i, true);
                    }
                    (v.focus, v.anchor) = (focus, anchor);
                }
            }
            Before::Pick(pick) => self.restore_preview_pick(pick),
            Before::Nothing => {}
        }
        self.forget_press();
        self.end_marquee();
        if matches!(self.drag, Drag::Tab { .. }) {
            self.cancel_tab_drag();
        }
        self.drag = Drag::None;
        self.pressed = Hit::None;
        self.group_press = None;
        self.search_let_go();
        if let Some(c) = self.context_menu_mut() {
            c.let_go();
        }
    }

    fn touch_cancel(&mut self) {
        self.touch.fling = None;
        let before = self.touch.press.take().map_or(Before::Nothing, |mut p| std::mem::take(&mut p.before));
        self.touch_release(before);
        self.dirty = true;
    }

    pub(super) fn touch_timer(&mut self, now: Instant) {
        let Some(due) = self.touch.press.as_ref().filter(|p| p.phase == Phase::Pending && self.caps.unix_paths).map(|p| p.since + HOLD) else { return };
        if now + TIMER_SLACK < due {
            return self.arm_timer(now, due);
        }
        let Some(mut p) = self.touch.press.take() else { return };
        if !self.touch_valid(p.target) {
            return;
        }
        let before = std::mem::take(&mut p.before);
        self.touch_release(before);
        let (x, y) = p.last;
        self.handle_event(&Event::MouseDown { x, y, button: MouseButton::Right, clicks: 1 }, now);
        self.handle_event(&Event::MouseUp { x, y, button: MouseButton::Right }, now);
        p.phase = Phase::Done;
        self.touch.press = Some(p);
        self.dirty = true;
    }

    pub(super) fn touch_fling(&mut self, dt: f32) -> bool {
        let Some((target, v)) = self.touch.fling else { return false };
        if !self.touch_valid(target) {
            self.touch.fling = None;
            return false;
        }
        let (mx, my) = self.touch_scroll(target, v.0 * dt, v.1 * dt);
        let decay = (-FLING_DECAY * dt).exp();
        let keep = |moved: bool, v: f32| if moved || dt <= 0.0 { v * decay } else { 0.0 };
        let v = (keep(mx, v.0), keep(my, v.1));
        self.touch.fling = (v.0.hypot(v.1) >= FLING_MIN).then_some((target, v));
        self.dirty = true;
        self.touch.fling.is_some()
    }
}

#[cfg(test)]
impl App {
    pub(super) fn touch_swipe(&mut self, from: (f32, f32), to: (f32, f32), at: Instant) -> Instant {
        self.on_event(&Event::Pointer { touch: true }, at);
        self.on_event(&Event::MouseDown { x: from.0, y: from.1, button: MouseButton::Left, clicks: 1 }, at);
        let mut now = at;
        for k in 1..=10u32 {
            now = at + Duration::from_millis(10 * u64::from(k));
            let f = k as f32 / 10.0;
            self.on_event(&Event::MouseMove { x: from.0 + (to.0 - from.0) * f, y: from.1 + (to.1 - from.1) * f }, now);
        }
        self.on_event(&Event::MouseUp { x: to.0, y: to.1, button: MouseButton::Left }, now);
        now
    }

    pub(super) fn touch_tap(&mut self, (x, y): (f32, f32), at: Instant) {
        self.on_event(&Event::Pointer { touch: true }, at);
        self.on_event(&Event::MouseDown { x, y, button: MouseButton::Left, clicks: 1 }, at);
        self.on_event(&Event::MouseUp { x, y, button: MouseButton::Left }, at + Duration::from_millis(60));
    }

    pub(super) fn touch_flick(&mut self, end: Instant, read: impl Fn(&App) -> f32) {
        let mut g = crate::text::Gfx::new(crate::text::tests::Mono, 1024);
        let after = read(self);
        self.frame(&mut g, end + Duration::from_millis(16));
        self.frame(&mut g, end + Duration::from_millis(100));
        let flung = read(self);
        assert!(flung > after + 10.0, "a quick flick glides on: {after} -> {flung}");
        for k in 0..400 {
            self.frame(&mut g, end + Duration::from_millis(116 + 16 * k));
        }
        assert!(self.touch.fling.is_none(), "the glide stops");
        let rest = read(self);
        self.frame(&mut g, end + Duration::from_secs(9));
        assert_eq!(read(self), rest, "and stays at rest");
    }
}

#[cfg(test)]
#[path = "../../test/ui/touch.rs"]
mod tests;
