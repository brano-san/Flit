use crate::take_pwstr;
use crossbeam_channel::{bounded, unbounded, Receiver, Sender};
use flit_core::open::{Open, OpenTarget};
use flit_core::FsPath;
use flit_platform::{OpenSink, Wake};
use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Duration;
use windows::core::{BSTR, GUID, HRESULT, PCWSTR};
use windows::Win32::Foundation::{E_NOINTERFACE, E_NOTIMPL, HWND, LPARAM, LRESULT, RECT, SHANDLE_PTR, S_FALSE, VARIANT_BOOL, VARIANT_TRUE, WPARAM};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, IDispatch, IDispatch_Impl, IServiceProvider, IServiceProvider_Impl, IStream, ITypeInfo, CLSCTX_ALL, COINIT_MULTITHREADED, DISPATCH_FLAGS, DISPPARAMS, EXCEPINFO};
use windows::Win32::System::Ole::{IOleWindow_Impl, OLEMENUGROUPWIDTHS};
use windows::Win32::System::Variant::{InitVariantFromBuffer, VARIANT};
use windows::Win32::UI::Controls::{LPFNSVADDPROPSHEETPAGE, TBBUTTON};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    ILCombine, ILFree, ILGetSize, IShellBrowser, IShellBrowser_Impl, IShellView, IShellView_Impl, IShellWindows, IWebBrowserApp, IWebBrowserApp_Impl, IWebBrowser_Impl, SHGetNameFromIDList, SHParseDisplayName, ShellWindows, FOLDERSETTINGS, SIGDN_FILESYSPATH, SVSI_SELECT, SWC_BROWSER, _SVGIO,
};
use windows::Win32::UI::WindowsAndMessaging::{GetWindowThreadProcessId, HMENU, MSG};
use windows_core::{implement, ComObject, IUnknownImpl, Interface, Ref};

enum Msg {
    Set(isize, Option<Vec<u16>>),
    Off,
    Ack(Sender<()>),
}

const REVOKE_WAIT: Duration = Duration::from_millis(500);

#[derive(Default)]
struct State {
    on: bool,
    folders: HashMap<isize, Vec<u16>>,
    worker: Option<Sender<Msg>>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);
static DELIVERY: OnceLock<(OpenSink, Wake)> = OnceLock::new();

fn state() -> MutexGuard<'static, Option<State>> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    f(state().get_or_insert_with(State::default))
}

pub fn deliver_to(opens: OpenSink, wake: Wake) {
    let _ = DELIVERY.set((opens, wake));
}

pub fn enable(on: bool) {
    with(|s| {
        if s.on == on {
            return;
        }
        s.on = on;
        if !on {
            s.post(Msg::Off);
            return;
        }
        let folders: Vec<_> = s.folders.iter().map(|(h, f)| (*h, f.clone())).collect();
        for (hwnd, folder) in folders {
            s.post(Msg::Set(hwnd, Some(folder)));
        }
    })
}

pub fn show(hwnd: HWND, folder: Option<&FsPath>) {
    let key = hwnd.0 as isize;
    let folder = folder.map(|f| f.to_wide(false));
    let ack = with(|s| {
        if s.folders.get(&key) == folder.as_ref() {
            return None;
        }
        match &folder {
            Some(f) => s.folders.insert(key, f.clone()),
            None => s.folders.remove(&key),
        };
        if !s.on {
            return None;
        }
        let gone = folder.is_none();
        s.post(Msg::Set(key, folder));
        gone.then(|| {
            let (tx, rx) = bounded(1);
            s.post(Msg::Ack(tx));
            rx
        })
    });
    if let Some(rx) = ack {
        let _ = rx.recv_timeout(REVOKE_WAIT);
    }
}

impl State {
    fn post(&mut self, msg: Msg) {
        let tx = self.worker.get_or_insert_with(|| {
            let (tx, rx) = unbounded();
            let _ = std::thread::Builder::new().name("flit-shell-windows".into()).spawn(move || work(rx));
            tx
        });
        let _ = tx.send(msg);
    }
}

struct Registration {
    cookie: i32,
    folder: Vec<u16>,
    object: ComObject<Browser>,
}

fn work(rx: Receiver<Msg>) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    let mut shell: Option<IShellWindows> = None;
    let mut registered: HashMap<isize, Registration> = HashMap::new();
    for msg in rx {
        if let Msg::Ack(done) = msg {
            let _ = done.send(());
            continue;
        }
        let windows = match &shell {
            Some(w) => w.clone(),
            None => match unsafe { CoCreateInstance::<_, IShellWindows>(&ShellWindows, None, CLSCTX_ALL) } {
                Ok(w) => shell.insert(w).clone(),
                Err(e) => {
                    tracing::info!("no shell window list to join: {e}");
                    continue;
                }
            },
        };
        match msg {
            Msg::Off => {
                for (_, r) in registered.drain() {
                    let _ = unsafe { windows.Revoke(r.cookie) };
                }
            }
            Msg::Set(hwnd, folder) => {
                if let Err(e) = set(&windows, &mut registered, hwnd, folder) {
                    tracing::info!("cannot list a Flit window among the shell windows: {e}");
                }
            }
            Msg::Ack(_) => {}
        }
    }
}

