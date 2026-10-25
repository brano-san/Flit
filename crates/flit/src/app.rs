use crate::metrics::{Metrics, Step};
use crate::places::PlacesWorker;
use crate::search_client::SearchClient;
use crate::state::{self, Saved};
use crate::tags::{self, TagWorker};
use crate::thumbs::ThumbCache;
use crossbeam_channel::{unbounded, Receiver, Sender};
use flit_core::input::Event;
use flit_core::listing::DirSnapshot;
use flit_core::open::{self, Open, OpenTarget, Route};
use flit_core::sort::{self, ViewSpec};
use flit_core::tags::TagRequest;
use flit_core::text::Gfx;
use flit_core::ui::{App, DragEvent, Effect, IconRequest, IconResult, Persisted, WindowCmd};
use flit_core::{Location, ProviderEvent, ProviderId, ProviderRequest, TabId};
use flit_platform::*;
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const ATLAS: u32 = 2048;
const OPS_GRACE: Duration = Duration::from_secs(2);
const FS_THREADS: usize = 4;
const SHELL_THREADS: usize = 2;
const THUMB_THREADS: usize = 3;
const ICON_MARGIN: u32 = 32;
const TAB_STRIDE: TabId = 1 << 32;

mod agents {
    include!(concat!(env!("OUT_DIR"), "/wsl_agent.rs"));
}

type Ws<P> = <P as Platform>::WindowSystem;
type Win<P> = <Ws<P> as WindowSystem>::Window;
type Device<P> = <<P as Platform>::Renderer as Renderer>::Device;

#[derive(Default)]
struct Visible {
    gen: AtomicU64,
    first: AtomicU32,
    last: AtomicU32,
}

impl Visible {
    fn wants(&self, req: &IconRequest) -> bool {
        req.gen == 0
            || req.gen == self.gen.load(Ordering::Relaxed)
                && req.row + ICON_MARGIN >= self.first.load(Ordering::Relaxed)
                && req.row <= self.last.load(Ordering::Relaxed) + ICON_MARGIN
    }
}

enum ShellJob {
    Icon { window: WindowId, req: IconRequest, visible: Arc<Visible> },
    TypeName(Vec<u8>),
}

struct ThumbJob {
    window: WindowId,
    req: IconRequest,
    visible: Arc<Visible>,
}

type Cache = Option<Arc<Mutex<ThumbCache>>>;

fn locked(c: &Mutex<ThumbCache>) -> std::sync::MutexGuard<'_, ThumbCache> {
    c.lock().unwrap_or_else(|e| e.into_inner())
}

fn thumbnail<S: Shell>(shell: &mut S, cache: Option<&Mutex<ThumbCache>>, req: &IconRequest) -> IconResult {
    if let Some(b) = cache.and_then(|c| locked(c).get(req.key)) {
        return IconResult::Ready(b);
    }
    match shell.thumbnail(req) {
        Some(b) => {
            if let Some(c) = cache {
                locked(c).put(req.key, &b);
            }
            IconResult::Ready(b)
        }
        None => IconResult::Failed,
    }
}

struct SortJob {
    window: WindowId,
    tab: TabId,
    gen: u64,
    snap: Arc<DirSnapshot>,
    view: ViewSpec,
}

enum Done {
    Icon { window: WindowId, key: u64, result: IconResult },
    Thumb { window: WindowId, key: u64, result: IconResult },
    TypeName(Vec<u8>, Option<String>),
    Sorted { window: WindowId, tab: TabId, gen: u64, view: ViewSpec, base_len: usize, order: Vec<u32>, ms: f64 },
}

fn spawn(name: &str, f: impl FnOnce() + Send + 'static) -> Result<(), PlatformError> {
    std::thread::Builder::new().name(name.into()).spawn(f).map(drop).map_err(|e| PlatformError::new(-1, format!("cannot start {name}: {e}")))
}

type Workers = (Sender<ShellJob>, Sender<ThumbJob>, Sender<SortJob>, Receiver<Done>);

