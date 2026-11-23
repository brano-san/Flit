use crate::jumps::{self, JumpList};
use crate::metrics::{Metrics, Step};
use crate::places::{worth_keeping, KeptIcon, KeptIcons, PlacesWorker, Reply, ICONS_FILE, PLACES_FILE};
use crate::search_client::SearchClient;
use crate::session::{self, Files, Session, Writer};
use crate::settings::{self, Job, Settings};
use crate::state::{self, Saved, Tools};
use crate::tags::{self, TagWorker};
use crate::thumbs::ThumbCache;
use crate::update::{self, UpdateWorker};
use flit_core::update::{UpdateEvent, UpdateRequest};
use crossbeam_channel::{unbounded, Receiver, Sender};
use flit_core::i18n::S;
use flit_core::input::Event;
use flit_core::listing::DirSnapshot;
use flit_core::open::{self, Open, OpenTarget, Route};
use flit_core::preview::{PreviewData, PreviewRequest};
use flit_core::sort::{self, ViewSpec};
use flit_core::tags::TagRequest;
use flit_core::text::Gfx;
use flit_core::ui::{About, App, DragEvent, Effect, IconRequest, IconResult, MovedTab, NavState, Persisted, SettingsCmd, Startup, Typed, Value, WindowCmd, WindowSession};
use flit_core::{FsPath, Location, ProviderEvent, ProviderId, ProviderRequest, TabId};
use flit_platform::*;
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const ATLAS: u32 = 2048;
const OPS_GRACE: Duration = Duration::from_secs(2);
const ACTIVATION_WAIT: Duration = Duration::from_secs(2);
const ICON_REFRESH_DELAY: Duration = Duration::from_secs(2);
const FS_THREADS: usize = 4;
const SHELL_THREADS: usize = 4;
const THUMB_THREADS: usize = 3;
const PREVIEW_THREADS: usize = 2;
const BACKDROP_PX: u32 = 2560;
const ICON_MARGIN: u32 = 32;
const ADMIN_PASSWORD_WAIT: Duration = Duration::from_secs(300);
const ADMIN_IDLE: Duration = Duration::from_secs(10);
const TAB_STRIDE: TabId = 1 << 32;
const TORN_GRAB: (f32, f32) = (60.0, 18.0);

mod agents {
    include!(concat!(env!("OUT_DIR"), "/wsl_agent.rs"));
}

type Ws<P> = <P as WindowPlatform>::WindowSystem;
type Win<P> = <Ws<P> as WindowSystem>::Window;
type Device<P> = <<P as WindowPlatform>::Renderer as Renderer>::Device;

#[derive(Default)]
struct Rows {
    gen: AtomicU64,
    first: AtomicU32,
    last: AtomicU32,
}

fn within(req: &IconRequest, (gen, first, last): (u64, u32, u32)) -> bool {
    req.gen == gen && req.row + ICON_MARGIN >= first && req.row <= last + ICON_MARGIN
}

impl Rows {
    fn wants(&self, req: &IconRequest) -> bool {
        within(req, (self.gen.load(Ordering::Relaxed), self.first.load(Ordering::Relaxed), self.last.load(Ordering::Relaxed)))
    }

    fn store(&self, (gen, first, last): (u64, u32, u32)) {
        self.gen.store(gen, Ordering::Relaxed);
        self.first.store(first, Ordering::Relaxed);
        self.last.store(last, Ordering::Relaxed);
    }
}

#[derive(Default)]
struct Visible {
    closed: std::sync::atomic::AtomicBool,
    pane: Rows,
    parked: Mutex<Vec<(u64, u32, u32)>>,
    content: Mutex<Vec<u64>>,
    overlays: crate::overlays::Shown,
}

impl Visible {
    fn wants(&self, req: &IconRequest) -> bool {
        if self.closed.load(Ordering::Relaxed) {
            return false;
        }
        if req.kind == flit_core::ui::IconKind::Content {
            return self.content.lock().is_ok_and(|keys| keys.contains(&req.key));
        }
        req.gen == 0 || self.pane.wants(req) || self.parked.lock().is_ok_and(|p| p.iter().any(|&r| within(req, r)))
    }
}

const FOLDER_BATCH: usize = 100;

fn batched<S: Shell>(req: &IconRequest) -> bool {
    S::BATCHES_FOLDER_ICONS && req.kind == flit_core::ui::IconKind::CustomFolder
}

fn folder_batches(reqs: Vec<IconRequest>) -> Vec<Vec<IconRequest>> {
    let mut out: Vec<Vec<IconRequest>> = Vec::new();
    for req in reqs {
        match out.last_mut() {
            Some(last) if last.len() < FOLDER_BATCH => last.push(req),
            _ => out.push(vec![req]),
        }
    }
    out
}

fn icon_result<S: Shell>(shell: &mut S, req: &IconRequest) -> IconResult {
    match shell.shared_icon(req) {
        (Some(image), Some(share)) => IconResult::Shared(share, image),
        (Some(image), None) => IconResult::Ready(image),
        (None, _) => IconResult::Failed,
    }
}

fn batch_icons<S: Shell>(shell: &mut S, window: WindowId, reqs: Vec<IconRequest>, visible: &Visible) -> Vec<Done> {
    let (wanted, skipped): (Vec<IconRequest>, Vec<IconRequest>) = reqs.into_iter().partition(|r| visible.wants(r));
    if !wanted.is_empty() {
        shell.prepare_icons(&wanted);
    }
    let mut out: Vec<Done> = skipped.into_iter().map(|req| Done::Icon { window, req, result: IconResult::Skipped }).collect();
    out.extend(wanted.into_iter().map(|req| {
        let result = if visible.wants(&req) { icon_result(shell, &req) } else { IconResult::Skipped };
        Done::Icon { window, req, result }
    }));
    out
}

enum ShellJob {
    Icon { window: WindowId, req: IconRequest, visible: Arc<Visible> },
    Icons { window: WindowId, reqs: Vec<IconRequest>, visible: Arc<Visible> },
    TypeName(Vec<u8>),
    LinkChoices { window: WindowId, generation: u64, items: Option<Vec<FsPath>>, dest: FsPath },
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

fn thumbnail<S: Shell>(shell: &mut S, cache: Option<&Mutex<ThumbCache>>, req: &IconRequest, current: &dyn Fn() -> bool, visible: &Visible) -> IconResult {
    if !current() {
        return IconResult::Skipped;
    }
    if let Some(b) = cache.and_then(|c| locked(c).get(req.key)) {
        return IconResult::Ready(b);
    }
    match crate::preview::thumbnail(shell, req, current) {
        Some(b) => {
            if let Some(c) = cache {
                let mut cache = locked(c);
                if !visible.closed.load(Ordering::Relaxed) {
                    cache.put(req.key, &b);
                }
            }
            IconResult::Ready(b)
        }
        None if !current() => IconResult::Skipped,
        None => IconResult::Failed,
    }
}

enum SortJob {
    Rows { window: WindowId, tab: TabId, gen: u64, snap: Arc<DirSnapshot>, view: ViewSpec },
    Pictures { window: WindowId, tab: TabId, ask: u64, snap: Arc<DirSnapshot>, cancel: flit_core::provider::GenToken },
}

enum PreviewJob {
    Load { window: WindowId, req: PreviewRequest },
    Pretty { window: WindowId, req: PreviewRequest },
    PdfPage { window: WindowId, req: PreviewRequest, page: u32 },
    QuickView { window: WindowId, path: FsPath },
    Backdrop { window: WindowId, gen: u64, path: FsPath },
    Browse { window: WindowId, exts: &'static [&'static str], folder: bool, start: Option<FsPath> },
    ColumnProps { window: WindowId, tab: TabId, gen: u64, ask: u64, dir: FsPath, names: Vec<(Vec<u8>, bool)>, system: bool, portable: bool },
}

enum Done {
    Icon { window: WindowId, req: IconRequest, result: IconResult },
    Refreshed { req: IconRequest, image: flit_core::ui::Bitmap },
    Thumb { window: WindowId, key: u64, result: IconResult },
    Overlays { window: WindowId, reply: flit_core::ui::OverlayReply },
    TypeName(Vec<u8>, Option<String>),
    LinkChoices { window: WindowId, generation: u64, choices: flit_core::provider::LinkChoices },
    Sorted { window: WindowId, tab: TabId, gen: u64, view: ViewSpec, base_len: usize, order: Vec<u32>, ms: f64 },
    PictureShare { window: WindowId, tab: TabId, ask: u64, share: flit_core::ui::PictureShare },
    Preview { window: WindowId, gen: u64, data: PreviewData },
    Pretty { window: WindowId, gen: u64, result: Result<flit_core::preview::Doc, flit_core::preview::PrettyError> },
    PdfPage { window: WindowId, gen: u64, page: u32, image: Option<flit_core::ui::Bitmap> },
    QuickView { window: WindowId, shown: bool },
    Backdrop { window: WindowId, gen: u64, image: Option<flit_core::ui::Bitmap> },
    Browsed { window: WindowId, path: Option<FsPath> },
    ColumnProps { window: WindowId, tab: TabId, gen: u64, ask: u64, rows: Vec<(Vec<u8>, flit_core::listing::ColumnValues)> },
}

fn tools(app: &App) -> Tools {
    let mut tools = Tools::new(app.search_history().to_vec());
    tools.commands = app.command_usage();
    tools
}

fn spawn(name: &str, f: impl FnOnce() + Send + 'static) -> Result<(), PlatformError> {
    std::thread::Builder::new().name(name.into()).spawn(f).map(drop).map_err(|e| PlatformError::new(-1, format!("cannot start {name}: {e}")))
}

type Workers = (Sender<ShellJob>, Sender<ThumbJob>, Sender<SortJob>, Sender<PreviewJob>, crate::overlays::Overlays, Receiver<Done>);

