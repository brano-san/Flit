use crate::take_pwstr;
use crossbeam_channel::{bounded, unbounded, Receiver, Sender};
use flit_core::open::{Open, OpenTarget};
use flit_core::FsPath;
use flit_platform::{OpenSink, Wake};
use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Duration;
use windows::core::{BSTR, GUID, HRESULT, PCWSTR};
use windows::Win32::Foundation::{DISP_E_BADINDEX, E_FAIL, E_INVALIDARG, E_NOINTERFACE, E_NOTIMPL, HWND, LPARAM, LRESULT, POINT, RECT, SHANDLE_PTR, S_FALSE, S_OK, VARIANT_BOOL, VARIANT_FALSE, VARIANT_TRUE, WPARAM};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, IDispatch, IDispatch_Impl, IServiceProvider, IServiceProvider_Impl, IStream, ITypeInfo, CLSCTX_ALL, COINIT_MULTITHREADED, DISPATCH_FLAGS, DISPPARAMS, EXCEPINFO};
use windows::Win32::System::Ole::{DispGetIDsOfNames, DispInvoke, IEnumVARIANT, IEnumVARIANT_Impl, IOleWindow_Impl, LoadRegTypeLib, OLECMDEXECOPT, OLECMDF, OLECMDID, OLEMENUGROUPWIDTHS, READYSTATE, READYSTATE_COMPLETE};
use windows::Win32::System::Variant::{InitVariantFromBuffer, VARIANT};
use windows::Win32::UI::Controls::{LPFNSVADDPROPSHEETPAGE, TBBUTTON};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    Folder, FolderItem, FolderItems, FolderItems_Impl, IShellDispatch, IShellFolderViewDual, IShellFolderViewDual_Impl, Shell, ILClone, ILCombine, ILCreateFromPathW, ILFindLastID, ILFree, ILGetSize, IFolderView, IFolderView_Impl, IShellBrowser, IShellBrowser_Impl, IShellItem, IShellItemArray, IShellView, IShellView_Impl, IShellWindows, IWebBrowser2, IWebBrowser2_Impl, IWebBrowserApp_Impl,
    IWebBrowser_Impl, SHCreateItemFromIDList, SHCreateShellItemArrayFromIDLists, SHGetNameFromIDList, ShellWindows, BHID_DataObject, BHID_SFObject, FOLDERSETTINGS, FVM_DETAILS, SIGDN, SIGDN_FILESYSPATH, SIGDN_NORMALDISPLAY, SIGDN_URL, SVGIO_ALLVIEW, SVGIO_SELECTION,
    SVGIO_TYPE_MASK, SVSI_SELECT, SWC_BROWSER, _SVGIO,
};
use windows::Win32::UI::WindowsAndMessaging::{GetWindowRect, GetWindowThreadProcessId, HMENU, MSG};
use windows_core::{implement, ComObject, IUnknown, IUnknownImpl, Interface, Ref};

const LIBID_SHDOCVW: GUID = GUID::from_u128(0xeab22ac0_30c1_11cf_a7eb_0000c05bae0b);
const LIBID_SHELL32: GUID = GUID::from_u128(0x50a7e9b0_70ef_11d1_b75a_00a0c90564fe);

enum Msg {
    Set(isize, Option<Vec<u16>>),
    Off,
    Ack(Sender<()>),
}

const REVOKE_WAIT: Duration = Duration::from_millis(500);

#[derive(Clone, Default)]
struct Selection {
    items: Vec<Vec<u16>>,
    focused: Option<Vec<u16>>,
}