fn start_workers<P: Platform>(wake: Wake, cache: Cache) -> Result<Workers, PlatformError> {
    let (done_tx, done) = unbounded();
    let (thumb_tx, thumb_rx) = unbounded::<ThumbJob>();
    for k in 0..THUMB_THREADS {
        let (rx, tx, wake, cache) = (thumb_rx.clone(), done_tx.clone(), wake.clone(), cache.clone());
        spawn(&format!("flit-thumb-{k}"), move || {
            let Ok(mut shell) = P::Shell::new() else { return };
            for ThumbJob { window, req, visible } in rx {
                let result = if visible.wants(&req) { thumbnail(&mut shell, cache.as_deref(), &req) } else { IconResult::Skipped };
                if tx.send(Done::Thumb { window, key: req.key, result }).is_err() {
                    return;
                }
                wake();
            }
        })?;
    }
    let (shell_tx, shell_rx) = unbounded::<ShellJob>();
    for k in 0..SHELL_THREADS {
        let (rx, tx, wake) = (shell_rx.clone(), done_tx.clone(), wake.clone());
        spawn(&format!("flit-shell-{k}"), move || {
            let Ok(mut shell) = P::Shell::new() else { return };
            for job in rx {
                let done = match job {
                    ShellJob::Icon { window, req, visible } => {
                        let result = if !visible.wants(&req) {
                            IconResult::Skipped
                        } else {
                            shell.icon(&req).map_or(IconResult::Failed, IconResult::Ready)
                        };
                        Done::Icon { window, key: req.key, result }
                    }
                    ShellJob::TypeName(ext) => {
                        let name = shell.type_name(&ext);
                        Done::TypeName(ext, name)
                    }
                };
                if tx.send(done).is_err() {
                    return;
                }
                wake();
            }
        })?;
    }
    let (sort_tx, sort_rx) = unbounded::<SortJob>();
    spawn("flit-sort", move || {
        for job in sort_rx {
            let t = Instant::now();
            let mut order = Vec::new();
            sort::sort(&job.snap.listing, &job.snap.keys, job.view, &mut order);
            let ms = t.elapsed().as_secs_f64() * 1e3;
            let done = Done::Sorted { window: job.window, tab: job.tab, gen: job.gen, view: job.view, base_len: job.snap.listing.len(), order, ms };
            if done_tx.send(done).is_err() {
                return;
            }
            wake();
        }
    })?;
    Ok((shell_tx, thumb_tx, sort_tx, done))
}

struct WinState<P: Platform> {
    app: App,
    gfx: Gfx<P::Text>,
    renderer: Option<P::Renderer>,
    software: bool,
    device_gen: u64,
    visible: Arc<Visible>,
}

struct Handler<P: Platform> {
    windows: HashMap<WindowId, WinState<P>>,
    pending: VecDeque<App>,
    next_base: TabId,
    active: Option<WindowId>,
    local: Arc<LocalProvider>,
    wsl: HashMap<String, WslProvider>,
    distros: Option<Vec<String>>,
    events_tx: Sender<ProviderEvent>,
    events: Receiver<ProviderEvent>,
    opens: Receiver<Open>,
    sink: OpenSink,
    queued_opens: Vec<Open>,
    shell: Sender<ShellJob>,
    thumbs: Sender<ThumbJob>,
    thumb_cache: Cache,
    sorter: Sender<SortJob>,
    done: Receiver<Done>,
    saved: Saved,
    metrics: Option<Metrics>,
    gpu: Receiver<(bool, Result<Device<P>, PlatformError>)>,
    hw: Option<Device<P>>,
    sw: Option<Device<P>>,
    gpu_pending: usize,
    injection: Option<(Instant, Event)>,
    wake: Wake,
    device_gen: u64,
    search: SearchClient,
    places: PlacesWorker,
    tags: TagWorker,
}

impl<P: Platform> Handler<P> {
    fn poll_gpu(&mut self, block: bool) {
        let mut wait = block && self.hw.is_none() && self.sw.is_none();
        while self.gpu_pending > 0 {
            let msg = if wait { self.gpu.recv().ok() } else { self.gpu.try_recv().ok() };
            let Some((software, result)) = msg else { break };
            self.gpu_pending -= 1;
            match result {
                Ok(d) if software => self.sw = Some(d),
                Ok(d) => self.hw = Some(d),
                Err(e) => tracing::warn!("graphics device (software: {software}): {e}"),
            }
            wait = wait && self.hw.is_none() && self.sw.is_none();
        }
    }