fn set(windows: &IShellWindows, registered: &mut HashMap<isize, Registration>, hwnd: isize, folder: Option<Vec<u16>>) -> windows::core::Result<()> {
    let pidl = folder.as_deref().and_then(parse);
    let (Some(folder), Some(pidl)) = (folder, pidl) else {
        if let Some(r) = registered.remove(&hwnd) {
            unsafe { windows.Revoke(r.cookie)? };
        }
        return Ok(());
    };
    let location = unsafe { InitVariantFromBuffer(pidl.as_ptr().cast(), pidl.len() as u32)? };
    if let Some(r) = registered.get_mut(&hwnd) {
        if r.folder != folder {
            *lock(&r.object.folder) = pidl;
            unsafe { windows.OnNavigate(r.cookie, &location)? };
            r.folder = folder;
        }
        return Ok(());
    }
    let object = ComObject::new(Browser { hwnd, folder: Mutex::new(pidl) });
    let thread = unsafe { GetWindowThreadProcessId(HWND(hwnd as _), None) };
    let cookie = unsafe { windows.RegisterPending(thread as i32, &location, &VARIANT::default(), SWC_BROWSER)? };
    let app: IWebBrowserApp = object.to_interface();
    if let Err(e) = unsafe { windows.Register(&app.cast::<IDispatch>()?, hwnd as i32, SWC_BROWSER) } {
        let _ = unsafe { windows.Revoke(cookie) };
        return Err(e);
    }
    registered.insert(hwnd, Registration { cookie, folder, object });
    Ok(())
}

