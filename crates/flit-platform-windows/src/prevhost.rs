use flit_core::preview::{HostCmd, HostEvent, Hosted, MediaCmd};
use flit_core::render::Rect;
use flit_core::FsPath;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;
use windows::core::{w, Interface, BSTR, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{CreateSolidBrush, DeleteObject, FillRect, HDC, HGDIOBJ};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{CLSIDFromString, CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, CLSCTX_LOCAL_SERVER, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, STGM_READ, STGM_SHARE_DENY_NONE};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::PropertiesSystem::{IInitializeWithFile, IInitializeWithStream};
use windows::Win32::UI::Shell::{IInitializeWithItem, IPreviewHandler, IPreviewHandlerVisuals, IShellItem, SHCreateItemFromParsingName, SHCreateStreamOnFileEx};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows_core::implement;

pub(crate) const WM_HOSTED: u32 = WM_APP + 0x41;
const WM_JOB: u32 = WM_APP + 1;
const POLL_MS: u32 = 250;
const HOST_CLASS: PCWSTR = w!("FlitPreviewHost");
const WORKER_CLASS: PCWSTR = w!("FlitPreviewWorker");

enum Job {
    Show { parent: isize, host: isize, gen: u64, what: Hosted, path: FsPath, volume: f32, bg: u32 },
    Place { parent: isize },
    Media { parent: isize, cmd: MediaCmd },
    Close { parent: isize },
}

impl Job {
    fn parent(&self) -> isize {
        match self {
            Job::Show { parent, .. } | Job::Place { parent } | Job::Media { parent, .. } | Job::Close { parent } => *parent,
        }
    }
}

struct Session {
    host: HWND,
    gen: u64,
    handler: Option<IPreviewHandler>,
    media: Option<IMFMediaEngine>,
    shown: (f64, f64, bool),
}

static EVENTS: Mutex<Vec<(isize, HostEvent)>> = Mutex::new(Vec::new());
static QUEUE: Mutex<VecDeque<Job>> = Mutex::new(VecDeque::new());
static WORKER: OnceLock<isize> = OnceLock::new();

thread_local! {
    static HOSTS: RefCell<HashMap<isize, isize>> = RefCell::default();
    static SESSIONS: RefCell<HashMap<isize, Session>> = RefCell::default();
    static BUSY: Cell<bool> = const { Cell::new(false) };
    static MF: Cell<bool> = const { Cell::new(false) };
}

fn post(parent: isize, ev: HostEvent) {
    EVENTS.lock().unwrap_or_else(|e| e.into_inner()).push((parent, ev));
    unsafe {
        let _ = PostMessageW(Some(HWND(parent as _)), WM_HOSTED, WPARAM(0), LPARAM(0));
    }
}

pub(crate) fn take(parent: HWND) -> Vec<HostEvent> {
    let mut all = EVENTS.lock().unwrap_or_else(|e| e.into_inner());
    let (mine, rest): (Vec<_>, Vec<_>) = all.drain(..).partition(|(p, _)| *p == parent.0 as isize);
    *all = rest;
    mine.into_iter().map(|(_, e)| e).collect()
}

fn send(job: Job) {
    let hwnd = *WORKER.get_or_init(start);
    if hwnd == 0 {
        return;
    }
    QUEUE.lock().unwrap_or_else(|e| e.into_inner()).push_back(job);
    unsafe {
        let _ = PostMessageW(Some(HWND(hwnd as _)), WM_JOB, WPARAM(0), LPARAM(0));
    }
}

fn hinst() -> HINSTANCE {
    unsafe { GetModuleHandleW(None).unwrap_or_default().into() }
}

fn register(class: PCWSTR, proc: WNDPROC) {
    unsafe {
        let wc = WNDCLASSEXW { cbSize: size_of::<WNDCLASSEXW>() as u32, lpfnWndProc: proc, hInstance: hinst(), lpszClassName: class, hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(), ..Default::default() };
        RegisterClassExW(&wc);
    }
}

fn start() -> isize {
    let (tx, rx) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new().name("flit-prevhost".into()).spawn(move || unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
        register(WORKER_CLASS, Some(worker_proc));
        let hwnd = CreateWindowExW(WINDOW_EX_STYLE(0), WORKER_CLASS, PCWSTR::null(), WINDOW_STYLE(0), 0, 0, 0, 0, Some(HWND_MESSAGE), None, Some(hinst()), None);
        let _ = tx.send(hwnd.as_ref().map_or(0, |h| h.0 as isize));
        if hwnd.is_err() {
            return;
        }
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    });
    spawned.ok().and_then(|_| rx.recv().ok()).unwrap_or(0)
}