    fn new_app(&mut self, routes: Vec<Route>, persisted: Persisted) -> App {
        let base = self.next_base;
        self.next_base += TAB_STRIDE;
        let mut app = App::new(persisted, routes, base, P::dark_mode(), 1.0, (1100, 720), Instant::now());
        app.utc_offset_min = P::utc_offset_minutes();
        app.double_click = P::double_click_time();
        if base == 0 && P::elevated() {
            app.status("Flit runs as administrator, so Windows blocks files dragged in from other programs");
        }
        app.set_nav_state(self.saved.nav.clone());
        app.set_places(self.places.current());
        app.set_tags(self.tags.current());
        let fx: Vec<Effect> = app.drain_effects().collect();
        for e in fx {
            if let Effect::Provider { provider, req } = e {
                self.submit(provider, req);
            }
        }
        app
    }

    fn submit(&mut self, provider: ProviderId, req: ProviderRequest) {
        let ProviderId::Wsl(distro) = provider else { return self.local.submit(req) };
        let known = self.distros.get_or_insert_with(P::wsl_distros);
        let distro = known.iter().find(|d| d.eq_ignore_ascii_case(&distro)).cloned().unwrap_or(distro);
        if !self.wsl.contains_key(&distro) {
            let launcher = P::wsl_launcher(&distro, &agents::AGENTS);
            match WslProvider::new(distro.clone(), launcher, self.local.clone(), self.events_tx.clone(), self.wake.clone(), Timing::default()) {
                Ok(p) => {
                    self.wsl.insert(distro.clone(), p);
                }
                Err(e) => tracing::warn!("{e}"),
            }
        }
        match self.wsl.get(&distro) {
            Some(p) => p.submit(req),
            None => match via_unc(&distro, req) {
                Some(req) => self.local.submit(req),
                None => tracing::warn!("path is not reachable through \\\\wsl.localhost\\{distro}"),
            },
        }
    }

    fn window_of_tab(&self, tab: TabId) -> Option<WindowId> {
        let base = tab / TAB_STRIDE * TAB_STRIDE;
        self.windows.iter().find(|(_, w)| w.app.tabs().first().is_some_and(|t| t.id / TAB_STRIDE * TAB_STRIDE == base)).map(|(&id, _)| id)
    }