#[derive(Default)]
struct State {
    on: bool,
    folders: HashMap<isize, Vec<u16>>,
    selections: HashMap<isize, flit_platform::SelectionSource>,
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
            None => {
                s.selections.remove(&key);
                s.folders.remove(&key)
            }
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

pub fn select(hwnd: HWND, selection: &dyn Fn() -> flit_platform::SelectionSource) -> bool {
    if !with(|s| s.on) {
        return false;
    }
    let source = selection();
    with(|s| s.selections.insert(hwnd.0 as isize, source));
    true
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
    let object = ComObject::new(Browser { hwnd, folder: Mutex::new(pidl), dispatch: browser_type() });
    let thread = unsafe { GetWindowThreadProcessId(HWND(hwnd as _), None) };
    let cookie = unsafe { windows.RegisterPending(thread as i32, &location, &VARIANT::default(), SWC_BROWSER)? };
    let app: IWebBrowser2 = object.to_interface();
    if let Err(e) = unsafe { windows.Register(&app.cast::<IDispatch>()?, hwnd as i32, SWC_BROWSER) } {
        let _ = unsafe { windows.Revoke(cookie) };
        return Err(e);
    }
    registered.insert(hwnd, Registration { cookie, folder, object });
    Ok(())
}

pub(crate) fn parse(folder: &[u16]) -> Option<Vec<u8>> {
    let pidl = unsafe { ILCreateFromPathW(PCWSTR(folder.as_ptr())) };
    if pidl.is_null() {
        return None;
    }
    let bytes = unsafe { std::slice::from_raw_parts(pidl.cast::<u8>(), ILGetSize(Some(pidl)) as usize) }.to_vec();
    unsafe { ILFree(Some(pidl)) };
    Some(bytes)
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn browser_type() -> Option<ITypeInfo> {
    unsafe { LoadRegTypeLib(&LIBID_SHDOCVW, 1, 1, 0).and_then(|lib| lib.GetTypeInfoOfGuid(&IWebBrowser2::IID)) }.inspect_err(|e| tracing::info!("no browser type library: {e}")).ok()
}

fn pidls(paths: &[Vec<u16>]) -> Vec<Vec<u8>> {
    paths.iter().filter_map(|p| parse(p)).collect()
}

#[implement(IWebBrowser2, IServiceProvider, IShellBrowser, IShellView, IFolderView)]
struct Browser {
    hwnd: isize,
    folder: Mutex<Vec<u8>>,
    dispatch: Option<ITypeInfo>,
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

    fn folder_name(&self, form: SIGDN) -> windows::core::Result<BSTR> {
        let folder = lock(&self.folder);
        let name = unsafe { SHGetNameFromIDList(folder.as_ptr().cast(), form)? };
        let wide = unsafe { take_pwstr(name) }.unwrap_or_default();
        Ok(BSTR::from_wide(wide.strip_suffix(&[0]).unwrap_or(&wide)))
    }

    fn rect(&self) -> windows::core::Result<RECT> {
        let mut r = RECT::default();
        unsafe { GetWindowRect(HWND(self.hwnd as _), &mut r)? };
        Ok(r)
    }

    fn selection(&self) -> Selection {
        let Some(source) = with(|s| s.selections.get(&self.hwnd).cloned()) else { return Selection::default() };
        let (items, focused) = source();
        Selection { items: items.iter().map(|p| p.to_wide(false)).collect(), focused: focused.map(|p| p.to_wide(false)) }
    }

    fn listed(&self) -> Vec<Vec<u16>> {
        let Selection { mut items, focused } = self.selection();
        if let Some(f) = focused.filter(|f| !items.contains(f)) {
            items.push(f);
        }
        items
    }

    fn paths(&self, flags: &_SVGIO) -> windows::core::Result<Vec<Vec<u16>>> {
        match _SVGIO(flags.0 & SVGIO_TYPE_MASK.0) {
            SVGIO_SELECTION => Ok(self.selection().items),
            SVGIO_ALLVIEW => Ok(self.listed()),
            _ => unsupported(),
        }
    }

    fn items(&self, flags: &_SVGIO, riid: *const GUID, out: *mut *mut core::ffi::c_void) -> windows::core::Result<()> {
        let pidls = pidls(&self.paths(flags)?);
        if pidls.is_empty() {
            return Err(E_FAIL.into());
        }
        let list: Vec<*const ITEMIDLIST> = pidls.iter().map(|p| p.as_ptr().cast()).collect();
        let array: IShellItemArray = unsafe { SHCreateShellItemArrayFromIDLists(&list)? };
        if unsafe { array.query(riid, out) }.is_ok() {
            return Ok(());
        }
        unsafe { (Interface::vtable(&array).BindToHandler)(array.as_raw(), std::ptr::null_mut(), &BHID_DataObject, riid, out).ok() }
    }

    fn select_path(&self, path: FsPath) {
        let Some((opens, wake)) = DELIVERY.get() else { return };
        let open = Open { select: vec![path.as_bytes().to_vec()], target: OpenTarget::CurrentTab, ..Default::default() };
        if lock(opens).as_ref().is_some_and(|tx| tx.send(open).is_ok()) {
            wake();
        }
    }
}

fn unsupported<T>() -> windows::core::Result<T> {
    Err(E_NOTIMPL.into())
}

impl IFolderView_Impl for Browser_Impl {
    fn GetCurrentViewMode(&self) -> windows::core::Result<u32> {
        Ok(FVM_DETAILS.0 as u32)
    }
    fn SetCurrentViewMode(&self, _mode: u32) -> windows::core::Result<()> {
        unsupported()
    }
    fn GetFolder(&self, riid: *const GUID, out: *mut *mut core::ffi::c_void) -> windows::core::Result<()> {
        let item: IShellItem = unsafe { SHCreateItemFromIDList(lock(&self.folder).as_ptr().cast())? };
        if unsafe { item.query(riid, out) }.is_ok() {
            return Ok(());
        }
        unsafe { (Interface::vtable(&item).BindToHandler)(item.as_raw(), std::ptr::null_mut(), &BHID_SFObject, riid, out).ok() }
    }
    fn Item(&self, index: i32) -> windows::core::Result<*mut ITEMIDLIST> {
        let listed = self.listed();
        let pidl = usize::try_from(index).ok().and_then(|i| listed.get(i)).and_then(|p| parse(p)).ok_or(windows::core::Error::from(E_FAIL))?;
        Ok(unsafe { ILClone(ILFindLastID(pidl.as_ptr().cast())) })
    }
    fn ItemCount(&self, flags: &_SVGIO) -> windows::core::Result<i32> {
        Ok(self.paths(flags)?.len() as i32)
    }
    fn Items(&self, flags: &_SVGIO, riid: *const GUID, out: *mut *mut core::ffi::c_void) -> windows::core::Result<()> {
        self.items(flags, riid, out)
    }
    fn GetSelectionMarkedItem(&self) -> windows::core::Result<i32> {
        unsupported()
    }
    fn GetFocusedItem(&self) -> windows::core::Result<i32> {
        let Selection { items, focused } = self.selection();
        let focused = focused.ok_or(windows::core::Error::from(E_FAIL))?;
        Ok(items.iter().position(|i| *i == focused).unwrap_or(items.len()) as i32)
    }
    fn GetItemPosition(&self, _item: *const ITEMIDLIST) -> windows::core::Result<POINT> {
        unsupported()
    }
    fn GetSpacing(&self, _spacing: *mut POINT) -> windows::core::Result<()> {
        unsupported()
    }
    fn GetDefaultSpacing(&self) -> windows::core::Result<POINT> {
        unsupported()
    }
    fn GetAutoArrange(&self) -> windows::core::Result<()> {
        Err(S_FALSE.into())
    }
    fn SelectItem(&self, index: i32, flags: u32) -> windows::core::Result<()> {
        let listed = self.listed();
        let path = usize::try_from(index).ok().and_then(|i| listed.get(i)).and_then(|p| FsPath::from_utf16(p.strip_suffix(&[0]).unwrap_or(p)));
        if let Some(path) = path.filter(|_| flags & SVSI_SELECT.0 as u32 != 0) {
            self.select_path(path);
        }
        Ok(())
    }
    fn SelectAndPositionItems(&self, _count: u32, _items: *const *const ITEMIDLIST, _points: *const POINT, _flags: u32) -> windows::core::Result<()> {
        unsupported()
    }
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
        if let Some(path) = self.item_path(item) {
            self.select_path(path);
        }
        Ok(())
    }
    fn GetItemObject(&self, item: &_SVGIO, riid: *const GUID, out: *mut *mut core::ffi::c_void) -> windows::core::Result<()> {
        self.items(item, riid, out)
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

fn shell_type(iid: &GUID) -> Option<ITypeInfo> {
    unsafe { LoadRegTypeLib(&LIBID_SHELL32, 1, 0, 0).and_then(|lib| lib.GetTypeInfoOfGuid(iid)) }.ok()
}

fn type_info(dispatch: &Option<ITypeInfo>, index: u32) -> windows::core::Result<ITypeInfo> {
    match dispatch {
        Some(t) if index == 0 => Ok(t.clone()),
        Some(_) => Err(DISP_E_BADINDEX.into()),
        None => unsupported(),
    }
}

fn ids_of(dispatch: &Option<ITypeInfo>, names: *const PCWSTR, count: u32, ids: *mut i32) -> windows::core::Result<()> {
    let Some(t) = dispatch else { return unsupported() };
    unsafe { DispGetIDsOfNames(t, names, count, ids) }
}

#[allow(clippy::too_many_arguments)]
fn invoke(dispatch: &Option<ITypeInfo>, this: *mut core::ffi::c_void, id: i32, flags: DISPATCH_FLAGS, params: *const DISPPARAMS, result: *mut VARIANT, exception: *mut EXCEPINFO, arg: *mut u32) -> windows::core::Result<()> {
    let Some(t) = dispatch else { return unsupported() };
    unsafe { DispInvoke(this, t, id, flags.0, params.cast_mut(), result, exception, arg) }
}

impl IDispatch_Impl for Browser_Impl {
    fn GetTypeInfoCount(&self) -> windows::core::Result<u32> {
        Ok(u32::from(self.dispatch.is_some()))
    }
    fn GetTypeInfo(&self, index: u32, _lcid: u32) -> windows::core::Result<ITypeInfo> {
        type_info(&self.dispatch, index)
    }
    fn GetIDsOfNames(&self, _riid: *const GUID, names: *const PCWSTR, count: u32, _lcid: u32, ids: *mut i32) -> windows::core::Result<()> {
        ids_of(&self.dispatch, names, count, ids)
    }
    fn Invoke(&self, id: i32, _riid: *const GUID, _lcid: u32, flags: DISPATCH_FLAGS, params: *const DISPPARAMS, result: *mut VARIANT, exception: *mut EXCEPINFO, arg: *mut u32) -> windows::core::Result<()> {
        invoke(&self.dispatch, self.to_interface::<IWebBrowser2>().as_raw(), id, flags, params, result, exception, arg)
    }
}

fn shell() -> windows::core::Result<IShellDispatch> {
    unsafe { CoCreateInstance(&Shell, None, CLSCTX_ALL) }
}

fn folder_item(shell: &IShellDispatch, path: &[u16]) -> Option<FolderItem> {
    let path = std::path::PathBuf::from(String::from_utf16_lossy(path.strip_suffix(&[0]).unwrap_or(path)));
    let folder = unsafe { shell.NameSpace(&VARIANT::from(path.parent()?.to_str()?)) }.ok()?;
    unsafe { folder.ParseName(&BSTR::from(path.file_name()?.to_str()?)) }.ok()
}

#[implement(IShellFolderViewDual, IServiceProvider)]
struct View {
    owner: ComObject<Browser>,
    dispatch: Option<ITypeInfo>,
}

impl IServiceProvider_Impl for View_Impl {
    fn QueryService(&self, service: *const GUID, riid: *const GUID, out: *mut *mut core::ffi::c_void) -> windows::core::Result<()> {
        IServiceProvider_Impl::QueryService(&*self.owner, service, riid, out)
    }
}

impl IDispatch_Impl for View_Impl {
    fn GetTypeInfoCount(&self) -> windows::core::Result<u32> {
        Ok(u32::from(self.dispatch.is_some()))
    }
    fn GetTypeInfo(&self, index: u32, _lcid: u32) -> windows::core::Result<ITypeInfo> {
        type_info(&self.dispatch, index)
    }
    fn GetIDsOfNames(&self, _riid: *const GUID, names: *const PCWSTR, count: u32, _lcid: u32, ids: *mut i32) -> windows::core::Result<()> {
        ids_of(&self.dispatch, names, count, ids)
    }
    fn Invoke(&self, id: i32, _riid: *const GUID, _lcid: u32, flags: DISPATCH_FLAGS, params: *const DISPPARAMS, result: *mut VARIANT, exception: *mut EXCEPINFO, arg: *mut u32) -> windows::core::Result<()> {
        invoke(&self.dispatch, self.to_interface::<IShellFolderViewDual>().as_raw(), id, flags, params, result, exception, arg)
    }
}

impl IShellFolderViewDual_Impl for View_Impl {
    fn Application(&self) -> windows::core::Result<IDispatch> {
        shell()?.cast()
    }
    fn Parent(&self) -> windows::core::Result<IDispatch> {
        self.owner.to_interface::<IWebBrowser2>().cast()
    }
    fn Folder(&self) -> windows::core::Result<Folder> {
        let path = self.owner.folder_name(SIGDN_FILESYSPATH)?;
        unsafe { shell()?.NameSpace(&VARIANT::from(path.to_string().as_str())) }
    }
    fn SelectedItems(&self) -> windows::core::Result<FolderItems> {
        let shell = shell()?;
        let list = self.owner.selection().items.iter().filter_map(|p| folder_item(&shell, p)).collect();
        Ok(ComObject::new(Items { list, view: self.to_interface::<IShellFolderViewDual>().cast()?, dispatch: shell_type(&FolderItems::IID) }).to_interface())
    }
    fn FocusedItem(&self) -> windows::core::Result<FolderItem> {
        let focused = self.owner.selection().focused.ok_or(windows::core::Error::from(S_FALSE))?;
        folder_item(&shell()?, &focused).ok_or(S_FALSE.into())
    }
    fn SelectItem(&self, item: *const VARIANT, flags: i32) -> windows::core::Result<()> {
        let item = unsafe { item.as_ref() }.ok_or(windows::core::Error::from(E_INVALIDARG))?;
        let path = match IDispatch::try_from(item) {
            Ok(d) => unsafe { d.cast::<FolderItem>()?.Path()? },
            Err(_) => BSTR::try_from(item)?,
        };
        if let Some(path) = FsPath::parse(&path.to_string()).filter(|_| flags & SVSI_SELECT.0 != 0) {
            self.owner.select_path(path);
        }
        Ok(())
    }
    fn PopupItemMenu(&self, _item: Ref<FolderItem>, _x: &VARIANT, _y: &VARIANT) -> windows::core::Result<BSTR> {
        unsupported()
    }
    fn Script(&self) -> windows::core::Result<IDispatch> {
        unsupported()
    }
    fn ViewOptions(&self) -> windows::core::Result<i32> {
        unsupported()
    }
}

#[implement(FolderItems)]
struct Items {
    list: Vec<FolderItem>,
    view: IDispatch,
    dispatch: Option<ITypeInfo>,
}

impl IDispatch_Impl for Items_Impl {
    fn GetTypeInfoCount(&self) -> windows::core::Result<u32> {
        Ok(u32::from(self.dispatch.is_some()))
    }
    fn GetTypeInfo(&self, index: u32, _lcid: u32) -> windows::core::Result<ITypeInfo> {
        type_info(&self.dispatch, index)
    }
    fn GetIDsOfNames(&self, _riid: *const GUID, names: *const PCWSTR, count: u32, _lcid: u32, ids: *mut i32) -> windows::core::Result<()> {
        ids_of(&self.dispatch, names, count, ids)
    }
    fn Invoke(&self, id: i32, _riid: *const GUID, _lcid: u32, flags: DISPATCH_FLAGS, params: *const DISPPARAMS, result: *mut VARIANT, exception: *mut EXCEPINFO, arg: *mut u32) -> windows::core::Result<()> {
        invoke(&self.dispatch, self.to_interface::<FolderItems>().as_raw(), id, flags, params, result, exception, arg)
    }
}

impl FolderItems_Impl for Items_Impl {
    fn Count(&self) -> windows::core::Result<i32> {
        Ok(self.list.len() as i32)
    }
    fn Application(&self) -> windows::core::Result<IDispatch> {
        shell()?.cast()
    }
    fn Parent(&self) -> windows::core::Result<IDispatch> {
        Ok(self.view.clone())
    }
    fn Item(&self, index: &VARIANT) -> windows::core::Result<FolderItem> {
        let at = i32::try_from(index).ok().and_then(|i| usize::try_from(i).ok());
        at.and_then(|i| self.list.get(i)).cloned().ok_or(S_FALSE.into())
    }
    fn _NewEnum(&self) -> windows::core::Result<IUnknown> {
        Ok(ComObject::new(Walk { list: self.list.clone(), at: Mutex::new(0) }).to_interface::<IEnumVARIANT>().into())
    }
}

#[implement(IEnumVARIANT)]
struct Walk {
    list: Vec<FolderItem>,
    at: Mutex<usize>,
}

impl IEnumVARIANT_Impl for Walk_Impl {
    fn Next(&self, count: u32, out: *mut VARIANT, fetched: *mut u32) -> HRESULT {
        let mut at = lock(&self.at);
        let taken: Vec<&FolderItem> = self.list.iter().skip(*at).take(count as usize).collect();
        for (k, item) in taken.iter().enumerate() {
            let value = item.cast::<IDispatch>().map(VARIANT::from).unwrap_or_default();
            unsafe { out.add(k).write(value) };
        }
        *at += taken.len();
        if let Some(f) = unsafe { fetched.as_mut() } {
            *f = taken.len() as u32;
        }
        if taken.len() == count as usize { S_OK } else { S_FALSE }
    }
    fn Skip(&self, count: u32) -> HRESULT {
        let mut at = lock(&self.at);
        *at += count as usize;
        if *at <= self.list.len() { S_OK } else { S_FALSE }
    }
    fn Reset(&self) -> windows::core::Result<()> {
        *lock(&self.at) = 0;
        Ok(())
    }
    fn Clone(&self) -> windows::core::Result<IEnumVARIANT> {
        Ok(ComObject::new(Walk { list: self.list.clone(), at: Mutex::new(*lock(&self.at)) }).to_interface())
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
        self.to_interface::<IWebBrowser2>().cast()
    }
    fn Parent(&self) -> windows::core::Result<IDispatch> {
        self.to_interface::<IWebBrowser2>().cast()
    }
    fn Container(&self) -> windows::core::Result<IDispatch> {
        unsupported()
    }
    fn Document(&self) -> windows::core::Result<IDispatch> {
        ComObject::new(View { owner: self.to_object(), dispatch: shell_type(&IShellFolderViewDual::IID) }).to_interface::<IShellFolderViewDual>().cast()
    }
    fn TopLevelContainer(&self) -> windows::core::Result<VARIANT_BOOL> {
        Ok(VARIANT_TRUE)
    }
    fn Type(&self) -> windows::core::Result<BSTR> {
        unsupported()
    }
    fn Left(&self) -> windows::core::Result<i32> {
        Ok(self.rect()?.left)
    }
    fn SetLeft(&self, _left: i32) -> windows::core::Result<()> {
        unsupported()
    }
    fn Top(&self) -> windows::core::Result<i32> {
        Ok(self.rect()?.top)
    }
    fn SetTop(&self, _top: i32) -> windows::core::Result<()> {
        unsupported()
    }
    fn Width(&self) -> windows::core::Result<i32> {
        self.rect().map(|r| r.right - r.left)
    }
    fn SetWidth(&self, _width: i32) -> windows::core::Result<()> {
        unsupported()
    }
    fn Height(&self) -> windows::core::Result<i32> {
        self.rect().map(|r| r.bottom - r.top)
    }
    fn SetHeight(&self, _height: i32) -> windows::core::Result<()> {
        unsupported()
    }
    fn LocationName(&self) -> windows::core::Result<BSTR> {
        self.folder_name(SIGDN_NORMALDISPLAY)
    }
    fn LocationURL(&self) -> windows::core::Result<BSTR> {
        self.folder_name(SIGDN_URL)
    }
    fn Busy(&self) -> windows::core::Result<VARIANT_BOOL> {
        Ok(VARIANT_FALSE)
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
        let exe = std::env::current_exe().map_err(|_| windows::core::Error::from(E_FAIL))?;
        Ok(BSTR::from(exe.to_string_lossy().as_ref()))
    }
    fn Path(&self) -> windows::core::Result<BSTR> {
        let exe = std::env::current_exe().map_err(|_| windows::core::Error::from(E_FAIL))?;
        let dir = exe.parent().ok_or(windows::core::Error::from(E_FAIL))?;
        Ok(BSTR::from(format!("{}\\", dir.to_string_lossy().trim_end_matches('\\'))))
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

impl IWebBrowser2_Impl for Browser_Impl {
    fn Navigate2(&self, _url: *const VARIANT, _flags: *const VARIANT, _frame: *const VARIANT, _post: *const VARIANT, _headers: *const VARIANT) -> windows::core::Result<()> {
        unsupported()
    }
    fn QueryStatusWB(&self, _cmd: OLECMDID) -> windows::core::Result<OLECMDF> {
        unsupported()
    }
    fn ExecWB(&self, _cmd: OLECMDID, _option: OLECMDEXECOPT, _input: *const VARIANT, _output: *mut VARIANT) -> windows::core::Result<()> {
        unsupported()
    }
    fn ShowBrowserBar(&self, _clsid: *const VARIANT, _show: *const VARIANT, _size: *const VARIANT) -> windows::core::Result<()> {
        unsupported()
    }
    fn ReadyState(&self) -> windows::core::Result<READYSTATE> {
        Ok(READYSTATE_COMPLETE)
    }
    fn Offline(&self) -> windows::core::Result<VARIANT_BOOL> {
        Ok(VARIANT_FALSE)
    }
    fn SetOffline(&self, _value: VARIANT_BOOL) -> windows::core::Result<()> {
        unsupported()
    }
    fn Silent(&self) -> windows::core::Result<VARIANT_BOOL> {
        Ok(VARIANT_FALSE)
    }
    fn SetSilent(&self, _value: VARIANT_BOOL) -> windows::core::Result<()> {
        unsupported()
    }
    fn RegisterAsBrowser(&self) -> windows::core::Result<VARIANT_BOOL> {
        Ok(VARIANT_TRUE)
    }
    fn SetRegisterAsBrowser(&self, _value: VARIANT_BOOL) -> windows::core::Result<()> {
        unsupported()
    }
    fn RegisterAsDropTarget(&self) -> windows::core::Result<VARIANT_BOOL> {
        Ok(VARIANT_FALSE)
    }
    fn SetRegisterAsDropTarget(&self, _value: VARIANT_BOOL) -> windows::core::Result<()> {
        unsupported()
    }
    fn TheaterMode(&self) -> windows::core::Result<VARIANT_BOOL> {
        Ok(VARIANT_FALSE)
    }
    fn SetTheaterMode(&self, _value: VARIANT_BOOL) -> windows::core::Result<()> {
        unsupported()
    }
    fn AddressBar(&self) -> windows::core::Result<VARIANT_BOOL> {
        Ok(VARIANT_TRUE)
    }
    fn SetAddressBar(&self, _value: VARIANT_BOOL) -> windows::core::Result<()> {
        unsupported()
    }
    fn Resizable(&self) -> windows::core::Result<VARIANT_BOOL> {
        Ok(VARIANT_TRUE)
    }
    fn SetResizable(&self, _value: VARIANT_BOOL) -> windows::core::Result<()> {
        unsupported()
    }
}

#[cfg(test)]
#[path = "../test/shellwin.rs"]
mod tests;