unsafe extern "system" fn host_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_ERASEBKGND {
        let brush = CreateSolidBrush(COLORREF(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as u32));
        let mut rc = RECT::default();
        let _ = GetClientRect(hwnd, &mut rc);
        FillRect(HDC(wp.0 as _), &rc, brush);
        let _ = DeleteObject(HGDIOBJ(brush.0));
        return LRESULT(1);
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}

fn host_window(parent: HWND, bg: u32) -> Option<HWND> {
    static CLASS: OnceLock<()> = OnceLock::new();
    CLASS.get_or_init(|| register(HOST_CLASS, Some(host_proc)));
    let key = parent.0 as isize;
    let hwnd = match HOSTS.with(|h| h.borrow().get(&key).copied()) {
        Some(h) => HWND(h as _),
        None => unsafe {
            let style = WS_CHILD | WS_CLIPCHILDREN | WS_CLIPSIBLINGS;
            let h = match CreateWindowExW(WS_EX_NOPARENTNOTIFY, HOST_CLASS, PCWSTR::null(), style, 0, 0, 0, 0, Some(parent), None, Some(hinst()), None) {
                Ok(h) => h,
                Err(e) => {
                    tracing::warn!("preview host window: {e}");
                    return None;
                }
            };
            HOSTS.with(|m| m.borrow_mut().insert(key, h.0 as isize));
            h
        },
    };
    unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, (bg & 0x00FF_FFFF) as isize) };
    Some(hwnd)
}

fn place(host: HWND, rect: Option<Rect>) {
    unsafe {
        match rect {
            Some(r) => {
                let _ = SetWindowPos(host, Some(HWND_TOP), r.x as i32, r.y as i32, r.w as i32, r.h as i32, SWP_NOACTIVATE | SWP_SHOWWINDOW);
            }
            None => {
                let _ = ShowWindow(host, SW_HIDE);
            }
        }
    }
}

fn existing(parent: HWND) -> Option<HWND> {
    HOSTS.with(|h| h.borrow().get(&(parent.0 as isize)).map(|&h| HWND(h as _)))
}

pub(crate) fn command(parent: HWND, cmd: HostCmd) {
    let key = parent.0 as isize;
    match cmd {
        HostCmd::Show { gen, what, path, rect, volume, bg } => match host_window(parent, bg) {
            Some(host) => {
                place(host, rect);
                send(Job::Show { parent: key, host: host.0 as isize, gen, what, path, volume, bg });
            }
            None => post(key, HostEvent::Failed { gen, message: "Cannot create the preview window".into() }),
        },
        HostCmd::Place(rect) => {
            if let Some(host) = existing(parent) {
                place(host, rect);
                if rect.is_some() {
                    send(Job::Place { parent: key });
                }
            }
        }
        HostCmd::Media(cmd) => send(Job::Media { parent: key, cmd }),
        HostCmd::Close => {
            if let Some(host) = existing(parent) {
                place(host, None);
            }
            send(Job::Close { parent: key });
        }
    }
}

pub(crate) fn forget(parent: HWND) {
    if HOSTS.with(|h| h.borrow_mut().remove(&(parent.0 as isize))).is_some() {
        send(Job::Close { parent: parent.0 as isize });
    }
    take(parent);
}

#[implement(IMFMediaEngineNotify)]
struct Notify;