    fn flush(&mut self, window: &Win<P>) {
        let id = window.id();
        let Some(st) = self.windows.get_mut(&id) else { return };
        let fx: Vec<Effect> = st.app.drain_effects().collect();
        for e in fx {
            match e {
                Effect::Provider { provider, req } => self.submit(provider, req),
                Effect::Resort { tab, gen, snap, view } => {
                    let _ = self.sorter.send(SortJob { window: id, tab, gen, snap, view });
                }
                Effect::Icon(req) => {
                    let visible = self.windows[&id].visible.clone();
                    let _ = self.shell.send(ShellJob::Icon { window: id, req, visible });
                }
                Effect::Thumbnail(req) => {
                    let visible = self.windows[&id].visible.clone();
                    let _ = self.thumbs.send(ThumbJob { window: id, req, visible });
                }
                Effect::ClearThumbnails => {
                    if let Some(c) = &self.thumb_cache {
                        locked(c).clear();
                    }
                }
                Effect::TypeName(ext) => {
                    let _ = self.shell.send(ShellJob::TypeName(ext));
                }
                Effect::SetClipboard(text) => window.set_clipboard_text(&text),
                Effect::Paste => {
                    if let (Some(text), Some(st)) = (window.clipboard_text(), self.windows.get_mut(&id)) {
                        st.app.on_paste(&text);
                    }
                }
                Effect::Window(WindowCmd::Minimize) => window.minimize(),
                Effect::Window(WindowCmd::ToggleMaximize) => window.toggle_maximize(),
                Effect::Window(WindowCmd::Close) => {
                    if let Some(st) = self.windows.get(&id) {
                        self.saved = Saved::new(st.app.persisted(), Some(window.placement())).with_nav(st.app.nav_state());
                    }
                    window.close();
                }
                Effect::Cursor(c) => window.set_cursor(c),
                Effect::Title(t) => window.set_title(&t),
                Effect::Open(cmd) => {
                    if let Some(st) = self.windows.get_mut(&id) {
                        st.app.open(&cmd, &P::known_folder, Instant::now());
                    }
                }
                Effect::Search(req) => self.search.submit(id, req),
                Effect::NewWindow(routes) => {
                    let persisted = self.saved.persisted.clone();
                    let app = self.new_app(routes, persisted);
                    self.pending.push_back(app);
                    window.open_window(WindowOptions { title: "Flit".into(), placement: None, dark: window.is_dark_mode() });
                }
                Effect::SetClipboardFiles { paths, cut } => window.set_clipboard_files(&paths, cut),
                Effect::ShellMenu(menu) => window.shell_menu(menu),
                Effect::Properties(paths) => window.properties(paths),
                Effect::StartDrag(paths) => window.start_drag(paths),
                Effect::Pinned(pinned) => {
                    for (_, st) in self.windows.iter_mut().filter(|(w, _)| **w != id) {
                        st.app.set_pinned(pinned.clone());
                    }
                    self.saved.nav.pinned = pinned;
                    (self.wake)();
                }
                Effect::RefreshPlaces => self.places.refresh(),
                Effect::Timer(d) => window.set_timer(d.as_millis().clamp(1, u32::MAX as u128) as u32),
                Effect::Tags(req) => self.tags.submit(Some(id), req),
            }
        }
        if self.windows.get(&id).is_some_and(|st| st.app.has_effects()) {
            return self.flush(window);
        }
        if let Some(st) = self.windows.get_mut(&id) {
            if let Some(area) = st.app.ime_area() {
                window.set_ime_area(area);
            }
            if st.app.needs_redraw() {
                window.request_redraw();
            }
        }
    }

    fn drain(&mut self) {
        let now = Instant::now();
        while let Ok(ev) = self.events.try_recv() {
            let target = match &ev {
                ProviderEvent::Changes { tab, .. } | ProviderEvent::Rescan { tab, .. } | ProviderEvent::DirSize { tab, .. } => self.window_of_tab(*tab),
                ProviderEvent::OpDone { tab, .. } => self.window_of_tab(*tab).or(self.active),
                ProviderEvent::Navigate { tab, .. } | ProviderEvent::OpProgress { tab, .. } | ProviderEvent::OpConflict { tab, .. } | ProviderEvent::Status { tab, .. } => self.window_of_tab(*tab),
            };
            if let Some(st) = target.and_then(|id| self.windows.get_mut(&id)) {
                st.app.on_provider_event(ev, now);
            }
        }
        while let Ok(done) = self.done.try_recv() {
            match done {
                Done::Icon { window, key, result } => {
                    if let Some(st) = self.windows.get_mut(&window) {
                        st.app.on_icon(&mut st.gfx, key, result);
                    }
                }
                Done::Thumb { window, key, result } => {
                    if let Some(st) = self.windows.get_mut(&window) {
                        st.app.on_thumb(&mut st.gfx, key, result);
                    }
                }
                Done::TypeName(ext, name) => {
                    for st in self.windows.values_mut() {
                        st.app.on_type_name(ext.clone(), name.clone());
                    }
                }
                Done::Sorted { window, tab, gen, view, base_len, order, ms } => {
                    if let Some(st) = self.windows.get_mut(&window) {
                        st.app.on_resorted(tab, gen, view, base_len, order);
                    }
                    if let Some(m) = &mut self.metrics {
                        m.on_sorted(ms);
                    }
                }
            }
        }
        self.take_opens();
        while let Ok((window, ev)) = self.tags.events.try_recv() {
            let wanted = |id: &WindowId| window.is_none_or(|w| w == *id);
            for (_, st) in self.windows.iter_mut().filter(|(id, _)| wanted(id)) {
                st.app.on_tags(ev.clone(), now);
            }
            for app in self.pending.iter_mut().filter(|_| window.is_none()) {
                app.on_tags(ev.clone(), now);
            }
        }
        while let Ok((window, ev)) = self.search.events.try_recv() {
            if let Some(st) = self.windows.get_mut(&window) {
                st.app.on_search_event(ev, now);
            }
        }
        if let Some(places) = self.places.poll() {
            self.distros = Some(places.wsl.clone());
            for st in self.windows.values_mut() {
                st.app.set_places(places.clone());
            }
        }
    }

