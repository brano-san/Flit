use crate::player::{lock, percent, External, Order, Progress, Report, Start};
use flit_core::preview::{Frames, HostEvent};
use flit_core::ui::Bitmap;
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::os::unix::ffi::OsStrExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub(crate) const LIBRARIES: [&str; 2] = ["libmpv.so.2", "libmpv.so.1"];
pub(crate) const MAX_FRAME: (u32, u32) = (1280, 720);
const DEFAULT_AREA: (u32, u32) = (640, 360);
const ROW_ALIGN: usize = 64;

const FORMAT_FLAG: c_int = 3;
const FORMAT_INT64: c_int = 4;
const FORMAT_DOUBLE: c_int = 5;
const EVENT_NONE: c_int = 0;
const EVENT_SHUTDOWN: c_int = 1;
const EVENT_END_FILE: c_int = 7;
const EVENT_VIDEO_RECONFIG: c_int = 17;
const EVENT_PLAYBACK_RESTART: c_int = 21;
const EVENT_PROPERTY_CHANGE: c_int = 22;
const END_EOF: c_int = 0;
const END_ERROR: c_int = 4;
const PARAM_END: c_int = 0;
const PARAM_API_TYPE: c_int = 1;
const PARAM_SW_SIZE: c_int = 17;
const PARAM_SW_FORMAT: c_int = 18;
const PARAM_SW_STRIDE: c_int = 19;
const PARAM_SW_POINTER: c_int = 20;
const UPDATE_FRAME: u64 = 1;
const OBSERVED: [(&CStr, c_int); 3] = [(c"time-pos", FORMAT_DOUBLE), (c"duration", FORMAT_DOUBLE), (c"pause", FORMAT_FLAG)];

#[repr(C)]
struct Param {
    kind: c_int,
    data: *mut c_void,
}

#[repr(C)]
struct Event {
    id: c_int,
    error: c_int,
    reply: u64,
    data: *mut c_void,
}

#[repr(C)]
struct Property {
    name: *const c_char,
    format: c_int,
    data: *mut c_void,
}

#[repr(C)]
struct EndFile {
    reason: c_int,
    error: c_int,
}

type Callback = unsafe extern "C" fn(*mut c_void);

struct Api {
    create: unsafe extern "C" fn() -> *mut c_void,
    set_option_string: unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char) -> c_int,
    initialize: unsafe extern "C" fn(*mut c_void) -> c_int,
    command: unsafe extern "C" fn(*mut c_void, *mut *const c_char) -> c_int,
    set_property: unsafe extern "C" fn(*mut c_void, *const c_char, c_int, *mut c_void) -> c_int,
    get_property: unsafe extern "C" fn(*mut c_void, *const c_char, c_int, *mut c_void) -> c_int,
    get_property_string: unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_char,
    free: unsafe extern "C" fn(*mut c_void),
    observe_property: unsafe extern "C" fn(*mut c_void, u64, *const c_char, c_int) -> c_int,
    wait_event: unsafe extern "C" fn(*mut c_void, f64) -> *mut Event,
    set_wakeup_callback: unsafe extern "C" fn(*mut c_void, Option<Callback>, *mut c_void),
    terminate_destroy: unsafe extern "C" fn(*mut c_void),
    render_create: unsafe extern "C" fn(*mut *mut c_void, *mut c_void, *mut Param) -> c_int,
    render_set_update_callback: unsafe extern "C" fn(*mut c_void, Option<Callback>, *mut c_void),
    render_update: unsafe extern "C" fn(*mut c_void) -> u64,
    render: unsafe extern "C" fn(*mut c_void, *mut Param) -> c_int,
    render_free: unsafe extern "C" fn(*mut c_void),
}

unsafe impl Send for Api {}
unsafe impl Sync for Api {}

unsafe fn cast<T: Copy>(p: *mut c_void) -> T {
    std::mem::transmute_copy(&p)
}

