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
    let windows_dir = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
    unsafe { app.Navigate(&BSTR::from(windows_dir.as_str()), None, None, None, None) }.unwrap();
    let open = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!((open.paths, open.target), (vec![windows_dir.clone().into_bytes()], OpenTarget::CurrentTab), "a utility steers the Flit tab");
    let new_tab = VARIANT::from(navOpenInNewTab.0);
    unsafe { app.Navigate2(&variant(&item), Some(&new_tab), None, None, None) }.unwrap();
    let open = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!((open.paths, open.target), (vec![file.to_string_lossy().into_owned().into_bytes()], OpenTarget::NewTab));
    unsafe { browser.BrowseObject(std::ptr::null(), SBSP_PARENT) }.unwrap();
    let open = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(open.paths, [dir.parent().unwrap().to_string_lossy().into_owned().into_bytes()]);
    unsafe { browser.BrowseObject(child, SBSP_RELATIVE | SBSP_NEWBROWSER) }.unwrap();
    let open = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!((open.paths, open.target), (vec![file.to_string_lossy().into_owned().into_bytes()], OpenTarget::NewWindow));
    assert!(unsafe { browser.BrowseObject(std::ptr::null(), SBSP_NAVIGATEBACK) }.is_err());
    enable(false);
    let gone = wait_for(|| find(&windows, &folder).is_none().then_some(()));
    cleanup();
    assert!(gone.is_some(), "turning the setting off takes the window off the list");
}

#[test]
fn navigation_reads_paths_id_lists_and_special_folders_and_the_window_flags() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    let windows_dir = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
    assert_eq!(navigation_target(&VARIANT::from(BSTR::from(windows_dir.as_str()))), Some(windows_dir.clone()));
    let wide: Vec<u16> = windows_dir.encode_utf16().chain([0]).collect();
    assert_eq!(navigation_target(&variant(&browser_pidl(&wide))).map(|s| s.to_lowercase()), Some(windows_dir.to_lowercase()));
    const CSIDL_DRIVES: i32 = 0x11;
    assert_eq!(navigation_target(&VARIANT::from(CSIDL_DRIVES)).as_deref(), Some("::{20D04FE0-3AEA-1069-A2D8-08002B30309D}"));
    assert_eq!(navigation_target(&VARIANT::default()), None);
    assert_eq!(navigation_window(None), OpenTarget::CurrentTab);
    assert_eq!(navigation_window(Some(&VARIANT::from(navOpenInNewWindow.0))), OpenTarget::NewWindow);
    assert_eq!(navigation_window(Some(&VARIANT::from(navOpenInBackgroundTab.0))), OpenTarget::NewTab);
}

#[test]
fn a_folder_handed_to_explorer_is_read_from_its_quoted_argument() {
    assert_eq!(explorer_place(r#""::{26EE0668-A00A-44D7-9371-BEB064C98683}""#), Some("::{26EE0668-A00A-44D7-9371-BEB064C98683}"));
    assert_eq!(explorer_place(r#" "C:\Work" "#), Some(r"C:\Work"));
    assert_eq!(explorer_place(""), None, "Explorer opened without a folder always gets a new window");
    assert_eq!(explorer_place(r#""""#), None);
    assert_eq!(explorer_place(r#""a" "b""#), None);
    assert_eq!(explorer_place(r"/n,C:\Work"), None);
}

#[test]
fn an_explorer_window_already_showing_the_place_is_reused_and_flit_windows_are_not() {
    let me = 7;
    let windows = [(1, me, "::{26EE0668-A00A-44D7-9371-BEB064C98683}".to_owned()), (2, 40, r"C:\Work".to_owned()), (3, 41, "::{26ee0668-a00a-44d7-9371-beb064c98683}".to_owned())];
    assert_eq!(reusable(&windows, me, "::{26EE0668-A00A-44D7-9371-BEB064C98683}"), Some(3), "Flit's own window listed among the shell windows is skipped");
    assert_eq!(reusable(&windows, me, r"c:\work"), Some(2));
    assert_eq!(reusable(&windows, me, r"C:\Other"), None, "a place no window shows opens a new Explorer window");
    assert_eq!(reusable(&[], me, r"C:\Work"), None);
}

#[test]
fn a_place_no_explorer_window_shows_is_left_for_a_new_window() {
    let dir = crate::test_temp().join(format!("flit-explorer-nowhere-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let shown = raise_explorer(&dir.to_string_lossy());
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(!shown);
    assert!(!raise_explorer("::{00000000-0000-0000-0000-000000000000}"), "a place the shell cannot parse is never matched");
}