    fn take_opens(&mut self) {
        for o in self.opens.try_iter() {
            match &o.tag {
                Some(name) => self.tags.submit(self.active, TagRequest::Command { name: name.clone(), items: tags::command_items::<P>(&o) }),
                None => self.queued_opens.push(o),
            }
        }
    }

    fn inject_due(&mut self, window: &Win<P>) {
        let due = self.injection.as_ref().is_some_and(|(at, _)| Instant::now() >= *at);
        let (Some(st), true) = (self.windows.get_mut(&window.id()), due) else { return };
        let Some((_, ev)) = self.injection.take() else { return };
        let now = Instant::now();
        st.app.on_event(&ev, now);
        if let Some(m) = &mut self.metrics {
            m.on_injected(now);
        }
        window.request_redraw();
    }

    fn apply_opens(&mut self, window: &Win<P>) {
        if self.queued_opens.is_empty() || self.active.is_some_and(|a| a != window.id()) {
            return;
        }
        let Some(st) = self.windows.get_mut(&window.id()) else { return };
        let now = Instant::now();
        for mut o in std::mem::take(&mut self.queued_opens) {
            if o.is_empty() {
                o.paths.push(Location::from_local(P::home()).to_bytes());
            }
            st.app.open(&o, &P::known_folder, now);
        }
        window.activate();
        self.flush(window);
    }

    fn render(&mut self, window: &Win<P>) {
        let id = window.id();
        let needs = self.windows.get(&id).is_some_and(|st| st.renderer.is_none());
        self.poll_gpu(needs);
        let Some(st) = self.windows.get_mut(&id) else { return };
        if st.renderer.is_none() || (st.software && self.hw.is_some()) {
            let (dev, software) = match (&self.hw, &self.sw) {
                (Some(d), _) => (d, false),
                (None, Some(d)) => (d, true),
                (None, None) => {
                    tracing::error!("no graphics device");
                    return;
                }
            };
            st.renderer = None;
            match P::Renderer::new(dev, window, ATLAS) {
                Ok(r) => {
                    st.gfx.reset_atlas();
                    st.gfx.thumbs.clear();
                    st.renderer = Some(r);
                    st.software = software;
                    st.device_gen = self.device_gen;
                }
                Err(e) => {
                    tracing::error!("renderer: {e}");
                    return;
                }
            }
            if self.hw.is_some() && self.windows.values().all(|w| !w.software) {
                self.sw = None;
            }
        }
        let Some(st) = self.windows.get_mut(&id) else { return };
        let t = Instant::now();
        let animating = st.app.frame(&mut st.gfx, t);
        let renderer = st.renderer.as_mut().unwrap();
        for &(slot, off) in &st.gfx.uploads.rects {
            let len = (slot.w * slot.h * 4) as usize;
            renderer.upload(slot, &st.gfx.uploads.data[off..off + len]);
        }
        st.gfx.uploads.clear();
        let thumbs = &mut st.gfx.thumbs.uploads;
        for &(slot, off) in &thumbs.rects {
            renderer.upload_thumb(slot, &thumbs.data[off..off + (slot.w * slot.h * 4) as usize]);
        }
        thumbs.clear();
        let lost = matches!(renderer.render(&st.gfx.dl), Err(RenderError::DeviceLost));
        if lost {
            let (software, stale) = (st.software, st.device_gen != self.device_gen);
            st.renderer = None;
            if !stale {
                self.device_gen += 1;
                if software {
                    self.sw = None;
                } else {
                    self.hw = P::Renderer::device(false).map_err(|e| tracing::error!("graphics device: {e}")).ok();
                }
                if self.hw.is_none() && self.sw.is_none() {
                    self.sw = P::Renderer::device(true).map_err(|e| tracing::error!("software device: {e}")).ok();
                }
            }
            window.request_redraw();
            return;
        }
        let (gen, first, last) = st.app.visible_rows();
        st.visible.gen.store(gen, Ordering::Relaxed);
        st.visible.first.store(first, Ordering::Relaxed);
        st.visible.last.store(last, Ordering::Relaxed);
        self.places.start_once();
        let now = Instant::now();
        if animating {
            window.request_redraw();
        }
        if let Some(m) = &mut self.metrics {
            let cpu = (now - t).as_secs_f32() * 1e3;
            match m.after_frame(&st.app, st.software, cpu, now) {
                Step::None => {}
                Step::Inject(ev) => st.app.on_event(&ev, now),
                Step::InjectAfter(delay, ev) => {
                    self.injection = Some((now + delay, ev));
                    let wake = self.wake.clone();
                    let _ = spawn("flit-metrics-input", move || {
                        std::thread::sleep(delay);
                        wake();
                    });
                }
                Step::Finish => {
                    self.metrics = None;
                    window.close();
                }
            }
            window.request_redraw();
        }
        self.flush(window);
    }
}

