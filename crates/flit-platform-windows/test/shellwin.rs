use super::*;
use std::sync::Arc;
use std::time::{Duration, Instant};
use windows::core::w;
use windows::Win32::System::Com::DISPATCH_PROPERTYGET;
use windows::Win32::UI::Shell::{ILFindLastID, SID_STopLevelBrowser, SWFO_NEEDDISPATCH};
use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, HWND_MESSAGE, WINDOW_EX_STYLE, WINDOW_STYLE};

fn variant(pidl: &[u8]) -> VARIANT {
    unsafe { InitVariantFromBuffer(pidl.as_ptr().cast(), pidl.len() as u32) }.unwrap()
}

fn find(windows: &IShellWindows, folder: &[u8]) -> Option<(i32, IDispatch)> {
    let mut hwnd = 0i32;
    let disp = unsafe { windows.FindWindowSW(&variant(folder), &VARIANT::default(), SWC_BROWSER, &mut hwnd, SWFO_NEEDDISPATCH) }.ok()?;
    Some((hwnd, disp))
}

fn location_url(disp: &IDispatch) -> String {
    let name = w!("LocationURL");
    let mut id = 0;
    unsafe { disp.GetIDsOfNames(&GUID::zeroed(), &name, 1, 0, &mut id) }.unwrap();
    let mut result = VARIANT::default();
    unsafe { disp.Invoke(id, &GUID::zeroed(), 0, DISPATCH_PROPERTYGET, &DISPPARAMS::default(), Some(&mut result), None, None) }.unwrap();
    BSTR::try_from(&result).unwrap().to_string()
}

fn browser_pidl(path: &[u16]) -> Vec<u8> {
    let pidl = unsafe { windows::Win32::UI::Shell::ILCreateFromPathW(PCWSTR(path.as_ptr())) };
    assert!(!pidl.is_null());
    let bytes = unsafe { std::slice::from_raw_parts(pidl.cast::<u8>(), ILGetSize(Some(pidl)) as usize) }.to_vec();
    unsafe { ILFree(Some(pidl)) };
    bytes
}

fn wait_for<T>(mut f: impl FnMut() -> Option<T>) -> Option<T> {
    let until = Instant::now() + Duration::from_secs(5);
    while Instant::now() < until {
        if let Some(v) = f() {
            return Some(v);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    None
}

#[test]
fn a_registered_flit_window_receives_the_item_the_shell_asks_it_to_select() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    let Ok(windows) = (unsafe { CoCreateInstance::<_, IShellWindows>(&ShellWindows, None, CLSCTX_ALL) }) else { return };
    let dir = crate::test_temp().join(format!("flit-shellwin-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("a b.txt");
    std::fs::write(&file, b"x").unwrap();
    let folder_path = FsPath::parse(&dir.to_string_lossy()).unwrap();
    let folder = browser_pidl(&folder_path.to_wide(false));
    let wide_file: Vec<u16> = file.to_string_lossy().encode_utf16().chain([0]).collect();
    let item = parse(&wide_file).unwrap();
    let hwnd = unsafe { CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), PCWSTR::null(), WINDOW_STYLE(0), 0, 0, 0, 0, Some(HWND_MESSAGE), None, None, None) }.unwrap();
    let (tx, rx) = unbounded();
    deliver_to(Arc::new(Mutex::new(Some(tx))), Arc::new(|| {}));
    show(hwnd, Some(&folder_path));
    assert!(find(&windows, &folder).is_none(), "nothing is listed while the setting is off");
    enable(true);
    let found = wait_for(|| find(&windows, &folder));
    let cleanup = || {
        enable(false);
        show(hwnd, None);
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
        let _ = std::fs::remove_dir_all(&dir);
    };
    let Some((found_hwnd, disp)) = found else {
        cleanup();
        panic!("the window never appeared among the shell windows");
    };
    assert_eq!(found_hwnd, hwnd.0 as i32);
    let app: IWebBrowser2 = disp.cast().unwrap();
    let doc = unsafe { app.Document() }.unwrap();
    let provider: IServiceProvider = doc.cast().unwrap();
    let browser: IShellBrowser = unsafe { provider.QueryService(&SID_STopLevelBrowser) }.unwrap();
    let view = unsafe { browser.QueryActiveShellView() }.unwrap();
    let child = unsafe { ILFindLastID(item.as_ptr().cast()) };
    unsafe { view.SelectItem(child, SVSI_SELECT.0 as u32) }.unwrap();
    let open = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(open.target, OpenTarget::CurrentTab);
    assert_eq!(open.select, [FsPath::parse(&file.to_string_lossy()).unwrap().as_bytes().to_vec()]);
    assert_eq!(location_url(&disp), unsafe { app.LocationURL() }.unwrap().to_string());
    assert!(location_url(&disp).starts_with("file:///") && location_url(&disp).ends_with(&format!("flit-shellwin-{}", std::process::id())));
    let picked = [file.clone(), dir.join("c.txt")];
    std::fs::write(&picked[1], b"x").unwrap();
    let as_path = |p: &std::path::PathBuf| FsPath::parse(&p.to_string_lossy()).unwrap();
    let (paths, focused) = (picked.iter().map(as_path).collect::<Vec<_>>(), as_path(&picked[1]));
    assert!(select(hwnd, &|| {
        let (paths, focused) = (paths.clone(), focused.clone());
        Arc::new(move || (paths.clone(), Some(focused.clone())))
    }));
    let shown: IFolderView = view.cast().unwrap();
    let items: IShellItemArray = unsafe { shown.Items(SVGIO_SELECTION) }.unwrap();
    let names: Vec<String> = (0..unsafe { items.GetCount() }.unwrap()).map(|i| unsafe { items.GetItemAt(i).unwrap().GetDisplayName(SIGDN_FILESYSPATH).unwrap().to_string() }.unwrap()).collect();
    assert_eq!(names, picked.iter().map(|p| p.to_string_lossy().into_owned()).collect::<Vec<_>>());
    assert_eq!(unsafe { shown.GetFocusedItem() }.unwrap(), 1);
    let scripted: IShellFolderViewDual = doc.cast().unwrap();
    assert_eq!(unsafe { scripted.SelectedItems().unwrap().Count() }.unwrap(), 2);
    assert_eq!(unsafe { scripted.FocusedItem().unwrap().Path() }.unwrap().to_string(), picked[1].to_string_lossy());
    enable(false);
    let gone = wait_for(|| find(&windows, &folder).is_none().then_some(()));
    cleanup();
    assert!(gone.is_some(), "turning the setting off takes the window off the list");
}
