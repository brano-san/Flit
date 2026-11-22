use super::*;

#[test]
fn ime_cursor_positions_become_byte_offsets() {
    let units: Vec<u16> = "a日😀b".encode_utf16().collect();
    assert_eq!(preedit_from_utf16(&units, 0), Preedit::new("a日😀b".into(), Some(0)));
    assert_eq!(preedit_from_utf16(&units, 2).cursor, Some("a日".len()));
    assert_eq!(preedit_from_utf16(&units, 4).cursor, Some("a日😀".len()));
    assert_eq!(preedit_from_utf16(&units, 5).cursor, Some("a日😀b".len()));
    assert_eq!(preedit_from_utf16(&units, -1).cursor, None);
    assert_eq!(preedit_from_utf16(&units, 9).cursor, None);
    assert_eq!(preedit_from_utf16(&units, 3).cursor, None, "a cursor between surrogates is not a character boundary");
}

#[test]
fn wheel_and_double_click_changes_reread_the_mouse_settings() {
    assert!(mouse_setting(WPARAM(SPI_SETWHEELSCROLLLINES.0 as usize)));
    assert!(mouse_setting(WPARAM(SPI_SETDOUBLECLICKTIME.0 as usize)));
    assert!(!mouse_setting(WPARAM(SPI_SETHIGHCONTRAST.0 as usize)));
    assert!(!mouse_setting(WPARAM(SPI_SETCLIENTAREAANIMATION.0 as usize)));
    assert!(!mouse_setting(WPARAM(0)), "a bare broadcast keeps the values Flit already has");
    assert_eq!(crate::wheel_lines(), <crate::Windows as flit_platform::Platform>::wheel_lines());
    assert_eq!(crate::double_click_time(), <crate::Windows as flit_platform::Platform>::double_click_time());
}
#[test]
fn a_window_rect_larger_than_its_monitor_is_cut_down_to_it() {
    let r = |left, top, right, bottom| RECT { left, top, right, bottom };
    let work = r(1920, 0, 5760, 2112);
    assert_eq!(fit_rect(r(1000, -50, 9000, 2500), work), r(1920, 0, 5760, 2112), "a window spread over every monitor shrinks to the one it is on");
    assert_eq!(fit_rect(r(2000, 100, 3100, 820), work), r(2000, 100, 3100, 820), "a window that fits stays where it is");
    assert_eq!(fit_rect(r(5000, 1800, 6100, 2520), work), r(4660, 1392, 5760, 2112), "a window hanging over the edge is pulled back in whole");
    assert_eq!(fit_rect(r(0, 0, 100, 100), r(0, 0, 0, 0)), r(0, 0, 0, 0));
}

#[derive(Default)]
struct HubHandler {
    quit: bool,
    saved: bool,
}

impl WindowHandler<Win32> for HubHandler {
    fn created(&mut self, _: &Win32Window) {}
    fn event(&mut self, _: &Win32Window, _: Event) {}
    fn hit_test(&mut self, _: &Win32Window, _: f32, _: f32) -> HitArea { HitArea::Client }
    fn redraw(&mut self, _: &Win32Window) {}
    fn woken(&mut self, _: &Win32Window) {}
    fn destroyed(&mut self, _: &Win32Window) {}
    fn session_ending_without_windows(&mut self) { self.saved = true; }
    fn quit(&mut self) { self.quit = true; }
    fn drag(&mut self, _: &Win32Window, _: DragEvent) -> DropReply { DropReply::default() }
}

#[test]
fn the_hidden_hub_consumes_close_and_dispatches_graceful_quit() {
    let hub = test_hub();
    let ctx = &hub as *const Shared<HubHandler> as *const ();
    assert!(!unsafe { hub_dispatch::<HubHandler>(ctx, WM_NULL, WPARAM(0), LPARAM(0)) });
    assert!(unsafe { hub_dispatch::<HubHandler>(ctx, WM_CLOSE, WPARAM(0), LPARAM(0)) });
    assert!(hub.handler.borrow().quit);
}

#[test]
fn the_windowless_hub_saves_on_confirmed_end_session_before_returning() {
    let hub = test_hub();
    let ctx = &hub as *const Shared<HubHandler> as *const ();
    assert!(!unsafe { hub_dispatch::<HubHandler>(ctx, WM_ENDSESSION, WPARAM(0), LPARAM(0)) });
    assert!(!hub.handler.borrow().saved);
    assert!(unsafe { hub_dispatch::<HubHandler>(ctx, WM_ENDSESSION, WPARAM(1), LPARAM(0)) });
    assert!(hub.handler.borrow().saved);
}

fn test_hub() -> Shared<HubHandler> {
    Shared {
        handler: RefCell::default(), nodes: RefCell::default(), dead: RefCell::default(),
        woken: Cell::new(false), quit_requested: Cell::new(false), next_id: Cell::new(1),
        hinst: HINSTANCE::default(), hub: HWND::default(), wake_pending: Arc::default(), tray: None,
    }
}