impl<P: Platform> WindowHandler<Ws<P>> for Handler<P> {
    fn created(&mut self, window: &Win<P>) {
        let mut app = match self.pending.pop_front() {
            Some(app) => app,
            None => {
                let persisted = self.saved.persisted.clone();
                self.new_app(Vec::new(), persisted)
            }
        };
        let text = match P::text() {
            Ok(t) => t,
            Err(e) => {
                tracing::error!("text: {e}");
                window.close();
                return;
            }
        };
        let now = Instant::now();
        let (width, height) = window.size();
        app.on_event(&Event::ScaleChanged(window.scale()), now);
        app.on_event(&Event::Resized { width, height }, now);
        app.on_event(&Event::ThemeChanged { dark: window.is_dark_mode() }, now);
        app.set_maximized(window.is_maximized());
        window.set_title(&format!("{} — Flit", app.tab().title()));
        let id = window.id();
        self.windows.insert(id, WinState { app, gfx: Gfx::new(text, ATLAS), renderer: None, software: false, device_gen: 0, visible: Arc::default() });
        self.active = Some(id);
        self.flush(window);
        self.apply_opens(window);
        window.request_redraw();
    }

    fn event(&mut self, window: &Win<P>, event: Event) {
        let id = window.id();
        if event == Event::Focus(true) {
            self.active = Some(id);
        }
        let Some(st) = self.windows.get_mut(&id) else { return };
        if let Event::Resized { width, height } = event {
            if let Some(r) = &mut st.renderer {
                r.resize(width, height);
            }
            st.app.set_maximized(window.is_maximized());
        }
        st.app.on_event(&event, Instant::now());
        self.flush(window);
    }

    fn hit_test(&mut self, window: &Win<P>, x: f32, y: f32) -> HitArea {
        self.windows.get(&window.id()).map_or(HitArea::Client, |st| st.app.hit_test(x, y))
    }

    fn redraw(&mut self, window: &Win<P>) {
        self.render(window);
    }

    fn woken(&mut self, window: &Win<P>) {
        self.drain();
        self.inject_due(window);
        self.poll_gpu(false);
        if self.hw.is_some() && self.windows.get(&window.id()).is_some_and(|st| st.software) {
            window.request_redraw();
        }
        self.apply_opens(window);
        if let Some(st) = self.windows.get_mut(&window.id()) {
            st.app.poll();
        }
        self.flush(window);
    }

    fn drag(&mut self, window: &Win<P>, event: DragEvent) -> DropReply {
        let Some(st) = self.windows.get_mut(&window.id()) else { return DropReply::default() };
        let reply = DropReply { effect: st.app.drag(&event, Instant::now()), caption: st.app.drop_caption() };
        self.flush(window);
        reply
    }

