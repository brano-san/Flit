use super::*;
use flit_core::render::Rect;
use std::sync::mpsc;
use windows::core::w;
use windows::Win32::Foundation::LRESULT;
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, COINIT_MULTITHREADED};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

thread_local! {
    static ACCESS: std::cell::RefCell<Option<Access>> = const { std::cell::RefCell::new(None) };
}

fn sample() -> Tree {
    let mut t = Tree::new("Flit test", Rect::new(0.0, 0.0, 400.0, 300.0));
    let bar = t.add(0, 2, Role::ToolBar, "Commands", Rect::new(0.0, 0.0, 400.0, 30.0));
    let copy = t.add(bar, 20, Role::Button, "Copy", Rect::new(0.0, 0.0, 60.0, 30.0));
    t.nodes[copy].help = "Copy the selection".into();
    t.add(0, 3, Role::List, "Files", Rect::new(0.0, 30.0, 400.0, 270.0));
    t.focus = Some(copy);
    t
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_GETOBJECT && lp.0 as i32 == UiaRootObjectId {
        let root = ACCESS.with_borrow_mut(|a| a.get_or_insert_with(|| Access::new(hwnd, sample())).root.clone());
        return UiaReturnRawElementProvider(hwnd, wp, lp, &root);
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}

fn hidden_window(tx: mpsc::Sender<isize>) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let hinst = GetModuleHandleW(None).unwrap();
        let class = w!("FlitUiaSmoke");
        let wc = WNDCLASSEXW { cbSize: size_of::<WNDCLASSEXW>() as u32, lpfnWndProc: Some(proc), hInstance: hinst.into(), lpszClassName: class, ..Default::default() };
        RegisterClassExW(&wc);
        let hwnd = CreateWindowExW(WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW, class, w!("Flit test"), WS_POPUP, -32000, -32000, 400, 300, None, None, Some(hinst.into()), None).unwrap();
        tx.send(hwnd.0 as isize).unwrap();
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            DispatchMessageW(&msg);
        }
    }
}

#[test]
fn a_hidden_window_exposes_names_roles_and_help_through_ui_automation() {
    let (tx, rx) = mpsc::channel();
    let ui = std::thread::spawn(move || hidden_window(tx));
    let hwnd = HWND(rx.recv().unwrap() as _);
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let uia: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).unwrap();
        let root = uia.ElementFromHandle(hwnd).unwrap();
        assert_eq!(root.CurrentName().unwrap().to_string(), "Flit test");
        let all = root.FindAll(TreeScope_Descendants, &uia.CreateTrueCondition().unwrap()).unwrap();
        let mut seen = Vec::new();
        for i in 0..all.Length().unwrap() {
            let e = all.GetElement(i).unwrap();
            seen.push((e.CurrentName().unwrap().to_string(), e.CurrentControlType().unwrap(), e.CurrentHelpText().unwrap().to_string()));
        }
        assert!(seen.contains(&("Commands".into(), UIA_ToolBarControlTypeId, String::new())), "{seen:?}");
        assert!(seen.contains(&("Copy".into(), UIA_ButtonControlTypeId, "Copy the selection".into())), "{seen:?}");
        assert!(seen.contains(&("Files".into(), UIA_ListControlTypeId, String::new())), "{seen:?}");
        let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
        let _ = PostMessageW(Some(hwnd), WM_QUIT, WPARAM(0), LPARAM(0));
    }
    ui.join().unwrap();
}