fn start_workers<P: ShellPlatform + 'static>(wake: Wake, cache: Cache, refresh: Vec<IconRequest>) -> Result<Workers, PlatformError> {
    let (done_tx, done) = unbounded();
    let (tx, refresh_wake) = (done_tx.clone(), wake.clone());
    spawn("flit-icon-refresh", move || {
        std::thread::sleep(ICON_REFRESH_DELAY);
        let Ok(mut shell) = P::Shell::new() else { return };
        let fresh: Vec<Done> = refresh.into_iter().filter_map(|req| shell.icon(&req).map(|image| Done::Refreshed { req, image })).collect();
        if fresh.into_iter().all(|d| tx.send(d).is_ok()) {
            refresh_wake();
        }
    })?;
    let (preview_tx, preview_rx) = unbounded::<PreviewJob>();
    for k in 0..PREVIEW_THREADS {
        let (rx, tx, wake) = (preview_rx.clone(), done_tx.clone(), wake.clone());
        spawn(&format!("flit-preview-{k}"), move || {
            let Ok(mut shell) = P::Shell::new() else { return };
            for job in rx {
                let done = match job {
                    PreviewJob::Load { window, req } => match crate::preview::load(&mut shell, &req) {
                        Some(data) => Done::Preview { window, gen: req.gen, data },
                        None => continue,
                    },
                    PreviewJob::Pretty { window, req } => match crate::preview::pretty::load(&req) {
                        Some(result) => Done::Pretty { window, gen: req.gen, result },
                        None => continue,
                    },
                    PreviewJob::PdfPage { window, req, page } => {
                        let image = shell.pdf_page(&req, page);
                        if !req.cancel.is_current(req.gen) {
                            continue;
                        }
                        Done::PdfPage { window, gen: req.gen, page, image }
                    }
                    PreviewJob::QuickView { window, path } => Done::QuickView { window, shown: shell.quick_view(&path) },
                    PreviewJob::ColumnProps { window, tab, gen, ask, dir, names, system, portable } => {
                        let rows = names
                            .into_iter()
                            .map(|(n, probe)| {
                                let item = dir.join(&n);
                                let mut values = if system { shell.column_props(&item) } else { flit_core::listing::ColumnValues::default() };
                                values.elf = probe && crate::preview::is_elf_program(&item);
                                if portable {
                                    values.text.resize(flit_core::listing::SHELL_PROPS.len(), String::new());
                                    if let Ok(meta) = crate::portable_meta::read(&item) {
                                        values.text.extend([meta.display_rating(), meta.comment, meta.tags.join(", ")]);
                                    }
                                }
                                (n, values)
                            })
                            .collect();
                        Done::ColumnProps { window, tab, gen, ask, rows }
                    }
                    PreviewJob::Backdrop { window, gen, path } => Done::Backdrop { window, gen, image: shell.preview_image(&path, BACKDROP_PX) },
                    PreviewJob::Browse { window, exts, folder, start } => Done::Browsed { window, path: shell.browse(exts, folder, start.as_ref()) },
                };
                if tx.send(done).is_err() {
                    return;
                }
                wake();
            }
        })?;
    }
    let (thumb_tx, thumb_rx) = unbounded::<ThumbJob>();
    for k in 0..THUMB_THREADS {
        let (rx, tx, wake, cache) = (thumb_rx.clone(), done_tx.clone(), wake.clone(), cache.clone());
        spawn(&format!("flit-thumb-{k}"), move || {
            let Ok(mut shell) = P::Shell::new() else { return };
            for ThumbJob { window, req, visible } in rx {
                let result = thumbnail(&mut shell, cache.as_deref(), &req, &|| visible.wants(&req), &visible);
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
            let _ = shell.icon(&IconRequest { key: 0, kind: flit_core::ui::IconKind::Place, path: P::home(), size: 16, gen: 0, row: 0 });
            for job in rx {
                let done = match job {
                    ShellJob::Icon { window, req, visible } => {
                        let result = if !visible.wants(&req) { IconResult::Skipped } else { icon_result(&mut shell, &req) };
                        vec![Done::Icon { window, req, result }]
                    }
                    ShellJob::Icons { window, reqs, visible } => batch_icons(&mut shell, window, reqs, &visible),
                    ShellJob::TypeName(ext) => {
                        let name = shell.type_name(&ext);
                        vec![Done::TypeName(ext, name)]
                    }
                    ShellJob::LinkChoices { window, generation, items, dest } => vec![Done::LinkChoices { window, generation, choices: shell.link_choices(items.as_deref(), &dest) }],
                };
                if done.into_iter().any(|d| tx.send(d).is_err()) {
                    return;
                }
                wake();
            }
        })?;
    }
    let (overlay_tx, overlay_wake) = (done_tx.clone(), wake.clone());
    let overlays = crate::overlays::Overlays::start::<P::Shell>(Arc::new(move |window, reply| {
        if overlay_tx.send(Done::Overlays { window, reply }).is_ok() {
            overlay_wake();
        }
    }));
    let (sort_tx, sort_rx) = unbounded::<SortJob>();
    spawn("flit-sort", move || {
        for job in sort_rx {
            let done = match job {
                SortJob::Rows { window, tab, gen, snap, view } => {
                    let t = Instant::now();
                    let mut order = Vec::new();
                    sort::sort(&snap.listing, &snap.keys, view, &mut order);
                    Done::Sorted { window, tab, gen, view, base_len: snap.listing.len(), order, ms: t.elapsed().as_secs_f64() * 1e3 }
                }
                SortJob::Pictures { window, tab, ask, snap, cancel } => {
                    let Some(share) = flit_core::ui::count_pictures(&snap.listing, &cancel, ask) else { continue };
                    Done::PictureShare { window, tab, ask, share }
                }
            };
            if done_tx.send(done).is_err() {
                return;
            }
            wake();
        }
    })?;
    Ok((shell_tx, thumb_tx, sort_tx, preview_tx, overlays, done))
}

struct WinState<P: Platform> {
    app: App,
    gfx: Gfx<P::Text>,
    renderer: Option<P::Renderer>,
    software: bool,
    device_gen: u64,
    visible: Arc<Visible>,
    session_rev: u64,
    placement: Option<Placement>,
    born: Option<Instant>,
    preview_rev: u64,
    backdrop_rev: u64,
    dark: Option<bool>,
    selection_key: Option<(TabId, u64, u64, Option<u32>)>,
    progress: Option<(u32, bool)>,
}

fn apply_bookmark_effect<'a>(effect: Effect, nav: &mut NavState, windows: impl Iterator<Item = &'a mut App>, pending: &'a mut VecDeque<App>, quick_pinned: impl FnOnce(&[Location], &[Location]), bookmarks_changed: impl FnOnce(&[Location], &[Location])) {
    let apps = windows.chain(pending.iter_mut());
    let pinned = match effect {
        Effect::Pinned(pinned) => {
            quick_pinned(&nav.pinned, &pinned);
            let diff = flit_core::quick_sync::pins_diff(&nav.pinned, &pinned);
            if !diff.is_empty() {
                bookmarks_changed(&diff.added, &diff.removed);
            }
            pinned
        }
        Effect::PinnedCleared => Vec::new(),
        Effect::PinnedFiles(files) => {
            for app in apps {
                app.set_pinned_files(files.clone());
            }
            nav.files = files;
            return;
        }
        _ => return,
    };
    for app in apps {
        app.set_pinned(pinned.clone());
    }
    nav.pinned = pinned;
}

struct Handler<P: Platform> {
    windows: HashMap<WindowId, WinState<P>>,
    pending: VecDeque<App>,
    next_base: TabId,
    active: Option<WindowId>,
    local: Arc<LocalProvider>,
    wsl: HashMap<String, WslProvider>,
    admin: Option<WslProvider>,
    distros: Option<Vec<String>>,
    events_tx: Sender<ProviderEvent>,
    events: Receiver<ProviderEvent>,
    opens: Receiver<Open>,
    sink: OpenSink,
    queued_opens: Vec<Open>,
    shell: Sender<ShellJob>,
    overlays: crate::overlays::Overlays,
    thumbs: Sender<ThumbJob>,
    thumb_cache: Cache,
    sorter: Sender<SortJob>,
    previews: Sender<PreviewJob>,
    done: Receiver<Done>,
    saved: Saved,
    command_usage: Arc<Mutex<flit_core::ui::CommandUsage>>,
    metrics: Option<Metrics>,
    graphics: Graphics<Device<P>>,
    injection: Option<(Instant, Event)>,
    wake: Wake,
    device_gen: u64,
    search: SearchClient,
    places: PlacesWorker,
    kept: KeptIcons,
    files: Option<Files>,
    writer: Option<Writer>,
    crashed: Option<Session>,
    typed: Typed,
    queued_windows: VecDeque<(WindowSession, Option<Placement>)>,
    startup_open: Option<Open>,
    raise: Option<WindowId>,
    ending: bool,
    timing: bool,
    requested: Option<Instant>,
    tags: TagWorker,
    git: crate::git::GitWorker,
    settings: Settings,
    dates: (String, String),
    about: About,
    updates: update::UpdateWorker,
    update_window: Option<WindowId>,
    restarting: bool,
    awake: KeepAwake,
    quitting: std::collections::HashSet<WindowId>,
    quit_session: Option<Session>,
    closed_session: Option<Session>,
    tray: Option<flit_platform::tray::Tray>,
    background_start: bool,
    tray_seen: bool,
    background_startup: Option<StartupRestore>,
    picker_result: Option<crate::picker::ResultSlot>,
    jumps: JumpList,
    quick: crate::quick_sync::QuickSyncWorker,
}

struct StartupRestore {
    previous: Option<Session>,
    allowed: bool,
    dump: Option<FsPath>,
}

fn tray_controls_available(tray: Option<&flit_platform::tray::Tray>) -> bool {
    tray.is_some_and(|t| t.available() == Some(true))
}

impl<P: Platform> Handler<P> {
    fn background_requested(&self) -> bool {
        self.background_start || settings::flag(&self.settings.values, flit_core::ui::settings::BACKGROUND) == Some(true)
    }

    fn sync_tray(&self) {
        let Some(tray) = &self.tray else { return };
        let requested = self.background_requested();
        #[cfg(target_os = "linux")]
        let disabled = !requested && tray.menu().is_some();
        tray.set_menu(requested.then(|| {
            flit_platform::tray::Menu {
                new_window: S::MenuNewWindow.t().into(),
                bookmarks: S::SidebarSectionBookmarks.t().into(),
                pinned: self
                    .saved
                    .nav
                    .pinned
                    .iter()
                    .map(|loc| {
                        let label = flit_core::name::display(loc.file_name());
                        (loc.clone(), if label.is_empty() { loc.display().into_owned() } else { label.into_owned() })
                    })
                    .collect(),
                quit: S::ActionsAppQuit.t().into(),
            }
        }));
        #[cfg(target_os = "linux")]
        if disabled {
            self.check_tray();
        }
    }

    #[cfg(target_os = "linux")]
    fn check_tray(&self) {
        if self.tray.as_ref().is_some_and(|tray| tray.menu().is_none()) {
            self.settings.send(Job::CheckTray);
        }
    }

    fn tray_actions(&mut self, window: Option<&Win<P>>) {
        use flit_platform::tray::Action;
        let Some(tray) = &self.tray else { return };
        for action in tray.take_actions() {
            match action {
                Action::Activate => match window {
                    Some(window) => window.activate(),
                    None => self.queued_opens.push(Open::default()),
                },
                Action::NewWindow => self.queued_opens.push(Open { target: OpenTarget::NewWindow, ..Default::default() }),
                Action::Bookmark(loc) => self.queued_opens.push(Open::location(&loc, OpenTarget::NewWindow)),
                Action::Quit => {
                    self.quit();
                    break;
                }
            }
        }
    }

    fn finish_idle(&mut self) -> bool {
        let Some(sink) = self.sink.lock().unwrap_or_else(|e| e.into_inner()).take() else { return true };
        if !self.ending && self.quit_session.is_none() {
            self.take_opens();
            if !self.queued_opens.is_empty() {
                *self.sink.lock().unwrap_or_else(|e| e.into_inner()) = Some(sink);
                return false;
            }
        }
        self.settings.finish();
        if self.picker_result.is_none() && self.background_startup.is_none() && flit_core::i18n::lang() != flit_core::i18n::Lang::Pseudo {
            state::save::<P>(&self.saved);
        }
        let snapshot = self.quit_session.take().or(self.closed_session.take());
        self.final_session(snapshot);
        self.local.wait_idle(OPS_GRACE);
        true
    }

