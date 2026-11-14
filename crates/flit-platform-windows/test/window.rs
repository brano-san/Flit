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
