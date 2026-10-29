use super::*;
use std::sync::Arc;
use std::time::{Duration, Instant};
use windows::core::w;
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
    let folder = parse(&folder_path.to_wide(false)).unwrap();
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
    let app: IWebBrowserApp = disp.cast().unwrap();
    let doc = unsafe { app.Document() }.unwrap();
    let provider: IServiceProvider = doc.cast().unwrap();
    let browser: IShellBrowser = unsafe { provider.QueryService(&SID_STopLevelBrowser) }.unwrap();
    let view = unsafe { browser.QueryActiveShellView() }.unwrap();
    let child = unsafe { ILFindLastID(item.as_ptr().cast()) };
    unsafe { view.SelectItem(child, SVSI_SELECT.0 as u32) }.unwrap();
    let open = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(open.target, OpenTarget::CurrentTab);
    assert_eq!(open.select, [FsPath::parse(&file.to_string_lossy()).unwrap().as_bytes().to_vec()]);
    enable(false);
    let gone = wait_for(|| find(&windows, &folder).is_none().then_some(()));
    cleanup();
    assert!(gone.is_some(), "turning the setting off takes the window off the list");
}