    fn start_gpu(&mut self) {
        if self.graphics.pending == 0 && self.graphics.hw.is_none() && self.graphics.sw.is_none() {
            match start_gpu::<P>(self.wake.clone()) {
                Ok(rx) => {
                    self.graphics.replies = rx;
                    self.graphics.pending = 2;
                }
                Err(e) => tracing::error!("{e}"),
            }
        }
    }

    fn poll_gpu(&mut self, block: bool) {
        let mut wait = block && self.graphics.hw.is_none() && self.graphics.sw.is_none();
        while self.graphics.pending > 0 {
            let msg = if wait { self.graphics.replies.recv().ok() } else { self.graphics.replies.try_recv().ok() };
            let Some((software, result)) = msg else { break };
            self.graphics.pending -= 1;
            match result {
                Ok(d) if software => self.graphics.sw = Some(d),
                Ok(d) => self.graphics.hw = Some(d),
                Err(e) => tracing::warn!("graphics device (software: {software}): {e}"),
            }
            wait = wait && self.graphics.hw.is_none() && self.graphics.sw.is_none();
        }
    }

    fn new_app(&mut self, routes: Vec<Route>, persisted: Persisted) -> App {
        let base = self.next_base;
        self.next_base += TAB_STRIDE;
        let mut app = App::new(persisted, routes, base, P::dark_mode(), 1.0, (1100, 720), Instant::now());
        app.utc_offset_min = P::utc_offset_minutes();
        app.double_click = P::double_click_time();
        app.wheel_lines = P::wheel_lines();
        app.set_caps(P::caps());
        app.set_tray_available(tray_controls_available(self.tray.as_ref()));
        app.set_shell_overlays(P::Shell::ICON_OVERLAYS);
        flit_core::i18n::set_unix_terms(app.caps.unix_paths);
        if base == 0 && P::elevated() {
            app.warn_elevated();
        }
        app.set_nav_state(self.saved.nav.clone());
        app.set_nav_prefs(self.saved.side.clone());
        app.set_pane_state(self.saved.pane);
        app.set_search_history(self.saved.tools.searches.clone());
        app.share_command_usage(self.command_usage.clone());
        app.set_places(self.places.current());
        app.share_typed(self.typed.clone());
        app.set_tags(self.tags.current());
        app.set_date_patterns(self.dates.0.clone(), self.dates.1.clone());
        app.about = self.about.clone();
        app.set_layouts(crate::layouts::dir(P::state_dir()).map(|d| crate::layouts::list(&d)).unwrap_or_default());
        if !self.settings.values.is_empty() {
            let all: Vec<(usize, Value)> = self.settings.values.iter().cloned().enumerate().collect();
            app.apply_settings(&all);
        }
        set_remember_recent(app.keeps_recents());
        let fx: Vec<Effect> = app.drain_effects().collect();
        for e in fx {
            match e {
                Effect::Provider { provider, req } => self.submit(provider, req),
                Effect::Git(req) => self.git.submit(req),
                _ => {}
            }
        }
        app
    }

    fn restored_app(&mut self, ws: &WindowSession) -> App {
        let cur = ws.cur.min(ws.tabs.len().saturating_sub(1));
        let routes = ws.tabs.get(cur).map(|l| vec![Route { folder: l.clone(), select: None }]).unwrap_or_default();
        let mut app = self.new_app(routes, self.saved.persisted.clone());
        app.restore_tabs(&ws.tabs, cur);
        app.restore_trails(&ws.trails);
        if let Some(p) = &ws.panels {
            app.restore_panels(p);
        }
        app
    }

    fn startup_app(&mut self, open: &Open, restore: StartupRestore) -> (App, Option<Placement>) {
        let startup = &self.saved.persisted.startup;
        let unix = P::caps().unix_paths;
        let continue_with_open = unix && matches!(startup, Startup::Continue);
        let allowed = restore.allowed && (open.is_empty() || continue_with_open);
        let mut windows = session::startup_windows(startup, restore.previous.as_ref(), allowed, update::restarted());
        let (routes, errors) = if open.is_empty() { Default::default() } else { open::routes_for(open, &P::known_folder, unix) };
        if windows.len() > 1 && open.target == OpenTarget::Default {
            session::open_requested_window_last(&mut windows, &routes);
        }
        let (mut app, placement) = match windows.pop_front().filter(|(w, _)| !w.tabs.is_empty()) {
            Some((ws, placement)) => {
                if !open.is_empty() {
                    self.startup_open = Some(open.clone());
                }
                (self.restored_app(&ws), placement)
            }
            None => {
                let mut app = self.new_app(routes, self.saved.persisted.clone());
                app.show_open_errors(&errors);
                (app, if open.target == OpenTarget::NewWindow { None } else { self.saved.placement() })
            }
        };
        self.queued_windows = windows;
        match restore.dump {
            Some(d) => app.offer_crash_report(self.crashed.is_some(), d),
            None if self.crashed.is_some() => app.offer_restore(),
            None => {}
        }
        (app, placement)
    }

    fn open_next(&mut self, window: &Win<P>) {
        let Some((ws, placement)) = self.queued_windows.pop_front() else { return };
        self.requested = Some(Instant::now());
        let app = self.restored_app(&ws);
        let server_decorations = app.system_title_wanted();
        self.pending.push_back(app);
        window.open_window(WindowOptions { title: "Flit".into(), placement, dark: window.is_dark_mode(), server_decorations });
    }

    fn tab_over(&mut self, window: &Win<P>, at: Option<(f32, f32)>) {
        let id = window.id();
        let target = at.and_then(|(x, y)| window.window_at(x, y)).filter(|(t, ..)| *t != id);
        let mut changed = false;
        for (w, st) in self.windows.iter_mut().filter(|(w, _)| **w != id) {
            changed |= st.app.tab_incoming(target.filter(|(t, ..)| t == w).map(|(_, x, y)| (x, y)));
        }
        if changed {
            (self.wake)();
        }
    }

    fn move_tab(&mut self, window: &Win<P>, tab: MovedTab, at: Option<(f32, f32)>, last: bool) {
        self.tab_over(window, None);
        let (id, now) = (window.id(), Instant::now());
        let target = at.and_then(|(x, y)| window.window_at(x, y)).filter(|(t, ..)| *t != id && self.windows.contains_key(t));
        if let Some((t, x, y)) = target {
            if let Some(st) = self.windows.get_mut(&t) {
                st.app.insert_tab(tab, Some((x, y)), now);
            }
            self.raise = Some(t);
            if last && !self.windows.get(&id).is_some_and(|s| s.app.ops_pending()) {
                window.close();
            }
            return (self.wake)();
        }
        if last {
            if let Some(st) = self.windows.get_mut(&id) {
                st.app.adopt_tab(tab, now);
            }
            return;
        }
        let placement = at.map(|(x, y)| {
            let ((sx, sy), p, s) = (window.to_screen(x, y), window.placement(), window.scale());
            Placement { x: sx - (TORN_GRAB.0 * s) as i32, y: sy - (TORN_GRAB.1 * s) as i32, width: p.width, height: p.height, maximized: false }
        });
        self.requested = Some(now);
        if let Some(st) = self.windows.get(&id) {
            self.saved.side = st.app.nav_prefs();
            self.saved.tools = tools(&st.app);
        }
        let mut app = self.new_app(vec![Route { folder: tab.location().clone(), select: None }], self.saved.persisted.clone());
        app.adopt_tab(tab, now);
        let server_decorations = app.system_title_wanted();
        self.pending.push_back(app);
        window.open_window(WindowOptions { title: "Flit".into(), placement, dark: window.is_dark_mode(), server_decorations });
    }

    fn session(&self) -> Session {
        let mut ids: Vec<WindowId> = self.windows.keys().copied().collect();
        ids.sort_unstable();
        let windows = ids.iter().map(|id| (self.windows[id].app.session(), self.windows[id].placement)).collect();
        Session::new(windows, self.typed.lock().unwrap_or_else(|e| e.into_inner()).clone())
    }

    fn note_session(&mut self, window: &Win<P>) {
        if !self.queued_windows.is_empty() || !self.pending.is_empty() {
            return;
        }
        let Some(st) = self.windows.get_mut(&window.id()).filter(|_| self.writer.is_some() && !self.ending && self.quit_session.is_none()) else { return };
        let rev = st.app.session_rev();
        if rev == st.session_rev {
            return;
        }
        st.session_rev = rev;
        st.placement = Some(window.placement());
        let s = self.session();
        if let Some(w) = &self.writer {
            w.send(s);
        }
    }

    fn final_session(&mut self, s: Option<Session>) {
        if let Some(w) = &mut self.writer {
            w.finish();
        }
        self.writer = None;
        self.kept.finish();
        if let Some(f) = self.files.take().filter(|_| self.background_startup.is_none()) {
            f.finish(s.as_ref());
        }
    }

    fn kept_icon(&mut self, req: &IconRequest, image: flit_core::ui::Bitmap) {
        self.kept.keep(req, &image);
        for st in self.windows.values_mut() {
            st.app.on_icon(&mut st.gfx, req.key, IconResult::Ready(image.clone()));
        }
    }

    fn submit_admin(&mut self, req: ProviderRequest) {
        if self.admin.is_none() {
            let timing = Timing { hello: ADMIN_PASSWORD_WAIT, idle: ADMIN_IDLE, max_crashes: usize::MAX, ..Timing::default() };
            match WslProvider::admin(P::admin_launcher(), self.local.clone(), self.events_tx.clone(), self.wake.clone(), timing, Clipboard::of::<P::Shell>()) {
                Ok(p) => self.admin = Some(p),
                Err(e) => tracing::warn!("{e}"),
            }
        }
        match &self.admin {
            Some(p) => p.submit(req),
            None => tracing::warn!("the administrator provider is not running"),
        }
    }

