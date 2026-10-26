use super::*;
use flit_core::render::Rect;
use std::sync::mpsc;
use windows::core::w;
use windows::Win32::Foundation::LRESULT;
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, COINIT_MULTITHREADED};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

const WM_TEST_FOCUS: u32 = WM_APP + 20;
const WM_TEST_GROW: u32 = WM_APP + 21;

thread_local! {
    static ACCESS: std::cell::RefCell<Option<Access>> = const { std::cell::RefCell::new(None) };
}

static POSTED: Mutex<Vec<(u32, u64)>> = Mutex::new(Vec::new());

fn sample() -> Tree {
    let mut t = Tree::new("Flit test", Rect::new(0.0, 0.0, 400.0, 300.0));
    let bar = t.add(0, 2, Role::ToolBar, "Commands", Rect::new(0.0, 0.0, 400.0, 30.0));
    let copy = t.add(bar, 20, Role::Button, "Copy", Rect::new(0.0, 0.0, 60.0, 30.0));
    t.nodes[copy].help = "Copy the selection".into();
    let hidden = t.add(bar, 21, Role::CheckBox, "Hidden files", Rect::new(60.0, 0.0, 60.0, 30.0));
    t.nodes[hidden].selected = true;
    let files = t.add(0, 3, Role::List, "Files", Rect::new(0.0, 30.0, 400.0, 270.0));
    t.add(files, 30, Role::ListItem, "a.txt", Rect::new(0.0, 30.0, 400.0, 20.0));
    t.focus = Some(copy);
    t
}

fn with_access(f: impl FnOnce(&Access)) {
    ACCESS.with_borrow(|a| a.as_ref().map(f));
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_GETOBJECT if lp.0 as i32 == UiaRootObjectId => {
            let root = ACCESS.with_borrow_mut(|a| a.get_or_insert_with(|| Access::new(hwnd, sample())).root.clone());
            return UiaReturnRawElementProvider(hwnd, wp, lp, &root);
        }
        WM_A11Y_INVOKE | WM_A11Y_FOCUS | WM_A11Y_SELECT => POSTED.lock().unwrap().push((msg, wp.0 as u64)),
        WM_TEST_FOCUS => with_access(|a| {
            let el: IRawElementProviderFragment = Element { hub: a.hub.clone(), id: wp.0 as u64 }.into();
            let _ = el.SetFocus();
        }),
        WM_TEST_GROW => with_access(|a| {
            let mut t = sample();
            t.add(3, 31, Role::ListItem, "b.txt", Rect::new(0.0, 50.0, 400.0, 20.0));
            a.update(t);
        }),
        _ => {}
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

fn wait_for(want: (u32, u64)) {
    let start = std::time::Instant::now();
    while !POSTED.lock().unwrap().contains(&want) {
        assert!(start.elapsed() < std::time::Duration::from_secs(5), "{want:?} never arrived: {:?}", POSTED.lock().unwrap());
        std::thread::yield_now();
    }
}

unsafe fn names(uia: &IUIAutomation, root: &IUIAutomationElement) -> Vec<(String, UIA_CONTROLTYPE_ID, String)> {
    let all = root.FindAll(TreeScope_Descendants, &uia.CreateTrueCondition().unwrap()).unwrap();
    (0..all.Length().unwrap())
        .map(|i| {
            let e = all.GetElement(i).unwrap();
            (e.CurrentName().unwrap().to_string(), e.CurrentControlType().unwrap(), e.CurrentHelpText().unwrap().to_string())
        })
        .collect()
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
        let seen = names(&uia, &root);
        assert!(seen.contains(&("Commands".into(), UIA_ToolBarControlTypeId, String::new())), "{seen:?}");
        assert!(seen.contains(&("Copy".into(), UIA_ButtonControlTypeId, "Copy the selection".into())), "{seen:?}");
        assert!(seen.contains(&("Files".into(), UIA_ListControlTypeId, String::new())), "{seen:?}");
        assert!(seen.iter().any(|(n, t, _)| n == "Hidden files" && *t == UIA_CheckBoxControlTypeId), "{seen:?}");

        let by_name = |name: &str| root.FindFirst(TreeScope_Descendants, &uia.CreatePropertyCondition(UIA_NamePropertyId, &text(name)).unwrap()).unwrap();
        let toggle: IUIAutomationTogglePattern = by_name("Hidden files").GetCurrentPatternAs(UIA_TogglePatternId).unwrap();
        assert_eq!(toggle.CurrentToggleState().unwrap(), ToggleState_On);
        toggle.Toggle().unwrap();
        wait_for((WM_A11Y_INVOKE, 21));

        let row: IUIAutomationSelectionItemPattern = by_name("a.txt").GetCurrentPatternAs(UIA_SelectionItemPatternId).unwrap();
        row.Select().unwrap();
        wait_for((WM_A11Y_SELECT, 30));

        PostMessageW(Some(hwnd), WM_TEST_FOCUS, WPARAM(20), LPARAM(0)).unwrap();
        wait_for((WM_A11Y_FOCUS, 20));

        PostMessageW(Some(hwnd), WM_TEST_GROW, WPARAM(0), LPARAM(0)).unwrap();
        let start = std::time::Instant::now();
        while !names(&uia, &root).iter().any(|(n, ..)| n == "b.txt") {
            assert!(start.elapsed() < std::time::Duration::from_secs(5), "the new row never showed up");
            std::thread::yield_now();
        }

        let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
        let _ = PostMessageW(Some(hwnd), WM_QUIT, WPARAM(0), LPARAM(0));
    }
    ui.join().unwrap();
}