fn table(lib: *mut c_void) -> Option<Api> {
    let sym = |name: &CStr| {
        let p = unsafe { libc::dlsym(lib, name.as_ptr()) };
        (!p.is_null()).then_some(p)
    };
    unsafe {
        Some(Api {
            create: cast(sym(c"mpv_create")?),
            set_option_string: cast(sym(c"mpv_set_option_string")?),
            initialize: cast(sym(c"mpv_initialize")?),
            command: cast(sym(c"mpv_command")?),
            set_property: cast(sym(c"mpv_set_property")?),
            get_property: cast(sym(c"mpv_get_property")?),
            get_property_string: cast(sym(c"mpv_get_property_string")?),
            free: cast(sym(c"mpv_free")?),
            observe_property: cast(sym(c"mpv_observe_property")?),
            wait_event: cast(sym(c"mpv_wait_event")?),
            set_wakeup_callback: cast(sym(c"mpv_set_wakeup_callback")?),
            terminate_destroy: cast(sym(c"mpv_terminate_destroy")?),
            render_create: cast(sym(c"mpv_render_context_create")?),
            render_set_update_callback: cast(sym(c"mpv_render_context_set_update_callback")?),
            render_update: cast(sym(c"mpv_render_context_update")?),
            render: cast(sym(c"mpv_render_context_render")?),
            render_free: cast(sym(c"mpv_render_context_free")?),
        })
    }
}