    fn submit(&mut self, provider: ProviderId, req: ProviderRequest) {
        let distro = match provider {
            ProviderId::Wsl(distro) => distro,
            ProviderId::Admin => return self.submit_admin(req),
            ProviderId::Local => return self.local.submit(req),
        };
        let known = self.distros.get_or_insert_with(P::wsl_distros);
        let distro = known.iter().find(|d| d.eq_ignore_ascii_case(&distro)).cloned().unwrap_or(distro);
        if !self.wsl.contains_key(&distro) {
            let launcher = P::wsl_launcher(&distro, &agents::AGENTS);
            match WslProvider::new(distro.clone(), launcher, self.local.clone(), self.events_tx.clone(), self.wake.clone(), Timing::default(), Clipboard::of::<P::Shell>()) {
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

    fn layout_cmd(&mut self, id: WindowId, cmd: flit_core::ui::LayoutCmd) {
        use flit_core::ui::LayoutCmd;
        let Some(dir) = crate::layouts::dir(P::state_dir()) else { return };
        let written = match &cmd {
            LayoutCmd::Save(name, layout) => crate::layouts::save(&dir, name, layout),
            LayoutCmd::Delete(name) => crate::layouts::delete(&dir, name),
            LayoutCmd::Load(name) => {
                let layout = crate::layouts::load(&dir, name);
                if let Some(st) = self.windows.get_mut(&id) {
                    match layout {
                        Some(l) => st.app.apply_layout(&l, Instant::now()),
                        None => st.app.status(flit_core::tr!(S::MenusLayoutMissing, name = name)),
                    }
                }
                return;
            }
        };
        if let Err(e) = written {
            tracing::warn!("layout file: {e}");
        }
        let names = crate::layouts::list(&dir);
        for st in self.windows.values_mut() {
            st.app.set_layouts(names.clone());
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
        let mut folders = Vec::new();
        for e in fx {
            if self.picker_result.is_some() && matches!(e, Effect::NewWindow(_) | Effect::NewNetworkWindow(_) | Effect::Settings(_) | Effect::Update(_) | Effect::ShellMenu(_) | Effect::RestoreSession | Effect::StartDrag(_)) {
                continue;
            }
            match e {
                Effect::Icon(req) if batched::<P::Shell>(&req) => folders.push(req),
                Effect::Provider { provider, req } => {
                    if self.picker_result.is_none() || matches!(req, ProviderRequest::List { .. } | ProviderRequest::Watch { .. } | ProviderRequest::DirSizes { .. }) {
                        self.submit(provider, req);
                    }
                }
                Effect::Picked { paths, filter, choices } => {
                    if let Some(result) = &self.picker_result {
                        *result.lock().unwrap_or_else(|e| e.into_inner()) = Some((paths, filter, choices));
                        window.close();
                    }
                }
                Effect::Resort { tab, gen, snap, view } => {
                    let _ = self.sorter.send(SortJob::Rows { window: id, tab, gen, snap, view });
                }
                Effect::ViewPictures { tab, ask, snap, cancel } => {
                    let _ = self.sorter.send(SortJob::Pictures { window: id, tab, ask, snap, cancel });
                }
                Effect::Icon(req) => {
                    let visible = self.windows[&id].visible.clone();
                    let _ = self.shell.send(ShellJob::Icon { window: id, req, visible });
                }
                Effect::Thumbnail(req) => {
                    let visible = self.windows[&id].visible.clone();
                    let _ = self.thumbs.send(ThumbJob { window: id, req, visible });
                }
                Effect::Overlays(batch) => {
                    let shown = self.windows[&id].visible.overlays.clone();
                    self.overlays.send(crate::overlays::Job { window: id, batch, shown });
                }
                Effect::ClearThumbnails => {
                    if let Some(c) = &self.thumb_cache {
                        locked(c).clear();
                    }
                }
                Effect::TypeName(ext) => {
                    let _ = self.shell.send(ShellJob::TypeName(ext));
                }
                Effect::LinkChoices { generation, items, dest } => {
                    let _ = self.shell.send(ShellJob::LinkChoices { window: id, generation, items, dest });
                }
                Effect::SetClipboard(text) if !text.is_empty() => window.set_clipboard_text(&text),
                Effect::SetClipboard(_) => {}
                Effect::SetClipboardImage(image) => window.set_clipboard_image(&image),
                Effect::Paste => {
                    if let (Some(text), Some(st)) = (window.clipboard_text(), self.windows.get_mut(&id)) {
                        st.app.on_paste(&text);
                    }
                }
                Effect::SetPrimary(text) => window.set_primary_text(&text),
                Effect::PastePrimary(paste) => {
                    if let (Some(text), Some(st)) = (window.primary_text(paste), self.windows.get_mut(&id)) {
                        st.app.on_primary_paste(paste, &text);
                    }
                }
                Effect::Window(WindowCmd::Minimize) => window.minimize(),
                Effect::Window(WindowCmd::ToggleMaximize) => window.toggle_maximize(),
                Effect::Window(WindowCmd::Fullscreen) => window.toggle_fullscreen(),
                Effect::Window(WindowCmd::Compact(on)) => window.set_compact(on),
                Effect::Window(WindowCmd::Attention) => window.request_attention(),
                Effect::Window(WindowCmd::ServerDecorations(on)) => window.set_server_decorations(on),
                Effect::Notify(text) => window.notify(&flit_core::redact::passwords(&text)),
                Effect::MoveTab { tab, at, last } => self.move_tab(window, tab, at, last),
                Effect::TabOver(at) => self.tab_over(window, at),
                Effect::RestoreSession => {
                    if let Some(s) = self.crashed.take() {
                        self.queued_windows.extend(s.windows());
                        self.open_next(window);
                    }
                }
                Effect::Window(WindowCmd::Quit) => {
                    let snapshot = self.session();
                    if let Some(w) = self.writer.as_ref().filter(|_| cfg!(not(windows))) {
                        w.flush(snapshot.clone());
                    }
                    self.quit_session = Some(snapshot);
                    self.quitting = self.windows.keys().filter(|&&w| w != id).copied().collect();
                    (self.wake)();
                    if let Some(st) = self.windows.get(&id) {
                        self.saved = Saved::new(st.app.persisted(), Some(window.placement())).with_nav(st.app.nav_state()).with_pane(st.app.pane_state()).with_side(st.app.nav_prefs()).with_tools(tools(&st.app));
                    }
                    window.close();
                }
                Effect::Window(WindowCmd::Close) => {
                    if let Some(w) = self.writer.as_ref().filter(|_| cfg!(not(windows)) && self.quit_session.is_none()) {
                        if self.windows.len() <= 1 {
                            w.flush(self.session())
                        } else {
                            w.send(self.session())
                        }
                    }
                    if let Some(st) = self.windows.get(&id) {
                        self.saved = Saved::new(st.app.persisted(), Some(window.placement())).with_nav(st.app.nav_state()).with_pane(st.app.pane_state()).with_side(st.app.nav_prefs()).with_tools(tools(&st.app));
                    }
                    window.close();
                }
                Effect::Window(WindowCmd::KeepOpen) => {
                    if self.quit_session.take().is_some() {
                        if let Some(w) = &self.writer {
                            w.send(self.session());
                        }
                    }
                }
                Effect::Cursor(c) => window.set_cursor(c),
                Effect::Title(t) => {
                    window.set_title(&t);
                    if let Some(st) = self.windows.get(&id) {
                        window.set_folder(st.app.current_location().to_windows().as_ref());
                        let lists = jumps::lists(&st.app.nav_state(), &st.app.nav_prefs());
                        self.jumps.show(lists);
                    }
                }
                Effect::Open(cmd) => {
                    if let Some(st) = self.windows.get_mut(&id) {
                        st.app.open(&cmd, &P::known_folder, Instant::now());
                    }
                }
                Effect::Search(req) => {
                    self.search.enable_metadata(self.windows.get(&id).is_some_and(|st| st.app.metadata_enabled()));
                    self.search.submit(id, req);
                }
                Effect::Preview(req) => {
                    let _ = self.previews.send(PreviewJob::Load { window: id, req });
                }
                Effect::PrettyPreview(req) => {
                    let _ = self.previews.send(PreviewJob::Pretty { window: id, req });
                }
                Effect::PreviewPdfPage { req, page } => {
                    let _ = self.previews.send(PreviewJob::PdfPage { window: id, req, page });
                }
                Effect::QuickView(path) => {
                    let _ = self.previews.send(PreviewJob::QuickView { window: id, path });
                }
                Effect::Backdrop { gen, path } => {
                    let _ = self.previews.send(PreviewJob::Backdrop { window: id, gen, path });
                }
                Effect::Host(cmd) => window.host(cmd),
                effect @ (Effect::NewWindow(_) | Effect::NewNetworkWindow(_)) => {
                    let connect = matches!(effect, Effect::NewNetworkWindow(_));
                    let (Effect::NewWindow(routes) | Effect::NewNetworkWindow(routes)) = effect else { unreachable!() };
                    self.requested = Some(Instant::now());
                    if let Some(st) = self.windows.get(&id) {
                        self.saved.side = st.app.nav_prefs();
                        self.saved.tools = tools(&st.app);
                    }
                    let persisted = self.saved.persisted.clone();
                    let mut app = self.new_app(routes, persisted);
                    if connect {
                        app.activate_network_tabs();
                    }
                    let server_decorations = app.system_title_wanted();
                    self.pending.push_back(app);
                    window.open_window(WindowOptions { title: "Flit".into(), placement: None, dark: window.is_dark_mode(), server_decorations });
                }
                Effect::SetClipboardFiles { paths, cut } => window.set_clipboard_files(&paths, cut),
                Effect::ShellMenu(menu) if !menu.shell && menu.items.is_empty() => {
                    if let Some(st) = self.windows.get_mut(&id) {
                        st.app.on_event(&Event::ShowMenu { items: menu.menu, x: menu.x, y: menu.y, launches: Vec::new(), kind: menu.kind, shell: Vec::new(), pending: false }, Instant::now());
                    }
                    window.request_redraw();
                }
                Effect::ShellMenu(menu) => window.shell_menu(menu),
                Effect::ShellPick(cmd) => window.shell_pick(cmd),
                Effect::PaletteShell { ticket, menu } => {
                    if !flit_platform::isolated_gui_test() {
                        window.palette_shell(ticket, menu);
                    }
                }
                Effect::PaletteShellPick { ticket, command } => window.palette_shell_pick(ticket, command),
                Effect::ShellPrefetch(menu) => {
                    if !flit_platform::isolated_gui_test() {
                        window.shell_prefetch(menu);
                    }
                }
                Effect::ShellWarm(menus) => {
                    if !flit_platform::isolated_gui_test() {
                        window.shell_warm(menus);
                    }
                }
                Effect::Layout(cmd) => self.layout_cmd(id, cmd),
                Effect::Properties(paths) => window.properties(paths),
                Effect::Share(paths) => window.share(paths),
                Effect::SetVolumeLabel { drive, label } => window.set_volume_label(drive, label),
                Effect::StartDrag(paths) => window.start_drag(paths),
                Effect::ShellProps { tab, gen, ask, dir, names, shell, portable } => {
                    let _ = self.previews.send(PreviewJob::ColumnProps { window: id, tab, gen, ask, dir, names, system: shell, portable });
                }
                Effect::PortableMetadata { id: request, req } => self.settings.send(Job::Portable(id, request, req)),
                Effect::DiskChanges { id: request, request: req } => self.settings.send(Job::DiskChanges(id, request, req)),
                effect @ (Effect::Pinned(_) | Effect::PinnedCleared | Effect::PinnedFiles(_)) => {
                    let wake = !matches!(&effect, Effect::PinnedFiles(_));
                    let windows = self.windows.iter_mut().filter(|(w, _)| **w != id).map(|(_, st)| &mut st.app);
                    apply_bookmark_effect(effect, &mut self.saved.nav, windows, &mut self.pending, |old, new| self.quick.pinned(old, new), P::bookmarks_changed);
                    self.sync_tray();
                    if wake {
                        (self.wake)();
                    }
                }
                Effect::RefreshPlaces => self.places.refresh(),
                Effect::NetworkDiscovery { tab, gen } => {
                    let (events, wake) = (self.events_tx.clone(), self.wake.clone());
                    if let Err(error) = spawn("flit-network-discovery", move || {
                        let _ = events.send(ProviderEvent::NetworkDiscovery { tab, gen, warning: P::network_warning() });
                        wake();
                    }) {
                        tracing::warn!("cannot check network discovery: {error}");
                    }
                }
                Effect::SavedServer(change) => {
                    change.apply(&mut self.saved.side.saved_servers);
                    for app in self.windows.iter_mut().filter(|(w, _)| **w != id).map(|(_, st)| &mut st.app).chain(self.pending.iter_mut()) {
                        app.apply_server_change(&change);
                    }
                    (self.wake)();
                }
                Effect::PinFolders(paths) => self.places.pin(paths),
                Effect::CheckPlaces(paths) => self.places.check(paths),
                Effect::Library(op) => self.places.library(op),
                Effect::Timer(d) => window.set_timer(d.as_millis().clamp(1, u32::MAX as u128) as u32),
                Effect::Tags(req) => self.tags.submit(Some(id), req),
                Effect::Git(req) => self.git.submit(req),
                Effect::Settings(SettingsCmd::Export { path, text, replace }) => self.settings.send(Job::Export(crate::preview::std_path(&path), text, replace)),
                Effect::Settings(SettingsCmd::Import(path)) => self.settings.send(Job::Import(id, crate::preview::std_path(&path))),
                Effect::Settings(SettingsCmd::ConfirmImport(plan)) => {
                    if self.settings.read_only {
                        self.status(id, S::SettingsPageImportNewer.t().to_owned());
                    } else {
                        let text = plan.backup(&self.settings.unknown);
                        self.settings.send(Job::BackupImport(id, plan, text));
                    }
                }
                Effect::Settings(SettingsCmd::CacheSize) => self.settings.send(Job::CacheSize),
                Effect::Settings(SettingsCmd::RaiseWatchLimit) => self.settings.send(Job::RaiseWatchLimit(P::raise_watch_limit)),
                Effect::Settings(SettingsCmd::RefreshSystemChanges) => self.settings.send(Job::SystemChanges(crate::system_changes::Action::Refresh)),
                Effect::Settings(SettingsCmd::UndoSystemChanges) => self.settings.send(Job::SystemChanges(crate::system_changes::Action::Undo)),
                Effect::Settings(SettingsCmd::RegisterArchive) => self.settings.send(Job::SystemChanges(crate::system_changes::Action::RegisterArchive { startup: false })),
                Effect::Settings(SettingsCmd::ArchiveOfferShown) => self.settings.send(Job::SystemChanges(crate::system_changes::Action::ArchiveOfferShown)),
                Effect::Settings(SettingsCmd::Fonts) => {
                    if let Some(st) = self.windows.get_mut(&id) {
                        let t = Instant::now();
                        let fonts = st.gfx.text.families();
                        tracing::debug!("{} font families in {:.2} ms", fonts.len(), t.elapsed().as_secs_f64() * 1e3);
                        st.app.set_fonts(fonts);
                    }
                }
                Effect::Settings(SettingsCmd::Browse { exts, folder, start }) => {
                    let _ = self.previews.send(PreviewJob::Browse { window: id, exts, folder, start });
                }
                Effect::Update(req) => self.update_request(id, req),
            }
        }
        if let Some(st) = self.windows.get(&id).filter(|_| !folders.is_empty()) {
            for reqs in folder_batches(folders) {
                let _ = self.shell.send(ShellJob::Icons { window: id, reqs, visible: st.visible.clone() });
            }
        }
        self.sync_settings(id);
        if self.windows.get(&id).is_some_and(|st| st.app.has_effects()) {
            return self.flush(window);
        }
        if let Some(st) = self.windows.get_mut(&id) {
            if let Some(area) = st.app.ime_area() {
                window.set_ime_area(area);
            }
            let key = st.app.selection_key();
            if st.selection_key != Some(key) && window.set_selection(&|| st.app.selection_source()) {
                st.selection_key = Some(key);
            }
            if st.app.needs_redraw() {
                window.request_redraw();
            }
        }
        self.note_session(window);
        self.stay_awake();
        self.taskbar_progress(window);
    }

    fn stay_awake(&mut self) {
        let busy = self.windows.values().any(|st| st.app.keeps_awake());
        self.awake.track(busy, || P::keep_awake(S::UiAwakeReason.t()));
    }

    fn taskbar_progress(&mut self, window: &Win<P>) {
        let Some(st) = self.windows.get_mut(&window.id()) else { return };
        let now = st.app.overall_progress().map(|(f, waiting)| (((f * 100.0).round() as u32).min(100), waiting));
        if st.progress != now {
            st.progress = now;
            window.set_taskbar_progress(now);
        }
    }

    fn update_request(&mut self, id: WindowId, req: UpdateRequest) {
        let busy_elsewhere = matches!(req, UpdateRequest::Install(_)) && self.windows.iter().any(|(w, st)| *w != id && st.app.file_jobs_running() > 0);
        if busy_elsewhere {
            if let Some(st) = self.windows.get_mut(&id) {
                st.app.on_update(UpdateEvent::Failed(S::UpdateErrorOtherWindow.t().to_owned()));
            }
            return;
        }
        self.update_window = Some(id);
        self.updates.submit(req);
    }

    fn update_replies(&mut self) {
        while let Ok(reply) = self.updates.events.try_recv() {
            let ev = match reply {
                update::Reply::Ui(ev) => ev,
                update::Reply::Restart => {
                    let s = self.session();
                    self.final_session(Some(s));
                    self.ending = true;
                    self.restarting = true;
                    continue;
                }
            };
            let target = self.update_window.filter(|w| self.windows.contains_key(w)).or(self.active).or_else(|| self.windows.keys().next().copied());
            match target.and_then(|w| self.windows.get_mut(&w)) {
                Some(st) => st.app.on_update(ev),
                None => {
                    if let Some(app) = self.pending.front_mut() {
                        app.on_update(ev);
                    }
                }
            }
        }
    }

    fn sync_settings(&mut self, id: WindowId) {
        let Some(st) = self.windows.get(&id) else { return };
        if self.settings.values.is_empty() {
            return;
        }
        set_remember_recent(st.app.keeps_recents());
        let values = st.app.setting_values();
        if values == self.settings.values {
            return;
        }
        let changed: Vec<(usize, Value)> = values.iter().enumerate().filter(|(i, v)| self.settings.values.get(*i) != Some(*v)).map(|(i, v)| (i, v.clone())).collect();
        for (_, st) in self.windows.iter_mut().filter(|(w, _)| **w != id) {
            st.app.apply_settings(&changed);
        }
        for app in &mut self.pending {
            app.apply_settings(&changed);
        }
        self.settings.set_values(values);
        self.follow_quick_sync();
        self.sync_tray();
        self.settings.save();
        (self.wake)();
    }

    fn settings_replies(&mut self, window: &Win<P>) {
        let id = window.id();
        while let Ok(reply) = self.settings.replies.try_recv() {
            match reply {
                #[cfg(target_os = "linux")]
                settings::Reply::TrayAvailable(available) => {
                    if let Some(tray) = self.tray.as_ref().filter(|tray| tray.menu().is_none()) {
                        tray.set_available(available);
                    }
                }
                settings::Reply::System { os, dates, missing } => {
                    self.about.os = os;
                    self.about.missing = missing;
                    self.share_about();
                    for app in self.windows.values_mut().map(|st| &mut st.app).chain(self.pending.iter_mut()) {
                        app.set_date_patterns(dates.0.clone(), dates.1.clone());
                    }
                    self.dates = dates;
                }
                settings::Reply::ResolvedLink(target, link) => {
                    if let Some(st) = self.windows.get_mut(&target) {
                        crate::system_link::open_in(&mut st.app, link);
                    }
                }
                settings::Reply::WatchLimit(outcome) => {
                    let now = Instant::now();
                    for app in self.windows.values_mut().map(|st| &mut st.app) {
                        app.watch_limit_raised(&outcome, now);
                    }
                }
                settings::Reply::CacheUsed(bytes) => {
                    self.about.cache_bytes = Some(bytes);
                    self.share_about();
                }
                settings::Reply::Changed(text) => self.reload_settings(id, &text),
                settings::Reply::SystemChangesUnavailable { notify } => {
                    self.about.system_integration_unavailable = true;
                    self.share_about();
                    if notify {
                        self.status(id, S::SettingsPageSystemUnavailable.t().to_owned());
                    }
                }
                settings::Reply::SystemChanges(reply) => {
                    if reply.disabled_autostart {
                        self.clear_system_settings(&[flit_core::ui::settings::AUTOSTART]);
                    }
                    if reply.disabled_protocol {
                        self.clear_system_settings(&[flit_core::ui::settings::PROTOCOL_LINKS]);
                    }
                    if reply.disabled_portal {
                        self.clear_system_settings(&[flit_core::ui::settings::PORTAL_PICKER]);
                    }
                    self.about.system_changes = Some(reply.changes);
                    self.share_about();
                    if let Err(error) = reply.result {
                        self.status(id, flit_core::tr!(S::SettingsPageSystemWriteFailed, error = error));
                    } else if reply.undone {
                        self.status(id, S::SettingsPageSystemUndone.t().to_owned());
                    } else if reply.archive_registered {
                        self.status(id, S::SettingsPageSystemRegistered.t().to_owned());
                    } else if reply.offer_archives {
                        if let Some(st) = self.windows.get_mut(&id) {
                            st.app.offer_archive_registration();
                        }
                    }
                }
                #[cfg(target_os = "linux")]
                settings::Reply::Portal(on, result) => match result {
                    Ok(Some(true)) => self.status(id, S::PickPortalRestart.t().into()),
                    Ok(Some(false)) => {}
                    Ok(None) => self.clear_system_settings(&[flit_core::ui::settings::PORTAL_PICKER]),
                    Err(error) => {
                        tracing::warn!("file chooser portal configuration: {error}");
                        self.status(id, flit_core::tr!(S::PickPortalFailed, error = flit_core::i18n::localize(&error)));
                        if let Some(k) = flit_core::ui::settings::index(flit_core::ui::settings::PORTAL_PICKER) {
                            if self.settings.values.get(k) == Some(&Value::Bool(on)) {
                                let back = [(k, Value::Bool(!on))];
                                for app in self.windows.values_mut().map(|st| &mut st.app).chain(self.pending.iter_mut()) {
                                    app.apply_settings(&back);
                                }
                                self.settings.values[k] = Value::Bool(!on);
                                self.settings.save();
                            }
                        }
                    }
                },
                settings::Reply::SaveFailed(e) => {
                    tracing::warn!("cannot save the settings: {e}");
                    self.status(id, flit_core::tr!(S::ErrorsSettingsWriteFailed, error = flit_core::i18n::localize(&e)));
                }
                settings::Reply::IntegrationFailed(on, error) => {
                    self.status(id, flit_core::tr!(S::UiAppIntegrationFailed, error = flit_core::i18n::localize(&error)));
                    let Some(k) = flit_core::ui::settings::index(flit_core::ui::settings::DEFAULT_MANAGER) else { continue };
                    let back = [(k, Value::Bool(!on))];
                    for app in self.windows.values_mut().map(|st| &mut st.app).chain(self.pending.iter_mut()) {
                        app.apply_settings(&back);
                    }
                    if let Some(v) = self.settings.values.get_mut(k) {
                        *v = Value::Bool(!on);
                    }
                    self.settings.save();
                }
                settings::Reply::Imported(target, Ok(doc)) => {
                    if let Some(st) = self.windows.get_mut(&target) {
                        st.app.preview_settings_import(doc);
                    }
                }
                settings::Reply::Imported(target, Err(e)) | settings::Reply::ImportBackedUp(target, _, Err(e)) => {
                    tracing::warn!("cannot import settings: {e}");
                    self.status(target, flit_core::tr!(S::UiAppImportFailed, error = flit_core::i18n::localize(&e)));
                }
                settings::Reply::ImportBackedUp(target, plan, Ok(path)) => {
                    if self.settings.read_only {
                        self.status(target, S::SettingsPageImportNewer.t().to_owned());
                    } else if let Some(st) = self.windows.get_mut(&target) {
                        st.app.apply_confirmed_settings_import(*plan, path.to_string_lossy().into_owned());
                        self.sync_settings(target);
                    }
                }
                settings::Reply::Portable(target, request, result) => {
                    if let Some(st) = self.windows.get_mut(&target) {
                        st.app.on_portable_metadata(request, result);
                    }
                }
                settings::Reply::DiskChanges(target, request, result) => {
                    if let Some(st) = self.windows.get_mut(&target) {
                        st.app.on_disk_changes(request, result);
                    }
                }
                settings::Reply::Exported(Ok(path)) => self.status(id, flit_core::tr!(S::UiAppExported, path = path.display())),
                settings::Reply::Exported(Err(e)) => {
                    tracing::warn!("cannot export settings: {e}");
                    self.status(id, flit_core::tr!(S::UiAppExportFailed, error = flit_core::i18n::localize(&e.to_string())));
                }
                settings::Reply::ExportExists { path, size, modified } => {
                    if let (Some(st), Some(path)) = (self.windows.get_mut(&id), FsPath::parse(&path.to_string_lossy())) {
                        st.app.ask_replace_export(path, size, modified);
                    }
                }
            }
        }
    }

    fn clear_system_settings(&mut self, keys: &[&str]) {
        let changes: Vec<_> = keys
            .iter()
            .filter_map(|key| {
                let k = flit_core::ui::settings::index(key)?;
                (self.settings.values.get(k) == Some(&Value::Bool(true))).then_some((k, Value::Bool(false)))
            })
            .collect();
        if changes.is_empty() {
            return;
        }
        for (k, value) in &changes {
            self.settings.values[*k] = value.clone();
        }
        for app in self.windows.values_mut().map(|st| &mut st.app).chain(self.pending.iter_mut()) {
            app.apply_settings(&changes);
        }
        self.settings.save();
    }

    fn share_about(&mut self) {
        for st in self.windows.values_mut() {
            st.app.set_about(self.about.clone());
        }
        for app in &mut self.pending {
            app.set_about(self.about.clone());
        }
    }

    fn status(&mut self, id: WindowId, text: String) {
        if let Some(st) = self.windows.get_mut(&id) {
            st.app.status(text);
        }
    }

    fn follow_quick_sync(&mut self) {
        let wanted = settings::flag(&self.settings.values, flit_core::ui::settings::QUICK_ACCESS_SYNC) == Some(true);
        self.quick.follow(wanted, &self.saved.nav.pinned);
    }

    fn reload_settings(&mut self, id: WindowId, text: &str) {
        let doc = flit_core::ui::settings::parse(text);
        let Some(st) = self.windows.get_mut(&id) else { return };
        st.app.apply_doc(&doc);
        set_remember_recent(st.app.keeps_recents());
        let values = st.app.setting_values();
        let all: Vec<(usize, Value)> = values.iter().cloned().enumerate().collect();
        for app in self.windows.iter_mut().filter(|(w, _)| **w != id).map(|(_, st)| &mut st.app).chain(self.pending.iter_mut()) {
            app.apply_settings(&all);
        }
        self.settings.adopt(&doc, values);
        self.follow_quick_sync();
        self.sync_tray();
        (self.wake)();
    }

    fn drain(&mut self) {
        let now = Instant::now();
        while let Ok(ev) = self.events.try_recv() {
            match &ev {
                ProviderEvent::OpDone { error: Some(e), .. } => tracing::warn!("operation failed: {}", flit_core::redact::passwords(&e.to_string())),
                ProviderEvent::Hashed { result: Err(e), .. } => tracing::warn!("hashing failed: {}", flit_core::redact::passwords(e)),
                ProviderEvent::Denied { error, .. } => tracing::warn!("{}", flit_core::redact::passwords(error)),
                _ => {}
            }
            let target = match &ev {
                ProviderEvent::NetworkDiscovery { tab, .. } => self.window_of_tab(*tab),
                ProviderEvent::Changes { tab, .. } | ProviderEvent::Rescan { tab, .. } | ProviderEvent::DirSize { tab, .. } | ProviderEvent::SizesSkipped { tab, .. } => self.window_of_tab(*tab),
                ProviderEvent::OpDone { tab, .. } | ProviderEvent::Untrashable { tab, .. } | ProviderEvent::Denied { tab, .. } => self.window_of_tab(*tab).or(self.active),
                ProviderEvent::Navigate { tab, .. }
                | ProviderEvent::SavedServerConnected { tab, .. }
                | ProviderEvent::WrongPassphrase { tab, .. }
                | ProviderEvent::NetworkConnected { tab, .. }
                | ProviderEvent::Reveal { tab, .. }
                | ProviderEvent::OpProgress { tab, .. }
                | ProviderEvent::OpConflict { tab, .. }
                | ProviderEvent::OpConflicts { tab, .. }
                | ProviderEvent::OpFailure { tab, .. }
                | ProviderEvent::Hashed { tab, .. }
                | ProviderEvent::Measured { tab, .. }
                | ProviderEvent::Status { tab, .. }
                | ProviderEvent::OpNotice { tab, .. }
                | ProviderEvent::Access { tab, .. }
                | ProviderEvent::NotExecutable { tab, .. }
                | ProviderEvent::OpenAsk { tab, .. }
                | ProviderEvent::Unwatched { tab, .. }
                | ProviderEvent::MountAsk { tab, .. }
                | ProviderEvent::MountAskDone { tab, .. } => self.window_of_tab(*tab),
            };
            if let Some(st) = target.and_then(|id| self.windows.get_mut(&id)) {
                st.app.on_provider_event(ev, now);
            }
        }
        while let Ok(done) = self.done.try_recv() {
            match done {
                Done::Icon { req, result: IconResult::Ready(image), .. } if worth_keeping(&req) => self.kept_icon(&req, image),
                Done::Refreshed { req, image } => self.kept_icon(&req, image),
                Done::Icon { window, req, result } => {
                    if let Some(st) = self.windows.get_mut(&window) {
                        st.app.on_icon(&mut st.gfx, req.key, result);
                    }
                }
                Done::Thumb { window, key, result } => {
                    if let Some(st) = self.windows.get_mut(&window) {
                        st.app.on_thumb(&mut st.gfx, key, result);
                    }
                }
                Done::Overlays { window, reply } => {
                    if let Some(st) = self.windows.get_mut(&window) {
                        st.app.on_overlays(&mut st.gfx, reply);
                    }
                }
                Done::TypeName(ext, name) => {
                    for st in self.windows.values_mut() {
                        st.app.on_type_name(ext.clone(), name.clone());
                    }
                }
                Done::LinkChoices { window, generation, choices } => {
                    if let Some(st) = self.windows.get_mut(&window) {
                        st.app.on_event(&Event::LinkChoices { generation, choices }, Instant::now());
                    }
                }
                Done::Preview { window, gen, data } => {
                    if let Some(st) = self.windows.get_mut(&window) {
                        st.app.on_preview(gen, data);
                    }
                }
                Done::Pretty { window, gen, result } => {
                    if let Some(st) = self.windows.get_mut(&window) {
                        st.app.on_pretty(gen, result);
                    }
                }
                Done::PdfPage { window, gen, page, image } => {
                    if let Some(st) = self.windows.get_mut(&window) {
                        st.app.on_pdf_page(&mut st.gfx, gen, page, image);
                    }
                }
                Done::ColumnProps { window, tab, gen, ask, rows } => {
                    if let Some(st) = self.windows.get_mut(&window) {
                        st.app.on_shell_props(tab, gen, ask, rows);
                    }
                }
                Done::QuickView { window, shown } => {
                    if let Some(st) = self.windows.get_mut(&window) {
                        st.app.on_quick_view(shown);
                    }
                }
                Done::Backdrop { window, gen, image } => {
                    if let Some(st) = self.windows.get_mut(&window) {
                        st.app.on_backdrop(gen, image);
                    }
                }
                Done::Browsed { window, path } => {
                    if let Some(st) = self.windows.get_mut(&window) {
                        st.app.on_browsed(path);
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
                Done::PictureShare { window, tab, ask, share } => {
                    if let Some(st) = self.windows.get_mut(&window) {
                        st.app.on_picture_share(tab, ask, share);
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
        while let Ok(ev) = self.git.events.try_recv() {
            let base = ev.tab() / TAB_STRIDE * TAB_STRIDE;
            let owns = |app: &App| app.tabs().first().is_some_and(|t| t.id / TAB_STRIDE * TAB_STRIDE == base);
            let target = self.windows.values_mut().map(|st| &mut st.app).chain(self.pending.iter_mut()).find(|app| owns(app));
            if let Some(app) = target {
                app.on_git(ev);
            }
        }
        while let Ok((window, ev)) = self.search.events.try_recv() {
            if let Some(st) = self.windows.get_mut(&window) {
                st.app.on_search_event(ev, now);
            }
        }
        for change in self.quick.poll() {
            let Some(list) = flit_core::quick_sync::apply_incoming(&self.saved.nav.pinned, &change) else { continue };
            for app in self.windows.values_mut().map(|st| &mut st.app).chain(self.pending.iter_mut()) {
                app.set_pinned(list.clone());
            }
            self.saved.nav.pinned = list;
            self.sync_tray();
        }
        for reply in self.places.poll() {
            match reply {
                Reply::Places(places) => {
                    self.distros = Some(places.wsl.clone());
                    for st in self.windows.values_mut() {
                        st.app.set_places(places.clone());
                    }
                }
                Reply::Failed(message) => {
                    tracing::warn!("library: {message}");
                    if let Some(st) = self.active.and_then(|id| self.windows.get_mut(&id)) {
                        st.app.library_failed(&message);
                    }
                }
                Reply::Missing(gone) => {
                    for st in self.windows.values_mut() {
                        st.app.forget_missing(&gone);
                    }
                }
                Reply::Pin(folders, files) => {
                    if let Some(st) = self.active.and_then(|id| self.windows.get_mut(&id)) {
                        st.app.pin_entries(folders, files);
                    }
                }
            }
        }
    }

    fn take_opens(&mut self) {
        for o in self.opens.try_iter() {
            if crate::system_link::handles(&o) {
                self.queued_opens.push(o);
                continue;
            }
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
        if self.queued_opens.is_empty() || !self.windows.contains_key(&window.id()) {
            return;
        }
        let ids: Vec<WindowId> = self.windows.keys().copied().collect();
        let mine = open::take_for_window(&mut self.queued_opens, window.id(), self.active, &ids);
        let Some(st) = self.windows.get_mut(&window.id()).filter(|_| !mine.is_empty()) else { return };
        let now = Instant::now();
        for mut o in mine {
            if crate::system_link::handles(&o) {
                if st.app.protocol_links {
                    self.settings.send(Job::ResolveLink(window.id(), o));
                } else {
                    st.app.status(S::SettingsPageSystemLinkDisabled.t());
                }
                continue;
            }
            if o.is_empty() {
                // A bare Linux launcher activation raises the existing session.
                if P::caps().unix_paths && o.target == OpenTarget::Default {
                    continue;
                }
                o.paths.push(Location::from_local(P::home()).to_bytes());
            }
            st.app.open(&o, &P::known_folder, now);
        }
        window.activate();
        self.flush(window);
    }

    fn apply_startup_open(&mut self, window: &Win<P>) {
        if !self.queued_windows.is_empty() {
            return;
        }
        let Some(open) = self.startup_open.take() else { return };
        if open.target != OpenTarget::Default {
            self.queued_opens.push(open);
            return;
        }
        let (routes, errors) = open::routes_for(&open, &P::known_folder, true);
        let total = routes.len();
        let mut target = window.id();
        let mut ids: Vec<_> = self.windows.keys().copied().collect();
        ids.sort_unstable();
        let now = Instant::now();
        for route in routes.into_iter().take(flit_core::ui::MAX_ROUTES) {
            let found = std::iter::once(target).chain(ids.iter().copied().filter(|&id| id != target)).find(|id| self.windows.get_mut(id).is_some_and(|w| w.app.focus_folder(&route.folder)));
            if let Some(id) = found {
                target = id;
                if route.select.is_none() {
                    continue;
                }
            }
            let mode = if found.is_some() { OpenTarget::CurrentTab } else { OpenTarget::NewTab };
            let Some(st) = self.windows.get_mut(&target) else { return };
            st.app.dispatch_open(vec![route], mode, open.properties, now);
        }
        let Some(st) = self.windows.get_mut(&target) else { return };
        st.app.report_dropped(total);
        st.app.show_open_errors(&errors);
        self.raise = Some(target);
        (self.wake)();
    }

    fn render(&mut self, window: &Win<P>) {
        let id = window.id();
        let needs = self.windows.get(&id).is_some_and(|st| st.renderer.is_none());
        self.poll_gpu(needs);
        let Some(st) = self.windows.get_mut(&id) else { return };
        if st.renderer.is_none() || (st.software && self.graphics.hw.is_some()) {
            let (dev, software) = match (&self.graphics.hw, &self.graphics.sw) {
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
                    st.preview_rev = u64::MAX;
                    st.backdrop_rev = u64::MAX;
                    st.renderer = Some(r);
                    st.software = software;
                    st.device_gen = self.device_gen;
                }
                Err(e) => {
                    tracing::error!("renderer: {e}");
                    return;
                }
            }
            if self.graphics.hw.is_some() && self.windows.values().all(|w| !w.software) {
                self.graphics.sw = None;
            }
        }
        let Some(st) = self.windows.get_mut(&id) else { return };
        if st.dark != Some(st.app.is_dark()) {
            st.dark = Some(st.app.is_dark());
            window.set_dark(st.app.is_dark());
        }
        st.app.set_clock(filetime_now());
        let t = Instant::now();
        let animating = st.app.frame(&mut st.gfx, t);
        *st.visible.content.lock().unwrap_or_else(|e| e.into_inner()) = st.app.visible_content_icons();
        *st.visible.overlays.lock().unwrap_or_else(|e| e.into_inner()) = st.app.visible_overlays();
        let Some(renderer) = &mut st.renderer else { return };
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
        let (rev, image) = st.app.preview_image();
        if rev != st.preview_rev {
            renderer.upload_preview(image);
            st.preview_rev = rev;
        }
        let (rev, image) = st.app.backdrop_image();
        if rev != st.backdrop_rev {
            renderer.upload_backdrop(image);
            st.backdrop_rev = rev;
        }
        let lost = matches!(renderer.render(&st.gfx.dl), Err(RenderError::DeviceLost));
        if lost {
            let (software, stale) = (st.software, st.device_gen != self.device_gen);
            st.renderer = None;
            if !stale {
                self.device_gen += 1;
                if software {
                    self.graphics.sw = None;
                } else {
                    self.graphics.hw = P::Renderer::device(false).map_err(|e| tracing::error!("graphics device: {e}")).ok();
                }
                if self.graphics.hw.is_none() && self.graphics.sw.is_none() {
                    self.graphics.sw = P::Renderer::device(true).map_err(|e| tracing::error!("software device: {e}")).ok();
                }
            }
            window.request_redraw();
            return;
        }
        if let Some(b) = st.born.take() {
            let (ms, tabs) = (b.elapsed().as_secs_f64() * 1e3, st.app.tabs().len());
            SHOWN.store(true, Ordering::Release);
            tracing::info!("window ready in {ms:.1} ms with {tabs} tabs, software rendering: {}", st.software);
            if self.timing {
                eprintln!("window ready in {ms:.1} ms with {tabs} tabs, software rendering: {}", st.software);
            }
        }
        st.visible.pane.store(st.app.visible_rows());
        *st.visible.parked.lock().unwrap_or_else(|e| e.into_inner()) = st.app.parked_rows().collect();
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
        self.quit_session = None;
        self.closed_session = None;
        if std::mem::take(&mut self.background_start) {
            self.sync_tray();
        }
        self.start_gpu();
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
        window.set_title(&app.window_title());
        window.set_folder(app.current_location().to_windows().as_ref());
        let id = window.id();
        let born = self.requested.take();
        let mut gfx = Gfx::new(text, ATLAS);
        for k in &self.kept.kept {
            gfx.add_image(k.key, k.image.width, k.image.height, k.image.rgba.clone());
        }
        let st = WinState { app, gfx, renderer: None, software: false, device_gen: 0, visible: Arc::default(), session_rev: 0, placement: Some(window.placement()), born, preview_rev: u64::MAX, backdrop_rev: u64::MAX, dark: None, selection_key: None, progress: None };
        self.windows.insert(id, st);
        self.active = Some(id);
        self.apply_startup_open(window);
        self.flush(window);
        self.apply_opens(window);
        self.open_next(window);
        window.request_redraw();
    }

    fn event(&mut self, window: &Win<P>, event: Event) {
        let id = window.id();
        if event == Event::Focus(true) {
            self.active = Some(id);
            self.settings.send(Job::Check);
            #[cfg(target_os = "linux")]
            self.check_tray();
        }
        if event == Event::SessionEnding && !self.ending {
            if let Some(st) = self.windows.get_mut(&id) {
                st.placement = Some(window.placement());
            }
            let s = self.quit_session.take().unwrap_or_else(|| self.session());
            self.final_session(Some(s));
            self.ending = true;
        }
        let Some(st) = self.windows.get_mut(&id) else { return };
        if let Event::Resized { width, height } = event {
            if let Some(r) = &mut st.renderer {
                r.resize(width, height);
            }
            st.app.set_maximized(window.is_maximized());
            st.placement = Some(window.placement());
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
        if self.active == Some(window.id()) {
            self.tray_actions(Some(window));
        }
        if let Some(st) = self.windows.get_mut(&window.id()) {
            st.app.set_tray_available(tray_controls_available(self.tray.as_ref()));
        }
        if self.raise == Some(window.id()) {
            self.raise = None;
            window.activate();
        }
        self.drain();
        self.update_replies();
        if self.restarting {
            return window.close();
        }
        self.settings_replies(window);
        self.inject_due(window);
        self.poll_gpu(false);
        if self.graphics.hw.is_some() && self.windows.get(&window.id()).is_some_and(|st| st.software) {
            window.request_redraw();
        }
        self.apply_opens(window);
        if let Some(st) = self.windows.get_mut(&window.id()) {
            st.app.poll();
            if self.quitting.remove(&window.id()) {
                st.app.close_window(Instant::now());
                window.request_redraw();
            }
        }
        self.flush(window);
    }

    fn drag(&mut self, window: &Win<P>, event: DragEvent) -> DropReply {
        if self.picker_result.is_some() {
            return DropReply::default();
        }
        let Some(st) = self.windows.get_mut(&window.id()) else { return DropReply::default() };
        let reply = DropReply { effect: st.app.drag(&event, Instant::now()), caption: st.app.drop_caption(), folder: st.app.drop_folder(), onto: st.app.drop_onto() };
        self.flush(window);
        reply
    }

    fn accessibility(&mut self, window: &Win<P>) -> Option<flit_core::a11y::Tree> {
        self.windows.get_mut(&window.id()).map(|st| st.app.accessibility())
    }

    fn destroyed(&mut self, window: &Win<P>) {
        let id = window.id();
        let mut last = None;
        if let Some(mut st) = self.windows.remove(&id) {
            if self.windows.is_empty() {
                self.saved = Saved::new(st.app.persisted(), Some(window.placement())).with_nav(st.app.nav_state()).with_pane(st.app.pane_state()).with_side(st.app.nav_prefs()).with_tools(tools(&st.app));
                last = Some(Session::new(vec![(st.app.session(), Some(window.placement()))], self.typed.lock().unwrap_or_else(|e| e.into_inner()).clone()));
            }
            st.visible.closed.store(true, Ordering::Relaxed);
            st.app.shutdown();
            for e in st.app.drain_effects().collect::<Vec<_>>() {
                if let Effect::Provider { provider, req } = e {
                    self.submit(provider, req);
                }
            }
        }
        self.search.forget(id);
        self.stay_awake();
        if self.active == Some(id) {
            self.active = self.windows.keys().next().copied();
        }
        if self.windows.is_empty() {
            self.graphics.release();
            self.closed_session = last.filter(|_| !self.ending);
            if self.background_requested() && self.quit_session.is_none() && !self.ending {
                if let Some((writer, snapshot)) = self.writer.as_ref().zip(self.closed_session.as_ref()) {
                    writer.send(snapshot.clone());
                }
                if let Some(path) = state::file::<P>() {
                    self.settings.send(Job::SaveState(path, Box::new(self.saved.clone())));
                }
                if let Some(cache) = self.thumb_cache.clone() {
                    let _ = spawn("flit-release-thumbs", move || locked(&cache).release());
                }
            }
        } else if let Some(w) = self.writer.as_ref().filter(|_| !self.ending && self.quit_session.is_none()) {
            w.send(self.session());
        }
    }

    fn idle(&mut self) -> flit_platform::Idle {
        use flit_core::ui::background::{self, Idle};
        self.drain();
        self.update_replies();
        self.tray_actions(None);
        self.take_opens();
        let available = self.tray.as_ref().and_then(|t| t.available());
        self.tray_seen |= available == Some(true);
        let tray = if self.background_start && !self.tray_seen { Some(true) } else { self.tray.as_ref().map_or(Some(false), |t| t.available()) };
        match background::idle(self.background_requested(), tray, self.ending || self.quit_session.is_some(), !self.queued_opens.is_empty()) {
            Idle::Exit if self.finish_idle() => flit_platform::Idle::Exit,
            Idle::Wait => flit_platform::Idle::Wait,
            Idle::Open | Idle::Exit => {
                let mut open = self.queued_opens.remove(0);
                let last_window = self.closed_session.as_ref().filter(|_| open.target == OpenTarget::Default && open.is_empty() && matches!(self.saved.persisted.startup, Startup::Continue)).and_then(|s| s.windows().next());
                let (app, placement) = if let Some(restore) = self.background_startup.take() {
                    if let Some(files) = &self.files {
                        files.mark();
                    }
                    self.startup_app(&open, restore)
                } else if let Some((window, placement)) = last_window {
                    (self.restored_app(&window), placement)
                } else {
                    let mut app = self.new_app(Vec::new(), self.saved.persisted.clone());
                    if !open.is_empty() {
                        open.target = OpenTarget::CurrentTab;
                        app.open(&open, &P::known_folder, Instant::now());
                    }
                    (app, self.saved.placement())
                };
                let opts = WindowOptions { title: app.window_title(), placement, dark: app.is_dark(), server_decorations: app.system_title_wanted() };
                self.requested = Some(Instant::now());
                self.pending.push_back(app);
                flit_platform::Idle::Open(opts)
            }
        }
    }

    fn session_ending_without_windows(&mut self) {
        self.ending = true;
        self.finish_idle();
    }

    fn quit(&mut self) {
        if let Some(st) = self.active.and_then(|id| self.windows.get_mut(&id)) {
            st.app.quit_app(Instant::now());
        } else {
            self.ending = true;
        }
    }
}

use flit_core::input::HitArea;

fn filetime_now() -> i64 {
    let unix = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    (unix + 11_644_473_600) * 10_000_000
}

static SHOWN: AtomicBool = AtomicBool::new(false);

pub fn shown() -> bool {
    SHOWN.load(Ordering::Acquire)
}

type Gpu<D> = (bool, Result<D, PlatformError>);

struct Graphics<D> {
    replies: Receiver<Gpu<D>>,
    hw: Option<D>,
    sw: Option<D>,
    pending: usize,
}

impl<D> Default for Graphics<D> {
    fn default() -> Self {
        Self { replies: crossbeam_channel::never(), hw: None, sw: None, pending: 0 }
    }
}

impl<D> Graphics<D> {
    fn release(&mut self) {
        *self = Self::default();
    }
}

fn start_gpu<P: WindowPlatform>(wake: Wake) -> Result<Receiver<Gpu<Device<P>>>, PlatformError> {
    let (tx, rx) = unbounded();
    spawn("flit-gpu", move || {
        for software in [true, false] {
            if tx.send((software, P::Renderer::device(software))).is_err() {
                break;
            }
            wake();
        }
    })?;
    Ok(rx)
}

fn open_inbox<I: SingleInstance>(instance: Option<I>, wake: Wake) -> (OpenSink, Receiver<Open>) {
    let (tx, rx) = unbounded();
    let sink = Arc::new(Mutex::new(Some(tx)));
    if let Some(instance) = instance {
        instance.listen(sink.clone(), wake);
    }
    (sink, rx)
}

pub fn run<P: Platform>(open: Open, instance: Option<P::Instance>, metrics: Option<PathBuf>, activated: bool, background: bool, picker: Option<(flit_core::pick::Request, crate::picker::ResultSlot)>, started: Instant) -> Result<(), PlatformError> {
    flit_core::ui::set_unix_defaults(P::caps().unix_paths);
    let mut saved = state::load::<P>();
    let picking = picker.is_some();
    let settings_file = settings::file::<P>();
    let settings_text = settings_file.as_deref().and_then(settings::read);
    let doc = settings_text.as_deref().map(flit_core::ui::settings::parse).unwrap_or_default();
    if let Some(Value::Choice(i)) = doc.value(flit_core::ui::settings::LANGUAGE) {
        flit_core::i18n::set_lang(flit_core::ui::settings::language_of(*i));
    }
    if let Some(startup) = doc.startup() {
        saved.persisted.startup = startup;
    }
    if doc.value(flit_core::ui::settings::CHECKBOXES).is_none() {
        saved.persisted.prefs.checkboxes = true;
    }
    if let Some(mb) = settings::thumb_cache_mb(&doc) {
        saved.persisted.prefs.thumb_cache_mb = mb;
    }
    let ws = Ws::<P>::new()?;
    let waker = ws.waker();
    let wake: Wake = Arc::new(move || waker.wake());
    let mut graphics = Graphics::default();
    if !background {
        graphics.replies = start_gpu::<P>(wake.clone())?;
        graphics.pending = 2;
    }
    let (events_tx, events) = unbounded();
    let (recovery_events, recovery_wake) = (events_tx.clone(), wake.clone());
    flit_platform::pending::set_recovery_reporter(move |error| {
        let _ = recovery_events.send(ProviderEvent::OpDone { tab: 0, id: 0, error: Some(error.into()), report: None });
        recovery_wake();
    });
    let local = LocalProvider::new::<_, P::Watcher, P::Shell>(P::fs(), FS_THREADS, events_tx.clone(), wake.clone())?;
    #[cfg(target_os = "linux")]
    let local = if picker.as_ref().is_some_and(|(request, _)| !request.directories() && request.filters.iter().any(|filter| !filter.mime_types.is_empty())) { local.with_picker_mime(flit_platform_linux_gui::file_chooser::picker_mime) } else { local };
    let local = Arc::new(local);
    let (sink, opens) = open_inbox(instance, wake.clone());
    let thumb_cache: Cache = P::cache_dir().filter(|_| !picking).map(|d| Arc::new(Mutex::new(ThumbCache::new(d.join("thumbs"), saved.persisted.prefs.thumb_cache_mb))));
    let kept = KeptIcons::load(P::cache_dir().filter(|_| !picking).map(|d| d.join(ICONS_FILE)));
    let refresh = kept.kept.iter().map(KeptIcon::request).collect();
    let (shell, thumbs, sorter, previews, overlays, done) = start_workers::<P>(wake.clone(), thumb_cache.clone(), refresh)?;
    let tags = TagWorker::start::<P>(local.tags().clone(), wake.clone())?;
    let fs_path = |p: PathBuf| FsPath::parse(&p.to_string_lossy());
    let pseudo = flit_core::i18n::lang() == flit_core::i18n::Lang::Pseudo;
    let headless = picking || metrics.is_some() || pseudo || flit_platform::isolated_gui_test();
    let capture = std::env::var_os("FLIT_HIDDEN_CAPTURE").is_some();
    let integrate = !headless && !capture;
    let about = About { file: settings_file.clone().and_then(fs_path), logs: P::state_dir().map(|d| d.join("logs")).and_then(fs_path), store_managed: update::store_managed(), system_integration_unavailable: !integrate, ..Default::default() };
    let settings = Settings::start::<P>(settings_file.filter(|_| !picking && metrics.is_none() && !pseudo), settings_text, thumb_cache.clone(), integrate, wake.clone());
    let search = SearchClient::start::<P>(wake.clone(), P::utc_offset_minutes(), local.tags().clone())?;
    let open = if activated { opens.recv_timeout(ACTIVATION_WAIT).unwrap_or(open) } else { open };
    let unix = P::caps().unix_paths;
    let (servers, plain): (Vec<_>, Vec<_>) = open.paths.iter().cloned().partition(|p| unix && std::str::from_utf8(p).ok().and_then(flit_core::ui::server_uri).is_some());
    let mut queued: Vec<Open> = (!servers.is_empty()).then(|| Open { paths: servers, ..Default::default() }).into_iter().collect();
    let open = Open { paths: plain, ..open };
    let open = if crate::system_link::handles(&open) {
        queued.push(open);
        Open::default()
    } else {
        open
    };
    let files = if integrate { Files::of::<P>() } else { None };
    let crashed = files.as_ref().is_some_and(Files::crashed);
    let dump = files.as_ref().filter(|_| crashed).and_then(Files::marked_at).zip(P::state_dir()).and_then(|(t, d)| crate::logs::crash_report(&d.join("logs"), t)).and_then(fs_path);
    let previous = files.as_ref().and_then(|f| session::load(&f.session));
    let settled = files.as_ref().is_some_and(Files::settled_before_exit);
    if let Some(f) = files.as_ref().filter(|_| !background) {
        f.mark();
    }
    let typed: Typed = Arc::new(Mutex::new(previous.as_ref().map(|s| s.typed.clone()).unwrap_or_default()));
    let restore_crash = session::restore_crash(&saved.persisted.startup, unix, settled);
    let crashed_session = previous.as_ref().filter(|_| crashed && !restore_crash).cloned();
    let restore = StartupRestore { previous, allowed: !picking && (!crashed || restore_crash), dump };
    let h_wake = wake.clone();
    let update_wake = wake.clone();
    let (pick_request, picker_result) = picker.map_or((None, None), |(request, result)| (Some(request), Some(result)));
    let tray = integrate.then(|| flit_platform::tray::Tray::new(wake.clone()));
    let mut h = Handler::<P> {
        windows: HashMap::new(),
        pending: VecDeque::new(),
        next_base: 0,
        active: None,
        local,
        wsl: HashMap::new(),
        admin: None,
        distros: None,
        events_tx,
        events,
        opens,
        sink,
        queued_opens: queued,
        shell,
        overlays,
        thumbs,
        thumb_cache,
        sorter,
        previews,
        done,
        saved: saved.clone(),
        command_usage: Arc::new(Mutex::new(saved.tools.commands.clone().sanitized())),
        metrics: metrics.map(|m| Metrics::new(m, started, P::refresh_period())),
        places: PlacesWorker::new::<P>(wake.clone(), P::cache_dir().map(|d| d.join(PLACES_FILE))),
        quick: crate::quick_sync::QuickSyncWorker::new::<P>(integrate, wake.clone()),
        kept,
        injection: None,
        wake,
        graphics,
        device_gen: 0,
        search,
        writer: files.as_ref().map(|f| Writer::start(f.session.clone())),
        files,
        crashed: crashed_session,
        typed,
        queued_windows: VecDeque::new(),
        startup_open: None,
        raise: None,
        ending: false,
        timing: std::env::var_os("FLIT_TIMING").is_some(),
        requested: Some(started),
        tags,
        git: crate::git::GitWorker::start(h_wake)?,
        settings,
        dates: ("yyyy-MM-dd".into(), "HH:mm".into()),
        about,
        updates: UpdateWorker::start::<P>(update_wake, headless)?,
        update_window: None,
        restarting: false,
        awake: KeepAwake::default(),
        quitting: Default::default(),
        quit_session: None,
        closed_session: None,
        tray: tray.clone(),
        background_start: background,
        tray_seen: false,
        background_startup: None,
        picker_result,
        jumps: JumpList::start::<P>(integrate),
    };
    h.jumps.show(jumps::lists(&saved.nav, &saved.side));
    let (mut first, placement) = if background {
        h.background_startup = Some(restore);
        (App::new(saved.persisted.clone(), Vec::new(), 0, false, 1.0, (1100, 720), started), None)
    } else {
        h.startup_app(&open, restore)
    };
    first.apply_doc(&doc);
    if let Some(request) = pick_request {
        first.start_picker(request);
    }
    let values = first.setting_values();
    h.settings.adopt(&doc, values);
    h.follow_quick_sync();
    h.sync_tray();
    #[cfg(target_os = "linux")]
    h.check_tray();
    if !picking && first.checks_updates_on_start() && !update::restarted() {
        h.updates.submit(UpdateRequest::Check { manual: false });
    }
    let dark = first.is_dark();
    let server_decorations = first.system_title_wanted();
    let title = first.window_title();
    if !background {
        h.pending.push_back(first);
    }
    ws.run((!background).then_some(WindowOptions { title, placement, dark, server_decorations }), tray, h)
}

#[cfg(test)]
#[path = "../test/icon_visibility.rs"]
mod icon_visibility_tests;

#[cfg(test)]
#[path = "../test/bookmarks.rs"]
mod bookmark_tests;

#[cfg(test)]
#[path = "../test/background.rs"]
mod background_tests;