    fn destroyed(&mut self, window: &Win<P>) {
        let id = window.id();
        if let Some(mut st) = self.windows.remove(&id) {
            if self.windows.is_empty() {
                self.saved = Saved::new(st.app.persisted(), Some(window.placement())).with_nav(st.app.nav_state());
            }
            st.app.shutdown();
            for e in st.app.drain_effects().collect::<Vec<_>>() {
                if let Effect::Provider { provider, req } = e {
                    self.submit(provider, req);
                }
            }
        }
        self.search.forget(id);
        if self.active == Some(id) {
            self.active = self.windows.keys().next().copied();
        }
        if self.windows.is_empty() {
            let tx = self.sink.lock().unwrap_or_else(|e| e.into_inner()).take();
            self.take_opens();
            if self.queued_opens.is_empty() {
                state::save::<P>(&self.saved);
                self.local.wait_idle(OPS_GRACE);
            } else {
                *self.sink.lock().unwrap_or_else(|e| e.into_inner()) = tx;
                window.open_window(WindowOptions { title: "Flit".into(), placement: self.saved.placement(), dark: window.is_dark_mode() });
            }
        }
    }
}

use flit_core::input::HitArea;

pub fn run<P: Platform>(open: Open, instance: Option<P::Instance>, metrics: Option<PathBuf>, started: Instant) -> Result<(), PlatformError> {
    let saved = state::load::<P>();
    let ws = Ws::<P>::new()?;
    let waker = ws.waker();
    let wake: Wake = Arc::new(move || waker.wake());
    let (gpu_tx, gpu) = unbounded();
    let gpu_wake = wake.clone();
    spawn("flit-gpu", move || {
        for software in [true, false] {
            let _ = gpu_tx.send((software, P::Renderer::device(software)));
            gpu_wake();
        }
    })?;
    let (events_tx, events) = unbounded();
    let local = Arc::new(LocalProvider::new::<_, P::Watcher, P::Shell>(P::fs(), FS_THREADS, events_tx.clone(), wake.clone())?);
    let (opens_tx, opens) = unbounded();
    let sink: OpenSink = Arc::new(Mutex::new(Some(opens_tx)));
    if let Some(i) = instance {
        i.listen(sink.clone(), wake.clone());
    }
    let thumb_cache: Cache = P::state_dir().map(|d| Arc::new(Mutex::new(ThumbCache::new(d.join("thumbs"), saved.persisted.prefs.thumb_cache_mb))));
    let (shell, thumbs, sorter, done) = start_workers::<P>(wake.clone(), thumb_cache.clone())?;
    let tags = TagWorker::start::<P>(local.tags().clone(), wake.clone())?;
    let search = SearchClient::start::<P>(wake.clone(), P::utc_offset_minutes(), local.tags().clone())?;
    let (routes, errors) = if open.is_empty() { Default::default() } else { open::routes(&open, &P::known_folder) };
    let new_window = open.target == OpenTarget::NewWindow;
    let mut h = Handler::<P> {
        windows: HashMap::new(),
        pending: VecDeque::new(),
        next_base: 0,
        active: None,
        local,
        wsl: HashMap::new(),
        distros: None,
        events_tx,
        events,
        opens,
        sink,
        queued_opens: Vec::new(),
        shell,
        thumbs,
        thumb_cache,
        sorter,
        done,
        saved: saved.clone(),
        metrics: metrics.map(|m| Metrics::new(m, started, P::refresh_period())),
        places: PlacesWorker::new::<P>(wake.clone()),
        injection: None,
        wake,
        gpu,
        hw: None,
        sw: None,
        gpu_pending: 2,
        device_gen: 0,
        search,
        tags,
    };
    let mut first = h.new_app(routes, saved.persisted.clone());
    first.show_open_errors(&errors);
    h.pending.push_back(first);
    let placement = if new_window { None } else { saved.placement() };
    ws.run(WindowOptions { title: "Flit".into(), placement, dark: P::dark_mode() }, h)
}