fn open() -> Option<Api> {
    LIBRARIES.iter().find_map(|name| {
        let name = CString::new(*name).ok()?;
        let lib = unsafe { libc::dlopen(name.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
        if lib.is_null() {
            return None;
        }
        let api = table(lib);
        if api.is_none() {
            unsafe { libc::dlclose(lib) };
        }
        api
    })
}

static API: OnceLock<Option<Api>> = OnceLock::new();

fn api() -> Option<&'static Api> {
    API.get_or_init(open).as_ref()
}

pub(crate) fn known() -> Option<bool> {
    API.get().map(Option::is_some)
}

pub(crate) fn loaded() -> bool {
    known() == Some(true)
}

pub(crate) fn options(video: bool, volume: Option<f32>) -> Vec<(&'static str, String)> {
    let fixed = [("config", "no"), ("terminal", "no"), ("load-scripts", "no"), ("ytdl", "no"), ("osc", "no"), ("input-default-bindings", "no"), ("input-vo-keyboard", "no"), ("keep-open", "no"), ("hwdec", "no"), ("audio-client-name", "Flit")];
    let output: &[(&'static str, &str)] = if video { &[("vo", "libmpv"), ("pause", "yes")] } else { &[("vid", "no"), ("vo", "null")] };
    let mut all: Vec<(&'static str, String)> = fixed.iter().chain(output).map(|&(k, v)| (k, v.to_owned())).collect();
    all.extend(volume.map(|v| ("volume", percent(v).to_string())));
    all
}

pub(crate) fn frame_size(video: Option<(u32, u32)>, area: Option<(u32, u32)>) -> (u32, u32) {
    let whole = |s: &(u32, u32)| s.0 > 0 && s.1 > 0;
    let (aw, ah) = area.filter(whole).unwrap_or(DEFAULT_AREA);
    let (vw, vh) = video.filter(whole).unwrap_or((aw, ah));
    let (vw, vh) = (f64::from(vw), f64::from(vh));
    let k = (f64::from(aw) / vw).min(f64::from(ah) / vh).min(f64::from(MAX_FRAME.0) / vw).min(f64::from(MAX_FRAME.1) / vh);
    (((vw * k).round() as u32).clamp(1, MAX_FRAME.0), ((vh * k).round() as u32).clamp(1, MAX_FRAME.1))
}

pub(crate) fn stride(width: u32) -> usize {
    (width as usize * 4).next_multiple_of(ROW_ALIGN)
}

pub(crate) fn to_rgba(pixels: &[u8], (w, h): (u32, u32), stride: usize) -> Vec<u8> {
    let row = w as usize * 4;
    let mut out = Vec::with_capacity(row * h as usize);
    for line in pixels.chunks(stride).take(h as usize) {
        out.extend_from_slice(&line[..row.min(line.len())]);
    }
    out.resize(row * h as usize, 0);
    for p in out.chunks_exact_mut(4) {
        p[3] = 255;
    }
    out
}

pub(crate) trait Surface {
    fn wants_frame(&mut self) -> bool;
    fn draw(&mut self, size: (u32, u32), stride: usize, pixels: &mut [u8]) -> bool;
}

#[derive(Default)]
struct Easel {
    held: bool,
    due: bool,
    resized: bool,
    stop: bool,
    size: (u32, u32),
}

pub(crate) struct Canvas {
    easel: Mutex<Easel>,
    cv: Condvar,
}

impl Canvas {
    pub(crate) fn new(size: (u32, u32)) -> Self {
        Self { easel: Mutex::new(Easel { size, ..Easel::default() }), cv: Condvar::new() }
    }

    pub(crate) fn poke(&self) {
        lock(&self.easel).due = true;
        self.cv.notify_all();
    }

    pub(crate) fn resize(&self, size: (u32, u32)) {
        let mut e = lock(&self.easel);
        if e.size != size {
            e.size = size;
            e.resized = true;
            self.cv.notify_all();
        }
    }

    pub(crate) fn stop(&self) {
        lock(&self.easel).stop = true;
        self.cv.notify_all();
    }

    fn hold(&self, held: bool) {
        let mut e = lock(&self.easel);
        if e.held != held {
            e.held = held;
            e.resized = true;
            self.cv.notify_all();
        }
    }

    fn size(&self) -> (u32, u32) {
        lock(&self.easel).size
    }

    fn next(&self) -> Option<(bool, bool, bool)> {
        let mut e = lock(&self.easel);
        while !(e.due || e.resized || e.stop) {
            e = self.cv.wait(e).unwrap_or_else(|e| e.into_inner());
        }
        (!e.stop).then(|| (std::mem::take(&mut e.due), std::mem::take(&mut e.resized), e.held))
    }

    fn rest(&self, until: Instant) -> bool {
        let mut e = lock(&self.easel);
        loop {
            let now = Instant::now();
            if e.stop || now >= until {
                return !e.stop;
            }
            e = self.cv.wait_timeout(e, until - now).unwrap_or_else(|e| e.into_inner()).0;
        }
    }
}

#[repr(C, align(64))]
#[derive(Clone, Copy)]
struct Block([u8; ROW_ALIGN]);

#[derive(Default)]
struct Scratch(Vec<Block>);

impl Scratch {
    fn draw(&mut self, surface: &mut dyn Surface, size: (u32, u32)) -> Option<Bitmap> {
        let stride = stride(size.0);
        let len = stride * size.1 as usize;
        let blocks = len.div_ceil(ROW_ALIGN);
        if self.0.len() < blocks {
            self.0.resize(blocks, Block([0; ROW_ALIGN]));
        }
        let pixels = unsafe { std::slice::from_raw_parts_mut(self.0.as_mut_ptr().cast::<u8>(), len) };
        surface.draw(size, stride, pixels).then(|| Bitmap { width: size.0, height: size.1, rgba: to_rgba(pixels, size, stride) })
    }
}

pub(crate) fn paint(canvas: &Canvas, surface: &mut dyn Surface, gen: u64, frames: &Frames, every: Duration, report: &dyn Fn(HostEvent)) {
    let mut scratch = Scratch::default();
    let mut drawn: Option<Instant> = None;
    while let Some((due, resized, held)) = canvas.next() {
        let fresh = due && surface.wants_frame();
        if !fresh && !(resized && drawn.is_some()) {
            continue;
        }
        if drawn.is_some_and(|at| !canvas.rest(at + every)) {
            break;
        }
        let Some(frame) = scratch.draw(surface, canvas.size()) else { continue };
        drawn = Some(Instant::now());
        if !held && frames.put(gen, frame) {
            report(HostEvent::Frame(frames.clone()));
        }
    }
}

struct Core {
    api: &'static Api,
    handle: *mut c_void,
}

fn hdr_transfer(gamma: &CStr) -> bool {
    matches!(gamma.to_bytes(), b"hlg" | b"pq")
}

fn color_filter(hdr: bool) -> Option<&'static CStr> {
    hdr.then_some(c"@flit-hdr:lavfi=[\
        zscale=w='if(gte(iw,ih),min(iw,1280),-2)':h='if(gte(iw,ih),-2,min(ih,1280))':t=linear:npl=100,\
        format=gbrpf32le,zscale=p=bt709,tonemap=tonemap=hable:desat=0,\
        zscale=t=bt709:m=bt709:r=tv,format=yuv444p]:o=[threads=2]")
}

#[derive(Default)]
struct ToneMap {
    requested: bool,
    active: bool,
}

impl Drop for Core {
    fn drop(&mut self) {
        unsafe { (self.api.terminate_destroy)(self.handle) };
    }
}

impl Core {
    fn new(api: &'static Api, options: &[(&'static str, String)]) -> Option<Self> {
        let handle = unsafe { (api.create)() };
        if handle.is_null() {
            return None;
        }
        let core = Self { api, handle };
        for (key, value) in options {
            let (Ok(k), Ok(v)) = (CString::new(*key), CString::new(value.as_str())) else { continue };
            if unsafe { (api.set_option_string)(handle, k.as_ptr(), v.as_ptr()) } < 0 {
                tracing::debug!("libmpv does not take the option {key}={value}");
            }
        }
        (unsafe { (api.initialize)(handle) } >= 0).then_some(core)
    }

    fn command(&self, args: &[&CStr]) -> bool {
        let mut argv: Vec<*const c_char> = args.iter().map(|a| a.as_ptr()).chain([std::ptr::null()]).collect();
        unsafe { (self.api.command)(self.handle, argv.as_mut_ptr()) >= 0 }
    }

    fn set(&self, name: &CStr, format: c_int, data: *mut c_void) -> bool {
        unsafe { (self.api.set_property)(self.handle, name.as_ptr(), format, data) >= 0 }
    }

    fn int(&self, name: &CStr) -> Option<u32> {
        let mut v: i64 = 0;
        let ok = unsafe { (self.api.get_property)(self.handle, name.as_ptr(), FORMAT_INT64, (&raw mut v).cast()) } >= 0;
        ok.then(|| u32::try_from(v).ok()).flatten().filter(|&v| v > 0)
    }

    fn apply(&self, o: Order) -> bool {
        match o {
            Order::Pause(paused) => {
                let mut flag = c_int::from(paused);
                self.set(c"pause", FORMAT_FLAG, (&raw mut flag).cast())
            }
            Order::Seek(at) => CString::new(format!("{at}")).is_ok_and(|at| self.command(&[c"seek", &at, c"absolute"])),
            Order::Volume(level) => {
                let mut level = level;
                self.set(c"volume", FORMAT_DOUBLE, (&raw mut level).cast())
            }
        }
    }

    fn source_is_hdr(&self) -> Option<bool> {
        let value = unsafe { (self.api.get_property_string)(self.handle, c"video-params/gamma".as_ptr()) };
        if value.is_null() {
            return None;
        }
        let hdr = hdr_transfer(unsafe { CStr::from_ptr(value) });
        unsafe { (self.api.free)(value.cast()) };
        Some(hdr)
    }

    fn seekable(&self) -> bool {
        let mut flag: c_int = 0;
        let ok = unsafe { (self.api.get_property)(self.handle, c"seekable".as_ptr(), FORMAT_FLAG, (&raw mut flag).cast()) } >= 0;
        ok && flag != 0
    }

    fn configure_color(&self, mode: &mut ToneMap) -> bool {
        let Some(hdr) = self.source_is_hdr() else { return false };
        if hdr == mode.requested {
            return false;
        }
        mode.requested = hdr;
        if let Some(filter) = color_filter(hdr) {
            mode.active = self.command(&[c"vf", c"add", filter]);
            if !mode.active {
                tracing::warn!("libmpv could not enable HDR tone mapping, using its standard video output");
            }
            return mode.active;
        } else if mode.active {
            self.command(&[c"vf", c"remove", c"@flit-hdr"]);
            mode.active = false;
        }
        false
    }

    fn sw_render(&self) -> Option<Render> {
        let mut ctx = std::ptr::null_mut();
        let mut params = [Param { kind: PARAM_API_TYPE, data: c"sw".as_ptr().cast_mut().cast() }, Param { kind: PARAM_END, data: std::ptr::null_mut() }];
        let ok = unsafe { (self.api.render_create)(&mut ctx, self.handle, params.as_mut_ptr()) } >= 0;
        (ok && !ctx.is_null()).then_some(Render { api: self.api, ctx })
    }

    fn drain(&self, progress: &mut Progress, video: &mut Option<(u32, u32)>, color: &mut ToneMap, starting: bool) -> Drained {
        let mut out = Drained::default();
        loop {
            let ev = unsafe { (self.api.wait_event)(self.handle, 0.0) };
            let Some(ev) = (unsafe { ev.as_ref() }) else { break };
            match ev.id {
                EVENT_NONE => break,
                EVENT_SHUTDOWN => {
                    out.ended = true;
                    break;
                }
                EVENT_PROPERTY_CHANGE => out.changed |= unsafe { property(ev.data.cast::<Property>().as_ref(), progress) },
                EVENT_VIDEO_RECONFIG => {
                    if !starting {
                        self.configure_color(color);
                    }
                    let size = self.int(c"dwidth").zip(self.int(c"dheight"));
                    out.resized |= size != std::mem::replace(video, size);
                }
                EVENT_PLAYBACK_RESTART if starting => out.ready = !self.configure_color(color) || !self.seekable(),
                EVENT_END_FILE => {
                    let reason = unsafe { ev.data.cast::<EndFile>().as_ref() }.map(|e| e.reason);
                    if matches!(reason, Some(END_EOF | END_ERROR)) {
                        out.ended = true;
                    }
                }
                _ => {}
            }
        }
        out
    }
}

unsafe fn property(p: Option<&Property>, progress: &mut Progress) -> bool {
    let Some(p) = p.filter(|p| !p.name.is_null()) else { return false };
    let name = CStr::from_ptr(p.name).to_str().unwrap_or_default();
    let (number, flag) = match p.format {
        FORMAT_DOUBLE if !p.data.is_null() => (Some(*p.data.cast::<f64>()), None),
        FORMAT_FLAG if !p.data.is_null() => (None, Some(*p.data.cast::<c_int>() != 0)),
        _ => (None, None),
    };
    progress.change(name, number, flag)
}

#[derive(Default)]
struct Drained {
    ready: bool,
    changed: bool,
    resized: bool,
    ended: bool,
}

struct Render {
    api: &'static Api,
    ctx: *mut c_void,
}

unsafe impl Send for Render {}

impl Drop for Render {
    fn drop(&mut self) {
        unsafe { (self.api.render_free)(self.ctx) };
    }
}

impl Surface for Render {
    fn wants_frame(&mut self) -> bool {
        unsafe { (self.api.render_update)(self.ctx) & UPDATE_FRAME != 0 }
    }

    fn draw(&mut self, size: (u32, u32), stride: usize, pixels: &mut [u8]) -> bool {
        let (Ok(w), Ok(h)) = (c_int::try_from(size.0), c_int::try_from(size.1)) else { return false };
        if pixels.len() < stride * size.1 as usize {
            return false;
        }
        let mut dims = [w, h];
        let mut stride = stride;
        let mut params = [
            Param { kind: PARAM_SW_SIZE, data: dims.as_mut_ptr().cast() },
            Param { kind: PARAM_SW_FORMAT, data: c"rgb0".as_ptr().cast_mut().cast() },
            Param { kind: PARAM_SW_STRIDE, data: (&raw mut stride).cast() },
            Param { kind: PARAM_SW_POINTER, data: pixels.as_mut_ptr().cast() },
            Param { kind: PARAM_END, data: std::ptr::null_mut() },
        ];
        unsafe { (self.api.render)(self.ctx, params.as_mut_ptr()) >= 0 }
    }
}

unsafe extern "C" fn woken(data: *mut c_void) {
    if let Some(shared) = data.cast::<Shared>().cast_const().as_ref() {
        lock(&shared.inbox).events = true;
        shared.cv.notify_all();
    }
}

unsafe extern "C" fn repaint(data: *mut c_void) {
    if let Some(canvas) = data.cast::<Canvas>().cast_const().as_ref() {
        canvas.poke();
    }
}

#[derive(Default)]
struct Inbox {
    orders: Vec<Order>,
    area: Option<(u32, u32)>,
    placed: bool,
    events: bool,
    stop: bool,
    external: Option<External>,
}

struct Shared {
    inbox: Mutex<Inbox>,
    cv: Condvar,
    alive: AtomicBool,
}

pub(crate) struct Embedded {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl Embedded {
    pub(crate) fn start(s: Start, report: Report) -> Self {
        let shared = Arc::new(Shared { inbox: Mutex::new(Inbox { area: s.area, events: true, ..Inbox::default() }), cv: Condvar::new(), alive: AtomicBool::new(true) });
        let worker = shared.clone();
        let thread = std::thread::Builder::new().name("flit-mpv".into()).spawn(move || run(&s, &worker, &report));
        if let Err(e) = &thread {
            tracing::warn!("cannot start the media player: {e}");
            shared.alive.store(false, Ordering::Release);
        }
        Self { shared, thread: thread.ok() }
    }

    pub(crate) fn order(&self, o: Order) -> bool {
        let mut inbox = lock(&self.shared.inbox);
        if let Some(e) = &inbox.external {
            return e.order(o);
        }
        if inbox.stop || !self.shared.alive.load(Ordering::Acquire) {
            return false;
        }
        inbox.orders.push(o);
        self.shared.cv.notify_all();
        true
    }

    pub(crate) fn place(&self, area: (u32, u32)) {
        let mut inbox = lock(&self.shared.inbox);
        if inbox.area != Some(area) {
            inbox.area = Some(area);
            inbox.placed = true;
            self.shared.cv.notify_all();
        }
    }

    fn halt(&self) {
        let external = {
            let mut inbox = lock(&self.shared.inbox);
            inbox.stop = true;
            inbox.external.take()
        };
        self.shared.cv.notify_all();
        if let Some(e) = external {
            e.stop();
        }
    }

    pub(crate) fn stop(mut self) -> Option<JoinHandle<()>> {
        self.halt();
        self.thread.take()
    }
}

impl Drop for Embedded {
    fn drop(&mut self) {
        self.halt();
        if let Some(thread) = self.thread.take() {
            keep(thread);
        }
    }
}

static RUNNING: Mutex<Vec<JoinHandle<()>>> = Mutex::new(Vec::new());

pub(crate) fn keep(thread: JoinHandle<()>) {
    let mut running = lock(&RUNNING);
    running.retain(|t| !t.is_finished());
    running.push(thread);
}

pub(crate) fn settle(limit: Duration) -> bool {
    let until = Instant::now() + limit;
    loop {
        let left = {
            let mut running = lock(&RUNNING);
            running.retain(|t| !t.is_finished());
            running.len()
        };
        if left == 0 || Instant::now() >= until {
            return left == 0;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn fall_back(s: &Start, shared: &Shared, report: &Report) {
    shared.alive.store(false, Ordering::Release);
    if lock(&shared.inbox).stop {
        return;
    }
    let Some(external) = crate::player::external(s, report.clone()) else { return };
    let mut inbox = lock(&shared.inbox);
    if inbox.stop {
        drop(inbox);
        return external.stop();
    }
    for o in inbox.orders.drain(..) {
        external.order(o);
    }
    inbox.external = Some(external);
}

fn run(s: &Start, shared: &Arc<Shared>, report: &Report) {
    let Some(core) = api().and_then(|api| Core::new(api, &options(s.video, s.volume))) else { return fall_back(s, shared, report) };
    let render = if s.video {
        match core.sw_render() {
            Some(r) => Some(r),
            None => {
                drop(core);
                return fall_back(s, shared, report);
            }
        }
    } else {
        None
    };
    unsafe { (core.api.set_wakeup_callback)(core.handle, Some(woken), Arc::as_ptr(shared).cast_mut().cast()) };
    for (k, (name, format)) in (1u64..).zip(OBSERVED) {
        unsafe { (core.api.observe_property)(core.handle, k, name.as_ptr(), format) };
    }
    let canvas = Arc::new(Canvas::new(frame_size(None, s.area)));
    canvas.hold(s.video);
    let painter = render.and_then(|mut r| {
        unsafe { (core.api.render_set_update_callback)(r.ctx, Some(repaint), Arc::as_ptr(&canvas).cast_mut().cast()) };
        let (canvas, report, gen, every) = (canvas.clone(), report.clone(), s.gen, s.every);
        let paint = move || {
            paint(&canvas, &mut r, gen, &Frames::default(), every, &*report);
            drop(r);
        };
        std::thread::Builder::new().name("flit-mpv-render".into()).spawn(paint).map_err(|e| tracing::warn!("cannot start the video renderer: {e}")).ok()
    });
    let loaded = CString::new(s.path.as_os_str().as_bytes()).is_ok_and(|path| core.command(&[c"loadfile", &path]));
    let mut progress = Progress { playing: true, ..Progress::default() };
    let mut color = ToneMap::default();
    let mut initial_pause = s.video.then_some(false);
    let (mut sent, mut video, mut area) = ((Instant::now(), progress), None, s.area);
    if !loaded {
        progress.finish();
        report(progress.event(s.gen));
    }
    while !progress.ended {
        let (orders, placed) = {
            let mut inbox = lock(&shared.inbox);
            while !(inbox.stop || inbox.events || inbox.placed || !inbox.orders.is_empty()) {
                inbox = shared.cv.wait(inbox).unwrap_or_else(|e| e.into_inner());
            }
            if inbox.stop {
                break;
            }
            inbox.events = false;
            (std::mem::take(&mut inbox.orders), std::mem::take(&mut inbox.placed).then_some(inbox.area).flatten())
        };
        for o in orders {
            if let (Some(paused), Order::Pause(wanted)) = (&mut initial_pause, o) {
                *paused = wanted;
            } else {
                core.apply(o);
            }
        }
        let drained = core.drain(&mut progress, &mut video, &mut color, initial_pause.is_some());
        if placed.is_some() || drained.resized {
            area = placed.or(area);
            canvas.resize(frame_size(video, area));
        }
        if drained.ready {
            canvas.hold(false);
            if let Some(paused) = initial_pause.take() {
                if !paused {
                    core.apply(Order::Pause(false));
                }
                progress.playing = !paused;
            }
        }
        if drained.ended {
            progress.finish();
        }
        if progress.ended || (initial_pause.is_none() && (drained.changed || drained.ready) && progress.due(&sent)) {
            report(progress.event(s.gen));
            sent = (Instant::now(), progress);
        }
    }
    shared.alive.store(false, Ordering::Release);
    canvas.stop();
    if let Some(t) = painter {
        let _ = t.join();
    }
    drop(core);
}

#[cfg(test)]
#[path = "../test/mpv.rs"]
mod tests;

#[cfg(test)]
#[path = "../test/hdr.rs"]
mod hdr_tests;