pub(crate) fn parse(folder: &[u16]) -> Option<Vec<u8>> {
    let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
    unsafe { SHParseDisplayName(PCWSTR(folder.as_ptr()), None, &mut pidl, 0, None) }.ok()?;
    let bytes = unsafe { std::slice::from_raw_parts(pidl.cast::<u8>(), ILGetSize(Some(pidl)) as usize) }.to_vec();
    unsafe { ILFree(Some(pidl)) };
    Some(bytes)
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[implement(IWebBrowserApp, IServiceProvider, IShellBrowser, IShellView)]
struct Browser {
    hwnd: isize,
    folder: Mutex<Vec<u8>>,
}

impl Browser_Impl {
    fn item_path(&self, item: *const ITEMIDLIST) -> Option<FsPath> {
        let folder = lock(&self.folder);
        let full = unsafe { ILCombine(Some(folder.as_ptr().cast()), Some(item)) };
        if full.is_null() {
            return None;
        }
        let name = unsafe { SHGetNameFromIDList(full, SIGDN_FILESYSPATH) };
        unsafe { ILFree(Some(full)) };
        unsafe { take_pwstr(name.ok()?) }.and_then(|w| FsPath::from_utf16(&w))
    }
}

fn unsupported<T>() -> windows::core::Result<T> {
    Err(E_NOTIMPL.into())
}

impl IShellView_Impl for Browser_Impl {
    fn TranslateAccelerator(&self, _msg: *const MSG) -> HRESULT {
        S_FALSE
    }
    fn EnableModeless(&self, _enable: windows_core::BOOL) -> windows::core::Result<()> {
        unsupported()
    }
    fn UIActivate(&self, _state: u32) -> windows::core::Result<()> {
        Ok(())
    }
    fn Refresh(&self) -> windows::core::Result<()> {
        Ok(())
    }
    fn CreateViewWindow(&self, _previous: Ref<IShellView>, _settings: *const FOLDERSETTINGS, _browser: Ref<IShellBrowser>, _rect: *const RECT) -> windows::core::Result<HWND> {
        unsupported()
    }
    fn DestroyViewWindow(&self) -> windows::core::Result<()> {
        unsupported()
    }
    fn GetCurrentInfo(&self) -> windows::core::Result<FOLDERSETTINGS> {
        unsupported()
    }
    fn AddPropertySheetPages(&self, _reserved: u32, _add: LPFNSVADDPROPSHEETPAGE, _lparam: LPARAM) -> windows::core::Result<()> {
        unsupported()
    }
    fn SaveViewState(&self) -> windows::core::Result<()> {
        Ok(())
    }
    fn SelectItem(&self, item: *const ITEMIDLIST, flags: u32) -> windows::core::Result<()> {
        if flags & SVSI_SELECT.0 as u32 == 0 || item.is_null() {
            return Ok(());
        }
        let (Some(path), Some((opens, wake))) = (self.item_path(item), DELIVERY.get()) else { return Ok(()) };
        let open = Open { select: vec![path.as_bytes().to_vec()], target: OpenTarget::CurrentTab, ..Default::default() };
        if lock(opens).as_ref().is_some_and(|tx| tx.send(open).is_ok()) {
            wake();
        }
        Ok(())
    }
    fn GetItemObject(&self, _item: &_SVGIO, _riid: *const GUID, _out: *mut *mut core::ffi::c_void) -> windows::core::Result<()> {
        unsupported()
    }
}

impl IShellBrowser_Impl for Browser_Impl {
    fn InsertMenusSB(&self, _menu: HMENU, _widths: *mut OLEMENUGROUPWIDTHS) -> windows::core::Result<()> {
        unsupported()
    }
    fn SetMenuSB(&self, _menu: HMENU, _resource: isize, _active: HWND) -> windows::core::Result<()> {
        unsupported()
    }
    fn RemoveMenusSB(&self, _menu: HMENU) -> windows::core::Result<()> {
        unsupported()
    }
    fn SetStatusTextSB(&self, _text: &PCWSTR) -> windows::core::Result<()> {
        Ok(())
    }
    fn EnableModelessSB(&self, _enable: windows_core::BOOL) -> windows::core::Result<()> {
        Ok(())
    }
    fn TranslateAcceleratorSB(&self, _msg: *const MSG, _id: u16) -> windows::core::Result<()> {
        Err(S_FALSE.into())
    }
    fn BrowseObject(&self, _pidl: *const ITEMIDLIST, _flags: u32) -> windows::core::Result<()> {
        unsupported()
    }
    fn GetViewStateStream(&self, _mode: u32) -> windows::core::Result<IStream> {
        unsupported()
    }
    fn GetControlWindow(&self, _id: u32) -> windows::core::Result<HWND> {
        unsupported()
    }
    fn SendControlMsg(&self, _id: u32, _msg: u32, _wparam: WPARAM, _lparam: LPARAM, _ret: *mut LRESULT) -> windows::core::Result<()> {
        unsupported()
    }
    fn QueryActiveShellView(&self) -> windows::core::Result<IShellView> {
        Ok(self.to_object().to_interface())
    }
    fn OnViewWindowActive(&self, _view: Ref<IShellView>) -> windows::core::Result<()> {
        Ok(())
    }
    fn SetToolbarItems(&self, _buttons: *const TBBUTTON, _count: u32, _flags: u32) -> windows::core::Result<()> {
        unsupported()
    }
}

impl IOleWindow_Impl for Browser_Impl {
    fn GetWindow(&self) -> windows::core::Result<HWND> {
        Ok(HWND(self.hwnd as _))
    }
    fn ContextSensitiveHelp(&self, _enter: windows_core::BOOL) -> windows::core::Result<()> {
        unsupported()
    }
}

impl IServiceProvider_Impl for Browser_Impl {
    fn QueryService(&self, _service: *const GUID, riid: *const GUID, out: *mut *mut core::ffi::c_void) -> windows::core::Result<()> {
        if riid.is_null() || out.is_null() {
            return Err(E_NOINTERFACE.into());
        }
        let me: IShellView = self.to_object().to_interface();
        unsafe { me.query(riid, out).ok() }
    }
}

impl IDispatch_Impl for Browser_Impl {
    fn GetTypeInfoCount(&self) -> windows::core::Result<u32> {
        Ok(0)
    }
    fn GetTypeInfo(&self, _index: u32, _lcid: u32) -> windows::core::Result<ITypeInfo> {
        unsupported()
    }
    fn GetIDsOfNames(&self, _riid: *const GUID, _names: *const PCWSTR, _count: u32, _lcid: u32, _ids: *mut i32) -> windows::core::Result<()> {
        unsupported()
    }
    fn Invoke(&self, _id: i32, _riid: *const GUID, _lcid: u32, _flags: DISPATCH_FLAGS, _params: *const DISPPARAMS, _result: *mut VARIANT, _exception: *mut EXCEPINFO, _arg: *mut u32) -> windows::core::Result<()> {
        unsupported()
    }
}

impl IWebBrowser_Impl for Browser_Impl {
    fn GoBack(&self) -> windows::core::Result<()> {
        unsupported()
    }
    fn GoForward(&self) -> windows::core::Result<()> {
        unsupported()
    }
    fn GoHome(&self) -> windows::core::Result<()> {
        unsupported()
    }
    fn GoSearch(&self) -> windows::core::Result<()> {
        unsupported()
    }
    fn Navigate(&self, _url: &BSTR, _flags: *const VARIANT, _frame: *const VARIANT, _post: *const VARIANT, _headers: *const VARIANT) -> windows::core::Result<()> {
        unsupported()
    }
    fn Refresh(&self) -> windows::core::Result<()> {
        Ok(())
    }
    fn Refresh2(&self, _level: *const VARIANT) -> windows::core::Result<()> {
        Ok(())
    }
    fn Stop(&self) -> windows::core::Result<()> {
        Ok(())
    }
    fn Application(&self) -> windows::core::Result<IDispatch> {
        self.Document()
    }
    fn Parent(&self) -> windows::core::Result<IDispatch> {
        self.Document()
    }
    fn Container(&self) -> windows::core::Result<IDispatch> {
        unsupported()
    }
    fn Document(&self) -> windows::core::Result<IDispatch> {
        self.to_object().to_interface::<IWebBrowserApp>().cast()
    }
    fn TopLevelContainer(&self) -> windows::core::Result<VARIANT_BOOL> {
        Ok(VARIANT_TRUE)
    }
    fn Type(&self) -> windows::core::Result<BSTR> {
        unsupported()
    }
    fn Left(&self) -> windows::core::Result<i32> {
        unsupported()
    }
    fn SetLeft(&self, _left: i32) -> windows::core::Result<()> {
        unsupported()
    }
    fn Top(&self) -> windows::core::Result<i32> {
        unsupported()
    }
    fn SetTop(&self, _top: i32) -> windows::core::Result<()> {
        unsupported()
    }
    fn Width(&self) -> windows::core::Result<i32> {
        unsupported()
    }
    fn SetWidth(&self, _width: i32) -> windows::core::Result<()> {
        unsupported()
    }
    fn Height(&self) -> windows::core::Result<i32> {
        unsupported()
    }
    fn SetHeight(&self, _height: i32) -> windows::core::Result<()> {
        unsupported()
    }
    fn LocationName(&self) -> windows::core::Result<BSTR> {
        unsupported()
    }
    fn LocationURL(&self) -> windows::core::Result<BSTR> {
        unsupported()
    }
    fn Busy(&self) -> windows::core::Result<VARIANT_BOOL> {
        Ok(VARIANT_BOOL(0))
    }
}

impl IWebBrowserApp_Impl for Browser_Impl {
    fn Quit(&self) -> windows::core::Result<()> {
        unsupported()
    }
    fn ClientToWindow(&self, _x: *mut i32, _y: *mut i32) -> windows::core::Result<()> {
        unsupported()
    }
    fn PutProperty(&self, _name: &BSTR, _value: &VARIANT) -> windows::core::Result<()> {
        unsupported()
    }
    fn GetProperty(&self, _name: &BSTR) -> windows::core::Result<VARIANT> {
        unsupported()
    }
    fn Name(&self) -> windows::core::Result<BSTR> {
        Ok(BSTR::from("Flit"))
    }
    fn HWND(&self) -> windows::core::Result<SHANDLE_PTR> {
        Ok(SHANDLE_PTR(self.hwnd))
    }
    fn FullName(&self) -> windows::core::Result<BSTR> {
        unsupported()
    }
    fn Path(&self) -> windows::core::Result<BSTR> {
        unsupported()
    }
    fn Visible(&self) -> windows::core::Result<VARIANT_BOOL> {
        Ok(VARIANT_TRUE)
    }
    fn SetVisible(&self, _value: VARIANT_BOOL) -> windows::core::Result<()> {
        Ok(())
    }
    fn StatusBar(&self) -> windows::core::Result<VARIANT_BOOL> {
        unsupported()
    }
    fn SetStatusBar(&self, _value: VARIANT_BOOL) -> windows::core::Result<()> {
        unsupported()
    }
    fn StatusText(&self) -> windows::core::Result<BSTR> {
        unsupported()
    }
    fn SetStatusText(&self, _text: &BSTR) -> windows::core::Result<()> {
        unsupported()
    }
    fn ToolBar(&self) -> windows::core::Result<i32> {
        unsupported()
    }
    fn SetToolBar(&self, _value: i32) -> windows::core::Result<()> {
        unsupported()
    }
    fn MenuBar(&self) -> windows::core::Result<VARIANT_BOOL> {
        unsupported()
    }
    fn SetMenuBar(&self, _value: VARIANT_BOOL) -> windows::core::Result<()> {
        unsupported()
    }
    fn FullScreen(&self) -> windows::core::Result<VARIANT_BOOL> {
        unsupported()
    }
    fn SetFullScreen(&self, _value: VARIANT_BOOL) -> windows::core::Result<()> {
        unsupported()
    }
}

#[cfg(test)]
#[path = "../test/shellwin.rs"]
mod tests;