impl IMFMediaEngineNotify_Impl for Notify_Impl {
    fn EventNotify(&self, _: u32, _: usize, _: u32) -> windows_core::Result<()> {
        Ok(())
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn client(host: HWND) -> RECT {
    let mut rc = RECT::default();
    unsafe {
        let _ = GetClientRect(host, &mut rc);
    }
    rc
}

pub(crate) unsafe fn open_handler(clsid: &str, path: &FsPath, host: HWND, bg: u32) -> windows::core::Result<IPreviewHandler> {
    let id = CLSIDFromString(PCWSTR(wide(clsid).as_ptr()))?;
    let handler: IPreviewHandler = CoCreateInstance(&id, None, CLSCTX_LOCAL_SERVER)?;
    let file = path.to_wide(false);
    if let Ok(init) = handler.cast::<IInitializeWithFile>() {
        init.Initialize(PCWSTR(file.as_ptr()), STGM_READ.0)?;
    } else if let Ok(init) = handler.cast::<IInitializeWithItem>() {
        let item: IShellItem = SHCreateItemFromParsingName(PCWSTR(file.as_ptr()), None)?;
        init.Initialize(&item, STGM_READ.0)?;
    } else {
        let init: IInitializeWithStream = handler.cast()?;
        let stream = SHCreateStreamOnFileEx(PCWSTR(file.as_ptr()), STGM_READ.0 | STGM_SHARE_DENY_NONE.0, 0, false, None)?;
        init.Initialize(&stream, STGM_READ.0)?;
    }
    handler.SetWindow(host, &client(host))?;
    if let Ok(visuals) = handler.cast::<IPreviewHandlerVisuals>() {
        let _ = visuals.SetBackgroundColor(COLORREF(bg & 0x00FF_FFFF));
    }
    handler.DoPreview()?;
    Ok(handler)
}

pub(crate) unsafe fn open_media(path: &FsPath, host: HWND, video: bool, volume: f32) -> windows::core::Result<IMFMediaEngine> {
    if !MF.get() {
        MFStartup(MF_VERSION, MFSTARTUP_FULL)?;
        MF.set(true);
    }
    let factory: IMFMediaEngineClassFactory = CoCreateInstance(&CLSID_MFMediaEngineClassFactory, None, CLSCTX_INPROC_SERVER)?;
    let mut attrs = None;
    MFCreateAttributes(&mut attrs, 2)?;
    let attrs = attrs.ok_or_else(windows::core::Error::empty)?;
    let notify: IMFMediaEngineNotify = Notify.into();
    attrs.SetUnknown(&MF_MEDIA_ENGINE_CALLBACK, &notify)?;
    let flags = if video {
        attrs.SetUINT64(&MF_MEDIA_ENGINE_PLAYBACK_HWND, host.0 as u64)?;
        0
    } else {
        MF_MEDIA_ENGINE_AUDIOONLY.0 as u32
    };
    let engine = factory.CreateInstance(flags, &attrs)?;
    engine.SetVolume(volume as f64)?;
    engine.SetSource(&BSTR::from(path.display().as_ref()))?;
    Ok(engine)
}

fn close(s: Session) {
    unsafe {
        if let Some(h) = s.handler {
            let _ = h.Unload();
        }
        if let Some(m) = s.media {
            let _ = m.Shutdown();
        }
    }
}

unsafe fn show(parent: isize, host: HWND, gen: u64, what: Hosted, path: &FsPath, volume: f32, bg: u32) {
    if let Some(old) = SESSIONS.with(|s| s.borrow_mut().remove(&parent)) {
        close(old);
    }
    let t = Instant::now();
    let opened = match &what {
        Hosted::Handler(clsid) => open_handler(clsid, path, host, bg).map(|h| (Some(h), None)),
        Hosted::Media { video } => open_media(path, host, *video, volume).map(|m| (None, Some(m))),
    };
    match opened {
        Ok((handler, media)) => {
            post(parent, HostEvent::Ready { gen, ms: t.elapsed().as_millis() as u32 });
            SESSIONS.with(|s| s.borrow_mut().insert(parent, Session { host, gen, handler, media, shown: (-1.0, -1.0, false) }));
        }
        Err(e) => post(parent, HostEvent::Failed { gen, message: format!("The preview could not be shown: {}", e.message()) }),
    }
}

unsafe fn media(s: &Session, cmd: MediaCmd) {
    let Some(m) = &s.media else { return };
    let _ = match cmd {
        MediaCmd::Play => {
            if m.IsEnded().as_bool() {
                let _ = m.SetCurrentTime(0.0);
            }
            m.Play()
        }
        MediaCmd::Pause => m.Pause(),
        MediaCmd::Seek(t) => m.SetCurrentTime(t),
        MediaCmd::Volume(v) => m.SetVolume(v as f64),
    };
}

unsafe fn poll(worker: HWND) {
    let mut failed = Vec::new();
    let (dead, any) = SESSIONS.with(|all| {
        let mut all = all.borrow_mut();
        for (&parent, s) in all.iter_mut() {
            let Some(m) = &s.media else { continue };
            if let Ok(e) = m.GetError() {
                failed.push((parent, s.gen, e.GetErrorCode()));
                continue;
            }
            let ended = m.IsEnded().as_bool();
            let dur = m.GetDuration();
            let dur = if dur.is_finite() { dur } else { 0.0 };
            let now = (if ended { dur } else { m.GetCurrentTime() }, dur, !ended && !m.IsPaused().as_bool());
            if now != s.shown {
                s.shown = now;
                post(parent, HostEvent::Media { gen: s.gen, position: now.0, duration: now.1, playing: now.2 });
            }
        }
        let dead: Vec<_> = failed.iter().map(|&(parent, gen, code)| (parent, gen, code, all.remove(&parent))).collect();
        (dead, all.values().any(|s| s.media.is_some()))
    });
    for (parent, gen, code, session) in dead {
        if let Some(s) = session {
            close(s);
        }
        post(parent, HostEvent::Failed { gen, message: format!("media error {code}") });
    }
    if any {
        SetTimer(Some(worker), 1, POLL_MS, None);
    } else {
        let _ = KillTimer(Some(worker), 1);
    }
}

unsafe fn process(worker: HWND) {
    if BUSY.replace(true) {
        return;
    }
    loop {
        let batch: Vec<Job> = QUEUE.lock().unwrap_or_else(|e| e.into_inner()).drain(..).collect();
        if batch.is_empty() {
            break;
        }
        let mut last: HashMap<isize, usize> = HashMap::new();
        for (i, j) in batch.iter().enumerate() {
            if matches!(j, Job::Show { .. } | Job::Close { .. }) {
                last.insert(j.parent(), i);
            }
        }
        for (i, job) in batch.into_iter().enumerate() {
            match job {
                Job::Show { parent, .. } if last.get(&parent) != Some(&i) => {}
                Job::Show { parent, host, gen, what, path, volume, bg } => show(parent, HWND(host as _), gen, what, &path, volume, bg),
                Job::Place { parent } => SESSIONS.with(|s| {
                    if let Some(s) = s.borrow().get(&parent) {
                        let rc = client(s.host);
                        if let Some(h) = &s.handler {
                            let _ = h.SetRect(&rc);
                        }
                        if let Some(ex) = s.media.as_ref().and_then(|m| m.cast::<IMFMediaEngineEx>().ok()) {
                            let _ = ex.UpdateVideoStream(None, Some(&rc), None);
                        }
                    }
                }),
                Job::Media { parent, cmd } => SESSIONS.with(|s| {
                    if let Some(s) = s.borrow().get(&parent) {
                        media(s, cmd);
                    }
                }),
                Job::Close { parent } => {
                    if let Some(s) = SESSIONS.with(|s| s.borrow_mut().remove(&parent)) {
                        close(s);
                    }
                }
            }
        }
        poll(worker);
    }
    BUSY.set(false);
}

unsafe extern "system" fn worker_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_JOB => process(hwnd),
        WM_TIMER if !BUSY.replace(true) => {
            poll(hwnd);
            BUSY.set(false);
            if !QUEUE.lock().unwrap_or_else(|e| e.into_inner()).is_empty() {
                process(hwnd);
            }
        }
        WM_TIMER => {}
        _ => return DefWindowProcW(hwnd, msg, wp, lp),
    }
    LRESULT(0)
}

#[cfg(test)]
#[path = "../test/prevhost.rs"]
mod tests;
