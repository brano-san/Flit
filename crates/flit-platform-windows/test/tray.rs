use super::*;

#[test]
fn shell_v4_context_menu_preserves_negative_screen_coordinates() {
    let coords = ((-225i16 as u16 as usize) << 16) | (-120i16 as u16 as usize);
    let event = LPARAM(((1 << 16) | WM_CONTEXTMENU) as isize);
    assert_eq!(click(WPARAM(coords), event), Some(Click::Menu(-120, -225)));
}

#[test]
fn shell_v4_keyboard_activation_opens_and_unrelated_events_are_ignored() {
    assert_eq!(click(WPARAM(0), LPARAM(((1 << 16) | NIN_KEYSELECT) as isize)), Some(Click::Activate));
    assert_eq!(click(WPARAM(0), LPARAM(((1 << 16) | WM_MOUSEMOVE) as isize)), None);
}

#[test]
fn the_folder_tab_is_at_the_top_at_standard_and_high_dpi() {
    for (width, height, scale) in [(16, 16, 1), (32, 32, 2)] {
        let (mask, pixels) = icon_pixels(false, width, height);
        let stride = width.div_ceil(16) * 2;
        assert_eq!(mask.len(), stride * height);
        assert_eq!(pixels.len(), width * height * 4);
        let alpha = |x: usize, y: usize| pixels[(y * scale * width + x * scale) * 4 + 3];
        assert_eq!(alpha(3, 3), 255);
        assert_eq!(alpha(8, 3), 0, "the tab is narrower than the folder");
        assert_eq!(alpha(8, 12), 255, "the base spans the folder");
    }
    let (mask, pixels) = icon_pixels(true, 24, 24);
    assert_eq!(mask.len(), 24 * 4, "the monochrome mask rows are word aligned");
    assert!(pixels.chunks_exact(4).any(|rgba| rgba == [36, 36, 36, 255]));
}

#[test]
fn registration_retries_only_on_timer_and_waits_for_explorer_after_timeout() {
    let start = Instant::now();
    let until = start + RETRY_WINDOW;
    let waiting = Retry::Waiting(until);
    assert!(Retry::Ready.can_try(Refresh::Normal, start));
    assert!(!waiting.can_try(Refresh::Normal, start));
    assert!(!waiting.can_try(Refresh::Theme, start));
    assert!(waiting.can_try(Refresh::Timer, start + Duration::from_secs(2)));
    assert!(!waiting.can_try(Refresh::Timer, until));
    assert!(!Retry::Exhausted.can_try(Refresh::Normal, until));
    assert!(!Retry::Exhausted.can_try(Refresh::Timer, until));
    assert!(Retry::Exhausted.can_try(Refresh::Taskbar, until));
}

#[test]
fn windows_reads_the_icon_rows_from_top_to_bottom() {
    use windows::Win32::Graphics::Gdi::{CreateCompatibleDC, DeleteDC, DeleteObject, GetDIBits, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS};

    let icon = themed_icon(false, 16, 16);
    let mut info = ICONINFO::default();
    let mut bits = BITMAPINFO { bmiHeader: BITMAPINFOHEADER { biSize: size_of::<BITMAPINFOHEADER>() as u32, biWidth: 16, biHeight: -16, biPlanes: 1, biBitCount: 32, biCompression: BI_RGB.0, ..Default::default() }, ..Default::default() };
    let mut pixels = [0u8; 16 * 16 * 4];
    let rows = unsafe {
        let read = GetIconInfo(icon, &mut info);
        let _ = DestroyIcon(icon);
        read.unwrap();
        let dc = CreateCompatibleDC(None);
        let rows = GetDIBits(dc, info.hbmColor, 0, 16, Some(pixels.as_mut_ptr().cast()), &mut bits, DIB_RGB_COLORS);
        let _ = DeleteDC(dc);
        let _ = DeleteObject(info.hbmColor.into());
        let _ = DeleteObject(info.hbmMask.into());
        rows
    };
    assert_eq!(rows, 16);
    assert_eq!(pixels[(3 * 16 + 3) * 4], 245);
    assert_eq!(pixels[(3 * 16 + 8) * 4], 0);
    assert_eq!(pixels[(12 * 16 + 8) * 4], 245);
}
